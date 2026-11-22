//! Minimal WebM (Matroska/EBML) muxer for a single VP9 video track.
//!
//! Layout written:
//!   EBML header
//!   Segment (size patched at the end)
//!     [reserved space → SeekHead + Void, written at the end]
//!     Info (Duration patched at the end)
//!     Tracks
//!     Cluster*  (Timecode + SimpleBlock*)
//!     Cues

use std::io::{self, Seek, SeekFrom, Write};

pub mod ids {
    pub const EBML: u32 = 0x1A45DFA3;
    pub const EBML_VERSION: u32 = 0x4286;
    pub const EBML_READ_VERSION: u32 = 0x42F7;
    pub const EBML_MAX_ID_LENGTH: u32 = 0x42F2;
    pub const EBML_MAX_SIZE_LENGTH: u32 = 0x42F3;
    pub const DOC_TYPE: u32 = 0x4282;
    pub const DOC_TYPE_VERSION: u32 = 0x4287;
    pub const DOC_TYPE_READ_VERSION: u32 = 0x4285;
    pub const SEGMENT: u32 = 0x18538067;
    pub const SEEK_HEAD: u32 = 0x114D9B74;
    pub const SEEK: u32 = 0x4DBB;
    pub const SEEK_ID: u32 = 0x53AB;
    pub const SEEK_POSITION: u32 = 0x53AC;
    pub const VOID: u32 = 0xEC;
    pub const INFO: u32 = 0x1549A966;
    pub const TIMECODE_SCALE: u32 = 0x2AD7B1;
    pub const DURATION: u32 = 0x4489;
    pub const MUXING_APP: u32 = 0x4D80;
    pub const WRITING_APP: u32 = 0x5741;
    pub const TRACKS: u32 = 0x1654AE6B;
    pub const TRACK_ENTRY: u32 = 0xAE;
    pub const TRACK_NUMBER: u32 = 0xD7;
    pub const TRACK_UID: u32 = 0x73C5;
    pub const TRACK_TYPE: u32 = 0x83;
    pub const FLAG_LACING: u32 = 0x9C;
    pub const CODEC_ID: u32 = 0x86;
    pub const DEFAULT_DURATION: u32 = 0x23E383;
    pub const VIDEO: u32 = 0xE0;
    pub const PIXEL_WIDTH: u32 = 0xB0;
    pub const PIXEL_HEIGHT: u32 = 0xBA;
    pub const CLUSTER: u32 = 0x1F43B675;
    pub const TIMECODE: u32 = 0xE7;
    pub const SIMPLE_BLOCK: u32 = 0xA3;
    pub const CUES: u32 = 0x1C53BB6B;
    pub const CUE_POINT: u32 = 0xBB;
    pub const CUE_TIME: u32 = 0xB3;
    pub const CUE_TRACK_POSITIONS: u32 = 0xB7;
    pub const CUE_TRACK: u32 = 0xF7;
    pub const CUE_CLUSTER_POSITION: u32 = 0xF1;
}
use ids::*;

/// Matroska timestamps are in milliseconds (TimecodeScale = 1,000,000 ns).
const TIMECODE_SCALE_NS: u64 = 1_000_000;
const MAX_CLUSTER_MS: u64 = 5_000;
const SEEKHEAD_RESERVED: usize = 128;

fn put_id(buf: &mut Vec<u8>, id: u32) {
    let bytes = id.to_be_bytes();
    let skip = bytes.iter().position(|&b| b != 0).unwrap_or(3);
    buf.extend_from_slice(&bytes[skip..]);
}

/// EBML variable-length size.
fn put_size(buf: &mut Vec<u8>, n: u64) {
    let mut len = 1;
    while len < 8 && n >= (1u64 << (7 * len)) - 1 {
        len += 1;
    }
    put_size_len(buf, n, len);
}

fn put_size_len(buf: &mut Vec<u8>, n: u64, len: usize) {
    let v = n | (1u64 << (7 * len));
    buf.extend_from_slice(&v.to_be_bytes()[8 - len..]);
}

fn put_uint(buf: &mut Vec<u8>, id: u32, v: u64) {
    let bytes = v.to_be_bytes();
    let skip = bytes.iter().position(|&b| b != 0).unwrap_or(7);
    put_id(buf, id);
    put_size(buf, (8 - skip) as u64);
    buf.extend_from_slice(&bytes[skip..]);
}

/// Unsigned int with a fixed 8-byte payload (so it can be patched in place).
fn put_uint8(buf: &mut Vec<u8>, id: u32, v: u64) {
    put_id(buf, id);
    put_size(buf, 8);
    buf.extend_from_slice(&v.to_be_bytes());
}

fn put_float(buf: &mut Vec<u8>, id: u32, v: f64) {
    put_id(buf, id);
    put_size(buf, 8);
    buf.extend_from_slice(&v.to_be_bytes());
}

fn put_bytes(buf: &mut Vec<u8>, id: u32, v: &[u8]) {
    put_id(buf, id);
    put_size(buf, v.len() as u64);
    buf.extend_from_slice(v);
}

fn put_master(buf: &mut Vec<u8>, id: u32, content: &[u8]) {
    put_bytes(buf, id, content);
}

fn void(len: usize) -> Vec<u8> {
    // ID (1 byte) + 1-byte size (< 127) or 8-byte size.
    let mut b = vec![];
    if len < 2 {
        panic!("void element needs at least 2 bytes");
    }
    put_id(&mut b, VOID);
    if len - 2 < 127 {
        put_size_len(&mut b, (len - 2) as u64, 1);
    } else {
        put_size_len(&mut b, (len - 9) as u64, 8);
    }
    b.resize(len, 0);
    b
}

fn id_bytes(id: u32) -> Vec<u8> {
    let mut b = vec![];
    put_id(&mut b, id);
    b
}

struct Cluster {
    timecode_ms: u64,
    body: Vec<u8>,
    starts_with_key: bool,
}

pub struct WebmWriter<W: Write + Seek> {
    w: W,
    segment_size_pos: u64,
    segment_data_start: u64,
    seekhead_pos: u64,
    duration_pos: u64,
    info_pos: u64,
    tracks_pos: u64,
    cluster: Option<Cluster>,
    cues: Vec<(u64, u64)>,
    frames: u64,
}

impl<W: Write + Seek> WebmWriter<W> {
    pub fn new(mut w: W, width: u32, height: u32, fps_num: u32, fps_den: u32, app: &str) -> io::Result<Self> {
        // EBML header
        let mut h = vec![];
        put_uint(&mut h, EBML_VERSION, 1);
        put_uint(&mut h, EBML_READ_VERSION, 1);
        put_uint(&mut h, EBML_MAX_ID_LENGTH, 4);
        put_uint(&mut h, EBML_MAX_SIZE_LENGTH, 8);
        put_bytes(&mut h, DOC_TYPE, b"webm");
        put_uint(&mut h, DOC_TYPE_VERSION, 4);
        put_uint(&mut h, DOC_TYPE_READ_VERSION, 2);
        let mut out = vec![];
        put_master(&mut out, EBML, &h);

        // Segment with an 8-byte size placeholder ("unknown" until finish()).
        put_id(&mut out, SEGMENT);
        let segment_size_pos = out.len() as u64;
        out.extend_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        let segment_data_start = out.len() as u64;

        // Reserved space for the SeekHead.
        let seekhead_pos = out.len() as u64;
        out.extend_from_slice(&void(SEEKHEAD_RESERVED));

        // Info
        let info_pos = out.len() as u64;
        let mut info = vec![];
        put_uint(&mut info, TIMECODE_SCALE, TIMECODE_SCALE_NS);
        put_bytes(&mut info, MUXING_APP, app.as_bytes());
        put_bytes(&mut info, WRITING_APP, app.as_bytes());
        let dur_rel = info.len() + 3; // 2-byte ID + 1-byte size precede the float
        put_float(&mut info, DURATION, 0.0);
        let mut info_el = vec![];
        put_id(&mut info_el, INFO);
        put_size(&mut info_el, info.len() as u64);
        let duration_pos = info_pos + info_el.len() as u64 + dur_rel as u64;
        info_el.extend_from_slice(&info);
        out.extend_from_slice(&info_el);

        // Tracks
        let tracks_pos = out.len() as u64;
        let mut video = vec![];
        put_uint(&mut video, PIXEL_WIDTH, width as u64);
        put_uint(&mut video, PIXEL_HEIGHT, height as u64);
        let mut entry = vec![];
        put_uint(&mut entry, TRACK_NUMBER, 1);
        put_uint(&mut entry, TRACK_UID, 1);
        put_uint(&mut entry, TRACK_TYPE, 1);
        put_uint(&mut entry, FLAG_LACING, 0);
        put_bytes(&mut entry, CODEC_ID, b"V_VP9");
        let frame_ns = (1_000_000_000u64 * fps_den as u64 + fps_num as u64 / 2) / fps_num as u64;
        put_uint(&mut entry, DEFAULT_DURATION, frame_ns);
        put_master(&mut entry, VIDEO, &video);
        let mut tracks = vec![];
        put_master(&mut tracks, TRACK_ENTRY, &entry);
        put_master(&mut out, TRACKS, &tracks);

        w.write_all(&out)?;
        Ok(Self {
            w,
            segment_size_pos,
            segment_data_start,
            seekhead_pos,
            duration_pos,
            info_pos,
            tracks_pos,
            cluster: None,
            cues: vec![],
            frames: 0,
        })
    }

    fn flush_cluster(&mut self) -> io::Result<()> {
        if let Some(c) = self.cluster.take() {
            let pos = self.w.stream_position()?;
            if c.starts_with_key {
                self.cues.push((c.timecode_ms, pos - self.segment_data_start));
            }
            let mut head = vec![];
            put_id(&mut head, CLUSTER);
            put_size(&mut head, c.body.len() as u64);
            self.w.write_all(&head)?;
            self.w.write_all(&c.body)?;
        }
        Ok(())
    }

    /// Append one compressed frame with an absolute timestamp in milliseconds.
    pub fn write_frame(&mut self, data: &[u8], timestamp_ms: u64, keyframe: bool) -> io::Result<()> {
        let need_new = match &self.cluster {
            None => true,
            Some(c) => {
                keyframe
                    || timestamp_ms < c.timecode_ms
                    || timestamp_ms - c.timecode_ms >= MAX_CLUSTER_MS
                    || timestamp_ms - c.timecode_ms > i16::MAX as u64
            }
        };
        if need_new {
            self.flush_cluster()?;
            let mut body = vec![];
            put_uint(&mut body, TIMECODE, timestamp_ms);
            self.cluster = Some(Cluster { timecode_ms: timestamp_ms, body, starts_with_key: keyframe });
        }
        let c = self.cluster.as_mut().unwrap();
        let rel = (timestamp_ms - c.timecode_ms) as i16;
        put_id(&mut c.body, SIMPLE_BLOCK);
        put_size(&mut c.body, 4 + data.len() as u64);
        c.body.push(0x81); // track number 1 as a 1-byte vint
        c.body.extend_from_slice(&rel.to_be_bytes());
        c.body.push(if keyframe { 0x80 } else { 0x00 });
        c.body.extend_from_slice(data);
        self.frames += 1;
        Ok(())
    }

    pub fn frames_written(&self) -> u64 {
        self.frames
    }

    /// Write Cues, SeekHead, Duration and the Segment size. Returns the inner writer.
    pub fn finish(mut self, duration_ms: f64) -> io::Result<W> {
        self.flush_cluster()?;

        let cues_pos = self.w.stream_position()?;
        let mut cues = vec![];
        for &(t, pos) in &self.cues {
            let mut tp = vec![];
            put_uint(&mut tp, CUE_TRACK, 1);
            put_uint(&mut tp, CUE_CLUSTER_POSITION, pos);
            let mut cp = vec![];
            put_uint(&mut cp, CUE_TIME, t);
            put_master(&mut cp, CUE_TRACK_POSITIONS, &tp);
            put_master(&mut cues, CUE_POINT, &cp);
        }
        let mut cues_el = vec![];
        put_master(&mut cues_el, CUES, &cues);
        self.w.write_all(&cues_el)?;
        let end = self.w.stream_position()?;

        // SeekHead into the reserved space.
        let mut seeks = vec![];
        for (id, pos) in [(INFO, self.info_pos), (TRACKS, self.tracks_pos), (CUES, cues_pos)] {
            let mut s = vec![];
            put_bytes(&mut s, SEEK_ID, &id_bytes(id));
            put_uint8(&mut s, SEEK_POSITION, pos - self.segment_data_start);
            put_master(&mut seeks, SEEK, &s);
        }
        let mut sh = vec![];
        put_master(&mut sh, SEEK_HEAD, &seeks);
        assert!(sh.len() + 2 <= SEEKHEAD_RESERVED);
        let rest = SEEKHEAD_RESERVED - sh.len();
        sh.extend_from_slice(&void(rest));
        self.w.seek(SeekFrom::Start(self.seekhead_pos))?;
        self.w.write_all(&sh)?;

        // Duration
        self.w.seek(SeekFrom::Start(self.duration_pos))?;
        self.w.write_all(&duration_ms.to_be_bytes())?;

        // Segment size
        let mut size = vec![];
        put_size_len(&mut size, end - self.segment_data_start, 8);
        self.w.seek(SeekFrom::Start(self.segment_size_pos))?;
        self.w.write_all(&size)?;

        self.w.seek(SeekFrom::Start(end))?;
        self.w.flush()?;
        Ok(self.w)
    }
}

/// A tiny EBML reader, used by tests and by `probe()` for sanity checks.
pub mod read {
    #[derive(Debug, Clone)]
    pub struct Element {
        pub id: u32,
        pub data_start: usize,
        pub size: usize,
    }

    fn vint(buf: &[u8], pos: usize, keep_marker: bool) -> Option<(u64, usize)> {
        let first = *buf.get(pos)?;
        let len = first.leading_zeros() as usize + 1;
        if len > 8 || pos + len > buf.len() {
            return None;
        }
        let mut v = if keep_marker { first as u64 } else { (first as u64) & ((1u64 << (8 - len)) - 1) };
        for i in 1..len {
            v = (v << 8) | buf[pos + i] as u64;
        }
        Some((v, len))
    }

    /// Parse the children of `buf[start..end]`.
    pub fn children(buf: &[u8], start: usize, end: usize) -> Option<Vec<Element>> {
        let mut out = vec![];
        let mut pos = start;
        while pos < end {
            let (id, il) = vint(buf, pos, true)?;
            let (size, sl) = vint(buf, pos + il, false)?;
            let data_start = pos + il + sl;
            let size = usize::try_from(size).ok()?;
            let data_end = data_start.checked_add(size)?;
            if data_end > end || end > buf.len() {
                return None;
            }
            out.push(Element { id: id as u32, data_start, size });
            pos = data_start + size;
        }
        Some(out)
    }

    /// Unsigned integer element (at most 8 bytes); `None` if it is longer.
    pub fn uint(buf: &[u8], e: &Element) -> Option<u64> {
        if e.size > 8 {
            return None;
        }
        Some(buf[e.data_start..e.data_start + e.size].iter().fold(0, |a, &b| (a << 8) | b as u64))
    }

    /// Float element: 4 or 8 bytes (or 0 = 0.0) per the EBML spec.
    pub fn float(buf: &[u8], e: &Element) -> Option<f64> {
        let b = &buf[e.data_start..e.data_start + e.size];
        match e.size {
            0 => Some(0.0),
            4 => Some(f32::from_be_bytes(b.try_into().ok()?) as f64),
            8 => Some(f64::from_be_bytes(b.try_into().ok()?)),
            _ => None,
        }
    }
}

/// Summary of a WebM file, as parsed by our reader.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub codec: String,
    pub width: u64,
    pub height: u64,
    pub frames: u64,
    pub keyframes: u64,
    pub duration_ms: f64,
}

pub fn probe(buf: &[u8]) -> Option<Probe> {
    use read::*;
    let top = children(buf, 0, buf.len())?;
    let seg = top.iter().find(|e| e.id == SEGMENT)?;
    let mut p = Probe { codec: String::new(), width: 0, height: 0, frames: 0, keyframes: 0, duration_ms: 0.0 };
    for e in children(buf, seg.data_start, seg.data_start + seg.size)? {
        match e.id {
            INFO => {
                for c in children(buf, e.data_start, e.data_start + e.size)? {
                    if c.id == DURATION {
                        p.duration_ms = float(buf, &c)?;
                    }
                }
            }
            TRACKS => {
                for te in children(buf, e.data_start, e.data_start + e.size)? {
                    for c in children(buf, te.data_start, te.data_start + te.size)? {
                        match c.id {
                            CODEC_ID => {
                                p.codec = String::from_utf8_lossy(&buf[c.data_start..c.data_start + c.size]).into()
                            }
                            VIDEO => {
                                for v in children(buf, c.data_start, c.data_start + c.size)? {
                                    match v.id {
                                        PIXEL_WIDTH => p.width = uint(buf, &v)?,
                                        PIXEL_HEIGHT => p.height = uint(buf, &v)?,
                                        _ => {}
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            CLUSTER => {
                for b in children(buf, e.data_start, e.data_start + e.size)? {
                    if b.id == SIMPLE_BLOCK {
                        // track number (vint) + 2-byte timecode + flags; we only write 1-byte track numbers.
                        if b.size < 4 {
                            return None;
                        }
                        p.frames += 1;
                        if buf[b.data_start + 3] & 0x80 != 0 {
                            p.keyframes += 1;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn size_encoding() {
        let mut b = vec![];
        put_size(&mut b, 0);
        assert_eq!(b, [0x80]);
        b.clear();
        put_size(&mut b, 126);
        assert_eq!(b, [0xFE]);
        b.clear();
        put_size(&mut b, 127); // 0x7F would be "unknown" in 1 byte
        assert_eq!(b, [0x40, 0x7F]);
        b.clear();
        put_size_len(&mut b, 5, 8);
        assert_eq!(b, [0x01, 0, 0, 0, 0, 0, 0, 5]);
    }

    #[test]
    fn void_lengths() {
        for n in [2usize, 3, 50, 128, 129, 200] {
            let v = void(n);
            assert_eq!(v.len(), n);
            let els = read::children(&v, 0, v.len()).unwrap();
            assert_eq!(els.len(), 1);
            assert_eq!(els[0].id, VOID);
        }
    }

    #[test]
    fn roundtrip_structure() {
        let mut w = WebmWriter::new(Cursor::new(vec![]), 320, 240, 30, 1, "test").unwrap();
        for k in 0..200u64 {
            let ts = k * 1000 / 30;
            w.write_frame(&[0xAA; 37], ts, k % 90 == 0).unwrap();
        }
        let buf = w.finish(200.0 * 1000.0 / 30.0).unwrap().into_inner();
        let p = probe(&buf).expect("parse");
        assert_eq!(p.codec, "V_VP9");
        assert_eq!((p.width, p.height), (320, 240));
        assert_eq!(p.frames, 200);
        assert_eq!(p.keyframes, 3);
        assert!((p.duration_ms - 6666.67).abs() < 0.1);

        // Segment size is known and covers the whole file.
        let top = read::children(&buf, 0, buf.len()).unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(top[1].data_start + top[1].size, buf.len());

        // SeekHead points at real elements.
        let seg = &top[1];
        let kids = read::children(&buf, seg.data_start, seg.data_start + seg.size).unwrap();
        assert_eq!(kids[0].id, SEEK_HEAD);
        let ids: Vec<u32> = kids.iter().map(|e| e.id).collect();
        assert!(ids.contains(&CUES) && ids.contains(&INFO) && ids.contains(&TRACKS));
    }

    #[test]
    fn probe_rejects_malformed_input_without_panicking() {
        let mut w = WebmWriter::new(Cursor::new(vec![]), 64, 48, 30, 1, "test").unwrap();
        for k in 0..40u64 {
            w.write_frame(&[0x55; 9], k * 33, k % 10 == 0).unwrap();
        }
        let good = w.finish(40.0 * 33.0).unwrap().into_inner();
        assert!(probe(&good).is_some());

        // Every truncation and a sweep of single-byte corruptions must return
        // Some/None, never panic.
        for len in 0..good.len() {
            let _ = probe(&good[..len]);
        }
        for i in 0..good.len() {
            for v in [0x00u8, 0x01, 0x7F, 0x80, 0xFF] {
                let mut bad = good.clone();
                bad[i] = v;
                let _ = probe(&bad);
            }
        }
        // Huge declared sizes (8-byte vint near u64::MAX >> 8).
        let _ = probe(&[0x1A, 0x45, 0xDF, 0xA3, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE]);
        let _ = probe(&[0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    }
}
