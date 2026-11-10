//! Tests of the `mod ui` block of `examples/mandelbrot.rs`.

use host_tests::gfx::Color6;
use host_tests::mandelbrot::*;

#[test]
fn points_in_the_set_are_black() {
    assert_eq!(color_at(0.0, 0.0), Color6::Black);
    assert_eq!(color_at(-0.5, 0.0), Color6::Black);
    assert_eq!(color_at(-1.0, 0.0), Color6::Black);
}

#[test]
fn points_that_escape_at_once_are_the_first_palette_color() {
    assert_eq!(color_at(2.0, 2.0), Color6::White);
}

#[test]
fn the_escape_time_picks_the_band() {
    // z goes 0, 1, 2, 5: three steps before it leaves the radius 2 disc.
    assert_eq!(color_at(1.0, 0.0), PALETTE[1]);
}

#[test]
fn a_scan_over_the_whole_set_uses_every_color() {
    let mut seen = Vec::new();
    let range_y = RANGE_X * 480.0 / 800.0;
    for j in 0..48 {
        for i in 0..80 {
            let x = CENTER_X - RANGE_X / 2.0 + i as Real * RANGE_X / 80.0;
            let y = CENTER_Y - range_y / 2.0 + j as Real * range_y / 48.0;
            seen.push(color_at(x, y));
        }
    }
    for color in [
        Color6::Black,
        Color6::White,
        Color6::Yellow,
        Color6::Red,
        Color6::Green,
        Color6::Blue,
    ] {
        assert!(seen.contains(&color), "{color:?} never appears");
    }
}
