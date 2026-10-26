#!/usr/bin/env bash
# Install the latest FFmpeg build that includes libvvenc (software H.266/VVC).
#
# Uses the BtbN FFmpeg-Builds "lgpl" artefacts, so the external binary does not
# pull GPL components into the toolchain. The binary is placed in
# .tools/ffmpeg/bin and is auto-detected by kine-encode. Override with
# KINE_FFMPEG=/path/to/ffmpeg if you already have a suitable build.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="${KINE_FFMPEG_DIR:-$ROOT/.tools/ffmpeg}"

case "$(uname -s)" in
    Linux*) OS=linux ;;
    MINGW*|MSYS*|CYGWIN*) OS=win ;;
    Darwin*)
        echo "No BtbN macOS build with libvvenc is published." >&2
        echo "Install FFmpeg with --enable-libvvenc and set KINE_FFMPEG=/path/to/ffmpeg." >&2
        exit 1
        ;;
    *)
        echo "Unsupported OS: $(uname -s)" >&2
        exit 1
        ;;
esac

case "$(uname -m)" in
    x86_64|amd64) ARCH=64 ;;
    aarch64|arm64) ARCH=arm64 ;;
    *)
        echo "Unsupported architecture: $(uname -m)" >&2
        exit 1
        ;;
esac

TARGET="${OS}${ARCH}"
if [[ "$OS" == "win" ]]; then
    EXT=zip
else
    EXT=tar.xz
fi
URL="https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-${TARGET}-lgpl.${EXT}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "==> Downloading latest LGPL FFmpeg (${TARGET})"
curl -fL --retry 3 --retry-delay 2 -o "$TMP/ffmpeg.${EXT}" "$URL"

echo "==> Extracting"
if [[ "$EXT" == "zip" ]]; then
    unzip -q "$TMP/ffmpeg.${EXT}" -d "$TMP"
else
    tar -xf "$TMP/ffmpeg.${EXT}" -C "$TMP"
fi

INNER="$(find "$TMP" -maxdepth 1 -mindepth 1 -type d | head -1)"
[[ -n "$INNER" ]] || { echo "cannot locate extracted FFmpeg directory" >&2; exit 1; }

rm -rf "$DEST"
mkdir -p "$DEST"
cp -a "$INNER"/. "$DEST"/

FFMPEG="$DEST/bin/ffmpeg"
[[ -x "$FFMPEG" ]] || { echo "FFmpeg binary not found after extraction" >&2; exit 1; }
if ! "$FFMPEG" -hide_banner -encoders 2>/dev/null | grep libvvenc >/dev/null; then
    echo "This FFmpeg build has no libvvenc encoder; cannot encode H.266." >&2
    exit 1
fi

echo "==> Installed $("$FFMPEG" -hide_banner -version | head -1)"
echo "==> H.266 encoder: libvvenc"
echo "KINE_FFMPEG=$FFMPEG"
