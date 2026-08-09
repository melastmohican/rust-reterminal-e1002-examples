//! # User Buttons Example (reTerminal E1002)
//!
//! Reads and debounces the three user buttons (KEY0, KEY1, KEY2) on the Seeed Studio reTerminal E1002.
//!
//! The buttons are connected with hardware pull-up resistors on the carrier board,
//! reading HIGH when open and LOW when pressed (active-LOW logic). State changes are filtered
//! using a time-based debouncing algorithm (50ms stability threshold).
//!
//! ## Hardware
//!
//! - **Board:** Seeed Studio reTerminal E1002 (XIAO ESP32-S3 MCU)
//! - **Peripherals:** 3 Onboard User Buttons (KEY0, KEY1, KEY2)
//!
//! ## Pin Mapping
//!
//! | Button | GPIO | Logic | Notes |
//! |---|---|---|---|
//! | KEY0 | GPIO3 | Active-LOW | User Button 0 |
//! | KEY1 | GPIO4 | Active-LOW | User Button 1 |
//! | KEY2 | GPIO5 | Active-LOW | User Button 2 |
//!
//! ## Wiring Schematic
//!
//! ```text
//!            Seeed Studio reTerminal E1002 Carrier Board
//!          +-------------------------------------------------+
//!          | 3.3V ---[ Pull-Up ]---+                         |
//!          |                       |                         |
//!          |            GPIO3 (KEY0) / GPIO4 (KEY1) / GPIO5 (KEY2)
//!          |                       |                         |
//!          |                 [ Tactile Switch ]              |
//!          |                       |                         |
//!          |                      GND                        |
//!          +-------------------------------------------------+
//! ```
//!
//! ## Run
//!
//! ```bash
//! cargo run --example button
//! ```

#![no_std]
#![no_main]

use defmt::info;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Pull};
use esp_println as _;

esp_bootloader_esp_idf::esp_app_desc!();

/// Minimum time (ms) a raw pin state must remain stable to confirm a button press or release.
const DEBOUNCE_MS: u64 = 50;

/// State tracking structure for an individual user button.
struct Button<'a> {
    name: &'static str,
    pin_number: u8,
    input: Input<'a>,
    last_stable: bool,
    last_raw: bool,
    last_change: Instant,
}

impl<'a> Button<'a> {
    /// Creates a new button monitor instance initialized with current pin state.
    fn new(name: &'static str, pin_number: u8, input: Input<'a>) -> Self {
        let raw = input.is_high();
        Self {
            name,
            pin_number,
            input,
            last_stable: raw,
            last_raw: raw,
            last_change: Instant::now(),
        }
    }

    /// Evaluates current pin state with debouncing.
    /// Returns `Some(true)` on press, `Some(false)` on release, or `None` if unchanged.
    fn update(&mut self) -> Option<bool> {
        let raw = self.input.is_high();
        let now = Instant::now();

        if raw != self.last_raw {
            self.last_raw = raw;
            self.last_change = now;
        }

        if now.duration_since(self.last_change).as_millis() >= DEBOUNCE_MS
            && raw != self.last_stable
        {
            self.last_stable = raw;
            // Active-LOW logic: false means PRESSED, true means RELEASED
            return Some(!raw);
        }

        None
    }
}

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("[E1002] Button demo (KEY0=GPIO3, KEY1=GPIO4, KEY2=GPIO5)");
    info!("[E1002] Press any button...");

    let input_config = InputConfig::default().with_pull(Pull::Up);

    let key0 = Input::new(peripherals.GPIO3, input_config);
    let key1 = Input::new(peripherals.GPIO4, input_config);
    let key2 = Input::new(peripherals.GPIO5, input_config);

    let mut buttons = [
        Button::new("KEY0", 3, key0),
        Button::new("KEY1", 4, key1),
        Button::new("KEY2", 5, key2),
    ];

    loop {
        for btn in buttons.iter_mut() {
            if let Some(pressed) = btn.update() {
                if pressed {
                    info!("[btn] {} (GPIO{}) PRESSED", btn.name, btn.pin_number);
                } else {
                    info!("[btn] {} (GPIO{}) released", btn.name, btn.pin_number);
                }
            }
        }
        Timer::after(Duration::from_millis(10)).await;
    }
}

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    defmt::error!("{}", panic_info);
    loop {}
}
