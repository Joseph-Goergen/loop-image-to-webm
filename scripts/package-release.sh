#!/usr/bin/env bash
# Package built targets from dist/<target>/ into release archives in dist/release/.
#
#   ./scripts/package-release.sh <version> <target>...
#   ./scripts/package-release.sh v0.1.0 linux-amd64 windows-amd64 macos-universal
#
# Linux   -> loopwebm-<version>-<target>.tar.gz  (CLI + GUI; keeps executable bits)
# Windows -> loopwebm-<version>-<target>.zip     (CLI + GUI)
# macOS   -> loopwebm-<version>-<target>.dmg     (LoopWebM.app + Applications shortcut)
#            loopwebm-<version>-<target>.tar.gz  (the loopwebm CLI on its own)
# Every archive carries the README/licence files and third-party notices.
# The .dmg needs hdiutil, so macOS targets must be packaged on a Mac.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
[ $# -ge 2 ] || { sed -n '2,4p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }
version="$1"; shift
[[ "$version" =~ ^[A-Za-z0-9._+-]+$ ]] || { echo "invalid version '$version'" >&2; exit 1; }

out="$root/dist/release"
# Keep host metadata out of the archives: no macOS ._ files or xattrs, no
# build-machine user names.
export COPYFILE_DISABLE=1
if tar --version 2>/dev/null | grep -q bsdtar; then
  tar_meta=(--no-xattrs --no-mac-metadata --uid 0 --gid 0 --uname "" --gname "")
else
  tar_meta=(--owner=0 --group=0 --numeric-owner)
fi
mkdir -p "$out"
stage_root="$(mktemp -d)"
trap 'rm -rf "$stage_root"' EXIT

# make_dmg <target> <name>: LoopWebM.app + an /Applications shortcut to drag it
# onto, plus a Licenses folder. Compressed (UDZO), opens on macOS 11+.
make_dmg() {
  local t="$1" name="$2" vol="$stage_root/dmg-$1" i
  command -v hdiutil >/dev/null || { echo "building a .dmg needs macOS (hdiutil)" >&2; exit 1; }
  [ -d "dist/$t/LoopWebM.app" ] || { echo "dist/$t/LoopWebM.app not found" >&2; exit 1; }
  mkdir -p "$vol/Licenses"
  # ditto keeps the bundle (and its signature) intact; drop xattrs like quarantine.
  ditto --norsrc --noextattr --noqtn --noacl "dist/$t/LoopWebM.app" "$vol/LoopWebM.app"
  ln -s /Applications "$vol/Applications"
  [ -f LICENSE ] && cp LICENSE "$vol/Licenses/LoopWebM-LICENSE"
  cp packaging/licenses/* "$vol/Licenses/"
  codesign --verify --deep --strict "$vol/LoopWebM.app"
  rm -f "$out/$name.dmg"
  # hdiutil sometimes fails with "Resource busy" on CI runners; retry a few times.
  for i in 1 2 3 4 5; do
    if hdiutil create -quiet -volname "LoopWebM $version" -srcfolder "$vol" \
         -fs HFS+ -format UDZO -imagekey zlib-level=9 -ov "$out/$name.dmg"; then
      hdiutil verify -quiet "$out/$name.dmg"
      return 0
    fi
    echo "hdiutil create failed (attempt $i), retrying..." >&2
    sleep $((i * 5))
  done
  echo "could not create $name.dmg" >&2
  exit 1
}

for t in "$@"; do
  case "$t" in
    linux-amd64|linux-arm64|windows-amd64|windows-arm64|macos-amd64|macos-arm64|macos-universal) ;;
    *) echo "unknown target '$t'" >&2; exit 1 ;;
  esac
  [ -d "dist/$t" ] || { echo "dist/$t not found - build it first" >&2; exit 1; }

  name="loopwebm-$version-$t"
  stage="$stage_root/$name"
  mkdir -p "$stage/licenses"
  cp README.md "$stage/"
  [ -f LICENSE ] && cp LICENSE "$stage/"
  cp packaging/licenses/* "$stage/licenses/"

  case "$t" in
    linux-*)
      cp dist/"$t"/loopwebm dist/"$t"/loopwebm-gui "$stage/"
      tar "${tar_meta[@]}" -C "$stage_root" -czf "$out/$name.tar.gz" "$name"
      echo "   $out/$name.tar.gz" ;;
    windows-*)
      cp dist/"$t"/loopwebm.exe dist/"$t"/loopwebm-gui.exe "$stage/"
      rm -f "$out/$name.zip"
      (cd "$stage_root" && zip -qrX "$out/$name.zip" "$name")
      echo "   $out/$name.zip" ;;
    macos-*)
      # CLI tarball
      cp dist/"$t"/loopwebm "$stage/"
      tar "${tar_meta[@]}" -C "$stage_root" -czf "$out/$name.tar.gz" "$name"
      echo "   $out/$name.tar.gz"
      # App disk image
      make_dmg "$t" "$name"
      echo "   $out/$name.dmg" ;;
  esac
done
