//! Host tests for the hardware-free code of the examples. `build.rs` copies the `mod ui` block out of
//! each example, and `common/gfx.rs` and `common/icons.rs` are included as they are, so what is tested
//! is the code the board runs. Run with `./host-tests/run.sh`.

extern crate alloc;

#[path = "../../examples/common/gfx.rs"]
pub mod gfx;
#[path = "../../examples/common/icons.rs"]
pub mod icons;

/// What the examples see as `crate::common`, without the panel and the network.
pub mod common {
    pub use crate::gfx::*;
    pub use crate::icons;
}

/// The escape-time colours of `mandelbrot`.
pub mod mandelbrot {
    include!(concat!(env!("OUT_DIR"), "/mandelbrot_ui.rs"));
}

/// The parsing and drawing of `quotables`.
pub mod quotables {
    include!(concat!(env!("OUT_DIR"), "/quotables_ui.rs"));
}

/// The parsing, date lines and drawing of `news`.
pub mod news {
    include!(concat!(env!("OUT_DIR"), "/news_ui.rs"));
}

/// The request, parsing, mappings and drawing of `weather_station`.
pub mod weather_station {
    include!(concat!(env!("OUT_DIR"), "/weather_station_ui.rs"));
}
