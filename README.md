# AeroGlove

Wearable glove with an IMU that works as a gamepad and air mouse on PC.

*Project by Boinegri Ștefania-Denisa, Group 1222EEB*
*Universitatea Politehnica București, Faculty of Electronics, Telecommunications and Information Technology*
*Academic year 2025–2026*


## About the project

AeroGlove is a glove that lets you control games on PC by moving your hand. Tilt your palm and the joystick moves on screen. Press a tactile button with your thumb and it sends an action. Long-press the button to switch between gamepad mode and air-mouse mode, and the motor vibrates shortly to confirm the switch.

The system uses an MPU6050 IMU sensor to measure palm tilt, a vibration motor for haptic feedback, and an RGB LED to show which mode is active. The microcontroller is an STM32U545RE (ARM Cortex-M33 at 160MHz), and the firmware is written in Rust using the embassy async framework.

Orientation data is sent over UART to a Python script on the PC, which exposes the device as a virtual Xbox 360 controller through ViGEmBus. Any game that supports a gamepad recognizes it automatically.

## Features

- Real-time orientation detection at 100 Hz (pitch + roll) using a complementary filter on accelerometer and gyroscope data
- Two operating modes switched by long-press:
  - Racing/Gamepad — palm tilt mapped to left Xbox stick, button mapped to A
  - Mouse — tilt mapped to cursor movement, button mapped to left click
- Tactile button with software debouncing and short/long press detection (600ms threshold)
- Haptic feedback through PWM motor:
  - Light vibration (60%, 40ms) on short press
  - Stronger vibration (90%, 150ms) on mode switch
- RGB LED indication (red for Racing, blue for Mouse)
- ASCII serial protocol at 115200 baud, 50 Hz

## Hardware

### Components

| Component | Specs |
|---|---|
| Nucleo-U545RE-Q | STM32U545RE, ARM Cortex-M33, 160MHz, 512KB Flash, 274KB SRAM |
| MPU6050 | 3-axis accelerometer ±2g, 3-axis gyroscope ±250°/s, I2C 400kHz |
| Tactile button 6x6x5mm | 4-pin, normally open |
| Vibration motor module | 3-5.3V, <60mA, built-in transistor switch |
| CJMCU RGB LED module | Common cathode, built-in resistors, 3-5V |

### Pinout

| STM32 pin | Function | Connected to | Arduino label |
|---|---|---|---|
| PA0 | TIM2_CH1 PWM | Motor IN | A0 |
| PA1 | GPIO out | LED Red | A1 |
| PA3 | GPIO out | LED Blue | D0 |
| PA4 | GPIO out | LED Green | A2 |
| PA9 | USART1 TX | ST-Link VCP | D8 |
| PB0 | EXTI0 input | Button (internal pull-up) | A3 |
| PB6 | I2C1 SCL | MPU6050 SCL | D15 |
| PB7 | I2C1 SDA | MPU6050 SDA | D14 |


## Software

### Firmware (embedded Rust)

- Rust 2021 edition, target `thumbv8m.main-none-eabihf`
- embassy-stm32 0.4.0 — async hardware abstraction layer for STM32
- embassy-executor 0.9.0 — async runtime for microcontrollers
- embassy-sync 0.7.2 — synchronization primitives (Watch, Channel, Mutex)
- embassy-time 0.5 — async timers
- defmt 1.0 with probe-rs for logging and debugging over SWD
- libm for `atan2f` and `sqrtf` used in the complementary filter
- heapless 0.8 for stack-allocated collections without dynamic memory

### Host (Python)

- pyserial 3.5 for serial communication
- vgamepad 0.1 with ViGEmBus driver for virtual Xbox 360 controller
- mouse 0.7 for cross-platform mouse control

## How to use

### Prerequisites

```powershell
rustup target add thumbv8m.main-none-eabihf
cargo install probe-rs-tools --locked
pip install -r requirements.txt
```

On first run, vgamepad will prompt to install the ViGEmBus driver (automatic .msi installer).

### Build and flash firmware

```powershell
cargo run --release
```

This compiles in release mode, flashes via ST-Link, and shows RTT logs in the terminal.

### Run the host script

In a second PowerShell window:

```powershell
py aeroglove_host.py
```

Make sure `COM_PORT = "COM10"` in the script matches your port (check in Device Manager).

### Testing

Open any game with gamepad support or go to https://hardwaretester.com/gamepad. Press the tactile button once to wake up gamepad detection in the browser. Tilting your palm will move the left joystick.

### Controls

| Action | Racing mode | Mouse mode |
|---|---|---|
| Tilt left/right | Joystick X | Mouse X |
| Tilt forward/back | Joystick Y | Mouse Y |
| Short press button | Button A | Left click |
| Long press (>600ms) | Switch to Mouse | Switch to Racing |

## Code structure

```
aeroglove/
├── Cargo.toml              cargo manifest and dependencies
├── memory.x                linker script (512K flash, 256K RAM)
├── build.rs                includes memory.x in the build
├── .cargo/config.toml      ARM target and probe-rs runner
├── README.md               this file
├── requirements.txt        Python dependencies
├── aeroglove_host.py       PC script: serial -> virtual gamepad / mouse
└── src/
    ├── main.rs             entry point, peripheral init, task spawning
    ├── mpu6050.rs          async I2C driver for MPU6050 + complementary filter
    ├── button.rs           button task with debouncing and long-press detection
    ├── led.rs              RGB LED task driven by the active mode
    ├── motor.rs            PWM motor task with haptic pulse queue
    └── serial.rs           UART transmit task sending packets to the host
```

### Async tasks

The firmware runs 7 async tasks concurrently on a single core:

1. `mpu_task` — reads I2C at 100Hz, applies complementary filter (α=0.98), publishes orientation to a Watch
2. `button_task` — waits on EXTI, applies debouncing, detects short/long press, publishes events and held state
3. `led_task` — reacts to mode changes and updates the LED color
4. `motor_task` — consumes pulses from HAPTIC_QUEUE and generates PWM on TIM2
5. `serial_task` — formats and sends ASCII packets over UART at 50Hz
6. Coordinator (in main.rs) — processes button events, toggles modes, sends haptic pulses

Tasks communicate only through embassy sync primitives (Watch, Channel). There are no global mutable variables, so synchronization is guaranteed at compile time.

## UART protocol

ASCII format, 115200 baud, 8N1:

```
<MODE> <pitch> <roll> <button>\n
```

Example output:

```
R 12.50 -3.25 0
R 12.48 -3.26 0
R 13.10 -2.15 1
M 5.20 8.40 0
```

| Field | Type | Range | Description |
|---|---|---|---|
| MODE | char | R or M | R = Racing/gamepad, M = Mouse |
| pitch | float | -90 to +90 | forward/backward tilt in degrees |
| roll | float | -180 to +180 | left/right tilt in degrees |
| button | int | 0 or 1 | 0 = released, 1 = held |

Rate: 50 Hz (one packet every 20ms).

## Implementation notes

### Complementary filter for orientation

The accelerometer alone gives noisy angles, and the gyroscope alone drifts over time. A complementary filter combines both:

```
pitch_new = α * (pitch_old + gyro_x * dt) + (1 - α) * accel_pitch
```

With α = 0.98, 98% of the signal comes from gyro integration (fast response) and 2% from the accelerometer (long-term drift correction). The result is stable and responsive.

### Long-press detection with async timeout

Instead of polling with a counter, embassy's `with_timeout` is used:

```rust
match with_timeout(Duration::from_millis(600), button.wait_for_rising_edge()).await {
    Ok(_)  => ButtonEvent::ShortPress,  // released before 600ms
    Err(_) => ButtonEvent::LongPress,   // timeout fired = long press
}
```

No manual timers, no CPU overhead.

### Why a Python host translator

The initial plan was to implement USB HID directly on the STM32, but the embassy USB driver for STM32U545RE seems to have a bug with interrupt binding in version 0.4.0. The chip pulls D+ high (Windows detects it connecting) but never generates interrupts for SETUP packets, which causes "Device Descriptor Request Failed".

The solution was to send raw data over UART and have a Python script on the PC expose the device as a virtual Xbox controller through ViGEmBus. Some advantages of this approach:

- Separation of concerns — firmware handles sensing and actuating, the host handles complex protocols
- New modes can be added without recompiling the firmware
- Easy to debug by reading the serial output directly
- Cross-platform — the Python script runs on Linux/macOS with minor changes

## Possible improvements

- Bluetooth module (HC-05) or ESP32 to remove the USB cable
- LiPo 3.7V battery with boost converter for wireless operation
- Auto-calibration of the zero position at startup
- Kalman filter instead of complementary filter for better accuracy
- Capacitive touch sensors on fingers for more input combinations
- Per-game profiles in the Python script (different sensitivity and button mappings)

## References

- STM32U545RE Reference Manual (RM0456)
- Nucleo-U545RE-Q User Manual (UM3062)
- MPU6050 Datasheet (InvenSense)
- embassy-rs documentation (https://embassy.dev)
