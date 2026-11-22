# syntax=docker/dockerfile:1.7
#
# Toolchain image for loopwebm. Builds all six targets from any Docker host:
#   linux-{amd64,arm64}, windows-{amd64,arm64}, macos-{amd64,arm64}
#
# Build context is the docker/ directory (see scripts/build.sh).
# A macOS SDK tarball in docker/sdk/ enables the macOS targets (osxcross).
#
# Supply chain: every download is pinned to an exact version and verified -
# tarballs/binaries by SHA-256, git checkouts by commit hash, the LLVM apt repo
# by its signing-key fingerprint, and the base image by digest. To upgrade a
# component, change its version AND its hash together.

# ubuntu:22.04 (multi-arch index digest)
FROM ubuntu:22.04@sha256:f64b2ffc2f10ab8f8505aa56b2664bd846cbb9df87c7e2532e30f7667021c0e6
ARG TARGETARCH

ARG LLVM_VERSION=22
ARG LLVM_KEY_FPR=6084F3CF814B57C1CF12EFD515CF4D18AF4F7421

ARG RUST_VERSION=1.100.0
ARG RUSTUP_VERSION=1.29.3
ARG RUSTUP_SHA256_AMD64=b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55
ARG RUSTUP_SHA256_ARM64=9f608c142e0e38a78957d0661ba1dfaf0b6fb37184cfaa4b9a0aa57af6184282

ARG LLVM_MINGW_VERSION=20261117
ARG LLVM_MINGW_SHA256_AMD64=e7a15a49a2ea95e9f3d8a9d0cfabe5052a4b10fb4777cc54de5a8c27b64bb701
ARG LLVM_MINGW_SHA256_ARM64=6d9442ead9ace3b3e4c3389bfef69f93f873305e0705e6cc60978ac895320872

ARG RCODESIGN_VERSION=0.29.0
ARG RCODESIGN_SHA256_AMD64=dbe85cedd8ee4217b64e9a0e4c2aef92ab8bcaaa41f20bde99781ff02e600002
ARG RCODESIGN_SHA256_ARM64=4af92c87ddf52f5f2d1258a3b4e56c7dcb8f1b2468df744976c5f139e031961f

ARG LIBVPX_VERSION=v1.17.0
ARG LIBVPX_COMMIT=6df3ec34557879fff673706f4a1d9fbd0f3a6f0e

ARG OSXCROSS_REV=899260caf42d609293b12b88e8260d78b702cbf6

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    MACOSX_DEPLOYMENT_TARGET=11.0 \
    PATH=/opt/cargo/bin:/opt/osxcross/bin:/opt/llvm-mingw/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

# --- Base packages + cross gcc for the "other" Linux architecture -------------
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl wget git xz-utils bzip2 cpio file \
        make cmake ninja-build nasm yasm pkg-config python3 patch \
        gnupg \
        libssl-dev libxml2-dev liblzma-dev zlib1g-dev libbz2-dev uuid-dev \
    && if [ "$TARGETARCH" = "arm64" ]; then \
         apt-get install -y --no-install-recommends gcc-x86-64-linux-gnu g++-x86-64-linux-gnu libc6-dev-amd64-cross; \
       else \
         apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu g++-aarch64-linux-gnu libc6-dev-arm64-cross; \
       fi \
    && rm -rf /var/lib/apt/lists/*

# --- LLVM/clang (osxcross, lipo) ---------------------------------------------
# apt.llvm.org repo, trusted only after its signing key matches the pinned fingerprint.
RUN curl -sSfL -o /tmp/llvm.asc https://apt.llvm.org/llvm-snapshot.gpg.key \
    && fpr=$(gpg --show-keys --with-colons /tmp/llvm.asc | awk -F: '$1=="fpr"{print $10; exit}') \
    && if [ "$fpr" != "$LLVM_KEY_FPR" ]; then echo "LLVM apt key fingerprint mismatch: $fpr" >&2; exit 1; fi \
    && gpg --dearmor -o /usr/share/keyrings/llvm.gpg /tmp/llvm.asc && rm /tmp/llvm.asc \
    && echo "deb [signed-by=/usr/share/keyrings/llvm.gpg] https://apt.llvm.org/jammy/ llvm-toolchain-jammy-${LLVM_VERSION} main" \
         > /etc/apt/sources.list.d/llvm.list \
    && apt-get update && apt-get install -y --no-install-recommends \
         clang-${LLVM_VERSION} lld-${LLVM_VERSION} llvm-${LLVM_VERSION} \
         llvm-${LLVM_VERSION}-dev libclang-${LLVM_VERSION}-dev \
    && for t in clang clang++ llvm-ar llvm-lipo llvm-ranlib llvm-strip llvm-nm llvm-config ld.lld lld; do \
         ln -sf /usr/bin/$t-${LLVM_VERSION} /usr/local/bin/$t; done \
    && rm -rf /var/lib/apt/lists/*

# --- Rust ---------------------------------------------------------------------
RUN arch=$([ "$TARGETARCH" = "arm64" ] && echo aarch64 || echo x86_64) \
    && sha=$([ "$TARGETARCH" = "arm64" ] && echo "$RUSTUP_SHA256_ARM64" || echo "$RUSTUP_SHA256_AMD64") \
    && curl -sSfL -o /tmp/rustup-init "https://static.rust-lang.org/rustup/archive/${RUSTUP_VERSION}/${arch}-unknown-linux-gnu/rustup-init" \
    && echo "$sha  /tmp/rustup-init" | sha256sum -c - \
    && chmod +x /tmp/rustup-init \
    && /tmp/rustup-init -y --no-modify-path --profile minimal --default-toolchain ${RUST_VERSION} \
        --target x86_64-unknown-linux-gnu --target aarch64-unknown-linux-gnu \
        --target x86_64-pc-windows-gnullvm --target aarch64-pc-windows-gnullvm \
        --target x86_64-apple-darwin --target aarch64-apple-darwin \
    && rm /tmp/rustup-init \
    && chmod -R a+rwX /opt/rustup /opt/cargo

# --- llvm-mingw (Windows amd64 + arm64) ---------------------------------------
RUN arch=$([ "$TARGETARCH" = "arm64" ] && echo aarch64 || echo x86_64) \
    && sha=$([ "$TARGETARCH" = "arm64" ] && echo "$LLVM_MINGW_SHA256_ARM64" || echo "$LLVM_MINGW_SHA256_AMD64") \
    && curl -sSfL -o /tmp/llvm-mingw.tar.xz "https://github.com/mstorsjo/llvm-mingw/releases/download/${LLVM_MINGW_VERSION}/llvm-mingw-${LLVM_MINGW_VERSION}-ucrt-ubuntu-22.04-${arch}.tar.xz" \
    && echo "$sha  /tmp/llvm-mingw.tar.xz" | sha256sum -c - \
    && tar -xJf /tmp/llvm-mingw.tar.xz -C /opt && rm /tmp/llvm-mingw.tar.xz \
    && mv /opt/llvm-mingw-* /opt/llvm-mingw

# --- rcodesign (ad-hoc signing of macOS binaries) -----------------------------
RUN arch=$([ "$TARGETARCH" = "arm64" ] && echo aarch64 || echo x86_64) \
    && sha=$([ "$TARGETARCH" = "arm64" ] && echo "$RCODESIGN_SHA256_ARM64" || echo "$RCODESIGN_SHA256_AMD64") \
    && curl -sSfL -o /tmp/rcodesign.tar.gz "https://github.com/indygreg/apple-platform-rs/releases/download/apple-codesign%2F${RCODESIGN_VERSION}/apple-codesign-${RCODESIGN_VERSION}-${arch}-unknown-linux-musl.tar.gz" \
    && echo "$sha  /tmp/rcodesign.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/rcodesign.tar.gz -C /tmp && rm /tmp/rcodesign.tar.gz \
    && mv /tmp/apple-codesign-*/rcodesign /usr/local/bin/rcodesign && rm -rf /tmp/apple-codesign-*

# --- libvpx source (pinned) ----------------------------------------------------
RUN git clone --depth 1 --branch ${LIBVPX_VERSION} https://chromium.googlesource.com/webm/libvpx /opt/libvpx-src \
    && got=$(git -C /opt/libvpx-src rev-parse HEAD) \
    && if [ "$got" != "$LIBVPX_COMMIT" ]; then echo "libvpx ${LIBVPX_VERSION} is $got, expected $LIBVPX_COMMIT" >&2; exit 1; fi

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
      cd /tmp/osxcross && git checkout --detach ${OSXCROSS_REV}; \
      [ "$(git rev-parse HEAD)" = "${OSXCROSS_REV}" ] || { echo "osxcross checkout mismatch" >&2; exit 1; }; \
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
