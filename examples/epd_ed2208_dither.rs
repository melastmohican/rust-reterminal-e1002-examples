//! # Bayer Ordered Dithering Demonstration using `epdsi` (async)
//!
//! Exercises `epdsi::graphics::dither::dither_seven` on real Spectra 6 ink: six horizontal bands,
//! each a smooth 800 px ramp, quantized per pixel to the six native colors with a 4x4 Bayer
//! matrix. No line buffer and no heap are used; the only large allocation is the 192,000 byte
//! packed frame that `write_frame` takes.
//!
//! See `epd_ed2208_demo.rs`'s module doc for why this uses a local `AsyncRefCellDevice` instead of
//! `embedded_hal_bus::spi::ExclusiveDevice` on this target.
//!
//! ## What to look at
//!
//! Bands, top to bottom, each 80 px tall:
//!
//! 1. Black to white gray ramp: should read as a smooth density change of black and white dots.
//! 2. Black to red ramp: black and red dots, with white never appearing.
//! 3. Black to green ramp.
//! 4. Black to blue ramp.
//! 5. Black to yellow ramp.
//! 6. Full-saturation hue sweep red, yellow, green, cyan, blue, magenta, back to red. Cyan and
//!    magenta are not inks, so they come out as green/blue and blue/red dot mixes.
//!
//! The palette is ideal primaries, not measured ink colors. Real Spectra 6 inks are more muted,
//! so mid-ramp brightness will not match a monitor; judge the dot structure, not color accuracy.
//! Bench-confirmed on the reTerminal E1002 (9 Oct 2026): the ramps show a regular fine dot grid
//! with no stripes or seams, and the colors come out duller than on a monitor.
//!
//! ## Display Specification
//! - **Panel:** Good Display GDEP073E01 (7.3" 800x480 Spectra 6 e-Paper display)
//! - **Controller IC:** ED2208 (via local `epdsi` driver crate)
//! - **Host Board:** Seeed Studio reTerminal E1002 (XIAO ESP32-S3)
//!
//! ## Hardware & Pin Mapping
//!
//! | Signal | GPIO | Notes |
//! |---|---|---|
//! | SPI SCK | GPIO7 | Shared SPI bus |
//! | SPI MISO | GPIO8 | Shared SPI bus |
//! | SPI MOSI | GPIO9 | Shared SPI bus |
//! | EPD CS | GPIO10 | Active-LOW Chip Select |
//! | EPD DC | GPIO11 | Data / Command Selection |
//! | EPD RES | GPIO12 | Hardware Reset |
//! | EPD BUSY | GPIO13 | Busy Signal (Active-LOW: LOW = Busy) |
//!
//! ## Run
//! ```bash
//! cargo run --release --example epd_ed2208_dither
//! ```
//!
//! Needs `epdsi` 0.6.2 or later with the `graphics` feature (enabled in this repo's `Cargo.toml`).

#![no_std]
#![no_main]

use core::cell::RefCell;
use defmt::{error, info};
use embassy_time::{Duration, Timer};

use embassy_time::Delay;
use embedded_graphics::pixelcolor::Rgb888;
use embedded_hal::digital::OutputPin;
use embedded_hal::spi::{ErrorType, Operation};
use embedded_hal_async::delay::DelayNs as AsyncDelayNs;
use embedded_hal_async::spi::{SpiBus as AsyncSpiBus, SpiDevice as AsyncSpiDevice};
use embedded_hal_bus::spi::DeviceError;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::spi::Mode as SpiMode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_println as _;

use epdsi::SpiBusWrapper;
use epdsi::controllers::Ed2208Controller;
use epdsi::driver::EpdBuilder;
use epdsi::graphics::dither::dither_seven;
use epdsi::panels::GDEP073E01;
use epdsi::traits::{ColorChannel, EpdPanel};

esp_bootloader_esp_idf::esp_app_desc!();

/// `RefCell`-shared async `SpiDevice`, used instead of `embedded_hal_bus::spi::ExclusiveDevice`.
/// See `epd_ed2208_demo.rs`'s module doc's "Note on the SPI device wrapper" for why.
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

/// Display geometry.
const WIDTH: usize = 800;
const HEIGHT: usize = 480;
/// Packed frame size: 2 pixels per byte (4-bit nibbles) -> 800 * 480 / 2 = 192,000 bytes.
const FRAME_BYTES: usize = (WIDTH * HEIGHT) / 2;
/// Height of each of the six bands.
const BAND_HEIGHT: usize = HEIGHT / 6;

/// Static 192 KB frame buffer placed directly in DRAM (BSS).
static mut FRAME_BUFFER: [u8; FRAME_BYTES] = [0u8; FRAME_BYTES];

/// Source color for pixel column `x` in band `band`, before dithering.
fn source_color(band: usize, x: usize) -> Rgb888 {
    let v = (x * 255 / (WIDTH - 1)) as u8;
    match band {
        0 => Rgb888::new(v, v, v),
        1 => Rgb888::new(v, 0, 0),
        2 => Rgb888::new(0, v, 0),
        3 => Rgb888::new(0, 0, v),
        4 => Rgb888::new(v, v, 0),
        _ => {
            // Six hue sectors of 256 steps each, at full saturation and value.
            let h = x * 6 * 256 / WIDTH;
            let f = (h % 256) as u8;
            match h / 256 {
                0 => Rgb888::new(255, f, 0),
                1 => Rgb888::new(255 - f, 255, 0),
                2 => Rgb888::new(0, 255, f),
                3 => Rgb888::new(0, 255 - f, 255),
                4 => Rgb888::new(f, 0, 255),
                _ => Rgb888::new(255, 0, 255 - f),
            }
        }
    }
}

/// Dithers every pixel of the test image into the packed 4 bpp frame.
fn fill_dithered(buf: &mut [u8; FRAME_BYTES]) {
    for y in 0..HEIGHT {
        let band = (y / BAND_HEIGHT).min(5);
        for x in 0..WIDTH {
            let color = dither_seven(x as u32, y as u32, source_color(band, x)) as u8;
            let byte_idx = (y * WIDTH + x) / 2;
            if x % 2 == 0 {
                buf[byte_idx] = (buf[byte_idx] & 0x0F) | (color << 4);
            } else {
                buf[byte_idx] = (buf[byte_idx] & 0xF0) | (color & 0x0F);
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

    let mut delay = Delay;

    info!("\n============================================================");
    info!("[E1002] epdsi ED2208 / GDEP073E01 Bayer Dither Demo (async)");
    info!("============================================================");

    let frame_buf: &'static mut [u8; FRAME_BYTES] =
        unsafe { &mut *core::ptr::addr_of_mut!(FRAME_BUFFER) };
    fill_dithered(frame_buf);
    info!("[EPD] Dithered test image rendered into the frame buffer.");

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

    let epd_cs = Output::new(peripherals.GPIO10, Level::High, OutputConfig::default());
    let epd_dc = Output::new(peripherals.GPIO11, Level::Low, OutputConfig::default());
    let epd_rst = Output::new(peripherals.GPIO12, Level::High, OutputConfig::default());
    let epd_busy = Input::new(
        peripherals.GPIO13,
        InputConfig::default().with_pull(Pull::Up),
    );

    let spi_bus_cell = RefCell::new(spi_bus);
    let epd_spi_dev = AsyncRefCellDevice::new(&spi_bus_cell, epd_cs, Delay).unwrap();
    let bus = SpiBusWrapper::new(epd_spi_dev, epd_dc, epd_rst, epd_busy);

    let controller = Ed2208Controller::new(GDEP073E01::WIDTH, GDEP073E01::HEIGHT);
    let mut driver = EpdBuilder::<_, GDEP073E01>::new(controller).build(bus);

    info!("[EPD] Initializing ED2208 controller hardware...");
    if let Err(_e) = driver.init(&mut delay).await {
        error!("[EPD] Driver initialization failed!");
    } else {
        info!("[EPD] Driver initialized successfully.");

        if let Err(_e) = driver.write_frame(ColorChannel::Color7(0), frame_buf).await {
            error!("[EPD] write_frame failed");
        } else if let Err(_e) = driver.refresh(&mut delay).await {
            error!("[EPD] refresh failed");
        } else {
            info!("[EPD] Dithered image shown.");
        }

        info!("[EPD] Putting display into deep sleep.");
        let _ = driver.sleep(&mut delay).await;
    }

    info!("[E1002] Entering idle loop.");
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", panic_info);
    loop {}
}
