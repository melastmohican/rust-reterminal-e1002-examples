//! # Mandelbrot set on the 6-colour panel (async)
//!
//! Port of the GxEPD2 `Mandelbrot` sketch (itself from the Soldered Inkplate 6COLOR example).
//!
//! Points inside the set are black; escaping points are coloured by how fast they escape, cycling
//! through white, yellow, red, green and blue. Every pixel is computed once, straight into the packed
//! frame buffer, and the panel is then written and refreshed once.
//!
//! The maths is `f32`, which is much faster than `f64` on the ESP32-S3. When zooming in far (range below
//! about 1e-5) switch `Real` to `f64`.
//!
//! The image is computed with the panel untouched, then written back to back. The sketch does the same: a
//! long pause between panel init and the data transfer made it show noise on some refreshes.
//!
//! Explore other regions: <http://www.cuug.ab.ca/dewara/mandelbrot/Mandelbrowser.html>
//!
//! ## Run
//!
//! ```bash
//! cargo run --release --example mandelbrot
//! ```
//!
//! ## Expected
//!
//! The log prints `Computed in N s`, then the refresh messages (25 to 30 s). The panel shows the whole set,
//! black on a coloured banded background. It renders once; reset to render again.

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use common::{EpdBuffer, HEIGHT, WIDTH};
use defmt::{error, info};
use embassy_time::Instant;
use esp_hal::clock::CpuClock;
use esp_println as _;
use ui::{CENTER_X, CENTER_Y, RANGE_X, Real, color_at};

esp_bootloader_esp_idf::esp_app_desc!();

/// The hardware-free part, tested on the host by `host-tests`.
mod ui {
    use crate::common::Color6;

    pub type Real = f32;

    pub const MAX_ITERATIONS: u32 = 150;

    /// Region to draw: centre and horizontal width. The height follows the 800 x 480 aspect.
    pub const CENTER_X: Real = -0.5;
    pub const CENTER_Y: Real = 0.0;
    /// The whole set. A zoomed example: centre (-0.7943, 0.1597), range 0.104.
    pub const RANGE_X: Real = 3.0;

    /// Colours of the escaping points, in the order they cycle.
    pub const PALETTE: [Color6; 5] = [
        Color6::White,
        Color6::Yellow,
        Color6::Red,
        Color6::Green,
        Color6::Blue,
    ];

    /// The colour of the point `x + yi`: black inside the set, else by how fast it escapes.
    pub fn color_at(x: Real, y: Real) -> Color6 {
        let (mut zr, mut zi) = (0.0, 0.0);
        let mut i = 0;
        while i < MAX_ITERATIONS && zr * zr + zi * zi <= 4.0 {
            let t = zr * zr - zi * zi + x;
            zi = 2.0 * zr * zi + y;
            zr = t;
            i += 1;
        }
        if i == MAX_ITERATIONS {
            Color6::Black
        } else {
            PALETTE[(i / 2) as usize % PALETTE.len()]
        }
    }
}

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    common::add_psram_to_heap(p.PSRAM);
    let timg0 = esp_hal::timer::timg::TimerGroup::new(p.TIMG0);
    let sw_interrupt = esp_hal::interrupt::software::SoftwareInterruptControl::new(p.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("[E1002] Mandelbrot: rendering");

    let range_y = RANGE_X * HEIGHT as Real / WIDTH as Real;
    let x_from = CENTER_X - RANGE_X / 2.0;
    let y_from = CENTER_Y - range_y / 2.0;

    let mut frame = common::new_frame();
    let started = Instant::now();
    {
        let mut target = EpdBuffer::new(&mut frame);
        for j in 0..HEIGHT {
            for i in 0..WIDTH {
                let color = color_at(
                    x_from + i as Real * RANGE_X / WIDTH as Real,
                    y_from + j as Real * range_y / HEIGHT as Real,
                );
                target.set_pixel(i as u32, j as u32, color);
            }
        }
    }
    info!("[E1002] Computed in {} s", started.elapsed().as_secs());

    let Some(mut epd) = common::Epd::new(
        p.SPI2, p.GPIO7, p.GPIO8, p.GPIO9, p.GPIO10, p.GPIO11, p.GPIO12, p.GPIO13,
    )
    .await
    else {
        error!("[E1002] display init failed");
        common::halt().await
    };
    epd.show(&frame).await;
    info!("[E1002] Done in {} s", started.elapsed().as_secs());

    // Render once. Re-flashing or pressing reset renders again; looping would needlessly wear the e-paper.
    common::halt().await
}
