#!/usr/bin/env bash
# Package the macOS SDK from the local Command Line Tools (or Xcode) into
# docker/sdk/MacOSX<ver>.sdk.tar.xz so the Docker image can build osxcross.
# Run on a Mac. Override the SDK with:  MACOS_SDK_PATH=/path/to/MacOSX26.5.sdk
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out_dir="$root/docker/sdk"

if [ "$(uname -s)" != "Darwin" ]; then
  echo "package-macos-sdk.sh must run on macOS." >&2
  echo "On Linux/Windows hosts, copy a MacOSX<ver>.sdk.tar.xz into docker/sdk/ yourself" >&2
  echo "(e.g. produced by this script on a Mac, or osxcross's gen_sdk_package.sh)." >&2
  exit 1
fi

sdk="${MACOS_SDK_PATH:-}"
if [ -z "$sdk" ]; then
  for base in /Library/Developer/CommandLineTools/SDKs \
              "$(xcode-select -p 2>/dev/null)/Platforms/MacOSX.platform/Developer/SDKs"; do
    if [ -e "$base/MacOSX.sdk" ]; then sdk="$base/MacOSX.sdk"; break; fi
  done
fi
if [ -z "$sdk" ] || [ ! -e "$sdk" ]; then
  echo "No macOS SDK found. Install the Command Line Tools: xcode-select --install" >&2
  exit 1
fi

# Resolve MacOSX.sdk -> MacOSX27.0.sdk etc.
real="$(cd "$sdk" && pwd -P)"
name="$(basename "$real")"                      # MacOSX27.0.sdk
case "$name" in
  MacOSX[0-9]*.sdk) ;;
  *) ver="$(/usr/bin/plutil -extract Version raw "$real/SDKSettings.plist")"; name="MacOSX${ver}.sdk" ;;
esac

out="$out_dir/$name.tar.xz"
if [ -f "$out" ]; then echo "SDK already packaged: $out"; exit 0; fi

mkdir -p "$out_dir"
rm -f "$out_dir"/MacOSX*.sdk.tar.*
echo "Packaging $real -> $out (takes a minute)..."
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
ln -s "$real" "$tmp/$name"
# -H follows only the command-line symlink; symlinks inside the SDK are preserved.
( cd "$tmp" && COPYFILE_DISABLE=1 tar --no-xattrs -H -cf - "$name" ) | xz -T0 -6 > "$out"
echo "Done: $(du -h "$out" | cut -f1)  $out"
