//! # Quotables: a random quote on the 6-colour panel (async)
//!
//! Port of the GxEPD2 `Quotables` sketch (itself from the Soldered Inkplate 10 example).
//!
//! Joins Wi-Fi, fetches a random quote from `api.quotable.kurokeita.dev` (no key), and draws it: a big red
//! opening quote mark, the quote in black, the author in blue at the bottom right. Quotes over 128 bytes
//! are skipped (they would not fit) and up to 10 are tried. It runs once and stops; reset for a new quote.
//! A full refresh takes about 30 s on this panel.
//!
//! Differences from the sketch: the fonts are `u8g2` bitmap fonts of about the same size, and the
//! "italic" quote font is upright (there is no italic serif). See `common/mod.rs`.
//!
//! ## Run
//!
//! ```bash
//! cp secret.rs.example secret.rs   # then edit it, with the Wi-Fi name and password
//! cargo run --release --example quotables
//! ```
//!
//! ## Expected
//!
//! The log prints `got IP`, `quote: ... / author`, then the refresh messages. On failure the panel shows
//! `WiFi connection failed.` or `Could not fetch a quote.`

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use alloc::vec;
use common::{EpdBuffer, net};
use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_println as _;
use ui::{MAX_TRIES, URL, draw_quote, parse};

esp_bootloader_esp_idf::esp_app_desc!();

mod secret {
    include!("../secret.rs");
}

/// The hardware-free part, tested on the host by `host-tests`.
mod ui {
    use crate::common::{self, Color6, EpdBuffer, HEIGHT, WIDTH};
    use alloc::{format, string::String};
    use u8g2_fonts::types::HorizontalAlignment;

    pub const URL: &str = "https://api.quotable.kurokeita.dev/api/quotes/random";
    /// Longer quotes are skipped (they would not fit).
    pub const MAX_QUOTE_LEN: usize = 128;
    pub const MAX_TRIES: u32 = 10;

    /// A quote and its author, folded to ASCII.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Quote {
        pub text: String,
        pub author: String,
    }

    /// The quote of a response, or `None` when the JSON is bad, a field is missing or the quote is longer
    /// than [`MAX_QUOTE_LEN`] bytes.
    pub fn parse(body: &[u8]) -> Option<Quote> {
        let value: serde_json::Value = serde_json::from_slice(body).ok()?;
        let content = value["quote"]["content"].as_str()?;
        let name = value["quote"]["author"]["name"].as_str()?;
        if content.len() > MAX_QUOTE_LEN {
            return None;
        }
        Some(Quote {
            text: common::ascii_fold(content),
            author: common::ascii_fold(name),
        })
    }

    /// The quote screen: the mark, the wrapped quote and the author at the bottom right.
    pub fn draw_quote(target: &mut EpdBuffer, quote: &Quote) {
        let (w, h) = (WIDTH as i32, HEIGHT as i32);
        target.clear(Color6::White);

        // Big red opening quote mark.
        common::draw_text(
            target,
            &common::SERIF_BOLD_24,
            "\"",
            30,
            90,
            HorizontalAlignment::Left,
            Color6::Red,
        );
        common::draw_wrapped(
            target,
            &common::SERIF_24,
            60,
            120,
            w - 120,
            6,
            46,
            &quote.text,
            Color6::Black,
        );
        common::draw_text(
            target,
            &common::SANS_12,
            &format!("- {}", quote.author),
            w - 50,
            h - 30,
            HorizontalAlignment::Right,
            Color6::Blue,
        );
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

    info!("[E1002] Quotables");

    let Some(mut epd) = common::Epd::new(
        p.SPI2, p.GPIO7, p.GPIO8, p.GPIO9, p.GPIO10, p.GPIO11, p.GPIO12, p.GPIO13,
    )
    .await
    else {
        error!("[E1002] display init failed");
        common::halt().await
    };
    let mut frame = common::new_frame();

    match net::connect(&spawner, p.WIFI, secret::WIFI_SSID, secret::WIFI_PASSWORD).await {
        None => {
            error!("[E1002] WiFi FAILED");
            common::draw_message(&mut EpdBuffer::new(&mut frame), "WiFi connection failed.");
        }
        Some(wifi) => {
            let mut body = vec![0u8; 8 * 1024];
            let mut quote = None;
            for _ in 0..MAX_TRIES {
                match net::https_get(wifi.stack, URL, "reTerminal-E1002", &mut body).await {
                    Ok(n) => quote = parse(&body[..n]),
                    Err(reason) => error!("[quote] {}", reason),
                }
                if quote.is_some() {
                    break;
                }
                Timer::after(Duration::from_secs(1)).await;
            }
            let mut target = EpdBuffer::new(&mut frame);
            match quote {
                Some(quote) => {
                    info!("quote: {} / {}", quote.text.as_str(), quote.author.as_str());
                    draw_quote(&mut target, &quote);
                }
                None => common::draw_message(&mut target, "Could not fetch a quote."),
            }
        }
    }

    epd.show(&frame).await;
    // Runs once. Press reset for a new quote.
    common::halt().await
}
