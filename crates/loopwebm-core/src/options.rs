use crate::Error;

/// Which way the image content travels across the frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    /// Content moves from right to left (the default). Left/right edges are stitched.
    #[default]
    RightToLeft,
    /// Content moves from left to right. Left/right edges are stitched.
    LeftToRight,
    /// Content moves from top to bottom. Top/bottom edges are stitched.
    TopToBottom,
    /// Content moves from bottom to top. Top/bottom edges are stitched.
    BottomToTop,
}

/// The axis the image scrolls along (and whose edges are stitched together).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

impl Direction {
    pub const ALL: [Direction; 4] =
        [Direction::RightToLeft, Direction::LeftToRight, Direction::TopToBottom, Direction::BottomToTop];

    pub fn label(self) -> &'static str {
        match self {
            Direction::RightToLeft => "Right to Left",
            Direction::LeftToRight => "Left to Right",
            Direction::TopToBottom => "Top to Bottom",
            Direction::BottomToTop => "Bottom to Top",
        }
    }

    pub fn axis(self) -> Axis {
        match self {
            Direction::RightToLeft | Direction::LeftToRight => Axis::Horizontal,
            Direction::TopToBottom | Direction::BottomToTop => Axis::Vertical,
        }
    }

    /// Length in pixels of the scroll axis for a `width × height` image.
    pub fn axis_len(self, width: u32, height: u32) -> u32 {
        match self.axis() {
            Axis::Horizontal => width,
            Axis::Vertical => height,
        }
    }
}

/// Everything that controls how a loop is rendered and encoded.
#[derive(Clone, Debug, PartialEq)]
pub struct LoopOptions {
    /// Output frame rate. Scroll speed is `fps × px_per_frame` pixels per second.
    pub fps: u32,
    pub direction: Direction,
    /// Movement per frame along the scroll axis, in source pixels. The actual step is
    /// adjusted slightly so that a whole number of frames covers the axis exactly.
    pub px_per_frame: f64,
    /// Soften the junction where opposite edges meet (left/right or top/bottom).
    pub seam_blur: bool,
    /// Width (px) of the blurred band on each side of the seam.
    pub blur_width: u32,
    /// VP9 constant-quality level, 0 (best) ..= 63 (smallest).
    pub crf: u8,
    /// libvpx speed/quality trade-off, 0 (slowest, best) ..= 8 (fastest).
    pub cpu_used: u8,
    /// Colour transparent pixels are composited onto (RGB).
    pub background: [u8; 3],
    /// Encoder threads; 0 = number of logical CPUs.
    pub threads: usize,
}

impl Default for LoopOptions {
    fn default() -> Self {
        Self {
            fps: 30,
            direction: Direction::RightToLeft,
            px_per_frame: 1.0,
            seam_blur: false,
            blur_width: 16,
            crf: 31,
            cpu_used: 4,
            background: [0, 0, 0],
            threads: 0,
        }
    }
}

pub const MAX_FPS: u32 = 240;

impl LoopOptions {
    pub fn validate(&self) -> Result<(), Error> {
        if self.fps == 0 || self.fps > MAX_FPS {
            return Err(Error::InvalidOption(format!("fps must be 1..={MAX_FPS}")));
        }
        if !(self.px_per_frame.is_finite() && self.px_per_frame > 0.0) {
            return Err(Error::InvalidOption("px_per_frame must be > 0".into()));
        }
        if self.crf > 63 {
            return Err(Error::InvalidOption("crf must be 0..=63".into()));
        }
        if self.cpu_used > 8 {
            return Err(Error::InvalidOption("cpu_used must be 0..=8".into()));
        }
        if self.seam_blur && self.blur_width == 0 {
            return Err(Error::InvalidOption("blur_width must be > 0".into()));
        }
        Ok(())
    }

    /// Number of frames in one full loop for a `width × height` image
    /// (the image scrolls its full width, or height for vertical directions).
    pub fn frame_count(&self, width: u32, height: u32) -> u32 {
        let len = self.direction.axis_len(width, height);
        ((len as f64 / self.px_per_frame).round() as u32).max(1)
    }

    /// Length of one loop in seconds.
    pub fn duration_secs(&self, width: u32, height: u32) -> f64 {
        self.frame_count(width, height) as f64 / self.fps as f64
    }

    pub fn effective_threads(&self) -> usize {
        if self.threads > 0 {
            self.threads
        } else {
            std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
        }
        .min(64)
    }
}

/// Parse `#rrggbb` / `rrggbb` into RGB.
pub fn parse_hex_color(s: &str) -> Result<[u8; 3], Error> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::InvalidOption(format!("invalid colour '{s}', expected #rrggbb")));
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap();
    Ok([p(0), p(2), p(4)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_count_rounds_and_is_at_least_one() {
        let mut o = LoopOptions::default();
        assert_eq!(o.frame_count(1920, 1080), 1920);
        o.px_per_frame = 3.0;
        assert_eq!(o.frame_count(1000, 50), 333);
        o.px_per_frame = 10_000.0;
        assert_eq!(o.frame_count(100, 100), 1);
    }

    #[test]
    fn vertical_directions_use_height() {
        for d in [Direction::TopToBottom, Direction::BottomToTop] {
            let o = LoopOptions { direction: d, px_per_frame: 2.0, ..Default::default() };
            assert_eq!(o.frame_count(1920, 1080), 540);
        }
    }

    #[test]
    fn hex_colors() {
        assert_eq!(parse_hex_color("#ff8000").unwrap(), [255, 128, 0]);
        assert_eq!(parse_hex_color("000000").unwrap(), [0, 0, 0]);
        assert!(parse_hex_color("#fff").is_err());
    }
}
