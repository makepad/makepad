//! The media module of the Windows HAL: everything of the old os/windows backend that is not
//! D3D11 or Win32. Video and audio-file playback (Media Foundation) with decoded frames landing in
//! HAL textures named by the app's ids, WASAPI devices and their realtime entries, WinRT MIDI,
//! camera capture, game controllers (XInput, DirectInput wheels with force feedback) and
//! permissions. Its commands and events are the media section of `cmds.rs` / `events.rs`
//! (`crate::media` holds the codes and payload codecs).
//!
//! Everything is keyed by session: a relaunch (or a closed domain) stops that session's players,
//! streams and cameras ([`Media::session_closed`]) and leaves the others alone. The module runs
//! inside the D3D11 backend, under the HAL's state lock: commands arrive in [`Media::command`],
//! the app thread's pump calls [`Media::step`], which turns what the service threads left in
//! mailboxes into events. Every service owns long-lived worker threads (COM MTA) that make all
//! the OS calls; the HAL's thread never waits on them. Workers never touch the session's event
//! queue: they fill a mailbox and ring the session's [`Signal`].
//!
//! Not on Windows, answered as the protocol says: native views (system browser, camera preview
//! views: `media_unsupported`), camera previews as videos (`VideoStatus` ERROR), location
//! (`Location` UNAVAILABLE), raw-HID output reports (`media_unsupported`).

mod audio;
mod camera;
mod game_input;
mod midi;
mod video;

pub use video::can_play_type;

use super::d3d11::device::Device;
use super::d3d11::resources::Texture;
use super::d3d11::Tex;
use crate::cmds::HalCmd;
use crate::ledger::{Kind, LedgerEntry};
use crate::media::{audio_dir, game_op, location_kind, location_op, midi_op, permission_kind, permission_op, permission_status, read_u64s, video_mode, video_op, video_status};
use crate::session::{Session, TextureObj, World};
use crate::types::{format, usage, Attr, Id};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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

/// COM on the calling (service) thread, multithreaded apartment, for as long as this lives.
pub(super) struct Mta(bool);

impl Mta {
    pub(super) fn enter() -> Mta {
        let hr = unsafe { makepad_windows_sys::Win32::System::Com::CoInitializeEx(None, makepad_windows_sys::Win32::System::Com::COINIT_MULTITHREADED) };
        Mta(hr.0 >= 0)
    }
}

impl Drop for Mta {
    fn drop(&mut self) {
        if self.0 {
            unsafe { makepad_windows_sys::Win32::System::Com::CoUninitialize() };
        }
    }
}

/// The table entry of a texture the media module owns the storage of.
fn external_texture(fmt: u32, w: u32, h: u32) -> TextureObj {
    TextureObj { format: fmt, w, h, depth: 1, mips: 1, usage: usage::SAMPLED, bytes: format::level_bytes(fmt, w, h), imported: true, uploaded: 0, lent: None }
}

/// Where decoded frames go: the D3D11 backend's texture storage. Installing a texture keeps the
/// session's table entry (size, format) and the ledger row in step with it.
pub(crate) struct Sink<'a> {
    pub(crate) dev: &'a Arc<Device>,
    pub(crate) tex: &'a mut HashMap<(u32, u32), Tex>,
    pub(crate) now: u64,
}

impl Sink<'_> {
    fn set_row(&self, s: &mut Session, id: Id, fmt: u32, w: u32, h: u32, bytes: u64) {
        let (now, frame) = (self.now, s.frame);
        match s.ledger.get_mut(Kind::Texture, id.index).filter(|l| l.gen == id.gen) {
            Some(l) => {
                l.bytes = bytes;
                l.format = fmt;
                l.width = w;
                l.height = h;
            }
            None => s.ledger.add(LedgerEntry {
                kind: Kind::Texture,
                detail: "video frame",
                index: id.index,
                gen: id.gen,
                bytes,
                format: fmt,
                width: w,
                height: h,
                mips: 1,
                attr: Attr::default(),
                created_ns: now,
                created_frame: frame,
                last_used_frame: 0,
                last_used_ns: 0,
            }),
        }
    }

    /// Makes the table entry of `id` with a 1x1 storage of `fmt`: a draw that samples it before
    /// the first frame (or a plane the frame format leaves unused) samples black.
    fn declare(&mut self, s: &mut Session, id: Id, fmt: u32) {
        if id.is_none() || s.textures.get(s.id, id).is_ok() {
            return;
        }
        let obj = external_texture(fmt, 1, 1);
        if s.textures.insert(s.id, id, s.id, obj.clone()).is_err() {
            s.stale();
            return;
        }
        match Texture::new(self.dev.clone(), obj) {
            Ok(t) => {
                let bytes = t.bytes;
                self.tex.insert((s.id, id.index), Tex { gen: id.gen, tex: t });
                self.set_row(s, id, fmt, 1, 1, bytes);
            }
            Err(e) => eprintln!("[hal-media] video: placeholder texture: {e}"),
        }
    }

    /// `t` becomes the storage of `id`. False when `id` is stale (the app destroyed it or made
    /// a newer texture of the slot).
    fn adopt(&mut self, s: &mut Session, id: Id, t: Arc<Texture>) -> bool {
        let Ok(entry) = s.textures.get_mut(s.id, id) else {
            s.stale();
            return false;
        };
        let (fmt, w, h, bytes) = (t.desc.format, t.desc.w, t.desc.h, t.bytes);
        entry.val = external_texture(fmt, w, h);
        entry.val.bytes = bytes;
        self.tex.insert((s.id, id.index), Tex { gen: id.gen, tex: t });
        self.set_row(s, id, fmt, w, h, bytes);
        true
    }

    /// Gives the storage of `id` back; its table entry stays until the app destroys the texture.
    fn drop_texture(&mut self, s: &mut Session, id: Id) {
        if self.tex.get(&(s.id, id.index)).is_some_and(|t| t.gen == id.gen) {
            self.tex.remove(&(s.id, id.index));
            if let Some(l) = s.ledger.get_mut(Kind::Texture, id.index).filter(|l| l.gen == id.gen) {
                l.bytes = 0;
            }
        }
    }
}

/// What one session has running.
struct SessionMedia {
    players: HashMap<u64, video::Player>,
    /// rings when a player's worker posted something
    video: Signal,
    audio: Option<audio::WasapiAccess>,
    audio_change: Signal,
    midi: Option<midi::WinRtMidiAccess>,
    midi_change: Signal,
    midi_data: Signal,
    game: Option<game_input::GameInput>,
    game_change: Signal,
    camera: Option<camera::CameraAccess>,
    camera_change: Signal,
    camera_frames: Signal,
}

impl SessionMedia {
    fn new(sid: u32) -> SessionMedia {
        SessionMedia {
            players: HashMap::new(),
            video: Signal::new(sid),
            audio: None,
            audio_change: Signal::new(sid),
            midi: None,
            midi_change: Signal::new(sid),
            midi_data: Signal::new(sid),
            game: None,
            game_change: Signal::new(sid),
            camera: None,
            camera_change: Signal::new(sid),
            camera_frames: Signal::new(sid),
        }
    }

    /// The session's WASAPI service, made (and listing the devices) on first use.
    fn audio(&mut self) -> &audio::WasapiAccess {
        self.audio.get_or_insert_with(|| audio::WasapiAccess::new(self.audio_change.clone()))
    }

    fn midi(&mut self) -> &midi::WinRtMidiAccess {
        self.midi.get_or_insert_with(|| midi::WinRtMidiAccess::new(self.midi_change.clone(), self.midi_data.clone()))
    }

    fn camera(&mut self) -> &camera::CameraAccess {
        self.camera.get_or_insert_with(|| camera::CameraAccess::new(self.camera_change.clone(), self.camera_frames.clone()))
    }
}

#[derive(Default)]
pub struct Media {
    sessions: HashMap<u32, SessionMedia>,
}

impl Media {
    pub fn new() -> Media {
        Media::default()
    }

    /// Executes a media command. False when the command is not one of this module's.
    pub(crate) fn command(&mut self, sink: &mut Sink, s: &mut Session, c: &HalCmd) -> bool {
        match *c {
            HalCmd::VideoPrepare { tex, mode, video, y, u, v, src } => {
                self.video_prepare(sink, s, tex, mode, video, [y, u, v], src);
                true
            }
            HalCmd::VideoControl { video, op, arg } => {
                self.video_control(sink, s, video, op, arg);
                true
            }
            HalCmd::AudioUse { dir, options: _, devices } => {
                // echo cancellation (`audio_option::ECHO_CANCELLATION`) has no WASAPI stream
                // equivalent here: the capture is the device's own
                let sm = self.sessions.entry(s.id).or_insert_with(|| SessionMedia::new(s.id));
                let ids = read_u64s(devices);
                if dir == audio_dir::INPUT {
                    sm.audio().use_inputs(&ids);
                } else {
                    sm.audio().use_outputs(&ids);
                }
                true
            }
            HalCmd::AudioEntry { dir, index, entry, ctx } => {
                let sm = self.sessions.entry(s.id).or_insert_with(|| SessionMedia::new(s.id));
                let index = index as usize;
                if index >= audio::MAX_AUDIO_DEVICE_INDEX {
                    s.bad();
                    return true;
                }
                let access = sm.audio();
                let slot = if dir == audio_dir::INPUT { &access.input_cb[index] } else { &access.output_cb[index] };
                // SAFETY: a non-zero `entry` is the address of the `extern "C" fn` the app registered
                let entry = (entry != 0).then(|| audio::Entry { f: unsafe { std::mem::transmute::<usize, crate::media::AudioFn>(entry as usize) }, ctx: ctx as usize });
                // waits for a block of the old entry that is running: when this returns the old
                // entry is not called any more
                *slot.lock().unwrap_or_else(|e| e.into_inner()) = entry;
                true
            }
            HalCmd::Midi { op, port, data, ports } => {
                let sm = self.sessions.entry(s.id).or_insert_with(|| SessionMedia::new(s.id));
                let midi = sm.midi();
                match op {
                    midi_op::OPEN => {}
                    midi_op::USE_INPUTS => midi.use_inputs(&read_u64s(ports)),
                    midi_op::USE_OUTPUTS => midi.use_outputs(&read_u64s(ports)),
                    midi_op::RESET => midi.reset(),
                    midi_op::SEND => midi.send(port, data),
                    _ => s.bad(),
                }
                true
            }
            HalCmd::GameOut { id, op, a, b, c, d, data: _ } => {
                let sm = self.sessions.entry(s.id).or_insert_with(|| SessionMedia::new(s.id));
                let f = f32::from_bits;
                match op {
                    game_op::ENABLE => {
                        if sm.game.is_none() {
                            sm.game = Some(game_input::GameInput::new(sm.game_change.clone()));
                        }
                        sm.game_change.set();
                    }
                    // no raw-HID output path on Windows: DirectInput drives the wheels' forces
                    game_op::REPORT => s.stats.media_unsupported += 1,
                    game_op::PULSE => {
                        if !sm.game.as_ref().is_some_and(|g| g.pulse(id, crate::media::game::HapticPulse { left: f(a), right: f(b), sharpness: f(c), duration_s: f(d) })) {
                            s.stats.media_unsupported += 1;
                        }
                    }
                    game_op::FORCE => {
                        if !sm.game.as_ref().is_some_and(|g| g.force(id, f(a), f(b), f(c))) {
                            s.stats.media_unsupported += 1;
                        }
                    }
                    _ => s.bad(),
                }
                true
            }
            HalCmd::ViewOp { .. } => {
                // no native views (system browser, camera preview) on Windows
                s.stats.media_unsupported += 1;
                true
            }
            HalCmd::VideoInput { consumers, inputs } => {
                let sm = self.sessions.entry(s.id).or_insert_with(|| SessionMedia::new(s.id));
                let ids = read_u64s(inputs);
                let pairs: Vec<(u64, u64)> = ids.chunks_exact(2).map(|p| (p[0], p[1])).collect();
                sm.camera().use_video_input(&pairs, consumers);
                true
            }
            HalCmd::Location { op } => {
                match op {
                    location_op::START => s.events.location(location_kind::UNAVAILABLE, 0.0, 0.0, 0.0, 0.0, f32::NAN, f32::NAN, f32::NAN, b"no location service on Windows"),
                    location_op::STOP => {}
                    _ => s.bad(),
                }
                true
            }
            HalCmd::Permission { op, kind, req } => {
                if op != permission_op::CHECK && op != permission_op::REQUEST {
                    s.bad();
                    return true;
                }
                // desktop apps are not asked: microphone and camera access is the system's privacy
                // setting, and a denied device fails to open (its `has_failed` / no frames); the
                // services Windows has no implementation of here are denied for good
                let status = match kind {
                    permission_kind::AUDIO_INPUT | permission_kind::CAMERA => permission_status::GRANTED,
                    _ => permission_status::DENIED_PERMANENT,
                };
                s.events.permission(req, kind, status);
                true
            }
            _ => false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn video_prepare(&mut self, sink: &mut Sink, s: &mut Session, tex: Id, mode: u16, video: u64, planes: [Id; 3], src: &[u8]) {
        let sm = self.sessions.entry(s.id).or_insert_with(|| SessionMedia::new(s.id));
        if mode & video_mode::CAMERA != 0 {
            s.events.video_status(video, video_status::ERROR, b"camera preview views are not available on Windows; read the camera's frames instead");
            s.stats.media_unsupported += 1;
            return;
        }
        // a new prepare of the same id replaces the old player
        if let Some(old) = sm.players.remove(&video) {
            Self::release_player(sink, s, old);
        }
        let kind = (mode & video_mode::SRC_MASK) >> video_mode::SRC_SHIFT;
        if super::win32::log_enabled() {
            eprintln!("[hal-media] video {video:x} prepare: source kind {kind}, mode {mode:#x}, {} bytes", src.len());
        }
        let source = video::Source::new(kind, src);
        let targets = video::Targets { bgra: tex, y: planes[0], u: planes[1], v: planes[2] };
        for (id, fmt) in [(tex, format::BGRA8), (planes[0], format::R8), (planes[1], format::RG8), (planes[2], format::R8)] {
            sink.declare(s, id, fmt);
        }
        let audio_only = mode & video_mode::AUDIO_ONLY != 0;
        let player = video::Player::new(&sink.dev.device, targets, source, video, mode & video_mode::AUTOPLAY != 0, mode & video_mode::LOOP != 0, audio_only, sm.video.clone());
        sm.players.insert(video, player);
    }

    fn release_player(sink: &mut Sink, s: &mut Session, player: video::Player) {
        let t = player.targets;
        drop(player);
        for id in [t.bgra, t.y, t.u, t.v] {
            if !id.is_none() {
                sink.drop_texture(s, id);
            }
        }
    }

    fn video_control(&mut self, sink: &mut Sink, s: &mut Session, video: u64, op: u32, arg: u64) {
        if super::win32::log_enabled() {
            eprintln!("[hal-media] video {video:x} control op {op} arg {arg}");
        }
        let Some(sm) = self.sessions.get_mut(&s.id) else { return };
        if op == video_op::CLEANUP {
            if let Some(player) = sm.players.remove(&video) {
                Self::release_player(sink, s, player);
                s.events.video_status(video, video_status::RELEASED, b"");
            }
            return;
        }
        let Some(p) = sm.players.get_mut(&video) else { return };
        match op {
            video_op::PLAY | video_op::RESUME => p.play(),
            video_op::PAUSE => p.pause(),
            video_op::MUTE => p.mute(true),
            video_op::UNMUTE => p.mute(false),
            video_op::SEEK => p.seek(arg),
            video_op::VOLUME => p.volume(f32::from_bits(arg as u32) as f64),
            video_op::RATE => p.rate(f32::from_bits(arg as u32) as f64),
            // the players here have one video and one audio track
            video_op::SELECT_VIDEO_TRACK | video_op::SELECT_AUDIO_TRACK => s.stats.media_unsupported += 1,
            _ => s.bad(),
        }
    }

    /// One step of the app's pump: what the service threads left for the session becomes events,
    /// decoded frames become the storage of the players' textures.
    pub(crate) fn step(&mut self, sink: &mut Sink, s: &mut Session) {
        let Some(sm) = self.sessions.get_mut(&s.id) else { return };
        let log = super::win32::log_enabled();
        if sm.camera_change.take() {
            if let Some(camera) = &sm.camera {
                let descs = camera.descs();
                if log {
                    eprintln!("[hal-media] cameras: {:?}", descs.iter().map(|d| format!("{} ({} formats)", d.name, d.formats.len())).collect::<Vec<_>>());
                }
                s.events.video_inputs(&crate::media::pack_video_inputs(&descs));
            }
        }
        if sm.camera_frames.take() {
            if let Some(camera) = &sm.camera {
                for frame in camera.take_frames() {
                    s.events.camera_frame(frame.slots, &frame.payload);
                }
            }
        }
        if sm.game_change.take() {
            if let Some(game) = &sm.game {
                let (list, changed) = game.take();
                if let Some(list) = list {
                    if log {
                        eprintln!("[hal-media] game devices: {:?}", list.iter().map(|d| d.name.as_str()).collect::<Vec<_>>());
                    }
                    s.events.game_devices(&crate::media::game::pack_devices(&list));
                }
                for (id, state) in changed {
                    s.events.game_input(id, &crate::media::game::pack_state(&state));
                }
            }
        }
        if sm.audio_change.take() {
            if let Some(audio) = &sm.audio {
                let descs = audio.descs();
                if log {
                    eprintln!("[hal-media] audio devices: {:?}", descs.iter().map(|d| format!("{} kind={} default={} failed={} ch={}", d.name, d.kind, d.is_default, d.has_failed, d.channels)).collect::<Vec<_>>());
                }
                s.events.audio_devices(&crate::media::pack_audio_devices(&descs));
            }
        }
        if sm.midi_change.take() {
            if let Some(midi) = &sm.midi {
                let ports = midi.ports();
                if log {
                    eprintln!("[hal-media] midi ports: {:?}", ports.iter().map(|p| format!("{} out={}", p.name, p.output)).collect::<Vec<_>>());
                }
                s.events.midi_ports(&crate::media::pack_midi_ports(&ports));
            }
        }
        if sm.midi_data.take() {
            if let Some(midi) = &sm.midi {
                let messages = std::mem::take(&mut *midi.shared.inbox.lock().unwrap_or_else(|e| e.into_inner()));
                for (port, data) in messages {
                    s.events.midi_data(port, data);
                }
            }
        }
        if sm.video.take() {
            for (&id, player) in sm.players.iter_mut() {
                for note in player.take() {
                    match note {
                        video::Note::Prepared(p) => {
                            if super::win32::log_enabled() {
                                eprintln!("[hal-media] video {id:x} prepared {}x{} {} ms audio={}", p.width, p.height, p.duration_ms, p.has_audio);
                            }
                            let video_tracks: Vec<String> = if p.width > 0 && p.height > 0 { vec!["video".into()] } else { Vec::new() };
                            let audio_tracks: Vec<String> = if p.has_audio { vec!["audio".into()] } else { Vec::new() };
                            let tracks = crate::media::pack_tracks(&video_tracks, &audio_tracks);
                            let seekable = p.duration_ms > 0;
                            s.events.video_prepared(id, [p.width.min(u16::MAX as u32) as u16, p.height.min(u16::MAX as u32) as u16], p.duration_ms, seekable, &tracks);
                            if seekable {
                                s.events.video_ranges(id, crate::media::ranges::SEEKABLE, &crate::media::pack_ranges(&[(0.0, p.duration_ms as f64 / 1000.0)]));
                            }
                        }
                        video::Note::Frame(f) => {
                            if super::win32::log_enabled() && (player.frames_shown % 30 == 0 || f.position_ms.abs_diff(player.position_ms) > 250) {
                                eprintln!("[hal-media] video {id:x} frame #{} at {} ms nv12={}", player.frames_shown, f.position_ms, f.is_nv12());
                            }
                            if player.present(sink, s, &f) {
                                player.frames_shown += 1;
                                // a BGRA frame is sampled as is (no YUV bits)
                                let mut bits = if f.is_nv12() { crate::media::yuv::ENABLED | crate::media::yuv::BIPLANAR } else { 0 };
                                if f.full_range {
                                    bits |= crate::media::yuv::FULL_RANGE;
                                }
                                s.events.video_frame(id, f.position_ms, bits, f.matrix);
                            }
                        }
                        video::Note::Completed => s.events.video_status(id, video_status::COMPLETED, b""),
                        video::Note::Error(e) => {
                            eprintln!("[hal-media] video {id:x}: {e}");
                            s.events.video_status(id, video_status::ERROR, e.as_bytes());
                        }
                    }
                }
            }
        }
    }

    /// The GPU device was replaced: a player decodes on the device it was made with, so every
    /// player stops and its app hears why (it prepares the video again). Audio, MIDI, game input
    /// and cameras do not use the device and go on.
    pub fn device_reset(&mut self, world: &mut World) {
        for (&sid, sm) in self.sessions.iter_mut() {
            let ids: Vec<u64> = sm.players.keys().copied().collect();
            sm.players.clear();
            for id in ids {
                world.to_session(sid, |q| q.video_status(id, video_status::ERROR, b"the GPU device was reset: prepare the video again"));
            }
        }
    }

    /// The session is closing: everything it started stops.
    pub fn session_closed(&mut self, sid: u32) {
        if let Some(mut sm) = self.sessions.remove(&sid) {
            sm.players.clear();
            // the entries go first: a block of a stream thread that is running finishes, no later
            // one calls into the session (which is gone)
            if let Some(audio) = &sm.audio {
                for slot in audio.input_cb.iter().chain(audio.output_cb.iter()) {
                    *slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
                }
            }
        }
    }
}
