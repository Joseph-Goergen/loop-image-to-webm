//! Wrap-around scrolling frames and the seam-blur pre-pass, along either axis.

use crate::options::{Axis, Direction};
use image::RgbaImage;

/// An RGB8 source image with alpha already flattened (and the seam optionally blurred).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub width: u32,
    pub height: u32,
    /// Packed RGB, `width * height * 3` bytes.
    pub rgb: Vec<u8>,
}

impl Source {
    /// Composite `img` over `background` and drop the alpha channel.
    pub fn from_rgba(img: &RgbaImage, background: [u8; 3]) -> Self {
        let (width, height) = img.dimensions();
        let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
        for p in img.pixels() {
            let a = p[3] as u32;
            for c in 0..3 {
                let v = (p[c] as u32 * a + background[c] as u32 * (255 - a) + 127) / 255;
                rgb.push(v as u8);
            }
        }
        Self { width, height, rgb }
    }

    /// RGBA copy (alpha = 255), e.g. for GUI textures.
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rgb.len() / 3 * 4);
        for px in self.rgb.chunks_exact(3) {
            out.extend_from_slice(px);
            out.push(255);
        }
        out
    }

    fn row(&self, y: u32) -> &[u8] {
        let w = self.width as usize * 3;
        &self.rgb[y as usize * w..(y as usize + 1) * w]
    }
}

/// Blend a Gaussian blur into the pixels near the junction where the image wraps.
///
/// For [`Axis::Horizontal`] that is the left/right edge pair (blurred horizontally);
/// for [`Axis::Vertical`] the top/bottom pair (blurred vertically). `band` is the width
/// in pixels on each side of the seam. The blur samples with wrap-around (so it mixes
/// both edges) and fades out with a smoothstep so it merges invisibly into the
/// untouched image. Pixels at distance `>= band` from the seam are left unchanged.
pub fn apply_seam_blur(src: &mut Source, band: u32, axis: Axis) {
    let (w, h) = (src.width as usize, src.height as usize);
    // A "line" runs along the scroll axis; `n` is its length, `lines` how many there are.
    let (n, lines) = match axis {
        Axis::Horizontal => (w, h),
        Axis::Vertical => (h, w),
    };
    if n < 2 || band == 0 {
        return;
    }
    // Byte offset of position `pos` on line `line`.
    let offset = |line: usize, pos: usize| -> usize {
        match axis {
            Axis::Horizontal => (line * w + pos) * 3,
            Axis::Vertical => (pos * w + line) * 3,
        }
    };

    let band = (band as usize).min(n / 2).max(1);
    let radius = band;
    let sigma = (band as f32 / 2.0).max(0.5);
    let kernel: Vec<f32> = {
        let k: Vec<f32> = (-(radius as i64)..=radius as i64)
            .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
            .collect();
        let sum: f32 = k.iter().sum();
        k.into_iter().map(|v| v / sum).collect()
    };

    // Positions affected: [0, band) and [n - band, n), with their blend weight.
    let positions: Vec<(usize, f32)> = (0..n)
        .filter_map(|p| {
            let d = (p as f32 + 0.5).min(n as f32 - p as f32 - 0.5);
            if d >= band as f32 {
                return None;
            }
            let t = 1.0 - d / band as f32;
            Some((p, t * t * (3.0 - 2.0 * t)))
        })
        .collect();

    let orig = src.rgb.clone();
    for line in 0..lines {
        for &(p, weight) in &positions {
            let mut acc = [0f32; 3];
            for (ki, kv) in kernel.iter().enumerate() {
                let sp = (p as i64 + ki as i64 - radius as i64).rem_euclid(n as i64) as usize;
                let o = offset(line, sp);
                for c in 0..3 {
                    acc[c] += orig[o + c] as f32 * kv;
                }
            }
            let o = offset(line, p);
            for c in 0..3 {
                let v = orig[o + c] as f32;
                src.rgb[o + c] = (v + (acc[c] - v) * weight).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Timing of one loop: how many frames, and how far each frame is shifted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FramePlan {
    /// Length of the scroll axis in pixels (image width or height).
    pub len: u32,
    pub frames: u32,
    pub direction: Direction,
}

impl FramePlan {
    pub fn new(len: u32, frames: u32, direction: Direction) -> Self {
        Self { len, frames: frames.max(1), direction }
    }

    /// Build the plan for a `width × height` image.
    pub fn for_image(width: u32, height: u32, frames: u32, direction: Direction) -> Self {
        Self::new(direction.axis_len(width, height), frames, direction)
    }

    pub fn axis(&self) -> Axis {
        self.direction.axis()
    }

    /// Signed sample shift for frame `k`, in source pixels along the scroll axis.
    /// Output pixel at position `p` shows source pixel `p + shift` (mod axis length).
    pub fn shift(&self, k: u32) -> f64 {
        let k = k % self.frames;
        let off = k as f64 * self.len as f64 / self.frames as f64;
        match self.direction {
            // Content moves left / up => we sample further right / down as time goes on.
            Direction::RightToLeft | Direction::BottomToTop => off,
            Direction::LeftToRight | Direction::TopToBottom => -off,
        }
    }
}

/// Render frame `k` of `plan` into `out` (packed RGB, same size as `src`).
pub fn render_frame(src: &Source, plan: &FramePlan, k: u32, out: &mut [u8]) {
    render_shift(src, plan.shift(k), plan.axis(), out);
}

/// Render `src` shifted by `shift` pixels along `axis`, wrapping around the edges.
/// Fractional shifts blend the two neighbouring pixels (or rows).
pub fn render_shift(src: &Source, shift: f64, axis: Axis, out: &mut [u8]) {
    assert_eq!(out.len(), src.rgb.len());
    let (w, h) = (src.width as usize, src.height as usize);
    let n = match axis {
        Axis::Horizontal => w,
        Axis::Vertical => h,
    };
    if n == 0 {
        return;
    }
    let base = shift.floor();
    let mut start = (base as i64).rem_euclid(n as i64) as usize;
    // 8-bit fixed-point weight for the next pixel/row.
    let mut fw = ((shift - base) * 256.0).round() as u32;
    if fw == 256 {
        start = (start + 1) % n;
        fw = 0;
    }
    let iw = 256 - fw;
    let stride = w * 3;

    match axis {
        Axis::Horizontal => {
            for y in 0..src.height {
                let row = src.row(y);
                let dst = &mut out[y as usize * stride..(y as usize + 1) * stride];
                if fw == 0 {
                    // Pure rotation: two memcpys.
                    let first = (w - start) * 3;
                    dst[..first].copy_from_slice(&row[start * 3..]);
                    dst[first..].copy_from_slice(&row[..start * 3]);
                } else {
                    for x in 0..w {
                        let a = (start + x) % w;
                        let b = (a + 1) % w;
                        for c in 0..3 {
                            let v = row[a * 3 + c] as u32 * iw + row[b * 3 + c] as u32 * fw;
                            dst[x * 3 + c] = ((v + 128) >> 8) as u8;
                        }
                    }
                }
            }
        }
        Axis::Vertical => {
            for y in 0..h {
                let a = (start + y) % h;
                let dst = &mut out[y * stride..(y + 1) * stride];
                let ra = src.row(a as u32);
                if fw == 0 {
                    dst.copy_from_slice(ra);
                } else {
                    let rb = src.row(((a + 1) % h) as u32);
                    for ((d, &pa), &pb) in dst.iter_mut().zip(ra).zip(rb) {
                        *d = ((pa as u32 * iw + pb as u32 * fw + 128) >> 8) as u8;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Source {
        let mut rgb = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgb.extend_from_slice(&[(x * 7 % 256) as u8, (y * 13 % 256) as u8, ((x + y) % 256) as u8]);
            }
        }
        Source { width: w, height: h, rgb }
    }

    fn frame(src: &Source, plan: &FramePlan, k: u32) -> Vec<u8> {
        let mut out = vec![0; src.rgb.len()];
        render_frame(src, plan, k, &mut out);
        out
    }

    fn px(src: &Source, x: u32, y: u32) -> &[u8] {
        let i = ((y * src.width + x) * 3) as usize;
        &src.rgb[i..i + 3]
    }

    fn frame_px(f: &[u8], w: u32, x: u32, y: u32) -> &[u8] {
        let i = ((y * w + x) * 3) as usize;
        &f[i..i + 3]
    }

    #[test]
    fn frame_zero_is_source() {
        let src = gradient(37, 23);
        for dir in Direction::ALL {
            let plan = FramePlan::for_image(37, 23, 11, dir);
            assert_eq!(frame(&src, &plan, 0), src.rgb, "{dir:?}");
        }
    }

    #[test]
    fn loop_wraps_back_to_start() {
        let src = gradient(40, 30);
        for dir in Direction::ALL {
            for frames in [40, 13, 7] {
                let plan = FramePlan::for_image(40, 30, frames, dir);
                assert_eq!(frame(&src, &plan, frames), frame(&src, &plan, 0), "{dir:?} {frames}");
            }
        }
    }

    #[test]
    fn content_moves_in_the_named_direction() {
        let (w, h) = (10, 8);
        let src = gradient(w, h);
        // Right to Left: the pixel at x=1 moves to x=0; old left edge wraps to the right.
        let rtl = FramePlan::for_image(w, h, w, Direction::RightToLeft);
        let f1 = frame(&src, &rtl, 1);
        assert_eq!(frame_px(&f1, w, 0, 0), px(&src, 1, 0));
        assert_eq!(frame_px(&f1, w, w - 1, 0), px(&src, 0, 0));
        // Left to Right: x=0 moves to x=1.
        let ltr = frame(&src, &FramePlan::for_image(w, h, w, Direction::LeftToRight), 1);
        assert_eq!(frame_px(&ltr, w, 1, 0), px(&src, 0, 0));
        // Top to Bottom: row 0 moves to row 1; bottom row wraps to the top.
        let ttb = frame(&src, &FramePlan::for_image(w, h, h, Direction::TopToBottom), 1);
        assert_eq!(frame_px(&ttb, w, 3, 1), px(&src, 3, 0));
        assert_eq!(frame_px(&ttb, w, 3, 0), px(&src, 3, h - 1));
        // Bottom to Top: row 1 moves to row 0; top row wraps to the bottom.
        let btt = frame(&src, &FramePlan::for_image(w, h, h, Direction::BottomToTop), 1);
        assert_eq!(frame_px(&btt, w, 3, 0), px(&src, 3, 1));
        assert_eq!(frame_px(&btt, w, 3, h - 1), px(&src, 3, 0));
    }

    #[test]
    fn opposite_directions_mirror_each_other() {
        let src = gradient(24, 18);
        for (a, b, n) in [
            (Direction::RightToLeft, Direction::LeftToRight, 24),
            (Direction::BottomToTop, Direction::TopToBottom, 18),
        ] {
            let pa = FramePlan::for_image(24, 18, n, a);
            let pb = FramePlan::for_image(24, 18, n, b);
            for k in 0..n {
                assert_eq!(frame(&src, &pa, k), frame(&src, &pb, (n - k) % n), "{a:?} k={k}");
            }
        }
    }

    #[test]
    fn fractional_steps_interpolate() {
        let src = Source { width: 4, height: 1, rgb: vec![0, 0, 0, 100, 100, 100, 200, 200, 200, 100, 100, 100] };
        let mut out = vec![0; 12];
        render_shift(&src, 0.5, Axis::Horizontal, &mut out);
        assert_eq!(&out[0..3], &[50, 50, 50]);
        assert_eq!(&out[9..12], &[50, 50, 50]); // wraps between last and first

        // Same data as a 1×4 column.
        let col = Source { width: 1, height: 4, rgb: src.rgb.clone() };
        render_shift(&col, 0.5, Axis::Vertical, &mut out);
        assert_eq!(&out[0..3], &[50, 50, 50]);
        assert_eq!(&out[9..12], &[50, 50, 50]);
    }

    #[test]
    fn seam_blur_only_touches_band() {
        let (w, h) = (64u32, 48u32);
        let orig = gradient(w, h);
        for axis in [Axis::Horizontal, Axis::Vertical] {
            let mut src = orig.clone();
            apply_seam_blur(&mut src, 8, axis);
            let n = if axis == Axis::Horizontal { w } else { h };
            for y in 0..h {
                for x in 0..w {
                    let p = if axis == Axis::Horizontal { x } else { y };
                    if (8..n - 8).contains(&p) {
                        assert_eq!(px(&src, x, y), px(&orig, x, y), "{axis:?} ({x},{y}) changed");
                    }
                }
            }
            assert_ne!(src.rgb, orig.rgb, "{axis:?}: blur had no effect at the seam");
        }
    }

    #[test]
    fn seam_blur_reduces_edge_jump() {
        // Hard seam: first half white, second half black, along each axis.
        let n = 32u32;
        let half = |i: u32| if i < n / 2 { 255u8 } else { 0 };
        let row: Vec<u8> = (0..n).flat_map(|x| [half(x); 3]).collect();
        let mut hsrc = Source { width: n, height: 1, rgb: row.clone() };
        let mut vsrc = Source { width: 1, height: n, rgb: row };
        let jump = |s: &Source| (s.rgb[0] as i32 - s.rgb[s.rgb.len() - 3] as i32).abs();
        for (src, axis) in [(&mut hsrc, Axis::Horizontal), (&mut vsrc, Axis::Vertical)] {
            let before = jump(src);
            apply_seam_blur(src, 6, axis);
            let after = jump(src);
            assert!(after < before / 4, "{axis:?}: before={before} after={after}");
        }
    }

    #[test]
    fn flatten_alpha_onto_background() {
        let img = RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 0]).unwrap();
        let s = Source::from_rgba(&img, [10, 20, 30]);
        assert_eq!(s.rgb, vec![255, 0, 0, 10, 20, 30]);
    }
}
