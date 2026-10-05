//! # ED2208 Windowed Partial Refresh Demonstration using `epdsi` (async)
//!
//! Demonstrates `Ed2208Controller::trigger_partial_refresh()` (added for `epdsi` backlog item
//! `0i`), which refreshes only a caller-given window instead of `trigger_refresh`'s unconditional
//! full-panel widen. No existing example exercises this, since the capability is new.
//!
//! There is no speed benefit on this controller: full and partial refresh both take roughly the
//! same ~30-35s regardless of window size on real hardware.
//!
//! **Bench-confirmed real limitation:** the area outside the window visibly fades starting from
//! the very first partial refresh, not just after repeated ones. This is why Zephyr's own
//! `ed2208_gca` driver refuses to do partial refresh at all. Follow any partial refresh with a
//! prompt full one; don't chain them expecting the untouched area to hold.
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
//! 1. Fill the whole panel blue and refresh once (full refresh, establishes a fresh background).
//! 2. Cycle a 200x100 window through Red -> Green -> Yellow -> White -> Black, each one via
//!    `trigger_partial_refresh()`. Watch the blue background: expect it to visibly fade starting
//!    from the first partial update, not gradually over all five.
//! 3. One final plain `refresh()` while the window still shows black, for contrast: the *whole*
//!    panel refreshes and the background returns to a clean blue.
//!
//! ## Run
//! ```bash
//! cargo run --release --example epd_ed2208_partial_refresh
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
use epdsi::traits::{ColorChannel, EpdController, EpdPanel, SevenColor};

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

/// Window geometry: both start and width must be even (4bpp I4 format packs 2 pixels/byte).
const WIN_X: u32 = 300;
const WIN_Y: u32 = 190;
const WIN_W: u32 = 200;
const WIN_H: u32 = 100;

/// Hold time after each update (seconds), long enough to photograph each state.
const HOLD_SECS: u64 = 5;

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
    info!("[E1002] epdsi ED2208 / GDEP073E01 Partial Refresh Demo (async)");
    info!("============================================================");

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

        info!("[EPD] Filling panel blue (full refresh, establishes background)...");
        let blue_packed = SevenColor::pack(SevenColor::Blue, SevenColor::Blue);
        if let Err(_e) = driver
            .clear_frame(ColorChannel::Color7(0), blue_packed)
            .await
        {
            error!("[EPD] clear_frame (background) failed");
        } else if let Err(_e) = driver.refresh(&mut delay).await {
            error!("[EPD] refresh (background) failed");
        }
        Timer::after(Duration::from_secs(HOLD_SECS)).await;

        info!("--- Cycling the window through 5 colors via trigger_partial_refresh ---");
        const COLORS: [(SevenColor, &str); 5] = [
            (SevenColor::Red, "Red"),
            (SevenColor::Green, "Green"),
            (SevenColor::Yellow, "Yellow"),
            (SevenColor::White, "White"),
            (SevenColor::Black, "Black"),
        ];

        for (color, name) in COLORS {
            info!(
                "[EPD] Window -> {} (partial refresh; background should stay put)...",
                name
            );
            let packed = SevenColor::pack(color, color);
            let x_end = WIN_X + WIN_W - 1;
            let y_end = WIN_Y + WIN_H - 1;
            if let Err(_e) = driver.set_window(WIN_X, WIN_Y, x_end, y_end).await {
                error!("[EPD] set_window failed for {}", name);
                continue;
            }

            let (bus, controller) = driver.split_mut();
            let byte_count = (WIN_W * WIN_H / 2) as usize;
            if let Err(_e) = controller
                .write_frame_pattern(bus, ColorChannel::Color7(0), packed, byte_count)
                .await
            {
                error!("[EPD] write_frame_pattern failed for {}", name);
                continue;
            }
            if let Err(_e) = controller
                .trigger_partial_refresh(bus, &mut delay, WIN_X, WIN_Y, WIN_W, WIN_H)
                .await
            {
                error!("[EPD] trigger_partial_refresh failed for {}", name);
                continue;
            }
            info!("[EPD] Window is now {}. Holding {}s...", name, HOLD_SECS);
            Timer::after(Duration::from_secs(HOLD_SECS)).await;
        }

        info!("--- Comparison: one plain refresh() (whole panel flashes) ---");
        if let Err(_e) = driver.refresh(&mut delay).await {
            error!("[EPD] comparison refresh failed");
        }
        Timer::after(Duration::from_secs(HOLD_SECS)).await;

        // Restore the full-frame RAM window for any subsequent updates.
        if let Err(_e) = driver
            .set_window(0, 0, GDEP073E01::WIDTH - 1, GDEP073E01::HEIGHT - 1)
            .await
        {
            error!("[EPD] restoring full window failed");
        }

        info!("[EPD] Demo complete. Putting display into deep sleep.");
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
