#!/usr/bin/env bash
# Build the macOS targets natively on a Mac, without Docker.
#
# Used by the GitHub release workflow: GitHub's macOS runners can't run Docker,
# and Apple's SDK must stay on Apple hardware. Local builds should keep using
# scripts/build.sh (Docker). Produces the same layout as the Docker build:
#   dist/macos-arm64/  dist/macos-amd64/  dist/macos-universal/  (+ LoopWebM.app)
#
#   ./scripts/build-macos-native.sh          # build
#   ./scripts/build-macos-native.sh --test   # also run the test suite (native arch)
#
# Needs: Xcode Command Line Tools, rustup, git, nasm (x86_64 libvpx assembly).
# libvpx and Rust versions are read from docker/Dockerfile, so both build paths
# always use the same pinned versions.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
run_tests=0
[ "${1:-}" = "--test" ] && run_tests=1

[ "$(uname -s)" = Darwin ] || { echo "build-macos-native.sh must run on macOS" >&2; exit 1; }
for tool in rustup git nasm clang lipo codesign; do
  command -v "$tool" >/dev/null || { echo "missing required tool: $tool" >&2; exit 1; }
done

dockerfile_arg() { sed -n "s/^ARG $1=//p" docker/Dockerfile | head -n1; }
LIBVPX_VERSION=$(dockerfile_arg LIBVPX_VERSION)
LIBVPX_COMMIT=$(dockerfile_arg LIBVPX_COMMIT)
RUST_VERSION=$(dockerfile_arg RUST_VERSION)
: "${LIBVPX_VERSION:?}" "${LIBVPX_COMMIT:?}" "${RUST_VERSION:?}"

export MACOSX_DEPLOYMENT_TARGET=11.0
work="${LOOPWEBM_WORK_DIR:-$root/target/macos-native}"
jobs=$(sysctl -n hw.ncpu)
mkdir -p "$work"

echo "==> Rust $RUST_VERSION"
rustup toolchain install "$RUST_VERSION" --profile minimal \
  --target aarch64-apple-darwin --target x86_64-apple-darwin
cargo=(rustup run "$RUST_VERSION" cargo)

echo "==> libvpx $LIBVPX_VERSION"
src="$work/libvpx-src"
if [ ! -d "$src/.git" ]; then
  git clone --quiet --depth 1 --branch "$LIBVPX_VERSION" https://chromium.googlesource.com/webm/libvpx "$src"
fi
got=$(git -C "$src" rev-parse HEAD)
[ "$got" = "$LIBVPX_COMMIT" ] || { echo "libvpx checkout is $got, expected $LIBVPX_COMMIT" >&2; exit 1; }

build_vpx() { # build_vpx <name> <libvpx --target>
  local name="$1" target="$2" prefix="$work/vpx/$1" build="$work/vpx-build-$1"
  if [ -f "$prefix/lib/libvpx.a" ] && [ "$(cat "$prefix/.commit" 2>/dev/null)" = "$LIBVPX_COMMIT" ]; then
    echo "    $name: cached"; return 0
  fi
  echo "    $name: building ($target)"
  rm -rf "$build" "$prefix" && mkdir -p "$build"
  (
    cd "$build"
    CC=clang CXX=clang++ "$src/configure" \
      --target="$target" --prefix="$prefix" \
      --enable-static --disable-shared --enable-pic \
      --disable-examples --disable-tools --disable-docs --disable-unit-tests \
      --disable-vp8 --enable-vp9 --disable-vp9-decoder \
      --disable-webm-io --disable-libyuv >/dev/null \
      || { tail -40 config.log; exit 1; }
    make -j"$jobs" >/dev/null
    make install >/dev/null
  )
  echo "$LIBVPX_COMMIT" > "$prefix/.commit"
  rm -rf "$build"
}
build_vpx macos-arm64 arm64-darwin20-gcc
build_vpx macos-amd64 x86_64-darwin20-gcc
export LOOPWEBM_VPX_ROOT="$work/vpx"

if [ $run_tests = 1 ]; then
  native=$([ "$(uname -m)" = arm64 ] && echo aarch64-apple-darwin || echo x86_64-apple-darwin)
  echo "==> cargo test ($native)"
  "${cargo[@]}" test --workspace --release --target "$native"
fi

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
sign() { codesign --force --sign - "$1"; }
make_app() { # make_app <dir containing loopwebm-gui>
  local dir="$1" app="$1/LoopWebM.app"
  rm -rf "$app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  cp "$dir/loopwebm-gui" "$app/Contents/MacOS/loopwebm-gui"
  sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"
  printf 'APPL????' > "$app/Contents/PkgInfo"
  sign "$app"
}

target_dir="${CARGO_TARGET_DIR:-$root/target}"
for pair in macos-arm64:aarch64-apple-darwin macos-amd64:x86_64-apple-darwin; do
  t=${pair%%:*} triple=${pair#*:}
  echo "==> Building $t ($triple)"
  "${cargo[@]}" build --release --target "$triple" -p loopwebm-cli -p loopwebm-gui
  out="dist/$t"; rm -rf "$out"; mkdir -p "$out"
  for b in loopwebm loopwebm-gui; do
    cp "$target_dir/$triple/release/$b" "$out/"
    sign "$out/$b"
  done
  make_app "$out"
done

echo "==> Creating macos-universal"
out=dist/macos-universal; rm -rf "$out"; mkdir -p "$out"
for b in loopwebm loopwebm-gui; do
  lipo -create dist/macos-arm64/$b dist/macos-amd64/$b -output "$out/$b"
  sign "$out/$b"
done
make_app "$out"

echo
echo "==> Done"
for t in macos-arm64 macos-amd64 macos-universal; do
  for f in dist/$t/loopwebm dist/$t/loopwebm-gui; do echo "   $(file -b "$f" | cut -c1-70)  <- $f"; done
done
