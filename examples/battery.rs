//! # Battery Voltage Monitor Example (reTerminal E1002)
//!
//! Reads and reports the Li-Po battery voltage on the Seeed Studio reTerminal E1002.
//!
//! The carrier board includes a Li-Po battery connector connected to an onboard
//! 1:2 resistor voltage divider that halves the battery voltage before feeding it to
//! ADC1_CH0 (GPIO1). The divider circuit is controlled by an active-high enable pin (GPIO21).
//! To avoid draining the battery through the divider when not measuring, GPIO21 is driven
//! HIGH only during ADC sampling and kept LOW during idle intervals.
//!
//! ## Hardware
//!
//! - **Board:** Seeed Studio reTerminal E1002 (XIAO ESP32-S3 MCU)
//! - **Peripherals:** Onboard Li-Po Battery Voltage Divider Circuit & ADC1
//!
//! ## Pin Mapping
//!
//! | Signal | GPIO | Notes |
//! |---|---|---|
//! | Battery ADC | GPIO1 | ADC1 Channel 0 (1:2 voltage divider output) |
//! | Battery Enable | GPIO21 | Active-HIGH enable for voltage divider circuit |
//!
//! ## Wiring Schematic
//!
//! ```text
//!            Seeed Studio reTerminal E1002 Carrier Board
//!          +-------------------------------------------------+
//!          | Li-Po Battery Connector (+)                     |
//!          |       |                                         |
//!          |       +---[ 100k Ohm ]---+                      |
//!          |                          |                      |
//!          |                 GPIO1 (ADC1 CH0)                |
//!          |                          |                      |
//!          |       +---[ 100k Ohm ]---+                      |
//!          |       |                                         |
//!          |   [ MOSFET Switch ] <--- GPIO21 (Batt Enable)   |
//!          |       |                                         |
//!          |      GND                                        |
//!          +-------------------------------------------------+
//! ```
//!
//! ## Run
//!
//! ```bash
//! cargo run --example battery
//! ```

#![no_std]
#![no_main]

use defmt::info;
use embassy_time::{Duration, Timer};
use esp_hal::analog::adc::{Adc, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
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

    info!("[E1002] Battery voltage monitor initializing...");

    // Configure Battery Enable Pin (GPIO 21, active-high)
    let mut batt_en = Output::new(peripherals.GPIO21, Level::Low, OutputConfig::default());

    // Configure ADC1 for GPIO 1 with 11dB attenuation (~0 - 3.1V input range)
    let mut adc_config = AdcConfig::new();
    let mut batt_adc_pin = adc_config.enable_pin(peripherals.GPIO1, Attenuation::_11dB);
    let mut adc = Adc::new(peripherals.ADC1, adc_config);

    loop {
        // Enable circuit -> sample -> disable to minimize quiescent current drain
        batt_en.set_high();
        Timer::after(Duration::from_millis(5)).await;

        let raw_val: u16 = nb::block!(adc.read_oneshot(&mut batt_adc_pin)).unwrap_or(0);

        // Turn off divider circuit to prevent passive battery discharge
        batt_en.set_low();

        // 12-bit ADC raw range 0..4095 mapped to ~3300 mV full scale
        let mv = (raw_val as u32 * 3300) / 4095;

        // The voltage divider halves the battery voltage, so multiply by 2
        let batt_mv = mv * 2;
        let batt_v_int = batt_mv / 1000;
        let batt_v_dec = (batt_mv % 1000) / 10;

        info!(
            "[batt] ADC Raw: {} ({} mV) -> Battery: {}.{:02} V",
            raw_val, mv, batt_v_int, batt_v_dec
        );

        Timer::after(Duration::from_secs(2)).await;
    }
}

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    defmt::error!("{}", panic_info);
    loop {}
}
