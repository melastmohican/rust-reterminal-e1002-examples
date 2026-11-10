//! The hardware-free half of `common`: colours, the frame buffer, fonts, text helpers and the calendar.
//!
//! `host-tests` includes this file as it is, so what the tests exercise is what the board runs.

use alloc::{string::String, vec::Vec};
use embedded_graphics::{
    Pixel,
    geometry::{OriginDimensions, Point, Size},
    pixelcolor::PixelColor,
    prelude::DrawTarget,
};
use epdsi::traits::SevenColor;
use u8g2_fonts::{
    FontRenderer, fonts,
    types::{FontColor, HorizontalAlignment, VerticalPosition},
};

pub const WIDTH: usize = 800;
pub const HEIGHT: usize = 480;
/// Packed frame size: 2 pixels per byte (4-bit nibbles).
pub const FRAME_BYTES: usize = WIDTH * HEIGHT / 2;

// =====================================================================
// Colours and the frame buffer
// =====================================================================

/// The 6 native colours of the GDEP073E01 (E Ink Spectra 6).
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

/// An `embedded-graphics` target over a packed 4 bpp frame of [`FRAME_BYTES`].
pub struct EpdBuffer<'a> {
    buf: &'a mut [u8],
}

impl<'a> EpdBuffer<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        assert!(buf.len() >= FRAME_BYTES);
        Self { buf }
    }

    pub fn clear(&mut self, color: Color6) {
        let sc: SevenColor = color.into();
        self.buf.fill(SevenColor::pack(sc, sc));
    }

    #[allow(clippy::manual_is_multiple_of)]
    pub fn set_pixel(&mut self, x: u32, y: u32, color: Color6) {
        if x >= WIDTH as u32 || y >= HEIGHT as u32 {
            return;
        }
        let byte_idx = (y as usize * WIDTH + x as usize) / 2;
        let val = SevenColor::from(color) as u8;
        if x % 2 == 0 {
            self.buf[byte_idx] = (self.buf[byte_idx] & 0x0F) | (val << 4);
        } else {
            self.buf[byte_idx] = (self.buf[byte_idx] & 0xF0) | (val & 0x0F);
        }
    }
}

impl OriginDimensions for EpdBuffer<'_> {
    fn size(&self) -> Size {
        Size::new(WIDTH as u32, HEIGHT as u32)
    }
}

impl DrawTarget for EpdBuffer<'_> {
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

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        EpdBuffer::clear(self, color);
        Ok(())
    }
}

/// A white frame in the heap.
pub fn new_frame() -> Vec<u8> {
    let white = SevenColor::pack(SevenColor::White, SevenColor::White);
    alloc::vec![white; FRAME_BYTES]
}

// =====================================================================
// Text
// =====================================================================
//
// u8g2 fonts stand in for the sketches' Adafruit GFX fonts, at about the same size: FreeSans 9, 12 and
// 18 pt become Helvetica 14, 18 and 24 px; FreeSerif 12 and 24 pt become New Century Schoolbook 18 and 24.
// There is no italic serif, so the italic faces are upright.

pub const SANS_9: FontRenderer = FontRenderer::new::<fonts::u8g2_font_helvR14_tf>();
pub const SANS_12: FontRenderer = FontRenderer::new::<fonts::u8g2_font_helvR18_tf>();
pub const SANS_18: FontRenderer = FontRenderer::new::<fonts::u8g2_font_helvB24_tf>();
pub const SERIF_BOLD_12: FontRenderer = FontRenderer::new::<fonts::u8g2_font_ncenB18_tf>();
pub const SERIF_BOLD_24: FontRenderer = FontRenderer::new::<fonts::u8g2_font_ncenB24_tf>();
pub const SERIF_24: FontRenderer = FontRenderer::new::<fonts::u8g2_font_ncenR24_tf>();

/// Draws `text` with its baseline at `y`; `x` is the left, centre or right edge.
pub fn draw_text<D: DrawTarget<Color = Color6>>(
    target: &mut D,
    font: &FontRenderer,
    text: &str,
    x: i32,
    y: i32,
    align: HorizontalAlignment,
    color: Color6,
) {
    let _ = font.render_aligned(
        text,
        Point::new(x, y),
        VerticalPosition::Baseline,
        align,
        FontColor::Transparent(color),
        target,
    );
}

/// Width of `text` in pixels.
pub fn text_width(font: &FontRenderer, text: &str) -> i32 {
    font.get_rendered_dimensions(text, Point::zero(), VerticalPosition::Baseline)
        .map(|d| d.advance.x)
        .unwrap_or(0)
}

/// The fonts have no glyphs beyond Latin-1: curly quotes, dashes and the ellipsis become their ASCII
/// look, and every other non-ASCII character is dropped.
pub fn ascii_fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            c if c.is_ascii() => out.push(c),
            '\u{2018}' | '\u{2019}' => out.push('\''),
            '\u{201C}' | '\u{201D}' => out.push('"'),
            '\u{2013}' | '\u{2014}' => out.push('-'),
            '\u{2026}' => out.push_str("..."),
            _ => {}
        }
    }
    out
}

/// The sketches' `drawWrapped`: breaks `text` at spaces into lines no wider than `width`, at most
/// `max_lines`, with `...` near the right end of the last line when words are left. `y` is the baseline
/// of the first line. Returns the baseline of the line after the last one.
#[allow(clippy::too_many_arguments)]
pub fn draw_wrapped<D: DrawTarget<Color = Color6>>(
    target: &mut D,
    font: &FontRenderer,
    x: i32,
    y: i32,
    width: i32,
    max_lines: usize,
    line_height: i32,
    text: &str,
    color: Color6,
) -> i32 {
    let n = text.len();
    let mut pos = 0;
    let mut line = 0usize;
    let mut current = String::new();
    while pos < n && line < max_lines {
        let rest = &text[pos..];
        let (word, advance) = match rest.find(' ') {
            Some(space) => (&rest[..space], space + 1),
            None => (rest, rest.len()),
        };
        pos += advance;
        let mut trial = current.clone();
        if !trial.is_empty() {
            trial.push(' ');
        }
        trial.push_str(word);
        if text_width(font, &trial) > width && !current.is_empty() {
            draw_text(
                target,
                font,
                &current,
                x,
                y + line as i32 * line_height,
                HorizontalAlignment::Left,
                color,
            );
            line += 1;
            current = String::from(word);
        } else {
            current = trial;
        }
    }
    if line < max_lines && !current.is_empty() {
        draw_text(
            target,
            font,
            &current,
            x,
            y + line as i32 * line_height,
            HorizontalAlignment::Left,
            color,
        );
        line += 1;
    } else if pos < n && line >= max_lines {
        draw_text(
            target,
            font,
            "...",
            x + width - 20,
            y + (max_lines as i32 - 1) * line_height,
            HorizontalAlignment::Left,
            color,
        );
    }
    y + line as i32 * line_height
}

/// A one-line message on a white panel, in red like the sketches' `drawMessage`.
pub fn draw_message<D: DrawTarget<Color = Color6>>(target: &mut D, message: &str) {
    let _ = target.clear(Color6::White);
    draw_text(
        target,
        &SANS_12,
        message,
        50,
        230,
        HorizontalAlignment::Left,
        Color6::Red,
    );
}

/// A civil date and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clock {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
}

/// The calendar date and time of Unix seconds plus a UTC offset in hours (after Howard Hinnant's
/// `civil_from_days`). The sketches use a fixed offset too.
pub fn local_clock(unix_utc: i64, utc_offset_hours: i32) -> Clock {
    let unix = unix_utc + i64::from(utc_offset_hours) * 3600;
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400) as u32;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as u16;
    Clock {
        year,
        month,
        day,
        hour: (secs / 3600) as u8,
        minute: (secs % 3600 / 60) as u8,
    }
}
