//! # News: top headlines on the 6-colour panel (async)
//!
//! Port of the GxEPD2 `News` sketch (itself from the Soldered Inkplate 6COLOR example).
//!
//! Joins Wi-Fi, reads the time over SNTP, fetches the top headlines from NewsAPI and draws the first three
//! that have a description: a title over a red rule, the date and the time of the update, then for
//! each article a blue headline (two lines at most) and a description (three lines at most). A failed
//! request is retried 3 times, and each request gives up after 30 s. It then waits awake with Wi-Fi off and resets
//! itself every 30 minutes on USB and every 120 on battery (10 after a failure, keeping the last good picture), well inside NewsAPI's 100
//! requests a day. It does not use deep sleep: see the `epd_ed2208_wake` bench on the `troubleshooting` branch for why.
//!
//! Needs a free key from <https://newsapi.org/>. The key and the Wi-Fi credentials are in `secret.rs`, and
//! the request URL, which contains the key, is never logged:
//!
//! ```bash
//! cp secret.rs.example secret.rs   # then edit it, with the Wi-Fi name and password and the key
//! cargo run --release --example news
//! ```
//!
//! Differences from the sketch: the fonts are `u8g2` bitmap fonts of about the same size, and the title is
//! upright (there is no italic serif). The time is SNTP plus a fixed `TIME_ZONE` offset, as in the sketch's
//! `configTime`. See `common/mod.rs`.
//!
//! ## Expected
//!
//! The log prints `got IP`, `time known: true`, `fetch ok, N items`, then the refresh messages. On failure
//! the panel shows `WiFi connection failed.` or `Failed to fetch news.`

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use alloc::vec;
use common::{EpdBuffer, net};
use defmt::{error, info};
use embassy_executor::Spawner;
use esp_hal::clock::CpuClock;
use esp_println as _;
use ui::{TIME_ZONE, date_line, draw_news, parse, update_line};

esp_bootloader_esp_idf::esp_app_desc!();

/// Minutes between refreshes, and after a failed one. 48 refreshes a day stay under NewsAPI's 100 requests.
const REFRESH_MINUTES: u64 = 30;
/// The same on battery alone, to spare the cell. The charger tells the two apart (see `common::on_usb`).
const BATTERY_MINUTES: u64 = 120;
const RETRY_MINUTES: u64 = 10;

mod secret {
    include!("../secret.rs");
}

/// The hardware-free part, tested on the host by `host-tests`.
mod ui {
    use crate::common::{self, Clock, Color6, EpdBuffer, WIDTH};
    use alloc::{format, string::String, vec::Vec};
    use embedded_graphics::{
        prelude::*,
        primitives::{Line, PrimitiveStyle, Rectangle},
    };
    use u8g2_fonts::types::HorizontalAlignment;

    /// UTC offset in hours (the sketch's `timeZone`).
    pub const TIME_ZONE: i32 = -8;
    /// NewsAPI country code.
    pub const NEWS_COUNTRY: &str = "us";
    pub const MAX_ITEMS: usize = 3;

    /// An article with a headline and a description, folded to ASCII.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Item {
        pub title: String,
        pub description: String,
    }

    /// The request URL for a key. It contains the key, so it must not be logged.
    pub fn url(key: &str) -> String {
        format!(
            "https://newsapi.org/v2/top-headlines?country={NEWS_COUNTRY}&pageSize=10&apiKey={key}"
        )
    }

    /// Up to [`MAX_ITEMS`] articles that have a title and a non-empty description, or `None` when the JSON
    /// is bad or no article qualifies.
    pub fn parse(body: &[u8]) -> Option<Vec<Item>> {
        let value: serde_json::Value = serde_json::from_slice(body).ok()?;
        let mut items = Vec::new();
        for article in value["articles"].as_array()? {
            if items.len() >= MAX_ITEMS {
                break;
            }
            let (Some(title), Some(description)) =
                (article["title"].as_str(), article["description"].as_str())
            else {
                continue;
            };
            if description.is_empty() {
                continue;
            }
            items.push(Item {
                title: common::ascii_fold(title),
                description: common::ascii_fold(description),
            });
        }
        (!items.is_empty()).then_some(items)
    }

    /// `Date: dd.mm.yyyy`, as the sketch's `dateStr`.
    pub fn date_line(now: Option<Clock>) -> String {
        match now {
            Some(t) => format!("Date: {:02}.{:02}.{:04}", t.day, t.month, t.year),
            None => String::from("Date: unknown"),
        }
    }

    /// `Last update: hh:mm`, as the sketch's `updStr`.
    pub fn update_line(now: Option<Clock>) -> String {
        match now {
            Some(t) => format!("Last update: {:02}:{:02}", t.hour, t.minute),
            None => String::from("Last update: unknown"),
        }
    }

    fn fill(target: &mut EpdBuffer, x: i32, y: i32, w: u32, h: u32, color: Color6) {
        let _ = Rectangle::new(Point::new(x, y), Size::new(w, h))
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(target);
    }

    /// The news screen as the sketch's `drawNews`.
    pub fn draw_news(target: &mut EpdBuffer, items: &[Item], date: &str, updated: &str) {
        let w = WIDTH as i32;
        let margin = 30;
        let inner = w - 2 * margin;
        target.clear(Color6::White);

        common::draw_text(
            target,
            &common::SERIF_BOLD_24,
            "World News",
            w / 2,
            42,
            HorizontalAlignment::Center,
            Color6::Black,
        );
        fill(target, margin, 54, inner as u32, 3, Color6::Red);

        common::draw_text(
            target,
            &common::SANS_9,
            date,
            margin,
            80,
            HorizontalAlignment::Left,
            Color6::Black,
        );
        common::draw_text(
            target,
            &common::SANS_9,
            updated,
            w - margin,
            80,
            HorizontalAlignment::Right,
            Color6::Black,
        );
        fill(target, margin, 90, inner as u32, 2, Color6::Black);

        let (top, box_h) = (100, 125);
        for (i, item) in items.iter().enumerate() {
            let y = top + i as i32 * box_h;
            let next = common::draw_wrapped(
                target,
                &common::SERIF_BOLD_12,
                margin,
                y + 22,
                inner,
                2,
                26,
                &item.title,
                Color6::Blue,
            );
            common::draw_wrapped(
                target,
                &common::SANS_9,
                margin,
                next + 2,
                inner,
                3,
                20,
                &item.description,
                Color6::Black,
            );
            if i + 1 < items.len() {
                let _ = Line::new(
                    Point::new(margin, y + box_h - 8),
                    Point::new(margin + inner - 1, y + box_h - 8),
                )
                .into_styled(PrimitiveStyle::with_stroke(Color6::Black, 1))
                .draw(target);
            }
        }
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

    info!("[E1002] News");

    // No answer from the charger counts as USB, the shorter interval.
    let on_usb = common::on_usb(p.I2C1, p.GPIO39, p.GPIO40).unwrap_or(true);
    info!("power: {}", if on_usb { "USB" } else { "battery" });

    let mut frame = common::new_frame();
    let mut fetched = false;

    match net::connect(&spawner, p.WIFI, secret::WIFI_SSID, secret::WIFI_PASSWORD).await {
        None => {
            error!("[E1002] WiFi FAILED");
            common::draw_message(&mut EpdBuffer::new(&mut frame), "WiFi connection failed.");
        }
        Some(wifi) => {
            let now = net::unix_time(wifi.stack)
                .await
                .map(|unix| common::local_clock(unix, TIME_ZONE));
            info!("time known: {}", now.is_some());

            // A response with ten articles is tens of KiB; the heap is the 8 MB PSRAM.
            let mut body = vec![0u8; 96 * 1024];
            let url = ui::url(secret::NEWS_API_KEY);
            let items =
                match net::https_get_retry(wifi.stack, &url, "reTerminal-E1002", &mut body).await {
                    Ok(n) => parse(&body[..n]),
                    Err(reason) => {
                        error!("[news] {}", reason);
                        None
                    }
                };

            let mut target = EpdBuffer::new(&mut frame);
            match items {
                Some(items) => {
                    info!("fetch ok, {} items", items.len());
                    fetched = true;
                    let (date, updated) = (date_line(now), update_line(now));
                    draw_news(&mut target, &items, &date, &updated);
                }
                None => {
                    error!("fetch failed");
                    common::draw_message(&mut target, "Failed to fetch news.");
                }
            }
        }
    }

    // After a failed refresh the panel keeps the last good picture; only a first run shows the error.
    if fetched || !common::after_loop_reset() {
        let Some(mut epd) = common::Epd::new(
            p.SPI2, p.GPIO7, p.GPIO8, p.GPIO9, p.GPIO10, p.GPIO11, p.GPIO12, p.GPIO13,
        )
        .await
        else {
            error!("[E1002] display init failed");
            common::halt().await
        };
        epd.show(&frame).await;
    }
    let minutes = match (fetched, on_usb) {
        (false, _) => RETRY_MINUTES,
        (true, true) => REFRESH_MINUTES,
        (true, false) => BATTERY_MINUTES,
    };
    common::wait_then_reset(minutes).await
}
