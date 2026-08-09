//! # microSD Card Reader / Writer Example (reTerminal E1002)
//!
//! Reads and writes files on the onboard microSD card slot of the Seeed Studio reTerminal E1002.
//!
//! The carrier board includes a microSD card slot with:
//! - **Power Enable (GPIO16):** Active-HIGH enable pin driving an onboard load switch.
//! - **Card Detect (GPIO15):** Active-LOW input pin (LOW = card inserted, HIGH = no card).
//! - **SPI Bus (SPI2 / HSPI):** SCK=GPIO7, MISO=GPIO8, MOSI=GPIO9, CS=GPIO14.
//!
//! Demonstrates card detect checking, mounting FAT volumes using `embedded-sdmmc`,
//! listing root directory contents, writing `/HELLO.TXT`, and reading file size.
//!
//! ## Hardware
//!
//! - **Board:** Seeed Studio reTerminal E1002 (XIAO ESP32-S3 MCU)
//! - **Peripherals:** Onboard microSD Card Slot & Load Switch
//!
//! ## Pin Mapping
//!
//! | Signal | GPIO | Peripheral | Notes |
//! |---|---|---|---|
//! | SPI SCK | GPIO7 | SPI2 Master | Shared SPI bus |
//! | SPI MISO | GPIO8 | SPI2 Master | Shared SPI bus |
//! | SPI MOSI | GPIO9 | SPI2 Master | Shared SPI bus |
//! | SD CS | GPIO14 | GPIO Output | SD Card Chip Select |
//! | Card Detect | GPIO15 | Input (Pull-Up) | Active-LOW (LOW = Card Present) |
//! | Power Enable | GPIO16 | Output | Active-HIGH (HIGH = Power ON) |
//!
//! ## Wiring Schematic
//!
//! ```text
//!            Seeed Studio reTerminal E1002 Carrier Board
//!          +-------------------------------------------------+
//!          | GPIO16 (Power Enable) ----> [ Load Switch VDD ] |
//!          | GPIO15 (Card Detect) <---- [ MicroSD Detect ]   |
//!          | GPIO7  (SPI SCK)  --------> [ MicroSD SCK ]     |
//!          | GPIO8  (SPI MISO) <-------- [ MicroSD MISO ]    |
//!          | GPIO9  (SPI MOSI) --------> [ MicroSD MOSI ]    |
//!          | GPIO14 (SD CS)    --------> [ MicroSD CS ]      |
//!          +-------------------------------------------------+
//! ```
//!
//! ## Run
//!
//! ```bash
//! cargo run --example sd
//! ```

#![no_std]
#![no_main]

use defmt::info;
use embassy_time::{Duration, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::sdcard::DummyCsPin;
use embedded_sdmmc::{Mode, SdCard, TimeSource, Timestamp, VolumeManager};
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::spi::Mode as SpiMode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_println as _;

esp_bootloader_esp_idf::esp_app_desc!();

/// Dummy Timestamp Provider for FAT filesystem file creation/modification timestamps.
struct DummyTimeSource;
impl TimeSource for DummyTimeSource {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp {
            year_since_1970: 55, // 2025
            zero_indexed_month: 6,
            zero_indexed_day: 9,
            hours: 12,
            minutes: 0,
            seconds: 0,
        }
    }
}

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    let delay = Delay::new();

    info!("[E1002] SD card demo initializing...");

    // Power enable (GPIO 16, active-high) - enable power rail to microSD slot
    let mut sd_en = Output::new(peripherals.GPIO16, Level::High, OutputConfig::default());
    sd_en.set_high();
    Timer::after(Duration::from_millis(50)).await;

    // Card detect (GPIO 15, active-low)
    let sd_det = Input::new(
        peripherals.GPIO15,
        InputConfig::default().with_pull(Pull::Up),
    );

    if sd_det.is_high() {
        defmt::warn!("[SD] No card detected (DET pin HIGH).");
        defmt::warn!("[E1002] Halting - please insert a microSD card.");
        loop {
            Timer::after(Duration::from_secs(1)).await;
        }
    }

    info!("[SD] Card detected in slot.");

    // Configure SPI (HSPI: SCK=7, MISO=8, MOSI=9)
    let spi_bus = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(esp_hal::time::Rate::from_mhz(10))
            .with_mode(SpiMode::_0),
    )
    .unwrap()
    .with_sck(peripherals.GPIO7)
    .with_miso(peripherals.GPIO8)
    .with_mosi(peripherals.GPIO9);

    let sd_cs = Output::new(peripherals.GPIO14, Level::High, OutputConfig::default());
    let spi_device = ExclusiveDevice::new_no_delay(spi_bus, sd_cs).unwrap();

    let sdcard = SdCard::new(spi_device, DummyCsPin, delay);
    let mut volume_mgr = VolumeManager::new(sdcard, DummyTimeSource);

    info!("[SD] Attempting to mount volume 0...");
    match volume_mgr.open_volume(embedded_sdmmc::VolumeIdx(0)) {
        Ok(mut volume) => {
            info!("[SD] Volume mounted successfully.");
            match volume.open_root_dir() {
                Ok(mut root_dir) => {
                    info!("[SD] Listing root directory:");
                    let _ = root_dir.iterate_dir(|entry| {
                        info!(
                            "[SD]   {} ({} bytes)",
                            defmt::Display2Format(&entry.name),
                            entry.size
                        );
                    });

                    info!("[SD] Writing to /hello.txt...");
                    if let Ok(mut file) =
                        root_dir.open_file_in_dir("HELLO.TXT", Mode::ReadWriteCreateOrTruncate)
                    {
                        let msg = b"Hello from reTerminal E1002 in Rust!\n";
                        let _ = file.write(msg);
                        info!("[SD] Successfully wrote to /hello.txt");
                    } else {
                        defmt::error!("[SD] Failed to open /hello.txt for writing.");
                    }
                }
                Err(_) => {
                    defmt::error!("[SD] Failed to open root directory.");
                }
            }
        }
        Err(_) => {
            defmt::error!("[SD] Failed to mount volume. Check filesystem format (FAT16/FAT32).");
        }
    }

    info!("[E1002] SD demo complete.");
    loop {
        Timer::after(Duration::from_secs(5)).await;
    }
}

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    defmt::error!("{}", panic_info);
    loop {}
}
