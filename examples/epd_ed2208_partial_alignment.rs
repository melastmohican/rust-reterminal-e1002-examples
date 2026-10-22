//! # ED2208 Partial Window Alignment Bench Test using `epdsi` (async)
//!
//! Settles `epdsi` backlog item `ed2208-partial-window-alignment`. Zephyr PR #121618 claims the
//! ED2208 reads an even number of bytes per row in a partial window, so a 386 px window
//! (193 bytes) shears one byte per row while 388 px (194 bytes) stays clean. `epdsi` 0.9.3 and
//! earlier accepted any even `x` and `w`, which is what lets this example run all three windows.
//!
//! **Bench result (22 Oct 2026, `GDEP073E01`):** `x=4 w=386` sheared, `x=4 w=388` was clean and
//! `x=2 w=388` sheared, so the multiple-of-4 rule holds for both `x` and `w`. Newer `epdsi`
//! releases reject bands 1 and 3 with `InvalidPartialWindowAlignment`, so against a release
//! that has the check this example logs an error for those two bands and only draws band 2.
//! Pin `epdsi` to 0.9.3 or earlier to reproduce the shear.
//!
//! The panel is filled yellow, then three 24-row bands of black and white vertical stripes
//! (4 px per stripe) are written, one partial refresh each. Vertical stripes cannot hide a
//! stride error: a sheared window shows slanted or broken stripes, a clean one shows straight
//! stripes whose edges line up with the same x positions in every band.
//!
//! | Band | y | x | w | Bytes per row | Question |
//! |---|---|---|---|---|---|
//! | 1 | 40 | 4 | 386 | 193 | Does an odd byte count shear? |
//! | 2 | 200 | 4 | 388 | 194 | Control, expected clean |
//! | 3 | 360 | 2 | 388 | 194 | Does `x` = 2 (not a multiple of 4) shear? |
//!
//! Each partial refresh takes about 35 s, so the run is about two minutes. Photograph the panel
//! after each band while the log says "Holding". Expect the yellow outside each window to fade,
//! which is the known ED2208 limitation and not part of this test.
//!
//! See `epd_ed2208_demo.rs`'s module doc for why this uses a local `AsyncRefCellDevice` instead of
//! `embedded_hal_bus::spi::ExclusiveDevice` on this target.
//!
//! ## Display Specification
//! - **Panel:** Good Display GDEP073E01 (7.3" 800x480 6-Color ACeP / Spectra 6 e-Paper display)
//! - **Controller IC:** ED2208 (via local `epdsi` driver crate)
//! - **Host Board:** Seeed Studio reTerminal E1002 (XIAO ESP32-S3)
//!
//! ## Run
//! ```bash
//! cargo run --release --example epd_ed2208_partial_alignment
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

/// One bench window: a band of vertical stripes written at (`x`, `y`) with width `w`.
const BANDS: [(u32, u32, u32, &str); 3] = [
    (4, 40, 386, "x=4 w=386 (193 bytes/row)"),
    (4, 200, 388, "x=4 w=388 (194 bytes/row)"),
    (2, 360, 388, "x=2 w=388 (194 bytes/row)"),
];
const BAND_H: u32 = 24;
const MAX_ROW_BYTES: usize = 194;

/// Hold time after each update (seconds), long enough to photograph each state.
const HOLD_SECS: u64 = 10;

/// Fill `row` with black/white stripes, 4 px wide, phase locked to the absolute x position so a
/// correct window lines up with the panel's own x axis. `x` must be even.
fn fill_stripe_row(row: &mut [u8], x: u32) {
    for (i, byte) in row.iter_mut().enumerate() {
        let px = x + 2 * i as u32;
        let color = if (px / 4).is_multiple_of(2) {
            SevenColor::Black
        } else {
            SevenColor::White
        };
        *byte = SevenColor::pack(color, color);
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
    info!("[E1002] epdsi ED2208 / GDEP073E01 Partial Alignment Bench (async)");
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

        info!("[EPD] Filling panel yellow (full refresh, establishes background)...");
        let yellow_packed = SevenColor::pack(SevenColor::Yellow, SevenColor::Yellow);
        if let Err(_e) = driver
            .clear_frame(ColorChannel::Color7(0), yellow_packed)
            .await
        {
            error!("[EPD] clear_frame (background) failed");
        } else if let Err(_e) = driver.refresh(&mut delay).await {
            error!("[EPD] refresh (background) failed");
        }
        Timer::after(Duration::from_secs(HOLD_SECS)).await;

        let mut buf = [0u8; MAX_ROW_BYTES * BAND_H as usize];
        for (x, y, w, name) in BANDS {
            info!("[EPD] Band {} ...", name);
            let row_bytes = (w / 2) as usize;
            let total = row_bytes * BAND_H as usize;
            for r in 0..BAND_H as usize {
                fill_stripe_row(&mut buf[r * row_bytes..(r + 1) * row_bytes], x);
            }

            if let Err(_e) = driver.set_window(x, y, x + w - 1, y + BAND_H - 1).await {
                error!("[EPD] set_window failed for {}", name);
                continue;
            }
            let (bus, controller) = driver.split_mut();
            if let Err(_e) = controller
                .write_frame(bus, ColorChannel::Color7(0), &buf[..total])
                .await
            {
                error!("[EPD] write_frame failed for {}", name);
                continue;
            }
            if let Err(_e) = controller
                .trigger_partial_refresh(bus, &mut delay, x, y, w, BAND_H)
                .await
            {
                error!("[EPD] trigger_partial_refresh failed for {}", name);
                continue;
            }
            info!(
                "[EPD] Band done: {}. Holding {}s, photograph now.",
                name, HOLD_SECS
            );
            Timer::after(Duration::from_secs(HOLD_SECS)).await;
        }

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
