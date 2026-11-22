//! Safe wrapper around the libvpx VP9 encoder (via `vpx_shim.c`).

use crate::color::I420;
use crate::Error;
use std::ffi::{c_char, c_int, c_void, CStr};
use std::ptr::NonNull;

#[repr(C)]
struct LwEncParams {
    width: c_int,
    height: c_int,
    fps_num: c_int,
    fps_den: c_int,
    crf: c_int,
    cpu_used: c_int,
    threads: c_int,
    kf_max_dist: c_int,
}

extern "C" {
    fn lw_vpx_version() -> *const c_char;
    fn lw_enc_new(p: *const LwEncParams, err: *mut *const c_char) -> *mut c_void;
    fn lw_enc_error(e: *mut c_void) -> *const c_char;
    fn lw_enc_encode(e: *mut c_void, y: *const u8, u: *const u8, v: *const u8, pts: i64, force_key: c_int) -> c_int;
    fn lw_enc_next_packet(
        e: *mut c_void,
        data: *mut *const u8,
        size: *mut usize,
        pts: *mut i64,
        is_key: *mut c_int,
    ) -> c_int;
    fn lw_enc_free(e: *mut c_void);
}

fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        return "unknown error".into();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// libvpx version string, e.g. "v1.17.0".
pub fn vpx_version() -> String {
    cstr(unsafe { lw_vpx_version() })
}

#[derive(Clone, Debug)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub crf: u8,
    pub cpu_used: u8,
    pub threads: usize,
    pub kf_max_dist: u32,
}

/// One compressed frame.
pub struct Packet<'a> {
    pub data: &'a [u8],
    /// Presentation timestamp in frames.
    pub pts: i64,
    pub keyframe: bool,
}

pub struct Vp9Encoder {
    ptr: NonNull<c_void>,
    width: u32,
    height: u32,
}

// The libvpx context is only ever used from one thread at a time.
unsafe impl Send for Vp9Encoder {}

impl Vp9Encoder {
    pub fn new(cfg: &EncoderConfig) -> Result<Self, Error> {
        if cfg.width == 0 || cfg.height == 0 || cfg.width > 16384 || cfg.height > 16384 {
            return Err(Error::InvalidOption(format!(
                "image size {}x{} is outside VP9's supported range (1..=16384)",
                cfg.width, cfg.height
            )));
        }
        let params = LwEncParams {
            width: cfg.width as c_int,
            height: cfg.height as c_int,
            fps_num: cfg.fps as c_int,
            fps_den: 1,
            crf: cfg.crf as c_int,
            cpu_used: cfg.cpu_used as c_int,
            threads: cfg.threads as c_int,
            kf_max_dist: cfg.kf_max_dist as c_int,
        };
        let mut err: *const c_char = std::ptr::null();
        let ptr = unsafe { lw_enc_new(&params, &mut err) };
        match NonNull::new(ptr) {
            Some(ptr) => Ok(Self { ptr, width: cfg.width, height: cfg.height }),
            None => Err(Error::Encoder(cstr(err))),
        }
    }

    /// Encode one frame (or flush with `None`) and hand every produced packet to `sink`.
    pub fn encode(
        &mut self,
        frame: Option<&I420>,
        pts: i64,
        force_key: bool,
        mut sink: impl FnMut(Packet<'_>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if let Some(f) = frame {
            // The C side reads exactly w*h luma and cw*ch bytes per chroma plane from
            // these pointers, so the buffers must be at least that long (I420's fields
            // are public, so this can't be assumed from the width/height alone).
            let (cw, ch) = crate::color::chroma_size(self.width, self.height);
            assert!(f.width == self.width && f.height == self.height, "frame size mismatch");
            assert!(
                f.y.len() >= self.width as usize * self.height as usize
                    && f.u.len() >= cw * ch
                    && f.v.len() >= cw * ch,
                "I420 plane buffers too small for {}x{}",
                self.width,
                self.height
            );
        }
        let rc = unsafe {
            match frame {
                Some(f) => {
                    lw_enc_encode(self.ptr.as_ptr(), f.y.as_ptr(), f.u.as_ptr(), f.v.as_ptr(), pts, force_key as c_int)
                }
                None => lw_enc_encode(
                    self.ptr.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    0,
                ),
            }
        };
        if rc != 0 {
            return Err(Error::Encoder(cstr(unsafe { lw_enc_error(self.ptr.as_ptr()) })));
        }
        loop {
            let mut data: *const u8 = std::ptr::null();
            let mut size = 0usize;
            let mut pts = 0i64;
            let mut key: c_int = 0;
            let got = unsafe { lw_enc_next_packet(self.ptr.as_ptr(), &mut data, &mut size, &mut pts, &mut key) };
            if got == 0 {
                break;
            }
            let slice = unsafe { std::slice::from_raw_parts(data, size) };
            sink(Packet { data: slice, pts, keyframe: key != 0 })?;
        }
        Ok(())
    }

    /// Drain all buffered frames. Call repeatedly until it yields no packets.
    pub fn flush(&mut self, mut sink: impl FnMut(Packet<'_>) -> Result<(), Error>) -> Result<(), Error> {
        loop {
            let mut any = false;
            self.encode(None, 0, false, |p| {
                any = true;
                sink(p)
            })?;
            if !any {
                return Ok(());
            }
        }
    }
}

impl Drop for Vp9Encoder {
    fn drop(&mut self) {
        unsafe { lw_enc_free(self.ptr.as_ptr()) }
    }
}
