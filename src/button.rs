//! tactile button input handling.
//!
//! utilizes exti lines to avoid cpu polling.
//! distinguishes short and long presses with timeouts.
//! publishes:
//!   * discrete events on a channel.
//!   * live held or released state on a watch.

use defmt::*; // logging macros.
use embassy_stm32::exti::ExtiInput; // external interrupt input.
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex; // mutex safe for interrupt context.
use embassy_sync::channel::Channel; // async queue for events.
use embassy_sync::watch::Watch; // shared state for button status.
use embassy_time::{with_timeout, Duration, Timer}; // asynchronous timing utilities.

const DEBOUNCE_MS: u64 = 20; // mechanical debounce duration.
const LONG_PRESS_MS: u64 = 600; // threshold for long press detection.

#[derive(Clone, Copy, defmt::Format)] // traits for cloning and formatting.
pub enum ButtonEvent {
    ShortPress, // released prior to threshold.
    LongPress,  // held beyond threshold.
}

#[embassy_executor::task] // concurrent background task declaration.
pub async fn button_task(
    mut button: ExtiInput<'static>, // physical button pin with interrupt.
    events: &'static Channel<CriticalSectionRawMutex, ButtonEvent, 4>, // channel for event transmission.
    state: &'static Watch<CriticalSectionRawMutex, bool, 3>, // watch for state observation.
) {
    info!("Button task running."); // task initialization log.
    let state_tx = state.sender(); 
    state_tx.send(false); // initialize state as released.

    loop {
        // suspend until active-low falling edge.
        button.wait_for_falling_edge().await;
        Timer::after_millis(DEBOUNCE_MS).await; // delay for mechanical debounce.
        if button.is_high() {
            continue; // ignore false trigger from bounce.
        }

        state_tx.send(true); // confirm press and update shared state.

        // await release or timeout limit.
        match with_timeout(
            Duration::from_millis(LONG_PRESS_MS),
            button.wait_for_rising_edge(), // wait for rising edge indicating release.
        )
        .await
        {
            Ok(_) => {
                // released within timeout period.
                state_tx.send(false); // update state to released.
                let _ = events.try_send(ButtonEvent::ShortPress); // transmit short press event.
            }
            Err(_) => {
                // timeout reached, classifying as long press.
                let _ = events.try_send(ButtonEvent::LongPress); // transmit long press event.
                button.wait_for_rising_edge().await; // await physical release.
                state_tx.send(false); // update state to released.
            }
        }

        Timer::after_millis(DEBOUNCE_MS).await; // delay to prevent release bounce.
    }
}