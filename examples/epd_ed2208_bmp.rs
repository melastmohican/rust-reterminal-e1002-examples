//! # BMP Image Display Example using `epdsi` (ED2208 Controller & GDEP073E01 Panel, async)
//!
//! Displays a 24-bit or 32-bit BMP image from a microSD card on the Seeed Studio reTerminal E1002 carrier board.
//!
//! The SD card leg stays synchronous — `embedded_sdmmc` is a blocking-only crate — using the
//! ordinary blocking `embedded_hal_bus::spi::RefCellDevice` against the shared bus. The EPD leg
//! is driven through `epdsi`'s async API (`default-features = false, features = ["defmt"]`)
//! instead, via a small local [`AsyncRefCellDevice`] (see below): `embedded-hal-bus` only gives
//! `ExclusiveDevice` an async impl, not `RefCellDevice`, so sharing one async-mode
//! [`esp_hal::spi::master::Spi`] between a blocking consumer and an async one needs this. It
//! works because `Spi<Async>` implements both the blocking `embedded_hal::spi::SpiBus` (used by
//! the SD leg, synchronously, from within this `async fn main`) and the async
//! `embedded_hal_async::spi::SpiBus` (used by the EPD leg) at once.
//!
//! ## Display Specification
//! - **Panel:** Good Display GDEP073E01 (7.3" 800x480 6-Color ACeP / Spectra 6 e-Paper display)
//! - **Controller IC:** ED2208 (via local `epdsi` driver)
//! - **Host Board:** Seeed Studio reTerminal E1002 (XIAO ESP32-S3)
//!
//! ## Hardware & Pin Mapping
//!
//! | Signal | GPIO | Notes |
//! |---|---|---|
//! | SPI SCK | GPIO7 | Shared between EPD & SD |
//! | SPI MISO | GPIO8 | SD Card only (EPD has no MISO) |
//! | SPI MOSI | GPIO9 | Shared between EPD & SD |
//! | EPD CS | GPIO10 | Chip Select for EPD |
//! | EPD DC | GPIO11 | Data / Command for EPD |
//! | EPD RES | GPIO12 | Hardware Reset for EPD |
//! | EPD BUSY | GPIO13 | Busy Signal (Active-LOW: LOW = Busy) |
//! | SD CS | GPIO14 | Chip Select for microSD |
//! | SD DET | GPIO15 | Card Detect (Active-LOW: LOW = Card Inserted) |
//! | SD EN | GPIO16 | Power Enable Load Switch (Active-HIGH: HIGH = Power ON) |
//!
//! ## Workflow
//! 1. Powers on the microSD slot via GPIO16.
//! 2. Checks card detect pin (GPIO15).
//! 3. Mounts the FAT filesystem and decodes `/IMAGE.BMP` or `/images/image.bmp` from SD card into a 192 KB nibble frame buffer.
//! 4. Quantizes BMP RGB colors to the 6 native panel colors using nearest Euclidean RGB distance.
//! 5. If no card or BMP file is found, generates a fallback 6-color test stripe pattern.
//! 6. Initializes `epdsi` `Ed2208Controller` + `GDEP073E01` driver and refreshes the display.
//!
//! ## Run
//! ```bash
//! cargo run --example epd_ed2208_bmp
//! ```

#![no_std]
#![no_main]

use core::cell::RefCell;

use defmt::{error, info, warn};
use embassy_time::{Duration, Timer};
use embedded_hal::digital::OutputPin;
use embedded_hal::spi::{ErrorType, Operation};
use embedded_hal_async::delay::DelayNs as AsyncDelayNs;
use embedded_hal_async::spi::{SpiBus as AsyncSpiBus, SpiDevice as AsyncSpiDevice};
use embedded_hal_bus::spi::{DeviceError, RefCellDevice};
use embedded_sdmmc::sdcard::DummyCsPin;
use embedded_sdmmc::{Mode, SdCard, TimeSource, Timestamp, VolumeManager};
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::spi::Mode as SpiMode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_println as _;

use epdsi::SpiBusWrapper;
use epdsi::controllers::Ed2208Controller;
use epdsi::driver::EpdBuilder;
use epdsi::panels::GDEP073E01;
use epdsi::traits::{ColorChannel, EpdPanel, SevenColor};

/// A `RefCell`-shared async [`AsyncSpiDevice`], borrowing the bus for the duration of each
/// transaction. `embedded-hal-bus` 0.2 only gives its `ExclusiveDevice` an async impl — there is
/// no async `RefCellDevice` — so this is the minimal equivalent for a bus with one async and one
/// blocking (`RefCellDevice`, for the SD card) consumer. Mirrors `ExclusiveDevice`'s own async
/// `transaction` body.
struct AsyncRefCellDevice<'a, BUS, CS, D> {
    bus: &'a RefCell<BUS>,
    cs: CS,
    delay: D,
}

impl<'a, BUS, CS, D> AsyncRefCellDevice<'a, BUS, CS, D>
where
    CS: OutputPin,
{
    fn new(bus: &'a RefCell<BUS>, mut cs: CS, delay: D) -> Result<Self, CS::Error> {
        cs.set_high()?;
        Ok(Self { bus, cs, delay })
    }
}

impl<BUS, CS, D> ErrorType for AsyncRefCellDevice<'_, BUS, CS, D>
where
    BUS: ErrorType,
    CS: OutputPin,
{
    type Error = DeviceError<BUS::Error, CS::Error>;
}

impl<BUS, CS, D> AsyncSpiDevice for AsyncRefCellDevice<'_, BUS, CS, D>
where
    BUS: AsyncSpiBus,
    CS: OutputPin,
    D: AsyncDelayNs,
{
    // Held across `.await` deliberately: this bus has exactly one async consumer (the EPD, on
    // this one `esp-rtos` task), so there is no concurrent borrower to conflict with, and no
    // panic risk from re-entrant `borrow_mut()`.
    #[allow(clippy::await_holding_refcell_ref)]
    async fn transaction(
        &mut self,
        operations: &mut [Operation<'_, u8>],
    ) -> Result<(), Self::Error> {
        let mut bus = self.bus.borrow_mut();
        self.cs.set_low().map_err(DeviceError::Cs)?;

        let op_res = 'ops: {
            for op in operations {
                let res = match op {
                    Operation::Read(buf) => bus.read(buf).await,
                    Operation::Write(buf) => bus.write(buf).await,
                    Operation::Transfer(read, write) => bus.transfer(read, write).await,
                    Operation::TransferInPlace(buf) => bus.transfer_in_place(buf).await,
                    Operation::DelayNs(ns) => match bus.flush().await {
                        Err(e) => Err(e),
                        Ok(()) => {
                            self.delay.delay_ns(*ns).await;
                            Ok(())
                        }
                    },
                };
                if let Err(e) = res {
                    break 'ops Err(e);
                }
            }
            Ok(())
        };

        let flush_res = bus.flush().await;
        let cs_res = self.cs.set_high();

        op_res.map_err(DeviceError::Spi)?;
        flush_res.map_err(DeviceError::Spi)?;
        cs_res.map_err(DeviceError::Cs)?;

        Ok(())
    }
}

esp_bootloader_esp_idf::esp_app_desc!();

/// Display Geometry
const WIDTH: usize = 800;
const HEIGHT: usize = 480;
/// Packed frame size: 2 pixels per byte (4-bit nibbles) -> 800 * 480 / 2 = 192,000 bytes
const FRAME_BYTES: usize = (WIDTH * HEIGHT) / 2;

/// Static 192 KB display frame buffer placed directly in DRAM (BSS)
static mut FRAME_BUFFER: [u8; FRAME_BYTES] = [0u8; FRAME_BYTES];

/// Static row buffer for BMP scanlines (3.2 KB)
static mut ROW_BUF: [u8; 800 * 4] = [0u8; 800 * 4];

/// Timestamp Provider for FAT filesystem operations
struct DummyTimeSource;
impl TimeSource for DummyTimeSource {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp {
            year_since_1970: 56, // 2026
            zero_indexed_month: 7,
            zero_indexed_day: 8,
            hours: 12,
            minutes: 0,
            seconds: 0,
        }
    }
}

/// 6-Color Palette for nearest RGB color matching on GDEP073E01
const PALETTE: [(SevenColor, [u8; 3]); 6] = [
    (SevenColor::Black, [0, 0, 0]),
    (SevenColor::White, [255, 255, 255]),
    (SevenColor::Yellow, [255, 255, 0]),
    (SevenColor::Red, [255, 0, 0]),
    (SevenColor::Blue, [0, 0, 255]),
    (SevenColor::Green, [0, 255, 0]),
];

/// Returns the closest `SevenColor` for an RGB pixel using Euclidean distance squared
fn nearest_nibble(r: u8, g: u8, b: u8) -> SevenColor {
    let mut best_dist = i32::MAX;
    let mut best_color = SevenColor::White;
    for (color, [pr, pg, pb]) in PALETTE {
        let dr = r as i32 - pr as i32;
        let dg = g as i32 - pg as i32;
        let db = b as i32 - pb as i32;
        let dist = dr * dr + dg * dg + db * db;
        if dist < best_dist {
            best_dist = dist;
            best_color = color;
        }
    }
    best_color
}

/// Decodes an open uncompressed 24-bit / 32-bit BMP file handle into the frame buffer
fn decode_bmp_file<
    'a,
    D: embedded_sdmmc::BlockDevice,
    T: TimeSource,
    const MAX_DIRS: usize,
    const MAX_FILES: usize,
    const MAX_VOLUMES: usize,
>(
    file: &mut embedded_sdmmc::File<'a, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>,
    frame_buf: &mut [u8; FRAME_BYTES],
) -> Result<(), &'static str> {
    let mut header = [0u8; 54];
    file.read(&mut header)
        .map_err(|_| "Failed to read BMP header")?;

    if header[0] != b'B' || header[1] != b'M' {
        return Err("Not a valid BMP file (magic != 'BM')");
    }

    let data_offset = u32::from_le_bytes([header[10], header[11], header[12], header[13]]);
    let img_w = i32::from_le_bytes([header[18], header[19], header[20], header[21]]);
    let mut img_h = i32::from_le_bytes([header[22], header[23], header[24], header[25]]);
    let bpp = u16::from_le_bytes([header[28], header[29]]);
    let compression = u32::from_le_bytes([header[30], header[31], header[32], header[33]]);

    if compression != 0 {
        return Err("Compressed BMP not supported — save as uncompressed");
    }
    if bpp != 24 && bpp != 32 {
        return Err("Bit depth unsupported — need 24-bit or 32-bit BMP");
    }

    let flip = img_h > 0;
    if img_h < 0 {
        img_h = -img_h;
    }

    info!(
        "[BMP] Image size: {}x{}, {} bpp, offset={}, flip={}",
        img_w, img_h, bpp, data_offset, flip
    );

    let bytes_per_px = (bpp / 8) as usize;
    let row_bytes = (img_w as usize * bytes_per_px + 3) & !3;

    let row_buf: &mut [u8; 800 * 4] = unsafe { &mut *core::ptr::addr_of_mut!(ROW_BUF) };

    let target_w = (img_w as usize).min(WIDTH);
    let target_h = (img_h as usize).min(HEIGHT);

    for row in 0..target_h {
        let bmp_row = if flip { target_h - 1 - row } else { row };
        let row_pos = data_offset as usize + bmp_row * row_bytes;

        file.seek_from_start(row_pos as u32)
            .map_err(|_| "Seek row failed")?;
        let read_len = file
            .read(&mut row_buf[..row_bytes])
            .map_err(|_| "Read row failed")?;
        if read_len < row_bytes {
            return Err("Unexpected end of file while reading row");
        }

        let row_start_idx = row * (WIDTH / 2);

        for col in (0..target_w).step_by(2) {
            let px0 = col * bytes_per_px;
            let b0 = row_buf[px0];
            let g0 = row_buf[px0 + 1];
            let r0 = row_buf[px0 + 2];

            let px1 = (col + 1) * bytes_per_px;
            let b1 = row_buf[px1];
            let g1 = row_buf[px1 + 1];
            let r1 = row_buf[px1 + 2];

            let n0 = nearest_nibble(r0, g0, b0);
            let n1 = nearest_nibble(r1, g1, b1);

            let packed = SevenColor::pack(n0, n1);
            let byte_idx = row_start_idx + (col / 2);
            if byte_idx < frame_buf.len() {
                frame_buf[byte_idx] = packed;
            }
        }
    }

    info!("[BMP] Successfully decoded BMP into frame buffer.");
    Ok(())
}

/// Lists SD card contents and searches root & `/IMAGES/` directory for BMP files
fn load_bmp_from_sd<D: embedded_sdmmc::BlockDevice, T: TimeSource>(
    volume_mgr: &mut VolumeManager<D, T>,
    frame_buf: &mut [u8; FRAME_BYTES],
) -> Result<(), &'static str> {
    let mut volume = volume_mgr
        .open_volume(embedded_sdmmc::VolumeIdx(0))
        .map_err(|_| "Failed to open volume 0")?;
    let mut root_dir = volume
        .open_root_dir()
        .map_err(|_| "Failed to open root directory")?;

    info!("[SD] Root directory contents:");
    let _ = root_dir.iterate_dir(|entry| {
        info!(
            "[SD]   {} ({} bytes)",
            defmt::Display2Format(&entry.name),
            entry.size
        );
    });

    let target_filenames = ["IMAGE.BMP", "PHOTO.BMP", "TEST.BMP", "SAMPLE.BMP"];

    // 1. Search root directory
    for filename in &target_filenames {
        info!("[SD] Checking '/' for '{}'...", filename);
        if let Ok(mut file) = root_dir.open_file_in_dir(*filename, Mode::ReadOnly) {
            info!("[SD] Found '/{}'. Decoding...", filename);
            if decode_bmp_file(&mut file, frame_buf).is_ok() {
                return Ok(());
            }
        }
    }

    // 2. Search /IMAGES/ subdirectory
    if let Ok(mut images_dir) = root_dir.open_dir("IMAGES") {
        info!("[SD] Subdirectory '/IMAGES/' contents:");
        let _ = images_dir.iterate_dir(|entry| {
            info!(
                "[SD]   IMAGES/{} ({} bytes)",
                defmt::Display2Format(&entry.name),
                entry.size
            );
        });

        for filename in &target_filenames {
            info!("[SD] Checking '/IMAGES/' for '{}'...", filename);
            if let Ok(mut file) = images_dir.open_file_in_dir(*filename, Mode::ReadOnly) {
                info!("[SD] Found '/IMAGES/{}'. Decoding...", filename);
                if decode_bmp_file(&mut file, frame_buf).is_ok() {
                    return Ok(());
                }
            }
        }
    }

    Err("No matching BMP file found on SD card")
}

/// Generates a vertical 6-color stripe test pattern
fn generate_test_pattern(frame_buf: &mut [u8; FRAME_BYTES]) {
    info!("[EPD] Generating 6-color stripe test pattern fallback...");
    let colors = [
        SevenColor::Black,
        SevenColor::White,
        SevenColor::Green,
        SevenColor::Blue,
        SevenColor::Red,
        SevenColor::Yellow,
    ];
    let stripe_width = WIDTH / colors.len();

    // Fill buffer with white default
    let white_packed = SevenColor::pack(SevenColor::White, SevenColor::White);
    frame_buf.fill(white_packed);

    for y in 0..HEIGHT {
        let row_start = y * (WIDTH / 2);
        for x in (0..WIDTH).step_by(2) {
            let c_idx0 = (x / stripe_width).min(colors.len() - 1);
            let c_idx1 = ((x + 1) / stripe_width).min(colors.len() - 1);

            let n0 = colors[c_idx0];
            let n1 = colors[c_idx1];

            let byte_idx = row_start + (x / 2);
            if byte_idx < frame_buf.len() {
                frame_buf[byte_idx] = SevenColor::pack(n0, n1);
            }
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

    info!("\n============================================================");
    info!("[E1002] epdsi ED2208 / GDEP073E01 Demo BMP Example (async)");
    info!("============================================================");

    // 1. Initialize static frame buffer (DRAM static allocation)
    let frame_buf: &'static mut [u8; FRAME_BYTES] =
        unsafe { &mut *core::ptr::addr_of_mut!(FRAME_BUFFER) };

    // 2. Power enable microSD slot (GPIO 16, active-HIGH)
    let mut sd_en = Output::new(peripherals.GPIO16, Level::High, OutputConfig::default());
    sd_en.set_high();
    Timer::after(Duration::from_millis(50)).await;

    // 3. Card detect check (GPIO 15, active-LOW)
    let sd_det = Input::new(
        peripherals.GPIO15,
        InputConfig::default().with_pull(Pull::Up),
    );

    // Configure shared SPI bus (HSPI: SCK=7, MISO=8, MOSI=9), async-mode. Still usable
    // synchronously by the SD leg below — `Spi<Async>` implements the blocking
    // `embedded_hal::spi::SpiBus` alongside its async one.
    let spi_bus = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(esp_hal::time::Rate::from_mhz(10))
            .with_mode(SpiMode::_0),
    )
    .unwrap()
    .with_sck(peripherals.GPIO7)
    .with_miso(peripherals.GPIO8)
    .with_mosi(peripherals.GPIO9)
    .into_async();

    let spi_bus_cell = RefCell::new(spi_bus);
    let mut epd_delay = embassy_time::Delay;

    let mut loaded_from_sd = false;

    if sd_det.is_low() {
        info!("[SD] Card detected in slot.");

        let sd_cs = Output::new(peripherals.GPIO14, Level::High, OutputConfig::default());
        let spi_sd_dev = RefCellDevice::new_no_delay(&spi_bus_cell, sd_cs).unwrap();

        let sdcard = SdCard::new(spi_sd_dev, DummyCsPin, delay);
        let mut volume_mgr = VolumeManager::new(sdcard, DummyTimeSource);

        if load_bmp_from_sd(&mut volume_mgr, frame_buf).is_ok() {
            loaded_from_sd = true;
        }

        if !loaded_from_sd {
            warn!("[SD] Could not open BMP file from SD card. Falling back to test pattern.");
        }
    } else {
        warn!("[SD] No card detected (GPIO15 DET is HIGH).");
    }

    if !loaded_from_sd {
        generate_test_pattern(frame_buf);
    }

    // 4. Drive EPD display using `epdsi`
    info!("[EPD] Initializing EPD SPI bus & driver...");

    let epd_cs = Output::new(peripherals.GPIO10, Level::High, OutputConfig::default());
    let epd_dc = Output::new(peripherals.GPIO11, Level::Low, OutputConfig::default());
    let epd_rst = Output::new(peripherals.GPIO12, Level::High, OutputConfig::default());
    let epd_busy = Input::new(
        peripherals.GPIO13,
        InputConfig::default().with_pull(Pull::Up),
    );

    let epd_spi_dev = AsyncRefCellDevice::new(&spi_bus_cell, epd_cs, embassy_time::Delay).unwrap();
    let bus = SpiBusWrapper::new(epd_spi_dev, epd_dc, epd_rst, epd_busy);

    let controller = Ed2208Controller::new(GDEP073E01::WIDTH, GDEP073E01::HEIGHT);
    let mut driver = EpdBuilder::<_, GDEP073E01>::new(controller).build(bus);

    info!("[EPD] Initializing ED2208 hardware & registers...");
    if let Err(_e) = driver.init(&mut epd_delay).await {
        error!("[EPD] Driver initialization failed!");
    } else {
        info!("[EPD] Driver initialized. Transmitting frame buffer...");
        if let Err(_e) = driver.write_frame(ColorChannel::Color7(0), frame_buf).await {
            error!("[EPD] Frame write failed!");
        } else {
            info!("[EPD] Triggering display refresh (~25-30s)...");
            if let Err(_e) = driver.refresh(&mut epd_delay).await {
                error!("[EPD] Display refresh failed!");
            } else {
                info!("[EPD] Refresh complete. Putting display into deep sleep.");
                let _ = driver.sleep(&mut epd_delay).await;
            }
        }
    }

    info!("[E1002] Demo complete. Entering idle loop.");
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", panic_info);
    loop {}
}
