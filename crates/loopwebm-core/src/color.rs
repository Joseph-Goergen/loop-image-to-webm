//! RGB → I420 (YUV 4:2:0, BT.709, limited range). Odd sizes are supported:
//! chroma planes are `ceil(w/2) × ceil(h/2)`.

/// Planar I420 frame buffer, tightly packed.
pub struct I420 {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl I420 {
    pub fn new(width: u32, height: u32) -> Self {
        let (cw, ch) = chroma_size(width, height);
        Self {
            width,
            height,
            y: vec![0; width as usize * height as usize],
            u: vec![0; cw * ch],
            v: vec![0; cw * ch],
        }
    }
}

pub fn chroma_size(width: u32, height: u32) -> (usize, usize) {
    (width.div_ceil(2) as usize, height.div_ceil(2) as usize)
}

#[inline]
fn luma(r: i32, g: i32, b: i32) -> u8 {
    (16 + ((47 * r + 157 * g + 16 * b + 128) >> 8)).clamp(16, 235) as u8
}

#[inline]
fn chroma(r: i32, g: i32, b: i32) -> (u8, u8) {
    let u = 128 + ((-26 * r - 86 * g + 112 * b + 128) >> 8);
    let v = 128 + ((112 * r - 102 * g - 10 * b + 128) >> 8);
    (u.clamp(16, 240) as u8, v.clamp(16, 240) as u8)
}

/// Convert packed RGB8 (`w*h*3`) into `out`.
pub fn rgb_to_i420(rgb: &[u8], out: &mut I420) {
    let w = out.width as usize;
    let h = out.height as usize;
    assert_eq!(rgb.len(), w * h * 3);
    let (cw, ch) = chroma_size(out.width, out.height);

    for (dst, src) in out.y.chunks_exact_mut(w).zip(rgb.chunks_exact(w * 3)) {
        for (d, p) in dst.iter_mut().zip(src.chunks_exact(3)) {
            *d = luma(p[0] as i32, p[1] as i32, p[2] as i32);
        }
    }

    for cy in 0..ch {
        let y0 = cy * 2;
        let y1 = (y0 + 1).min(h - 1);
        for cx in 0..cw {
            let x0 = cx * 2;
            let x1 = (x0 + 1).min(w - 1);
            let mut s = [0i32; 3];
            for (yy, xx) in [(y0, x0), (y0, x1), (y1, x0), (y1, x1)] {
                let i = (yy * w + xx) * 3;
                s[0] += rgb[i] as i32;
                s[1] += rgb[i + 1] as i32;
                s[2] += rgb[i + 2] as i32;
            }
            let (u, v) = chroma((s[0] + 2) >> 2, (s[1] + 2) >> 2, (s[2] + 2) >> 2);
            out.u[cy * cw + cx] = u;
            out.v[cy * cw + cx] = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_colors() {
        assert_eq!(luma(0, 0, 0), 16);
        assert_eq!(luma(255, 255, 255), 235);
        assert_eq!(chroma(128, 128, 128), (128, 128));
        assert_eq!(chroma(255, 255, 255), (128, 128));
        let (u, v) = chroma(255, 0, 0);
        assert!(u < 110 && v >= 239, "red -> u={u} v={v}");
        let (u, _) = chroma(0, 0, 255);
        assert!(u >= 239);
    }

    #[test]
    fn odd_dimensions() {
        let (w, h) = (5u32, 3u32);
        let rgb = vec![200u8; (w * h * 3) as usize];
        let mut f = I420::new(w, h);
        rgb_to_i420(&rgb, &mut f);
        assert_eq!(f.u.len(), 3 * 2);
        assert!(f.u.iter().all(|&c| c == 128));
    }
}
