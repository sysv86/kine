#!/usr/bin/env bash
# Launch Kine Studio with the optimized GPU build.
#
# Realtime preview is not usable from an unoptimized `cargo run` debug build:
# CPU-side scene evaluation + shader VM dominate the frame budget. This script
# always builds and runs the `release` profile, which uses the GPU backend when
# `cargo` was built with the default `gpu` feature (it is on by default).
set -euo pipefail
cd "$(dirname "$0")/.."
exec cargo run --release -p kine-studio -- "$@"
