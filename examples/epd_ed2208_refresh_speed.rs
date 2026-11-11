//! # ED2208 refresh speed with `Ed2208Controller::with_refresh_speed` (async)
//!
//! Shows the selectable refresh speed in `epdsi`'s `Ed2208Controller` on the Seeed Studio
//! reTerminal E1002 (`GDEP073E01`, Spectra 6). The controller writes its PLL control register
//! (`0x30`) during `init`, and [`Ed2208RefreshSpeed`] picks the value:
//!
//! | Setting | Refresh | Colour |
//! | :--- | ---: | :--- |
//! | `Normal` (default) | about 28 s | best |
//! | `Medium` | about 22 s | blue and green slightly duller |
//! | `Fast` | about 14 s | blue clearly greyer, green olive, red a little dimmer |
//!
//! ```ignore
//! let controller = Ed2208Controller::new(GDEP073E01::WIDTH, GDEP073E01::HEIGHT)
//!     .with_refresh_speed(Ed2208RefreshSpeed::Medium);
//! ```
//!
//! The example draws the same six-bar swatch (black, white, yellow, red, blue, green) once at
//! each speed, then once more at `Normal`, and logs the init and refresh time of each stage. The
//! controller is swapped through `driver.controller_mut()` so all four stages run in one boot.
//! Most programs build the controller once and never change it.
//!
//! Each stage holds the image for 10 seconds so the colour difference can be compared by eye.
//! Power-cycle the board before running and let the run finish. See `AGENTS.md`.
//!
//! ## Run
//! ```bash
//! cargo run --release --example epd_ed2208_refresh_speed
//! ```

#![no_std]
#![no_main]

use core::cell::RefCell;
use defmt::{error, info};
use embassy_time::{Delay, Duration, Instant, Timer};
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
use epdsi::controllers::{Ed2208Controller, Ed2208RefreshSpeed};
use epdsi::driver::EpdBuilder;
use epdsi::panels::GDEP073E01;
use epdsi::traits::EpdPanel;

esp_bootloader_esp_idf::esp_app_desc!();

const WIDTH: usize = 800;
const HEIGHT: usize = 480;
/// Seconds to hold each result before the next stage.
const HOLD_SECS: u64 = 10;
/// Bar width in pixels. Even, so both nibbles of a byte share a colour.
const BAR_PX: usize = 134;
/// Palette indexes: black, white, yellow, red, blue, green.
const BARS: [u8; 6] = [0x00, 0x01, 0x02, 0x03, 0x05, 0x06];

const DTM: u8 = 0x10;
/// Stages: name and refresh speed. The last repeats the first, as the control.
const STAGES: [(&str, Ed2208RefreshSpeed); 4] = [
    ("Normal", Ed2208RefreshSpeed::Normal),
    ("Medium", Ed2208RefreshSpeed::Medium),
    ("Fast", Ed2208RefreshSpeed::Fast),
    ("Normal (control)", Ed2208RefreshSpeed::Normal),
];

/// `RefCell`-shared async `SpiDevice`, used instead of `embedded_hal_bus::spi::ExclusiveDevice`.
/// `ExclusiveDevice`'s async impl silently corrupted every refresh on this target; see
/// `epd_ed2208_demo.rs` for the full note.
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

/// Writes the six-bar swatch one row at a time, so no 192 KB frame buffer is needed.
macro_rules! write_swatch {
    ($bus:expr) => {{
        let bus = $bus;
        let mut row = [0u8; WIDTH / 2];
        for (i, byte) in row.iter_mut().enumerate() {
            let bar = ((i * 2) / BAR_PX).min(BARS.len() - 1);
            let c = BARS[bar];
            *byte = (c << 4) | c;
        }
        bus.send_command(DTM).await?;
        for _ in 0..HEIGHT {
            bus.send_data(&row).await?;
        }
    }};
}

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("[E1002] ED2208 with_refresh_speed A/B (async)");

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
    let mut delay = Delay;

    for (n, (name, speed)) in STAGES.into_iter().enumerate() {
        let stage = n + 1;
        info!(
            "[stage {}] speed {}: init, write swatch, refresh",
            stage, name
        );
        let t0 = Instant::now();

        let init: Result<(), epdsi::bus::EpdBusError<_, _, _, _>> = async {
            // PLL is written by `init` itself, before `PON`, through the shipped setter.
            *driver.controller_mut() = Ed2208Controller::new(GDEP073E01::WIDTH, GDEP073E01::HEIGHT)
                .with_refresh_speed(speed);
            driver.init(&mut delay).await?;
            Ok(())
        }
        .await;
        if init.is_err() {
            error!("[stage {}] init failed", stage);
            break;
        }
        let t_init = t0.elapsed().as_millis();

        let wrote: Result<(), epdsi::bus::EpdBusError<_, _, _, _>> = async {
            write_swatch!(driver.bus_mut());
            Ok(())
        }
        .await;
        if wrote.is_err() {
            error!("[stage {}] swatch write failed", stage);
            break;
        }

        let t1 = Instant::now();
        if driver.refresh(&mut delay).await.is_err() {
            error!("[stage {}] refresh failed", stage);
            break;
        }
        info!(
            "[stage {}] speed {}: init {} ms, refresh {} ms",
            stage,
            name,
            t_init,
            t1.elapsed().as_millis()
        );
        Timer::after(Duration::from_secs(HOLD_SECS)).await;
    }

    let _ = driver.sleep(&mut delay).await;
    info!("[done] panel asleep. Power-cycle before the next run.");
    loop {
        Timer::after(Duration::from_secs(3600)).await;
    }
}

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", panic_info);
    loop {}
}
