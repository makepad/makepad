//! Desk Link XR: remote rendering for a headset. A desktop app renders its XR view with the
//! headset's predicted eyes and streams the stereo picture back; the headset shows it with the
//! eyes it was rendered with (its compositor reprojects to the current pose). Same framing, TLS
//! and fleet identity as the desktop protocol (`type u8 | length u32 | payload`), its own
//! messages and port: an XR session is not a desktop session (no screens, no desktop input; the
//! headset's controllers and hands reach the XR app only as pose data).
//!
//! The app's game audio goes down as datagrams of its own ([`audio_datagram`]) from the
//! session's pose socket to the address the poses come from: raw 16-bit stereo PCM at 48 kHz,
//! 5 ms per datagram, each with the time it is due on the desktop's clock (the headset lines it
//! up with the pictures through the link's ping clock).
//!
//! Headset → desktop: `Hello` (eye size, refresh, the decoder's codecs), then one `Pose` per
//! display frame (the `XrFrame` the app draws with: head, both eyes, controllers, hands; a
//! sequence number and the headset's sample and display times), `State` (visible / focused),
//! `Stats` (decode and motion-to-photon per shown picture), `Ping`, `Keyframe`.
//! Desktop → headset: `Welcome` (the picture size, codec and tiles), then per rendered frame one
//! `Video` per tile (the pose's sequence number, the eyes the picture was rendered with, the
//! server-side stage durations, one Annex-B access unit), `Pong`, `Bye`, `Note`.
//!
//! The picture: per eye a column `eye_w` wide and `eye_h * 3/2` tall: the eye's colour
//! (premultiplied on black) on top; below it a band `eye_h / 2` tall whose left half is the eye's
//! coverage (alpha) and right half its inverse depth (`1 / z` scaled, see [`DEPTH_NEAR_M`]; 0 =
//! nothing drawn), both at half resolution in the luma, neutral chroma. Left eye's column first.
//! With two tiles each eye's column is a stream of its own (one encoder each, the GPU's two
//! encode engines); with one tile both columns side by side are one stream.

use crate::{In, Out};
use std::io;

pub const XR_VERSION: u16 = 7;
/// The band's depth: luma `v` (1..255) is `z = DEPTH_NEAR_M * 255 / v` metres (inverse depth,
/// finest near the eye); 0 = no depth (nothing drawn there). 10 cm: things held up to the face
/// keep their depth (at 25 cm everything nearer read as 25 cm, and the room's occlusion hid
/// what was held in front of a hand); a step is ~4 cm at 1 m, ~16 cm at 2 m.
pub const DEPTH_NEAR_M: f32 = 0.1;

/// One eye a picture was rendered with: orientation (x, y, z, w), position (m) in the headset's
/// tracking space, fov tangents' angles (left, right, up, down; radians, left and down negative).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct XrRenderEye {
    pub orientation: [f32; 4],
    pub position: [f32; 3],
    pub fov: [f32; 4],
}

/// `Hello::codecs` bits: what the headset decodes.
pub mod codecs {
    pub const H264: u32 = 1;
    pub const HEVC: u32 = 2;
    /// Not a codec: the headset plays `XrServerMsg::Haptic` (a headset without it is never
    /// sent one, so older headsets keep linking).
    pub const HAPTICS: u32 = 1 << 16;
}
/// `Video::codec`.
pub const CODEC_H264: u8 = 0;
pub const CODEC_HEVC: u8 = 1;

#[derive(Clone, Debug, PartialEq)]
pub enum XrClientMsg {
    /// `instance`: the headset's instance id (`crate::xr_pair`): the server keeps it as paired
    /// when the PIN typed on the desktop matched this headset.
    Hello { version: u16, name: String, authorization: String, instance: u64, eye: [u32; 2], refresh_mhz: u32, codecs: u32 },
    /// `frame`: one `XrFrame` (makepad_hal `buffers::Wire`) predicted for `display_ns`.
    Pose { seq: u64, sample_ns: u64, display_ns: u64, frame: Vec<u8> },
    State { visible: bool, focused: bool },
    /// For the picture of pose `seq`: its decode, and pose sample to shown (motion to photon).
    Stats { seq: u64, decode_us: u32, m2p_us: u32 },
    Ping { client_ns: u64 },
    Keyframe,
    /// Bricks of the headset's room field (its surfaces as a signed distance field, in its
    /// tracking space): `voxel_size` (m) and `chunk_edge` (voxels) of the grid, then changed
    /// bricks (an empty payload: the brick is gone). Sent in the link's gaps, never ahead of a
    /// pose ([`link::XrLink::send_bulk`]).
    World { voxel_size: f32, chunk_edge: i32, bricks: Vec<WorldBrick> },
    /// The headset's numbers of its last second ([`report`] names them), for the server's
    /// stats panel.
    Report { values: Vec<f32> },
}

/// `XrClientMsg::Report`: what each value is (index) and its label.
pub mod report {
    pub const DISPLAY: usize = 0;
    pub const SHOWN: usize = 1;
    pub const REPEATS: usize = 2;
    pub const RECEIVED: usize = 3;
    pub const M2P_MS: usize = 4;
    pub const NET_MS: usize = 5;
    pub const DECODE_MS: usize = 6;
    pub const WAIT_MS: usize = 7;
    pub const ENV_AGE_MS: usize = 8;
    pub const UPLINK: usize = 9;
    pub const UPLINK_SD_MS: usize = 10;
    pub const LEAD_MS: usize = 11;
    pub const SKIPPED: usize = 12;
    /// The game audio (`audio_datagram`): due on the desktop to heard (ms: the link, the
    /// jitter buffer, the device's buffer), its parts, the picture's own latency it aims at.
    pub const AUDIO_MS: usize = 13;
    pub const AUDIO_NET_MS: usize = 14;
    pub const AUDIO_BUFFER_MS: usize = 15;
    pub const AUDIO_TARGET_MS: usize = 16;
    pub const AUDIO_DEVICE_MS: usize = 17;
    pub const AUDIO_VIDEO_MS: usize = 18;
    /// Datagrams per second, lost and late ones (this second), dry buffer (underruns) and
    /// concealed frames (this second), the drift nudge (ppm, + faster).
    pub const AUDIO_RECEIVED: usize = 19;
    pub const AUDIO_LOST: usize = 20;
    pub const AUDIO_LATE: usize = 21;
    pub const AUDIO_UNDERRUNS: usize = 22;
    pub const AUDIO_CONCEALED: usize = 23;
    pub const AUDIO_DRIFT_PPM: usize = 24;
    /// The test signal's clicks (`MAKEPAD_XR_REMOTE_AUDIO_TEST`): due to the output buffer
    /// (ms, the device's buffer not included; 0: no click this second).
    pub const AUDIO_CLICK_MS: usize = 25;
    /// The repeats per second by cause: the picture due was not there yet (late: in flight or
    /// decoding), the server never sent it (a newer one came), the headset let it go at its
    /// queue cap, no pose went up for that frame. Then the blank frames per second (a frame
    /// drawn without a picture for an eye) and the lead in display frames.
    /// The headset's battery (percent; -1 not known) and whether it charges (1/0).
    pub const BATTERY_PCT: usize = 32;
    pub const BATTERY_CHARGING: usize = 33;
    pub const REPEAT_LATE: usize = 26;
    pub const REPEAT_SERVER: usize = 27;
    pub const REPEAT_DROPPED: usize = 28;
    pub const REPEAT_NOPOSE: usize = 29;
    pub const BLANK: usize = 30;
    pub const AHEAD: usize = 31;
    /// The headset's own GPU time per frame (the runtime's app GPU time, ms; 0 not measured)
    /// and its eye buffers' size now (percent of their start size): the server decides on them.
    pub const CLIENT_GPU_MS: usize = 34;
    pub const EYE_SCALE_PCT: usize = 35;
    pub const COUNT: usize = 36;
    /// After the numbers: how many display frames the window had, then their codes, 8 to a
    /// value (3 bits each, the first frame lowest: exact in an f32).
    pub const FRAMES: usize = COUNT;
    /// A display frame's code: a new picture, the one before again (repeat), a new one with
    /// pictures skipped to catch up, an odd one (the odd-frame watch: a new picture off its
    /// frame, a jump, not one XR draw), the display having missed a frame before it (late), a
    /// frame without stream content (blank).
    pub const FRAME_NEW: u8 = 1;
    pub const FRAME_REPEAT: u8 = 2;
    pub const FRAME_SKIP: u8 = 3;
    pub const FRAME_ODD: u8 = 4;
    pub const FRAME_LATE: u8 = 5;
    pub const FRAME_BLANK: u8 = 6;
    /// `codes` packed after the numbers (`FRAMES`).
    pub fn pack_frames(values: &mut Vec<f32>, codes: &[u8]) {
        values.truncate(FRAMES);
        values.resize(FRAMES, 0.0);
        values.push(codes.len() as f32);
        for chunk in codes.chunks(8) {
            values.push(chunk.iter().enumerate().fold(0u32, |v, (i, c)| v | ((*c as u32 & 7) << (3 * i))) as f32);
        }
    }
    /// The frame codes of a report (empty from a headset that sends none).
    pub fn frames(values: &[f32]) -> Vec<u8> {
        let Some(&n) = values.get(FRAMES) else { return Vec::new() };
        let n = (n as usize).min(1024);
        (0..n).filter_map(|i| values.get(FRAMES + 1 + i / 8).map(|v| ((*v as u32 >> (3 * (i % 8))) & 7) as u8)).collect()
    }
}

/// One brick of a room field as it travels (see `XrClientMsg::World`).
#[derive(Clone, Debug, PartialEq)]
pub struct WorldBrick {
    pub key: [i32; 3],
    pub version: u32,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum XrServerMsg {
    /// `size`: the whole picture (both eye columns); `tiles`: 1 (one stream) or 2 (a stream per eye).
    /// `instance`: the server's instance id (the headset keeps the pair by it); `session`: this
    /// link's id.
    Welcome { version: u16, server: String, instance: u64, session: u64, size: [u32; 2], codec: u8, tiles: u8, encoder: String },
    Refused { reason: String },
    /// Tile `tile` of the picture of pose `seq` (predicted for the headset's `display_ns`),
    /// rendered with `eyes`; `focus_m`: the depth (m) of what the view centres on, for
    /// reprojection without per-pixel depth. Durations on the desktop (µs), `stages`: [`stage`].
    Video { seq: u64, tile: u8, key: bool, display_ns: u64, eyes: [XrRenderEye; 2], focus_m: f32, stages: [u32; 4], size: [u32; 2], codec: u8, data: Vec<u8> },
    Pong { client_ns: u64, server_ns: u64 },
    Bye { reason: String },
    Note { text: String },
    /// Send the poses as datagrams ([`pose_datagram`]) to `port` of the server's address, each
    /// sealed with `key` (32 bytes): no head-of-line blocking, a lost one is replaced by the next.
    Uplink { port: u16, key: Vec<u8> },
    /// Whether the server's app uses the headset's room field (its physics, particles or field
    /// view): only then does the headset integrate its depth into the field and send bricks
    /// (a whole core of its CPU). Sent after the welcome and on every change; off until then.
    RoomField { wanted: bool },
    /// The display rate the server's app asks of the headset (mHz): the headset switches its
    /// display to it; the server then ends the session and the headset links again at it (the
    /// encoders are made for one rate).
    Refresh { mhz: u32 },
    /// The stream runs (pictures and sound) or is off: off, the headset stops every part of it
    /// (decoders released, no stream pass) and shows the room alone. Sent after the welcome
    /// and on every change.
    Streaming { on: bool },
    /// Mixed reality (the room through the picture's empty parts) or VR (the picture opaque,
    /// no passthrough): the server's app's choice. Sent after the welcome and on every change.
    SeeThrough { on: bool },
    /// The desktop's PIN was changed or cleared: the pair is over (both forget it) and the
    /// server ends the link.
    Unpaired,
    /// The server's settings for the headset's own per-frame mechanisms (the headset executes
    /// them; the policy is the server's): its fixed foveation (0 none .. 3 high), the range its
    /// eye buffers may move in on its GPU time (percent of their start size), hands and
    /// controllers tracked at once, the room's depth hiding the picture (occlusion), the poses
    /// at least `ahead` display frames past the headset's newest. Sent after the welcome and on
    /// every change.
    Headset { foveation: u8, eye_min_pct: u8, eye_max_pct: u8, multimodal: bool, occlusion: bool, ahead: u8 },
    /// A short buzz on a controller (`hand` 0 left, 1 right; `amplitude` 0..1), the app's
    /// `Cx::xr_haptic`. Only to a headset whose `Hello::codecs` has [`codecs::HAPTICS`].
    Haptic { hand: u8, amplitude: f32, duration_us: u32 },
}

/// `Video::stages`: the desktop's durations of a picture (µs).
pub mod stage {
    /// The pose's arrival to the app's frame built (its wait for the frame slot, its CPU work).
    pub const RENDER: usize = 0;
    /// The frame built to its GPU work done (render + NV12 pack).
    pub const GPU: usize = 1;
    /// The encode (the encoder's input to its bitstream).
    pub const ENCODE: usize = 2;
    /// The bitstream to written to the socket.
    pub const SEND: usize = 3;
}

mod tag {
    pub const HELLO: u8 = 1;
    pub const POSE: u8 = 2;
    pub const STATE: u8 = 3;
    pub const STATS: u8 = 4;
    pub const PING: u8 = 5;
    pub const KEYFRAME: u8 = 6;
    pub const WORLD: u8 = 7;
    pub const REPORT: u8 = 8;
    pub const WELCOME: u8 = 100;
    pub const REFUSED: u8 = 101;
    pub const VIDEO: u8 = 102;
    pub const PONG: u8 = 103;
    pub const BYE: u8 = 104;
    pub const NOTE: u8 = 105;
    pub const UPLINK: u8 = 106;
    pub const ROOM_FIELD: u8 = 107;
    pub const REFRESH: u8 = 108;
    pub const STREAMING: u8 = 109;
    pub const SEE_THROUGH: u8 = 110;
    pub const UNPAIRED: u8 = 111;
    pub const HEADSET: u8 = 112;
    pub const HAPTIC: u8 = 113;
}

fn bad(ty: u8, side: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("unknown XR {side} message {ty}"))
}

impl XrClientMsg {
    pub fn encode(&self) -> Vec<u8> {
        use tag::*;
        match self {
            XrClientMsg::Hello { version, name, authorization, instance, eye, refresh_mhz, codecs } => {
                let mut o = Out::new(HELLO);
                o.u16(*version).str(name).str(authorization).u64(*instance).u32(eye[0]).u32(eye[1]).u32(*refresh_mhz).u32(*codecs);
                o.done()
            }
            XrClientMsg::Pose { seq, sample_ns, display_ns, frame } => {
                let mut o = Out::new(POSE);
                o.u64(*seq).u64(*sample_ns).u64(*display_ns).bytes(frame);
                o.done()
            }
            XrClientMsg::State { visible, focused } => {
                let mut o = Out::new(STATE);
                o.u8(*visible as u8).u8(*focused as u8);
                o.done()
            }
            XrClientMsg::Stats { seq, decode_us, m2p_us } => {
                let mut o = Out::new(STATS);
                o.u64(*seq).u32(*decode_us).u32(*m2p_us);
                o.done()
            }
            XrClientMsg::Ping { client_ns } => {
                let mut o = Out::new(PING);
                o.u64(*client_ns);
                o.done()
            }
            XrClientMsg::Keyframe => Out::new(KEYFRAME).done(),
            XrClientMsg::Report { values } => {
                let mut o = Out::new(REPORT);
                o.u32(values.len() as u32);
                for v in values {
                    o.f32(*v);
                }
                o.done()
            }
            XrClientMsg::World { voxel_size, chunk_edge, bricks } => {
                let mut o = Out::new(WORLD);
                o.f32(*voxel_size).i32(*chunk_edge).u32(bricks.len() as u32);
                for b in bricks {
                    o.i32(b.key[0]).i32(b.key[1]).i32(b.key[2]).u32(b.version).bytes(&b.payload);
                }
                o.done()
            }
        }
    }

    pub fn decode(ty: u8, payload: &[u8]) -> io::Result<Self> {
        use tag::*;
        let mut r = In { b: payload };
        Ok(match ty {
            HELLO => XrClientMsg::Hello { version: r.u16()?, name: r.str()?, authorization: r.str()?, instance: r.u64()?, eye: [r.u32()?, r.u32()?], refresh_mhz: r.u32()?, codecs: r.u32()? },
            POSE => XrClientMsg::Pose { seq: r.u64()?, sample_ns: r.u64()?, display_ns: r.u64()?, frame: r.bytes()?.to_vec() },
            STATE => XrClientMsg::State { visible: r.bool()?, focused: r.bool()? },
            STATS => XrClientMsg::Stats { seq: r.u64()?, decode_us: r.u32()?, m2p_us: r.u32()? },
            PING => XrClientMsg::Ping { client_ns: r.u64()? },
            KEYFRAME => XrClientMsg::Keyframe,
            REPORT => {
                let n = r.u32()? as usize;
                let mut values = Vec::with_capacity(n.min(64));
                for _ in 0..n {
                    values.push(r.f32()?);
                }
                XrClientMsg::Report { values }
            }
            WORLD => {
                let (voxel_size, chunk_edge, n) = (r.f32()?, r.i32()?, r.u32()? as usize);
                let mut bricks = Vec::with_capacity(n.min(1 << 16));
                for _ in 0..n {
                    bricks.push(WorldBrick { key: [r.i32()?, r.i32()?, r.i32()?], version: r.u32()?, payload: r.bytes()?.to_vec() });
                }
                XrClientMsg::World { voxel_size, chunk_edge, bricks }
            }
            _ => return Err(bad(ty, "client")),
        })
    }
}

impl XrServerMsg {
    pub fn encode(&self) -> Vec<u8> {
        use tag::*;
        match self {
            XrServerMsg::Welcome { version, server, instance, session, size, codec, tiles, encoder } => {
                let mut o = Out::new(WELCOME);
                o.u16(*version).str(server).u64(*instance).u64(*session).u32(size[0]).u32(size[1]).u8(*codec).u8(*tiles).str(encoder);
                o.done()
            }
            XrServerMsg::Refused { reason } => {
                let mut o = Out::new(REFUSED);
                o.str(reason);
                o.done()
            }
            XrServerMsg::Video { seq, tile, key, display_ns, eyes, focus_m, stages, size, codec, data } => {
                encode_xr_video(&VideoHead { seq: *seq, tile: *tile, key: *key, display_ns: *display_ns, eyes: *eyes, focus_m: *focus_m, stages: *stages, size: *size, codec: *codec }, data)
            }
            XrServerMsg::Pong { client_ns, server_ns } => {
                let mut o = Out::new(PONG);
                o.u64(*client_ns).u64(*server_ns);
                o.done()
            }
            XrServerMsg::Bye { reason } => {
                let mut o = Out::new(BYE);
                o.str(reason);
                o.done()
            }
            XrServerMsg::Note { text } => {
                let mut o = Out::new(NOTE);
                o.str(text);
                o.done()
            }
            XrServerMsg::Uplink { port, key } => {
                let mut o = Out::new(UPLINK);
                o.u16(*port).bytes(key);
                o.done()
            }
            XrServerMsg::RoomField { wanted } => {
                let mut o = Out::new(ROOM_FIELD);
                o.u8(*wanted as u8);
                o.done()
            }
            XrServerMsg::Refresh { mhz } => {
                let mut o = Out::new(REFRESH);
                o.u32(*mhz);
                o.done()
            }
            XrServerMsg::Streaming { on } => {
                let mut o = Out::new(STREAMING);
                o.u8(*on as u8);
                o.done()
            }
            XrServerMsg::SeeThrough { on } => {
                let mut o = Out::new(SEE_THROUGH);
                o.u8(*on as u8);
                o.done()
            }
            XrServerMsg::Unpaired => Out::new(UNPAIRED).done(),
            XrServerMsg::Headset { foveation, eye_min_pct, eye_max_pct, multimodal, occlusion, ahead } => {
                let mut o = Out::new(HEADSET);
                o.u8(*foveation).u8(*eye_min_pct).u8(*eye_max_pct).u8(*multimodal as u8).u8(*occlusion as u8).u8(*ahead);
                o.done()
            }
            XrServerMsg::Haptic { hand, amplitude, duration_us } => {
                let mut o = Out::new(HAPTIC);
                o.u8(*hand).f32(*amplitude).u32(*duration_us);
                o.done()
            }
        }
    }

    pub fn decode(ty: u8, payload: &[u8]) -> io::Result<Self> {
        use tag::*;
        let mut r = In { b: payload };
        Ok(match ty {
            WELCOME => XrServerMsg::Welcome { version: r.u16()?, server: r.str()?, instance: r.u64()?, session: r.u64()?, size: [r.u32()?, r.u32()?], codec: r.u8()?, tiles: r.u8()?, encoder: r.str()? },
            REFUSED => XrServerMsg::Refused { reason: r.str()? },
            VIDEO => {
                let (seq, tile, key, display_ns) = (r.u64()?, r.u8()?, r.bool()?, r.u64()?);
                let mut eyes = [XrRenderEye::default(); 2];
                for e in &mut eyes {
                    for v in e.orientation.iter_mut().chain(e.position.iter_mut()).chain(e.fov.iter_mut()) {
                        *v = r.f32()?;
                    }
                }
                let focus_m = r.f32()?;
                let stages = [r.u32()?, r.u32()?, r.u32()?, r.u32()?];
                XrServerMsg::Video { seq, tile, key, display_ns, eyes, focus_m, stages, size: [r.u32()?, r.u32()?], codec: r.u8()?, data: r.bytes()?.to_vec() }
            }
            PONG => XrServerMsg::Pong { client_ns: r.u64()?, server_ns: r.u64()? },
            BYE => XrServerMsg::Bye { reason: r.str()? },
            NOTE => XrServerMsg::Note { text: r.str()? },
            UPLINK => XrServerMsg::Uplink { port: r.u16()?, key: r.bytes()?.to_vec() },
            ROOM_FIELD => XrServerMsg::RoomField { wanted: r.u8()? != 0 },
            REFRESH => XrServerMsg::Refresh { mhz: r.u32()? },
            STREAMING => XrServerMsg::Streaming { on: r.u8()? != 0 },
            SEE_THROUGH => XrServerMsg::SeeThrough { on: r.u8()? != 0 },
            UNPAIRED => XrServerMsg::Unpaired,
            HEADSET => XrServerMsg::Headset { foveation: r.u8()?, eye_min_pct: r.u8()?, eye_max_pct: r.u8()?, multimodal: r.u8()? != 0, occlusion: r.u8()? != 0, ahead: r.u8()? },
            HAPTIC => XrServerMsg::Haptic { hand: r.u8()?, amplitude: r.f32()?, duration_us: r.u32()? },
            _ => return Err(bad(ty, "server")),
        })
    }
}

/// A `Video` message's fields but its access unit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VideoHead {
    pub seq: u64,
    pub tile: u8,
    pub key: bool,
    pub display_ns: u64,
    pub eyes: [XrRenderEye; 2],
    pub focus_m: f32,
    pub stages: [u32; 4],
    pub size: [u32; 2],
    pub codec: u8,
}

/// A `Video` message without copying the access unit into an `XrServerMsg` first.
pub fn encode_xr_video(h: &VideoHead, data: &[u8]) -> Vec<u8> {
    let mut o = Out(Vec::with_capacity(data.len() + 160));
    o.0.extend_from_slice(&[tag::VIDEO, 0, 0, 0, 0]);
    o.u64(h.seq).u8(h.tile).u8(h.key as u8).u64(h.display_ns);
    for e in &h.eyes {
        for v in e.orientation.iter().chain(e.position.iter()).chain(e.fov.iter()) {
            o.f32(*v);
        }
    }
    o.f32(h.focus_m);
    for v in h.stages {
        o.u32(v);
    }
    o.u32(h.size[0]).u32(h.size[1]).u8(h.codec).bytes(data);
    o.done()
}

/// The game audio as datagrams: `MXA1`, seq u32, frames u16, channels u8, flags u8
/// ([`audio_datagram::TEST`]: the test signal), due_ns u64 (when its first frame is due on the
/// desktop's clock: when the app's mixer made it), then frames x channels i16 LE interleaved,
/// then the first 16 bytes of HMAC-SHA256(key, all before), the pose datagrams' key.
#[cfg(feature = "server")]
pub mod audio_datagram {
    const MAGIC: &[u8; 4] = b"MXA1";
    const TAG: usize = 16;
    const HEAD: usize = 4 + 4 + 2 + 1 + 1 + 8;
    /// The wire's rate and channels.
    pub const RATE: u32 = 48_000;
    pub const CHANNELS: usize = 2;
    /// Frames per datagram (5 ms: 960 bytes, one datagram, never a fragment).
    pub const FRAMES: usize = 240;
    pub const TEST: u8 = 1;

    pub struct Audio<'a> {
        pub seq: u32,
        pub frames: usize,
        pub channels: usize,
        pub flags: u8,
        pub due_ns: u64,
        /// i16 LE, interleaved.
        pub pcm: &'a [u8],
    }

    pub fn seal(key: &[u8], seq: u32, channels: usize, flags: u8, due_ns: u64, samples: &[i16], out: &mut Vec<u8>) {
        out.clear();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&seq.to_le_bytes());
        out.extend_from_slice(&((samples.len() / channels.max(1)) as u16).to_le_bytes());
        out.push(channels as u8);
        out.push(flags);
        out.extend_from_slice(&due_ns.to_le_bytes());
        for s in samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        let tag = makepad_network::tunnel::hmac_sha256(key, out);
        out.extend_from_slice(&tag[..TAG]);
    }

    pub fn open<'a>(key: &[u8], d: &'a [u8]) -> Option<Audio<'a>> {
        if d.len() < HEAD + TAG || &d[..4] != MAGIC {
            return None;
        }
        let (body, tag) = d.split_at(d.len() - TAG);
        let want = makepad_network::tunnel::hmac_sha256(key, body);
        if want[..TAG].iter().zip(tag).fold(0u8, |a, (x, y)| a | (x ^ y)) != 0 {
            return None;
        }
        let frames = u16::from_le_bytes([body[8], body[9]]) as usize;
        let channels = body[10] as usize;
        let pcm = &body[HEAD..];
        if channels == 0 || pcm.len() != frames * channels * 2 {
            return None;
        }
        Some(Audio { seq: u32::from_le_bytes(body[4..8].try_into().unwrap()), frames, channels, flags: body[11], due_ns: u64::from_le_bytes(body[12..20].try_into().unwrap()), pcm })
    }
}

/// The poses as datagrams (`XrServerMsg::Uplink`): `MXP1`, seq, sample_ns, display_ns (u64 LE),
/// the `XrFrame`, then the first 16 bytes of HMAC-SHA256(key, all before).
#[cfg(feature = "server")]
pub mod pose_datagram {
    const MAGIC: &[u8; 4] = b"MXP1";
    const TAG: usize = 16;

    pub fn seal(key: &[u8], seq: u64, sample_ns: u64, display_ns: u64, frame: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 24 + frame.len() + TAG);
        out.extend_from_slice(MAGIC);
        for v in [seq, sample_ns, display_ns] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(frame);
        let tag = makepad_network::tunnel::hmac_sha256(key, &out);
        out.extend_from_slice(&tag[..TAG]);
        out
    }

    /// (seq, sample_ns, display_ns, frame) of a datagram sealed with `key`; `None` otherwise.
    pub fn open<'a>(key: &[u8], d: &'a [u8]) -> Option<(u64, u64, u64, &'a [u8])> {
        if d.len() < 4 + 24 + TAG || &d[..4] != MAGIC {
            return None;
        }
        let (body, tag) = d.split_at(d.len() - TAG);
        let want = makepad_network::tunnel::hmac_sha256(key, body);
        // (constant time: no early out on the first differing byte)
        if want[..TAG].iter().zip(tag).fold(0u8, |a, (x, y)| a | (x ^ y)) != 0 {
            return None;
        }
        let u = |at: usize| u64::from_le_bytes(body[at..at + 8].try_into().unwrap());
        Some((u(4), u(12), u(20), &body[28..]))
    }
}

#[cfg(feature = "server")]
pub mod link {
    //! The headset's end: a TLS connection to an XR server on its own thread (TLS contexts are not
    //! shared between threads); the caller sends and receives through channels and never waits.
    use super::*;
    use crate::{write_all, Inbox};
    use std::io::Read;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    pub struct XrLink {
        tx: Sender<Vec<u8>>,
        /// Low-priority messages (the room field): written only when nothing else waits.
        bulk: Sender<Vec<u8>>,
        /// Behind a lock so a link can be shared (one reader in practice).
        rx: std::sync::Mutex<Receiver<XrServerMsg>>,
        closed: Arc<AtomicBool>,
        reason: Arc<std::sync::Mutex<String>>,
    }

    /// What the headset says of itself in its `Hello`.
    #[derive(Clone, Debug)]
    pub struct HeadsetInfo {
        pub name: String,
        /// The headset's instance id (`crate::xr_pair::HeadsetPairing`).
        pub instance: u64,
        pub eye: [u32; 2],
        pub refresh_mhz: u32,
        pub codecs: u32,
    }

    impl XrLink {
        /// Connects to `host:port` (the server proves itself with its fleet certificate; the
        /// hello carries this device's fleet proof). Returns once the TLS handshake and the hello
        /// are done; the `Welcome` (or `Refused`) arrives on [`XrLink::try_recv`].
        pub fn connect(host: &str, port: u16, info: HeadsetInfo, wake: Arc<dyn Fn() + Send + Sync>) -> Result<XrLink, String> {
            let (mut stream, _server, authorization) = makepad_network::fleet::id::connect(host, &port.to_string()).map_err(|e| format!("connect {host}:{port}: {e}"))?;
            let hello = XrClientMsg::Hello { version: XR_VERSION, name: info.name, authorization, instance: info.instance, eye: info.eye, refresh_mhz: info.refresh_mhz, codecs: info.codecs };
            write_all(&mut stream, &hello.encode()).map_err(|e| format!("hello: {e}"))?;
            stream.set_read_timeout(Some(Duration::from_millis(1))).map_err(|e| e.to_string())?;
            stream.set_write_timeout(Some(Duration::from_millis(5))).map_err(|e| e.to_string())?;
            let (tx, out_rx) = channel::<Vec<u8>>();
            let (bulk, bulk_rx) = channel::<Vec<u8>>();
            let (in_tx, rx) = channel();
            let closed = Arc::new(AtomicBool::new(false));
            let reason = Arc::new(std::sync::Mutex::new(String::new()));
            let (closed2, reason2) = (closed.clone(), reason.clone());
            std::thread::Builder::new()
                .name("xr-link".into())
                .spawn(move || {
                    let why = run(&mut stream, out_rx, bulk_rx, in_tx, &wake);
                    *reason2.lock().unwrap() = why;
                    closed2.store(true, Ordering::Release);
                    wake();
                })
                .map_err(|e| e.to_string())?;
            Ok(XrLink { tx, bulk, rx: std::sync::Mutex::new(rx), closed, reason })
        }

        /// Queues a message (never waits; a closed link drops it).
        pub fn send(&self, msg: &XrClientMsg) {
            let _ = self.tx.send(msg.encode());
        }

        /// Queues a low-priority message: it goes out only in the gaps, one per turn of the
        /// link's loop and only when no other message waits (a pose is never behind it). Keep
        /// each one small (tens of KB) so a pose waits at most one such write.
        pub fn send_bulk(&self, msg: &XrClientMsg) {
            let _ = self.bulk.send(msg.encode());
        }

        pub fn try_recv(&self) -> Option<XrServerMsg> {
            self.rx.lock().unwrap().try_recv().ok()
        }

        /// The next message, waiting up to `timeout` (a worker thread's loop; never the UI's).
        pub fn recv_timeout(&self, timeout: Duration) -> Option<XrServerMsg> {
            self.rx.lock().unwrap().recv_timeout(timeout).ok()
        }

        /// Why the link ended, once it did.
        pub fn closed(&self) -> Option<String> {
            self.closed.load(Ordering::Acquire).then(|| self.reason.lock().unwrap().clone())
        }
    }

    fn run(stream: &mut makepad_network::SocketStream, out: Receiver<Vec<u8>>, bulk: Receiver<Vec<u8>>, inbox_tx: Sender<XrServerMsg>, wake: &Arc<dyn Fn() + Send + Sync>) -> String {
        let mut inbox = Inbox::default();
        let mut buf = vec![0u8; 1 << 20];
        let mut last_heard = Instant::now();
        loop {
            loop {
                match out.try_recv() {
                    Ok(bytes) => {
                        if let Err(e) = write_all(stream, &bytes) {
                            return format!("write: {e}");
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return "closed by the headset".into(),
                }
            }
            // one low-priority message in the gap (the next turn writes the poses first)
            if let Ok(bytes) = bulk.try_recv() {
                if let Err(e) = write_all(stream, &bytes) {
                    return format!("write: {e}");
                }
            }
            let mut got = false;
            match stream.read(&mut buf) {
                Ok(0) => return "the server closed the connection".into(),
                Ok(n) => {
                    inbox.feed(&buf[..n]);
                    got = true;
                }
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => {}
                Err(e) => return format!("read: {e}"),
            }
            if got {
                last_heard = Instant::now();
                loop {
                    match inbox.next() {
                        Ok(Some((ty, payload))) => match XrServerMsg::decode(ty, &payload) {
                            Ok(msg) => {
                                let end = matches!(msg, XrServerMsg::Bye { .. } | XrServerMsg::Refused { .. });
                                let why = match &msg {
                                    XrServerMsg::Bye { reason } | XrServerMsg::Refused { reason } => reason.clone(),
                                    _ => String::new(),
                                };
                                let _ = inbox_tx.send(msg);
                                wake();
                                if end {
                                    return why;
                                }
                            }
                            Err(e) => return e.to_string(),
                        },
                        Ok(None) => break,
                        Err(e) => return e.to_string(),
                    }
                }
            }
            if last_heard.elapsed() > Duration::from_secs(6) {
                return "the server was silent for 6 s".into();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Inbox;

    #[test]
    fn report_frame_codes_round_trip() {
        let codes: Vec<u8> = (0..91).map(|i| [1, 1, 2, 3, 4, 5, 1][i % 7]).collect();
        let mut v = vec![7.0; report::COUNT];
        report::pack_frames(&mut v, &codes);
        assert_eq!(v[report::DISPLAY], 7.0);
        assert_eq!(report::frames(&v), codes);
        assert!(report::frames(&v[..report::COUNT]).is_empty());
    }

    #[test]
    fn xr_messages_round_trip() {
        let client = [
            XrClientMsg::Hello { version: XR_VERSION, name: "quest".into(), authorization: "MKID x".into(), instance: 42, eye: [1680, 1760], refresh_mhz: 72_000, codecs: codecs::HEVC },
            XrClientMsg::Pose { seq: 9, sample_ns: 1, display_ns: 2, frame: vec![1, 2, 3] },
            XrClientMsg::State { visible: true, focused: false },
            XrClientMsg::Stats { seq: 9, decode_us: 4000, m2p_us: 50_000 },
            XrClientMsg::Ping { client_ns: 5 },
            XrClientMsg::Keyframe,
            XrClientMsg::World { voxel_size: 0.02, chunk_edge: 4, bricks: vec![WorldBrick { key: [1, -2, 3], version: 7, payload: vec![1, 2, 3] }, WorldBrick { key: [0, 0, 0], version: 1, payload: Vec::new() }] },
        ];
        for m in client {
            let mut inbox = Inbox::default();
            inbox.feed(&m.encode());
            let (ty, p) = inbox.next().unwrap().unwrap();
            assert_eq!(XrClientMsg::decode(ty, &p).unwrap(), m);
        }
        let server = [
            XrServerMsg::Welcome { version: XR_VERSION, server: "166".into(), instance: 7, session: 9, size: [3360, 2640], codec: CODEC_HEVC, tiles: 2, encoder: "nvidia".into() },
            XrServerMsg::Refused { reason: "busy".into() },
            XrServerMsg::Video {
                seq: 9,
                tile: 1,
                key: true,
                display_ns: 77,
                eyes: [XrRenderEye { orientation: [0.0, 0.1, 0.0, 0.99], position: [-0.03, 1.6, 0.0], fov: [-0.9, 0.8, 0.85, -0.9] }, XrRenderEye { orientation: [0.0, 0.1, 0.0, 0.99], position: [0.03, 1.6, 0.0], fov: [-0.8, 0.9, 0.85, -0.9] }],
                focus_m: 1.25,
                stages: [1, 2, 3, 4],
                size: [3360, 2640],
                codec: CODEC_HEVC,
                data: vec![0, 0, 1],
            },
            XrServerMsg::Pong { client_ns: 1, server_ns: 2 },
            XrServerMsg::Bye { reason: "done".into() },
            XrServerMsg::Note { text: "hi".into() },
            XrServerMsg::RoomField { wanted: true },
            XrServerMsg::Refresh { mhz: 72_000 },
            XrServerMsg::Streaming { on: false },
            XrServerMsg::SeeThrough { on: true },
            XrServerMsg::Unpaired,
            XrServerMsg::Headset { foveation: 2, eye_min_pct: 70, eye_max_pct: 100, multimodal: true, occlusion: true, ahead: 4 },
            XrServerMsg::Haptic { hand: 1, amplitude: 0.85, duration_us: 35_000 },
        ];
        for m in server {
            let mut inbox = Inbox::default();
            inbox.feed(&m.encode());
            let (ty, p) = inbox.next().unwrap().unwrap();
            assert_eq!(XrServerMsg::decode(ty, &p).unwrap(), m);
        }
    }
}
