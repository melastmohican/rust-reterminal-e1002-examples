//! Helpers shared by the tests: reading a packed 4 bpp frame back.

#![allow(dead_code)]

use host_tests::gfx::{Color6, FRAME_BYTES, HEIGHT, WIDTH};

pub const ALL: [Color6; 6] = [
    Color6::Black,
    Color6::White,
    Color6::Yellow,
    Color6::Red,
    Color6::Blue,
    Color6::Green,
];

/// The nibble at `(x, y)` of a packed frame.
pub fn nibble(frame: &[u8], x: usize, y: usize) -> u8 {
    let byte = frame[(y * WIDTH + x) / 2];
    if x % 2 == 0 { byte >> 4 } else { byte & 0x0F }
}

/// How many pixels of `color` the frame holds.
pub fn count(frame: &[u8], color: Color6) -> usize {
    assert_eq!(frame.len(), FRAME_BYTES);
    let mut n = 0;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            if nibble(frame, x, y) == color as u8 {
                n += 1;
            }
        }
    }
    n
}
