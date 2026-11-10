//! Shared by the `mandelbrot`, `news`, `quotables` and `weather_station` examples: the panel (here), a
//! 6-colour `embedded-graphics` frame buffer and text helpers (in [`gfx`], hardware-free and host-tested)
//! and Wi-Fi, SNTP and HTTPS (in [`net`]).
//!
//! Each example pulls this in with `mod common;`. Cargo does not treat `examples/common/` as an
//! example of its own, so it is only built as part of those four.
//!
//! The panel setup is the one of `epd_ed2208_demo`, including its `RefCell`-based SPI device (see the
//! note in that file for why `ExclusiveDevice` is not used). The frame buffer lives in the heap, in
//! PSRAM, because the Wi-Fi stack and the TLS buffers leave too little internal RAM for 192 KB.
#![allow(dead_code)]

extern crate alloc;

pub mod gfx;
pub mod icons;
pub mod net;

#[allow(unused_imports)]
pub use gfx::*;

use core::cell::RefCell;
use defmt::{error, info};
use embassy_time::Delay;
use embedded_hal::digital::OutputPin;
use embedded_hal::spi::{ErrorType, Operation};
use embedded_hal_async::delay::DelayNs as AsyncDelayNs;
use embedded_hal_async::spi::{SpiBus as AsyncSpiBus, SpiDevice as AsyncSpiDevice};
use embedded_hal_bus::spi::DeviceError;
use epdsi::SpiBusWrapper;
use epdsi::controllers::Ed2208Controller;
use epdsi::driver::{EpdBuilder, EpdDriver};
use epdsi::panels::GDEP073E01;
use epdsi::traits::{ColorChannel, EpdPanel};
use esp_hal::Async;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::peripherals::{
    GPIO7, GPIO8, GPIO9, GPIO10, GPIO11, GPIO12, GPIO13, GPIO39, GPIO40, I2C1, PSRAM, SPI2,
};
use esp_hal::rtc_cntl::{SocResetReason, reset_reason};
use esp_hal::spi::Mode as SpiMode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::system::Cpu;
use static_cell::StaticCell;

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", defmt::Display2Format(panic_info));
    loop {}
}

/// Parks the CPU after an unrecoverable error, so the log stays readable.
pub async fn halt() -> ! {
    loop {
        embassy_time::Timer::after(embassy_time::Duration::from_secs(60)).await;
    }
}

/// True when this boot follows a [`wait_then_reset`], not a power-on or a flash. A refresh that fails then
/// leaves the panel as it is, so the last good picture stays. Only the first run after power-on draws the error.
pub fn after_loop_reset() -> bool {
    reset_reason(Cpu::ProCpu) == Some(SocResetReason::CoreSw)
}

/// True when USB power is present, false on battery alone, `None` when the charger does not answer. Reads the
/// power-good bit (bit 2 of REG08) of the SY6974B charger on its own I2C bus, SDA GPIO39 and SCL GPIO40. There is
/// no VBUS pin. Checked on the board with `charger`: 1 on USB, 0 on battery.
pub fn on_usb(i2c1: I2C1<'static>, sda: GPIO39<'static>, scl: GPIO40<'static>) -> Option<bool> {
    const CHARGER: u8 = 0x6B;
    const REG_STATUS: u8 = 0x08;
    let mut i2c = I2c::new(i2c1, I2cConfig::default())
        .ok()?
        .with_sda(sda)
        .with_scl(scl);
    let mut status = [0u8];
    i2c.write_read(CHARGER, &[REG_STATUS], &mut status).ok()?;
    Some(status[0] & 0b100 != 0)
}

/// Waits `minutes` awake, then resets the chip in software, so `main` runs again from the start. Do not call
/// `Rtc::sleep_deep` or `Rtc::sleep_light` here. With esp-hal 1.1, a sleep followed by Wi-Fi makes every later
/// panel refresh draw noise until the power is removed (see `epd_ed2208_wake`). An awake wait does not.
pub async fn wait_then_reset(minutes: u64) -> ! {
    info!("next refresh in {} min", minutes);
    embassy_time::Timer::after(embassy_time::Duration::from_secs(minutes * 60)).await;
    esp_hal::system::software_reset()
}

/// Maps the 8 MB of PSRAM into the heap. Allocations that do not fit the internal regions land there.
pub fn add_psram_to_heap(psram: PSRAM<'static>) {
    use esp_alloc::{HEAP, HeapRegion, MemoryCapability};
    use esp_hal::psram::{Psram, PsramConfig};
    let psram = Psram::new(psram, PsramConfig::default());
    let (start, size) = psram.raw_parts();
    unsafe {
        HEAP.add_region(HeapRegion::new(
            start,
            size,
            MemoryCapability::External.into(),
        ))
    };
}

// =====================================================================
// The panel
// =====================================================================

/// `RefCell`-shared async `SpiDevice`; see the module doc of `epd_ed2208_demo`.
pub struct AsyncRefCellDevice<'a, BUS, CS, D> {
    bus: &'a RefCell<BUS>,
    cs: CS,
    delay: D,
}

impl<'a, BUS, CS, D> AsyncRefCellDevice<'a, BUS, CS, D>
where
    CS: OutputPin,
{
    pub fn new(bus: &'a RefCell<BUS>, mut cs: CS, delay: D) -> Result<Self, CS::Error> {
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
    // One async consumer on this bus, so no concurrent borrower to conflict with.
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

type Device = AsyncRefCellDevice<'static, Spi<'static, Async>, Output<'static>, Delay>;
type Bus = SpiBusWrapper<Device, Output<'static>, Output<'static>, Input<'static>>;

/// The ED2208 driver on the E1002's pins: SCK 7, MISO 8, MOSI 9, CS 10, DC 11, RES 12, BUSY 13.
pub struct Epd {
    driver: EpdDriver<Bus, Ed2208Controller, GDEP073E01>,
}

impl Epd {
    /// Sets up SPI and the control pins and initialises the controller. Call once.
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        spi2: SPI2<'static>,
        sck: GPIO7<'static>,
        miso: GPIO8<'static>,
        mosi: GPIO9<'static>,
        cs: GPIO10<'static>,
        dc: GPIO11<'static>,
        rst: GPIO12<'static>,
        busy: GPIO13<'static>,
    ) -> Option<Self> {
        static SPI_BUS: StaticCell<RefCell<Spi<'static, Async>>> = StaticCell::new();
        let spi = Spi::new(
            spi2,
            SpiConfig::default()
                .with_frequency(esp_hal::time::Rate::from_mhz(10))
                .with_mode(SpiMode::_0),
        )
        .ok()?
        .with_sck(sck)
        .with_miso(miso)
        .with_mosi(mosi)
        .into_async();
        let bus_cell: &'static RefCell<_> = SPI_BUS.init(RefCell::new(spi));

        let cs = Output::new(cs, Level::High, OutputConfig::default());
        let dc = Output::new(dc, Level::Low, OutputConfig::default());
        let rst = Output::new(rst, Level::High, OutputConfig::default());
        let busy = Input::new(busy, InputConfig::default().with_pull(Pull::Up));

        let device = AsyncRefCellDevice::new(bus_cell, cs, Delay).ok()?;
        let bus = SpiBusWrapper::new(device, dc, rst, busy);
        let controller = Ed2208Controller::new(GDEP073E01::WIDTH, GDEP073E01::HEIGHT);
        let mut driver = EpdBuilder::<_, GDEP073E01>::new(controller).build(bus);

        info!("[EPD] Initializing ED2208 controller hardware...");
        if driver.init(&mut Delay).await.is_err() {
            error!("[EPD] Driver initialization failed!");
            return None;
        }
        Some(Self { driver })
    }

    /// Writes the frame, runs the full refresh (about 25 to 30 s on this panel) and puts the
    /// controller to sleep. The panel keeps the picture without power.
    pub async fn show(&mut self, frame: &[u8]) {
        info!("[EPD] Writing frame...");
        let t0 = embassy_time::Instant::now();
        if self
            .driver
            .write_frame(ColorChannel::Color7(0), frame)
            .await
            .is_err()
        {
            error!("[EPD] Write frame failed");
            return;
        }
        info!("[EPD] Written in {} ms", t0.elapsed().as_millis());
        info!("[EPD] Refreshing display (~25-30s)...");
        let t1 = embassy_time::Instant::now();
        if self.driver.refresh(&mut Delay).await.is_err() {
            error!("[EPD] Refresh failed");
            return;
        }
        info!("[EPD] Refresh complete in {} ms.", t1.elapsed().as_millis());
        let _ = self.driver.sleep(&mut Delay).await;
    }
}
