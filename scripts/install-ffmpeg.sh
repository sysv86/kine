#!/usr/bin/env bash
# Install a recent FFmpeg build with AV1 and H.264 encoders.
#
# Kine encodes AV1 and H.264 through an external FFmpeg process. On Linux and
# Windows the script downloads the latest BtbN FFmpeg-Builds "lgpl" artefact, so
# the external binary does not pull GPL components into the toolchain. On macOS
# BtbN publishes no build, so the script validates an existing `ffmpeg`.
#
# The binary is placed in .tools/ffmpeg/bin and is auto-detected by
# kine-encode. Override with KINE_FFMPEG=/path/to/ffmpeg to use another build.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="${KINE_FFMPEG_DIR:-$ROOT/.tools/ffmpeg}"

# Print the first matching encoder name from an FFmpeg build, or nothing.
first_encoder() {
    local ffmpeg="$1"
    shift
    local list name
    list="$("$ffmpeg" -hide_banner -encoders 2>/dev/null)"
    for name in "$@"; do
        if printf '%s\n' "$list" | grep -E "[[:space:]]${name}[[:space:]]" >/dev/null; then
            echo "$name"
            return 0
        fi
    done
}

verify_ffmpeg() {
    local ffmpeg="$1"
    local av1 h264
    av1="$(first_encoder "$ffmpeg" libsvtav1 libaom-av1 || true)"
    h264="$(first_encoder "$ffmpeg" libopenh264 libx264 || true)"
    if [[ -z "$av1" ]]; then
        echo "This FFmpeg build has no AV1 encoder (libsvtav1 or libaom-av1)." >&2
        return 1
    fi
    if [[ -z "$h264" ]]; then
        echo "This FFmpeg build has no H.264 encoder (libopenh264 or libx264)." >&2
        return 1
    fi
    echo "==> AV1 encoder: $av1"
    echo "==> H.264 encoder: $h264"
}

case "$(uname -s)" in
    Linux*) OS=linux ;;
    MINGW*|MSYS*|CYGWIN*) OS=win ;;
    Darwin*)
        # No BtbN macOS artefact: validate the local Homebrew/MacPorts build.
        if ! command -v ffmpeg >/dev/null 2>&1; then
            echo "FFmpeg is not installed. Run: brew install ffmpeg" >&2
            echo "Then set KINE_FFMPEG=\$(command -v ffmpeg) or leave it on PATH." >&2
            exit 1
        fi
        FFMPEG="$(command -v ffmpeg)"
        verify_ffmpeg "$FFMPEG"
        echo "==> Installed $("$FFMPEG" -hide_banner -version | head -1)"
        echo "KINE_FFMPEG=$FFMPEG"
        exit 0
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
verify_ffmpeg "$FFMPEG"

echo "==> Installed $("$FFMPEG" -hide_banner -version | head -1)"
echo "KINE_FFMPEG=$FFMPEG"
