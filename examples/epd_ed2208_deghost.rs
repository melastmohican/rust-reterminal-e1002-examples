//! # ACeP De-Ghosting Demonstration using `epdsi` (async)
//!
//! Isolates `EpdDriver::de_ghost()` (added for `epdsi` backlog item `0h`, ported from
//! `Adafruit_ACEP::deGhost()`) as a before/after comparison.
//!
//! A uniform full-panel color swap (e.g. black -> white) gives ghosting nothing to leave a trace
//! of: every pixel makes the same transition, so there is no shape for a ghost to outline. This
//! demo instead writes a bold striped pattern, then swaps to a blank white screen and inspects
//! *that* screen for a faint outline of the stripes, which is the failure mode `de_ghost()`
//! targets.
//!
//! See `epd_ed2208_demo.rs`'s module doc for why this uses a local `AsyncRefCellDevice` instead of
//! `embedded_hal_bus::spi::ExclusiveDevice` on this target.
//!
//! ## Display Specification
//! - **Panel:** Good Display GDEP073E01 (7.3" 800x480 6-Color ACeP / Spectra 6 e-Paper display)
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
//! ## Workflow
//! Two cycles per phase. Each cycle: write a bold 6-color vertical-bar pattern and refresh (so
//! you can confirm the starting pattern), then swap straight to blank white and refresh again.
//! The blank screen is the one to inspect.
//!
//! 1. **Phase 1 (no de-ghost):** pattern -> blank, plain refresh, no clean sweep in between.
//!    Look for a faint outline of the bars on the blank screen.
//! 2. **Phase 2 (de-ghost):** the identical pattern -> blank sequence, but `de_ghost()` runs
//!    between writing the pattern and writing the blank. Compare each blank against the matching
//!    one in Phase 1.
//!
//! ## Run
//! ```bash
//! cargo run --release --example epd_ed2208_deghost
//! ```

#![no_std]
#![no_main]

use core::cell::RefCell;
use defmt::{error, info};
use embassy_time::{Duration, Timer};

use embassy_time::Delay;
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
use epdsi::panels::GDEP073E01;
use epdsi::traits::{ColorChannel, EpdPanel, SevenColor};

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

/// Hold time after each refresh (seconds), long enough to photograph each state.
const HOLD_SECS: u64 = 15;

/// Number of pattern -> blank cycles run per phase.
const CYCLES_PER_PHASE: usize = 2;

/// Static 96 KB pattern buffer placed directly in DRAM (BSS).
static mut PATTERN_BUFFER: [u8; FRAME_BYTES] = [0u8; FRAME_BYTES];

/// Fills `buf` with bold full-height vertical bars across all 6 native colors. Bold, sharp-edged
/// bars give ghosting a clear shape to leave a trace of; a uniform fill does not.
fn fill_bar_pattern(buf: &mut [u8; FRAME_BYTES]) {
    const BARS: [SevenColor; 6] = [
        SevenColor::Black,
        SevenColor::Red,
        SevenColor::Green,
        SevenColor::Blue,
        SevenColor::Yellow,
        SevenColor::White,
    ];
    let bar_width = WIDTH / BARS.len();

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let bar = (x / bar_width).min(BARS.len() - 1);
            let color = BARS[bar] as u8;
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
    info!("[E1002] epdsi ED2208 / GDEP073E01 De-Ghost Demo (async)");
    info!("============================================================");

    let pattern_buf: &'static mut [u8; FRAME_BYTES] =
        unsafe { &mut *core::ptr::addr_of_mut!(PATTERN_BUFFER) };
    fill_bar_pattern(pattern_buf);

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

        let white_packed = SevenColor::pack(SevenColor::White, SevenColor::White);

        for (phase, use_de_ghost) in [(1, false), (2, true)] {
            info!(
                "--- Phase {}: pattern -> blank, de_ghost={} ---",
                phase, use_de_ghost
            );
            for cycle in 1..=CYCLES_PER_PHASE {
                info!(
                    "[EPD] Phase {} cycle {}/{}: writing bar pattern...",
                    phase, cycle, CYCLES_PER_PHASE
                );
                if let Err(_e) = driver
                    .write_frame(ColorChannel::Color7(0), pattern_buf)
                    .await
                {
                    error!("[EPD] write_frame (pattern) failed");
                } else if let Err(_e) = driver.refresh(&mut delay).await {
                    error!("[EPD] refresh (pattern) failed");
                } else {
                    info!("[EPD] Pattern shown. Holding {}s...", HOLD_SECS);
                }
                Timer::after(Duration::from_secs(HOLD_SECS)).await;

                if use_de_ghost {
                    info!("[EPD] Running de_ghost()...");
                    if let Err(_e) = driver.de_ghost(&mut delay).await {
                        error!("[EPD] de_ghost failed");
                    }
                }

                info!(
                    "[EPD] Phase {} cycle {}/{}: swapping to blank white (inspect this one)...",
                    phase, cycle, CYCLES_PER_PHASE
                );
                if let Err(_e) = driver
                    .clear_frame(ColorChannel::Color7(0), white_packed)
                    .await
                {
                    error!("[EPD] clear_frame (blank) failed");
                } else if let Err(_e) = driver.refresh(&mut delay).await {
                    error!("[EPD] refresh (blank) failed");
                } else {
                    info!("[EPD] Blank shown. Holding {}s...", HOLD_SECS);
                }
                Timer::after(Duration::from_secs(HOLD_SECS)).await;
            }
        }

        info!("[EPD] De-ghost comparison complete. Putting display into deep sleep.");
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
