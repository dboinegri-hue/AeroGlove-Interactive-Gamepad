//! async driver for the mpu6050 imu sensor.
//!
//! reads raw accelerometer and gyroscope data over i2c, then runs a
//! complementary filter to get stable pitch and roll angles.
//! publishes the result to a watch so other tasks can always read
//! the latest orientation without waiting.

use defmt::*;
use embassy_stm32::i2c::{I2c, Master};
use embassy_stm32::mode::Async;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::watch::Sender;
use embassy_time::{Instant, Timer};
use libm::{atan2f, sqrtf};

// i2c address of the mpu6050. 0x68 is default when ad0 is held low.
// pulling ad0 high alters the address to 0x69 to resolve bus conflicts.
const MPU_ADDR: u8 = 0x68;

// register addresses defined in the mpu6050 datasheet.
const REG_SMPLRT_DIV:   u8 = 0x19; // clock divider for the output sample rate.
const REG_CONFIG:       u8 = 0x1A; // digital low-pass filter (dlpf) configuration.
const REG_GYRO_CONFIG:  u8 = 0x1B; // gyroscope full-scale range setup.
const REG_ACCEL_CONFIG: u8 = 0x1C; // accelerometer full-scale range setup.
const REG_ACCEL_XOUT_H: u8 = 0x3B; // starting register for the 14-byte sensor data payload.
const REG_PWR_MGMT_1:   u8 = 0x6B; // power management. writing 0x00 exits sleep mode.
const REG_WHO_AM_I:     u8 = 0x75; // device id register. expected value is 0x68 for verification.

// scaling factors to convert raw 16-bit adc readings into physical units.
const ACCEL_LSB_PER_G:    f32 = 16384.0; // scale factor for ±2g range (16384 lsb/g).
const GYRO_LSB_PER_DEG_S: f32 = 131.0;   // scale factor for ±250 dps range (131 lsb/deg/s).
const RAD_TO_DEG: f32 = 57.295_78;       // conversion constant from radians to degrees (180/pi).

// filter coefficient for the complementary filter.
// 0.98 heavily weights the integrated gyro data to prevent accelerometer noise interference.
const COMPLEMENTARY_ALPHA: f32 = 0.98;

// filtered pitch and roll output in degrees.
#[derive(Clone, Copy, Default, defmt::Format)]
pub struct Orientation {
    pub pitch: f32, // forward/backward tilt axis. positive indicates backward tilt.
    pub roll:  f32, // left/right tilt axis. positive indicates rightward tilt.
}

#[embassy_executor::task] // concurrent background task for sensor polling and filtering.
pub async fn mpu_task(
    mut i2c: I2c<'static, Async, Master>,
    sender: Sender<'static, CriticalSectionRawMutex, Orientation, 3>,
) {
    //  wake the sensor from default sleep state 
    if let Err(e) = i2c.write(MPU_ADDR, &[REG_PWR_MGMT_1, 0x00]).await {
        error!("MPU6050: wake-up write failed: {:?}", e); // failure here usually indicates incorrect wiring.
        return;
    }
    Timer::after_millis(50).await; // allow 50ms for internal oscillator stabilization.

    // verify i2c communication by reading the device id 
    let mut who = [0u8; 1]; // single byte buffer for the device id response.
    match i2c.write_read(MPU_ADDR, &[REG_WHO_AM_I], &mut who).await {
        Ok(()) => info!("MPU6050: WHO_AM_I = 0x{:02X} (expected 0x68)", who[0]),
        Err(e) => {
            error!("MPU6050: WHO_AM_I read failed: {:?}", e); // connection error, check sda/scl lines.
            return;
        }
    }

    // configure filtering and measurement ranges 
    //   dlpf = 3 sets accel bw to ~44hz and gyro bw to ~42hz (suitable for hand motion tracking).
    //   gyro range = 0 configures ±250 dps sensitivity.
    //   accel range = 0 configures ±2g sensitivity.
    //   smplrt_div = 9 yields a 100hz sample rate (1khz / (1 + 9)).
    let _ = i2c.write(MPU_ADDR, &[REG_CONFIG,       0x03]).await; // enable dlpf to attenuate high-frequency vibration.
    let _ = i2c.write(MPU_ADDR, &[REG_GYRO_CONFIG,  0x00]).await; // select maximum gyro sensitivity.
    let _ = i2c.write(MPU_ADDR, &[REG_ACCEL_CONFIG, 0x00]).await; // select maximum accel sensitivity.
    let _ = i2c.write(MPU_ADDR, &[REG_SMPLRT_DIV,   9   ]).await; // set data output rate to 100hz.
    info!("MPU6050: configured (±2g, ±250dps, 100 Hz)");

    // initialize complementary filter state variables 
    let mut pitch: f32 = 0.0; // filter converges rapidly from initial zero state.
    let mut roll:  f32 = 0.0;
    let mut last = Instant::now(); // timestamp required for gyro numerical integration.

    // buffer for a single i2c burst read: 6 bytes accel, 2 bytes temp (ignored), 6 bytes gyro.
    let mut buf = [0u8; 14]; 

    loop {
        let now = Instant::now();
        let dt = (now - last).as_micros() as f32 / 1_000_000.0; // calculate delta time (dt) in seconds.
        last = now;

        if let Err(e) = i2c
            .write_read(MPU_ADDR, &[REG_ACCEL_XOUT_H], &mut buf)
            .await
        {
            warn!("MPU6050: sensor read failed: {:?}", e);
            Timer::after_millis(10).await; // insert brief delay before retry on read failure.
            continue;
        }

        // parse big-endian 16-bit integers and apply scaling factors.
        let ax = i16::from_be_bytes([buf[0],  buf[1]])  as f32 / ACCEL_LSB_PER_G; // extracted x-axis acceleration.
        let ay = i16::from_be_bytes([buf[2],  buf[3]])  as f32 / ACCEL_LSB_PER_G; // extracted y-axis acceleration.
        let az = i16::from_be_bytes([buf[4],  buf[5]])  as f32 / ACCEL_LSB_PER_G; // extracted z-axis acceleration.
        // buf[6..8] contains temperature data, which is disregarded here.
        let gx = i16::from_be_bytes([buf[8],  buf[9]])  as f32 / GYRO_LSB_PER_DEG_S; // extracted x-axis angular velocity.
        let gy = i16::from_be_bytes([buf[10], buf[11]]) as f32 / GYRO_LSB_PER_DEG_S; // extracted y-axis angular velocity.
        // buf[12..14] contains z-axis angular velocity, not required for pitch/roll calculations.

        // calculate absolute angles from accelerometer using atan2 for full quadrant resolution.
        // this provides a drift-free but high-noise baseline.
        let accel_pitch = atan2f(-ax, sqrtf(ay * ay + az * az)) * RAD_TO_DEG;
        let accel_roll  = atan2f( ay, az) * RAD_TO_DEG;

        // apply complementary filter: fuse high-frequency gyro data with low-frequency accel data.
        // gyro integration handles rapid dynamic motion while the accelerometer corrects long-term drift.
        pitch = COMPLEMENTARY_ALPHA * (pitch + gy * dt)
              + (1.0 - COMPLEMENTARY_ALPHA) * accel_pitch;
        roll  = COMPLEMENTARY_ALPHA * (roll  + gx * dt)
              + (1.0 - COMPLEMENTARY_ALPHA) * accel_roll;

        sender.send(Orientation { pitch, roll }); // broadcast updated orientation to the watch channel.

        // suspend task to align with the ~100hz target sampling rate.
        Timer::after_millis(10).await;
    }
}