#!/usr/bin/env bash
# Kine acceptance check for Android (proot).
#
# Proves the engine actually works on the phone: platform detection, rendering,
# static inspection, the pixel-diff gate, and writing into shared storage.
# Run from inside the proot container (see scripts/install-android-proot.sh).
#
#   bash scripts/android-smoke.sh [output-dir]
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

BIN="$REPO/target/release/kine"
OUT_DIR="${1:-}"

pass() { printf '  \033[32mok\033[0m   %s\n' "$*"; }
fail() { printf '  \033[31mFAIL\033[0m %s\n' "$*" >&2; exit 1; }

command -v cargo >/dev/null || fail "cargo not found; run scripts/install-android-proot.sh first"

echo "==> building release CLI"
cargo build --release -p kine-cli

if [ -z "$OUT_DIR" ]; then
  # Ask the engine where outputs belong on this host.
  OUT_DIR="$("$BIN" host | sed -n 's/.*"output_dir": "\([^"]*\)".*/\1/p')"
fi
[ -n "$OUT_DIR" ] || fail "could not determine output directory"
mkdir -p "$OUT_DIR"
echo "==> output dir: $OUT_DIR"

echo "==> platform detection"
HOST_JSON="$("$BIN" host)"
echo "$HOST_JSON" | grep -q '"kind": "Android' \
  || fail "expected Android classification, got: $HOST_JSON"
pass "classified as $(echo "$HOST_JSON" | sed -n 's/.*"kind": "\([^"]*\)".*/\1/p')"
echo "$HOST_JSON" | grep -q '"gpu_expected": false' \
  || fail "proot cannot reach the GPU; gpu_expected should be false"
pass "gpu_expected=false (CPU path is expected inside proot)"

echo "==> render into shared storage"
FRAME="$OUT_DIR/kine-smoke.png"
"$BIN" render --scene examples/hello/scene.ts --out "$FRAME"
[ -s "$FRAME" ] || fail "render produced no file at $FRAME"
pass "wrote $(wc -c <"$FRAME") bytes to $FRAME"

echo "==> static inspection (no rasterization)"
INSPECT="$("$BIN" inspect --scene examples/hello/scene.ts)"
echo "$INSPECT" | grep -q '"findings": \[\]' \
  || fail "example scene should be clean, got: $INSPECT"
pass "no findings"

echo "==> pixel-diff gate"
"$BIN" snapshot --scene examples/shaders/scene.ts --against "$FRAME" >/dev/null 2>&1 || true
"$BIN" snapshot --scene examples/hello/scene.ts --against "$FRAME" | grep -q '"passed": true' \
  || fail "snapshot against its own output must pass"
pass "identical frame passes the gate"

echo "==> WGSL validation (no GPU needed — this is the agent's shader loop)"
"$BIN" shader --file examples/shaders/grain.wgsl >/dev/null 2>&1 \
  || fail "the bundled example shader must validate"
pass "example shader validates and reflects its bindings"

BROKEN="$(mktemp)"
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
echo "$DIAG" | grep -q "line 6" || fail "the diagnostic must point at the agent's line 6: $DIAG"
pass "broken shader reported at the agent's own line, exit code 2"

echo "==> scene with an effect layer"
EFFECT_SCENE="$("$BIN" inspect --scene examples/shaders/scene.ts)"
echo "$EFFECT_SCENE" | grep -q '"severity": "Error"' \
  && fail "the shader example scene must have no errors: $EFFECT_SCENE"
# The host has no GPU, so the layer must be *reported*, not silently dropped.
echo "$EFFECT_SCENE" | grep -q 'shader-needs-gpu' \
  || fail "a validated-but-unexecutable layer must be reported: $EFFECT_SCENE"
pass "effect layer validates in-scene and is reported as GPU-only on this host"

echo "==> video render (frame -> MP4, pure Rust, no GPU)"
CLIP="$OUT_DIR/kine-smoke.mp4"
"$BIN" render --scene examples/motion/scene.ts --out "$CLIP" --frames 8 --fps 12 \
  --quality 160 --speed 10 2>&1 | tail -2
[ -s "$CLIP" ] || fail "video render produced no file at $CLIP"
pass "wrote $(wc -c <"$CLIP") bytes to $CLIP"

if command -v ffprobe >/dev/null; then
  CODEC="$(ffprobe -v error -show_entries stream=codec_name -of default=noprint_wrappers=1:nokey=1 "$CLIP")"
  FRAMES="$(ffprobe -v error -count_frames -show_entries stream=nb_read_frames -of default=noprint_wrappers=1:nokey=1 "$CLIP")"
  [ "$CODEC" = "av1" ] || fail "expected an av1 stream, got $CODEC"
  [ "$FRAMES" = "8" ] || fail "expected 8 decodable frames, got $FRAMES"
  pass "ffprobe: $CODEC, $FRAMES frames"
else
  echo "  (ffprobe not installed — skipping decode check)"
fi

echo "==> audio (tone -> mix -> FLAC in MP4)"
TONE="$OUT_DIR/kine-tone.wav"
"$BIN" tone --out "$TONE" --hz 440 --seconds 1 --gain 0.4 >/dev/null 2>&1 \
  || fail "tone generation failed"
pass "generated a test tone at $TONE"

# A scene that references the tone, written next to it so `src` resolves.
SCORED_DIR="$(dirname "$TONE")"
cat > "$SCORED_DIR/kine-smoke-scored.json" <<JSON
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
"$BIN" render --scene "$SCORED_DIR/kine-smoke-scored.json" --out "$SCORED" \
  --frames 12 --fps 12 --quality 170 --speed 10 2>&1 | tail -2
[ -s "$SCORED" ] || fail "scored render produced no file"
pass "wrote $(wc -c <"$SCORED") bytes to $SCORED"

if command -v ffprobe >/dev/null; then
  STREAMS="$(ffprobe -v error -show_entries stream=codec_name -of default=noprint_wrappers=1:nokey=1 "$SCORED" | tr '\n' ' ')"
  echo "$STREAMS" | grep -q "av1" || fail "no video stream in $STREAMS"
  echo "$STREAMS" | grep -q "flac" || fail "no audio stream in $STREAMS"
  pass "ffprobe: $STREAMS"
else
  echo "  (ffprobe not installed — skipping stream check)"
fi

echo
echo "Android smoke check passed."
echo "  frame:  $FRAME"
echo "  video:  $CLIP"
echo "  scored: $SCORED"
