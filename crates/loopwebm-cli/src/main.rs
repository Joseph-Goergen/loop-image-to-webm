use clap::{Parser, ValueEnum};
use indicatif::{ProgressBar, ProgressStyle};
use loopwebm_core::{self as core, Direction, LoopOptions};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Dir {
    /// Content moves right to left (default)
    Rtl,
    /// Content moves left to right
    Ltr,
    /// Content moves top to bottom
    Ttb,
    /// Content moves bottom to top
    Btt,
}

/// Turn an image into a seamless, horizontally scrolling WebM (VP9) loop.
///
/// The image scrolls horizontally (rtl/ltr) or vertically (ttb/btt) and its opposite
/// edges are stitched together. Each frame moves the image by --step pixels, so the
/// scroll speed is fps × step pixels per second and one loop lasts
/// (image width ÷ step) frames, or (image height ÷ step) for vertical directions.
#[derive(Parser, Debug)]
#[command(name = "loopwebm", version, about, long_about)]
struct Args {
    /// Input image (PNG, JPEG, WebP, BMP, GIF, TIFF)
    input: PathBuf,

    /// Output .webm file [default: <input>.webm]
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Target frames per second (also sets scroll speed)
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u32).range(1..=240))]
    fps: u32,

    /// Direction of motion
    #[arg(short, long, value_enum, default_value_t = Dir::Rtl)]
    direction: Dir,

    /// Pixels moved per frame
    #[arg(long, default_value_t = 1.0)]
    step: f64,

    /// Blur the seam where the opposite edges meet
    #[arg(long)]
    seam_blur: bool,

    /// Width in pixels of the blurred band on each side of the seam
    #[arg(long, default_value_t = 16)]
    blur_width: u32,

    /// VP9 quality: 0 (best) to 63 (smallest file)
    #[arg(long, default_value_t = 31, value_parser = clap::value_parser!(u8).range(0..=63))]
    crf: u8,

    /// Encoder speed: 0 (slowest, best) to 8 (fastest)
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(0..=8))]
    cpu_used: u8,

    /// Background colour for transparent pixels
    #[arg(long, default_value = "#000000")]
    bg: String,

    /// Encoder threads (0 = all cores)
    #[arg(long, default_value_t = 0)]
    threads: usize,

    /// Overwrite the output file if it exists
    #[arg(short = 'y', long)]
    overwrite: bool,

    /// Only print errors
    #[arg(short, long)]
    quiet: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), core::Error> {
    let opts = LoopOptions {
        fps: args.fps,
        direction: match args.direction {
            Dir::Rtl => Direction::RightToLeft,
            Dir::Ltr => Direction::LeftToRight,
            Dir::Ttb => Direction::TopToBottom,
            Dir::Btt => Direction::BottomToTop,
        },
        px_per_frame: args.step,
        seam_blur: args.seam_blur,
        blur_width: args.blur_width,
        crf: args.crf,
        cpu_used: args.cpu_used,
        background: core::parse_hex_color(&args.bg)?,
        threads: args.threads,
    };
    opts.validate()?;

    let output = args.output.clone().unwrap_or_else(|| core::default_output_path(&args.input));
    if output.exists() && !args.overwrite {
        return Err(core::Error::InvalidOption(format!(
            "{} already exists (use -y to overwrite)",
            output.display()
        )));
    }

    let img = core::load_image(&args.input)?;
    let (w, h) = img.dimensions();
    let frames = opts.frame_count(w, h);
    let secs = opts.duration_secs(w, h);
    if !args.quiet {
        eprintln!(
            "{}: {w}x{h} → {frames} frames @ {} fps = {:.2}s loop ({:.1} px/s, {})",
            args.input.display(),
            opts.fps,
            secs,
            opts.direction.axis_len(w, h) as f64 / secs,
            opts.direction.label(),
        );
    }

    let cancel = Arc::new(AtomicBool::new(false));
    {
        let c = cancel.clone();
        let _ = ctrlc::set_handler(move || c.store(true, Ordering::Relaxed));
    }

    let bar = if args.quiet { ProgressBar::hidden() } else { ProgressBar::new(frames as u64) };
    bar.set_style(
        ProgressStyle::with_template("{bar:40.cyan/blue} {pos}/{len} frames  {elapsed_precise} (eta {eta})")
            .unwrap()
            .progress_chars("█▉▊▋▌▍▎▏ "),
    );
    let summary = core::render(&img, &opts, &output, |done, _| bar.set_position(done as u64), &cancel);
    bar.finish_and_clear();
    let summary = summary?;

    if !args.quiet {
        eprintln!(
            "wrote {} ({:.2} MB, {} frames, {:.2}s)",
            summary.output.display(),
            summary.bytes as f64 / 1_048_576.0,
            summary.frames,
            summary.duration_secs
        );
    }
    Ok(())
}
