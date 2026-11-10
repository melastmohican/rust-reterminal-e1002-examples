#!/bin/sh
# Runs the host tests. Cargo reads `.cargo/config.toml` from the directory it is started in and
# upwards, and the examples crate's one forces the ESP target, build-std and `-nostartfiles` on
# everything below it. So start from a neutral directory and point at this crate's manifest.
#
#   ./host-tests/run.sh             all tests
#   ./host-tests/run.sh weather     tests whose name contains "weather"
set -eu
DIR=$(cd "$(dirname "$0")" && pwd)
cd "${TMPDIR:-/tmp}"
exec cargo test --manifest-path "$DIR/Cargo.toml" "$@"
