//! # Passive Buzzer Chimes & Alerts Example (reTerminal E1002)
//!
//! Drives the onboard passive piezo buzzer on the Seeed Studio reTerminal E1002.
//!
//! The buzzer is connected to GPIO45 and is driven with square waves via the ESP32-S3's
//! LEDC (LED Control) hardware PWM peripheral, generating audio frequencies without CPU load.
//! Demonstrates an ascending 3-note startup chime, a double beep, and a frequency sweep alert.
//!
//! ## Hardware
//!
//! - **Board:** Seeed Studio reTerminal E1002 (XIAO ESP32-S3 MCU)
//! - **Peripherals:** Onboard Passive Piezo Buzzer
//!
//! ## Pin Mapping
//!
//! | Signal | GPIO | Peripheral | Notes |
//! |---|---|---|---|
//! | Buzzer | GPIO45 | LEDC LowSpeed Channel 0 | Driven by LEDC PWM square wave |
//!
//! ## Wiring Schematic
//!
//! ```text
//!            Seeed Studio reTerminal E1002 Carrier Board
//!          +-------------------------------------------------+
//!          | GPIO45 (LEDC PWM) --------[ Piezo Buzzer ]      |
//!          |                                  |              |
//!          |                                 GND             |
//!          +-------------------------------------------------+
//! ```
//!
//! ## Run
//!
//! ```bash
//! cargo run --example buzzer
//! ```

#![no_std]
#![no_main]

use defmt::info;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::ledc::channel::ChannelIFace;
use esp_hal::ledc::timer::TimerIFace;
use esp_hal::ledc::{LSGlobalClkSource, Ledc, LowSpeed, channel, timer};
use esp_println as _;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("[E1002] Buzzer demo (GPIO45)");

    let mut ledc = Ledc::new(peripherals.LEDC);
    ledc.set_global_slow_clock(LSGlobalClkSource::APBClk);

    let mut gpio45 = peripherals.GPIO45;

    loop {
        // ---- Boot chime: ascending three-note sequence ----
        info!("[buzz] Ascending chime");
        play_tone(&ledc, &mut gpio45, 440, 200).await;
        Timer::after(Duration::from_millis(50)).await;
        play_tone(&ledc, &mut gpio45, 550, 200).await;
        Timer::after(Duration::from_millis(50)).await;
        play_tone(&ledc, &mut gpio45, 660, 300).await;
        Timer::after(Duration::from_millis(300)).await;

        // ---- Short double-beep ----
        info!("[buzz] Double beep");
        play_tone(&ledc, &mut gpio45, 1000, 100).await;
        Timer::after(Duration::from_millis(100)).await;
        play_tone(&ledc, &mut gpio45, 1000, 100).await;
        Timer::after(Duration::from_millis(500)).await;

        // ---- Alert: fast low-to-high frequency sweep ----
        info!("[buzz] Alert sweep");
        let mut freq = 500;
        while freq <= 1500 {
            play_tone(&ledc, &mut gpio45, freq, 20).await;
            freq += 50;
        }
        Timer::after(Duration::from_millis(2000)).await;
    }
}

/// Helper function to configure LEDC PWM frequency and play a tone for `duration_ms`.
async fn play_tone<'a>(
    ledc: &'a Ledc<'a>,
    pin: &mut esp_hal::peripherals::GPIO45<'a>,
    freq_hz: u32,
    duration_ms: u64,
) {
    if freq_hz == 0 {
        Timer::after(Duration::from_millis(duration_ms)).await;
        return;
    }

    let mut lstimer0 = ledc.timer::<LowSpeed>(timer::Number::Timer0);
    lstimer0
        .configure(timer::config::Config {
            duty: timer::config::Duty::Duty10Bit,
            clock_source: timer::LSClockSource::APBClk,
            frequency: esp_hal::time::Rate::from_hz(freq_hz),
        })
        .unwrap();

    let mut channel0 = ledc.channel(channel::Number::Channel0, pin.reborrow());
    channel0
        .configure(channel::config::Config {
            timer: &lstimer0,
            duty_pct: 50,
            drive_mode: esp_hal::gpio::DriveMode::PushPull,
        })
        .unwrap();

    Timer::after(Duration::from_millis(duration_ms)).await;

    // Silence tone at end of duration
    let _ = channel0.set_duty(0);
}

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    defmt::error!("{}", panic_info);
    loop {}
}
