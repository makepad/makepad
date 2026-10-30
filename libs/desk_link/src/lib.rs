//! Desk Link: the wire protocol between the window manager's remote desktop server and its
//! viewer app. The format is documented in `apps/desklink/PROTOCOL.md`; this crate is its one
//! implementation (both ends use it), with no dependencies (features `server`, the listener and
//! its sessions, and `host`, the deskhost engine over the platform's encoder, add them).
//!
//! Transport: TLS over TCP. Every message is `type u8 | length u32 LE | payload`, fixed fields
//! little-endian, strings and byte blobs `length u32 LE | bytes`. A message over [`MAX_MESSAGE`]
//! is a protocol error (the connection closes).

use std::io::{self, Write};

#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "host")]
pub mod host;
pub mod rate;
#[cfg(feature = "tiles")]
pub mod tiles;
pub mod xr;
#[cfg(feature = "host")]
pub mod xr_audio;
#[cfg(feature = "server")]
pub mod xr_pair;

pub const VERSION: u16 = 8;
pub const DEFAULT_PORT: u16 = 8790;
/// The largest message either side accepts (a key frame of a big desktop fits many times over).
pub const MAX_MESSAGE: usize = 16 << 20;

/// The cursor shapes, by index on the wire (`Cursor::shape`). Names are the makepad
/// `MouseCursor` variants; an index past the table is the default arrow.
pub const CURSOR_SHAPES: &[&str] = &[
    "Hidden", "Default", "Crosshair", "Hand", "Arrow", "Move", "Text", "Wait", "Help", "NotAllowed", "Grab", "Grabbing", "Progress", "ContextMenu", "Cell",
    "VerticalText", "Alias", "Copy", "NoDrop", "AllScroll", "ZoomIn", "ZoomOut", "NResize", "NeResize", "EResize", "SeResize", "SResize", "SwResize", "WResize",
    "NwResize", "NsResize", "NeswResize", "EwResize", "NwseResize", "ColResize", "RowResize",
];

pub fn cursor_index(name: &str) -> u8 {
    CURSOR_SHAPES.iter().position(|n| *n == name).unwrap_or(1) as u8
}

/// Gamepad buttons (`Gamepad::buttons` bits).
pub mod pad {
    pub const A: u32 = 1 << 0;
    pub const B: u32 = 1 << 1;
    pub const X: u32 = 1 << 2;
    pub const Y: u32 = 1 << 3;
    pub const LEFT_SHOULDER: u32 = 1 << 4;
    pub const RIGHT_SHOULDER: u32 = 1 << 5;
    pub const SELECT: u32 = 1 << 6;
    pub const START: u32 = 1 << 7;
    pub const HOME: u32 = 1 << 8;
    pub const LEFT_THUMB: u32 = 1 << 9;
    pub const RIGHT_THUMB: u32 = 1 << 10;
    pub const DPAD_UP: u32 = 1 << 11;
    pub const DPAD_DOWN: u32 = 1 << 12;
    pub const DPAD_LEFT: u32 = 1 << 13;
    pub const DPAD_RIGHT: u32 = 1 << 14;
}

/// Modifier bits (`Key::mods`).
pub mod mods {
    pub const SHIFT: u8 = 1;
    pub const CONTROL: u8 = 2;
    pub const ALT: u8 = 4;
    pub const LOGO: u8 = 8;
}

/// Pointer buttons (`Button::button`).
/// `ClientMsg::Hello` capabilities of the viewer's decoder.
pub mod caps {
    /// Decodes 4:4:4 H.264 (High 4:4:4 Predictive) on its hardware decoder.
    pub const FULL_CHROMA: u32 = 1;
    /// Only watches (a probe, a test client): its `Window` never decides the size of a desktop
    /// that follows its viewer.
    pub const VIEW_ONLY: u32 = 2;
    /// Decodes H.265 (Main, and Format range extensions 4:4:4 when it has `FULL_CHROMA`).
    pub const HEVC: u32 = 4;
}

/// `ClientMsg::Encoding` choices (0 = auto everywhere) and the codec of `Video` / `Rate`.
pub mod pref {
    pub const AUTO: u8 = 0;
    /// `chroma`.
    pub const FULL: u8 = 1;
    pub const HALF: u8 = 2;
    /// `codec` (also `ServerMsg::Video::codec`, `Rate::codec`).
    pub const H264: u8 = 1;
    pub const HEVC: u8 = 2;
    /// `preset`: the encoder's best pictures per bit (NVIDIA: ultra-low-latency tuning, about
    /// 60% more encoder time) or its fastest encode (low-latency tuning).
    pub const SHARP: u8 = 1;
    pub const FAST: u8 = 2;
}

/// `ServerMsg::Rate::limit`: what holds the stream's rate where it is now.
pub mod limit {
    /// The starting rate, nothing measured yet.
    pub const START: u8 = 0;
    /// Nothing: the rate creeps back up.
    pub const CLEAN: u8 = 1;
    /// The link: a viewer fell behind (its socket backlog), the rate went down.
    pub const LINK: u8 = 2;
    /// The encoder: it took longer than a picture's time (a big 4:4:4 desktop goes 4:2:0).
    pub const ENCODER: u8 = 3;
    /// A cap: the desktop's `max_mbps`, a viewer's `Encoding` cap, or the size's own rate.
    pub const CAP: u8 = 4;
    /// The desktop's settings name a fixed rate (`kbps`).
    pub const FIXED: u8 = 5;
}


pub mod button {
    pub const LEFT: u8 = 0;
    pub const RIGHT: u8 = 1;
    pub const MIDDLE: u8 = 2;
    pub const BACK: u8 = 3;
    pub const FORWARD: u8 = 4;
}

/// Wheel units per notch (`Scroll`): the hi-res wheel convention.
pub const SCROLL_UNITS_PER_NOTCH: i32 = 120;

/// A viewer's makepad scroll (points, 40 per notch; `scroll.y > 0` = the view moves down the
/// content, with the viewer OS's own direction setting applied) as the wire's `Scroll`
/// (1/120 notches, +dy = the view moves up, +dx = right): the content direction the viewer's
/// user saw. `rest` carries parts of a unit to the next scroll. PROTOCOL.md "Scroll direction".
pub fn scroll_to_wire(rest: &mut (f64, f64), scroll_x: f64, scroll_y: f64) -> (i32, i32) {
    let x = rest.0 + scroll_x * 3.0;
    let y = rest.1 - scroll_y * 3.0;
    let (dx, dy) = (x.trunc() as i32, y.trunc() as i32);
    *rest = (x - dx as f64, y - dy as f64);
    (dx, dy)
}

#[derive(Clone, Debug, PartialEq)]
pub struct GamepadState {
    pub buttons: u32,
    /// Sticks in -32767..32767 (up is negative y, as makepad's `GamepadState`).
    pub left: [i16; 2],
    pub right: [i16; 2],
    /// Triggers 0..255.
    pub triggers: [u8; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScreenInfo {
    pub name: String,
    /// Its rectangle on the server's desktop, in device pixels.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Viewer -> server.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientMsg {
    /// Semantic command on the desktop, under the same fleet admin right as input.
    A11yQuery { req: u64, line: String },
    /// First message. `authorization` is the fleet proof (`MKID ...`) bound to the server's
    /// certificate; `screen` < 0 asks for the server's default screen; `paused` opens the
    /// session without video (a background session of the viewer's list).
    /// `caps`: what the viewer's decoder takes (`caps::*`).
    Hello { version: u16, name: String, authorization: String, screen: i32, paused: bool, caps: u32 },
    /// Stop / restart video (Resume starts with a key frame). The session and its pings stay.
    Pause,
    Resume,
    SelectScreen { index: u32 },
    /// A key frame now (a decoder error).
    RequestKeyframe,
    /// The viewer's display refresh in Hz (measured from its vsync; 0 = not known): the desktop
    /// streams at most this many pictures a second.
    Refresh { hz: u16 },
    /// The viewer's picture area in device pixels and its scale (hundredths: 200 = 2.00x),
    /// sent after the Welcome and on every resize (settled). A desktop whose size follows its
    /// viewer (a virtual display) takes this size, so the picture is shown 1:1.
    Window { width: u32, height: u32, scale: u16 },
    /// `keycode` is makepad's `KeyCode` index; `mods` [`mods`] bits.
    Key { down: bool, keycode: u32, mods: u8 },
    /// The pointer in the stream picture, 0..=65535 on both axes.
    Pointer { x: u16, y: u16 },
    Button { down: bool, button: u8 },
    /// Wheel motion in 1/120 notch units, the content direction the viewer's user saw: +dy the
    /// view moves up (the content down), +dx the view moves right ([`scroll_to_wire`]).
    Scroll { dx: i32, dy: i32 },
    Gamepad { slot: u8, state: GamepadState },
    GamepadGone { slot: u8 },
    Ping { client_ns: u64 },
    /// How the last shown picture fared: decode-end -> on screen, its `frame`; and the decode
    /// time of a picture (its slowest stream's), the mean since the last `Stats` (the desktop
    /// cuts H.265 into more bands when it nears a picture's time).
    Stats { frame: u32, decode_us: u32, present_us: u32 },
    Clipboard { text: String },
    /// How this viewer wants the stream encoded ([`pref`]; 0 = auto each): `chroma`, `codec`,
    /// `preset`, the most it may take (`mbps`, all tiles; 0 = no cap of its own: the desktop
    /// streams at most the lowest of its watching viewers' caps) and pictures a second (`fps`;
    /// 0 = its display's refresh). Sent after the Hello and on every change; the last connected viewer's
    /// choices decide the stream, the decoders of every watching viewer bound them.
    Encoding { chroma: u8, codec: u8, preset: u8, mbps: u16, fps: u16 },
}

/// Server -> viewer.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerMsg {
    A11yReply { req: u64, text: String },
    /// The session is open. `stream` is the picture size of `screen` (its output size); `input`
    /// says whether input is injected, `input_note` why not.
    Welcome { version: u16, session: u32, screens: Vec<ScreenInfo>, screen: u32, stream: [u32; 2], input: bool, input_note: String, server: String },
    Refused { reason: String },
    /// One picture (an H.264 access unit, Annex-B; a key frame starts with SPS/PPS). Times are the
    /// server's clock: frame build start, bitstream ready, handed to the socket.
    /// A part of picture `frame` from encoder stream `stream` (each stream is an H.264 stream of
    /// its own, with its own SPS, decoded by its own decoder): its pixels from column `x0`, row
    /// `y0` on (its SPS gives how many; a picture wider than the encoder takes is split into
    /// columns as well as bands). The picture is complete with `parts` parts of the same `frame`: one per
    /// stream when the streams are tiles (bands of each picture, encoded at once), 1 when they
    /// interleave (stream `frame % streams` encodes whole pictures in turn).
    /// `codec`: [`pref::H264`] or [`pref::HEVC`] (H.265: VPS, SPS and PPS before each IDR).
    Video { frame: u32, key: bool, built_ns: u64, encoded_ns: u64, sent_ns: u64, stream: u8, parts: u8, x0: u32, y0: u32, codec: u8, data: Vec<u8> },
    /// The pointer over the stream picture (0..=65535), its shape ([`CURSOR_SHAPES`] index).
    Cursor { x: u16, y: u16, shape: u8, visible: bool },
    Pong { client_ns: u64, server_ns: u64 },
    /// The stream now shows another screen (or the same one at a new size).
    Stream { screen: u32, stream: [u32; 2] },
    /// The pictures a second the desktop streams at now (its own display's refresh, the
    /// watching viewers' and its settings' cap, whichever is lowest); sent after the Welcome and
    /// on every change. It can also precede Welcome with fps=0 and a preparation
    /// status in `why` while the desktop gets its screen ready. At fps=0 a nonempty
    /// `why` is a status notice to show over the picture. A still desktop sends fewer. `kbps`: the stream's rate now (all its
    /// tiles, the encoder's average target), `limit` what holds it there ([`limit`]),
    /// `full_chroma` whether it is 4:4:4 now.
    /// `codec` ([`pref`]), `preset` (SHARP / FAST), `streams` (the encoder streams a picture
    /// is cut into) and `why`: what the stream is and why, in words (e.g. "fast: the encoder is
    /// behind at sharp").
    Rate { fps: u16, kbps: u32, limit: u8, full_chroma: bool, codec: u8, preset: u8, streams: u8, why: String },
    Clipboard { text: String },
    Bye { reason: String },
}

// ---- encoding

struct Out(Vec<u8>);

impl Out {
    fn new(ty: u8) -> Self {
        Out(vec![ty, 0, 0, 0, 0])
    }
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn i16(&mut self, v: i16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn i32(&mut self, v: i32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn f32(&mut self, v: f32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
        self
    }
    fn str(&mut self, v: &str) -> &mut Self {
        self.bytes(v.as_bytes())
    }
    fn done(mut self) -> Vec<u8> {
        let len = (self.0.len() - 5) as u32;
        self.0[1..5].copy_from_slice(&len.to_le_bytes());
        self.0
    }
}

struct In<'a> {
    b: &'a [u8],
}

fn short() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "message too short")
}

impl<'a> In<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.b.len() < n {
            return Err(short());
        }
        let (a, rest) = self.b.split_at(n);
        self.b = rest;
        Ok(a)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn bool(&mut self) -> io::Result<bool> {
        Ok(self.u8()? != 0)
    }
    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn i16(&mut self) -> io::Result<i16> {
        Ok(i16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> io::Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> io::Result<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> io::Result<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn str(&mut self) -> io::Result<String> {
        String::from_utf8(self.bytes()?.to_vec()).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "string is not UTF-8"))
    }
}

mod tag {
    pub const HELLO: u8 = 1;
    pub const PAUSE: u8 = 2;
    pub const RESUME: u8 = 3;
    pub const SELECT_SCREEN: u8 = 4;
    pub const REQUEST_KEYFRAME: u8 = 5;
    pub const REFRESH: u8 = 7;
    pub const WINDOW: u8 = 8;
    pub const KEY: u8 = 10;
    pub const POINTER: u8 = 11;
    pub const BUTTON: u8 = 12;
    pub const SCROLL: u8 = 13;
    pub const GAMEPAD: u8 = 14;
    pub const GAMEPAD_GONE: u8 = 15;
    pub const PING: u8 = 20;
    pub const STATS: u8 = 21;
    pub const CLIPBOARD_C: u8 = 22;
    pub const ENCODING: u8 = 24;

    pub const WELCOME: u8 = 100;
    pub const REFUSED: u8 = 101;
    pub const VIDEO: u8 = 102;
    pub const CURSOR: u8 = 103;
    pub const PONG: u8 = 104;
    pub const STREAM: u8 = 105;
    pub const CLIPBOARD_S: u8 = 106;
    pub const BYE: u8 = 107;
    pub const RATE: u8 = 108;
    pub const A11Y_QUERY: u8 = 25;
    pub const A11Y_REPLY: u8 = 109;
}

impl ClientMsg {
    pub fn encode(&self) -> Vec<u8> {
        use tag::*;
        match self {
            ClientMsg::A11yQuery { req, line } => {
                let mut w = Out::new(A11Y_QUERY); w.u64(*req); w.str(line); w.done()
            }
            ClientMsg::Hello { version, name, authorization, screen, paused, caps } => {
                let mut o = Out::new(HELLO);
                o.u16(*version).str(name).str(authorization).i32(*screen).u8(*paused as u8).u32(*caps);
                o.done()
            }
            ClientMsg::Pause => Out::new(PAUSE).done(),
            ClientMsg::Resume => Out::new(RESUME).done(),
            ClientMsg::SelectScreen { index } => {
                let mut o = Out::new(SELECT_SCREEN);
                o.u32(*index);
                o.done()
            }
            ClientMsg::RequestKeyframe => Out::new(REQUEST_KEYFRAME).done(),
            ClientMsg::Refresh { hz } => {
                let mut o = Out::new(REFRESH);
                o.u16(*hz);
                o.done()
            }
            ClientMsg::Window { width, height, scale } => {
                let mut o = Out::new(WINDOW);
                o.u32(*width).u32(*height).u16(*scale);
                o.done()
            }
            ClientMsg::Key { down, keycode, mods } => {
                let mut o = Out::new(KEY);
                o.u8(*down as u8).u32(*keycode).u8(*mods);
                o.done()
            }
            ClientMsg::Pointer { x, y } => {
                let mut o = Out::new(POINTER);
                o.u16(*x).u16(*y);
                o.done()
            }
            ClientMsg::Button { down, button } => {
                let mut o = Out::new(BUTTON);
                o.u8(*down as u8).u8(*button);
                o.done()
            }
            ClientMsg::Scroll { dx, dy } => {
                let mut o = Out::new(SCROLL);
                o.i32(*dx).i32(*dy);
                o.done()
            }
            ClientMsg::Gamepad { slot, state } => {
                let mut o = Out::new(GAMEPAD);
                o.u8(*slot).u32(state.buttons).i16(state.left[0]).i16(state.left[1]).i16(state.right[0]).i16(state.right[1]).u8(state.triggers[0]).u8(state.triggers[1]);
                o.done()
            }
            ClientMsg::GamepadGone { slot } => {
                let mut o = Out::new(GAMEPAD_GONE);
                o.u8(*slot);
                o.done()
            }
            ClientMsg::Ping { client_ns } => {
                let mut o = Out::new(PING);
                o.u64(*client_ns);
                o.done()
            }
            ClientMsg::Stats { frame, decode_us, present_us } => {
                let mut o = Out::new(STATS);
                o.u32(*frame).u32(*decode_us).u32(*present_us);
                o.done()
            }
            ClientMsg::Clipboard { text } => {
                let mut o = Out::new(CLIPBOARD_C);
                o.str(text);
                o.done()
            }
            ClientMsg::Encoding { chroma, codec, preset, mbps, fps } => {
                let mut o = Out::new(ENCODING);
                o.u8(*chroma).u8(*codec).u8(*preset).u16(*mbps).u16(*fps);
                o.done()
            }
        }
    }

    pub fn decode(ty: u8, payload: &[u8]) -> io::Result<Self> {
        use tag::*;
        let mut r = In { b: payload };
        Ok(match ty {
            A11Y_QUERY => ClientMsg::A11yQuery { req: r.u64()?, line: r.str()? },
            HELLO => ClientMsg::Hello { version: r.u16()?, name: r.str()?, authorization: r.str()?, screen: r.i32()?, paused: r.bool()?, caps: r.u32()? },
            PAUSE => ClientMsg::Pause,
            RESUME => ClientMsg::Resume,
            SELECT_SCREEN => ClientMsg::SelectScreen { index: r.u32()? },
            REQUEST_KEYFRAME => ClientMsg::RequestKeyframe,
            REFRESH => ClientMsg::Refresh { hz: r.u16()? },
            WINDOW => ClientMsg::Window { width: r.u32()?, height: r.u32()?, scale: r.u16()? },
            KEY => ClientMsg::Key { down: r.bool()?, keycode: r.u32()?, mods: r.u8()? },
            POINTER => ClientMsg::Pointer { x: r.u16()?, y: r.u16()? },
            BUTTON => ClientMsg::Button { down: r.bool()?, button: r.u8()? },
            SCROLL => ClientMsg::Scroll { dx: r.i32()?, dy: r.i32()? },
            GAMEPAD => ClientMsg::Gamepad {
                slot: r.u8()?,
                state: GamepadState { buttons: r.u32()?, left: [r.i16()?, r.i16()?], right: [r.i16()?, r.i16()?], triggers: [r.u8()?, r.u8()?] },
            },
            GAMEPAD_GONE => ClientMsg::GamepadGone { slot: r.u8()? },
            PING => ClientMsg::Ping { client_ns: r.u64()? },
            STATS => ClientMsg::Stats { frame: r.u32()?, decode_us: r.u32()?, present_us: r.u32()? },
            ENCODING => ClientMsg::Encoding { chroma: r.u8()?, codec: r.u8()?, preset: r.u8()?, mbps: r.u16()?, fps: r.u16()? },
            CLIPBOARD_C => ClientMsg::Clipboard { text: r.str()? },
            _ => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unknown viewer message {ty}"))),
        })
    }
}

impl ServerMsg {
    pub fn encode(&self) -> Vec<u8> {
        use tag::*;
        match self {
            ServerMsg::A11yReply { req, text } => {
                let mut w = Out::new(A11Y_REPLY); w.u64(*req); w.str(text); w.done()
            }
            ServerMsg::Welcome { version, session, screens, screen, stream, input, input_note, server } => {
                let mut o = Out::new(WELCOME);
                o.u16(*version).u32(*session).u32(screens.len() as u32);
                for s in screens {
                    o.str(&s.name).u32(s.x).u32(s.y).u32(s.width).u32(s.height);
                }
                o.u32(*screen).u32(stream[0]).u32(stream[1]).u8(*input as u8).str(input_note).str(server);
                o.done()
            }
            ServerMsg::Refused { reason } => {
                let mut o = Out::new(REFUSED);
                o.str(reason);
                o.done()
            }
            ServerMsg::Video { frame, key, built_ns, encoded_ns, sent_ns, stream, parts, x0, y0, codec, data } => encode_video(*frame, *key, *built_ns, *encoded_ns, *sent_ns, (*stream, *parts, *x0, *y0), *codec, data),
            ServerMsg::Cursor { x, y, shape, visible } => {
                let mut o = Out::new(CURSOR);
                o.u16(*x).u16(*y).u8(*shape).u8(*visible as u8);
                o.done()
            }
            ServerMsg::Pong { client_ns, server_ns } => {
                let mut o = Out::new(PONG);
                o.u64(*client_ns).u64(*server_ns);
                o.done()
            }
            ServerMsg::Stream { screen, stream } => {
                let mut o = Out::new(STREAM);
                o.u32(*screen).u32(stream[0]).u32(stream[1]);
                o.done()
            }
            ServerMsg::Clipboard { text } => {
                let mut o = Out::new(CLIPBOARD_S);
                o.str(text);
                o.done()
            }
            ServerMsg::Bye { reason } => {
                let mut o = Out::new(BYE);
                o.str(reason);
                o.done()
            }
            ServerMsg::Rate { fps, kbps, limit, full_chroma, codec, preset, streams, why } => {
                let mut o = Out::new(RATE);
                o.u16(*fps).u32(*kbps).u8(*limit).u8(*full_chroma as u8).u8(*codec).u8(*preset).u8(*streams).str(why);
                o.done()
            }
        }
    }

    pub fn decode(ty: u8, payload: &[u8]) -> io::Result<Self> {
        use tag::*;
        let mut r = In { b: payload };
        Ok(match ty {
            A11Y_REPLY => ServerMsg::A11yReply { req: r.u64()?, text: r.str()? },
            WELCOME => {
                let version = r.u16()?;
                let session = r.u32()?;
                let n = r.u32()? as usize;
                if n > 64 {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "too many screens"));
                }
                let mut screens = Vec::with_capacity(n);
                for _ in 0..n {
                    screens.push(ScreenInfo { name: r.str()?, x: r.u32()?, y: r.u32()?, width: r.u32()?, height: r.u32()? });
                }
                ServerMsg::Welcome { version, session, screens, screen: r.u32()?, stream: [r.u32()?, r.u32()?], input: r.bool()?, input_note: r.str()?, server: r.str()? }
            }
            REFUSED => ServerMsg::Refused { reason: r.str()? },
            VIDEO => ServerMsg::Video { frame: r.u32()?, key: r.bool()?, built_ns: r.u64()?, encoded_ns: r.u64()?, sent_ns: r.u64()?, stream: r.u8()?, parts: r.u8()?, x0: r.u32()?, y0: r.u32()?, codec: r.u8()?, data: r.bytes()?.to_vec() },
            CURSOR => ServerMsg::Cursor { x: r.u16()?, y: r.u16()?, shape: r.u8()?, visible: r.bool()? },
            PONG => ServerMsg::Pong { client_ns: r.u64()?, server_ns: r.u64()? },
            STREAM => ServerMsg::Stream { screen: r.u32()?, stream: [r.u32()?, r.u32()?] },
            CLIPBOARD_S => ServerMsg::Clipboard { text: r.str()? },
            BYE => ServerMsg::Bye { reason: r.str()? },
            RATE => ServerMsg::Rate { fps: r.u16()?, kbps: r.u32()?, limit: r.u8()?, full_chroma: r.bool()?, codec: r.u8()?, preset: r.u8()?, streams: r.u8()?, why: r.str()? },
            _ => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unknown server message {ty}"))),
        })
    }
}

/// A `Video` message without copying the picture into a `ServerMsg` first.
/// `part`: (stream, parts of the picture, first row).
/// `part`: (stream, parts, x0, y0), see `ServerMsg::Video`.
pub fn encode_video(frame: u32, key: bool, built_ns: u64, encoded_ns: u64, sent_ns: u64, part: (u8, u8, u32, u32), codec: u8, data: &[u8]) -> Vec<u8> {
    let mut o = Out(Vec::with_capacity(data.len() + 60));
    o.0.extend_from_slice(&[tag::VIDEO, 0, 0, 0, 0]);
    o.u32(frame).u8(key as u8).u64(built_ns).u64(encoded_ns).u64(sent_ns).u8(part.0).u8(part.1).u32(part.2).u32(part.3).u8(codec).bytes(data);
    o.done()
}

/// Stamps `sent_ns` into every `Video` message of `bytes` (whole messages back to back: the
/// tiles of one picture go out together).
pub fn stamp_videos(bytes: &mut [u8], sent_ns: u64) {
    let mut at = 0;
    while at + 5 <= bytes.len() {
        let len = u32::from_le_bytes([bytes[at + 1], bytes[at + 2], bytes[at + 3], bytes[at + 4]]) as usize;
        if bytes[at] == tag::VIDEO && at + VIDEO_SENT_AT + 8 <= bytes.len() {
            bytes[at + VIDEO_SENT_AT..at + VIDEO_SENT_AT + 8].copy_from_slice(&sent_ns.to_le_bytes());
        }
        at += 5 + len;
    }
}

/// Offset of `sent_ns` in an encoded `Video` message: the sender stamps it as it writes.
pub const VIDEO_SENT_AT: usize = 5 + 4 + 1 + 8 + 8;

/// Incoming bytes -> whole messages. Feed whatever a read returned (a read timeout mid-message
/// is normal); take messages out with [`Inbox::next`].
#[derive(Default)]
pub struct Inbox {
    buf: Vec<u8>,
    at: usize,
}

impl Inbox {
    pub fn feed(&mut self, bytes: &[u8]) {
        if self.at > 0 && self.at == self.buf.len() {
            self.buf.clear();
            self.at = 0;
        }
        self.buf.extend_from_slice(bytes);
    }

    /// The next whole message (type, payload), or `None` until more bytes arrive.
    pub fn next(&mut self) -> io::Result<Option<(u8, Vec<u8>)>> {
        let rest = &self.buf[self.at..];
        if rest.len() < 5 {
            self.compact();
            return Ok(None);
        }
        let len = u32::from_le_bytes(rest[1..5].try_into().unwrap()) as usize;
        if len > MAX_MESSAGE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("message of {len} bytes")));
        }
        if rest.len() < 5 + len {
            self.compact();
            return Ok(None);
        }
        let msg = (rest[0], rest[5..5 + len].to_vec());
        self.at += 5 + len;
        Ok(Some(msg))
    }

    fn compact(&mut self) {
        if self.at > 0 {
            self.buf.drain(..self.at);
            self.at = 0;
        }
    }
}

/// Writes one encoded message whole (retrying a write that timed out part way: a TLS stream
/// resumes its record).
pub fn write_all(w: &mut dyn Write, bytes: &[u8]) -> io::Result<()> {
    let mut done = 0;
    let mut stalls = 0;
    while done < bytes.len() {
        match w.write(&bytes[done..]) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "connection closed")),
            Ok(n) => {
                done += n;
                stalls = 0;
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => {
                stalls += 1;
                // the peer has read nothing for about 5 s at the senders' 5 ms write timeout
                if stalls > 1_000 {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "the peer stopped reading"));
                }
            }
            Err(e) => return Err(e),
        }
    }
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_client(m: ClientMsg) {
        let bytes = m.encode();
        let mut inbox = Inbox::default();
        // byte by byte: a message only comes out whole
        for (i, b) in bytes.iter().enumerate() {
            inbox.feed(&[*b]);
            let got = inbox.next().unwrap();
            if i + 1 < bytes.len() {
                assert!(got.is_none());
            } else {
                let (ty, payload) = got.unwrap();
                assert_eq!(ClientMsg::decode(ty, &payload).unwrap(), m);
            }
        }
    }

    fn round_server(m: ServerMsg) {
        let bytes = m.encode();
        let mut inbox = Inbox::default();
        inbox.feed(&bytes);
        inbox.feed(&bytes);
        for _ in 0..2 {
            let (ty, payload) = inbox.next().unwrap().unwrap();
            assert_eq!(ServerMsg::decode(ty, &payload).unwrap(), m);
        }
        assert!(inbox.next().unwrap().is_none());
    }

    #[test]
    fn client_messages_round_trip() {
        round_client(ClientMsg::Hello { version: VERSION, name: "viewer".into(), authorization: "MKID mkid1.a.device.9.-.04.ff.1.ff".into(), screen: -1, paused: true, caps: caps::FULL_CHROMA });
        round_client(ClientMsg::Pause);
        round_client(ClientMsg::Resume);
        round_client(ClientMsg::SelectScreen { index: 2 });
        round_client(ClientMsg::RequestKeyframe);
        round_client(ClientMsg::Encoding { chroma: pref::FULL, codec: pref::HEVC, preset: pref::SHARP, mbps: 300, fps: 120 });
        round_client(ClientMsg::Refresh { hz: 120 });
        round_client(ClientMsg::Window { width: 7680, height: 2160, scale: 200 });
        round_client(ClientMsg::Key { down: true, keycode: 42, mods: mods::SHIFT | mods::LOGO });
        round_client(ClientMsg::Pointer { x: 65535, y: 12 });
        round_client(ClientMsg::Button { down: false, button: button::RIGHT });
        round_client(ClientMsg::Scroll { dx: -120, dy: 360 });
        round_client(ClientMsg::Gamepad { slot: 1, state: GamepadState { buttons: pad::A | pad::DPAD_LEFT, left: [-32767, 5], right: [0, 32767], triggers: [255, 7] } });
        round_client(ClientMsg::GamepadGone { slot: 1 });
        round_client(ClientMsg::Ping { client_ns: u64::MAX - 3 });
        round_client(ClientMsg::Stats { frame: 9, decode_us: 1200, present_us: 8000 });
        round_client(ClientMsg::Clipboard { text: "héllo".into() });
    }

    #[test]
    fn scroll_goes_on_the_wire_in_the_viewers_content_direction() {
        let mut rest = (0.0, 0.0);
        // a Mac user's two fingers up with natural scrolling (AppKit scrollingDeltaY < 0, makepad
        // scroll.y = +40: the view moves down, the content up): the view moves down on the wire
        assert_eq!(scroll_to_wire(&mut rest, 0.0, 40.0), (0, -120));
        // fingers down: the view moves up; fingers left (scroll.x > 0): the view moves right
        assert_eq!(scroll_to_wire(&mut rest, 0.0, -40.0), (0, 120));
        assert_eq!(scroll_to_wire(&mut rest, 40.0, 0.0), (120, 0));
        // a trackpad's fractions carry over
        assert_eq!(scroll_to_wire(&mut rest, 0.0, 0.2), (0, 0));
        assert_eq!(scroll_to_wire(&mut rest, 0.0, 0.2), (0, -1));
    }

    #[test]
    fn server_messages_round_trip() {
        round_server(ServerMsg::Welcome {
            version: VERSION,
            session: 7,
            screens: vec![ScreenInfo { name: "DP-1".into(), x: 0, y: 0, width: 2560, height: 1440 }, ScreenInfo { name: "HDMI-A-1".into(), x: 2560, y: 0, width: 1920, height: 1080 }],
            screen: 1,
            stream: [1920, 1080],
            input: false,
            input_note: "no /dev/uinput access".into(),
            server: "wm 166".into(),
        });
        round_server(ServerMsg::Refused { reason: "busy".into() });
        round_server(ServerMsg::Video { frame: 3, key: true, built_ns: 1, encoded_ns: 2, sent_ns: 3, stream: 1, parts: 4, x0: 3840, y0: 1072, codec: pref::HEVC, data: vec![0, 0, 0, 1, 0x67] });
        round_server(ServerMsg::Cursor { x: 1, y: 2, shape: cursor_index("Text"), visible: true });
        round_server(ServerMsg::Pong { client_ns: 5, server_ns: 6 });
        round_server(ServerMsg::Stream { screen: 0, stream: [2560, 1440] });
        round_server(ServerMsg::Clipboard { text: String::new() });
        round_server(ServerMsg::Bye { reason: "the WM quits".into() });
        round_server(ServerMsg::Rate { fps: 120, kbps: 250_000, limit: limit::LINK, full_chroma: true, codec: pref::HEVC, preset: pref::SHARP, streams: 2, why: "fast: the encoder is behind".into() });
    }

    #[test]
    fn video_sent_stamp_offset() {
        let mut bytes = encode_video(1, false, 10, 20, 0, (0, 1, 0, 0), pref::H264, &[9; 4]);
        bytes[VIDEO_SENT_AT..VIDEO_SENT_AT + 8].copy_from_slice(&30u64.to_le_bytes());
        let mut inbox = Inbox::default();
        inbox.feed(&bytes);
        let (ty, payload) = inbox.next().unwrap().unwrap();
        assert_eq!(ServerMsg::decode(ty, &payload).unwrap(), ServerMsg::Video { frame: 1, key: false, built_ns: 10, encoded_ns: 20, sent_ns: 30, stream: 0, parts: 1, x0: 0, y0: 0, codec: pref::H264, data: vec![9; 4] });
    }

    #[test]
    fn oversized_and_truncated_messages_are_errors() {
        let mut inbox = Inbox::default();
        inbox.feed(&[tag::VIDEO, 0xff, 0xff, 0xff, 0x7f]);
        assert!(inbox.next().is_err());
        assert!(ClientMsg::decode(tag::KEY, &[1, 2]).is_err());
        assert!(ServerMsg::decode(200, &[]).is_err());
        assert_eq!(cursor_index("NoSuchShape"), 1);
    }
}
