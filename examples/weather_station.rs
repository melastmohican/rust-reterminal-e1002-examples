//! # Weather station: an Open-Meteo dashboard on the 6-colour panel (async)
//!
//! Port of the GxEPD2 `WeatherStation` sketch (itself from the Soldered Inkplate 6COLOR "OpenMeteo
//! Weather Station" example).
//!
//! Joins Wi-Fi, fetches the current, hourly and daily weather from Open-Meteo (no key) and draws it:
//!
//! - a header with the city, the temperature, the condition and its icon, and a black panel with the
//!   battery level, the date, the refresh time and the user name;
//! - a graph of the next six hours: the temperature as a red line, the chance of rain as blue bars;
//! - a strip of six days with an icon, the high (red arrow) and the low (blue arrow).
//!
//! A failed request is retried 3 times, and each request gives up after 30 s. It then waits awake with Wi-Fi off and
//! resets itself every 30 minutes on USB and every 120 on battery (10 after a failure, keeping the last good picture). It does not use deep
//! sleep: see `epd_ed2208_wake` for why. The first refresh comes about a minute after Wi-Fi connects.
//!
//! Set the city, coordinates and units in the constants below. Wi-Fi credentials are in `secret.rs`:
//!
//! ```bash
//! cp secret.rs.example secret.rs   # then edit it, with the Wi-Fi name and password
//! cargo run --release --example weather_station
//! ```
//!
//! Differences from the sketch:
//!
//! - **No SNTP.** The request asks for `timezone=auto`, so the response's `current.time` is already local
//!   time. The date, the refresh time, the weekday names and the start of the hourly window come from it.
//!   (The sketch combined a fixed UTC offset with NTP, which is an hour off for half the year.)
//! - No "Connecting to Wi-Fi..." screen: it cost an extra 30 s refresh and panel wear.
//! - A missing or null value in the response is a failed fetch; the sketch shows zeros.
//! - The battery is hidden below 2.5 V (no battery fitted).
//! - The fonts are `u8g2` bitmap fonts of about the same size (see `common/mod.rs`); the icons are the
//!   sketch's Lucide icons (ISC licence, `assets/LICENSE-lucide.txt`) in `common/icons.rs`.
//!
//! ## Pins
//!
//! Display as in the other examples. Battery: GPIO1 ADC1 (1:2 divider), GPIO21 enable (active high).
//!
//! ## Expected
//!
//! The log prints `got IP`, `battery ... V`, `fetch ok`, then the refresh messages. On failure the panel
//! shows `WiFi connection failed.` or `HTTP request failed.`

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use alloc::vec;
use common::{EpdBuffer, net};
use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_println as _;
use ui::{draw_dashboard, draw_error, parse, url};

esp_bootloader_esp_idf::esp_app_desc!();

/// Minutes between refreshes, and after a failed one. Open-Meteo updates its model every 15 minutes.
const REFRESH_MINUTES: u64 = 30;
/// The same on battery alone, to spare the cell. The charger tells the two apart (see `common::on_usb`).
const BATTERY_MINUTES: u64 = 120;
const RETRY_MINUTES: u64 = 10;

mod secret {
    include!("../secret.rs");
}

/// The hardware-free part, tested on the host by `host-tests`.
mod ui {
    use crate::common::{self, Color6, EpdBuffer, HEIGHT, WIDTH, icons};
    use alloc::{format, string::String};
    use embedded_graphics::{
        prelude::*,
        primitives::{Line, PrimitiveStyle, Rectangle, Triangle},
    };
    use u8g2_fonts::types::HorizontalAlignment::Left;

    // ===== User and location =====
    pub const USERNAME: &str = "melastmohican";
    pub const CITY: &str = "Bellevue";
    pub const LATITUDE: f32 = 47.6101;
    pub const LONGITUDE: f32 = -122.2015;
    /// `false` = Fahrenheit.
    pub const METRIC_UNITS: bool = true;

    pub const DAYS: usize = 7;
    pub const SHOWN_DAYS: usize = 6;
    pub const HOURS: usize = 6;

    // ===== Layout (800 x 480) =====
    pub const SCREEN_W: i32 = WIDTH as i32;
    pub const SCREEN_H: i32 = HEIGHT as i32;
    pub const HEADER_H: i32 = 120;
    /// Black info panel (battery, date, user).
    pub const USER_X: i32 = 560;
    pub const GRAPH_Y: i32 = HEADER_H;
    pub const GRAPH_H: i32 = 220;
    pub const WEEK_Y: i32 = GRAPH_Y + GRAPH_H;
    pub const WEEK_H: i32 = SCREEN_H - WEEK_Y;
    pub const ICON: i32 = 48;

    pub const DAY_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

    // =====================================================================
    // Data
    // =====================================================================

    pub struct Weather {
        pub current_temp: f32,
        pub code: i32,
        pub daily_min: [f32; DAYS],
        pub daily_max: [f32; DAYS],
        pub daily_codes: [i32; DAYS],
        pub day_names: [&'static str; DAYS],
        pub hourly_temps: [f32; HOURS],
        pub hourly_precip: [f32; HOURS],
        pub hourly_times: [String; HOURS],
        /// `YYYY-MM-DD` and `HH:MM` of `current.time`, local to the place.
        pub date: String,
        pub time: String,
    }

    pub fn temp_label() -> &'static str {
        if METRIC_UNITS { " C" } else { " F" }
    }

    /// The request URL: the sketch's, metric or imperial.
    pub fn url() -> String {
        let mut url = format!(
            "https://api.open-meteo.com/v1/forecast?latitude={LATITUDE:.4}&longitude={LONGITUDE:.4}\
             &daily=temperature_2m_max,temperature_2m_min,weather_code\
             &hourly=temperature_2m,precipitation_probability\
             &current=temperature_2m,weather_code&timezone=auto"
        );
        if !METRIC_UNITS {
            url.push_str("&temperature_unit=fahrenheit");
        }
        url
    }

    /// Weekday (0 = Sunday) of a `YYYY-MM-DD` date.
    pub fn weekday(date: &str) -> Option<usize> {
        let mut parts = date.split('-');
        let year: i32 = parts.next()?.parse().ok()?;
        let month: i32 = parts.next()?.parse().ok()?;
        let day: i32 = parts.next()?.parse().ok()?;
        // Days since 1970-01-01 (Howard Hinnant's `days_from_civil`); 1970-01-01 was a Thursday.
        let y = if month <= 2 { year - 1 } else { year };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = (month + 9) % 12;
        let doy = (153 * mp + 2) / 5 + day - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        Some((days + 4).rem_euclid(7) as usize)
    }

    pub fn number(value: &serde_json::Value) -> Option<f32> {
        value.as_f64().map(|v| v as f32)
    }

    pub fn code(value: &serde_json::Value) -> Option<i32> {
        i32::try_from(value.as_i64()?).ok()
    }

    /// `hh:mm` out of `2026-11-07T06:11`, or `??:??`.
    pub fn hour_minute(iso: &str) -> String {
        match iso.find('T') {
            Some(i) if iso.len() > i + 5 => String::from(&iso[i + 1..i + 6]),
            _ => String::from("??:??"),
        }
    }

    /// Reads the response like the sketch's `fetchWeatherData`; `None` when anything is missing.
    pub fn parse(body: &[u8]) -> Option<Weather> {
        let v: serde_json::Value = serde_json::from_slice(body).ok()?;
        let (current, daily, hourly) = (&v["current"], &v["daily"], &v["hourly"]);

        let now = current["time"].as_str()?;
        let date = String::from(now.get(..10)?);
        let hour_key = now.get(..13)?;
        let first_day = weekday(&date)?;
        // The hourly window starts at the current hour.
        let times = hourly["time"].as_array()?;
        let start = times
            .iter()
            .position(|t| t.as_str().is_some_and(|t| t.starts_with(hour_key)))?;

        let mut weather = Weather {
            current_temp: number(&current["temperature_2m"])?,
            code: code(&current["weather_code"])?,
            daily_min: [0.0; DAYS],
            daily_max: [0.0; DAYS],
            daily_codes: [0; DAYS],
            day_names: DAY_NAMES,
            hourly_temps: [0.0; HOURS],
            hourly_precip: [0.0; HOURS],
            hourly_times: core::array::from_fn(|_| String::new()),
            date,
            time: hour_minute(now),
        };
        for i in 0..DAYS {
            weather.daily_min[i] = number(daily["temperature_2m_min"].get(i)?)?;
            weather.daily_max[i] = number(daily["temperature_2m_max"].get(i)?)?;
            weather.daily_codes[i] = code(daily["weather_code"].get(i)?)?;
            weather.day_names[i] = DAY_NAMES[(first_day + i) % 7];
        }
        for i in 0..HOURS {
            let src = start + i;
            weather.hourly_temps[i] = number(hourly["temperature_2m"].get(src)?)?;
            weather.hourly_precip[i] = number(hourly["precipitation_probability"].get(src)?)?;
            weather.hourly_times[i] = hour_minute(times.get(src)?.as_str()?);
        }
        Some(weather)
    }

    /// WMO weather code to a description, as the sketch's `getWeatherDescription`.
    pub fn description(code: i32) -> &'static str {
        match code {
            0 => "Clear sky",
            1..=3 => "Mainly clear, partly cloudy",
            45 | 48 => "Fog",
            51 | 53 | 55 => "Drizzle",
            56 | 57 => "Freezing Drizzle",
            61 | 63 | 65 => "Rain",
            66 | 67 => "Freezing Rain",
            71 | 73 | 75 => "Snowfall",
            77 => "Snow grains",
            80..=82 => "Rain showers",
            85 | 86 => "Snow showers",
            95 => "Thunderstorm",
            96 | 99 => "Thunderstorm, hail",
            _ => "Unknown condition",
        }
    }

    /// The icon for a WMO weather code, as the sketch's `getWeatherIcon`.
    pub fn weather_icon(code: i32) -> &'static [u8; 288] {
        match code {
            0 => &icons::CLEAR_SKY,
            1..=3 => &icons::PARTLY_CLOUDY,
            45 | 48 => &icons::FOG,
            51 | 53 | 55 | 56 | 57 | 61 | 63 | 65 | 66 | 67 | 80..=82 => &icons::RAIN,
            71 | 73 | 75 | 77 | 85 | 86 => &icons::SNOW,
            95 | 96 | 99 => &icons::STORM,
            _ => &icons::GRAY,
        }
    }

    /// The 6-colour panel has no orange, so the icons are tinted: the sketch's `getWeatherColor`.
    pub fn weather_color(code: i32) -> Color6 {
        match code {
            0..=3 => Color6::Yellow,
            51 | 53 | 55 | 56 | 57 | 61 | 63 | 65 | 66 | 67 | 80..=82 => Color6::Blue,
            95 | 96 | 99 => Color6::Red,
            _ => Color6::Black,
        }
    }

    pub fn battery_percent(volts: f32) -> i32 {
        if volts >= 4.2 {
            100
        } else if volts <= 3.0 {
            0
        } else {
            (((volts - 3.0) / (4.2 - 3.0)) * 100.0) as i32
        }
    }

    pub fn battery_icon(percent: i32) -> &'static [u8; 288] {
        if percent >= 75 {
            &icons::FULL_BATTERY
        } else if percent >= 50 {
            &icons::HIGH_BATTERY
        } else if percent >= 25 {
            &icons::HALF_BATTERY
        } else {
            &icons::LOW_BATTERY
        }
    }

    // =====================================================================
    // Drawing
    // =====================================================================

    pub fn line(t: &mut EpdBuffer, x0: i32, y0: i32, x1: i32, y1: i32, c: Color6) {
        let _ = Line::new(Point::new(x0, y0), Point::new(x1, y1))
            .into_styled(PrimitiveStyle::with_stroke(c, 1))
            .draw(t);
    }

    pub fn rect(t: &mut EpdBuffer, x: i32, y: i32, w: i32, h: i32, c: Color6) {
        let _ = Rectangle::new(Point::new(x, y), Size::new(w as u32, h as u32))
            .into_styled(PrimitiveStyle::with_stroke(c, 1))
            .draw(t);
    }

    pub fn fill(t: &mut EpdBuffer, x: i32, y: i32, w: i32, h: i32, c: Color6) {
        let _ = Rectangle::new(Point::new(x, y), Size::new(w as u32, h as u32))
            .into_styled(PrimitiveStyle::with_fill(c))
            .draw(t);
    }

    pub fn triangle(t: &mut EpdBuffer, p: [(i32, i32); 3], c: Color6) {
        let _ = Triangle::new(
            Point::new(p[0].0, p[0].1),
            Point::new(p[1].0, p[1].1),
            Point::new(p[2].0, p[2].1),
        )
        .into_styled(PrimitiveStyle::with_fill(c))
        .draw(t);
    }

    pub fn text(
        t: &mut EpdBuffer,
        font: &u8g2_fonts::FontRenderer,
        s: &str,
        x: i32,
        y: i32,
        c: Color6,
    ) {
        common::draw_text(t, font, s, x, y, Left, c);
    }

    /// The sketch's `drawBitmap`: the set bits of a 48 x 48 1-bit bitmap in one colour.
    pub fn draw_icon(t: &mut EpdBuffer, bits: &[u8; 288], x: i32, y: i32, c: Color6) {
        for row in 0..ICON {
            for col in 0..ICON {
                let byte = bits[(row * 6 + col / 8) as usize];
                if byte & (0x80 >> (col % 8)) != 0 {
                    t.set_pixel((x + col) as u32, (y + row) as u32, c);
                }
            }
        }
    }

    pub fn draw_graph(t: &mut EpdBuffer, w: &Weather) {
        let (graph_x, graph_y) = (60, GRAPH_Y + 25);
        let (graph_w, graph_h) = (680, GRAPH_H - 30);
        let (margin_x, margin_y) = (20, 20);

        text(
            t,
            &common::SANS_9,
            "Hourly temperature and precipitation",
            10,
            GRAPH_Y + 22,
            Color6::Black,
        );

        let chart_left = graph_x + margin_x;
        let chart_bottom = graph_y + graph_h - margin_y;
        let plot_h = graph_h - 2 * margin_y;

        let mut t_min = 100.0f32;
        let mut t_max = -100.0f32;
        for &v in &w.hourly_temps {
            t_min = t_min.min(v);
            t_max = t_max.max(v);
        }
        let (padded_min, padded_max) = (t_min - 2.0, t_max + 2.0);
        let padded_mid = (padded_min + padded_max) / 2.0;
        let mut range = padded_max - padded_min;
        if range == 0.0 {
            range = 1.0;
        }
        let x_step = (graph_w - 2 * margin_x) as f32 / 5.0;

        // Axes.
        line(
            t,
            chart_left,
            graph_y + margin_y,
            chart_left,
            chart_bottom,
            Color6::Black,
        );
        line(
            t,
            chart_left,
            chart_bottom,
            graph_x + graph_w + 20,
            chart_bottom,
            Color6::Black,
        );

        // Axis labels (min / mid / max), kept at least 20 px apart.
        let y_min = chart_bottom;
        let mut y_max = chart_bottom - plot_h;
        let mut y_mid = chart_bottom - plot_h / 2;
        if (y_max - y_min).abs() < 20 {
            y_max = y_min + 20;
        }
        if (y_mid - y_min).abs() < 20 {
            y_mid = y_min + 20;
        }
        if (y_max - y_mid).abs() < 20 {
            y_max = y_mid + 20;
        }
        let label = |v: f32| format!("{v:.1}{}", temp_label());
        text(
            t,
            &common::SANS_9,
            &label(padded_min),
            chart_left - 70,
            y_min - 20,
            Color6::Blue,
        );
        text(
            t,
            &common::SANS_9,
            &label(padded_mid),
            chart_left - 70,
            y_mid,
            Color6::Black,
        );
        text(
            t,
            &common::SANS_9,
            &label(padded_max),
            chart_left - 70,
            y_max + 20,
            Color6::Red,
        );

        // Precipitation probability bars, 0 to 100 %.
        for i in 0..HOURS {
            let x = (chart_left as f32 + i as f32 * x_step) as i32;
            let bar_h = ((w.hourly_precip[i] / 100.0) * plot_h as f32) as i32;
            let y = chart_bottom - bar_h;
            if bar_h > 10 {
                fill(t, x + 5, y + 10, 10, bar_h - 10, Color6::Blue);
            }
            text(
                t,
                &common::SANS_9,
                &format!("{:.0}%", w.hourly_precip[i]),
                x + 5,
                y - 5,
                Color6::Black,
            );
        }

        // Temperature line, thickened by drawing it twice, 1 px apart.
        let point = |i: usize| {
            let x = (chart_left as f32 + i as f32 * x_step) as i32;
            let y = (chart_bottom as f32
                - ((w.hourly_temps[i] - padded_min) / range) * plot_h as f32)
                as i32;
            (x, y)
        };
        for i in 0..HOURS - 1 {
            let ((x1, y1), (x2, y2)) = (point(i), point(i + 1));
            line(t, x1, y1, x2, y2, Color6::Red);
            line(t, x1, y1 + 1, x2, y2 + 1, Color6::Red);
        }

        // Time labels.
        for i in 0..HOURS {
            let x = (chart_left as f32 + i as f32 * x_step) as i32;
            text(
                t,
                &common::SANS_9,
                &w.hourly_times[i],
                x + 3,
                chart_bottom + 20,
                Color6::Black,
            );
        }
    }

    pub fn draw_dashboard(t: &mut EpdBuffer, w: &Weather, battery_volts: f32) {
        t.clear(Color6::White);

        // Background frames and the black info panel.
        rect(t, 0, 0, SCREEN_W, HEADER_H, Color6::Black);
        fill(t, USER_X, 0, SCREEN_W - USER_X, HEADER_H, Color6::Black);
        rect(t, 0, GRAPH_Y, SCREEN_W, GRAPH_H, Color6::Black);
        rect(t, 0, WEEK_Y, SCREEN_W, WEEK_H, Color6::Black);

        // Section 1: city, temperature, condition.
        draw_icon(t, &icons::GRAY, 10, 5, Color6::Red);
        text(t, &common::SANS_18, CITY, 70, 40, Color6::Black);
        text(
            t,
            &common::SANS_12,
            &format!("{:.1}{}", w.current_temp, temp_label()),
            70,
            90,
            Color6::Black,
        );
        draw_icon(t, weather_icon(w.code), 10, 55, weather_color(w.code));
        text(
            t,
            &common::SANS_12,
            description(w.code),
            200,
            90,
            Color6::Black,
        );

        // Section 2: battery, date, user, white on the black panel.
        let mut y = 10;
        if battery_volts > 2.5 {
            let percent = battery_percent(battery_volts);
            draw_icon(t, battery_icon(percent), USER_X + 5, 0, Color6::White);
            text(
                t,
                &common::SANS_9,
                &format!("{percent}%"),
                USER_X + 55,
                y + 19,
                Color6::White,
            );
        }
        y += 17 + 33;
        text(t, &common::SANS_9, &w.date, USER_X + 5, y, Color6::White);
        y += 20;
        text(
            t,
            &common::SANS_9,
            &format!("Last refresh: {}", w.time),
            USER_X + 5,
            y,
            Color6::White,
        );
        y += 20;
        text(t, &common::SANS_9, USERNAME, USER_X + 5, y, Color6::White);

        // Section 3: six days.
        let day_w = SCREEN_W / SHOWN_DAYS as i32;
        let start_y = WEEK_Y + 28;
        for i in 0..SHOWN_DAYS {
            // Icon, then the two temperatures: about 115 px, of the 133 px a column has.
            let x = i as i32 * day_w + 8;
            text(
                t,
                &common::SANS_12,
                w.day_names[i],
                x + 5,
                start_y,
                Color6::Black,
            );
            draw_icon(
                t,
                weather_icon(w.daily_codes[i]),
                x + 5,
                start_y + 8,
                weather_color(w.daily_codes[i]),
            );

            // High: red up-triangle.
            let ax = x + 62;
            let mut ay = start_y + 20;
            triangle(
                t,
                [(ax, ay), (ax - 4, ay + 6), (ax + 4, ay + 6)],
                Color6::Red,
            );
            text(
                t,
                &common::SANS_9,
                &format!("{:.0}{}", w.daily_max[i], temp_label()),
                ax + 10,
                ay + 8,
                Color6::Black,
            );

            // Low: blue down-triangle.
            ay += 28;
            triangle(t, [(ax, ay + 6), (ax - 4, ay), (ax + 4, ay)], Color6::Blue);
            text(
                t,
                &common::SANS_9,
                &format!("{:.0}{}", w.daily_min[i], temp_label()),
                ax + 10,
                ay + 8,
                Color6::Black,
            );
        }

        // Section 4: hourly graph.
        draw_graph(t, w);
    }

    pub fn draw_error(t: &mut EpdBuffer, line1: &str, line2: &str) {
        t.clear(Color6::White);
        text(t, &common::SANS_18, line1, 50, 200, Color6::Red);
        text(t, &common::SANS_18, line2, 50, 250, Color6::Black);
    }
}

// =====================================================================

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

    info!("[E1002] Weather station");

    // No answer from the charger counts as USB, the shorter interval.
    let on_usb = common::on_usb(p.I2C1, p.GPIO39, p.GPIO40).unwrap_or(true);
    info!("power: {}", if on_usb { "USB" } else { "battery" });

    // Battery: the divider halves the voltage, so x2. Enable, sample, disable to save the cell.
    let mut batt_en = Output::new(p.GPIO21, Level::Low, OutputConfig::default());
    let mut adc_config = AdcConfig::new();
    let mut batt_pin =
        adc_config.enable_pin_with_cal::<_, AdcCalCurve<_>>(p.GPIO1, Attenuation::_11dB);
    let mut adc = Adc::new(p.ADC1, adc_config);
    batt_en.set_high();
    Timer::after(Duration::from_millis(5)).await;
    let raw: u16 = nb::block!(adc.read_oneshot(&mut batt_pin)).unwrap_or(0);
    batt_en.set_low();
    // `raw` is already in mV: with the curve calibration the reading is calibrated and the ADC reference is set up.
    let battery_volts = raw as f32 / 1000.0 * 2.0;
    info!("battery {} mV", (battery_volts * 1000.0) as u32);

    let mut frame = common::new_frame();
    let mut fetched = false;

    match net::connect(&spawner, p.WIFI, secret::WIFI_SSID, secret::WIFI_PASSWORD).await {
        None => {
            error!("[E1002] WiFi FAILED");
            draw_error(
                &mut EpdBuffer::new(&mut frame),
                "WiFi connection failed.",
                "Check credentials or try again.",
            );
        }
        Some(wifi) => {
            let mut body = vec![0u8; 64 * 1024];
            let weather =
                match net::https_get_retry(wifi.stack, &url(), "reTerminal-E1002", &mut body).await
                {
                    Ok(n) => parse(&body[..n]),
                    Err(reason) => {
                        error!("[weather] {}", reason);
                        None
                    }
                };
            let mut target = EpdBuffer::new(&mut frame);
            match weather {
                Some(weather) => {
                    info!("fetch ok");
                    fetched = true;
                    draw_dashboard(&mut target, &weather, battery_volts);
                }
                None => draw_error(
                    &mut target,
                    "HTTP request failed.",
                    "Check API URL or try again.",
                ),
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
