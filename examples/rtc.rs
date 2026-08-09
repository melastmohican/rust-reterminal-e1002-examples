//! # PCF8563 Real-Time Clock Example (reTerminal E1002)
//!
//! Reads and sets the onboard PCF8563 I2C Real-Time Clock (RTC) on the Seeed Studio reTerminal E1002.
//!
//! The PCF8563 is an ultra-low power CMOS Real-Time Clock and calendar chip with a 32.768 kHz
//! crystal oscillator. It communicates over I2C (address `0x51`) on the carrier board's shared
//! I2C bus (SDA: GPIO19, SCL: GPIO20). This example initializes the RTC with a starting date/time
//! and logs the current timestamp every second.
//!
//! ## Hardware
//!
//! - **Board:** Seeed Studio reTerminal E1002 (XIAO ESP32-S3 MCU)
//! - **Peripherals:** Onboard PCF8563 RTC Chip (I2C Address `0x51`)
//!
//! ## Pin Mapping
//!
//! | Signal | GPIO | Peripheral | Notes |
//! |---|---|---|---|
//! | I2C SDA | GPIO19 | I2C0 Master | Shared I2C bus (RTC & SHT4x) |
//! | I2C SCL | GPIO20 | I2C0 Master | Shared I2C bus (RTC & SHT4x) |
//!
//! ## Wiring Schematic
//!
//! ```text
//!            Seeed Studio reTerminal E1002 Carrier Board
//!          +-------------------------------------------------+
//!          | GPIO19 (I2C SDA) <-------> PCF8563 RTC (SDA)    |
//!          | GPIO20 (I2C SCL) --------> PCF8563 RTC (SCL)    |
//!          | 3.3V / V_BAT ------------> PCF8563 RTC (VDD)    |
//!          | GND ---------------------> PCF8563 RTC (VSS)    |
//!          +-------------------------------------------------+
//! ```
//!
//! ## Run
//!
//! ```bash
//! cargo run --example rtc
//! ```

#![no_std]
#![no_main]

use defmt::info;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_println as _;

esp_bootloader_esp_idf::esp_app_desc!();

/// PCF8563 I2C 7-bit slave address
const PCF8563_ADDR: u8 = 0x51;

/// Converts Binary-Coded Decimal (BCD) to standard decimal.
fn bcd_to_dec(val: u8) -> u8 {
    (val >> 4) * 10 + (val & 0x0F)
}

/// Converts standard decimal to Binary-Coded Decimal (BCD).
fn dec_to_bcd(val: u8) -> u8 {
    ((val / 10) << 4) | (val % 10)
}

/// Date and time representation for PCF8563 RTC.
struct DateTime {
    year: u8,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
}

/// Writes date/time structure to PCF8563 registers.
fn set_rtc_time<'a>(
    i2c: &mut I2c<'a, esp_hal::Blocking>,
    dt: &DateTime,
) -> Result<(), esp_hal::i2c::master::Error> {
    // Stop clock (Control_1 bit 5 = STOP) to ensure atomic time register updates
    i2c.write(PCF8563_ADDR, &[0x00, 0x20])?;

    let buf = [
        0x02, // starting register address (VL_seconds)
        dec_to_bcd(dt.second) & 0x7F,
        dec_to_bcd(dt.minute) & 0x7F,
        dec_to_bcd(dt.hour) & 0x3F,
        dec_to_bcd(dt.day) & 0x3F,
        0, // weekday (0=Sunday)
        dec_to_bcd(dt.month) & 0x1F,
        dec_to_bcd(dt.year),
    ];
    i2c.write(PCF8563_ADDR, &buf)?;

    // Start clock (Control_1 = 0x00)
    i2c.write(PCF8563_ADDR, &[0x00, 0x00])?;
    Ok(())
}

/// Reads current date/time from PCF8563 registers.
fn read_rtc_time<'a>(
    i2c: &mut I2c<'a, esp_hal::Blocking>,
) -> Result<DateTime, esp_hal::i2c::master::Error> {
    let mut buf = [0u8; 7];
    i2c.write_read(PCF8563_ADDR, &[0x02], &mut buf)?;

    Ok(DateTime {
        second: bcd_to_dec(buf[0] & 0x7F),
        minute: bcd_to_dec(buf[1] & 0x7F),
        hour: bcd_to_dec(buf[2] & 0x3F),
        day: bcd_to_dec(buf[3] & 0x3F),
        month: bcd_to_dec(buf[5] & 0x1F),
        year: bcd_to_dec(buf[6]),
    })
}

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("[E1002] RTC demo (PCF8563)");

    let mut i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(esp_hal::time::Rate::from_khz(100)),
    )
    .unwrap()
    .with_sda(peripherals.GPIO19)
    .with_scl(peripherals.GPIO20);

    // Initial time setting (2025-07-10 12:00:00)
    let set_time = DateTime {
        year: 25,
        month: 7,
        day: 10,
        hour: 12,
        minute: 0,
        second: 0,
    };

    if set_rtc_time(&mut i2c, &set_time).is_ok() {
        info!(
            "[rtc] Initialized time to 20{:02}-{:02}-{:02} {:02}:{:02}:{:02}",
            set_time.year,
            set_time.month,
            set_time.day,
            set_time.hour,
            set_time.minute,
            set_time.second
        );
    } else {
        defmt::error!("[rtc] Failed to communicate with PCF8563 RTC on I2C bus!");
    }

    loop {
        match read_rtc_time(&mut i2c) {
            Ok(now) => {
                info!(
                    "[rtc] 20{:02}-{:02}-{:02} {:02}:{:02}:{:02}",
                    now.year, now.month, now.day, now.hour, now.minute, now.second
                );
            }
            Err(_) => {
                defmt::error!("[rtc] Read error!");
            }
        }

        Timer::after(Duration::from_secs(1)).await;
    }
}

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    defmt::error!("{}", panic_info);
    loop {}
}
