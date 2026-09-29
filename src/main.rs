#![no_std] // no standard library since this runs bare-metal on the stm32.
#![no_main] // using embassy's entry point macro instead of standard main.

//! aeroglove firmware -- serial mode.
//!
//! streams orientation, button state and current mode out the st-link virtual
//! com port at 50 hz. host reads the stream and forwards it to the godot game
//! environment. switching modes is handled by long-pressing the button.
//!
//! requires only one usb cable (the st-link usb-c connector).

mod button;  // button input handling with debounce.
mod led;     // rgb led mode indicator control.
mod motor;   // pwm driver for the haptic vibration motor.
mod mpu6050; // imu sensor data extraction.
mod serial;  // uart formatting and pc communication.

use defmt::*;            // logging utilities.
use defmt_rtt as _;      // route logs to the debugger terminal.
use panic_probe as _;    // route panic messages through rtt.

use embassy_executor::Spawner;                        // background task spawner.
use embassy_futures::select::{select, Either};        // concurrent async waiting.
use embassy_stm32::bind_interrupts;                   // hardware interrupt routing.
use embassy_stm32::exti::ExtiInput;                   // external interrupt for button.
use embassy_stm32::gpio::{Level, Output, OutputType, Pull, Speed}; // gpio configurations.
use embassy_stm32::i2c::{self, I2c};                  // i2c driver for the imu.
use embassy_stm32::peripherals;                       // peripheral access.
use embassy_stm32::time::khz;                         // frequency unit helper.
use embassy_stm32::timer::low_level::CountingMode;    // pwm timer configuration.
use embassy_stm32::timer::simple_pwm::{PwmPin, SimplePwm}; // motor pwm driver.
use embassy_stm32::usart::{Config as UartConfig, UartTx};  // pc serial communication.
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex; // interrupt-safe mutex.
use embassy_sync::channel::Channel;                   // inter-task message queue.
use embassy_sync::watch::Watch;                       // shared observable state.
use embassy_time::{Instant, Timer};                   // async timing utilities.

use crate::button::ButtonEvent;  // short and long press events.
use crate::led::Mode;            // racing and mouse modes.
use crate::motor::HapticPulse;   // motor strength and duration parameters.
use crate::mpu6050::Orientation; // pitch and roll degrees.

// route hardware interrupts to embassy handlers.
bind_interrupts!(struct Irqs {
    I2C1_EV => i2c::EventInterruptHandler<peripherals::I2C1>; // i2c transfer completion.
    I2C1_ER => i2c::ErrorInterruptHandler<peripherals::I2C1>; // i2c error event.
});

// global shared state for inter-task communication.
static ORIENTATION:   Watch<CriticalSectionRawMutex, Orientation, 3>   = Watch::new(); // latest pitch/roll data.
static MODE:          Watch<CriticalSectionRawMutex, Mode, 3>          = Watch::new(); // active mode state.
static BUTTON_STATE:  Watch<CriticalSectionRawMutex, bool, 3>          = Watch::new(); // physical button held state.
static BUTTON_EVENTS: Channel<CriticalSectionRawMutex, ButtonEvent, 4> = Channel::new(); // button press event queue.
static HAPTIC_QUEUE:  Channel<CriticalSectionRawMutex, HapticPulse, 4> = Channel::new(); // requested motor pulses.

#[embassy_executor::main] // async entry point.
async fn main(spawner: Spawner) {
    // initialize peripherals with default clocks.
    let p = embassy_stm32::init(Default::default());

    info!("AeroGlove firmware booting (Serial mode)..."); // startup confirmation.

    // configure i2c1 for mpu6050 communication.
    // using pb6 (scl) and pb7 (sda) based on board layout.
    let i2c = I2c::new(
        p.I2C1,
        p.PB6,
        p.PB7,
        Irqs,
        p.GPDMA1_CH0,    // tx dma channel.
        p.GPDMA1_CH1,    // rx dma channel.
        Default::default(),
    );

    // configure pb0 as active-low external interrupt button.
    let button = ExtiInput::new(p.PB0, p.EXTI0, Pull::Up);

    // configure rgb led pins.
    // blue is routed to pa3 since pa10 is reserved for usart1 rx.
    let led_r = Output::new(p.PA1, Level::Low, Speed::Low);
    let led_g = Output::new(p.PA4, Level::Low, Speed::Low);
    let led_b = Output::new(p.PA3, Level::Low, Speed::Low);

    // configure pa0 for pwm motor control via timer2.
    let pwm_pin = PwmPin::new(p.PA0, OutputType::PushPull);
    let pwm = SimplePwm::new(
        p.TIM2,
        Some(pwm_pin),
        None, None, None,
        khz(1),                   // 1khz pwm frequency.
        CountingMode::EdgeAlignedUp,
    );

    // configure usart1 for serial transmission to pc.
    // pa9 is physically wired to the st-link virtual com port.
    let mut uart_config = UartConfig::default();
    uart_config.baudrate = 115_200;
    let uart_tx = UartTx::new(
        p.USART1,
        p.PA9,         // tx pin.
        p.GPDMA1_CH2,  // tx dma channel.
        uart_config,
    )
    .expect("USART1 init failed");

    // establish initial mode state prior to task spawning.
    MODE.sender().send(Mode::Racing);

    // spawn concurrent background tasks.

    // initialize 100hz imu polling task.
    unwrap!(spawner.spawn(mpu6050::mpu_task(i2c, ORIENTATION.sender())));

    // initialize button interrupt listener.
    unwrap!(spawner.spawn(button::button_task(button, &BUTTON_EVENTS, &BUTTON_STATE)));

    // initialize led state manager.
    unwrap!(spawner.spawn(led::led_task(
        led_r, led_g, led_b,
        unwrap!(MODE.receiver()),
    )));

    // initialize haptic motor controller.
    unwrap!(spawner.spawn(motor::motor_task(pwm, &HAPTIC_QUEUE)));

    // initialize 50hz serial telemetry task.
    unwrap!(spawner.spawn(serial::serial_task(
        uart_tx,
        unwrap!(ORIENTATION.receiver()),
        unwrap!(BUTTON_STATE.receiver()),
        unwrap!(MODE.receiver()),
    )));

    // main coordinator setup.
    let mode_tx = MODE.sender();
    let mut orient_rx = unwrap!(ORIENTATION.receiver());
    let mut current_mode = Mode::Racing;
    let mut last_log = Instant::now();

    // maximum tilt angle before triggering haptic feedback limit.
    const TILT_LIMIT_DEG: f32 = 50.0;
    let mut at_tilt_limit = false; // state tracker for the limit threshold.

    loop {
        // await either a button event or a 50ms polling interval.
        match select(BUTTON_EVENTS.receive(), Timer::after_millis(50)).await {

            // button event registered.
            Either::First(evt) => match evt {

                ButtonEvent::ShortPress => {
                    info!("[input] short press (mode = {})", current_mode);
                    // trigger short feedback pulse.
                    let _ = HAPTIC_QUEUE.try_send(HapticPulse {
                        strength: 60,   // 60% duty cycle.
                        duration_ms: 40, // 40ms duration.
                    });
                }

                ButtonEvent::LongPress => {
                    // toggle active operational mode.
                    current_mode = match current_mode {
                        Mode::Racing => Mode::Mouse,
                        Mode::Mouse  => Mode::Racing,
                    };
                    info!("[input] long press -> new mode = {}", current_mode);
                    mode_tx.send(current_mode); // broadcast mode update.

                    // trigger extended feedback pulse for mode switch confirmation.
                    let _ = HAPTIC_QUEUE.try_send(HapticPulse {
                        strength: 90,    // 90% duty cycle.
                        duration_ms: 150, // 150ms duration.
                    });
                }
            },

            // 50ms polling interval reached.
            Either::Second(_) => {
                // fetch latest orientation reading if available.
                if let Some(o) = orient_rx.try_get() {

                    // calculate maximum tilt across both axes.
                    let max_tilt = libm::fabsf(o.pitch).max(libm::fabsf(o.roll));
                    let now_at_limit = max_tilt >= TILT_LIMIT_DEG;

                    // trigger haptic alert only on rising edge of the tilt limit.
                    if now_at_limit && !at_tilt_limit {
                        info!(
                            "[input] tilt limit reached ({} deg)",
                            max_tilt as i32
                        );
                        // trigger medium feedback pulse.
                        let _ = HAPTIC_QUEUE.try_send(HapticPulse {
                            strength: 70,   // 70% duty cycle.
                            duration_ms: 80, // 80ms duration.
                        });
                    }

                    // update limit state tracker.
                    at_tilt_limit = now_at_limit;

                    // periodic telemetry logging every 500ms.
                    if last_log.elapsed().as_millis() >= 500 {
                        info!("[mpu] pitch = {} deg, roll = {} deg", o.pitch, o.roll);
                        last_log = Instant::now();
                    }
                }
            }
        }
    }
}