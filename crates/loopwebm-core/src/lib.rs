//! loopwebm-core: turn a still image into a seamless, horizontally scrolling
//! VP9 WebM loop. The image scrolls horizontally or vertically and its opposite
//! edges are stitched together, so the last frame flows straight back into the first.
//!
//! Shared by the `loopwebm` CLI and the `loopwebm-gui` desktop app.

pub mod color;
pub mod encode;
pub mod frames;
pub mod options;
pub mod webm;

pub use frames::{FramePlan, Source};
pub use options::{parse_hex_color, Axis, Direction, LoopOptions};

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read image: {0}")]
    Image(#[from] image::ImageError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    InvalidOption(String),
    #[error("VP9 encoder error: {0}")]
    Encoder(String),
    #[error("cancelled")]
    Cancelled,
}

/// Image file extensions accepted by [`load_image`].
pub const SUPPORTED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"];

pub fn load_image(path: &Path) -> Result<image::RgbaImage, Error> {
    let img = image::ImageReader::open(path)?.with_guessed_format()?.decode()?;
    Ok(img.to_rgba8())
}

/// Flatten alpha and (optionally) blur the seam: the image that actually scrolls.
pub fn prepare_source(img: &image::RgbaImage, opts: &LoopOptions) -> Source {
    let mut src = Source::from_rgba(img, opts.background);
    if opts.seam_blur {
        frames::apply_seam_blur(&mut src, opts.blur_width, opts.direction.axis());
    }
    src
}

/// Default output path: same directory and name as the input, `.webm` extension.
pub fn default_output_path(input: &Path) -> PathBuf {
    input.with_extension("webm")
}

#[derive(Debug, Clone)]
pub struct RenderSummary {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub fps: u32,
    pub duration_secs: f64,
    pub bytes: u64,
    pub output: PathBuf,
}

/// Render `img` as a looping WebM at `output`.
///
/// `progress(done, total)` is called after each encoded frame. Setting `cancel`
/// stops the render, deletes the partial file and returns [`Error::Cancelled`].
pub fn render(
    img: &image::RgbaImage,
    opts: &LoopOptions,
    output: &Path,
    mut progress: impl FnMut(u32, u32),
    cancel: &AtomicBool,
) -> Result<RenderSummary, Error> {
    opts.validate()?;
    let src = prepare_source(img, opts);
    let (width, height) = (src.width, src.height);
    let n = opts.frame_count(width, height);
    let plan = FramePlan::for_image(width, height, n, opts.direction);

    let mut enc = encode::Vp9Encoder::new(&encode::EncoderConfig {
        width,
        height,
        fps: opts.fps,
        crf: opts.crf,
        cpu_used: opts.cpu_used,
        threads: opts.effective_threads(),
        kf_max_dist: (opts.fps * 10).max(1),
    })?;

    let tmp = partial_path(output);
    let result = (|| -> Result<u64, Error> {
        let file = BufWriter::with_capacity(1 << 20, File::create(&tmp)?);
        let app = concat!("loopwebm ", env!("CARGO_PKG_VERSION"));
        let mut mux = webm::WebmWriter::new(file, width, height, opts.fps, 1, app)?;
        let ts_ms = |pts: i64| ((pts as u64) * 1000 + opts.fps as u64 / 2) / opts.fps as u64;

        // Producer thread: frame generation + colour conversion, overlapped with encoding.
        std::thread::scope(|s| -> Result<(), Error> {
            let (tx, rx) = mpsc::sync_channel::<color::I420>(3);
            let (ret_tx, ret_rx) = mpsc::channel::<color::I420>();
            let src = &src;
            s.spawn(move || {
                let mut rgb = vec![0u8; src.rgb.len()];
                for k in 0..n {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    frames::render_frame(src, &plan, k, &mut rgb);
                    let mut yuv = ret_rx.try_recv().unwrap_or_else(|_| color::I420::new(width, height));
                    color::rgb_to_i420(&rgb, &mut yuv);
                    if tx.send(yuv).is_err() {
                        break;
                    }
                }
            });

            let mut sink = |p: encode::Packet<'_>| -> Result<(), Error> {
                mux.write_frame(p.data, ts_ms(p.pts), p.keyframe)?;
                progress(mux.frames_written() as u32, n);
                Ok(())
            };
            for k in 0..n {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                let frame = rx.recv().map_err(|_| Error::Cancelled)?;
                enc.encode(Some(&frame), k as i64, k == 0, &mut sink)?;
                let _ = ret_tx.send(frame);
            }
            enc.flush(&mut sink)?;
            Ok(())
        })?;

        if mux.frames_written() != n as u64 {
            return Err(Error::Encoder(format!("encoder produced {} of {n} frames", mux.frames_written())));
        }
        let duration_ms = n as f64 * 1000.0 / opts.fps as f64;
        let file = mux.finish(duration_ms)?;
        let file = file.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        Ok(file.metadata()?.len())
    })();

    match result {
        Ok(bytes) => {
            if output.exists() {
                std::fs::remove_file(output)?;
            }
            std::fs::rename(&tmp, output)?;
            Ok(RenderSummary {
                width,
                height,
                frames: n,
                fps: opts.fps,
                duration_secs: opts.duration_secs(width, height),
                bytes,
                output: output.to_path_buf(),
            })
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn partial_path(output: &Path) -> PathBuf {
    let mut name = output.file_name().map(|n| n.to_os_string()).unwrap_or_else(|| "out.webm".into());
    name.push(".partial");
    output.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_image(w: u32, h: u32) -> image::RgbaImage {
        image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, ((x ^ y) & 0xFF) as u8, 255])
        })
    }

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("loopwebm-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn encodes_a_valid_loop() {
        let img = test_image(64, 48);
        let opts = LoopOptions { fps: 24, px_per_frame: 2.0, cpu_used: 8, ..Default::default() };
        let out = tmpdir().join("loop.webm");
        let mut calls = 0;
        let s = render(&img, &opts, &out, |_, _| calls += 1, &AtomicBool::new(false)).unwrap();
        assert_eq!(s.frames, 32);
        assert_eq!(calls, 32);
        let buf = std::fs::read(&out).unwrap();
        let p = webm::probe(&buf).unwrap();
        assert_eq!(p.codec, "V_VP9");
        assert_eq!((p.width, p.height), (64, 48));
        assert_eq!(p.frames, 32);
        assert!(p.keyframes >= 1);
        assert!((p.duration_ms - 32.0 * 1000.0 / 24.0).abs() < 0.01);
        // VP9 keyframe signature in the first block.
        assert!(!partial_path(&out).exists());
    }

    #[test]
    fn odd_sizes_and_blur_and_ltr() {
        let img = test_image(33, 17);
        let opts = LoopOptions {
            fps: 10,
            px_per_frame: 3.0,
            seam_blur: true,
            blur_width: 4,
            direction: Direction::LeftToRight,
            cpu_used: 8,
            ..Default::default()
        };
        let out = tmpdir().join("odd.webm");
        let s = render(&img, &opts, &out, |_, _| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(s.frames, 11);
        let p = webm::probe(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!((p.width, p.height, p.frames), (33, 17, 11));
    }

    #[test]
    fn vertical_loops_use_height() {
        let img = test_image(40, 30);
        for direction in [Direction::TopToBottom, Direction::BottomToTop] {
            let opts = LoopOptions { fps: 15, direction, seam_blur: true, blur_width: 3, cpu_used: 8, ..Default::default() };
            let out = tmpdir().join(format!("{direction:?}.webm"));
            let s = render(&img, &opts, &out, |_, _| {}, &AtomicBool::new(false)).unwrap();
            assert_eq!(s.frames, 30, "{direction:?}");
            let p = webm::probe(&std::fs::read(&out).unwrap()).unwrap();
            assert_eq!((p.width, p.height, p.frames), (40, 30, 30));
        }
    }

    #[test]
    fn cancel_removes_partial_file() {
        let img = test_image(64, 64);
        let out = tmpdir().join("cancel.webm");
        let cancel = AtomicBool::new(true);
        let r = render(&img, &LoopOptions::default(), &out, |_, _| {}, &cancel);
        assert!(matches!(r, Err(Error::Cancelled)));
        assert!(!out.exists());
        assert!(!partial_path(&out).exists());
    }

    #[test]
    #[should_panic(expected = "plane buffers too small")]
    fn encoder_rejects_short_planes() {
        let mut enc = encode::Vp9Encoder::new(&encode::EncoderConfig {
            width: 16,
            height: 16,
            fps: 30,
            crf: 31,
            cpu_used: 8,
            threads: 1,
            kf_max_dist: 30,
        })
        .unwrap();
        let mut frame = color::I420::new(16, 16);
        frame.u.truncate(1);
        let _ = enc.encode(Some(&frame), 0, true, |_| Ok(()));
    }

    #[test]
    fn libvpx_is_linked() {
        assert!(encode::vpx_version().starts_with('v'));
    }
}
