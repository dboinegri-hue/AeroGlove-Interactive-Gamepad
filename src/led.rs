//! rgb indicator led for the aeroglove.
//!
//! monitors a watch channel for mode changes to adjust colors.
//! controls three standard gpio pins, one for each color channel.
//! bypassing pwm here; binary on/off states suffice for this indicator.

use defmt::*; // logging macros.
use embassy_stm32::gpio::Output; // gpio output driver for the nucleo pins.
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex; // interrupt-safe mutex.
use embassy_sync::watch::Receiver; // receiver for mode changes published by other tasks.

#[derive(Clone, Copy, PartialEq, defmt::Format)] // traits required for passing and comparing modes.
pub enum Mode {
    /// racing layout - red led. tilt acts as steering, button triggers acceleration.
    Racing,
    /// mouse layout - blue led. tilt controls cursor, button executes left click.
    Mouse,
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Racing // sets racing as the default startup mode.
    }
}

#[embassy_executor::task] // designates this as an asynchronous background task.
pub async fn led_task(
    mut r: Output<'static>,  // gpio output for red.
    mut g: Output<'static>,  // gpio output for green.
    mut b: Output<'static>,  // gpio output for blue.
    mut mode_rx: Receiver<'static, CriticalSectionRawMutex, Mode, 3>, // receiver suspending until mode updates.
) {
    // configures common-cathode rgb led modules (active high).
    // flip set_high/set_low if utilizing a common-anode component.
    apply(Mode::default(), &mut r, &mut g, &mut b); // initialize color prior to loop execution.

    loop {
        let mode = mode_rx.changed().await; // suspends task until a new mode is received.
        info!("LED: switching to {}", mode); // logs the mode transition for debugging purposes.
        apply(mode, &mut r, &mut g, &mut b); // applies the new state to the physical pins.
    }
}

// helper function to configure the three pins based on the active mode.
// isolates logic to prevent duplication within the task.
fn apply(mode: Mode, r: &mut Output<'static>, g: &mut Output<'static>, b: &mut Output<'static>) {
    match mode {
        Mode::Racing => {
            r.set_high(); // activates red for racing mode.
            g.set_low();  // deactivates green.
            b.set_low();  // deactivates blue.
        }
        Mode::Mouse => {
            r.set_low();  // deactivates red.
            g.set_low();  // deactivates green.
            b.set_high(); // activates blue for mouse mode.
        }
    }
}