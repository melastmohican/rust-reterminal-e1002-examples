//! # Power probe: reset reason, stage and lowest battery voltage on the LED (reTerminal E1002)
//!
//! Finds out why a boot on battery goes wrong. The serial link is the USB cable, so on battery there is no
//! log. The user LED (GPIO6, active-low) reports instead, in a loop that repeats every 8 s from the start.
//!
//! The firmware runs the same steps as `weather_station` (Wi-Fi, then a panel refresh) while a task reads the
//! battery every 10 ms and keeps the lowest value. The LED shows four groups of blinks, 1.5 s apart:
//!
//! | Group | Blinks | Meaning |
//! |---|---|---|
//! | 1: reset reason | 1 / 2 / 3 / 4 / 5 | power-on / software / **brownout** / watchdog / other |
//! | 2: stage reached | 1 / 2 / 3 / 4 / 5 / 6 | booted / Wi-Fi joined / Wi-Fi failed / refresh returned / panel init done / panel init failed |
//! | 3: lowest battery | N | lowest battery voltage so far is 3.0 V + N x 0.1 V (0 blinks: below 3.1 V) |
//! | 4: refresh time | N | the refresh call took N x 5 s (a good refresh is 28 s, so 5 or 6 blinks; 0 means it was not run or failed at once) |
//!
//! Group 2 stops at 1 or 2 if the board resets in the middle of Wi-Fi or the refresh, and the next boot then
//! starts again and reports group 1.
//!
//! ## Result so far (10 Nov 2026)
//!
//! On battery alone the LED reports reset reason 1 (no brownout), stage 4, a lowest battery of 3.8 V to 3.9 V and a
//! refresh of 25 to 29 s, the same as on USB, yet the panel shows noise. `WIFI = false` gives the same noise, so
//! Wi-Fi is not the cause. The cause is not found; the Arduino GxEPD2 bar test is clean on battery.
//!
//! ## Run
//!
//!
//! ```bash
//! cp secret.rs.example secret.rs   # then edit it, with the Wi-Fi name and password
//! cargo run --release --example power_probe
//! ```
//!
//! Flash it, unplug USB, switch the board off and on, and watch the LED for about 2 minutes.

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use common::{EpdBuffer, net};
use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use defmt::info;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::Blocking;
use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, AdcPin, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::peripherals::{ADC1, GPIO1};
use esp_hal::rtc_cntl::{SocResetReason, reset_reason};
use esp_hal::system::Cpu;
use esp_println as _;

esp_bootloader_esp_idf::esp_app_desc!();

mod secret {
    include!("../secret.rs");
}

/// False skips Wi-Fi, to tell a Wi-Fi problem on battery from a panel supply problem.
const WIFI: bool = true;

static STAGE: AtomicU8 = AtomicU8::new(1);
/// Seconds the refresh call took, 0 until it returns.
static REFRESH_SECS: AtomicU32 = AtomicU32::new(0);
/// Lowest battery voltage seen, in mV. `u32::MAX` until the first sample.
static MIN_MV: AtomicU32 = AtomicU32::new(u32::MAX);

type BattAdc = Adc<'static, ADC1<'static>, Blocking>;
type BattPin = AdcPin<GPIO1<'static>, ADC1<'static>, AdcCalCurve<ADC1<'static>>>;

async fn blink(led: &mut Output<'_>, times: u32, on_ms: u64, off_ms: u64) {
    for _ in 0..times {
        led.set_low();
        Timer::after(Duration::from_millis(on_ms)).await;
        led.set_high();
        Timer::after(Duration::from_millis(off_ms)).await;
    }
}

/// Blink count for the reset reason, as in the table of the module doc.
fn reason_code() -> u32 {
    match reset_reason(Cpu::ProCpu) {
        Some(SocResetReason::ChipPowerOn) => 1,
        Some(SocResetReason::CoreSw) => 2,
        Some(SocResetReason::SysBrownOut) => 3,
        Some(
            SocResetReason::CoreMwdt0
            | SocResetReason::CoreMwdt1
            | SocResetReason::CoreRtcWdt
            | SocResetReason::CpuMwdt0
            | SocResetReason::CpuMwdt1
            | SocResetReason::CpuRtcWdt
            | SocResetReason::SysRtcWdt
            | SocResetReason::SysSuperWdt,
        ) => 4,
        _ => 5,
    }
}

/// Keeps the battery divider on and records the lowest battery voltage.
#[embassy_executor::task]
async fn sample_battery(mut adc: BattAdc, mut pin: BattPin, _enable: Output<'static>) {
    loop {
        let mv = adc.read_blocking(&mut pin) as u32 * 2;
        MIN_MV.fetch_min(mv, Ordering::Relaxed);
        Timer::after(Duration::from_millis(10)).await;
    }
}

/// Repeats the three groups of blinks, as in the table of the module doc.
#[embassy_executor::task]
async fn report(mut led: Output<'static>) {
    let reason = reason_code();
    info!("[probe] reset reason code {}", reason);
    loop {
        blink(&mut led, reason, 200, 300).await;
        Timer::after(Duration::from_millis(1500)).await;
        blink(&mut led, STAGE.load(Ordering::Relaxed) as u32, 200, 300).await;
        Timer::after(Duration::from_millis(1500)).await;
        let min_mv = MIN_MV.load(Ordering::Relaxed);
        let tenths = if min_mv == u32::MAX {
            0
        } else {
            (min_mv / 100).saturating_sub(30)
        };
        info!("[probe] lowest battery {} mV", min_mv);
        blink(&mut led, tenths, 150, 250).await;
        Timer::after(Duration::from_millis(1500)).await;
        blink(&mut led, REFRESH_SECS.load(Ordering::Relaxed) / 5, 150, 250).await;
        Timer::after(Duration::from_secs(8)).await;
    }
}

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    // The radio wants internal RAM.
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);
    common::add_psram_to_heap(p.PSRAM);
    let timg0 = esp_hal::timer::timg::TimerGroup::new(p.TIMG0);
    let sw_interrupt = esp_hal::interrupt::software::SoftwareInterruptControl::new(p.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    // Battery divider on and sampled all the time.
    let batt_en = Output::new(p.GPIO21, Level::High, OutputConfig::default());
    let mut adc_config = AdcConfig::new();
    let batt_pin = adc_config.enable_pin_with_cal::<_, AdcCalCurve<_>>(p.GPIO1, Attenuation::_11dB);
    let adc = Adc::new(p.ADC1, adc_config);
    Timer::after(Duration::from_millis(20)).await;
    spawner.spawn(sample_battery(adc, batt_pin, batt_en).unwrap());

    let led = Output::new(p.GPIO6, Level::High, OutputConfig::default());
    spawner.spawn(report(led).unwrap());

    let mut frame = common::new_frame();
    let message = if !WIFI {
        "Power probe: no Wi-Fi"
    } else {
        match net::connect(&spawner, p.WIFI, secret::WIFI_SSID, secret::WIFI_PASSWORD).await {
            Some(_wifi) => {
                STAGE.store(2, Ordering::Relaxed);
                "Power probe: Wi-Fi ok"
            }
            None => {
                STAGE.store(3, Ordering::Relaxed);
                "Power probe: Wi-Fi failed"
            }
        }
    };
    common::draw_message(&mut EpdBuffer::new(&mut frame), message);

    match common::Epd::new(
        p.SPI2, p.GPIO7, p.GPIO8, p.GPIO9, p.GPIO10, p.GPIO11, p.GPIO12, p.GPIO13,
    )
    .await
    {
        Some(mut epd) => {
            STAGE.store(5, Ordering::Relaxed);
            let started = embassy_time::Instant::now();
            epd.show(&frame).await;
            REFRESH_SECS.store(started.elapsed().as_secs() as u32, Ordering::Relaxed);
            STAGE.store(4, Ordering::Relaxed);
        }
        None => STAGE.store(6, Ordering::Relaxed),
    }
    common::halt().await
}
