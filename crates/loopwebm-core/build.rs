//! Compiles the tiny C shim around libvpx and links libvpx statically.
//!
//! libvpx is located via (first match wins):
//!   1. `LOOPWEBM_VPX_DIR`            – a prefix containing include/ and lib/
//!   2. `LOOPWEBM_VPX_ROOT/<target>`  – e.g. /opt/vpx/linux-arm64 (set in the Docker image)
use std::env;
use std::path::PathBuf;

fn target_name(triple: &str) -> Option<&'static str> {
    Some(match triple {
        "x86_64-unknown-linux-gnu" => "linux-amd64",
        "aarch64-unknown-linux-gnu" => "linux-arm64",
        "x86_64-pc-windows-gnullvm" | "x86_64-pc-windows-gnu" => "windows-amd64",
        "aarch64-pc-windows-gnullvm" => "windows-arm64",
        "x86_64-apple-darwin" => "macos-amd64",
        "aarch64-apple-darwin" => "macos-arm64",
        _ => return None,
    })
}

fn main() {
    println!("cargo:rerun-if-changed=src/vpx_shim.c");
    println!("cargo:rerun-if-env-changed=LOOPWEBM_VPX_DIR");
    println!("cargo:rerun-if-env-changed=LOOPWEBM_VPX_ROOT");

    let triple = env::var("TARGET").unwrap();
    let dir = if let Ok(d) = env::var("LOOPWEBM_VPX_DIR") {
        PathBuf::from(d)
    } else {
        let root = env::var("LOOPWEBM_VPX_ROOT").unwrap_or_else(|_| "/opt/vpx".into());
        let name = target_name(&triple)
            .unwrap_or_else(|| panic!("unsupported target {triple}; set LOOPWEBM_VPX_DIR"));
        PathBuf::from(root).join(name)
    };
    let include = dir.join("include");
    let lib = dir.join("lib");
    if !include.join("vpx").join("vpx_encoder.h").exists() {
        panic!(
            "libvpx not found at {} (target {triple}). Build inside the Docker toolchain \
             (scripts/build.sh) or set LOOPWEBM_VPX_DIR to a static libvpx install prefix.",
            dir.display()
        );
    }

    cc::Build::new()
        .file("src/vpx_shim.c")
        .include(&include)
        .warnings(true)
        .compile("loopwebm_vpx_shim");

    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static=vpx");
    if triple.contains("linux") {
        println!("cargo:rustc-link-lib=pthread");
        println!("cargo:rustc-link-lib=m");
    } else if triple.contains("windows") {
        // MinGW libvpx uses winpthreads. Link the static archive at the final link
        // (-bundle) so the .exe doesn't need libwinpthread-1.dll at runtime.
        println!("cargo:rustc-link-lib=static:-bundle=pthread");
    }
}
