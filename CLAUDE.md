# CLAUDE.md

Guidance for Claude Code when working in this repository.

## What this repo is

Embedded Rust examples for the **Seeed Studio reTerminal E1002** — a carrier board built
around the XIAO ESP32-S3 with a 7.3" e-paper display bonded to it. `no_std`, Xtensa
target `xtensa-esp32s3-none-elf`, using `esp-hal` with the Embassy async executor and
`defmt` logging over espflash.

Content lives in `examples/*.rs`; each is a complete `#![no_std] #![no_main]` image.

## Commands

```bash
cargo build --release --examples
cargo run --release --example <name>       # flash and monitor via espflash
cargo clippy --release --examples
```

The Xtensa target needs the `esp` toolchain fork (`espup`), not stock stable — unlike the
RISC-V ESP32 parts. Check `rustup toolchain list` for `esp` before assuming a build
failure is a code problem.

## The display

7.3" **GDEP073E01**, 800x480, driven by `Ed2208Controller` from
[`epdsi`](https://crates.io/crates/epdsi).

**It is an E Ink Spectra 6 (E6) panel — six colours: black, white, red, yellow, blue,
green.** Not 7-colour ACeP, despite the shared 4 bpp palette encoding. The vendor part is
`GDEP073E01(E6)`. Consequently `SevenColor::Orange` is **not renderable here**; it belongs
to the older ACeP-7 generation and produces an undefined colour. `SevenColor::Clean`
renders as white.

The panel is bonded to the carrier, so there is no FPC to reseat — which removes the most
common source of e-paper trouble.

## E-paper behaviour

**Power-cycle, run once, do not interrupt, then judge.**

E-paper retains its last write, and the controller can be left latched busy by an
interrupted run. The *next* run then hits the driver's busy timeouts and looks broken:
shifted content, refreshes returning instantly, or refreshes that appear to hang. A reset
does not clear it; only removing power does.

Colour panels have **no fast waveform** — the pigment needs the full OTP waveform to
migrate, so every update takes seconds. That is physics, not a fault. Do not select a
partial or fast refresh mode to "fix" it.

### Debugging discipline

- **Suspect hardware and panel state before software.** Running stock Arduino GxEPD2 on
  comparable hardware has twice found in one experiment what hours of driver analysis did
  not.
- **Validate any new diagnostic against a known measurement.** A refresh reported far
  faster than the panel can physically manage means the instrument is broken.
- **Watch the panel, not the log.** A hand-rolled trigger sequence once reported entirely
  plausible timings while never driving the display at all.
- **Never reason from an interrupted run**, or from any run after one, until the board has
  been power-cycled.

Fuller notes, including reference timings and known board/panel incompatibilities, live in
a sibling repository:
<https://github.com/melastmohican/xiao-esp32c3-blinky/blob/main/BRINGUP.md>

## The epdsi dependency

Currently a **git dependency** on `melastmohican/epdsi`. It can move to the published
crate when convenient — `epdsi = { version = "0.1.2", features = ["defmt"] }`. Note the
`defmt` feature was a no-op before 0.1.1: it was declared in `Cargo.toml` but no code
referenced it. From 0.1.1 it genuinely derives `defmt::Format` on the public error and
mode enums.

## Related repositories

- [`epdsi`](https://github.com/melastmohican/epdsi) — the driver framework itself
- [`rust-rpico2-discovery`](https://github.com/melastmohican/rust-rpico2-discovery) —
  RP2350, `rp-hal`, blocking
- [`xiao-esp32c3-blinky`](https://github.com/melastmohican/xiao-esp32c3-blinky) —
  XIAO ESP32-C3, four panels on the Seeed ePaper Driver Board

The same `epdsi` code drives panels across all three: Cortex-M, RISC-V and Xtensa, with
both blocking and async HALs. Keep example structure close to the siblings where it costs
nothing — that portability is the point being demonstrated.

## Git

Commit directly to `main`. Do not open pull requests for the owner's own changes.
