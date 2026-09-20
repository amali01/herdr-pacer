#!/usr/bin/env bash
# herdr-pacer — herdr-build.sh
# Builds a Herdr with herdr-glue.patch applied. The sidebar gauges need the
# `glue` token option; stock Herdr always inserts " · " between row tokens, so
# the used arc and the track cannot sit flush.
#
# Usage: ./herdr-build.sh [version]     (default: the installed herdr's version)
# Leaves the binary at target/release/herdr and prints how to install it.
# Rerun this after a Herdr upgrade to rebase the patch onto the new release.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
VERSION="${1:-$(herdr --version 2>/dev/null | awk '{print $2}')}"
SRC="${HERDR_PACER_SRC_DIR:-${TMPDIR:-/tmp}/herdr-src-$VERSION}"
ZIG_VERSION=0.16.0

[[ -n $VERSION ]] || { echo "could not determine the herdr version; pass one" >&2; exit 1; }
command -v cargo > /dev/null || { echo "cargo not found; install Rust: https://rustup.rs" >&2; exit 1; }

if [[ ! -d $SRC ]]; then
  echo "==> cloning herdr v$VERSION"
  git clone --depth 1 --branch "v$VERSION" https://github.com/herdrdev/herdr.git "$SRC"
fi

echo "==> applying herdr-glue.patch"
cd "$SRC"
git apply --check "$ROOT/herdr-glue.patch" 2>/dev/null &&
  git apply "$ROOT/herdr-glue.patch" ||
  echo "    already applied (or needs a rebase onto v$VERSION)"

# the vendored libghostty-vt is built with Zig, which Herdr does not vendor
ZIG="${ZIG:-$(command -v zig || true)}"
if [[ -z $ZIG ]]; then
  cache="${TMPDIR:-/tmp}/zig-x86_64-linux-$ZIG_VERSION"
  if [[ ! -x $cache/zig ]]; then
    echo "==> downloading zig $ZIG_VERSION"
    curl -fsSL "https://ziglang.org/download/$ZIG_VERSION/zig-x86_64-linux-$ZIG_VERSION.tar.xz" \
      | tar xJ -C "${TMPDIR:-/tmp}"
  fi
  ZIG="$cache/zig"
fi

echo "==> building (this takes a few minutes the first time)"
ZIG="$ZIG" cargo build --release

cat <<EOF

built: $SRC/target/release/herdr

install it over your current herdr, then hand the running server over:

  cp -f "\$(command -v herdr)" ~/.local/bin/herdr.stock-backup
  cp -f "$SRC/target/release/herdr" "\$(command -v herdr)"
  herdr server live-handoff

live-handoff moves the running panes to the new binary. If anything looks
wrong, copy herdr.stock-backup back and hand off again.
EOF
