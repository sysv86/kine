#!/usr/bin/env bash
# Verify Kine's H.266/VVC bitstream against independent decoders.
#
# Requires `cargo`.  Optional decoders are auto-detected:
#   * vvdecapp / VVdeC  (VVdeC does not need to be installed; point VVDEC at it)
#   * ffmpeg with the native `vvc` decoder
#   * VTM DecoderApp (point VTM_DECODER at it)
#
# For every case the encoded stream is decoded and compared with the source
# I420 file (lossless QP) or with Kine's own reconstruction (lossy QP, which the
# spec decoder must reproduce exactly).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${TMPDIR:-/tmp}/kine-vvc-verify"
mkdir -p "$OUT"

echo "==> Building and encoding verification streams"
cargo build -q --release -p kine-encode --example vvc_dump
DUMP="$ROOT/target/release/examples/vvc_dump"
if [[ ! -x "$DUMP" ]]; then
    # Cargo names examples without the extra suffix on some platforms.
    DUMP="$(find "$ROOT/target/release" -maxdepth 2 -name 'vvc_dump*' -type f -perm -u+x | head -1)"
fi
[[ -x "$DUMP" ]] || { echo "cannot find the vvc_dump example binary" >&2; exit 1; }

# name width height frames quantizer mode
CASES=(
    "tiny 2 2 2 0"
    "odd 33 33 2 0"
    "crop 200 120 3 0"
    "ntsc 640 360 2 0"
    "lossy 320 240 2 100"
    "hd 1280 720 2 100"
    "noise 128 96 1 0 noise"
)

for spec in "${CASES[@]}"; do
    read -r name width height frames quantizer mode <<<"$spec"
    "$DUMP" "$OUT/$name" "$width" "$height" "$frames" "$quantizer" "$mode" >/dev/null
done

# --- VVdeC -----------------------------------------------------------------
if command -v vvdecapp >/dev/null 2>&1; then
    VVDEC_BIN="$(command -v vvdecapp)"
elif [[ -n "${VVDEC:-}" && -x "${VVDEC}" ]]; then
    VVDEC_BIN="${VVDEC}"
else
    VVDEC_BIN=""
fi
if [[ -n "$VVDEC_BIN" ]]; then
    for spec in "${CASES[@]}"; do
        read -r name width height frames quantizer _mode <<<"$spec"
        ref="$OUT/$name.yuv"
        [[ "$quantizer" != "0" ]] && ref="$OUT/$name.recon.yuv"
        dec="$OUT/decoded-vvdec-$name.yuv"
        "$VVDEC_BIN" -b "$OUT/$name.vvc" -o "$dec" >/dev/null
        cmp -s "$ref" "$dec" || { echo "FAIL: vvdec mismatch on $name" >&2; exit 1; }
        echo "==> vvdec $name: byte-exact"
    done
else
    echo "==> VVdeC not found (set VVDEC=/path/to/vvdecapp); skipping"
fi

# --- FFmpeg ----------------------------------------------------------------
if command -v ffmpeg >/dev/null 2>&1 && ffmpeg -hide_banner -decoders 2>/dev/null | grep "VVC (Versatile Video Coding)" >/dev/null; then
    for spec in "${CASES[@]}"; do
        read -r name width height frames quantizer _mode <<<"$spec"
        ref="$OUT/$name.yuv"
        [[ "$quantizer" != "0" ]] && ref="$OUT/$name.recon.yuv"
        dec="$OUT/decoded-ffmpeg-$name.yuv"
        ffmpeg -hide_banner -v error -i "$OUT/$name.vvc" -f rawvideo \
            -pix_fmt yuv420p "$dec" -y
        cmp -s "$ref" "$dec" || { echo "FAIL: ffmpeg mismatch on $name" >&2; exit 1; }
        echo "==> ffmpeg $name: byte-exact"
    done
else
    echo "==> ffmpeg with a native VVC decoder not found; skipping"
fi

# --- VTM -------------------------------------------------------------------
if [[ -n "${VTM_DECODER:-}" && -x "${VTM_DECODER}" ]]; then
    for spec in "${CASES[@]}"; do
        read -r name width height frames quantizer _mode <<<"$spec"
        ref="$OUT/$name.yuv"
        [[ "$quantizer" != "0" ]] && ref="$OUT/$name.recon.yuv"
        dec="$OUT/decoded-vtm-$name.yuv"
        "$VTM_DECODER" -b "$OUT/$name.vvc" -o "$dec" >/dev/null
        cmp -s "$ref" "$dec" || { echo "FAIL: VTM mismatch on $name" >&2; exit 1; }
        echo "==> VTM $name: byte-exact"
    done
else
    echo "==> VTM DecoderApp not configured (set VTM_DECODER=...); skipping"
fi

echo
echo "All available VVC decoders reproduced Kine's output byte-exactly."
