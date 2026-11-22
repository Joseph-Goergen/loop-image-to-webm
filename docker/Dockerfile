# syntax=docker/dockerfile:1.7
#
# Toolchain image for loopwebm. Builds all six targets from any Docker host:
#   linux-{amd64,arm64}, windows-{amd64,arm64}, macos-{amd64,arm64}
#
# Build context is the docker/ directory (see scripts/build.sh).
# A macOS SDK tarball in docker/sdk/ enables the macOS targets (osxcross).

FROM ubuntu:22.04
ARG TARGETARCH
ARG LLVM_VERSION=22
ARG LLVM_MINGW_VERSION=20261117
ARG OSXCROSS_REV=899260caf42d609293b12b88e8260d78b702cbf6
ARG RCODESIGN_VERSION=0.29.0
ARG LIBVPX_VERSION=v1.15.2

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    MACOSX_DEPLOYMENT_TARGET=11.0 \
    PATH=/opt/cargo/bin:/opt/osxcross/bin:/opt/llvm-mingw/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

# --- Base packages + cross gcc for the "other" Linux architecture -------------
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl wget git xz-utils bzip2 cpio file \
        make cmake ninja-build nasm yasm pkg-config python3 patch \
        gnupg lsb-release software-properties-common \
        libssl-dev libxml2-dev liblzma-dev zlib1g-dev libbz2-dev uuid-dev \
    && if [ "$TARGETARCH" = "arm64" ]; then \
         apt-get install -y --no-install-recommends gcc-x86-64-linux-gnu g++-x86-64-linux-gnu libc6-dev-amd64-cross; \
       else \
         apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu g++-aarch64-linux-gnu libc6-dev-arm64-cross; \
       fi \
    && rm -rf /var/lib/apt/lists/*

# --- LLVM/clang (osxcross, lipo) ---------------------------------------------
RUN wget -qO /tmp/llvm.sh https://apt.llvm.org/llvm.sh \
    && bash /tmp/llvm.sh ${LLVM_VERSION} \
    && apt-get install -y --no-install-recommends llvm-${LLVM_VERSION}-dev libclang-${LLVM_VERSION}-dev \
    && for t in clang clang++ llvm-ar llvm-lipo llvm-ranlib llvm-strip llvm-nm llvm-config ld.lld lld; do \
         ln -sf /usr/bin/$t-${LLVM_VERSION} /usr/local/bin/$t; done \
    && rm -rf /var/lib/apt/lists/* /tmp/llvm.sh

# --- Rust ---------------------------------------------------------------------
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
        --target x86_64-unknown-linux-gnu --target aarch64-unknown-linux-gnu \
        --target x86_64-pc-windows-gnullvm --target aarch64-pc-windows-gnullvm \
        --target x86_64-apple-darwin --target aarch64-apple-darwin \
    && chmod -R a+rwX /opt/rustup /opt/cargo

# --- llvm-mingw (Windows amd64 + arm64) ---------------------------------------
RUN arch=$([ "$TARGETARCH" = "arm64" ] && echo aarch64 || echo x86_64) \
    && curl -sSfL "https://github.com/mstorsjo/llvm-mingw/releases/download/${LLVM_MINGW_VERSION}/llvm-mingw-${LLVM_MINGW_VERSION}-ucrt-ubuntu-22.04-${arch}.tar.xz" \
       | tar -xJ -C /opt \
    && mv /opt/llvm-mingw-* /opt/llvm-mingw

# --- rcodesign (ad-hoc signing of macOS binaries) -----------------------------
RUN arch=$([ "$TARGETARCH" = "arm64" ] && echo aarch64 || echo x86_64) \
    && curl -sSfL "https://github.com/indygreg/apple-platform-rs/releases/download/apple-codesign%2F${RCODESIGN_VERSION}/apple-codesign-${RCODESIGN_VERSION}-${arch}-unknown-linux-musl.tar.gz" \
       | tar -xz -C /tmp \
    && mv /tmp/apple-codesign-*/rcodesign /usr/local/bin/rcodesign && rm -rf /tmp/apple-codesign-*

# --- libvpx source (pinned) ----------------------------------------------------
RUN git clone --depth 1 --branch ${LIBVPX_VERSION} https://chromium.googlesource.com/webm/libvpx /opt/libvpx-src

COPY build-libvpx.sh /usr/local/bin/

# libvpx: Linux + Windows (one layer each so a failure only redoes that target)
RUN build-libvpx.sh linux-amd64
RUN build-libvpx.sh linux-arm64
RUN build-libvpx.sh windows-amd64
RUN build-libvpx.sh windows-arm64

# --- osxcross (only if an SDK tarball was provided) ----------------------------
COPY sdk/ /tmp/sdk/
RUN set -e; sdk=$(ls /tmp/sdk/MacOSX*.sdk.tar.* 2>/dev/null | head -n1 || true); \
    if [ -z "$sdk" ]; then echo "No macOS SDK in docker/sdk/ - skipping osxcross (macOS targets disabled)"; \
    else \
      git clone https://github.com/tpoechtrager/osxcross /tmp/osxcross; \
      cd /tmp/osxcross && git checkout ${OSXCROSS_REV}; \
      mv "$sdk" tarballs/; \
      UNATTENDED=1 TARGET_DIR=/opt/osxcross JOBS=$(nproc) ./build.sh; \
      # Stable, version-free tool names (aarch64-apple-darwin-clang, ...). These must be
      # exec wrappers, not symlinks: osxcross derives the target from argv[0].
      for arch in aarch64 x86_64; do \
        for t in clang clang++ ar ranlib strip nm ld as lipo otool install_name_tool; do \
          src=$(ls /opt/osxcross/bin/${arch}-apple-darwin[0-9]*-$t 2>/dev/null | head -n1 || true); \
          [ -z "$src" ] && [ "$arch" = aarch64 ] && src=$(ls /opt/osxcross/bin/arm64-apple-darwin[0-9]*-$t 2>/dev/null | head -n1 || true); \
          if [ -n "$src" ]; then \
            printf '#!/bin/sh\nexec %s "$@"\n' "$src" > /usr/local/bin/${arch}-apple-darwin-$t; \
            chmod +x /usr/local/bin/${arch}-apple-darwin-$t; \
          fi; \
        done; \
      done; \
      ls /opt/osxcross/SDK > /opt/osxcross/SDK_NAME; \
    fi; \
    rm -rf /tmp/osxcross /tmp/sdk

RUN if [ -d /opt/osxcross ]; then build-libvpx.sh macos-amd64; fi
RUN if [ -d /opt/osxcross ]; then build-libvpx.sh macos-arm64; fi

# --- Per-target compiler / linker configuration for cargo + cc-rs --------------
ENV CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
    AR_x86_64_unknown_linux_gnu=x86_64-linux-gnu-ar \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_x86_64_pc_windows_gnullvm=x86_64-w64-mingw32-clang \
    AR_x86_64_pc_windows_gnullvm=x86_64-w64-mingw32-ar \
    CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER=x86_64-w64-mingw32-clang \
    CC_aarch64_pc_windows_gnullvm=aarch64-w64-mingw32-clang \
    AR_aarch64_pc_windows_gnullvm=aarch64-w64-mingw32-ar \
    CARGO_TARGET_AARCH64_PC_WINDOWS_GNULLVM_LINKER=aarch64-w64-mingw32-clang \
    CC_x86_64_apple_darwin=x86_64-apple-darwin-clang \
    AR_x86_64_apple_darwin=x86_64-apple-darwin-ar \
    CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER=x86_64-apple-darwin-clang \
    CC_aarch64_apple_darwin=aarch64-apple-darwin-clang \
    AR_aarch64_apple_darwin=aarch64-apple-darwin-ar \
    CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=aarch64-apple-darwin-clang \
    LOOPWEBM_VPX_ROOT=/opt/vpx

WORKDIR /src
