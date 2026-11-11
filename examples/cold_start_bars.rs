//! # Cold-start test program, Rust side (async)
//!
//! Twin of the Arduino sketch `ColdStartBars` (GxEPD2). Both do the same steps in the same order with the same
//! picture, LED and log format, so that the only differences are the panel driver (`epdsi`) and the runtime
//! (`esp-hal`). The plan is `.agents/notes/battery-boot-test-plan.md`: switch the board off for 20 s and on again,
//! on battery, and see whether the first picture is clean.
//!
//! Steps: 1 boot, 2 led proof, 3 wait 2 s, 4 panel init start, 5 panel init done, 6 draw start,
//! 7 draw done (frame written, refreshed, panel asleep), 8 idle.
//!
//! - **Picture:** six bars (black, white, yellow, red, blue, green) over 432 rows, then a white footer strip with
//!   `<program> <build> | reset <reason> | init <ms> | run <id>`. The run id is 4 hex digits from a floating ADC
//!   pin (GPIO2), different on every boot.
//! - **LED** (GPIO6, active low): three blinks after a power-on reset.
//! - **Log** (USB only): `[cold] program=... build=... step=N name t=MS`, `[cold] reset_reason=...` before step 1 and
//!   `[cold] run_id=XXXX` at step 3.
//! - No Wi-Fi, no PSRAM, SPI at 2 MHz, 240 MHz CPU.
//!
//! ## Run
//!
//! ```bash
//! cargo run --release --example cold_start_bars
//! ```

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use alloc::format;
use common::{Color6, EpdBuffer, HEIGHT, WIDTH};
use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::{
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};
use esp_hal::analog::adc::{Adc, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::rtc_cntl::{SocResetReason, reset_reason};
use esp_hal::system::Cpu;
use esp_println::println;
use u8g2_fonts::types::HorizontalAlignment;

esp_bootloader_esp_idf::esp_app_desc!();

const PROGRAM: &str = "Rust cold_start_bars";
const BUILD: &str = "b2";
const BAR_ROWS: u32 = 432;

/// One log line, in the format shared with the Arduino twin.
fn step(n: u8, name: &str) {
    println!(
        "[cold] program={} build={} step={} {} t={}",
        PROGRAM,
        BUILD,
        n,
        name,
        Instant::now().as_millis()
    );
}

fn reason_name() -> &'static str {
    match reset_reason(Cpu::ProCpu) {
        Some(SocResetReason::ChipPowerOn) => "power-on",
        Some(SocResetReason::CoreSw) => "software",
        _ => "other",
    }
}

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    // Internal RAM only, no PSRAM: the frame is 192 KB.
    esp_alloc::heap_allocator!(size: 210 * 1024);
    let timg0 = esp_hal::timer::timg::TimerGroup::new(p.TIMG0);
    let sw_interrupt = esp_hal::interrupt::software::SoftwareInterruptControl::new(p.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    let reason = reason_name();
    println!("[cold] reset_reason={}", reason);
    step(1, "boot");

    let mut led = Output::new(p.GPIO6, Level::High, OutputConfig::default());
    if reason == "power-on" {
        for _ in 0..3 {
            led.set_low();
            Timer::after(Duration::from_millis(200)).await;
            led.set_high();
            Timer::after(Duration::from_millis(300)).await;
        }
    }
    step(2, "led proof");

    Timer::after(Duration::from_secs(2)).await;
    step(3, "wait 2 s");

    // A floating pin gives a different reading on every boot.
    let mut adc_config = AdcConfig::new();
    let mut run_pin = adc_config.enable_pin(p.GPIO2, Attenuation::_11dB);
    let mut adc = Adc::new(p.ADC1, adc_config);
    let mut run_id: u16 = 0;
    for _ in 0..8 {
        let v: u16 = nb::block!(adc.read_oneshot(&mut run_pin)).unwrap_or(0);
        run_id = run_id.wrapping_mul(31).wrapping_add(v);
        Timer::after(Duration::from_millis(1)).await;
    }
    println!("[cold] run_id={:04x}", run_id);

    step(4, "panel init start");
    let Some(mut epd) = common::Epd::new_at(
        2, p.SPI2, p.GPIO7, p.GPIO8, p.GPIO9, p.GPIO10, p.GPIO11, p.GPIO12, p.GPIO13,
    )
    .await
    else {
        println!("[cold] panel init failed");
        common::halt().await
    };
    let init_ms = Instant::now().as_millis();
    step(5, "panel init done");

    let footer = format!(
        "{} {} | reset {} | init {} | run {:04x}",
        PROGRAM, BUILD, reason, init_ms, run_id
    );
    let mut frame = common::new_frame();
    {
        let mut target = EpdBuffer::new(&mut frame);
        target.clear(Color6::White);
        let colors = [
            Color6::Black,
            Color6::White,
            Color6::Yellow,
            Color6::Red,
            Color6::Blue,
            Color6::Green,
        ];
        for (i, color) in colors.into_iter().enumerate() {
            let x0 = (i * WIDTH / 6) as i32;
            let x1 = ((i + 1) * WIDTH / 6) as i32;
            let _ = Rectangle::new(Point::new(x0, 0), Size::new((x1 - x0) as u32, BAR_ROWS))
                .into_styled(PrimitiveStyle::with_fill(color))
                .draw(&mut target);
        }
        common::draw_text(
            &mut target,
            &common::SANS_9,
            &footer,
            8,
            HEIGHT as i32 - 14,
            HorizontalAlignment::Left,
            Color6::Black,
        );
    }

    step(6, "draw start");
    epd.show(&frame).await;
    step(7, "draw done");
    step(8, "idle");
    common::halt().await
}
