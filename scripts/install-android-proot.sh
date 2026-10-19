#!/usr/bin/env bash
# Kine on Android via proot.
#
# Runs in two modes and figures out which one it is:
#   1. Inside Termux (has `pkg`)  -> install proot-distro + Debian, then re-run
#      this script inside the container.
#   2. Inside the proot container -> install the toolchain and build Kine.
#
# Usage:
#   bash scripts/install-android-proot.sh            # default: Debian container
#   DISTRO=ubuntu bash scripts/install-android-proot.sh
#
# Notes:
#   * proot cannot expose /dev/dri, so the engine builds the deterministic CPU
#     path. This is expected: run `kine host` to confirm the classification.
#   * Outputs land in /sdcard/Download (see `kine host`). Run
#     `termux-setup-storage` in Termux if /sdcard is not visible.
set -euo pipefail

DISTRO="${DISTRO:-debian}"
RUST_TOOLCHAIN="${RUST_TOOLCHAIN:-1.98.1}"

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

in_termux() { [ -n "${TERMUX_VERSION:-}" ] || [ -d /data/data/com.termux/files/usr ]; }
in_proot()  { [ -n "${PROOT_LOADER:-}${PROOT_TMP_DIR:-}" ] || grep -qi proot /proc/version 2>/dev/null; }

# ── Mode 1: Termux host ──────────────────────────────────────────────────────
if in_termux && ! in_proot; then
  say "Termux detected — preparing proot-distro"

  command -v pkg >/dev/null || die "pkg not found; run this from Termux"
  pkg install -y proot-distro

  if [ ! -d "$PREFIX/var/lib/proot-distro/installed-rootfs/$DISTRO" ] \
     && [ ! -d "$PREFIX/../var/lib/proot-distro/installed-rootfs/$DISTRO" ]; then
    say "installing $DISTRO"
    proot-distro install "$DISTRO"
  else
    say "$DISTRO already installed"
  fi

  say "granting shared-storage access (needed for /sdcard/Download)"
  termux-setup-storage || true

  # Where the sources live; passed through to the container run.
  SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  say "entering $DISTRO to build (source: $SRC_DIR)"
  exec proot-distro login "$DISTRO" --bind "$SRC_DIR:/opt/kine" -- /bin/bash -lc \
    "RUST_TOOLCHAIN=$RUST_TOOLCHAIN bash /opt/kine/scripts/install-android-proot.sh && \
     cd /opt/kine && cargo build --release -p kine-cli && \
     echo && echo 'Built. Try:' && \
     echo '  /opt/kine/target/release/kine host' && \
     echo '  /opt/kine/target/release/kine render --scene /opt/kine/examples/hello/scene.ts --out hello.png'"
fi

# ── Mode 2: inside the proot container ───────────────────────────────────────
in_proot || die "not inside Termux nor proot; run this on Android, or see README for desktop Linux"

say "installing build toolchain inside $( . /etc/os-release 2>/dev/null && echo "${PRETTY_NAME:-container}" || echo container )"

if command -v apt-get >/dev/null; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update
  apt-get install -y --no-install-recommends \
    ca-certificates curl build-essential pkg-config git unzip
elif command -v apk >/dev/null; then
  apk add --no-cache ca-certificates curl build-base pkgconfig git unzip
elif command -v dnf >/dev/null; then
  dnf install -y ca-certificates curl gcc gcc-c++ make pkgconf-pkg-config git unzip
else
  die "no supported package manager found"
fi

# quickjs-ng (rquickjs-sys) needs a C compiler — provided by build-essential
# above. No other C dependency is required: layout, shaping, rasterization and
# MP4 muxing are all pure Rust.

if ! command -v cargo >/dev/null; then
  say "installing Rust $RUST_TOOLCHAIN"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain "$RUST_TOOLCHAIN" --profile minimal
fi

# shellcheck disable=SC1090
. "${CARGO_HOME:-$HOME/.cargo}/env" 2>/dev/null || true
say "rust: $(rustc --version 2>/dev/null || echo missing)"
say "done — build with: cargo build --release -p kine-cli"
