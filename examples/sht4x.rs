//! # Sensirion SHT4x Temperature & Humidity Sensor Example (reTerminal E1002)
//!
//! Reads ambient temperature (°C) and relative humidity (%RH) from the onboard SHT4x sensor
//! on the Seeed Studio reTerminal E1002.
//!
//! The Sensirion SHT40 / SHT41 / SHT45 is a high-accuracy digital I2C sensor:
//! - **Temperature accuracy:** ±0.2 °C (range -40 °C to +125 °C)
//! - **Humidity accuracy:** ±1.8 %RH (range 0 %RH to 100 %RH)
//! - **I2C Address:** `0x44` on the shared I2C bus (SDA: GPIO19, SCL: GPIO20)
//!
//! Demonstrates sensor detection, reading the unique 32-bit factory serial number,
//! triggering high-precision measurements (command `0xFD`), converting raw 16-bit sensor data,
//! and logging formatted readings every 2 seconds.
//!
//! ## Hardware
//!
//! - **Board:** Seeed Studio reTerminal E1002 (XIAO ESP32-S3 MCU)
//! - **Peripherals:** Onboard Sensirion SHT40 Sensor (I2C Address `0x44`)
//!
//! ## Pin Mapping
//!
//! | Signal | GPIO | Peripheral | Notes |
//! |---|---|---|---|
//! | I2C SDA | GPIO19 | I2C0 Master | Shared I2C bus (SHT4x & RTC) |
//! | I2C SCL | GPIO20 | I2C0 Master | Shared I2C bus (SHT4x & RTC) |
//!
//! ## Wiring Schematic
//!
//! ```text
//!            Seeed Studio reTerminal E1002 Carrier Board
//!          +-------------------------------------------------+
//!          | GPIO19 (I2C SDA) <-------> Sensirion SHT40 (SDA)|
//!          | GPIO20 (I2C SCL) --------> Sensirion SHT40 (SCL)|
//!          | 3.3V --------------------> Sensirion SHT40 (VDD)|
//!          | GND ---------------------> Sensirion SHT40 (VSS)|
//!          +-------------------------------------------------+
//! ```
//!
//! ## Run
//!
//! ```bash
//! cargo run --example sht4x
//! ```

#![no_std]
#![no_main]

use defmt::info;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_println as _;

esp_bootloader_esp_idf::esp_app_desc!();

/// SHT4x default I2C 7-bit slave address
const SHT4X_ADDR: u8 = 0x44;

/// High-precision measurement command
const CMD_MEASURE_HIGH_PRECISION: u8 = 0xFD;

/// Read serial number command
const CMD_READ_SERIAL: u8 = 0x89;

/// Reads the unique 32-bit serial number from SHT4x sensor.
fn read_sht4x_serial<'a>(
    i2c: &mut I2c<'a, esp_hal::Blocking>,
) -> Result<u32, esp_hal::i2c::master::Error> {
    let mut buf = [0u8; 6];
    i2c.write_read(SHT4X_ADDR, &[CMD_READ_SERIAL], &mut buf)?;

    let serial = ((buf[0] as u32) << 24)
        | ((buf[1] as u32) << 16)
        | ((buf[3] as u32) << 8)
        | (buf[4] as u32);
    Ok(serial)
}

/// Triggers high-precision measurement and converts raw 16-bit counts to °C and %RH.
fn read_sht4x_measurement<'a>(
    i2c: &mut I2c<'a, esp_hal::Blocking>,
) -> Result<(f32, f32), esp_hal::i2c::master::Error> {
    i2c.write(SHT4X_ADDR, &[CMD_MEASURE_HIGH_PRECISION])?;

    // Measurement duration for high precision is ~8.3 ms max
    embassy_time::block_for(Duration::from_millis(10));

    let mut buf = [0u8; 6];
    i2c.read(SHT4X_ADDR, &mut buf)?;

    let t_raw = ((buf[0] as u32) << 8) | (buf[1] as u32);
    let rh_raw = ((buf[3] as u32) << 8) | (buf[4] as u32);

    // Formula from Sensirion SHT4x datasheet:
    // Temp (°C) = -45 + 175 * (raw / 65535)
    // Humidity (%RH) = -6 + 125 * (raw / 65535)
    let temp_c = -45.0 + 175.0 * (t_raw as f32 / 65535.0);
    let humidity_rh = (-6.0 + 125.0 * (rh_raw as f32 / 65535.0)).clamp(0.0, 100.0);

    Ok((temp_c, humidity_rh))
}

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("[E1002] SHT4x temperature & humidity demo");

    let mut i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(esp_hal::time::Rate::from_khz(100)),
    )
    .unwrap()
    .with_sda(peripherals.GPIO19)
    .with_scl(peripherals.GPIO20);

    match read_sht4x_serial(&mut i2c) {
        Ok(serial) => {
            info!("[SHT4x] Found sensor: serial 0x{:08X}", serial);
        }
        Err(_) => {
            defmt::error!("[SHT4x] Sensor not found - check wiring and I2C address 0x44.");
        }
    }

    loop {
        let start = embassy_time::Instant::now();
        match read_sht4x_measurement(&mut i2c) {
            Ok((temp_c, rh)) => {
                let elapsed_ms = start.elapsed().as_millis();

                let temp_int = temp_c as i32;
                let temp_dec = ((temp_c.abs() * 100.0) as u32) % 100;

                let rh_int = rh as u32;
                let rh_dec = ((rh * 100.0) as u32) % 100;

                info!(
                    "[SHT4x] Temp: {}.{:02} C  Humidity: {}.{:02} %RH  (took {} ms)",
                    temp_int, temp_dec, rh_int, rh_dec, elapsed_ms
                );
            }
            Err(_) => {
                defmt::error!("[SHT4x] Read measurement failed!");
            }
        }

        Timer::after(Duration::from_secs(2)).await;
    }
}

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    defmt::error!("{}", panic_info);
    loop {}
}
