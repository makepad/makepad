//! The media module of the Apple HAL (macOS, iOS, tvOS): everything of `os/apple` that is not
//! Metal or the window shell (AppKit / UIKit).
//! Video and audio-file playback (AVFoundation) with decoded frames landing in HAL textures
//! named by the app's ids, native views (camera preview, system browser, web view), CoreAudio
//! devices and their realtime entries, CoreMIDI, camera capture and encoders, game controllers,
//! location and permissions. Its commands and events are the media section of `cmds.rs` /
//! `events.rs` (`crate::media` holds the codes and payload codecs).
//!
//! Everything is keyed by session: a relaunch (or a closed domain) stops that session's
//! players, streams and cameras ([`Media::session_closed`]) and leaves the others alone. The
//! module runs inside the Metal backend, under the HAL's state lock: commands arrive in
//! [`Media::command`], the app thread's pump calls [`Media::step`], which polls the players and
//! turns what the OS threads left in mailboxes into events. OS threads (CoreAudio, CoreMIDI,
//! capture queues, location and permission callbacks) never touch the session's event queue
//! directly: they fill a mailbox and ring the session's [`Signal`].
//!
//! The platforms differ per module, by cfg: audio devices (CoreAudio objects and the loopback
//! on macOS, the `AVAudioSession` defaults on iOS / tvOS), raw-HID game input (macOS), the
//! web view (macOS, iOS: tvOS has no WebKit) and location (macOS, iOS: tvOS has no
//! continuous updates).

mod audio;
#[cfg(target_os = "macos")]
mod audio_tap;
mod camera;
pub(super) mod capture;
mod game_input;
#[cfg(not(target_os = "tvos"))]
mod geo;
mod midi;
mod permission;
mod probe;
mod util;
mod video;
mod views;

pub use probe::video_h264_probe;
pub use video::can_play_type;

use crate::cmds::HalCmd;
use crate::media::{audio_dir, game_op, view_kind, view_op, location_kind, location_op, midi_op, permission_kind, permission_op, permission_status, read_u64s, video_mode, video_op, video_status};
use crate::session::{Session, TextureObj};
use crate::types::{format, usage, Id};
use makepad_apple_sys::*;
use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Rings the pump of one session: set from any thread, taken by [`Signal::take`] in `step`.
#[derive(Clone)]
pub struct Signal {
    flag: Arc<AtomicBool>,
    session: u32,
}

impl Signal {
    pub fn new(session: u32) -> Signal {
        Signal { flag: Arc::new(AtomicBool::new(false)), session }
    }

    /// Any thread: something is waiting for the pump of the session.
    pub fn set(&self) {
        self.flag.store(true, Ordering::Release);
        crate::Hal::global().wake_session(self.session);
    }

    /// Whether it was set since the last call.
    pub fn take(&self) -> bool {
        self.flag.swap(false, Ordering::AcqRel)
    }
}

/// Where decoded and captured frames go: the Metal backend's texture storage. The sink installs
/// a Metal texture as the storage of the texture `id` of the session (the app named it), replaces
/// what was there, and keeps the table entry and the ledger row in step.
pub trait Sink {
    /// The Metal device.
    fn device(&self) -> ObjcId;
    /// Makes the table entry and the ledger row of the texture `id` (format `fmt`, no storage
    /// yet): a draw that samples it before the first frame finds a live texture without
    /// storage, which samples as the fallback.
    fn declare(&mut self, s: &mut Session, id: Id, fmt: u32);
    /// `mtl` (not consumed: the sink retains it) becomes the storage of `id`; `keep` lives as long
    /// as the storage (CoreVideo objects the texture is a view of). False when `id` is stale
    /// (the app made a newer texture of the slot): nothing is installed and `keep` is dropped.
    fn adopt(&mut self, s: &mut Session, id: Id, mtl: ObjcId, w: u32, h: u32, fmt: u32, keep: Option<Box<dyn Any + Send>>) -> bool;
    /// A 1x1 R8 texture for `id` (the V plane a biplanar shader still binds).
    fn dummy_plane(&mut self, s: &mut Session, id: Id) -> bool;
    /// Gives the storage of `id` back; its table entry stays until the app destroys the texture.
    fn drop_texture(&mut self, s: &mut Session, id: Id);
}

/// The table entry of a texture the media module owns the storage of.
pub(crate) fn external_texture(fmt: u32, w: u32, h: u32) -> TextureObj {
    TextureObj {
        format: fmt,
        w,
        h,
        depth: 1,
        mips: 1,
        usage: usage::SAMPLED,
        bytes: format::level_bytes(fmt, w, h),
        imported: true,
        uploaded: 0,
        lent: None,
    }
}

/// What one session has running.
struct SessionMedia {
    players: HashMap<u64, video::Player>,
    audio: Option<Arc<Mutex<audio::AudioUnitAccess>>>,
    audio_change: Signal,
    midi: Option<Arc<Mutex<midi::CoreMidiAccess>>>,
    midi_change: Signal,
    midi_data: Signal,
    game: Option<game_input::GameInput>,
    /// rings when a controller connects or any value changed
    game_change: Signal,
    #[cfg(not(target_os = "tvos"))]
    geo: Option<geo::Geo>,
    /// rings when the location service or a permission answered
    system: Signal,
    permissions: permission::Answers,
    camera: Option<Arc<Mutex<camera::AvCaptureAccess>>>,
    camera_change: Signal,
    camera_frames: Signal,
    /// camera previews (a player of camera source): the (camera, format) each shows
    previews: HashMap<u64, (u64, u64)>,
}

impl SessionMedia {
    fn new(sid: u32) -> SessionMedia {
        SessionMedia {
            players: HashMap::new(),
            audio: None,
            audio_change: Signal::new(sid),
            midi: None,
            midi_change: Signal::new(sid),
            midi_data: Signal::new(sid),
            game: None,
            game_change: Signal::new(sid),
            #[cfg(not(target_os = "tvos"))]
            geo: None,
            system: Signal::new(sid),
            permissions: Default::default(),
            camera: None,
            camera_change: Signal::new(sid),
            camera_frames: Signal::new(sid),
            previews: HashMap::new(),
        }
    }

    /// The session's CoreAudio access, made (and announcing the devices) on first use.
    fn audio(&mut self) -> Arc<Mutex<audio::AudioUnitAccess>> {
        if self.audio.is_none() {
            self.audio = Some(audio::AudioUnitAccess::new(self.audio_change.clone()));
            self.audio_change.set();
        }
        self.audio.as_ref().expect("made above").clone()
    }

    /// The session's capture access, made (and listing the cameras) on first use.
    fn camera(&mut self) -> Arc<Mutex<camera::AvCaptureAccess>> {
        if self.camera.is_none() {
            self.camera = Some(camera::AvCaptureAccess::new(self.camera_change.clone(), self.camera_frames.clone()));
            self.camera_change.set();
        }
        self.camera.as_ref().expect("made above").clone()
    }

    fn midi(&mut self) -> Arc<Mutex<midi::CoreMidiAccess>> {
        if self.midi.is_none() {
            self.midi = Some(midi::CoreMidiAccess::new(self.midi_change.clone(), self.midi_data.clone()));
            self.midi_change.set();
        }
        self.midi.as_ref().expect("made above").clone()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Default)]
pub struct Media {
    sessions: HashMap<u32, SessionMedia>,
}

/// Polling period of the players while any plays or waits for a frame (about the display
/// refresh of a 120 Hz panel, as the display link beat the app polled on before).
const POLL_NS: u64 = 8_000_000;

impl Media {
    pub fn new() -> Media {
        Media::default()
    }

    /// Executes a media command. False when the command is not one of this module's.
    pub fn command(&mut self, sink: &mut dyn Sink, s: &mut Session, c: &HalCmd) -> bool {
        match *c {
            HalCmd::VideoPrepare { tex, mode, video, y, u, v, src } => {
                self.video_prepare(sink, s, tex, mode, video, [y, u, v], src);
                true
            }
            HalCmd::VideoControl { video, op, arg } => {
                self.video_control(sink, s, video, op, arg);
                true
            }
            HalCmd::AudioUse { dir, options, devices } => {
                let sm = self.sessions.entry(s.id).or_insert_with(|| SessionMedia::new(s.id));
                let access = sm.audio();
                let ids = read_u64s(devices);
                let mut access = lock(&access);
                if dir == audio_dir::INPUT {
                    access.use_audio_inputs(&ids, options);
                } else {
                    access.use_audio_outputs(&ids);
                }
                true
            }
            HalCmd::AudioEntry { dir, index, entry, ctx } => {
                let sm = self.sessions.entry(s.id).or_insert_with(|| SessionMedia::new(s.id));
                let access = sm.audio();
                let index = index as usize;
                if index >= audio::MAX_AUDIO_DEVICE_INDEX {
                    s.bad();
                    return true;
                }
                let slot = {
                    let access = lock(&access);
                    if dir == audio_dir::INPUT { access.input_cb[index].clone() } else { access.output_cb[index].clone() }
                };
                // SAFETY: a non-zero `entry` is the address of the `extern "C" fn` the app registered
                let entry = (entry != 0).then(|| audio::Entry { f: unsafe { std::mem::transmute::<usize, crate::media::AudioFn>(entry as usize) }, ctx: ctx as usize });
                // waits for a block of the old entry that is running: when this returns the old
                // entry is not called any more
                *lock(&slot) = entry;
                true
            }
            HalCmd::Midi { op, port, data, ports } => {
                let sm = self.sessions.entry(s.id).or_insert_with(|| SessionMedia::new(s.id));
                let access = sm.midi();
                let access = lock(&access);
                match op {
                    midi_op::OPEN => {}
                    midi_op::USE_INPUTS => access.use_midi_inputs(&read_u64s(ports)),
                    midi_op::USE_OUTPUTS => {}
                    midi_op::RESET => access.midi_reset(),
                    midi_op::SEND => access.send(port, data),
                    _ => s.bad(),
                }
                true
            }
            HalCmd::GameOut { id, op, a, b, c, d, data } => {
                let sm = self.sessions.entry(s.id).or_insert_with(|| SessionMedia::new(s.id));
                match op {
                    game_op::ENABLE => {
                        if sm.game.is_none() {
                            sm.game = Some(game_input::GameInput::new(sm.game_change.clone()));
                        }
                        sm.game_change.set();
                    }
                    game_op::REPORT => {
                        if let Some(game) = &sm.game {
                            game.send_report(id, a as u8, data);
                        }
                    }
                    game_op::PULSE => {
                        if let Some(game) = &mut sm.game {
                            let f = f32::from_bits;
                            game.pulse(id, crate::media::game::HapticPulse { left: f(a), right: f(b), sharpness: f(c), duration_s: f(d) });
                        }
                    }
                    _ => s.bad(),
                }
                true
            }
            HalCmd::ViewOp { window, op, view, area, extra, data } => {
                let sm = self.sessions.entry(s.id).or_insert_with(|| SessionMedia::new(s.id));
                // a camera preview shows the capture session of its camera
                let mut session = 0usize;
                if op & 0xff00 == view_kind::CAMERA && op & 0xff == view_op::UPDATE {
                    if let (Some(&(input, format)), Some(camera)) = (sm.previews.get(&view), &sm.camera) {
                        if let Some(found) = lock(camera).session_for(input, format) {
                            unsafe {
                                let _: ObjcId = msg_send![found, retain];
                            }
                            session = found as usize;
                        }
                    }
                }
                let cmd = views::ViewCmd { sid: s.id, window, op, view, area, extra, data: data.to_vec(), session };
                // native views live on the window system's main thread; offscreen sessions have none
                if !super::shell::on_main(move || views::apply(cmd)) {
                    s.stats.media_unsupported += 1;
                }
                true
            }
            HalCmd::VideoInput { consumers, inputs } => {
                let sm = self.sessions.entry(s.id).or_insert_with(|| SessionMedia::new(s.id));
                let camera = sm.camera();
                let ids = read_u64s(inputs);
                let pairs: Vec<(u64, u64)> = ids.chunks_exact(2).map(|p| (p[0], p[1])).collect();
                lock(&camera).use_video_input(&pairs, consumers);
                true
            }
            #[cfg(not(target_os = "tvos"))]
            HalCmd::Location { op } => {
                let sm = self.sessions.entry(s.id).or_insert_with(|| SessionMedia::new(s.id));
                let geo = sm.geo.get_or_insert_with(|| geo::Geo::new(sm.system.clone()));
                match op {
                    location_op::START => geo.start(),
                    location_op::STOP => geo.stop(),
                    _ => s.bad(),
                }
                true
            }
            // tvOS: CoreLocation has no continuous updates there
            #[cfg(target_os = "tvos")]
            HalCmd::Location { op } => {
                match op {
                    location_op::START => s.events.location(location_kind::UNAVAILABLE, 0.0, 0.0, 0.0, 0.0, f32::NAN, f32::NAN, f32::NAN, b"no location updates on tvOS"),
                    location_op::STOP => {}
                    _ => s.bad(),
                }
                true
            }
            HalCmd::Permission { op, kind, req } => {
                let sm = self.sessions.entry(s.id).or_insert_with(|| SessionMedia::new(s.id));
                let current = permission::check(kind);
                let ask = op == permission_op::REQUEST && current == permission_status::NOT_DETERMINED;
                match (ask, kind) {
                    (true, permission_kind::AUDIO_INPUT | permission_kind::CAMERA) => permission::request_av(kind, req, sm.permissions.clone(), sm.system.clone()),
                    #[cfg(not(target_os = "tvos"))]
                    (true, permission_kind::LOCATION) => sm.geo.get_or_insert_with(|| geo::Geo::new(sm.system.clone())).request_permission(req as i32),
                    _ => s.events.permission(req, kind, current),
                }
                true
            }
            _ => false,
        }
    }

    fn video_prepare(&mut self, sink: &mut dyn Sink, s: &mut Session, tex: Id, mode: u16, video: u64, planes: [Id; 3], src: &[u8]) {
        let sm = self.sessions.entry(s.id).or_insert_with(|| SessionMedia::new(s.id));
        if mode & video_mode::CAMERA != 0 {
            // a camera preview: the camera runs for the preview only (no frames to the app); the
            // native view comes with the `ViewOp`s
            let ids = read_u64s(src);
            let (Some(&input), Some(&format)) = (ids.first(), ids.get(1)) else {
                s.bad();
                return;
            };
            let camera = sm.camera();
            let size = {
                let mut camera = lock(&camera);
                camera.use_video_input(&[(input, format)], 0);
                camera.format_size(input, format).unwrap_or((0, 0))
            };
            sm.previews.insert(video, (input, format));
            let tracks = crate::media::pack_tracks(&["camera".to_string()], &[]);
            s.events.video_prepared(video, [size.0 as u16, size.1 as u16], 0, false, &tracks);
            return;
        }
        // a new prepare of the same id replaces the old player
        if let Some(mut old) = sm.players.remove(&video) {
            Self::release_player(sink, s, &mut old);
        }
        let kind = (mode & video_mode::SRC_MASK) >> video_mode::SRC_SHIFT;
        let source = match kind {
            crate::media::src_kind::NETWORK => video::Source::Network(String::from_utf8_lossy(src).into_owned()),
            crate::media::src_kind::FILE => video::Source::File(String::from_utf8_lossy(src).into_owned()),
            _ => video::Source::Memory(src.to_vec()),
        };
        let targets = video::Targets { bgra: tex, y: planes[0], u: planes[1], v: planes[2] };
        for (id, fmt) in [(tex, format::BGRA8), (planes[0], format::R8), (planes[1], format::RG8), (planes[2], format::R8)] {
            if !id.is_none() {
                sink.declare(s, id, fmt);
            }
        }
        let player = video::Player::new(sink.device(), targets, source, mode & video_mode::AUTOPLAY != 0, mode & video_mode::LOOP != 0);
        sm.players.insert(video, player);
    }

    fn release_player(sink: &mut dyn Sink, s: &mut Session, player: &mut video::Player) {
        player.cleanup();
        let t = player.targets();
        for id in [t.bgra, t.y, t.u, t.v] {
            if !id.is_none() {
                sink.drop_texture(s, id);
            }
        }
    }

    fn video_control(&mut self, sink: &mut dyn Sink, s: &mut Session, video: u64, op: u32, arg: u64) {
        let Some(sm) = self.sessions.get_mut(&s.id) else { return };
        if op == video_op::CLEANUP && sm.previews.remove(&video).is_some() {
            if let Some(camera) = &sm.camera {
                lock(camera).use_video_input(&[], 0);
            }
            let (sid, window) = (s.id, Id::NONE);
            let cmd = views::ViewCmd { sid, window, op: view_kind::CAMERA | view_op::CLOSE, view: video, area: [0.0; 4], extra: 0, data: Vec::new(), session: 0 };
            super::shell::on_main(move || views::apply(cmd));
            s.events.video_status(video, video_status::RELEASED, b"");
            return;
        }
        if op == video_op::CLEANUP {
            if let Some(mut player) = sm.players.remove(&video) {
                Self::release_player(sink, s, &mut player);
                s.events.video_status(video, video_status::RELEASED, b"");
            }
            return;
        }
        let Some(p) = sm.players.get_mut(&video) else { return };
        match op {
            video_op::PLAY => p.play(),
            video_op::PAUSE => p.pause(),
            video_op::RESUME => p.resume(),
            video_op::MUTE => p.mute(true),
            video_op::UNMUTE => p.mute(false),
            video_op::SEEK => p.seek_to(arg),
            video_op::VOLUME => p.set_volume(f32::from_bits(arg as u32) as f64),
            video_op::RATE => p.set_playback_rate(f32::from_bits(arg as u32) as f64),
            // track selection is implemented on GStreamer-based backends only
            video_op::SELECT_VIDEO_TRACK | video_op::SELECT_AUDIO_TRACK => {}
            _ => s.bad(),
        }
    }

    /// One step of the app's pump: poll the players for preparation and new frames, turn what
    /// the OS threads left for the session into events.
    pub fn step(&mut self, sink: &mut dyn Sink, s: &mut Session) {
        let Some(sm) = self.sessions.get_mut(&s.id) else { return };
        if sm.camera_change.take() {
            if let Some(camera) = &sm.camera {
                let inputs = lock(camera).get_updated_descs();
                s.events.video_inputs(&crate::media::pack_video_inputs(&inputs.iter().map(|i| i.clone()).collect::<Vec<_>>()));
            }
        }
        if sm.camera_frames.take() {
            if let Some(camera) = &sm.camera {
                let frames = lock(camera).take_frames();
                for frame in frames {
                    s.events.camera_frame(frame.slots, &frame.payload);
                }
            }
        }
        if sm.system.take() {
            for (req, kind, status) in std::mem::take(&mut *lock(&sm.permissions)) {
                s.events.permission(req, kind, status);
            }
            #[cfg(not(target_os = "tvos"))]
            if let Some(geo) = &sm.geo {
                for msg in geo.take() {
                    match msg {
                        geo::GeoMsg::Fix { lon, lat, accuracy_m, altitude, speed, heading, time } => {
                            let f = |v: Option<f64>| v.map_or(f32::NAN, |v| v as f32);
                            s.events.location(location_kind::FIX, lon, lat, accuracy_m, time, f(altitude), f(speed), f(heading), b"")
                        }
                        geo::GeoMsg::Denied => s.events.location(location_kind::DENIED, 0.0, 0.0, 0.0, 0.0, f32::NAN, f32::NAN, f32::NAN, b""),
                        geo::GeoMsg::Unavailable(why) => s.events.location(location_kind::UNAVAILABLE, 0.0, 0.0, 0.0, 0.0, f32::NAN, f32::NAN, f32::NAN, why.as_bytes()),
                        geo::GeoMsg::Permission(req, status) => {
                            s.events.permission(req as u32, permission_kind::LOCATION, geo::auth_status_to_granted(status))
                        }
                    }
                }
            }
        }
        if sm.game_change.take() {
            if let Some(game) = &mut sm.game {
                let (list_changed, changed) = game.step();
                if list_changed {
                    s.events.game_devices(&crate::media::game::pack_devices(&game.device_list()));
                }
                for i in changed {
                    if let (Some(info), Some(state)) = (game.infos.get(i), game.states.get(i)) {
                        s.events.game_input(info.id, &crate::media::game::pack_state(state));
                    }
                }
            }
        }
        if sm.audio_change.take() {
            if let Some(audio) = &sm.audio {
                let descs = lock(audio).get_updated_descs();
                s.events.audio_devices(&crate::media::pack_audio_devices(&descs));
            }
        }
        if sm.midi_change.take() {
            if let Some(midi) = &sm.midi {
                let ports = lock(midi).get_updated_descs();
                s.events.midi_ports(&crate::media::pack_midi_ports(&ports));
            }
        }
        if sm.midi_data.take() {
            if let Some(midi) = &sm.midi {
                let inbox = lock(midi).inbox.clone();
                let messages = std::mem::take(&mut *lock(&inbox));
                for (port, data) in messages {
                    s.events.midi_data(port, data);
                }
            }
        }
        let ids: Vec<u64> = sm.players.iter().filter(|(_, p)| p.needs_poll()).map(|(id, _)| *id).collect();
        for video in ids {
            let Some(p) = sm.players.get_mut(&video) else { continue };
            match p.check_prepared() {
                Some(Ok(info)) => {
                    let tracks = crate::media::pack_tracks(&info.video_tracks, &info.audio_tracks);
                    s.events.video_prepared(video, [info.width as u16, info.height as u16], info.duration_ms, info.seekable, &tracks);
                    let seekable = p.seekable_ranges();
                    if !seekable.is_empty() {
                        s.events.video_ranges(video, crate::media::ranges::SEEKABLE, &crate::media::pack_ranges(&seekable));
                    }
                    let buffered = p.buffered_ranges();
                    if !buffered.is_empty() {
                        s.events.video_ranges(video, crate::media::ranges::BUFFERED, &crate::media::pack_ranges(&buffered));
                    }
                }
                Some(Err(err)) => {
                    s.events.video_status(video, video_status::ERROR, err.as_bytes());
                }
                None => {}
            }
            if p.poll_frame(s, sink) {
                let info = p.frame_info();
                let mut bits = 0;
                if info.yuv {
                    bits |= crate::media::yuv::ENABLED | crate::media::yuv::BIPLANAR;
                }
                if info.full_range {
                    bits |= crate::media::yuv::FULL_RANGE;
                }
                // the colour matrix of AVFoundation frames is BT.709 (selector 0)
                s.events.video_frame(video, p.current_position_ms(), bits, 0.0);
            }
        }
    }

    /// The nearest time (HAL ns) the media module wants a step, if it does.
    pub fn next_due(&self, sid: u32, now: u64) -> Option<u64> {
        let sm = self.sessions.get(&sid)?;
        sm.players.values().any(|p| p.needs_poll()).then_some(now + POLL_NS)
    }

    /// The session is closing: everything it started stops.
    pub fn session_closed(&mut self, sid: u32) {
        if let Some(mut sm) = self.sessions.remove(&sid) {
            for (_, mut p) in sm.players.drain() {
                p.cleanup();
            }
            // the session's native views leave its windows
            let sid_closing = sid;
            super::shell::on_main(move || views::close_session(sid_closing));
            // the entries go first: a block of the realtime thread that is running finishes, no
            // later one calls into the session (which is gone)
            if let Some(audio) = sm.audio.take() {
                let access = lock(&audio);
                for slot in access.input_cb.iter().chain(access.output_cb.iter()) {
                    *lock(slot) = None;
                }
            }
        }
    }
}
