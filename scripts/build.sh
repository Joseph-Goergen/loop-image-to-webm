#!/usr/bin/env bash
# Build loopwebm (CLI + GUI) inside Docker. Works on macOS, Linux and Windows
# (Git Bash / WSL) hosts, amd64 or arm64.
#
#   ./scripts/build.sh                      # host OS + arch only (default)
#   ./scripts/build.sh --all                # all 6 targets (+ macOS universal)
#   ./scripts/build.sh --target linux-arm64 --target windows-amd64
#   ./scripts/build.sh --test               # run the test suite
#   ./scripts/build.sh --rebuild-image      # force a toolchain image rebuild
#
# Targets: linux-amd64 linux-arm64 windows-amd64 windows-arm64 macos-amd64 macos-arm64
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
ALL=(linux-amd64 linux-arm64 windows-amd64 windows-arm64 macos-amd64 macos-arm64)

usage() { sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }

targets=(); mode=build; rebuild=0
while [ $# -gt 0 ]; do
  case "$1" in
    --all) targets=("${ALL[@]}") ;;
    --target|-t) shift; targets+=("${1:?--target needs a value}") ;;
    --target=*) targets+=("${1#--target=}") ;;
    --test) mode=test ;;
    --rebuild-image) rebuild=1 ;;
    -h|--help) usage 0 ;;
    *) echo "unknown argument: $1" >&2; usage 1 ;;
  esac
  shift
done

host_target() {
  local os arch
  case "$(uname -s)" in
    Darwin) os=macos ;;
    Linux) os=linux ;;
    MINGW*|MSYS*|CYGWIN*) os=windows ;;
    *) echo "unsupported host OS $(uname -s)" >&2; exit 1 ;;
  esac
  case "$(uname -m)" in
    arm64|aarch64) arch=arm64 ;;
    x86_64|amd64) arch=amd64 ;;
    *) echo "unsupported host arch $(uname -m)" >&2; exit 1 ;;
  esac
  # WSL reports Linux; that's fine — Linux binaries are what runs there.
  echo "$os-$arch"
}

if [ "$mode" = build ] && [ ${#targets[@]} -eq 0 ]; then
  targets=("$(host_target)")
  echo "No target given; building for this host: ${targets[0]}  (use --all for every platform)"
fi
for t in ${targets[@]+"${targets[@]}"}; do
  [[ " ${ALL[*]} " == *" $t "* ]] || { echo "unknown target '$t' (valid: ${ALL[*]})" >&2; exit 1; }
done

command -v docker >/dev/null || { echo "Docker is required: https://docs.docker.com/get-docker/" >&2; exit 1; }

# macOS targets need an SDK tarball in docker/sdk/. On a Mac we can make one.
wants_mac=0
for t in ${targets[@]+"${targets[@]}"}; do [[ "$t" == macos-* ]] && wants_mac=1; done
if [ $wants_mac = 1 ] && ! ls "$root"/docker/sdk/MacOSX*.sdk.tar.* >/dev/null 2>&1; then
  if [ "$(uname -s)" = Darwin ]; then
    "$root/scripts/package-macos-sdk.sh"
  else
    echo "!! macOS targets need a macOS SDK tarball in docker/sdk/ (see README)." >&2
    echo "!! Continuing; macOS targets will be skipped." >&2
  fi
fi

# Toolchain image, tagged by a hash of everything that goes into it.
hash=$( { cat "$root/docker/Dockerfile" "$root/docker/build-libvpx.sh"; ls "$root/docker/sdk"; } | cksum | cut -d' ' -f1 )
image="loopwebm-toolchain:$hash"
if [ $rebuild = 1 ] || ! docker image inspect "$image" >/dev/null 2>&1; then
  echo "==> Building toolchain image $image (first time takes 30-40 minutes)"
  docker build -t "$image" -t loopwebm-toolchain:latest -f "$root/docker/Dockerfile" "$root/docker"
fi

tty=(); [ -t 1 ] && tty=(-t)
if [ "$mode" = test ]; then args=(--test); else args=("${targets[@]}"); fi
# MSYS (Git Bash) would mangle /src paths without this.
export MSYS_NO_PATHCONV=1
docker run --rm ${tty[@]+"${tty[@]}"} \
  -v "$root:/src" \
  -v loopwebm-cargo:/cache/cargo \
  -v loopwebm-target:/cache/target \
  -e CARGO_HOME=/cache/cargo \
  -e CARGO_TARGET_DIR=/cache/target \
  -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
  "$image" bash /src/scripts/in-container-build.sh "${args[@]}"
