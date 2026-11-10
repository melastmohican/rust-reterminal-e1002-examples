//! Tests of the `mod ui` block of `examples/weather_station.rs`: the request, the parsing and the
//! mappings, against a real response of Open-Meteo.

mod support;

use host_tests::gfx::{Color6, EpdBuffer, new_frame};
use host_tests::icons;
use host_tests::weather_station::*;
use support::count;

const FORECAST: &[u8] = include_bytes!("../fixtures/openmeteo_forecast.json");

fn json() -> serde_json::Value {
    serde_json::from_slice(FORECAST).unwrap()
}

fn f(v: &serde_json::Value) -> f32 {
    v.as_f64().unwrap() as f32
}

fn bytes(v: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}

#[test]
fn weekday_is_zero_on_sunday() {
    assert_eq!(weekday("1970-01-01"), Some(4)); // a Thursday
    assert_eq!(weekday("2026-11-08"), Some(0));
    assert_eq!(weekday("2026-11-09"), Some(1));
    assert_eq!(weekday("2000-02-29"), Some(2));
    assert_eq!(weekday("2024-02-29"), Some(4));
    assert_eq!(weekday("2025-01-01"), Some(3));
}

#[test]
fn weekday_of_a_bad_date_is_none() {
    assert_eq!(weekday(""), None);
    assert_eq!(weekday("soon"), None);
    assert_eq!(weekday("2026-11"), None);
}

#[test]
fn hour_minute_cuts_the_time_out_of_an_iso_string() {
    assert_eq!(hour_minute("2026-11-07T06:11"), "06:11");
    assert_eq!(hour_minute("2026-11-07T06:11:59"), "06:11");
    assert_eq!(hour_minute("2026-11-07"), "??:??");
    assert_eq!(hour_minute("2026-11-07T06"), "??:??");
}

#[test]
fn the_url_asks_for_what_the_dashboard_shows() {
    let url = url();
    assert!(url.starts_with("https://api.open-meteo.com/v1/forecast?"));
    assert!(url.contains("latitude=47.6101&longitude=-122.2015"));
    assert!(url.contains("daily=temperature_2m_max,temperature_2m_min,weather_code"));
    assert!(url.contains("hourly=temperature_2m,precipitation_probability"));
    assert!(url.contains("current=temperature_2m,weather_code"));
    assert!(url.contains("timezone=auto"));
    // Metric is the default, so there is no unit override.
    assert!(METRIC_UNITS);
    assert!(!url.contains("fahrenheit"));
}

#[test]
fn parses_the_current_weather() {
    let w = parse(FORECAST).unwrap();
    let v = json();
    assert_eq!(w.current_temp, f(&v["current"]["temperature_2m"]));
    assert_eq!(
        w.code,
        v["current"]["weather_code"].as_i64().unwrap() as i32
    );
    assert_eq!(w.date, "2026-11-08");
    assert_eq!(w.time, "23:00");
}

#[test]
fn names_the_days_from_the_date_of_the_response() {
    // 2026-11-08 is a Sunday.
    let w = parse(FORECAST).unwrap();
    assert_eq!(
        w.day_names,
        ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
    );
}

#[test]
fn parses_the_days() {
    let w = parse(FORECAST).unwrap();
    let v = json();
    for i in 0..DAYS {
        assert_eq!(w.daily_max[i], f(&v["daily"]["temperature_2m_max"][i]));
        assert_eq!(w.daily_min[i], f(&v["daily"]["temperature_2m_min"][i]));
        assert_eq!(
            w.daily_codes[i],
            v["daily"]["weather_code"][i].as_i64().unwrap() as i32
        );
    }
}

#[test]
fn the_hourly_window_starts_at_the_current_hour() {
    // The fixture's moment is 23:00, so the window runs into the next day.
    let w = parse(FORECAST).unwrap();
    let v = json();
    for i in 0..HOURS {
        let src = 23 + i;
        assert_eq!(w.hourly_temps[i], f(&v["hourly"]["temperature_2m"][src]));
        assert_eq!(
            w.hourly_precip[i],
            f(&v["hourly"]["precipitation_probability"][src])
        );
    }
    let times: Vec<&str> = w.hourly_times.iter().map(String::as_str).collect();
    assert_eq!(
        times,
        ["23:00", "00:00", "01:00", "02:00", "03:00", "04:00"]
    );
}

#[test]
fn a_different_current_hour_moves_the_window() {
    let mut v = json();
    v["current"]["time"] = "2026-11-08T05:45".into();
    let w = parse(&bytes(&v)).unwrap();
    assert_eq!(w.time, "05:45");
    assert_eq!(w.hourly_times[0], "05:00");
    assert_eq!(w.hourly_temps[0], f(&v["hourly"]["temperature_2m"][5]));
}

#[test]
fn a_missing_or_null_value_is_a_failed_fetch() {
    assert!(parse(b"").is_none());
    assert!(parse(b"{").is_none());
    for path in [
        ("current", "temperature_2m"),
        ("current", "weather_code"),
        ("current", "time"),
    ] {
        let mut v = json();
        v[path.0][path.1] = serde_json::Value::Null;
        assert!(parse(&bytes(&v)).is_none(), "{path:?}");
    }
    let mut v = json();
    v["daily"]["temperature_2m_max"][3] = serde_json::Value::Null;
    assert!(parse(&bytes(&v)).is_none());
    let mut v = json();
    v["hourly"]["precipitation_probability"][25] = serde_json::Value::Null;
    assert!(parse(&bytes(&v)).is_none());
}

#[test]
fn too_few_days_or_hours_is_a_failed_fetch() {
    let mut v = json();
    v["daily"]["temperature_2m_min"]
        .as_array_mut()
        .unwrap()
        .truncate(DAYS - 1);
    assert!(parse(&bytes(&v)).is_none());

    // The current hour is the last one in the list: no five hours follow it.
    let mut v = json();
    v["current"]["time"] = "2026-11-14T23:00".into();
    assert!(parse(&bytes(&v)).is_none());
}

#[test]
fn weather_codes_have_the_sketchs_descriptions() {
    assert_eq!(description(0), "Clear sky");
    assert_eq!(description(2), "Mainly clear, partly cloudy");
    assert_eq!(description(45), "Fog");
    assert_eq!(description(53), "Drizzle");
    assert_eq!(description(57), "Freezing Drizzle");
    assert_eq!(description(63), "Rain");
    assert_eq!(description(67), "Freezing Rain");
    assert_eq!(description(73), "Snowfall");
    assert_eq!(description(77), "Snow grains");
    assert_eq!(description(81), "Rain showers");
    assert_eq!(description(86), "Snow showers");
    assert_eq!(description(95), "Thunderstorm");
    assert_eq!(description(99), "Thunderstorm, hail");
    assert_eq!(description(12), "Unknown condition");
}

#[test]
fn weather_codes_map_to_icons_and_tints() {
    let table: &[(&[i32], &[u8; 288], Color6)] = &[
        (&[0], &icons::CLEAR_SKY, Color6::Yellow),
        (&[1, 2, 3], &icons::PARTLY_CLOUDY, Color6::Yellow),
        (&[45, 48], &icons::FOG, Color6::Black),
        (
            &[51, 53, 55, 56, 57, 61, 63, 65, 66, 67, 80, 81, 82],
            &icons::RAIN,
            Color6::Blue,
        ),
        (&[71, 73, 75, 77, 85, 86], &icons::SNOW, Color6::Black),
        (&[95, 96, 99], &icons::STORM, Color6::Red),
        (
            &[4, 10, 44, 46, 60, 70, 90, 100, -1],
            &icons::GRAY,
            Color6::Black,
        ),
    ];
    for (codes, icon, tint) in table {
        for &code in *codes {
            assert!(std::ptr::eq(weather_icon(code), *icon), "icon of {code}");
            assert_eq!(weather_color(code), *tint, "tint of {code}");
        }
    }
}

#[test]
fn battery_percent_runs_from_3_0_to_4_2_volts() {
    assert_eq!(battery_percent(4.5), 100);
    assert_eq!(battery_percent(4.2), 100);
    assert_eq!(battery_percent(3.6), 50);
    assert_eq!(battery_percent(3.0), 0);
    assert_eq!(battery_percent(2.0), 0);
}

#[test]
fn battery_icon_steps_at_25_50_and_75_percent() {
    assert!(std::ptr::eq(battery_icon(100), &icons::FULL_BATTERY));
    assert!(std::ptr::eq(battery_icon(75), &icons::FULL_BATTERY));
    assert!(std::ptr::eq(battery_icon(74), &icons::HIGH_BATTERY));
    assert!(std::ptr::eq(battery_icon(50), &icons::HIGH_BATTERY));
    assert!(std::ptr::eq(battery_icon(49), &icons::HALF_BATTERY));
    assert!(std::ptr::eq(battery_icon(25), &icons::HALF_BATTERY));
    assert!(std::ptr::eq(battery_icon(24), &icons::LOW_BATTERY));
    assert!(std::ptr::eq(battery_icon(0), &icons::LOW_BATTERY));
}

#[test]
fn the_dashboard_uses_every_ink_it_should() {
    let w = parse(FORECAST).unwrap();
    let mut frame = new_frame();
    draw_dashboard(&mut EpdBuffer::new(&mut frame), &w, 3.9);
    // Black frames and the info panel, the red location icon and temperature line, the yellow icons of the
    // days with clear or partly cloudy weather.
    assert!(count(&frame, Color6::Black) > 20_000, "frames and panel");
    assert!(count(&frame, Color6::Red) > 100, "icon and line");
    assert!(count(&frame, Color6::Yellow) > 100, "sun icons");
    assert!(count(&frame, Color6::White) > 50_000);
    assert_eq!(count(&frame, Color6::Green), 0);
}

#[test]
fn the_battery_is_hidden_without_one() {
    let w = parse(FORECAST).unwrap();
    let mut with = new_frame();
    draw_dashboard(&mut EpdBuffer::new(&mut with), &w, 3.9);
    let mut without = new_frame();
    draw_dashboard(&mut EpdBuffer::new(&mut without), &w, 0.0);
    // White ink on the black panel: the icon and the percentage.
    assert!(count(&without, Color6::White) < count(&with, Color6::White));
}

#[test]
fn a_flat_temperature_graph_still_draws() {
    let mut w = parse(FORECAST).unwrap();
    w.hourly_temps = [10.0; HOURS];
    w.hourly_precip = [0.0, 100.0, 5.0, 50.0, 0.0, 100.0];
    let mut frame = new_frame();
    draw_dashboard(&mut EpdBuffer::new(&mut frame), &w, 4.0);
    assert!(count(&frame, Color6::Blue) > 100, "rain bars");
}

#[test]
fn the_error_screen_is_red_over_black_on_white() {
    let mut frame = new_frame();
    draw_error(
        &mut EpdBuffer::new(&mut frame),
        "HTTP request failed.",
        "Check API URL or try again.",
    );
    assert!(count(&frame, Color6::Red) > 100);
    assert!(count(&frame, Color6::Black) > 100);
}
