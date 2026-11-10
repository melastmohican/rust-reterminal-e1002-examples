//! Tests of `examples/common/gfx.rs`: the colours, the frame buffer, the text helpers and the calendar.

mod support;

use host_tests::gfx::*;
use support::{ALL, count, nibble};

#[test]
fn colors_have_the_nibbles_the_controller_expects() {
    use epdsi::traits::SevenColor;
    for color in ALL {
        assert_eq!(SevenColor::from(color) as u8, color as u8, "{color:?}");
    }
}

#[test]
fn a_new_frame_is_white() {
    let frame = new_frame();
    assert_eq!(frame.len(), FRAME_BYTES);
    assert!(frame.iter().all(|&b| b == 0x11));
}

#[test]
fn pixels_pack_two_to_a_byte_high_nibble_first() {
    let mut frame = new_frame();
    let mut target = EpdBuffer::new(&mut frame);
    target.set_pixel(0, 0, Color6::Red);
    target.set_pixel(1, 0, Color6::Blue);
    target.set_pixel(799, 479, Color6::Green);
    assert_eq!(frame[0], 0x35);
    assert_eq!(nibble(&frame, 799, 479), Color6::Green as u8);
    assert_eq!(nibble(&frame, 2, 0), Color6::White as u8);
}

#[test]
fn pixels_outside_the_panel_are_ignored() {
    let mut frame = new_frame();
    let mut target = EpdBuffer::new(&mut frame);
    target.set_pixel(800, 0, Color6::Red);
    target.set_pixel(0, 480, Color6::Red);
    assert_eq!(count(&frame, Color6::Red), 0);
}

#[test]
fn clear_fills_the_frame() {
    let mut frame = new_frame();
    EpdBuffer::new(&mut frame).clear(Color6::Blue);
    assert_eq!(count(&frame, Color6::Blue), WIDTH * HEIGHT);
}

#[test]
fn ascii_fold_keeps_ascii_and_maps_typography() {
    assert_eq!(ascii_fold("plain text"), "plain text");
    assert_eq!(
        ascii_fold("\u{201C}Hi\u{201D} \u{2013} it\u{2019}s\u{2026}"),
        "\"Hi\" - it's..."
    );
    assert_eq!(ascii_fold("Caf\u{e9} \u{1F600}"), "Caf ");
}

#[test]
fn text_width_grows_with_the_text() {
    let a = text_width(&SANS_9, "a");
    let aaaa = text_width(&SANS_9, "aaaa");
    assert!(a > 0);
    assert_eq!(aaaa, 4 * a);
    assert_eq!(text_width(&SANS_9, ""), 0);
}

#[test]
fn draw_text_paints_ink_and_aligns() {
    let mut left = new_frame();
    draw_text(
        &mut EpdBuffer::new(&mut left),
        &SANS_12,
        "Hello",
        100,
        100,
        u8g2_fonts::types::HorizontalAlignment::Left,
        Color6::Red,
    );
    assert!(count(&left, Color6::Red) > 50);

    let mut right = new_frame();
    draw_text(
        &mut EpdBuffer::new(&mut right),
        &SANS_12,
        "Hello",
        100,
        100,
        u8g2_fonts::types::HorizontalAlignment::Right,
        Color6::Red,
    );
    // Right aligned text ends at x = 100, left aligned text starts there.
    let first_ink = |frame: &[u8]| {
        (0..WIDTH)
            .find(|&x| (0..HEIGHT).any(|y| nibble(frame, x, y) == Color6::Red as u8))
            .unwrap()
    };
    assert!(first_ink(&left) >= 98);
    assert!((first_ink(&right) as i32) < 100 - text_width(&SANS_12, "Hello") + 5);
}

#[test]
fn draw_wrapped_returns_the_baseline_after_the_last_line() {
    let mut frame = new_frame();
    let mut target = EpdBuffer::new(&mut frame);
    let one = draw_wrapped(
        &mut target,
        &SANS_9,
        10,
        50,
        700,
        3,
        20,
        "short",
        Color6::Black,
    );
    assert_eq!(one, 70);

    let long = "word ".repeat(80);
    let capped = draw_wrapped(
        &mut target,
        &SANS_9,
        10,
        50,
        300,
        2,
        20,
        long.trim(),
        Color6::Black,
    );
    assert_eq!(capped, 90);

    let none = draw_wrapped(&mut target, &SANS_9, 10, 50, 300, 2, 20, "", Color6::Black);
    assert_eq!(none, 50);
}

#[test]
fn draw_wrapped_breaks_lines_at_spaces_within_the_width() {
    let mut frame = new_frame();
    let text = "the quick brown fox jumps over the lazy dog";
    let width = text_width(&SANS_9, text) / 2 + 10;
    let next = draw_wrapped(
        &mut EpdBuffer::new(&mut frame),
        &SANS_9,
        0,
        30,
        width,
        6,
        20,
        text,
        Color6::Black,
    );
    let lines = (next - 30) / 20;
    assert!((2..=3).contains(&lines), "{lines} lines");
}

#[test]
fn draw_message_is_red_on_white() {
    let mut frame = new_frame();
    draw_message(&mut EpdBuffer::new(&mut frame), "WiFi connection failed.");
    assert!(count(&frame, Color6::Red) > 100);
    assert!(count(&frame, Color6::White) > WIDTH * HEIGHT / 2);
}

#[test]
fn local_clock_converts_unix_seconds() {
    let c = local_clock(0, 0);
    assert_eq!(
        (c.year, c.month, c.day, c.hour, c.minute),
        (1970, 1, 1, 0, 0)
    );
    let c = local_clock(0, -8);
    assert_eq!((c.year, c.month, c.day, c.hour), (1969, 12, 31, 16));
    // 2023-11-14 22:13:20 UTC.
    let c = local_clock(1_700_000_000, 0);
    assert_eq!(
        (c.year, c.month, c.day, c.hour, c.minute),
        (2023, 11, 14, 22, 13)
    );
    let c = local_clock(1_700_000_000, -8);
    assert_eq!((c.day, c.hour, c.minute), (14, 14, 13));
    // A leap day: 2024-02-29 12:00 UTC.
    let c = local_clock(1_709_208_000, 0);
    assert_eq!((c.year, c.month, c.day, c.hour), (2024, 2, 29, 12));
}
