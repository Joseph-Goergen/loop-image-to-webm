# LoopWebM

LoopWebM turns a still image into a **seamless, scrolling `.webm` loop**. The image scrolls horizontally or vertically, and its opposite edges are joined (left to right, or top to bottom), so the video wraps around with no visible jump. The last frame flows straight back into the first.

It comes as two self-contained programs that share one Rust core library (`loopwebm-core`):

| Program | What it is |
|---|---|
| `loopwebm` | Command-line tool |
| `loopwebm-gui` / `LoopWebM.app` | Desktop app with drag-and-drop and a live preview |

Neither program depends on the other. Neither needs ffmpeg or any other runtime library, because VP9 encoding comes from **libvpx, statically linked** into each binary.

## How the loop works

- Each frame moves the image by `step` pixels (default **1 px/frame**).
- **FPS controls speed.** Scroll speed = `fps × step` pixels/second.
- **Loop length** = `image width ÷ step` frames (`image height ÷ step` when scrolling vertically). For example, a 1920 px wide image at 30 fps loops every 64 s.
- **Direction**: content moves **Right to Left** (default), Left to Right, Top to Bottom, or Bottom to Top. Horizontal directions join the left and right edges; vertical directions join the top and bottom edges.
- **Seam blur** (off by default): softens the join with a blur along the scroll axis that fades into the image over `blur-width` px on each side. The blur is baked into the image, so it travels with the content. Use it for images whose opposite edges don't match.
- **Transparency**: transparent pixels are composited onto a background colour (default black).
- **Output**: VP9 in WebM, BT.709, at the source resolution.

## GUI

Open `LoopWebM.app` (macOS) or run `loopwebm-gui` (Windows/Linux).

1. **Drag and drop** an image onto the window, or click to choose one. You can also pass a path: `loopwebm-gui photo.png`.
2. Set the options:
   - **Target FPS** and **Pixels per frame**
   - **Direction**: Right to Left, Left to Right, Top to Bottom, or Bottom to Top
   - **Blur the seam**: a switch, with a width setting
   - **Advanced**: quality (CRF), encoder speed, background colour
3. Watch the live preview; it plays at the chosen FPS. Click the preview to pause or play.
4. Click **Export WebM…**. A progress bar shows while it encodes, with a Cancel button.

## CLI

```
loopwebm input.png                          # writes input.webm, 30 fps, right→left
loopwebm input.png -o out.webm --fps 60 --direction ltr
loopwebm tall.png --direction ttb --step 2            # vertical: top to bottom
loopwebm input.jpg --seam-blur --blur-width 24 -y
```

| Option | Default | Meaning |
|---|---|---|
| `-o, --output` | `<input>.webm` | Output file |
| `--fps` | 30 | Frames per second (also sets scroll speed) |
| `-d, --direction` | `rtl` | `rtl` (right to left), `ltr` (left to right), `ttb` (top to bottom), `btt` (bottom to top) |
| `--step` | 1 | Pixels moved per frame (fractional values are fine) |
| `--seam-blur` | off | Blur where the opposite edges meet |
| `--blur-width` | 16 | Blur band width in px, on each side of the seam |
| `--crf` | 31 | VP9 quality: 0 (best) to 63 (smallest) |
| `--cpu-used` | 4 | Encoder speed: 0 (slowest/best) to 8 (fastest) |
| `--bg` | `#000000` | Background colour for transparent pixels |
| `--threads` | all cores | Encoder threads |
| `-y, --overwrite` | | Replace an existing output file |
| `-q, --quiet` | | Only print errors |

## Building

Every build runs in **Docker**. The toolchain image downloads Rust, LLVM, llvm-mingw and osxcross, and builds libvpx from source for each target. The only thing you need on the host is Docker.

| Target | Rust triple |
|---|---|
| `linux-amd64` | `x86_64-unknown-linux-gnu` (glibc ≥ 2.35) |
| `linux-arm64` | `aarch64-unknown-linux-gnu` (glibc ≥ 2.35) |
| `windows-amd64` | `x86_64-pc-windows-gnullvm` |
| `windows-arm64` | `aarch64-pc-windows-gnullvm` |
| `macos-amd64` | `x86_64-apple-darwin` (macOS ≥ 11) |
| `macos-arm64` | `aarch64-apple-darwin` (macOS ≥ 11) |

### macOS / Linux / Git Bash / WSL

```bash
./scripts/build.sh                 # default: just this machine's OS + architecture
./scripts/build.sh --all           # all six targets + a universal macOS app
./scripts/build.sh --target linux-arm64 --target windows-amd64
./scripts/build.sh --test          # run the test suite
./scripts/build.sh --rebuild-image # force the toolchain image to rebuild
```

### Windows PowerShell

```powershell
.\scripts\build.ps1                # this machine (windows-amd64 or windows-arm64)
.\scripts\build.ps1 -All
.\scripts\build.ps1 -Target linux-amd64,macos-arm64
.\scripts\build.ps1 -Test
```

### Output

```
dist/<target>/loopwebm[.exe]
dist/<target>/loopwebm-gui[.exe]
dist/macos-*/LoopWebM.app             # ad-hoc signed
dist/macos-universal/                 # arm64 + x86_64 universal binaries and app (only with both macOS targets)
```

### Build notes

- **First build takes 30–40 minutes.** That's the toolchain image: osxcross plus six builds of libvpx. The image is cached and tagged by a hash of `docker/`, so it only rebuilds when those files change. Cargo's registry and `target/` directory live in Docker volumes (`loopwebm-cargo`, `loopwebm-target`), so later builds are incremental.
- **macOS targets need Apple's SDK.**
  - On a Mac, `build.sh` runs `scripts/package-macos-sdk.sh` the first time. It packages the SDK from the Command Line Tools (`xcode-select --install`) into `docker/sdk/`.
  - On Linux or Windows, copy a `MacOSX<ver>.sdk.tar.xz` made on a Mac into `docker/sdk/`. Without one, the macOS targets are skipped and everything else still builds.
  - The SDK is git-ignored. Don't commit or redistribute it.
- **macOS signing.** Binaries are ad-hoc signed with `rcodesign`; arm64 Macs require that. If you copy the app to another Mac via a download, Gatekeeper may need a right-click → Open the first time.
- **Docker architecture.** The image runs natively on amd64 and arm64 Docker hosts, and cross-compiles to all six targets from either.

## Project layout

```
crates/loopwebm-core   frames.rs (wrap scroll + seam blur), color.rs (RGB→I420),
                       encode.rs + vpx_shim.c (libvpx VP9), webm.rs (pure-Rust WebM muxer)
crates/loopwebm-cli    clap CLI
crates/loopwebm-gui    eframe/egui desktop app
docker/                toolchain Dockerfile, libvpx build script, sdk/ drop folder
scripts/               build.sh, build.ps1, in-container-build.sh, package-macos-sdk.sh
packaging/macos        Info.plist for the .app bundle
```
