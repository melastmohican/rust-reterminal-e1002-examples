//! # ED2208 vendor `EPD_init_fast` A/B (async), throwaway bench example
//!
//! Backlog item `ed2208-vendor-fast-init`. Good Display's `GDEP073E01` demo ships two inits. This
//! example runs the same six-bar swatch through the current `epdsi` init and through the vendor's
//! `EPD_init_fast` (`Display_EPD_W21.cpp`, nine writes differ), so the two can be compared on
//! refresh time and colour.
//!
//! ## Stages
//! 1. **Plain init** (`driver.init`), swatch, refresh. Hold for a photo.
//! 2. **Vendor fast init** (raw commands over `driver.bus_mut()`), same swatch, refresh. Hold for a photo.
//! 3. **Plain init again**, as the control, so a difference from stage 1 shows an order effect.
//!
//! Each refresh is timed and logged. The swatch is six vertical bars: black, white, yellow, red,
//! blue, green. Photograph each hold with the `capture-display-image` skill and compare saturation.
//!
//! **Power-cycle the board first and do not interrupt the run.** See `AGENTS.md`. Judge the panel,
//! not the log: a refresh that finishes in seconds means the display was not driven.
//!
//! ## Run
//! ```bash
//! cargo run --release --example epd_ed2208_fast_init
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
use epdsi::controllers::Ed2208Controller;
use epdsi::driver::EpdBuilder;
use epdsi::panels::GDEP073E01;
use epdsi::traits::EpdPanel;

esp_bootloader_esp_idf::esp_app_desc!();

const WIDTH: usize = 800;
const HEIGHT: usize = 480;
/// Seconds to hold each result while the photo is taken.
const HOLD_SECS: u64 = 30;
/// Bar width in pixels. Even, so both nibbles of a byte share a colour.
const BAR_PX: usize = 134;
/// Palette indexes: black, white, yellow, red, blue, green.
const BARS: [u8; 6] = [0x00, 0x01, 0x02, 0x03, 0x05, 0x06];

// Command bytes from the vendor demo's defines.
const CMDH: u8 = 0xAA;
const PWRR: u8 = 0x01;
const PSR: u8 = 0x00;
const POFS: u8 = 0x03;
const BTST1: u8 = 0x05;
const BTST2: u8 = 0x06;
const BTST3: u8 = 0x08;
const IPC: u8 = 0x13;
const PLL: u8 = 0x30;
const TSE: u8 = 0x41;
const CDI: u8 = 0x50;
const TCON: u8 = 0x60;
const TRES: u8 = 0x61;
const VDCS: u8 = 0x82;
const T_VDCS: u8 = 0x84;
const AGID: u8 = 0x86;
const PWS: u8 = 0xE3;
const CCSET: u8 = 0xE0;
const TSSET: u8 = 0xE6;
const POWER_ON: u8 = 0x04;
const DTM: u8 = 0x10;

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

/// The vendor `EPD_init_fast` sequence, byte for byte from `Display_EPD_W21.cpp:86-178`, ending in
/// `PON` and a busy wait. Nine writes differ from the current `epdsi` init: PWRR (six bytes), BTST2
/// `6F 1F 16 25`, PLL `02`, and the added IPC, TSE, VDCS, AGID, CCSET and TSSET.
macro_rules! vendor_fast_init {
    ($bus:expr, $delay:expr) => {{
        let bus = $bus;
        bus.hard_reset($delay, 20).await?;
        bus.send_command_with_data(CMDH, &[0x49, 0x55, 0x20, 0x08, 0x09, 0x18])
            .await?;
        bus.send_command_with_data(PWRR, &[0x3F, 0x00, 0x32, 0x2A, 0x0E, 0x2A])
            .await?;
        bus.send_command_with_data(PSR, &[0x5F, 0x69]).await?;
        bus.send_command_with_data(POFS, &[0x00, 0x54, 0x00, 0x44])
            .await?;
        bus.send_command_with_data(BTST1, &[0x40, 0x1F, 0x1F, 0x2C])
            .await?;
        bus.send_command_with_data(BTST2, &[0x6F, 0x1F, 0x16, 0x25])
            .await?;
        bus.send_command_with_data(BTST3, &[0x6F, 0x1F, 0x1F, 0x22])
            .await?;
        bus.send_command_with_data(IPC, &[0x00, 0x04]).await?;
        bus.send_command_with_data(PLL, &[0x02]).await?;
        bus.send_command_with_data(TSE, &[0x00]).await?;
        bus.send_command_with_data(CDI, &[0x3F]).await?;
        bus.send_command_with_data(TCON, &[0x02, 0x00]).await?;
        bus.send_command_with_data(TRES, &[0x03, 0x20, 0x01, 0xE0])
            .await?;
        bus.send_command_with_data(VDCS, &[0x1E]).await?;
        bus.send_command_with_data(T_VDCS, &[0x01]).await?;
        bus.send_command_with_data(AGID, &[0x00]).await?;
        bus.send_command_with_data(PWS, &[0x2F]).await?;
        bus.send_command_with_data(CCSET, &[0x00]).await?;
        bus.send_command_with_data(TSSET, &[0x00]).await?;
        bus.send_command(POWER_ON).await?;
        bus.wait_busy_with_delay($delay, false).await?;
    }};
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

    info!("[E1002] ED2208 vendor fast-init A/B (async)");

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

    // Stage order: plain, vendor fast, plain again (control).
    for (stage, name) in [
        (1u8, "plain init"),
        (2, "vendor EPD_init_fast"),
        (3, "plain init (control)"),
    ] {
        info!(
            "[stage {}] {}: init, write swatch, refresh (~35 s)",
            stage, name
        );
        let t0 = Instant::now();

        let init: Result<(), epdsi::bus::EpdBusError<_, _, _, _>> = async {
            if stage == 2 {
                vendor_fast_init!(driver.bus_mut(), &mut delay);
            } else {
                driver.init(&mut delay).await?;
            }
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
            "[stage {}] {}: init {} ms, refresh {} ms. PHOTO NOW ({} s hold).",
            stage,
            name,
            t_init,
            t1.elapsed().as_millis(),
            HOLD_SECS
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
