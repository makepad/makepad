//! The media section of the HAL interface: video and audio-file playback, native views (camera
//! preview, system browser, web view), audio devices and their realtime entries, MIDI, camera
//! capture and encoders, game input, location and permissions. The commands and events are
//! lines of `cmds.rs` / `events.rs`; this module holds what their small enums and payloads
//! mean, and the payload codecs both sides share. The implementation is
//! `backend/apple/media` (macOS) `backend/linux/media` or `backend/windows/media`; a backend without it answers every media command with the
//! matching failure event (see `exec::media_unavailable`).
//!
//! Ids: a video / camera preview / browser is named by the 64-bit id the app gave it
//! (`LiveId`), a device or port by its 64-bit id; the HAL keeps them as numbers.

/// `VideoPrepare::mode` bits (the command's `flags`).
pub mod video_mode {
    pub const AUTOPLAY: u16 = 1;
    pub const LOOP: u16 = 2;
    /// no picture: an audio file (the texture ids are `Id::NONE`)
    pub const AUDIO_ONLY: u16 = 4;
    /// the source payload is `src_kind::*` << 4
    pub const SRC_SHIFT: u16 = 4;
    pub const SRC_MASK: u16 = 0xf0;
    /// A camera source: the payload is the input and format ids (two u64)
    pub const CAMERA: u16 = 8;
}

/// What a `VideoPrepare` payload is.
pub mod src_kind {
    /// a URL (utf8)
    pub const NETWORK: u16 = 0;
    /// a file path (utf8)
    pub const FILE: u16 = 1;
    /// the bytes of the file
    pub const MEMORY: u16 = 2;
}

/// `VideoControl::op`
pub mod video_op {
    pub const PLAY: u32 = 0;
    pub const PAUSE: u32 = 1;
    pub const RESUME: u32 = 2;
    pub const MUTE: u32 = 3;
    pub const UNMUTE: u32 = 4;
    /// release the player (answered by `VideoStatus` RELEASED)
    pub const CLEANUP: u32 = 5;
    /// `arg` is the position in ms
    pub const SEEK: u32 = 6;
    /// `arg` is the volume as f32 bits
    pub const VOLUME: u32 = 7;
    /// `arg` is the rate as f32 bits
    pub const RATE: u32 = 8;
    pub const SELECT_VIDEO_TRACK: u32 = 9;
    pub const SELECT_AUDIO_TRACK: u32 = 10;
}

/// `VideoStatus::kind`
pub mod video_status {
    /// the player failed to prepare or decode; the text says why
    pub const ERROR: u32 = 0;
    /// the player and its textures were released
    pub const RELEASED: u32 = 1;
    /// playback reached the end
    pub const COMPLETED: u32 = 2;
}

/// `VideoRanges::kind`
pub mod ranges {
    pub const SEEKABLE: u32 = 0;
    pub const BUFFERED: u32 = 1;
}

/// `VideoFrame::yuv` bits: how the frame is in the textures.
pub mod yuv {
    /// the shader samples the Y / UV planes, not the BGRA texture
    pub const ENABLED: u32 = 1;
    /// UV is one RG8 plane (NV12)
    pub const BIPLANAR: u32 = 2;
    pub const FULL_RANGE: u32 = 4;
}

/// `AudioUse::dir` / `AudioEntry::dir`
pub mod audio_dir {
    pub const INPUT: u32 = 0;
    pub const OUTPUT: u32 = 1;
}

/// `AudioUse::options` bits.
pub mod audio_option {
    /// capture through the OS voice-processing path (echo cancellation)
    pub const ECHO_CANCELLATION: u32 = 1;
}

/// What an audio device is (`AudioDeviceInfo::kind`).
pub mod device_kind {
    pub const INPUT: u32 = 0;
    pub const OUTPUT: u32 = 1;
    /// an output device captured as an input (system audio)
    pub const LOOPBACK: u32 = 2;
}

/// One block of audio, handed to the entry the app registered with `AudioEntry`. The entry runs
/// on the OS's realtime audio thread: it must not block, allocate on its hot path or take a lock
/// the app's threads hold, and it must catch its own panics (an `extern "C"` fn that unwinds
/// aborts the process). It must not call `hal_submit` or `hal_pump` (the HAL's state lock is held
/// by the thread that replaces the entry while it waits for a running block to end); `hal_wake`
/// is fine. Planar samples: channel `c` is `frames` floats at `planes[c]`. An output
/// entry fills them (they start as zeros), an input entry reads them.
#[repr(C)]
pub struct AudioBlock {
    /// the device the block is of (the 64-bit id the app was told in `AudioDevices`)
    pub device: u64,
    pub sample_rate: f64,
    pub sample_time: f64,
    pub host_time: u64,
    pub rate_scalar: f64,
    /// whether the three time fields mean anything
    pub has_time: u32,
    pub frames: u32,
    pub channels: u32,
    pub planes: *mut *mut f32,
}

/// The realtime entry the app registers. `ctx` is the pointer it registered with.
pub type AudioFn = extern "C" fn(ctx: *mut std::ffi::c_void, block: *mut AudioBlock);

/// One audio device as `AudioDevices` lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioDeviceInfo {
    pub id: u64,
    /// `device_kind::*`
    pub kind: u32,
    pub is_default: bool,
    pub has_failed: bool,
    pub channels: u32,
    pub name: String,
}

pub fn pack_audio_devices(devices: &[AudioDeviceInfo]) -> Vec<u8> {
    let mut w = PackW::new();
    for d in devices {
        w.u64(d.id).u32(d.kind).u8(d.is_default as u8).u8(d.has_failed as u8).u32(d.channels).str(&d.name);
    }
    w.finish()
}

pub fn read_audio_devices(b: &[u8]) -> Vec<AudioDeviceInfo> {
    let mut r = PackR::new(b);
    let mut out = Vec::new();
    while !r.at_end() {
        let d = AudioDeviceInfo { id: r.u64(), kind: r.u32(), is_default: r.u8() != 0, has_failed: r.u8() != 0, channels: r.u32(), name: r.str() };
        if !r.ok {
            break;
        }
        out.push(d);
    }
    out
}

/// `Midi::op`
pub mod midi_op {
    /// start the MIDI client (the first `MidiPorts` follows); idempotent
    pub const OPEN: u32 = 0;
    /// `ports` lists the input ports to listen to (the others are disconnected)
    pub const USE_INPUTS: u32 = 1;
    pub const USE_OUTPUTS: u32 = 2;
    /// disconnect every input and list the ports again
    pub const RESET: u32 = 3;
    /// one 3-byte channel message (`data`: status << 16 | d1 << 8 | d2) to `port`, or to every
    /// output port when `port` is 0
    pub const SEND: u32 = 4;
}

/// One MIDI port as `MidiPorts` lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct MidiPortInfo {
    pub id: u64,
    pub output: bool,
    pub name: String,
}

pub fn pack_midi_ports(ports: &[MidiPortInfo]) -> Vec<u8> {
    let mut w = PackW::new();
    for p in ports {
        w.u64(p.id).u8(p.output as u8).str(&p.name);
    }
    w.finish()
}

pub fn read_midi_ports(b: &[u8]) -> Vec<MidiPortInfo> {
    let mut r = PackR::new(b);
    let mut out = Vec::new();
    while !r.at_end() {
        let p = MidiPortInfo { id: r.u64(), output: r.u8() != 0, name: r.str() };
        if !r.ok {
            break;
        }
        out.push(p);
    }
    out
}

pub fn pack_u64s(v: &[u64]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn read_u64s(b: &[u8]) -> Vec<u64> {
    b.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().expect("chunk of 8"))).collect()
}

/// The 64-bit id both sides derive from a name (`LiveId::from_str`): device and port ids are
/// names hashed, and the apps persist them.
pub const fn name_hash(name: &str) -> u64 {
    let bytes = name.as_bytes();
    let mut x: u64 = 0xd6e8_feb8_6659_fd93;
    let mut i = 0;
    while i < bytes.len() {
        x = x.overflowing_add(bytes[i] as u64).0;
        x ^= x >> 32;
        x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
        x ^= x >> 32;
        x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
        x ^= x >> 32;
        i += 1;
    }
    x & 0x0000_3fff_ffff_ffff
}

/// `ViewOp::op` low byte: what to do to a native view; the high byte is the `view_kind`.
pub mod view_op {
    /// the view is made (a browser starts loading the url in the payload)
    pub const CREATE: u16 = 0;
    /// placed in the window: `area` is the view's rect in window points (y up from the window's
    /// bottom), the payload the visible part of it (4 f32, a browser clips to it), `extra`
    /// whether it shows
    pub const UPDATE: u16 = 1;
    /// taken out of the window, kept
    pub const DETACH: u16 = 2;
    /// load the url of the payload (`extra`: replace the history entry)
    pub const SET_URL: u16 = 3;
    /// go back or forward in the history by `extra` (an i32)
    pub const HISTORY: u16 = 4;
    /// the view is released
    pub const CLOSE: u16 = 5;
}

/// What a `ViewOp` is about (`ViewOp::op` high byte).
pub mod view_kind {
    /// the system browser (a WKWebView)
    pub const BROWSER: u16 = 1 << 8;
    /// a camera preview of an open capture session
    pub const CAMERA: u16 = 2 << 8;
}

/// How a camera format lays out its pixels (`VideoFormatInfo::pixel`); the names are those the
/// app derives its format ids from (`{width} {height} {Debug} {fps}`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VideoPixel {
    RGB24,
    YUY2,
    NV12,
    YUV420,
    GRAY,
    MJPEG,
    Unsupported(u32),
}

impl VideoPixel {
    pub fn to_code(self) -> (u8, u32) {
        match self {
            VideoPixel::RGB24 => (0, 0),
            VideoPixel::YUY2 => (1, 0),
            VideoPixel::NV12 => (2, 0),
            VideoPixel::YUV420 => (3, 0),
            VideoPixel::GRAY => (4, 0),
            VideoPixel::MJPEG => (5, 0),
            VideoPixel::Unsupported(f) => (6, f),
        }
    }
    pub fn from_code(c: u8, fourcc: u32) -> VideoPixel {
        match c {
            0 => VideoPixel::RGB24,
            1 => VideoPixel::YUY2,
            2 => VideoPixel::NV12,
            3 => VideoPixel::YUV420,
            4 => VideoPixel::GRAY,
            5 => VideoPixel::MJPEG,
            _ => VideoPixel::Unsupported(fourcc),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct VideoFormatInfo {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub frame_rate: Option<f64>,
    pub pixel: VideoPixel,
}

/// A camera as `VideoInputs` lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoInputInfo {
    pub id: u64,
    pub name: String,
    /// Clockwise quarter-turn rotation needed to make the delivered pixels upright.
    pub sensor_orientation: u16,
    pub formats: Vec<VideoFormatInfo>,
}

pub fn pack_video_inputs(inputs: &[VideoInputInfo]) -> Vec<u8> {
    let mut w = PackW::new();
    for i in inputs {
        w.u64(i.id).str(&i.name).u32(i.sensor_orientation as u32).u32(i.formats.len() as u32);
        for f in &i.formats {
            let (code, fourcc) = f.pixel.to_code();
            w.u64(f.id).u32(f.width).u32(f.height).u8(f.frame_rate.is_some() as u8).f64(f.frame_rate.unwrap_or(0.0)).u8(code).u32(fourcc);
        }
    }
    w.finish()
}

pub fn read_video_inputs(b: &[u8]) -> Vec<VideoInputInfo> {
    let mut r = PackR::new(b);
    let mut out = Vec::new();
    while !r.at_end() {
        let (id, name, sensor_orientation, n) = (r.u64(), r.str(), r.u32() as u16, r.u32());
        let mut formats = Vec::new();
        for _ in 0..n {
            if !r.ok {
                break;
            }
            let (fid, width, height, has_rate, rate, code, fourcc) = (r.u64(), r.u32(), r.u32(), r.u8() != 0, r.f64(), r.u8(), r.u32());
            formats.push(VideoFormatInfo { id: fid, width, height, frame_rate: has_rate.then_some(rate), pixel: VideoPixel::from_code(code, fourcc) });
        }
        if !r.ok {
            break;
        }
        out.push(VideoInputInfo { id, name, sensor_orientation, formats });
    }
    out
}

/// `CameraPlane::layout` of a camera frame.
pub mod camera_layout {
    pub const I420: u8 = 0;
    pub const NV12: u8 = 1;
    pub const YUY2: u8 = 2;
    pub const MJPEG: u8 = 3;
    pub const UNKNOWN: u8 = 4;
    pub const RGB24: u8 = 5;
    pub const GRAY: u8 = 6;
}

/// One plane of a camera frame as the `CameraFrame` payload holds it.
pub struct CameraPlane<'a> {
    pub row_stride: u32,
    pub pixel_stride: u32,
    pub bytes: &'a [u8],
}

/// A camera frame as `CameraFrame` carries it: `layout` is `camera_layout::*`, `matrix` the
/// colour matrix selector (0 BT.709, 1 BT.601, 2 BT.2020, 3 unknown).
pub struct CameraFrameData<'a> {
    pub timestamp_ns: u64,
    pub width: u32,
    pub height: u32,
    pub layout: u8,
    pub matrix: u8,
    /// The display's rotation in quarter turns when the frame was taken
    /// (Android's Surface.ROTATION_*): what turns the frame upright on the
    /// screen as it is held now, with the input's `sensor_orientation`.
    /// 0 where the platform does not turn the display under the app.
    pub display_rotation: u8,
    pub planes: Vec<CameraPlane<'a>>,
}

pub fn pack_camera_frame(f: &CameraFrameData) -> Vec<u8> {
    let mut w = PackW::new();
    w.u64(f.timestamp_ns).u32(f.width).u32(f.height).u8(f.layout).u8(f.matrix).u8(f.display_rotation).u8(f.planes.len() as u8);
    for p in &f.planes {
        w.u32(p.row_stride).u32(p.pixel_stride).bytes(p.bytes);
    }
    w.finish()
}

pub fn read_camera_frame(b: &[u8]) -> Option<CameraFrameData<'_>> {
    let mut r = PackR::new(b);
    let (timestamp_ns, width, height, layout, matrix, display_rotation, n) = (r.u64(), r.u32(), r.u32(), r.u8(), r.u8(), r.u8(), r.u8());
    let mut planes = Vec::new();
    for _ in 0..n.min(3) {
        planes.push(CameraPlane { row_stride: r.u32(), pixel_stride: r.u32(), bytes: r.bytes() });
    }
    r.ok.then_some(CameraFrameData { timestamp_ns, width, height, layout, matrix, display_rotation, planes })
}

/// What VideoToolbox can do with H.264 on this machine (`video_h264_probe`).
#[makepad_relax_api::relax_api(plain)]
#[repr(C)]
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct H264Probe {
    pub encode_hardware: bool,
    pub encode_software: bool,
    pub decode_hardware: bool,
    pub decode_software: bool,
}

/// `Haptic::kind`: the Android window's `performHapticFeedback` constants.
pub mod haptic_kind {
    pub const CLICK: u32 = 0;
    pub const VIRTUAL_KEY: u32 = 1;
    pub const TICK: u32 = 2;
}

/// `Location::op`
pub mod location_op {
    /// start the location service (prompts for permission when not determined yet)
    pub const START: u32 = 0;
    pub const STOP: u32 = 1;
}

/// `Location` event kinds.
pub mod location_kind {
    /// a position fix: lon, lat, accuracy, time (unix seconds) and altitude / speed / heading
    /// (NaN when unknown)
    pub const FIX: u32 = 0;
    pub const DENIED: u32 = 1;
    /// no location service, or an OS error; the text says which
    pub const UNAVAILABLE: u32 = 2;
}

/// `Permission::op`
pub mod permission_op {
    /// answer with the current status
    pub const CHECK: u32 = 0;
    /// ask the user when the status is not determined yet, else answer with it
    pub const REQUEST: u32 = 1;
}

/// What a permission is for (`Permission::kind`).
pub mod permission_kind {
    pub const AUDIO_INPUT: u32 = 0;
    pub const CAMERA: u32 = 1;
    pub const HEADSET_CAMERA: u32 = 2;
    pub const SCENE_ACCESS: u32 = 3;
    pub const LOCATION: u32 = 4;
}

/// `Permission` event status.
pub mod permission_status {
    pub const GRANTED: u32 = 0;
    pub const NOT_DETERMINED: u32 = 1;
    pub const DENIED_CAN_RETRY: u32 = 2;
    pub const DENIED_PERMANENT: u32 = 3;
}

/// `GameOut::op`
pub mod game_op {
    /// start polling the controllers (the first `GameDevices` follows); idempotent
    pub const ENABLE: u32 = 0;
    /// one output report (`a` the report id, `data` the bytes) to a raw-HID device: the force
    /// feedback of a wheel
    pub const REPORT: u32 = 1;
    /// a haptic pulse on a pad: `a` left, `b` right, `c` sharpness (f32 bits), `d` seconds (f32 bits)
    pub const PULSE: u32 = 2;
    /// the force target of a device whose force feedback the platform drives
    /// (`game_output::FORCE`): `a` the constant force -1..1, `b` the centring spring 0..1, `c` the
    /// damper 0..1 (f32 bits). A target holds for 100 ms: the app sends it at least that often to
    /// keep a force, a device whose app stopped sending goes slack.
    pub const FORCE: u32 = 3;
}

/// How the app drives a device's force feedback (`game::GameDevice::output`).
pub mod game_output {
    /// it has none the app can drive
    pub const NONE: u8 = 0;
    /// raw output reports (`game_op::REPORT`): the app speaks the device's protocol
    pub const REPORTS: u8 = 1;
    /// the platform's force effects (`game_op::FORCE`): constant force, spring, damper
    pub const FORCE: u8 = 2;
}

/// Game controllers: the state types both sides know. A pad's values are 0..1 (sticks -1..1).
pub mod game {
    use super::{PackR, PackW};

    #[derive(Clone, Debug, PartialEq)]
    pub struct GameInfo {
        pub id: u64,
        pub name: String,
        /// USB ids of a raw-HID device (a wheel's force feedback protocol is keyed on them), else 0
        pub vendor: u32,
        pub product: u32,
    }

    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct GamepadState {
        pub a: f32,
        pub b: f32,
        pub x: f32,
        pub y: f32,
        pub left_shoulder: f32,
        pub right_shoulder: f32,
        pub left_trigger: f32,
        pub right_trigger: f32,
        pub select: f32,
        pub start: f32,
        pub home: f32,
        pub left_thumb: f32,
        pub right_thumb: f32,
        pub dpad_up: f32,
        pub dpad_down: f32,
        pub dpad_left: f32,
        pub dpad_right: f32,
        pub left_stick: [f32; 2],
        pub right_stick: [f32; 2],
    }

    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct WheelState {
        pub steering: f32,
        pub throttle: f32,
        pub brake: f32,
        pub clutch: f32,
        pub steer_force: f32,
        pub buttons: u32,
    }

    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct JoystickState {
        pub x: f32,
        pub y: f32,
        pub twist: f32,
        pub throttle: f32,
        pub hat: u8,
        pub buttons: u32,
    }

    #[derive(Clone, Debug, PartialEq)]
    pub enum GameState {
        Gamepad(GamepadState),
        Wheel(WheelState),
        Joystick(JoystickState),
    }

    /// What a pad's haptics can do.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct HapticCaps {
        pub handles: bool,
        pub separate_handles: bool,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct HapticPulse {
        pub left: f32,
        pub right: f32,
        pub sharpness: f32,
        pub duration_s: f32,
    }

    /// One entry of the ordered device list `GameDevices` carries.
    #[derive(Clone, Debug, PartialEq)]
    pub struct GameDevice {
        pub id: u64,
        pub name: String,
        /// a connection the app is told about (the pads of the GameController framework)
        pub announce: bool,
        pub haptics: HapticCaps,
        pub vendor: u32,
        pub product: u32,
        /// `game_output::*`
        pub output: u8,
    }

    pub fn pack_devices(devices: &[GameDevice]) -> Vec<u8> {
        let mut w = PackW::new();
        for d in devices {
            w.u64(d.id).u8(d.announce as u8).u8(d.haptics.handles as u8).u8(d.haptics.separate_handles as u8).u32(d.vendor).u32(d.product).u8(d.output).str(&d.name);
        }
        w.finish()
    }

    pub fn read_devices(b: &[u8]) -> Vec<GameDevice> {
        let mut r = PackR::new(b);
        let mut out = Vec::new();
        while !r.at_end() {
            let d = GameDevice { id: r.u64(), announce: r.u8() != 0, haptics: HapticCaps { handles: r.u8() != 0, separate_handles: r.u8() != 0 }, vendor: 0, product: 0, output: 0, name: String::new() };
            let d = GameDevice { vendor: r.u32(), product: r.u32(), ..d };
            let d = GameDevice { output: r.u8(), ..d };
            let d = GameDevice { name: r.str(), ..d };
            if !r.ok {
                break;
            }
            out.push(d);
        }
        out
    }

    fn put_f32s(w: &mut PackW, v: &[f32]) {
        for x in v {
            w.u32(x.to_bits());
        }
    }

    pub fn pack_state(state: &GameState) -> Vec<u8> {
        let mut w = PackW::new();
        match state {
            GameState::Gamepad(p) => {
                w.u8(0);
                put_f32s(
                    &mut w,
                    &[
                        p.a, p.b, p.x, p.y, p.left_shoulder, p.right_shoulder, p.left_trigger, p.right_trigger, p.select, p.start, p.home, p.left_thumb, p.right_thumb, p.dpad_up, p.dpad_down,
                        p.dpad_left, p.dpad_right, p.left_stick[0], p.left_stick[1], p.right_stick[0], p.right_stick[1],
                    ],
                );
            }
            GameState::Wheel(v) => {
                w.u8(1);
                put_f32s(&mut w, &[v.steering, v.throttle, v.brake, v.clutch, v.steer_force]);
                w.u32(v.buttons);
            }
            GameState::Joystick(v) => {
                w.u8(2);
                put_f32s(&mut w, &[v.x, v.y, v.twist, v.throttle]);
                w.u8(v.hat).u32(v.buttons);
            }
        }
        w.finish()
    }

    pub fn read_state(b: &[u8]) -> Option<GameState> {
        let mut r = PackR::new(b);
        let kind = r.u8();
        let mut f = || f32::from_bits(r.u32());
        let state = match kind {
            0 => {
                let mut v = [0f32; 21];
                for x in v.iter_mut() {
                    *x = f();
                }
                GameState::Gamepad(GamepadState {
                    a: v[0],
                    b: v[1],
                    x: v[2],
                    y: v[3],
                    left_shoulder: v[4],
                    right_shoulder: v[5],
                    left_trigger: v[6],
                    right_trigger: v[7],
                    select: v[8],
                    start: v[9],
                    home: v[10],
                    left_thumb: v[11],
                    right_thumb: v[12],
                    dpad_up: v[13],
                    dpad_down: v[14],
                    dpad_left: v[15],
                    dpad_right: v[16],
                    left_stick: [v[17], v[18]],
                    right_stick: [v[19], v[20]],
                })
            }
            1 => {
                let v = [f(), f(), f(), f(), f()];
                GameState::Wheel(WheelState { steering: v[0], throttle: v[1], brake: v[2], clutch: v[3], steer_force: v[4], buttons: r.u32() })
            }
            2 => {
                let v = [f(), f(), f(), f()];
                let hat = r.u8();
                GameState::Joystick(JoystickState { x: v[0], y: v[1], twist: v[2], throttle: v[3], hat, buttons: r.u32() })
            }
            _ => return None,
        };
        r.ok.then_some(state)
    }
}

// ------------------------------------------------------------------ payload codecs

/// Appends little-endian values to a payload.
#[derive(Default)]
pub struct PackW(pub Vec<u8>);

impl PackW {
    pub fn new() -> PackW {
        PackW(Vec::new())
    }
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn f64(&mut self, v: f64) -> &mut Self {
        self.u64(v.to_bits())
    }
    pub fn str(&mut self, s: &str) -> &mut Self {
        self.u32(s.len() as u32);
        self.0.extend_from_slice(s.as_bytes());
        self
    }
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
        self
    }
    pub fn finish(self) -> Vec<u8> {
        self.0
    }
}

/// Reads what [`PackW`] wrote. A short payload reads as zeros and empty strings, and `ok`
/// turns false: a damaged payload never panics.
pub struct PackR<'a> {
    b: &'a [u8],
    at: usize,
    pub ok: bool,
}

impl<'a> PackR<'a> {
    pub fn new(b: &'a [u8]) -> PackR<'a> {
        PackR { b, at: 0, ok: true }
    }
    fn take(&mut self, n: usize) -> &'a [u8] {
        match self.b.get(self.at..self.at.saturating_add(n)) {
            Some(s) => {
                self.at += n;
                s
            }
            None => {
                self.ok = false;
                self.at = self.b.len();
                &[]
            }
        }
    }
    pub fn at_end(&self) -> bool {
        self.at >= self.b.len()
    }
    pub fn u8(&mut self) -> u8 {
        self.take(1).first().copied().unwrap_or(0)
    }
    pub fn u32(&mut self) -> u32 {
        self.take(4).try_into().map_or(0, u32::from_le_bytes)
    }
    pub fn u64(&mut self) -> u64 {
        self.take(8).try_into().map_or(0, u64::from_le_bytes)
    }
    pub fn f64(&mut self) -> f64 {
        f64::from_bits(self.u64())
    }
    pub fn str(&mut self) -> String {
        let n = self.u32() as usize;
        String::from_utf8_lossy(self.take(n)).into_owned()
    }
    pub fn bytes(&mut self) -> &'a [u8] {
        let n = self.u32() as usize;
        self.take(n)
    }
}

/// The tracks of a prepared video: the labels of the video tracks, then of the audio tracks.
pub fn pack_tracks(video: &[String], audio: &[String]) -> Vec<u8> {
    let mut w = PackW::new();
    w.u32(video.len() as u32);
    for t in video {
        w.str(t);
    }
    w.u32(audio.len() as u32);
    for t in audio {
        w.str(t);
    }
    w.finish()
}

pub fn read_tracks(b: &[u8]) -> (Vec<String>, Vec<String>) {
    let mut r = PackR::new(b);
    let mut video = Vec::new();
    for _ in 0..r.u32() {
        if !r.ok {
            break;
        }
        video.push(r.str());
    }
    let mut audio = Vec::new();
    for _ in 0..r.u32() {
        if !r.ok {
            break;
        }
        audio.push(r.str());
    }
    (video, audio)
}

/// Time ranges (start, end in seconds) as a payload.
pub fn pack_ranges(ranges: &[(f64, f64)]) -> Vec<u8> {
    let mut w = PackW::new();
    for (a, b) in ranges {
        w.f64(*a).f64(*b);
    }
    w.finish()
}

pub fn read_ranges(b: &[u8]) -> Vec<(f64, f64)> {
    let mut r = PackR::new(b);
    let mut out = Vec::new();
    while !r.at_end() {
        let (a, e) = (r.f64(), r.f64());
        if !r.ok {
            break;
        }
        out.push((a, e));
    }
    out
}
