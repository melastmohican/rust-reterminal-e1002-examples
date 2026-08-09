//! # User LED Blink & Fade Example (reTerminal E1002)
//!
//! Controls the onboard green user LED on the Seeed Studio reTerminal E1002 carrier board.
//!
//! The user LED is connected to GPIO6 and uses **active-LOW** inverted logic:
//! - Driving GPIO6 **LOW** (0% duty cycle) turns the LED **ON**.
//! - Driving GPIO6 **HIGH** (100% duty cycle) turns the LED **OFF**.
//!
//! Demonstrates digital blinking and smooth brightness fading via LEDC hardware PWM.
//!
//! ## Hardware
//!
//! - **Board:** Seeed Studio reTerminal E1002 (XIAO ESP32-S3 MCU)
//! - **Peripherals:** Onboard User LED (GPIO6)
//!
//! ## Pin Mapping
//!
//! | Signal | GPIO | Logic | Notes |
//! |---|---|---|---|
//! | User LED | GPIO6 | Active-LOW | Digital Output & LEDC PWM Channel 0 |
//!
//! ## Wiring Schematic
//!
//! ```text
//!            Seeed Studio reTerminal E1002 Carrier Board
//!          +-------------------------------------------------+
//!          | 3.3V ----[ Resistor ]----( Anode - Green LED - Cathode )
//!          |                                  |              |
//!          |                           GPIO6 (active-LOW)    |
//!          +-------------------------------------------------+
//! ```
//!
//! ## Run
//!
//! ```bash
//! cargo run --example led
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

    info!("[E1002] LED demo (GPIO6, active-LOW)");

    let mut ledc = Ledc::new(peripherals.LEDC);
    ledc.set_global_slow_clock(LSGlobalClkSource::APBClk);

    let mut lstimer0 = ledc.timer::<LowSpeed>(timer::Number::Timer0);
    lstimer0
        .configure(timer::config::Config {
            duty: timer::config::Duty::Duty10Bit,
            clock_source: timer::LSClockSource::APBClk,
            frequency: esp_hal::time::Rate::from_hz(5000),
        })
        .unwrap();

    let mut channel0 = ledc.channel(channel::Number::Channel0, peripherals.GPIO6);
    channel0
        .configure(channel::config::Config {
            timer: &lstimer0,
            duty_pct: 100, // Active-LOW: 100% duty = HIGH = OFF
            drive_mode: esp_hal::gpio::DriveMode::PushPull,
        })
        .unwrap();

    loop {
        // Digital Blink (active-LOW: 0% duty = ON, 100% duty = OFF)
        info!("[led] ON");
        let _ = channel0.set_duty(0);
        Timer::after(Duration::from_millis(500)).await;

        info!("[led] OFF");
        let _ = channel0.set_duty(100);
        Timer::after(Duration::from_millis(500)).await;

        // PWM Fade demonstration (active-LOW: 100% -> 0% is fade ON)
        info!("[led] Fade ON");
        for duty in (0..=100).rev() {
            let _ = channel0.set_duty(duty);
            Timer::after(Duration::from_millis(15)).await;
        }

        info!("[led] Fade OFF");
        for duty in 0..=100 {
            let _ = channel0.set_duty(duty);
            Timer::after(Duration::from_millis(15)).await;
        }

        Timer::after(Duration::from_millis(500)).await;
    }
}

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    defmt::error!("{}", panic_info);
    loop {}
}
