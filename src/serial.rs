//! serial protocol over st-link virtual com port (usart1 on pa9).
//!
//! transmits a single ascii line at 50 hz formatted as:
//!
//!     "<mode> <pitch> <roll> <button>\n"
//!
//! where:
//!   mode   - 'r' for racing/gamepad (godot input), 'm' for mouse.
//!   pitch  - forward/back tilt in degrees (negative = forward), 2 decimals.
//!   roll   - left/right tilt in degrees (negative = left), 2 decimals.
//!   button - 0 if released, 1 if held.
//!
//! example: "r 12.50 -3.25 0\n"

use core::fmt::Write as _; // import write trait for heapless string formatting.

use defmt::info; // explicit import prevents macro collision with core::write.
use embassy_stm32::mode::Async; // specifies asynchronous (dma-backed) uart operation.
use embassy_stm32::usart::UartTx; // uart transmit driver interface.
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex; // interrupt-safe mutex.
use embassy_sync::watch::Receiver; // receiver for latest orientation, button state, and mode.
use embassy_time::Timer; // asynchronous timer for packet pacing.
use heapless::String; // stack-allocated string buffer to avoid heap usage.

use crate::led::Mode;            // enumerates active operational modes.
use crate::mpu6050::Orientation; // pitch and roll data structure.

#[embassy_executor::task] // concurrent background serial transmission task.
pub async fn serial_task(
    mut tx: UartTx<'static, Async>,                                            // uart transmit handle connected to pc host.
    mut orient_rx: Receiver<'static, CriticalSectionRawMutex, Orientation, 3>, // receiver for most recent imu calculations.
    mut button_rx: Receiver<'static, CriticalSectionRawMutex, bool, 3>,        // receiver for physical button state.
    mut mode_rx: Receiver<'static, CriticalSectionRawMutex, Mode, 3>,          // receiver for active layout configuration.
) {
    info!("Serial task running."); // initialization verification.

    let mut current_mode = Mode::Racing; // local state cache, refreshed per iteration.
    let mut button_held = false;         // local button cache.

    loop {
        // fetch latest sensor readings.
        let orient = orient_rx.try_get().unwrap_or_default(); // grab latest orientation, defaulting to zero if uninitialized.
        if let Some(b) = button_rx.try_get() {
            button_held = b; // refresh local button state if new data is present.
        }
        if let Some(m) = mode_rx.try_get() {
            current_mode = m; // refresh local mode state if updated.
        }

        // map active mode to packet header character.
        let mode_char = match current_mode {
            Mode::Racing => 'R', // 'r' designates racing or gamepad control.
            Mode::Mouse => 'M',  // 'm' designates system mouse control.
        };
        let btn = u8::from(button_held); // cast boolean to integer for simplified host parsing.

        // construct packet in stack-allocated buffer.
        // utilize core::write to avoid conflict with defmt output.
        let mut buf: String<64> = String::new(); // initialize 64-byte stack buffer for packet construction.
        let _ = core::write!(
            buf,
            "{} {:.2} {:.2} {}\n", // format string: mode, pitch, roll, button, newline.
            mode_char, orient.pitch, orient.roll, btn
        );

        // transmit buffer over uart. host buffers or discards unread bytes.
        let _ = tx.write(buf.as_bytes()).await; 

        Timer::after_millis(20).await; // suspend for 20ms to achieve a 50 hz transmission rate.
    }
}