//! vibration motor driver.
//!
//! processes hapticpulse messages via channel to execute timed pwm bursts on tim2_ch1 (pa0).
//!
//! currently utilized for local feedback pending full usb hid implementation 
//! for game-driven rumble (e.g., collision feedback from godot). 
//! short pulses verify hardware wiring on button press.

use defmt::*; // logging utilities.
use embassy_stm32::peripherals; // hardware peripheral access.
use embassy_stm32::timer::simple_pwm::SimplePwm; // pwm driver wrapper.
use embassy_stm32::timer::Channel as TimChannel; // timer channel selection.
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex; // interrupt-safe mutex.
use embassy_sync::channel::Channel; // async message queue.
use embassy_time::Timer; // asynchronous delay timer.

#[derive(Clone, Copy, defmt::Format)] // traits for value passing and logging.
pub struct HapticPulse {
    /// intensity percentage (0-100). clamped to maximum if exceeded.
    pub strength: u8,     // requested pwm duty cycle percentage.
    /// activation duration in milliseconds.
    pub duration_ms: u32, // length of active pulse.
}

#[embassy_executor::task] // concurrent motor control task.
pub async fn motor_task(
    mut pwm: SimplePwm<'static, peripherals::TIM2>,                    // configured timer2 pwm instance.
    queue: &'static Channel<CriticalSectionRawMutex, HapticPulse, 4>, // command reception channel.
) {
    let ch = TimChannel::Ch1; // target channel mapped to pa0.
    let max = pwm.max_duty_cycle() as u32; // maximum timer counter value based on frequency.

    pwm.channel(ch).set_duty_cycle(0); // initialize motor to off state.
    pwm.channel(ch).enable(); // activate hardware pwm output.

    info!("Motor task running (PWM max duty = {})", max); // initialization log with mathematical reference.

    loop {
        let pulse = queue.receive().await; // suspend execution until pulse command is received.
        let strength = pulse.strength.min(100) as u32; // enforce upper bound on intensity parameter.
        let duty = ((max * strength) / 100) as u16; // compute hardware duty cycle from percentage.

        info!(
            "Motor: pulse {}% for {} ms (duty = {})",
            strength, pulse.duration_ms, duty
        );

        pwm.channel(ch).set_duty_cycle(duty); // apply computed duty cycle.
        Timer::after_millis(pulse.duration_ms as u64).await; // maintain state for requested duration.
        pwm.channel(ch).set_duty_cycle(0); // terminate pulse by resetting duty cycle to zero.
    }
}