//! Copies the `mod ui { ... }` block of each example into `OUT_DIR`, so the tests exercise the
//! real code and not a copy that can drift.

use std::{env, fs, path::Path};

/// The body of `mod ui { ... }` in `source`: un-indented, with inner doc comments turned into
/// plain ones because the block is included into a module, not a file of its own.
fn extract(source: &str) -> String {
    let marker = "mod ui {\n";
    let start = source.find(marker).expect("`mod ui` in the example") + marker.len();
    let mut out = String::new();
    for line in source[start..].lines() {
        if line == "}" {
            break;
        }
        let line = line.strip_prefix("    ").unwrap_or(line);
        match line.strip_prefix("//!") {
            Some(rest) => {
                out.push_str("//");
                out.push_str(rest);
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

fn main() {
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    for example in ["mandelbrot", "quotables", "news", "weather_station"] {
        let path = format!("../examples/{example}.rs");
        println!("cargo:rerun-if-changed={path}");
        let source = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        fs::write(
            Path::new(&out_dir).join(format!("{example}_ui.rs")),
            extract(&source),
        )
        .expect("write the extracted block");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
