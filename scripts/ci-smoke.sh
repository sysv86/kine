#!/usr/bin/env bash
# End-to-end CLI & Studio smoke check for Linux, Windows (Git Bash), and macOS CI runners.
set -euo pipefail

OUT_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t 'kine-smoke')"
trap 'rm -rf "$OUT_DIR"' EXIT

echo "==> [1/7] Building kine & kine-studio binaries"
cargo build --bins

KINE_BIN="./target/debug/kine"
if [ -f "./target/debug/kine.exe" ]; then
    KINE_BIN="./target/debug/kine.exe"
fi

echo "==> [2/7] Checking host & doctor diagnostics"
"$KINE_BIN" host
"$KINE_BIN" doctor

echo "==> [3/7] Emitting TypeScript ambient declarations"
"$KINE_BIN" types --out "$OUT_DIR/kine.d.ts"
test -s "$OUT_DIR/kine.d.ts"

echo "==> [4/7] Generating starter scene and inspecting"
"$KINE_BIN" new --out "$OUT_DIR/starter.json"
"$KINE_BIN" inspect --scene "$OUT_DIR/starter.json"

echo "==> [5/7] Rendering still frame (PNG) and vector (SVG)"
"$KINE_BIN" render --scene "$OUT_DIR/starter.json" --out "$OUT_DIR/frame0.png" --frame 0
"$KINE_BIN" render --scene "$OUT_DIR/starter.json" --out "$OUT_DIR/frame0.svg" --frame 0
test -s "$OUT_DIR/frame0.png"
test -s "$OUT_DIR/frame0.svg"

echo "==> [6/7] Rendering short MP4 animation"
"$KINE_BIN" render --scene "$OUT_DIR/starter.json" --out "$OUT_DIR/clip.mp4" --frames 6 --fps 30
test -s "$OUT_DIR/clip.mp4"

echo "==> [7/7] Verifying in-process Kine Studio state & studio_look tool"
"$KINE_BIN" studio --scene "$OUT_DIR/starter.json" --headless-check

echo "==> Smoke verification passed."
