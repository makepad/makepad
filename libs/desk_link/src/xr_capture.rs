//! Desk Link XR capture: what a headset's cameras see, sent up while the desktop app asks for it
//! ([`crate::xr::XrServerMsg::Capture`]).
//!
//! - [`CaptureRecord::Cameras`] once, and again whenever the cameras change: each camera's
//!   calibration as the camera reports it.
//! - [`CaptureRecord::Keyframe`]s: the cameras' pictures, lossless, each with the head pose at
//!   the middle of its exposure, and the newest environment depth image with its own views.
//! - [`CaptureRecord::Preview`]: a low-rate stream of one camera (HEVC) for a live view.
//! - [`CaptureRecord::Status`]: what the capture is doing and, when it cannot run, why.
//!
//! A record goes up as [`crate::xr::XrClientMsg::Capture`] chunks of at most [`CHUNK_BYTES`],
//! in the link's gaps (`XrLink::send_bulk`: a pose never waits behind more than one chunk), and
//! the server puts them back together with a [`CaptureAssembler`]. The pictures are the
//! wearer's: the records carry them to the app that asked for them and nowhere else.
//!
//! Frames and units: poses are in the headset's tracking space (OpenXR's local space; the head
//! is its VIEW space), orientations `x, y, z, w`, positions in metres. Times are the headset's
//! clock in ns (OpenXR's time base). A camera's calibration is Android Camera2's: the
//! intrinsics in the pixels of its pre-correction array, the lens pose relative to its
//! `pose_reference`.

use crate::{In, Out};
use std::io;

/// The most a [`crate::xr::XrClientMsg::Capture`] chunk carries.
pub const CHUNK_BYTES: usize = 16 << 10;
/// A record past this many bytes is a protocol error (a keyframe of two 1280x1280 pictures is
/// about 5 MB raw).
pub const MAX_RECORD_BYTES: usize = 256 << 20;
/// A plane decoding past this many bytes is refused (a 4096x4096 picture of 16-bit samples is
/// 32 MB): the decoder's memory is bounded by this, never by what a peer claims.
pub const MAX_PLANE_BYTES: usize = 64 << 20;

/// [`CapturePlane::codec`].
pub mod plane_codec {
    /// The samples as they are.
    pub const RAW: u8 = 0;
    /// Lossless: each byte less the byte `bytes_per_sample` before it in its row (wrapping; a
    /// row's first sample less the first of the row above, the first row's as it is), then all
    /// of it deflated (RFC 1951, no zlib header).
    pub const DELTA_DEFLATE: u8 = 1;
}

/// [`CaptureRecord::Status`] states.
pub mod capture_state {
    /// Not capturing (asked off).
    pub const OFF: u8 = 0;
    /// Asked on: opening the cameras, waiting for permission, exposure settling.
    pub const STARTING: u8 = 1;
    pub const CAPTURING: u8 = 2;
    /// It cannot run (no permission, no cameras): the text says why.
    pub const FAILED: u8 = 3;
}

/// One plane of samples, rows packed tight (`width * bytes_per_sample` bytes each).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CapturePlane {
    pub width: u32,
    pub height: u32,
    pub bytes_per_sample: u8,
    /// [`plane_codec`].
    pub codec: u8,
    pub data: Vec<u8>,
}

/// A pose: orientation (x, y, z, w) and position (m).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CapturePose {
    pub orientation: [f32; 4],
    pub position: [f32; 3],
}

/// Where a lens sits: relative to `reference` (Camera2's LENS_POSE_REFERENCE: 0 the primary
/// camera, 1 the gyroscope, 2 undefined, 3 the vehicle).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CaptureLensPose {
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
    pub reference: u8,
}

/// A camera as it reports itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptureCamera {
    /// Its place on the headset: 0 the leftmost, 1 the rightmost, as the wearer sees.
    pub position: u8,
    /// The pictures' size.
    pub size: [u32; 2],
    /// fx, fy, cx, cy, skew in the pixels of `array` (None: the camera gives none).
    pub intrinsics: Option<[f32; 5]>,
    /// The pixel array the intrinsics are in (the pre-correction active array) and the active
    /// array the pictures show (left, top, width, height).
    pub array: [i32; 4],
    pub active: [i32; 4],
    /// Radial k1..k3 and tangential p1, p2 (Camera2's LENS_DISTORTION; None: none given).
    pub distortion: Option<[f32; 5]>,
    pub lens_pose: Option<CaptureLensPose>,
}

/// One camera's picture of a keyframe.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptureView {
    /// The camera (its index in the last [`CaptureRecord::Cameras`]).
    pub camera: u8,
    /// The middle of the exposure, and the head there (interpolated by the runtime, never the
    /// newest pose).
    pub time_ns: u64,
    pub head: CapturePose,
    /// The head's motion then (m/s, rad/s in tracking space): what a rolling shutter's rows
    /// need.
    pub velocity: [f32; 3],
    pub angular_velocity: [f32; 3],
    /// The exposure, the rolling shutter's first-to-last row time (0: not reported) and the
    /// sensitivity (ISO; 0: not reported).
    pub exposure_ns: u32,
    pub rolling_skew_ns: u32,
    pub sensitivity: u32,
    /// Y, U, V (I420: U and V at half size).
    pub planes: Vec<CapturePlane>,
}

/// One eye of the environment depth: the view it was made from (tracking space) and its fov
/// (tangents' angles: left, right, up, down; radians).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CaptureEye {
    pub pose: CapturePose,
    pub fov: [f32; 4],
}

/// The headset's environment depth image nearest a keyframe, as the runtime gave it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptureDepth {
    /// The display time of the frame that acquired it.
    pub time_ns: u64,
    /// The runtime's depth range (m) its 16-bit values map onto.
    pub near: f32,
    pub far: f32,
    pub views: [CaptureEye; 2],
    /// Its layers one after another (eye 0's first), 16-bit samples, `height` = rows of all of
    /// them.
    pub layers: u32,
    pub texels: CapturePlane,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptureKeyframe {
    /// Counts up from 0 within one capture.
    pub index: u32,
    pub views: Vec<CaptureView>,
    pub depth: Option<CaptureDepth>,
}

/// What the desktop asks of the headset's capture (`XrServerMsg::Capture`): a keyframe each time
/// the head turned `step_deg` or moved `step_m` since the last one, a preview of `preview_hz`
/// pictures a second (0: none).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptureAsk {
    pub step_deg: f32,
    pub step_m: f32,
    pub preview_hz: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CaptureRecord {
    Cameras(Vec<CaptureCamera>),
    Keyframe(CaptureKeyframe),
    /// One access unit of camera `camera`'s preview (`codec`: [`crate::xr::CODEC_HEVC`] or
    /// [`crate::xr::CODEC_H264`]).
    Preview { camera: u8, time_ns: u64, key: bool, codec: u8, size: [u32; 2], data: Vec<u8> },
    /// [`capture_state`] and its words.
    Status { state: u8, text: String },
}

mod kind {
    pub const CAMERAS: u8 = 1;
    pub const KEYFRAME: u8 = 2;
    pub const PREVIEW: u8 = 3;
    pub const STATUS: u8 = 4;
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("capture record: {what}"))
}

impl Out {
    fn f32s(&mut self, v: &[f32]) -> &mut Self {
        for x in v {
            self.f32(*x);
        }
        self
    }
    fn pose(&mut self, p: &CapturePose) -> &mut Self {
        self.f32s(&p.orientation).f32s(&p.position)
    }
    fn plane(&mut self, p: &CapturePlane) -> &mut Self {
        self.u32(p.width).u32(p.height).u8(p.bytes_per_sample).u8(p.codec).bytes(&p.data)
    }
}

impl<'a> In<'a> {
    fn f32n<const N: usize>(&mut self) -> io::Result<[f32; N]> {
        let mut v = [0.0; N];
        for x in &mut v {
            *x = self.f32()?;
        }
        Ok(v)
    }
    fn pose(&mut self) -> io::Result<CapturePose> {
        Ok(CapturePose { orientation: self.f32n()?, position: self.f32n()? })
    }
    /// A presence byte, then the values (there either way).
    fn opt5(&mut self) -> io::Result<Option<[f32; 5]>> {
        let has = self.bool()?;
        let v = self.f32n()?;
        Ok(has.then_some(v))
    }
    fn plane(&mut self) -> io::Result<CapturePlane> {
        Ok(CapturePlane { width: self.u32()?, height: self.u32()?, bytes_per_sample: self.u8()?, codec: self.u8()?, data: self.bytes()?.to_vec() })
    }
}

impl CaptureRecord {
    /// The record's bytes (`kind u8 | length u32 LE | fields`), cut into chunks by
    /// [`capture_chunks`].
    pub fn encode(&self) -> Vec<u8> {
        match self {
            CaptureRecord::Cameras(cameras) => {
                let mut o = Out::new(kind::CAMERAS);
                o.u32(cameras.len() as u32);
                for c in cameras {
                    o.u8(c.position).u32(c.size[0]).u32(c.size[1]);
                    o.u8(c.intrinsics.is_some() as u8).f32s(&c.intrinsics.unwrap_or_default());
                    for v in c.array.iter().chain(c.active.iter()) {
                        o.i32(*v);
                    }
                    o.u8(c.distortion.is_some() as u8).f32s(&c.distortion.unwrap_or_default());
                    let lens = c.lens_pose.unwrap_or_default();
                    o.u8(c.lens_pose.is_some() as u8).f32s(&lens.rotation).f32s(&lens.translation).u8(lens.reference);
                }
                o.done()
            }
            CaptureRecord::Keyframe(k) => {
                let mut o = Out::new(kind::KEYFRAME);
                o.u32(k.index).u32(k.views.len() as u32);
                for v in &k.views {
                    o.u8(v.camera).u64(v.time_ns).pose(&v.head).f32s(&v.velocity).f32s(&v.angular_velocity);
                    o.u32(v.exposure_ns).u32(v.rolling_skew_ns).u32(v.sensitivity).u32(v.planes.len() as u32);
                    for p in &v.planes {
                        o.plane(p);
                    }
                }
                match &k.depth {
                    Some(d) => {
                        o.u8(1).u64(d.time_ns).f32(d.near).f32(d.far);
                        for e in &d.views {
                            o.pose(&e.pose).f32s(&e.fov);
                        }
                        o.u32(d.layers).plane(&d.texels);
                    }
                    None => {
                        o.u8(0);
                    }
                }
                o.done()
            }
            CaptureRecord::Preview { camera, time_ns, key, codec, size, data } => {
                let mut o = Out::new(kind::PREVIEW);
                o.u8(*camera).u64(*time_ns).u8(*key as u8).u8(*codec).u32(size[0]).u32(size[1]).bytes(data);
                o.done()
            }
            CaptureRecord::Status { state, text } => {
                let mut o = Out::new(kind::STATUS);
                o.u8(*state).str(text);
                o.done()
            }
        }
    }

    pub fn decode(bytes: &[u8]) -> io::Result<CaptureRecord> {
        if bytes.len() < 5 {
            return Err(bad("too short"));
        }
        let len = u32::from_le_bytes(bytes[1..5].try_into().unwrap()) as usize;
        if bytes.len() != 5 + len {
            return Err(bad("length does not match"));
        }
        let mut r = In { b: &bytes[5..] };
        Ok(match bytes[0] {
            kind::CAMERAS => {
                let n = r.u32()? as usize;
                let mut cameras = Vec::with_capacity(n.min(16));
                for _ in 0..n {
                    let (position, size) = (r.u8()?, [r.u32()?, r.u32()?]);
                    let intrinsics = r.opt5()?;
                    let array = [r.i32()?, r.i32()?, r.i32()?, r.i32()?];
                    let active = [r.i32()?, r.i32()?, r.i32()?, r.i32()?];
                    let distortion = r.opt5()?;
                    let has = r.bool()?;
                    let lens = CaptureLensPose { rotation: r.f32n()?, translation: r.f32n()?, reference: r.u8()? };
                    cameras.push(CaptureCamera { position, size, intrinsics, array, active, distortion, lens_pose: has.then_some(lens) });
                }
                CaptureRecord::Cameras(cameras)
            }
            kind::KEYFRAME => {
                let (index, n) = (r.u32()?, r.u32()? as usize);
                let mut views = Vec::with_capacity(n.min(16));
                for _ in 0..n {
                    let mut v = CaptureView { camera: r.u8()?, time_ns: r.u64()?, head: r.pose()?, velocity: r.f32n()?, angular_velocity: r.f32n()?, ..Default::default() };
                    v.exposure_ns = r.u32()?;
                    v.rolling_skew_ns = r.u32()?;
                    v.sensitivity = r.u32()?;
                    let planes = r.u32()? as usize;
                    for _ in 0..planes.min(4) {
                        v.planes.push(r.plane()?);
                    }
                    if planes > 4 {
                        return Err(bad("more than 4 planes"));
                    }
                    views.push(v);
                }
                let depth = if r.bool()? {
                    let (time_ns, near, far) = (r.u64()?, r.f32()?, r.f32()?);
                    let mut eyes = [CaptureEye::default(); 2];
                    for e in &mut eyes {
                        *e = CaptureEye { pose: r.pose()?, fov: r.f32n()? };
                    }
                    Some(CaptureDepth { time_ns, near, far, views: eyes, layers: r.u32()?, texels: r.plane()? })
                } else {
                    None
                };
                CaptureRecord::Keyframe(CaptureKeyframe { index, views, depth })
            }
            kind::PREVIEW => CaptureRecord::Preview { camera: r.u8()?, time_ns: r.u64()?, key: r.bool()?, codec: r.u8()?, size: [r.u32()?, r.u32()?], data: r.bytes()?.to_vec() },
            kind::STATUS => CaptureRecord::Status { state: r.u8()?, text: r.str()? },
            k => return Err(bad(&format!("unknown kind {k}"))),
        })
    }
}

/// A record's chunks, in order, as the messages that carry them (`seq`: the record's number,
/// counting up per link).
pub fn capture_chunks(seq: u32, record: &[u8]) -> Vec<crate::xr::XrClientMsg> {
    let parts = record.len().div_ceil(CHUNK_BYTES).max(1) as u32;
    (0..parts)
        .map(|part| {
            let at = part as usize * CHUNK_BYTES;
            crate::xr::XrClientMsg::Capture { seq, part, parts, data: record[at..(at + CHUNK_BYTES).min(record.len())].to_vec() }
        })
        .collect()
}

/// The server's side: chunks in, whole records out. Chunks come in order (one stream); a chunk
/// of another record before the current one is whole means that record was cut off (the link
/// ended mid-record: a new link starts a new assembler), so it is dropped and the drop counted.
#[derive(Default)]
pub struct CaptureAssembler {
    seq: u32,
    parts: u32,
    next: u32,
    buf: Vec<u8>,
    /// Records dropped unfinished.
    pub dropped: u64,
}

impl CaptureAssembler {
    /// One chunk: `Some` once a record is whole (or broken).
    pub fn push(&mut self, seq: u32, part: u32, parts: u32, data: &[u8]) -> Option<io::Result<CaptureRecord>> {
        // (a link message may be far bigger than a chunk: the record's memory is bounded by
        // the chunk size and count, never by what a peer sends)
        if data.len() > CHUNK_BYTES {
            return Some(Err(self.broken(&format!("a chunk of {} bytes", data.len()))));
        }
        if part == 0 {
            if self.next != 0 {
                self.dropped += 1;
            }
            if parts as usize > MAX_RECORD_BYTES / CHUNK_BYTES {
                self.next = 0;
                self.buf = Vec::new();
                return Some(Err(bad(&format!("{parts} chunks"))));
            }
            self.seq = seq;
            self.parts = parts;
            self.next = 0;
            self.buf.clear();
        } else if seq != self.seq || part != self.next || parts != self.parts {
            // (the middle of a record whose start was never seen, or a skipped chunk)
            if self.next != 0 {
                self.dropped += 1;
            }
            self.next = 0;
            self.buf = Vec::new();
            return None;
        }
        if self.buf.len() + data.len() > MAX_RECORD_BYTES {
            return Some(Err(self.broken("a record over the size limit")));
        }
        self.buf.extend_from_slice(data);
        self.next = part + 1;
        if self.next < self.parts {
            return None;
        }
        self.next = 0;
        // (the record's camera planes are not kept past it)
        Some(CaptureRecord::decode(&std::mem::take(&mut self.buf)))
    }

    /// The record being put together is dropped (counted when it had begun): `why` as an error.
    fn broken(&mut self, why: &str) -> io::Error {
        if self.next != 0 {
            self.dropped += 1;
        }
        self.next = 0;
        self.buf = Vec::new();
        bad(why)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xr::XrClientMsg;

    fn plane(width: u32, height: u32, seed: u8) -> CapturePlane {
        CapturePlane { width, height, bytes_per_sample: 1, codec: plane_codec::RAW, data: (0..width * height).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect() }
    }

    fn records() -> Vec<CaptureRecord> {
        let camera = |position| CaptureCamera {
            position,
            size: [64, 48],
            intrinsics: Some([50.0, 50.5, 32.0, 24.0, 0.0]),
            array: [0, 0, 64, 48],
            active: [0, 0, 64, 48],
            distortion: (position == 0).then_some([0.1, -0.02, 0.003, 0.0, 0.0]),
            lens_pose: (position == 1).then_some(CaptureLensPose { rotation: [0.0, 0.0, 0.0, 1.0], translation: [0.03, -0.01, 0.0], reference: 1 }),
        };
        let head = CapturePose { orientation: [0.0, 0.1, 0.0, 0.995], position: [0.1, 1.5, -0.2] };
        let view = |camera: u8| CaptureView { camera, time_ns: 1_000_000 + camera as u64, head, velocity: [0.01, 0.0, 0.0], angular_velocity: [0.0, 0.2, 0.0], exposure_ns: 4_000_000, rolling_skew_ns: 0, sensitivity: 400, planes: vec![plane(64, 48, camera), plane(32, 24, 1), plane(32, 24, 2)] };
        let depth = CaptureDepth { time_ns: 999_000, near: 0.1, far: f32::INFINITY, views: [CaptureEye { pose: head, fov: [-0.9, 0.8, 0.85, -0.9] }; 2], layers: 2, texels: CapturePlane { width: 8, height: 12, bytes_per_sample: 2, codec: plane_codec::RAW, data: vec![7; 8 * 12 * 2] } };
        vec![
            CaptureRecord::Cameras(vec![camera(0), camera(1), CaptureCamera::default()]),
            CaptureRecord::Keyframe(CaptureKeyframe { index: 4, views: vec![view(0), view(1)], depth: Some(depth) }),
            CaptureRecord::Keyframe(CaptureKeyframe { index: 5, views: vec![view(1)], depth: None }),
            CaptureRecord::Preview { camera: 0, time_ns: 5, key: true, codec: crate::xr::CODEC_HEVC, size: [640, 640], data: vec![0, 0, 1, 0x40] },
            CaptureRecord::Status { state: capture_state::FAILED, text: "no camera permission".into() },
        ]
    }

    #[test]
    fn capture_records_round_trip() {
        for r in records() {
            assert_eq!(CaptureRecord::decode(&r.encode()).unwrap(), r);
        }
        assert!(CaptureRecord::decode(&[9, 0, 0, 0, 0]).is_err(), "unknown kind");
        let mut cut = records()[1].encode();
        cut.pop();
        assert!(CaptureRecord::decode(&cut).is_err(), "length mismatch");
    }

    /// Records cut into chunks, through the link's own framing, come back whole; one cut off
    /// mid-way (a new record's start before its end) is dropped and counted, never mixed in.
    #[test]
    fn capture_chunks_reassemble() {
        let mut big = records()[1].clone();
        if let CaptureRecord::Keyframe(k) = &mut big {
            k.views[0].planes[0] = plane(1280, 1280, 3);
        }
        let all = [records(), vec![big.clone()]].concat();
        let mut asm = CaptureAssembler::default();
        let mut out = Vec::new();
        for (seq, r) in all.iter().enumerate() {
            let chunks = capture_chunks(seq as u32, &r.encode());
            assert!(chunks.iter().all(|c| matches!(c, XrClientMsg::Capture { data, .. } if data.len() <= CHUNK_BYTES)));
            for c in chunks {
                let mut inbox = crate::Inbox::default();
                inbox.feed(&c.encode());
                let (ty, p) = inbox.next().unwrap().unwrap();
                let XrClientMsg::Capture { seq, part, parts, data } = XrClientMsg::decode(ty, &p).unwrap() else { panic!("not a capture chunk") };
                if let Some(r) = asm.push(seq, part, parts, &data) {
                    out.push(r.unwrap());
                }
            }
        }
        assert_eq!(out, all);
        assert_eq!(asm.dropped, 0);
        // the big record cut off after 3 chunks, then the next one whole
        let cut = capture_chunks(10, &big.encode());
        assert!(cut.len() > 3);
        let next = capture_chunks(11, &records()[4].encode());
        let mut got = Vec::new();
        for c in cut.iter().take(3).chain(next.iter()).chain(cut.iter().skip(5)) {
            let XrClientMsg::Capture { seq, part, parts, data } = c else { unreachable!() };
            if let Some(r) = asm.push(*seq, *part, *parts, data) {
                got.push(r.unwrap());
            }
        }
        assert_eq!(got, vec![records()[4].clone()]);
        assert_eq!(asm.dropped, 1);
        // a record claiming more than the bound is refused at its first chunk
        assert!(asm.push(12, 0, (MAX_RECORD_BYTES / CHUNK_BYTES) as u32 + 1, &[0; 8]).unwrap().is_err());
    }

    #[test]
    fn capture_assembler_bounds_its_memory() {
        // a chunk bigger than a chunk (a link message may be megabytes) is refused, and the
        // record it was part of dropped: the buffer never holds more than the record bound
        let mut asm = CaptureAssembler::default();
        assert!(asm.push(1, 0, 4, &[0; CHUNK_BYTES]).is_none());
        assert!(asm.push(1, 1, 4, &vec![0; CHUNK_BYTES + 1]).unwrap().is_err());
        assert_eq!((asm.dropped, asm.buf.capacity()), (1, 0));
        assert!(asm.push(2, 0, 1, &vec![0; 16 << 20]).unwrap().is_err());
        assert_eq!(asm.buf.capacity(), 0);
        // the next record still assembles
        let r = records()[4].clone();
        let mut out = None;
        for c in capture_chunks(4, &r.encode()) {
            let XrClientMsg::Capture { seq, part, parts, data } = c else { unreachable!() };
            out = asm.push(seq, part, parts, &data).or(out);
        }
        assert_eq!(out.map(|r| r.unwrap()), Some(r));
        // a whole record's bytes (its camera planes) are not kept once it is decoded
        assert_eq!(asm.buf.capacity(), 0);
    }

    #[test]
    fn capture_assembler_keeps_no_record_bytes_after_a_record() {
        // a keyframe of several chunks (its camera planes)
        let mut r = records()[1].clone();
        if let CaptureRecord::Keyframe(k) = &mut r {
            k.views[0].planes[0] = plane(256, 256, 3);
        }
        let chunks: Vec<_> = capture_chunks(9, &r.encode()).into_iter().map(|c| {
            let XrClientMsg::Capture { seq, part, parts, data } = c else { unreachable!() };
            (seq, part, parts, data)
        }).collect();
        assert!(chunks.len() > 1);
        let mut asm = CaptureAssembler::default();
        // cut off mid-record (a chunk of another record): the part held so far goes
        let (seq, part, parts, data) = &chunks[0];
        assert!(asm.push(*seq, *part, *parts, data).is_none());
        assert!(asm.buf.capacity() > 0);
        assert!(asm.push(10, 1, 2, &[0; 4]).is_none());
        assert_eq!((asm.dropped, asm.buf.capacity()), (1, 0));
        // a whole record: decoded, and nothing of it left behind
        let mut out = None;
        for (seq, part, parts, data) in &chunks {
            out = asm.push(*seq, *part, *parts, data).or(out);
        }
        assert_eq!(out.map(|r| r.unwrap()), Some(r));
        assert_eq!(asm.buf.capacity(), 0);
    }
}
