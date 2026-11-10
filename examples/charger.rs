//! # Charger status: USB or battery (reTerminal E1002)
//!
//! Probes the SY6974B Li-Po charger on its own I2C bus (SDA GPIO39, SCL GPIO40), to tell whether the board
//! runs from USB or from the battery. There is no VBUS GPIO, so the charger's registers are the only way.
//!
//! The chip is register-compatible with the TI BQ25601 family at address 0x6B, which is the assumption here.
//! The example first scans the bus and prints every address that answers, then every 2 s dumps registers
//! 0x00 to 0x0B and decodes REG08 (system status):
//!
//! | Bits | Field | Meaning |
//! |---|---|---|
//! | 7:5 | VBUS_STAT | 0 no input, 1 USB host (SDP), 2 adapter (CDP), 3 adapter, 7 OTG |
//! | 4:3 | CHRG_STAT | 0 not charging, 1 pre-charge, 2 fast charge, 3 charge done |
//! | 2 | PG_STAT | 1 power good (an input source is present) |
//!
//! ## Run
//!
//! ```bash
//! cargo run --release --example charger
//! ```
//!
//! The serial link is the USB cable, so the battery-only state cannot be logged. The user LED (GPIO6,
//! active-low) therefore blinks the answer every 2 s: once for USB (`PG` 1), twice for battery only
//! (`PG` 0), three times when the charger does not answer. Flash it, unplug USB, and count.

#![no_std]
#![no_main]

use defmt::{error, info};
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_println as _;

esp_bootloader_esp_idf::esp_app_desc!();

/// Assumed charger address (BQ25601 family).
const CHARGER_ADDR: u8 = 0x6B;

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let timg0 = esp_hal::timer::timg::TimerGroup::new(p.TIMG0);
    let sw_interrupt = esp_hal::interrupt::software::SoftwareInterruptControl::new(p.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    let mut i2c = I2c::new(p.I2C1, I2cConfig::default())
        .unwrap()
        .with_sda(p.GPIO39)
        .with_scl(p.GPIO40);

    let mut led = Output::new(p.GPIO6, Level::High, OutputConfig::default());

    info!("[charger] scanning GPIO39/GPIO40");
    let mut found = 0;
    for addr in 0x08u8..0x78 {
        if i2c.write(addr, &[]).is_ok() {
            info!("[charger] device at 0x{:02x}", addr);
            found += 1;
        }
    }
    info!("[charger] {} device(s)", found);

    loop {
        let mut regs = [0u8; 12];
        let mut blinks = 3;
        let mut ok = true;
        for (i, r) in regs.iter_mut().enumerate() {
            let mut b = [0u8];
            if i2c.write_read(CHARGER_ADDR, &[i as u8], &mut b).is_err() {
                ok = false;
                break;
            }
            *r = b[0];
        }
        if ok {
            let s = regs[8];
            blinks = if s & 4 != 0 { 1 } else { 2 };
            info!(
                "[charger] regs {=[u8]:02x} | VBUS_STAT {} CHRG_STAT {} PG {}",
                regs,
                s >> 5,
                (s >> 3) & 3,
                (s >> 2) & 1
            );
        } else {
            error!("[charger] no answer from 0x{:02x}", CHARGER_ADDR);
        }
        for _ in 0..blinks {
            led.set_low();
            Timer::after(Duration::from_millis(200)).await;
            led.set_high();
            Timer::after(Duration::from_millis(300)).await;
        }
        Timer::after(Duration::from_millis(1500)).await;
    }
}

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    defmt::error!("{}", panic_info);
    loop {}
}
