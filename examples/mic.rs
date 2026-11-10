//! # PDM microphone level (reTerminal E1002)
//!
//! Reads the onboard MSM261DHP006 PDM MEMS microphone and prints its sound level every 200 ms.
//!
//! esp-hal 1.1 has no PDM receive mode for the I2S peripheral ("only TDM mode is supported"). The mic only
//! needs a clock in and gives a 1-bit stream out, so I2S is used as a clock generator and a raw bit
//! capture: BCLK (GPIO42) clocks the mic at 2.4 MHz (75 kHz x 2 channels x 16 bits), DIN (GPIO41) is
//! captured by DMA as it comes, and the bits of each 32-bit word are counted. The count (0 to 32) is the mic's
//! signal at 75 kHz after a crude box filter: the density of ones follows the sound pressure. The
//! level is the largest deviation of a 0.2 ms average of that count from its running mean over 200 ms, x100, so a
//! short clap shows. It measures loudness, not audio quality.
//!
//! ## Hardware
//!
//! | Signal | GPIO | Notes |
//! |---|---|---|
//! | PDM_EN | GPIO38 | Active-HIGH enable of the mic supply (`MIC_PWR`) |
//! | PDM_CLK | GPIO42 | Clock to the mic |
//! | PDM_DATA | GPIO41 | 1-bit stream from the mic |
//!
//! ## Run
//!
//! ```bash
//! cargo run --release --example mic
//! ```
//!
//! ## Expected
//!
//! `[mic] level N ####` every 200 ms. It sits low in a quiet room and jumps when you clap or talk near the
//! board. A constant `level 0` means a dead stream: the mic is off, or the clock edge it answers on is not the one
//! sampled.

#![no_std]
#![no_main]

use defmt::info;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::dma_buffers;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::i2s::master::{Channels, Config, DataFormat, I2s};
use esp_hal::time::Rate;
use esp_println as _;

esp_bootloader_esp_idf::esp_app_desc!();

/// 75 kHz x 32 bits per frame = 2.4 MHz mic clock.
const FRAME_RATE_HZ: u32 = 75_000;
/// Words averaged per reading: 16 x 32 bits at 2.4 MHz = 0.21 ms, a box filter up to about 4.7 kHz.
const WINDOW_WORDS: u32 = 16;

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let timg0 = esp_hal::timer::timg::TimerGroup::new(p.TIMG0);
    let sw_interrupt = esp_hal::interrupt::software::SoftwareInterruptControl::new(p.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("[E1002] Microphone level");

    // Power the mic, and give it time to start.
    let _mic_power = Output::new(p.GPIO38, Level::High, OutputConfig::default());
    Timer::after(Duration::from_millis(50)).await;

    let (rx_buffer, rx_descriptors, _, _) = dma_buffers!(4 * 4092, 0);
    let i2s = I2s::new(
        p.I2S0,
        p.DMA_CH0,
        Config::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(FRAME_RATE_HZ))
            .with_data_format(DataFormat::Data16Channel16)
            .with_channels(Channels::STEREO),
    )
    .unwrap();
    let mut i2s_rx = i2s
        .i2s_rx
        .with_bclk(p.GPIO42)
        .with_din(p.GPIO41)
        .build(rx_descriptors);
    let mut transfer = i2s_rx.read_dma_circular(rx_buffer).unwrap();

    // `pop` needs room for everything available, and copies whole 4092-byte DMA descriptors.
    let mut chunk = [0u8; 4 * 4092];
    let mut mean = 16.0f32;
    let (mut window, mut in_window, mut peak) = (0.0f32, 0u32, 0.0f32);
    let mut last_print = embassy_time::Instant::now();
    loop {
        let avail_raw = transfer.available();
        let avail = avail_raw.unwrap_or(0);
        if avail > 0 {
            let Ok(n) = transfer.pop(&mut chunk[..avail]) else {
                continue;
            };
            for word in chunk[..n].chunks_exact(4) {
                let s = word.iter().map(|b| b.count_ones()).sum::<u32>() as f32;
                mean += (s - mean) / 4096.0;
                window += s - mean;
                in_window += 1;
                if in_window == WINDOW_WORDS {
                    peak = peak.max((window / WINDOW_WORDS as f32).abs());
                    (window, in_window) = (0.0, 0);
                }
            }
        } else {
            Timer::after(Duration::from_millis(2)).await;
        }

        if last_print.elapsed() >= Duration::from_millis(200) {
            let level = (peak * 100.0) as u32;
            info!(
                "[mic] level {} (mean {} / 32) {}",
                level,
                mean as u32,
                bar(level)
            );
            peak = 0.0;
            last_print = embassy_time::Instant::now();
        }
    }
}

/// A bar of up to 40 `#`, one per 4 level units.
fn bar(level: u32) -> &'static str {
    const BAR: &str = "########################################";
    &BAR[..((level / 4) as usize).min(BAR.len())]
}

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    defmt::error!("{}", panic_info);
    loop {}
}
