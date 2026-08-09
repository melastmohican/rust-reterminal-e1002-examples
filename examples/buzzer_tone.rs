//! # Imperial March Melody Score Player Example (reTerminal E1002)
//!
//! Plays the Imperial March (Darth Vader theme from Star Wars) on the onboard passive buzzer
//! of the Seeed Studio reTerminal E1002.
//!
//! Melody score format: pairs of `(frequency_hz, duration_divisor)`:
//! - **Divisor > 0:** Note duration = `whole_note_ms / divisor` (e.g. 4 = quarter note)
//! - **Divisor < 0:** Dotted note = `(whole_note_ms / |divisor|) * 1.5`
//! - **Frequency 0:** Rest / silence
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
//! cargo run --example buzzer_tone
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

/// Darth Vader theme (Imperial March) - Star Wars
/// Score format: (frequency_hz, divisor)
static MELODY: &[(u32, i32)] = &[
    (440, -4),
    (440, -4),
    (440, 16),
    (440, 16),
    (440, 16),
    (440, 16),
    (349, 8),
    (0, 8),
    (440, -4),
    (440, -4),
    (440, 16),
    (440, 16),
    (440, 16),
    (440, 16),
    (349, 8),
    (0, 8),
    (440, 4),
    (440, 4),
    (440, 4),
    (349, -8),
    (523, 16),
    (440, 4),
    (349, -8),
    (523, 16),
    (440, 2),
    (659, 4),
    (659, 4),
    (659, 4),
    (698, -8),
    (523, 16),
    (440, 4),
    (349, -8),
    (523, 16),
    (440, 2),
    (880, 4),
    (440, -8),
    (440, 16),
    (880, 4),
    (831, -8),
    (784, 16),
    (622, 16),
    (587, 16),
    (622, 8),
    (0, 8),
    (440, 8),
    (622, 4),
    (587, -8),
    (554, 16),
    (523, 16),
    (494, 16),
    (523, 16),
    (0, 8),
    (349, 8),
    (415, 4),
    (349, -8),
    (440, -16),
    (523, 4),
    (440, -8),
    (523, 16),
    (659, 2),
    (880, 4),
    (440, -8),
    (440, 16),
    (880, 4),
    (831, -8),
    (784, 16),
    (622, 16),
    (587, 16),
    (622, 8),
    (0, 8),
    (440, 8),
    (622, 4),
    (587, -8),
    (554, 16),
    (523, 16),
    (494, 16),
    (523, 16),
    (0, 8),
    (349, 8),
    (415, 4),
    (349, -8),
    (440, -16),
    (440, 4),
    (349, -8),
    (523, 16),
    (440, 2),
];

/// BPM 120 -> whole note = (60000 ms * 4) / 120 = 2000 ms
const WHOLE_NOTE_MS: u64 = 2000;

/// Plays an individual note score entry.
/// Tone is sounded for 90% of duration, followed by 10% silence for note articulation.
async fn play_note<'a>(
    ledc: &'a Ledc<'a>,
    pin: &mut esp_hal::peripherals::GPIO45<'a>,
    freq_hz: u32,
    duration_ms: u64,
) {
    let sound_ms = (duration_ms * 9) / 10;
    let gap_ms = duration_ms - sound_ms;

    if freq_hz > 0 {
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

        Timer::after(Duration::from_millis(sound_ms)).await;
        let _ = channel0.set_duty(0);
    } else {
        Timer::after(Duration::from_millis(sound_ms)).await;
    }

    Timer::after(Duration::from_millis(gap_ms)).await;
}

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("[E1002] BuzzerWithTone — Imperial March");

    let mut ledc = Ledc::new(peripherals.LEDC);
    ledc.set_global_slow_clock(LSGlobalClkSource::APBClk);

    let mut gpio45 = peripherals.GPIO45;

    loop {
        info!("[buzz] Playing Imperial March...");
        for &(freq, divisor) in MELODY.iter() {
            let duration_ms = if divisor > 0 {
                WHOLE_NOTE_MS / (divisor as u64)
            } else {
                (WHOLE_NOTE_MS / (-divisor as u64) * 3) / 2
            };

            play_note(&ledc, &mut gpio45, freq, duration_ms).await;
        }

        info!("[buzz] Done. Pausing 3 s.");
        Timer::after(Duration::from_secs(3)).await;
    }
}

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    defmt::error!("{}", panic_info);
    loop {}
}
