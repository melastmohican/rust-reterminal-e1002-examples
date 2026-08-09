//! # 6-Color EPD Graphic Demo Example using `epdsi` & `embedded-graphics`
//!
//! Comprehensive 6-color demo for the Seeed Studio reTerminal E1002 carrier board,
//! based on the GxEPD2 Demo Arduino sketch for the GDEP073E01 e-Paper display.
//!
//! ## Display Specification
//! - **Panel:** Good Display GDEP073E01 (7.3" 800x480 6-Color ACeP / Spectra 6 e-Paper display)
//! - **Controller IC:** ED2208 (via local `epdsi` driver crate)
//! - **Host Board:** Seeed Studio reTerminal E1002 (XIAO ESP32-S3)
//! - **Native Palette:** Black, White, Yellow, Red, Blue, Green
//!
//! ## Hardware & Pin Mapping
//!
//! | Signal | GPIO | Notes |
//! |---|---|---|
//! | SPI SCK | GPIO7 | Shared SPI bus |
//! | SPI MISO | GPIO8 | Shared SPI bus |
//! | SPI MOSI | GPIO9 | Shared SPI bus |
//! | EPD CS | GPIO10 | Active-LOW Chip Select |
//! | EPD DC | GPIO11 | Data / Command Selection |
//! | EPD RES | GPIO12 | Hardware Reset |
//! | EPD BUSY | GPIO13 | Busy Signal (Active-LOW: LOW = Busy) |
//!
//! ## Workflow
//! Sequences through 6 demo screens rendered using `embedded-graphics`:
//! 1. **Splash Screen:** Titles, banners, colorful top/bottom stripes.
//! 2. **Color Palette:** 6 color swatches, background/foreground combination tiles, full-width color bars.
//! 3. **Color Typography:** Multi-color text, white text on colored badges, dark card with color text.
//! 4. **Color Geometry:** Cascading rects, colored circles, triangles, Olympic rings, concentric circles.
//! 5. **Color Patterns:** Color checkerboard, horizontal/vertical stripes, dot grid, color bar sequence.
//! 6. **Dashboard:** Status metric cards, activity log with colored dot indicators, multi-color progress bar.
//!
//! ## Run
//! ```bash
//! cargo run --example epd_ed2208_demo
//! ```

#![no_std]
#![no_main]

use defmt::{error, info};
use embassy_time::{Duration, Timer};
use embedded_graphics::{
    geometry::{Point, Size},
    mono_font::{
        ascii::{FONT_10X20, FONT_6X10, FONT_9X15, FONT_9X15_BOLD},
        MonoFont, MonoTextStyle,
    },
    pixelcolor::PixelColor,
    primitives::{
        Circle, Line, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle, RoundedRectangle, Triangle,
    },
    text::{Baseline, Text},
    Drawable, Pixel, prelude::*,
};
use embedded_hal_bus::spi::RefCellDevice;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::spi::Mode as SpiMode;
use esp_println as _;

use epdsi::controllers::Ed2208Controller;
use epdsi::driver::EpdBuilder;
use epdsi::panels::GDEP073E01;
use epdsi::traits::{ColorChannel, EpdPanel, SevenColor};
use epdsi::SpiBusWrapper;

esp_bootloader_esp_idf::esp_app_desc!();

/// Display Geometry
const WIDTH: usize = 800;
const HEIGHT: usize = 480;
/// Packed frame size: 2 pixels per byte (4-bit nibbles) -> 800 * 480 / 2 = 192,000 bytes
const FRAME_BYTES: usize = (WIDTH * HEIGHT) / 2;

/// Page delay between screens (milliseconds)
const PAGE_HOLD_SECS: u64 = 30;

/// Static 192 KB display frame buffer placed directly in DRAM (BSS)
static mut FRAME_BUFFER: [u8; FRAME_BYTES] = [0u8; FRAME_BYTES];

/// 6 Native Colors supported by the GDEP073E01 ACeP / Spectra 6 panel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color6 {
    Black = 0x00,
    White = 0x01,
    Yellow = 0x02,
    Red = 0x03,
    Blue = 0x05,
    Green = 0x06,
}

impl PixelColor for Color6 {
    type Raw = ();
}

impl From<Color6> for SevenColor {
    fn from(c: Color6) -> Self {
        match c {
            Color6::Black => SevenColor::Black,
            Color6::White => SevenColor::White,
            Color6::Yellow => SevenColor::Yellow,
            Color6::Red => SevenColor::Red,
            Color6::Blue => SevenColor::Blue,
            Color6::Green => SevenColor::Green,
        }
    }
}

/// Target buffer wrapping the raw packed 4-bit frame buffer for `embedded-graphics`
pub struct EpdBuffer<'a> {
    buf: &'a mut [u8; FRAME_BYTES],
    width: u32,
    height: u32,
}

impl<'a> EpdBuffer<'a> {
    pub fn new(buf: &'a mut [u8; FRAME_BYTES]) -> Self {
        Self {
            buf,
            width: WIDTH as u32,
            height: HEIGHT as u32,
        }
    }

    pub fn clear(&mut self, color: Color6) {
        let sc: SevenColor = color.into();
        let packed = SevenColor::pack(sc, sc);
        self.buf.fill(packed);
    }

    pub fn set_pixel(&mut self, x: u32, y: u32, color: Color6) {
        if x >= self.width || y >= self.height {
            return;
        }
        let byte_idx = (y as usize * self.width as usize + x as usize) / 2;
        let sc: SevenColor = color.into();
        let val = sc as u8;
        if x % 2 == 0 {
            // High nibble (bits 7..4)
            self.buf[byte_idx] = (self.buf[byte_idx] & 0x0F) | (val << 4);
        } else {
            // Low nibble (bits 3..0)
            self.buf[byte_idx] = (self.buf[byte_idx] & 0xF0) | (val & 0x0F);
        }
    }
}

impl<'a> OriginDimensions for EpdBuffer<'a> {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl<'a> DrawTarget for EpdBuffer<'a> {
    type Color = Color6;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if point.x >= 0 && point.y >= 0 {
                self.set_pixel(point.x as u32, point.y as u32, color);
            }
        }
        Ok(())
    }
}

// =====================================================================
// Graphic Helper Functions
// =====================================================================

fn draw_centered_text(
    target: &mut EpdBuffer,
    text: &str,
    y: i32,
    font: &MonoFont,
    color: Color6,
) {
    let character_style = MonoTextStyle::new(font, color);
    let text_width = text.len() as i32 * font.character_size.width as i32;
    let x = (target.width as i32 - text_width) / 2;
    let _ = Text::with_baseline(text, Point::new(x, y), character_style, Baseline::Top).draw(target);
}

fn draw_header(target: &mut EpdBuffer, title: &str, bg_color: Color6) {
    let header_rect = Rectangle::new(Point::new(0, 0), Size::new(800, 40));
    let _ = header_rect
        .into_styled(PrimitiveStyle::with_fill(bg_color))
        .draw(target);
    draw_centered_text(target, title, 12, &FONT_9X15_BOLD, Color6::White);
}

// =====================================================================
// Screen 1: Splash
// =====================================================================
fn show_screen_1_splash(target: &mut EpdBuffer) {
    target.clear(Color6::White);

    // Colorful top stripe — 6 native colors
    let stripe_colors = [
        Color6::Red,
        Color6::Yellow,
        Color6::Green,
        Color6::Blue,
        Color6::Black,
        Color6::Red,
    ];
    let stripe_w = (WIDTH as i32 - 20) / 6;
    for (i, &c) in stripe_colors.iter().enumerate() {
        let rect = Rectangle::new(
            Point::new(10 + i as i32 * stripe_w, 10),
            Size::new(stripe_w as u32, 12),
        );
        let _ = rect.into_styled(PrimitiveStyle::with_fill(c)).draw(target);
    }

    // Outer border frame
    let border = Rectangle::new(Point::new(10, 30), Size::new(WIDTH as u32 - 20, HEIGHT as u32 - 40));
    let _ = border
        .into_styled(PrimitiveStyle::with_stroke(Color6::Black, 1))
        .draw(target);

    // Centered titles
    draw_centered_text(target, "reTerminal E1002", 120, &FONT_10X20, Color6::Black);
    draw_centered_text(target, "7.3\" 6-Color e-Paper", 175, &FONT_9X15_BOLD, Color6::Red);

    // Blue horizontal divider line
    let line = Line::new(Point::new(200, 220), Point::new(600, 220));
    let _ = line
        .into_styled(PrimitiveStyle::with_stroke(Color6::Blue, 2))
        .draw(target);

    draw_centered_text(target, "epdsi + GDEP073E01 Demo", 250, &FONT_9X15_BOLD, Color6::Green);
    draw_centered_text(target, "800 x 480 | 6 Colors", 290, &FONT_9X15, Color6::Blue);

    // Colorful bottom stripe (reversed order)
    let bottom_colors = [
        Color6::Blue,
        Color6::Green,
        Color6::Yellow,
        Color6::Red,
        Color6::Black,
        Color6::Blue,
    ];
    for (i, &c) in bottom_colors.iter().enumerate() {
        let rect = Rectangle::new(
            Point::new(10 + i as i32 * stripe_w, HEIGHT as i32 - 22),
            Size::new(stripe_w as u32, 12),
        );
        let _ = rect.into_styled(PrimitiveStyle::with_fill(c)).draw(target);
    }

    draw_centered_text(target, "Seeed Studio x epdsi", HEIGHT as i32 - 42, &FONT_9X15, Color6::Black);
}

// =====================================================================
// Screen 2: Color Palette
// =====================================================================
fn show_screen_2_palette(target: &mut EpdBuffer) {
    target.clear(Color6::White);
    draw_header(target, "6-Color Palette", Color6::Black);

    let swatches = [
        (Color6::Black, "Black"),
        (Color6::White, "White"),
        (Color6::Red, "Red"),
        (Color6::Green, "Green"),
        (Color6::Blue, "Blue"),
        (Color6::Yellow, "Yellow"),
    ];

    let sw = 110u32;
    let sh = 130u32;
    let gap = 18i32;
    let sx = (WIDTH as i32 - (6 * sw as i32 + 5 * gap)) / 2;
    let sy = 60i32;

    for (i, &(color, name)) in swatches.iter().enumerate() {
        let x = sx + i as i32 * (sw as i32 + gap);
        let rrect = RoundedRectangle::with_equal_corners(
            Rectangle::new(Point::new(x, sy), Size::new(sw, sh)),
            Size::new(8, 8),
        );

        let style = PrimitiveStyleBuilder::new()
            .fill_color(color)
            .stroke_color(Color6::Black)
            .stroke_width(1)
            .build();
        let _ = rrect.into_styled(style).draw(target);

        // Label below swatch
        let text_style = MonoTextStyle::new(&FONT_9X15_BOLD, Color6::Black);
        let text_w = name.len() as i32 * 9;
        let text_x = x + (sw as i32 - text_w) / 2;
        let _ = Text::with_baseline(name, Point::new(text_x, sy + sh as i32 + 10), text_style, Baseline::Top).draw(target);
    }

    // Color combinations row
    let row2_y = sy + sh as i32 + 45;
    let text_style = MonoTextStyle::new(&FONT_9X15, Color6::Black);
    let _ = Text::with_baseline("Color combinations:", Point::new(sx, row2_y), text_style, Baseline::Top).draw(target);

    let bg_colors = [Color6::Red, Color6::Green, Color6::Blue, Color6::Yellow, Color6::Black];
    let fg_colors = [Color6::Yellow, Color6::Red, Color6::Yellow, Color6::Blue, Color6::Red];
    let card_cx = sx + 40;

    for i in 0..5 {
        let x = card_cx + i as i32 * 130;
        let card = RoundedRectangle::with_equal_corners(
            Rectangle::new(Point::new(x, row2_y + 25), Size::new(80, 80)),
            Size::new(8, 8),
        );
        let _ = card
            .into_styled(PrimitiveStyle::with_fill(bg_colors[i]))
            .draw(target);

        let circle = Circle::new(Point::new(x + 15, row2_y + 40), 50);
        let _ = circle
            .into_styled(PrimitiveStyle::with_fill(fg_colors[i]))
            .draw(target);
    }

    // Horizontal color bars
    let bar_y = row2_y + 120;
    let _ = Text::with_baseline("Full-width color bars:", Point::new(sx, bar_y), text_style, Baseline::Top).draw(target);

    let bar_colors = [Color6::Red, Color6::Yellow, Color6::Green, Color6::Blue, Color6::Black];
    let bar_w = (WIDTH as i32 - 2 * sx) as u32;

    for (i, &c) in bar_colors.iter().enumerate() {
        let bar = Rectangle::new(
            Point::new(sx, bar_y + 20 + i as i32 * 14),
            Size::new(bar_w, 12),
        );
        let _ = bar.into_styled(PrimitiveStyle::with_fill(c)).draw(target);
    }

    draw_centered_text(target, "All 6 native colors on the GDEP073E01 panel", HEIGHT as i32 - 20, &FONT_9X15, Color6::Black);
}

// =====================================================================
// Screen 3: Color Typography
// =====================================================================
fn show_screen_3_typography(target: &mut EpdBuffer) {
    target.clear(Color6::White);
    draw_header(target, "Color Typography", Color6::Blue);

    let x = 40i32;
    let mut y = 60i32;

    // Row 1: Large colored text
    let large_texts = [
        ("Black", Color6::Black, x),
        ("Red", Color6::Red, x + 200),
        ("Green", Color6::Green, x + 360),
        ("Blue", Color6::Blue, x + 560),
    ];
    for (t, c, px) in large_texts {
        let style = MonoTextStyle::new(&FONT_10X20, c);
        let _ = Text::with_baseline(t, Point::new(px, y), style, Baseline::Top).draw(target);
    }

    y += 45;
    let yellow_style = MonoTextStyle::new(&FONT_9X15_BOLD, Color6::Yellow);
    let _ = Text::with_baseline("Yellow text - warm and bright", Point::new(x, y), yellow_style, Baseline::Top).draw(target);

    y += 35;
    let red_style = MonoTextStyle::new(&FONT_9X15_BOLD, Color6::Red);
    let _ = Text::with_baseline("Red - emphasis and warnings", Point::new(x, y), red_style, Baseline::Top).draw(target);

    y += 35;
    let line = Line::new(Point::new(x, y), Point::new(WIDTH as i32 - x, y));
    let _ = line.into_styled(PrimitiveStyle::with_stroke(Color6::Red, 2)).draw(target);

    // Left column: White text on colored badges
    y += 20;
    let badge_colors = [Color6::Red, Color6::Green, Color6::Blue, Color6::Black];
    let badge_labels = [
        "White on Red",
        "White on Green",
        "White on Blue",
        "White on Black",
    ];

    for i in 0..4 {
        let by = y + i as i32 * 48;
        let badge = RoundedRectangle::with_equal_corners(
            Rectangle::new(Point::new(x, by), Size::new(220, 36)),
            Size::new(4, 4),
        );
        let _ = badge
            .into_styled(PrimitiveStyle::with_fill(badge_colors[i]))
            .draw(target);

        let style = MonoTextStyle::new(&FONT_9X15_BOLD, Color6::White);
        let _ = Text::with_baseline(badge_labels[i], Point::new(x + 12, by + 10), style, Baseline::Top).draw(target);
    }

    // Right column: Black box with colored text
    let rbx = x + 250;
    let card = RoundedRectangle::with_equal_corners(
        Rectangle::new(Point::new(rbx, y), Size::new(450, 180)),
        Size::new(6, 6),
    );
    let _ = card
        .into_styled(PrimitiveStyle::with_fill(Color6::Black))
        .draw(target);

    let right_colors = [
        ("Red Text", Color6::Red),
        ("Green Text", Color6::Green),
        ("Blue Text", Color6::Blue),
        ("Yellow Text", Color6::Yellow),
        ("White Text", Color6::White),
    ];
    for (i, (t, c)) in right_colors.iter().enumerate() {
        let style = MonoTextStyle::new(&FONT_9X15_BOLD, *c);
        let _ = Text::with_baseline(t, Point::new(rbx + 25, y + 15 + i as i32 * 32), style, Baseline::Top).draw(target);
    }

    draw_centered_text(target, "6-color text rendering", HEIGHT as i32 - 20, &FONT_9X15, Color6::Black);
}

// =====================================================================
// Screen 4: Color Geometry
// =====================================================================
fn show_screen_4_geometry(target: &mut EpdBuffer) {
    target.clear(Color6::White);
    draw_header(target, "Color Geometry", Color6::Green);

    // Cascading rectangles (left top)
    let rc_colors = [Color6::Red, Color6::Yellow, Color6::Green, Color6::Blue, Color6::Black];
    for (i, &c) in rc_colors.iter().enumerate() {
        let rect = Rectangle::new(
            Point::new(40 + i as i32 * 55, 55 + i as i32 * 14),
            Size::new(110, 65),
        );
        let _ = rect.into_styled(PrimitiveStyle::with_fill(c)).draw(target);
    }

    // Colored circles (right top)
    let cc_colors = [Color6::Blue, Color6::Red, Color6::Green, Color6::Yellow, Color6::Black];
    for (i, &c) in cc_colors.iter().enumerate() {
        let circle = Circle::new(Point::new(500 + i as i32 * 55, 75), 48);
        let _ = circle.into_styled(PrimitiveStyle::with_fill(c)).draw(target);
    }

    // Colored triangles (left middle)
    let tri_colors = [Color6::Red, Color6::Green, Color6::Blue, Color6::Yellow, Color6::Black];
    let ty = 200i32;
    for (i, &c) in tri_colors.iter().enumerate() {
        let tx = 40 + i as i32 * 130;
        let tri = Triangle::new(
            Point::new(tx, ty + 60),
            Point::new(tx + 30, ty),
            Point::new(tx + 60, ty + 60),
        );
        let _ = tri.into_styled(PrimitiveStyle::with_fill(c)).draw(target);
    }

    // Olympic rings (middle center)
    let oly_y = 320i32;
    let ox = 200i32;
    let oly_colors = [Color6::Blue, Color6::Black, Color6::Red, Color6::Yellow, Color6::Green];
    let oly_offsets = [(0, 0), (70, 0), (140, 0), (35, 35), (105, 35)];

    for (i, &(dx, dy)) in oly_offsets.iter().enumerate() {
        for r in 0..4 {
            let circle = Circle::new(Point::new(ox + dx - r, oly_y + dy - r), (56 - 2 * r) as u32);
            let _ = circle
                .into_styled(PrimitiveStyle::with_stroke(oly_colors[i], 1))
                .draw(target);
        }
    }

    // 6-color swatch grid (right bottom)
    let wx = 560i32;
    let wy = 290i32;
    let grid_colors = [
        (wx, wy, Color6::Red, false),
        (wx + 45, wy, Color6::Yellow, false),
        (wx + 90, wy, Color6::Green, false),
        (wx, wy + 45, Color6::Blue, false),
        (wx + 45, wy + 45, Color6::Black, false),
        (wx + 90, wy + 45, Color6::White, true),
    ];
    for (gx, gy, gc, stroke) in grid_colors {
        let square = Rectangle::new(Point::new(gx, gy), Size::new(45, 45));
        let style = if stroke {
            PrimitiveStyleBuilder::new()
                .fill_color(gc)
                .stroke_color(Color6::Black)
                .stroke_width(1)
                .build()
        } else {
            PrimitiveStyle::with_fill(gc)
        };
        let _ = square.into_styled(style).draw(target);
    }

    // Concentric circles (bottom left)
    let conc_colors = [Color6::Red, Color6::Yellow, Color6::Green, Color6::Blue, Color6::Black];
    for (r_idx, &c) in conc_colors.iter().enumerate() {
        let radius = 100 - r_idx as i32 * 20;
        let circle = Circle::new(
            Point::new(100 + r_idx as i32 * 10, 420 - radius / 2),
            radius as u32,
        );
        let _ = circle
            .into_styled(PrimitiveStyle::with_stroke(c, 3))
            .draw(target);
    }

    draw_centered_text(target, "Colorful GFX primitives", HEIGHT as i32 - 20, &FONT_9X15, Color6::Black);
}

// =====================================================================
// Screen 5: Color Patterns
// =====================================================================
fn show_screen_5_patterns(target: &mut EpdBuffer) {
    target.clear(Color6::White);
    draw_header(target, "Color Patterns", Color6::Red);

    let p_colors = [Color6::Red, Color6::Green, Color6::Blue, Color6::Yellow, Color6::Black];
    let font_style = MonoTextStyle::new(&FONT_9X15, Color6::Black);

    let bw = 150u32;
    let bh = 150u32;
    let gap = 25i32;
    let bx1 = 30i32;
    let by = 70i32;

    // Pattern 1: Color Check
    let _ = Text::with_baseline("Color Check", Point::new(bx1, by - 20), font_style, Baseline::Top).draw(target);
    for py in 0..10 {
        for px in 0..10 {
            let c = p_colors[(px + py) % 5];
            let rect = Rectangle::new(
                Point::new(bx1 + px as i32 * 15, by + py as i32 * 15),
                Size::new(15, 15),
            );
            let _ = rect.into_styled(PrimitiveStyle::with_fill(c)).draw(target);
        }
    }

    // Pattern 2: H-Stripes
    let bx2 = bx1 + bw as i32 + gap;
    let _ = Text::with_baseline("H-Stripes", Point::new(bx2, by - 20), font_style, Baseline::Top).draw(target);
    for py in 0..10 {
        let c = p_colors[py % 5];
        let rect = Rectangle::new(
            Point::new(bx2, by + py as i32 * 15),
            Size::new(bw, 15),
        );
        let _ = rect.into_styled(PrimitiveStyle::with_fill(c)).draw(target);
    }

    // Pattern 3: V-Stripes
    let bx3 = bx2 + bw as i32 + gap;
    let _ = Text::with_baseline("V-Stripes", Point::new(bx3, by - 20), font_style, Baseline::Top).draw(target);
    for px in 0..10 {
        let c = p_colors[px % 5];
        let rect = Rectangle::new(
            Point::new(bx3 + px as i32 * 15, by),
            Size::new(15, bh),
        );
        let _ = rect.into_styled(PrimitiveStyle::with_fill(c)).draw(target);
    }

    // Pattern 4: Color Dots
    let bx4 = bx3 + bw as i32 + gap;
    let _ = Text::with_baseline("Color Dots", Point::new(bx4, by - 20), font_style, Baseline::Top).draw(target);
    for py in 0..8 {
        for px in 0..8 {
            let c = p_colors[(px + py) % 5];
            let circle = Circle::new(
                Point::new(bx4 + 5 + px as i32 * 18, by + 5 + py as i32 * 18),
                10,
            );
            let _ = circle.into_styled(PrimitiveStyle::with_fill(c)).draw(target);
        }
    }

    // Bottom section: Color bar sequence
    let bar_y = by + bh as i32 + 35;
    let _ = Text::with_baseline("Color bar sequence:", Point::new(bx1, bar_y - 20), font_style, Baseline::Top).draw(target);

    let seq_w = (WIDTH as i32 - 2 * bx1) as u32;
    for (i, &c) in p_colors.iter().enumerate() {
        let bar = Rectangle::new(
            Point::new(bx1, bar_y + i as i32 * 24),
            Size::new(seq_w, 20),
        );
        let _ = bar.into_styled(PrimitiveStyle::with_fill(c)).draw(target);
    }

    draw_centered_text(target, "Patterns with the 6 native colors", HEIGHT as i32 - 20, &FONT_9X15, Color6::Black);
}

// =====================================================================
// Screen 6: Dashboard
// =====================================================================
fn draw_color_card(
    target: &mut EpdBuffer,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    title: &str,
    value: &str,
    unit: &str,
    accent: Color6,
) {
    // Card frame
    let outer = RoundedRectangle::with_equal_corners(
        Rectangle::new(Point::new(x, y), Size::new(w, h)),
        Size::new(6, 6),
    );
    let _ = outer
        .into_styled(PrimitiveStyle::with_stroke(Color6::Black, 1))
        .draw(target);

    // Accent Header bar
    let header = RoundedRectangle::with_equal_corners(
        Rectangle::new(Point::new(x + 2, y + 2), Size::new(w - 4, 26)),
        Size::new(4, 4),
    );
    let _ = header
        .into_styled(PrimitiveStyle::with_fill(accent))
        .draw(target);

    // Header Title
    let title_style = MonoTextStyle::new(&FONT_9X15_BOLD, Color6::White);
    let title_w = title.len() as i32 * 9;
    let title_x = x + (w as i32 - title_w) / 2;
    let _ = Text::with_baseline(title, Point::new(title_x, y + 7), title_style, Baseline::Top).draw(target);

    // Main Value
    let val_style = MonoTextStyle::new(&FONT_10X20, accent);
    let val_w = value.len() as i32 * 10;
    let val_x = x + (w as i32 - val_w) / 2;
    let _ = Text::with_baseline(value, Point::new(val_x, y + 50), val_style, Baseline::Top).draw(target);

    // Unit
    let unit_style = MonoTextStyle::new(&FONT_9X15, Color6::Black);
    let unit_w = unit.len() as i32 * 9;
    let unit_x = x + (w as i32 - unit_w) / 2;
    let _ = Text::with_baseline(unit, Point::new(unit_x, y + h as i32 - 25), unit_style, Baseline::Top).draw(target);
}

fn show_screen_6_dashboard(target: &mut EpdBuffer) {
    target.clear(Color6::White);
    draw_header(target, "Dashboard", Color6::Black);

    let cw = 170u32;
    let ch = 125u32;
    let gap = 20i32;
    let sx = (WIDTH as i32 - (4 * cw as i32 + 3 * gap)) / 2;
    let row1_y = 55i32;

    draw_color_card(target, sx, row1_y, cw, ch, "Temp", "23.5", "Celsius", Color6::Red);
    draw_color_card(target, sx + cw as i32 + gap, row1_y, cw, ch, "Humidity", "65", "% RH", Color6::Blue);
    draw_color_card(target, sx + 2 * (cw as i32 + gap), row1_y, cw, ch, "Heap", "284", "kB free", Color6::Green);
    draw_color_card(target, sx + 3 * (cw as i32 + gap), row1_y, cw, ch, "Uptime", "120", "seconds", Color6::Black);

    // Log area with colored markers
    let log_y = row1_y + ch as i32 + 15;
    let card_w = (WIDTH as i32 - 2 * sx) as u32;
    let outer_log = RoundedRectangle::with_equal_corners(
        Rectangle::new(Point::new(sx, log_y), Size::new(card_w, 185)),
        Size::new(6, 6),
    );
    let _ = outer_log
        .into_styled(PrimitiveStyle::with_stroke(Color6::Black, 1))
        .draw(target);

    let log_header = RoundedRectangle::with_equal_corners(
        Rectangle::new(Point::new(sx + 2, log_y + 2), Size::new(card_w - 4, 26)),
        Size::new(4, 4),
    );
    let _ = log_header
        .into_styled(PrimitiveStyle::with_fill(Color6::Blue))
        .draw(target);

    let log_title_style = MonoTextStyle::new(&FONT_9X15_BOLD, Color6::White);
    let _ = Text::with_baseline("Activity Log", Point::new(sx + 15, log_y + 7), log_title_style, Baseline::Top).draw(target);

    let logs = [
        ("System boot - ESP32-S3 (Embassy)", Color6::Green),
        ("Panel: GDEP073E01 6-Color (800x480)", Color6::Blue),
        ("ED2208 controller ready", Color6::Yellow),
        ("SPI @ 10MHz (HSPI)", Color6::Red),
        ("Demo: 6 screens completed", Color6::Green),
    ];

    let mut ly = log_y + 40;
    let log_text_style = MonoTextStyle::new(&FONT_6X10, Color6::Black);

    for (text, c) in logs {
        let dot = Circle::new(Point::new(sx + 18, ly + 2), 8);
        let _ = dot.into_styled(PrimitiveStyle::with_fill(c)).draw(target);

        let _ = Text::with_baseline(text, Point::new(sx + 35, ly), log_text_style, Baseline::Top).draw(target);
        ly += 26;
    }

    // Multi-color progress bar
    let bar_y = log_y + 195;
    let font_bold = MonoTextStyle::new(&FONT_9X15_BOLD, Color6::Black);
    let _ = Text::with_baseline("Progress:", Point::new(sx, bar_y + 2), font_bold, Baseline::Top).draw(target);

    let bar_x = sx + 100;
    let bar_w = (WIDTH as i32 - 2 * sx - 160) as u32;
    let bar_h = 20u32;

    let bar_box = Rectangle::new(Point::new(bar_x, bar_y), Size::new(bar_w, bar_h));
    let _ = bar_box
        .into_styled(PrimitiveStyle::with_stroke(Color6::Black, 1))
        .draw(target);

    let bar_colors = [Color6::Red, Color6::Yellow, Color6::Green, Color6::Blue, Color6::Black];
    let seg_w = bar_w / 5;
    for (i, &c) in bar_colors.iter().enumerate() {
        let seg = Rectangle::new(
            Point::new(bar_x + 1 + i as i32 * seg_w as i32, bar_y + 1),
            Size::new(seg_w - 1, bar_h - 2),
        );
        let _ = seg.into_styled(PrimitiveStyle::with_fill(c)).draw(target);
    }

    let pct_style = MonoTextStyle::new(&FONT_9X15_BOLD, Color6::Green);
    let _ = Text::with_baseline("100%", Point::new(bar_x + bar_w as i32 + 10, bar_y + 2), pct_style, Baseline::Top).draw(target);

    draw_centered_text(target, "6-color ePaper: vivid and power-efficient", HEIGHT as i32 - 20, &FONT_9X15, Color6::Black);
}

// =====================================================================
// Main Task
// =====================================================================

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    let mut delay = Delay::new();

    info!("\n============================================================");
    info!("[E1002] epdsi ED2208 / GDEP073E01 Comprehensive Demo");
    info!("============================================================");

    // 1. Static frame buffer reference
    let frame_buf: &'static mut [u8; FRAME_BYTES] =
        unsafe { &mut *core::ptr::addr_of_mut!(FRAME_BUFFER) };

    // 2. Configure shared SPI bus (HSPI: SCK=7, MISO=8, MOSI=9)
    let spi_bus = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(esp_hal::time::Rate::from_mhz(10))
            .with_mode(SpiMode::_0),
    )
    .unwrap()
    .with_sck(peripherals.GPIO7)
    .with_miso(peripherals.GPIO8)
    .with_mosi(peripherals.GPIO9);

    let spi_bus_cell = core::cell::RefCell::new(spi_bus);

    // 3. Configure EPD driver signals
    let epd_cs = Output::new(peripherals.GPIO10, Level::High, OutputConfig::default());
    let epd_dc = Output::new(peripherals.GPIO11, Level::Low, OutputConfig::default());
    let epd_rst = Output::new(peripherals.GPIO12, Level::High, OutputConfig::default());
    let epd_busy = Input::new(
        peripherals.GPIO13,
        InputConfig::default().with_pull(Pull::Up),
    );

    let epd_spi_dev = RefCellDevice::new_no_delay(&spi_bus_cell, epd_cs).unwrap();
    let bus = SpiBusWrapper::new(epd_spi_dev, epd_dc, epd_rst, epd_busy);

    let controller = Ed2208Controller::new(GDEP073E01::WIDTH, GDEP073E01::HEIGHT);
    let mut driver = EpdBuilder::<_, GDEP073E01>::new(controller).build(bus);

    info!("[EPD] Initializing ED2208 controller hardware...");
    if let Err(_e) = driver.init(&mut delay) {
        error!("[EPD] Driver initialization failed!");
    } else {
        info!("[EPD] Driver initialized successfully.");

        let screens: [(&str, fn(&mut EpdBuffer)); 6] = [
            ("Screen 1: Splash", show_screen_1_splash),
            ("Screen 2: Color Palette", show_screen_2_palette),
            ("Screen 3: Color Typography", show_screen_3_typography),
            ("Screen 4: Color Geometry", show_screen_4_geometry),
            ("Screen 5: Color Patterns", show_screen_5_patterns),
            ("Screen 6: Dashboard", show_screen_6_dashboard),
        ];

        for (idx, (name, draw_fn)) in screens.iter().enumerate() {
            info!("[E1002] Rendering {}/6: {}", idx + 1, name);

            let mut target = EpdBuffer::new(frame_buf);
            draw_fn(&mut target);

            info!("[EPD] Writing frame buffer to display driver...");
            if let Err(_e) = driver.write_frame(ColorChannel::Color7(0), frame_buf) {
                error!("[EPD] Write frame failed for {}", name);
            } else {
                info!("[EPD] Refreshing display (~25-30s)...");
                if let Err(_e) = driver.refresh(&mut delay) {
                    error!("[EPD] Refresh failed for {}", name);
                } else {
                    info!("[EPD] Refresh complete for {}.", name);
                }
            }

            info!("[E1002] Holding screen for {}s...", PAGE_HOLD_SECS);
            Timer::after(Duration::from_secs(PAGE_HOLD_SECS)).await;
        }

        info!("[EPD] Demo sequence complete. Putting display into deep sleep.");
        let _ = driver.sleep(&mut delay);
    }

    info!("[E1002] Entering idle loop.");
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", panic_info);
    loop {}
}
