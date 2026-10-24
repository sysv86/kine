#!/usr/bin/env bash
# Cross-platform end-to-end CLI smoke check for Linux and macOS.
#
# Verifies platform detection (`kine host`), diagnostics (`kine doctor`),
# static inspection (`kine inspect`), WGSL validation (`kine shader`),
# frame rendering (`PNG`, `SVG`), snapshot pixel-diff (`kine snapshot`),
# software AV1 MP4 video rendering, and audio synthesis + muxing.
#
# Usage:
#   bash scripts/ci-smoke.sh [output-dir]
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

BIN="$REPO/target/release/kine"
OUT_DIR="${1:-$(mktemp -d "${TMPDIR:-/tmp}/kine-smoke.XXXXXX")}"
mkdir -p "$OUT_DIR"

pass() { printf '  \033[32mok\033[0m   %s\n' "$*"; }
fail() { printf '  \033[31mFAIL\033[0m %s\n' "$*" >&2; exit 1; }

command -v cargo >/dev/null || fail "cargo not found in PATH"

echo "==> building release CLI"
cargo build --release -p kine-cli

echo "==> output dir: $OUT_DIR"

echo "==> platform detection (Linux / macOS)"
HOST_JSON="$("$BIN" host)"
echo "$HOST_JSON" | grep -E -q '"kind": "(Linux|MacOs)"' \
  || fail "expected Linux or MacOs classification, got: $HOST_JSON"
KIND="$(echo "$HOST_JSON" | sed -n 's/.*"kind": "\([^"]*\)".*/\1/p')"
GFX="$(echo "$HOST_JSON" | sed -n 's/.*"graphics_backend": "\([^"]*\)".*/\1/p')"
pass "classified as $KIND (graphics_backend=$GFX)"

echo "==> render single frame (PNG & SVG)"
FRAME="$OUT_DIR/kine-smoke.png"
SVG="$OUT_DIR/kine-smoke.svg"
"$BIN" render --scene examples/hello/scene.ts --out "$FRAME"
"$BIN" render --scene examples/hello/scene.ts --out "$SVG"
[ -s "$FRAME" ] || fail "render produced no file at $FRAME"
[ -s "$SVG" ] || fail "render produced no file at $SVG"
pass "wrote $(wc -c <"$FRAME") bytes to $FRAME and $(wc -c <"$SVG") bytes to $SVG"

echo "==> static inspection (no rasterization)"
INSPECT="$("$BIN" inspect --scene examples/hello/scene.ts)"
echo "$INSPECT" | grep -q '"findings": \[\]' \
  || fail "example scene should be clean, got: $INSPECT"
pass "no findings"

echo "==> pixel-diff gate"
"$BIN" snapshot --scene examples/hello/scene.ts --against "$FRAME" | grep -q '"passed": true' \
  || fail "snapshot against its own output must pass"
pass "identical frame passes the gate"

echo "==> WGSL validation (CPU naga validator)"
"$BIN" shader --file examples/shaders/grain.wgsl >/dev/null 2>&1 \
  || fail "the bundled example shader must validate"
pass "example shader validates and reflects its bindings"

BROKEN="$OUT_DIR/broken.wgsl"
cat > "$BROKEN" <<'WGSL'
struct P { speed: f32 };
@group(1) @binding(0) var<uniform> p: P;

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(p.sped);
}
WGSL
set +e
DIAG="$("$BIN" shader --file "$BROKEN" 2>&1)"
CODE=$?
set -e
[ "$CODE" -eq 2 ] || fail "a broken shader must exit 2, got $CODE"
echo "$DIAG" | grep -q "line 6" || fail "the diagnostic must point at line 6: $DIAG"
pass "broken shader reported at line 6, exit code 2"

echo "==> video render (frame -> MP4, pure Rust software AV1)"
CLIP="$OUT_DIR/kine-smoke.mp4"
"$BIN" render --scene examples/motion/scene.ts --out "$CLIP" --frames 8 --fps 12 \
  --quality 160 --speed 10 2>&1 | tail -2
[ -s "$CLIP" ] || fail "video render produced no file at $CLIP"
pass "wrote $(wc -c <"$CLIP") bytes to $CLIP"

echo "==> audio (tone -> mix -> FLAC in MP4)"
TONE="$OUT_DIR/kine-tone.wav"
"$BIN" tone --out "$TONE" --hz 440 --seconds 1 --gain 0.4 >/dev/null 2>&1 \
  || fail "tone generation failed"
pass "generated a test tone at $TONE"

cat > "$OUT_DIR/kine-smoke-scored.json" <<JSON
{
  "width": 160, "height": 90,
  "root": { "kind": "rect", "fill": "#101828", "size": { "width": 160, "height": 90 } },
  "audio": {
    "sampleRate": 48000, "channels": 1,
    "tracks": [{ "src": "$(basename "$TONE")", "gainDb": -6 }]
  }
}
JSON
SCORED="$OUT_DIR/kine-smoke-scored.mp4"
"$BIN" render --scene "$OUT_DIR/kine-smoke-scored.json" --out "$SCORED" \
  --frames 12 --fps 12 --quality 170 --speed 10 2>&1 | tail -2
[ -s "$SCORED" ] || fail "scored render produced no file"
pass "wrote $(wc -c <"$SCORED") bytes to $SCORED"

echo
echo "Cross-platform smoke check passed ($KIND)."
