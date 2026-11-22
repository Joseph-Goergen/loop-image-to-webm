#!/usr/bin/env bash
# Build a static, VP9-encoder-only libvpx for one target into /opt/vpx/<target>.
# Usage: build-libvpx.sh <linux-amd64|linux-arm64|windows-amd64|windows-arm64|macos-amd64|macos-arm64>
set -euo pipefail

name="$1"
src=/opt/libvpx-src
prefix="/opt/vpx/$name"
work="/tmp/vpx-build-$name"

extra=()
case "$name" in
  linux-amd64)
    target=x86_64-linux-gcc; cross=x86_64-linux-gnu-
    export CC=${cross}gcc CXX=${cross}g++ AR=${cross}ar LD=${cross}gcc STRIP=${cross}strip NM=${cross}nm AS=nasm ;;
  linux-arm64)
    target=arm64-linux-gcc; cross=aarch64-linux-gnu-
    export CC=${cross}gcc CXX=${cross}g++ AR=${cross}ar LD=${cross}gcc STRIP=${cross}strip NM=${cross}nm AS=${cross}as ;;
  windows-amd64)
    target=x86_64-win64-gcc; cross=x86_64-w64-mingw32-
    export CC=${cross}clang CXX=${cross}clang++ AR=${cross}ar LD=${cross}clang STRIP=${cross}strip NM=${cross}nm AS=nasm ;;
  windows-arm64)
    target=arm64-win64-gcc; cross=aarch64-w64-mingw32-
    export CC=${cross}clang CXX=${cross}clang++ AR=${cross}ar LD=${cross}clang STRIP=${cross}strip NM=${cross}nm AS=${cross}clang
    extra+=(--disable-runtime-cpu-detect) ;;
  macos-amd64)
    target=x86_64-darwin20-gcc; cross=x86_64-apple-darwin-
    export CC=${cross}clang CXX=${cross}clang++ AR=${cross}ar LD=${cross}clang STRIP=${cross}strip NM=${cross}nm AS=nasm ;;
  macos-arm64)
    target=arm64-darwin20-gcc; cross=aarch64-apple-darwin-
    export CC=${cross}clang CXX=${cross}clang++ AR=${cross}ar LD=${cross}clang STRIP=${cross}strip NM=${cross}nm AS=${cross}clang ;;
  *) echo "unknown target: $name" >&2; exit 2 ;;
esac
export CROSS=$cross

rm -rf "$work" && mkdir -p "$work" && cd "$work"
"$src/configure" \
  --target="$target" \
  --prefix="$prefix" \
  --enable-static --disable-shared --enable-pic \
  --disable-examples --disable-tools --disable-docs --disable-unit-tests \
  --disable-vp8 --enable-vp9 --disable-vp9-decoder \
  --disable-webm-io --disable-libyuv \
  "${extra[@]}" || { cat config.log | tail -40; exit 1; }
make -j"$(nproc)"
make install
rm -rf "$work"
echo "libvpx for $name installed to $prefix"
ls -la "$prefix/lib"
