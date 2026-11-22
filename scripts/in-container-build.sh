#!/usr/bin/env bash
# Runs INSIDE the toolchain container (invoked by scripts/build.sh / build.ps1).
# Usage: in-container-build.sh --test
#        in-container-build.sh <target> [<target>...]
set -euo pipefail
cd /src

triple_for() {
  case "$1" in
    linux-amd64)   echo x86_64-unknown-linux-gnu ;;
    linux-arm64)   echo aarch64-unknown-linux-gnu ;;
    windows-amd64) echo x86_64-pc-windows-gnullvm ;;
    windows-arm64) echo aarch64-pc-windows-gnullvm ;;
    macos-amd64)   echo x86_64-apple-darwin ;;
    macos-arm64)   echo aarch64-apple-darwin ;;
    *) echo "unknown target '$1'" >&2; return 1 ;;
  esac
}

fix_owner() {
  if [ -n "${HOST_UID:-}" ] && [ -n "${HOST_GID:-}" ] && [ -d dist ]; then
    chown -R "$HOST_UID:$HOST_GID" dist || true
  fi
  if [ -n "${HOST_UID:-}" ] && [ -n "${HOST_GID:-}" ] && [ -f Cargo.lock ]; then
    chown "$HOST_UID:$HOST_GID" Cargo.lock || true
  fi
}
trap fix_owner EXIT

if [ "${1:-}" = "--test" ]; then
  arch=$(uname -m); [ "$arch" = aarch64 ] && t=linux-arm64 || t=linux-amd64
  echo "==> cargo test ($t)"
  cargo test --workspace --release --target "$(triple_for $t)"
  exit 0
fi

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)

make_app() { # make_app <dir containing loopwebm-gui>
  local dir="$1" app="$1/LoopWebM.app"
  rm -rf "$app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  cp "$dir/loopwebm-gui" "$app/Contents/MacOS/loopwebm-gui"
  sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"
  printf 'APPL????' > "$app/Contents/PkgInfo"
  quiet_sign "$app"
}

# rcodesign logs to stderr; only show it if signing fails.
quiet_sign() { local log; log=$(rcodesign sign "$1" 2>&1) || { echo "$log" >&2; return 1; }; }
sign_macho() { quiet_sign "$1"; }

built=()
skipped=()
for t in "$@"; do
  triple=$(triple_for "$t")
  if [[ "$t" == macos-* ]] && [ ! -d /opt/osxcross ]; then
    echo "!! Skipping $t: the toolchain image has no macOS SDK (see README: docker/sdk)." >&2
    skipped+=("$t"); continue
  fi
  echo "==> Building $t ($triple)"
  cargo build --release --target "$triple" -p loopwebm-cli -p loopwebm-gui
  out="dist/$t"; rm -rf "$out"; mkdir -p "$out"
  ext=""; [[ "$t" == windows-* ]] && ext=".exe"
  for b in loopwebm loopwebm-gui; do
    cp "$CARGO_TARGET_DIR/$triple/release/$b$ext" "$out/"
  done
  if [[ "$t" == macos-* ]]; then
    sign_macho "$out/loopwebm"; sign_macho "$out/loopwebm-gui"
    make_app "$out"
  fi
  built+=("$t")
done

# Universal macOS binaries, only when both architectures were built in this run
# (mixing a fresh slice with a stale one would produce an inconsistent app).
if [[ " ${built[*]-} " == *" macos-arm64 "* && " ${built[*]-} " == *" macos-amd64 "* ]]; then
  echo "==> Creating macos-universal"
  out=dist/macos-universal; rm -rf "$out"; mkdir -p "$out"
  for b in loopwebm loopwebm-gui; do
    llvm-lipo -create dist/macos-arm64/$b dist/macos-amd64/$b -output "$out/$b"
    sign_macho "$out/$b"
  done
  make_app "$out"
fi

echo
echo "==> Done"
for t in "${built[@]}"; do
  for f in dist/$t/*; do [ -f "$f" ] && echo "   $(file -b "$f" | cut -c1-70)  <- $f"; done
done
[ -d dist/macos-universal ] && echo "   (universal) dist/macos-universal/LoopWebM.app"
if [ ${#skipped[@]} -gt 0 ]; then echo "   skipped: ${skipped[*]}"; fi
