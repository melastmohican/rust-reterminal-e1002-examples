//! # ED2208 sleep and Wi-Fi bench (async), throwaway bench example
//!
//! Reproduces and bisects a fault found on 9 Nov 2026: after an `esp-hal` deep or light sleep, a Wi-Fi join
//! and fetch before the panel init makes every later refresh draw noise (brown or yellow speckle). A
//! software reset does not clear it; removing the power does. `weather_station` and `news` therefore wait
//! awake and reset instead of sleeping.
//!
//! Each boot draws six bars: black, white, yellow, red, blue, green after a power-on, and in the reverse
//! order after a sleep or a reset that follows one, so two screens can be told apart. Each refresh is timed
//! and logged. Wi-Fi needs `secret.rs`.
//!
//! ## Results (GDEP073E01 on the reTerminal E1002, 60 s sleep, photographed after each draw)
//!
//! | Variant | After the sleep |
//! | :--- | :--- |
//! | Arduino GxEPD2, deep sleep, no Wi-Fi | clean |
//! | `PSRAM_FRAME` false, deep sleep, no Wi-Fi | clean |
//! | `PSRAM_FRAME` true, deep sleep, no Wi-Fi | clean |
//! | `WIFI` true, deep sleep | noise |
//! | `WIFI` and `RESET_AFTER_WAKE`, deep sleep | noise |
//! | `WIFI` and `LIGHT_SLEEP` | noise |
//! | `weather_station` with an awake wait and a software reset, Wi-Fi each cycle | clean |
//!
//! Also tried and no help: holding the panel pads (RTC pad hold) through deep sleep. GPIO12 is both the panel
//! reset and its power switch (schematic v1.2, page 8), but the Arduino sketch lets it float in deep sleep and
//! draws clean, so that is not the cause. The cause itself is not found.
//!
//! ## Switches
//! - `PSRAM_FRAME`: build the swatch in a 192 KB PSRAM frame buffer and send it with `write_frame`, as
//!   `weather_station` does, instead of row by row.
//! - `WIFI`: join Wi-Fi and fetch a small page over TLS before the panel init, then drop Wi-Fi.
//! - `RESET_AFTER_WAKE`: on a timer wake-up, reset in software at once, before PSRAM or Wi-Fi are touched.
//! - `LIGHT_SLEEP`: light sleep and then a software reset, instead of deep sleep.
//!
//! The defaults reproduce the fault: Wi-Fi, then deep sleep.
//!
//! **Power-cycle the board first and do not interrupt the run.** See `AGENTS.md`. After a noisy run, power-cycle
//! again before judging anything.
//!
//! ## Run
//! ```bash
//! cargo run --release --example epd_ed2208_wake
//! ```
//! The USB serial log does not survive the sleep, so judge the panel, not the log.

#![no_std]
#![no_main]

extern crate alloc;

mod common;

mod secret {
    include!("../secret.rs");
}

use alloc::vec;
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
use esp_hal::rtc_cntl::{
    Rtc, SocResetReason, reset_reason, sleep::TimerWakeupSource, wakeup_cause,
};
use esp_hal::spi::Mode as SpiMode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::system::{Cpu, SleepSource};
use esp_println as _;

use epdsi::SpiBusWrapper;
use epdsi::controllers::Ed2208Controller;
use epdsi::driver::EpdBuilder;
use epdsi::panels::GDEP073E01;
use epdsi::traits::{ColorChannel, EpdPanel};

esp_bootloader_esp_idf::esp_app_desc!();

const WIDTH: usize = 800;
const HEIGHT: usize = 480;
/// Bar width in pixels. Even, so both nibbles of a byte share a colour.
const BAR_PX: usize = 134;
/// Palette indexes: black, white, yellow, red, blue, green.
const BARS: [u8; 6] = [0x00, 0x01, 0x02, 0x03, 0x05, 0x06];

const DTM: u8 = 0x10;
/// Build the swatch in a PSRAM frame buffer and send it with `write_frame`, instead of row by row.
const PSRAM_FRAME: bool = true;
/// Join Wi-Fi and fetch a page over TLS before the panel init, as `weather_station` does.
const WIFI: bool = true;
/// On a timer wake-up, reset in software at once, before PSRAM or Wi-Fi are touched.
const RESET_AFTER_WAKE: bool = false;
/// Light sleep, then a software reset, instead of deep sleep.
const LIGHT_SLEEP: bool = false;
/// Seconds of deep sleep between the two draws.
const SLEEP_SECS: u64 = 60;

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

/// Writes the six-bar swatch one row at a time, so no 192 KB frame buffer is needed. `$reverse` flips the
/// bar order.
macro_rules! write_swatch {
    ($bus:expr, $reverse:expr) => {{
        let bus = $bus;
        let mut row = [0u8; WIDTH / 2];
        for (i, byte) in row.iter_mut().enumerate() {
            let mut bar = ((i * 2) / BAR_PX).min(BARS.len() - 1);
            if $reverse {
                bar = BARS.len() - 1 - bar;
            }
            let c = BARS[bar];
            *byte = (c << 4) | c;
        }
        bus.send_command(DTM).await?;
        for _ in 0..HEIGHT {
            bus.send_data(&row).await?;
        }
    }};
}

/// Maps the 8 MB of PSRAM into the heap, as `common::add_psram_to_heap` does.
fn add_psram_to_heap(psram: esp_hal::peripherals::PSRAM<'static>) {
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

#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // `woke`: this draw is the second one, after a deep sleep, so the bars are reversed.
    let mut woke = matches!(wakeup_cause(), SleepSource::Timer);
    if RESET_AFTER_WAKE {
        if woke {
            info!("[wake] timer wake-up, software reset");
            esp_hal::system::software_reset();
        }
        woke = reset_reason(Cpu::ProCpu) == Some(SocResetReason::CoreSw);
    }
    // The same heap layout as `weather_station`: internal regions first, then the 8 MB of PSRAM.
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);
    if PSRAM_FRAME {
        add_psram_to_heap(peripherals.PSRAM);
    }

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("[E1002] ED2208 wake test (async)");

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

    info!("[wake] draw reversed: {}", woke);

    if WIFI {
        match common::net::connect(
            &spawner,
            peripherals.WIFI,
            secret::WIFI_SSID,
            secret::WIFI_PASSWORD,
        )
        .await
        {
            Some(wifi) => {
                let mut body = vec![0u8; 16 * 1024];
                let url = "https://api.open-meteo.com/v1/forecast?latitude=47.6101&longitude=-122.2015&current=temperature_2m";
                match common::net::https_get_retry(wifi.stack, url, "reTerminal-E1002", &mut body)
                    .await
                {
                    Ok(n) => info!("[wake] fetch ok, {} bytes", n),
                    Err(reason) => error!("[wake] fetch failed: {}", reason),
                }
            }
            None => error!("[wake] wifi failed"),
        }
        // `wifi` is dropped here, before the panel is touched.
    }

    let t0 = Instant::now();
    let drew: Result<(), epdsi::bus::EpdBusError<_, _, _, _>> = async {
        driver.init(&mut delay).await?;
        if PSRAM_FRAME {
            let mut frame = vec![0x11u8; WIDTH / 2 * HEIGHT];
            for y in 0..HEIGHT {
                for (i, byte) in frame[y * WIDTH / 2..(y + 1) * WIDTH / 2]
                    .iter_mut()
                    .enumerate()
                {
                    let mut bar = ((i * 2) / BAR_PX).min(BARS.len() - 1);
                    if woke {
                        bar = BARS.len() - 1 - bar;
                    }
                    let c = BARS[bar];
                    *byte = (c << 4) | c;
                }
            }
            // Read the buffer back and compare it, to see whether PSRAM kept what was written.
            let bad = frame
                .iter()
                .enumerate()
                .filter(|(i, b)| {
                    let mut bar = (((i % (WIDTH / 2)) * 2) / BAR_PX).min(BARS.len() - 1);
                    if woke {
                        bar = BARS.len() - 1 - bar;
                    }
                    **b != (BARS[bar] << 4) | BARS[bar]
                })
                .count();
            info!("[wake] PSRAM frame mismatches: {}", bad);
            driver.write_frame(ColorChannel::Color7(0), &frame).await?;
        } else {
            write_swatch!(driver.bus_mut(), woke);
        }
        Ok(())
    }
    .await;
    if drew.is_err() {
        error!("[wake] init or write failed");
        loop {
            Timer::after(Duration::from_secs(3600)).await;
        }
    }
    let t1 = Instant::now();
    if driver.refresh(&mut delay).await.is_err() {
        error!("[wake] refresh failed");
    } else {
        info!(
            "[wake] init+write {} ms, refresh {} ms",
            (t1 - t0).as_millis(),
            t1.elapsed().as_millis()
        );
    }
    let _ = driver.sleep(&mut delay).await;

    let timer = TimerWakeupSource::new(core::time::Duration::from_secs(SLEEP_SECS));
    let mut rtc = Rtc::new(peripherals.LPWR);
    if LIGHT_SLEEP {
        info!(
            "[wake] light sleep for {} s, then software reset",
            SLEEP_SECS
        );
        rtc.sleep_light(&[&timer]);
        esp_hal::system::software_reset()
    } else {
        info!("[wake] deep sleep for {} s", SLEEP_SECS);
        rtc.sleep_deep(&[&timer])
    }
}
