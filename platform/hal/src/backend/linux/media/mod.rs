//! Linux HAL media, keyed by session and by playback incarnation. Native Linux remains
//! independent until the desktop switch. Control, device I/O and decode run on long-lived
//! workers; the HAL pump only drains results and installs frames in its texture tables.
mod alsa_audio;
mod alsa_midi;
pub mod alsa_sys;
mod audio;
pub mod libc_sys;
mod linux_video_playback;
pub(crate) mod module_loader;
mod pulse_audio;
pub mod pulse_sys;
pub mod v4l2_camera;
pub mod v4l2_sys;
pub use linux_video_playback::can_play_type;
pub mod capture_surface;

/// Scratch quantum shared with the client. Larger negotiated periods are split, not limited.
pub const AUDIO_BLOCK_FRAMES: usize = 1024;
mod file_dialog;
mod gst;
pub mod gstreamer_sys;
mod va_dmabuf_modifier;
pub(crate) mod video;

use crate::{
    cmds::HalCmd,
    media::*,
    session::Session,
    types::{format, Id},
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone)]
pub struct Signal {
    flag: Arc<AtomicBool>,
    session: u32,
}
impl Signal {
    fn new(session: u32) -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            session,
        }
    }
    pub fn set(&self) {
        self.flag.store(true, Ordering::Release);
        crate::Hal::global().wake_session(self.session);
    }
    fn take(&self) -> bool {
        self.flag.swap(false, Ordering::AcqRel)
    }
}

pub(crate) trait Sink {
    fn declare(&mut self, session: &mut Session, id: Id, format: u32);
    fn present(
        &mut self,
        session: &mut Session,
        ids: [Id; 4],
        frame: &video::Frame,
    ) -> Result<bool, String>;
    fn release(&mut self, session: &mut Session, ids: [Id; 4]);
}

enum Request {
    Audio {
        direction: Option<usize>,
        devices: Vec<u64>,
        entries: audio::Entries,
    },
    Midi {
        op: u32,
        port: u64,
        data: u32,
        ports: Vec<u64>,
    },
    Prepare {
        video: u64,
        epoch: u64,
        mode: u16,
        source: Arc<[u8]>,
    },
    Control {
        video: u64,
        epoch: u64,
        op: u32,
        arg: u64,
    },
    Fallback {
        video: u64,
        epoch: u64,
    },
    Camera {
        epoch: u64,
        consumers: u32,
        inputs: Vec<(u64, u64)>,
    },
    Dialog {
        req: u32,
        kind: u32,
        title: String,
        location: String,
        filename: String,
        filters: Vec<String>,
    },
}
enum Message {
    Audio(Vec<AudioDeviceInfo>),
    MidiPorts(Vec<MidiPortInfo>),
    MidiData(u64, u32),
    Inputs(Vec<VideoInputInfo>),
    Camera(u64, u32, Arc<[u8]>),
    Prepared(u64, u64, video::Prepared),
    Frame(u64, u64, u64, Arc<video::Frame>),
    Status(u64, u64, u32, String),
    Ranges(u64, u64, u32, Vec<(f64, f64)>),
    Dialog(u32, Vec<String>),
}

/// Capacity one means one control operation is handed to a worker at a time. A full pipe
/// retains the exact request on the HAL side and retries on the next pump; no work is lost.
struct Service {
    sender: SyncSender<Request>,
    pending: VecDeque<Request>,
    inbox: Arc<Mutex<Vec<Message>>>,
    stop: Arc<AtomicBool>,
    pressured: bool,
}
impl Service {
    fn new(session: u32, lane: usize) -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let inbox = Arc::new(Mutex::new(Vec::new()));
        let output = inbox.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let signal = Signal::new(session);
        std::thread::Builder::new()
            .name(format!("hal-linux-media-{session}-{lane}"))
            .spawn(move || {
                let mut engine = Engine::new(session);
                while !stopped.load(Ordering::Acquire) {
                    let mut timeout = if engine.active() {
                        Duration::from_millis(8)
                    } else {
                        Duration::from_secs(1)
                    };
                    if let Some(cadence) = engine.camera.as_ref().and_then(|c| c.cadence()) {
                        timeout = timeout.min(cadence);
                    }
                    let mut messages = Vec::new();
                    match receiver.recv_timeout(timeout) {
                        Ok(request) => {
                            if stopped.load(Ordering::Acquire) {
                                break;
                            }
                            engine.command(request, &mut messages);
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    engine.step(&mut messages);
                    if !messages.is_empty() {
                        // The HAL shares only the swap, never the scan or native payload drop.
                        let mut queue = std::mem::take(&mut *lock(&output));
                        for message in messages {
                            // Like the native latest-frame mailbox: a slow consumer sees the
                            // newest complete frame, while metadata, MIDI and replies stay ordered.
                            let replace = queue.iter().position(|prior| match (&message, prior) {
                                (Message::Frame(a, e, _, _), Message::Frame(b, f, _, _)) => {
                                    a == b && e == f
                                }
                                (Message::Camera(e, a, _), Message::Camera(f, b, _)) => {
                                    e == f && a == b
                                }
                                _ => false,
                            });
                            if let Some(i) = replace {
                                queue.remove(i);
                            }
                            // Append so a new frame cannot overtake a Prepared/track event.
                            queue.push(message);
                        }
                        *lock(&output) = queue;
                        signal.set();
                    }
                }
                // All native owners drop on this worker: streams, sequencers, mmap buffers,
                // GStreamer pipelines, temporary media files and dialog child processes.
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            sender,
            pending: VecDeque::new(),
            inbox,
            stop,
            pressured: false,
        })
    }
    fn send(&mut self, request: Request) {
        self.pending.push_back(request);
        self.flush();
    }
    fn flush(&mut self) {
        while let Some(request) = self.pending.pop_front() {
            match self.sender.try_send(request) {
                Ok(()) => {}
                Err(TrySendError::Full(request)) => {
                    self.pending.push_front(request);
                    if !self.pressured {
                        eprintln!(
                            "[hal-media] control worker busy; retaining requests for the next pump"
                        );
                        self.pressured = true;
                    }
                    break;
                }
                Err(TrySendError::Disconnected(request)) => {
                    self.pending.push_front(request);
                    eprintln!("[hal-media] control worker exited unexpectedly");
                    break;
                }
            }
        }
        if self.pending.is_empty() {
            self.pressured = false;
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

struct Playback {
    epoch: u64,
    targets: [Id; 4],
    closing: bool,
    camera: bool,
}
#[derive(Default)]
struct SessionMedia {
    services: [Option<Service>; 5],
    entries: audio::Entries,
    players: HashMap<u64, Playback>,
    serial: u64,
    camera_epoch: u64,
}
impl SessionMedia {
    fn send(&mut self, sid: u32, lane: usize, request: Request) -> Result<(), String> {
        if self.services[lane].is_none() {
            self.services[lane] = Some(Service::new(sid, lane)?);
        }
        self.services[lane].as_mut().unwrap().send(request);
        Ok(())
    }
}
impl Drop for SessionMedia {
    fn drop(&mut self) {
        self.entries.clear();
    }
}

#[derive(Default)]
pub struct Media {
    sessions: HashMap<u32, SessionMedia>,
}
impl Media {
    pub(crate) fn command(&mut self, sink: &mut dyn Sink, s: &mut Session, cmd: &HalCmd) -> bool {
        match *cmd {
            HalCmd::Permission { op, kind, req } => {
                if op != permission_op::CHECK && op != permission_op::REQUEST {
                    s.bad();
                    return true;
                }
                // A Linux desktop does not ask: microphone and camera access is the device
                // node's, and a device that cannot be opened fails (no frames, a failed stream).
                // Unanswered, an app waited forever ("Waiting for camera permission...").
                let status = match kind {
                    permission_kind::AUDIO_INPUT | permission_kind::CAMERA => permission_status::GRANTED,
                    _ => permission_status::DENIED_PERMANENT,
                };
                s.events.permission(req, kind, status);
                return true;
            }
            HalCmd::Location { op } => {
                match op {
                    location_op::START => s.events.location(location_kind::UNAVAILABLE, 0.0, 0.0, 0.0, 0.0, f32::NAN, f32::NAN, f32::NAN, b"no location service on Linux"),
                    location_op::STOP => {}
                    _ => s.bad(),
                }
                return true;
            }
            _ => {}
        }
        let mut lane = match cmd {
            HalCmd::AudioUse { .. } | HalCmd::AudioEntry { .. } => 0,
            HalCmd::Midi { .. } => 1,
            HalCmd::VideoPrepare { .. } | HalCmd::VideoControl { .. } => 2,
            HalCmd::VideoInput { .. } => 4,
            _ => return false,
        };
        let sm = self.sessions.entry(s.id).or_default();
        let request = match *cmd {
            HalCmd::AudioEntry {
                dir,
                index,
                entry,
                ctx,
            } => {
                if dir > 1 {
                    s.bad();
                    return true;
                }
                let slot = sm.entries.slot(dir as usize, index as usize);
                slot.replace((entry != 0).then(|| audio::Entry {
                    function: unsafe { std::mem::transmute::<usize, AudioFn>(entry as usize) },
                    context: ctx as usize,
                }));
                Request::Audio {
                    direction: None,
                    devices: Vec::new(),
                    entries: sm.entries.clone(),
                }
            }
            HalCmd::AudioUse {
                dir,
                options: _,
                devices,
            } => {
                if dir > 1 || devices.len() % 8 != 0 {
                    s.bad();
                    return true;
                }
                let devices = read_u64s(devices);
                for index in 0..devices.len() {
                    sm.entries.slot(dir as usize, index);
                }
                Request::Audio {
                    direction: Some(dir as usize),
                    devices,
                    entries: sm.entries.clone(),
                }
            }
            HalCmd::Midi {
                op,
                port,
                data,
                ports,
            } => Request::Midi {
                op,
                port,
                data,
                ports: read_u64s(ports),
            },
            HalCmd::VideoInput { consumers, inputs } => {
                if inputs.len() % 16 != 0 {
                    s.bad();
                    return true;
                }
                sm.camera_epoch += 1;
                Request::Camera {
                    epoch: sm.camera_epoch,
                    consumers,
                    inputs: read_u64s(inputs)
                        .chunks_exact(2)
                        .map(|p| (p[0], p[1]))
                        .collect(),
                }
            }
            HalCmd::VideoPrepare {
                tex,
                mode,
                video,
                y,
                u,
                v,
                src,
            } => {
                let camera = mode & video_mode::CAMERA != 0;
                if camera {
                    lane = 4;
                }
                sm.serial += 1;
                if let Some(old) = sm.players.remove(&video) {
                    sink.release(s, old.targets);
                    // Stop the old owner even if the replacement source uses the other worker.
                    let _ = sm.send(
                        s.id,
                        if old.camera { 4 } else { 2 },
                        Request::Control {
                            video,
                            epoch: old.epoch,
                            op: video_op::CLEANUP,
                            arg: 0,
                        },
                    );
                }
                let targets = [tex, y, u, v];
                for (id, fmt) in
                    targets
                        .into_iter()
                        .zip([format::RGBA8, format::R8, format::R8, format::R8])
                {
                    if !id.is_none() {
                        sink.declare(s, id, fmt);
                    }
                }
                sm.players.insert(
                    video,
                    Playback {
                        epoch: sm.serial,
                        targets,
                        closing: false,
                        camera,
                    },
                );
                Request::Prepare {
                    video,
                    epoch: sm.serial,
                    mode,
                    source: Arc::from(src),
                }
            }
            HalCmd::VideoControl { video, op, arg } => {
                let Some(player) = sm.players.get_mut(&video) else {
                    return true;
                };
                if player.camera {
                    lane = 4;
                }
                if op == video_op::CLEANUP {
                    player.closing = true;
                    sink.release(s, player.targets);
                }
                Request::Control {
                    video,
                    epoch: player.epoch,
                    op,
                    arg,
                }
            }
            _ => unreachable!(),
        };
        if let Err(error) = sm.send(s.id, lane, request) {
            eprintln!("[hal-media] {error}");
            s.bad();
        }
        true
    }

    pub fn dialog(&mut self, sid: u32, cmd: crate::backend::exec::OsCmd) -> bool {
        let crate::backend::exec::OsCmd::FileDialog {
            req,
            kind,
            title,
            location,
            filename,
            filters,
        } = cmd
        else {
            return false;
        };
        let sm = self.sessions.entry(sid).or_default();
        match sm.send(
            sid,
            3,
            Request::Dialog {
                req,
                kind,
                title,
                location,
                filename,
                filters,
            },
        ) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("[hal-dialog] {e}");
                false
            }
        }
    }

    pub(crate) fn step(&mut self, sink: &mut dyn Sink, s: &mut Session) {
        let Some(sm) = self.sessions.get_mut(&s.id) else {
            return;
        };
        let mut messages = Vec::new();
        for service in sm.services.iter_mut().flatten() {
            service.flush();
            messages.extend(std::mem::take(&mut *lock(&service.inbox)));
        }
        for message in messages {
            match message {
                Message::Audio(devices) => s.events.audio_devices(&pack_audio_devices(&devices)),
                Message::MidiPorts(ports) => s.events.midi_ports(&pack_midi_ports(&ports)),
                Message::MidiData(port, data) => s.events.midi_data(port, data),
                Message::Inputs(inputs) => s.events.video_inputs(&pack_video_inputs(&inputs)),
                Message::Camera(epoch, slots, bytes) => {
                    if epoch == sm.camera_epoch {
                        s.events.camera_frame(slots, &bytes);
                    }
                }
                Message::Dialog(req, paths) => s.events.dialog_result(
                    req,
                    paths.len() as u32,
                    &crate::buffers::pack_strs(
                        &paths.iter().map(String::as_str).collect::<Vec<_>>(),
                    ),
                ),
                Message::Prepared(id, epoch, info) => {
                    if !sm
                        .players
                        .get(&id)
                        .is_some_and(|p| p.epoch == epoch && !p.closing)
                    {
                        continue;
                    }
                    s.events.video_prepared(
                        id,
                        [info.width as u16, info.height as u16],
                        info.duration_ms,
                        info.seekable,
                        &pack_tracks(&info.video_tracks, &info.audio_tracks),
                    );
                }
                Message::Frame(id, epoch, pos, frame) => {
                    let Some(player) = sm
                        .players
                        .get(&id)
                        .filter(|p| p.epoch == epoch && !p.closing)
                    else {
                        continue;
                    };
                    match sink.present(s, player.targets, &frame) {
                        Ok(true) => s.events.video_frame(id, pos, frame.yuv, frame.matrix),
                        Ok(false) => {} // the client retired a texture generation
                        Err(e) => {
                            eprintln!("[hal-video] {e}");
                            if player.camera {
                                s.events.video_status(id, video_status::ERROR, e.as_bytes());
                            } else {
                                let _ = sm.send(s.id, 2, Request::Fallback { video: id, epoch });
                            }
                        }
                    }
                }
                Message::Status(id, epoch, kind, text) => {
                    if !sm.players.get(&id).is_some_and(|p| p.epoch == epoch) {
                        continue;
                    }
                    if kind == video_status::RELEASED {
                        sm.players.remove(&id);
                    }
                    s.events.video_status(id, kind, text.as_bytes());
                }
                Message::Ranges(id, epoch, kind, ranges) => {
                    if sm
                        .players
                        .get(&id)
                        .is_some_and(|p| p.epoch == epoch && !p.closing)
                    {
                        s.events.video_ranges(id, kind, &pack_ranges(&ranges));
                    }
                }
            }
        }
    }
    pub fn next_due(&self, sid: u32, now: u64) -> Option<u64> {
        self.sessions
            .get(&sid)?
            .services
            .iter()
            .flatten()
            .any(|s| !s.pending.is_empty())
            .then_some(now + 1_000_000)
    }
    pub fn session_closed(&mut self, sid: u32) {
        self.sessions.remove(&sid);
        capture_surface::close_session(sid);
    }
}

struct Preview {
    epoch: u64,
    key: (u64, u64),
    active: bool,
}
struct Engine {
    session: u32,
    audio: Option<alsa_audio::AlsaAudioAccess>,
    pulse: Option<pulse_audio::PulseAudioAccess>,
    entries: audio::Entries,
    devices: [Vec<u64>; 2],
    audio_change: Signal,
    audio_descs: Vec<AudioDeviceInfo>,
    midi: Option<alsa_midi::Midi>,
    midi_descs: Vec<MidiPortInfo>,
    midi_started: bool,
    camera: Option<v4l2_camera::Camera>,
    camera_descs: Vec<VideoInputInfo>,
    camera_epoch: u64,
    consumers: u32,
    inputs: Vec<(u64, u64)>,
    gst: Option<Arc<gstreamer_sys::LibGStreamer>>,
    players: HashMap<u64, (u64, linux_video_playback::Player)>,
    previews: HashMap<u64, Preview>,
    dialogs: Vec<file_dialog::Dialog>,
    enumerated: Instant,
}
impl Engine {
    fn new(session: u32) -> Self {
        Self {
            session,
            audio: None,
            pulse: None,
            entries: Default::default(),
            devices: Default::default(),
            audio_change: Signal::new(session),
            audio_descs: Vec::new(),
            midi: None,
            midi_descs: Vec::new(),
            midi_started: false,
            camera: None,
            camera_descs: Vec::new(),
            camera_epoch: 0,
            consumers: 0,
            inputs: Vec::new(),
            gst: None,
            players: HashMap::new(),
            previews: HashMap::new(),
            dialogs: Vec::new(),
            enumerated: Instant::now(),
        }
    }
    fn active(&self) -> bool {
        self.midi.as_ref().is_some_and(|m| m.active())
            || self.camera.as_ref().is_some_and(|c| c.active())
            || !self.players.is_empty()
            || !self.dialogs.is_empty()
    }
    fn wanted_cameras(&self) -> Vec<(u64, u64)> {
        let mut wanted = self.inputs.clone();
        for preview in self.previews.values().filter(|p| p.active) {
            if !wanted.contains(&preview.key) {
                wanted.push(preview.key);
            }
        }
        wanted
    }
    fn refresh_camera(&mut self, messages: &mut Vec<Message>) {
        if self.camera.is_none() {
            let mut camera = v4l2_camera::Camera::default();
            self.camera_descs = camera.enumerate();
            messages.push(Message::Inputs(self.camera_descs.clone()));
            self.camera = Some(camera);
        }
        let wanted = self.wanted_cameras();
        self.camera.as_mut().unwrap().use_inputs(&wanted);
    }
    fn command(&mut self, request: Request, messages: &mut Vec<Message>) {
        match request {
            Request::Audio {
                direction,
                devices,
                entries,
            } => {
                self.entries = entries;
                if self.audio.is_none() {
                    let mut audio = alsa_audio::AlsaAudioAccess::new(self.audio_change.clone());
                    self.audio_descs = audio.get_updated_descs();
                    self.audio = Some(audio);
                    if std::env::var_os("MAKEPAD_DISABLE_PULSE_AUDIO").is_none() {
                        self.pulse = pulse_audio::PulseAudioAccess::new(
                            self.audio_change.clone(),
                            &self.entries,
                        );
                        if let Some(pulse) = &mut self.pulse {
                            self.audio_descs.extend(pulse.get_updated_descs());
                            if let Some(server) = pulse.server() {
                                eprintln!("[hal-audio] {server}");
                            }
                        }
                    }
                    messages.push(Message::Audio(self.audio_descs.clone()));
                }
                if let Some(p) = &mut self.pulse {
                    p.audio_input_cb = self.entries.0[0].clone();
                    p.audio_output_cb = self.entries.0[1].clone();
                }
                if let Some(dir) = direction {
                    self.devices[dir] = devices;
                    self.audio.as_mut().unwrap().use_devices(
                        dir,
                        &self.devices[dir],
                        &self.entries.0[dir],
                    );
                    if let Some(p) = &mut self.pulse {
                        if dir == 0 {
                            p.use_audio_inputs(&self.devices[0]);
                        } else {
                            p.use_audio_outputs(&self.devices[1]);
                        }
                    }
                }
            }
            Request::Midi {
                op,
                port,
                data,
                ports,
            } => {
                if !self.midi_started || (op == midi_op::RESET && self.midi.is_none()) {
                    self.midi_started = true;
                    match alsa_midi::Midi::new() {
                        Ok(mut m) => {
                            self.midi_descs = m.enumerate();
                            self.midi = Some(m);
                        }
                        Err(e) => eprintln!("[hal-midi] {e}"),
                    }
                    messages.push(Message::MidiPorts(self.midi_descs.clone()));
                }
                if let Some(m) = &mut self.midi {
                    match op {
                        midi_op::OPEN => {}
                        midi_op::USE_INPUTS => m.use_ports(false, ports),
                        midi_op::USE_OUTPUTS => m.use_ports(true, ports),
                        midi_op::RESET => {
                            m.reset();
                            self.midi_descs = m.enumerate();
                            messages.push(Message::MidiPorts(self.midi_descs.clone()));
                        }
                        midi_op::SEND => m.send(port, data),
                        _ => eprintln!("[hal-midi] unknown operation {op}"),
                    }
                }
            }
            Request::Camera {
                epoch,
                consumers,
                inputs,
            } => {
                self.camera_epoch = epoch;
                self.consumers = consumers;
                self.inputs = inputs;
                self.refresh_camera(messages);
            }
            Request::Prepare {
                video,
                epoch,
                mode,
                source,
            } => {
                self.players.remove(&video);
                self.previews.remove(&video);
                if mode & video_mode::CAMERA != 0 {
                    let ids = read_u64s(&source);
                    if ids.len() != 2 {
                        messages.push(Message::Status(
                            video,
                            epoch,
                            video_status::ERROR,
                            "invalid camera source".into(),
                        ));
                        return;
                    }
                    let key = (ids[0], ids[1]);
                    self.previews.insert(
                        video,
                        Preview {
                            epoch,
                            key,
                            active: true,
                        },
                    );
                    self.refresh_camera(messages);
                    if let Some(f) = self.camera.as_ref().and_then(|c| c.format(key)) {
                        messages.push(Message::Prepared(
                            video,
                            epoch,
                            video::Prepared::new(
                                f.width,
                                f.height,
                                0,
                                false,
                                vec!["camera".into()],
                                vec![],
                            ),
                        ));
                    } else {
                        messages.push(Message::Status(
                            video,
                            epoch,
                            video_status::ERROR,
                            "camera could not be opened".into(),
                        ));
                    }
                    return;
                }
                if self.gst.is_none() {
                    self.gst = gstreamer_sys::LibGStreamer::load();
                }
                let Some(gst) = &self.gst else {
                    messages.push(Message::Status(
                        video,
                        epoch,
                        video_status::ERROR,
                        "GStreamer libraries are unavailable".into(),
                    ));
                    return;
                };
                let source = match (mode & video_mode::SRC_MASK) >> video_mode::SRC_SHIFT {
                    src_kind::NETWORK => linux_video_playback::Source::Network(
                        String::from_utf8_lossy(&source).into_owned(),
                    ),
                    src_kind::FILE => linux_video_playback::Source::File(
                        String::from_utf8_lossy(&source).into_owned(),
                    ),
                    _ => linux_video_playback::Source::Memory(source),
                };
                match linux_video_playback::Player::new(
                    gst.clone(),
                    self.session,
                    video,
                    source,
                    mode & video_mode::AUTOPLAY != 0,
                    mode & video_mode::LOOP != 0,
                    mode & video_mode::AUDIO_ONLY != 0,
                ) {
                    Ok(player) => {
                        self.players.insert(video, (epoch, player));
                    }
                    Err(e) => messages.push(Message::Status(video, epoch, video_status::ERROR, e)),
                }
            }
            Request::Fallback { video, epoch } => {
                if let Some((e, p)) = self.players.get_mut(&video).filter(|(e, _)| *e == epoch) {
                    if !p.fallback() {
                        messages.push(Message::Status(
                            video,
                            *e,
                            video_status::ERROR,
                            "decoded frame could not be presented".into(),
                        ));
                    }
                }
            }
            Request::Control {
                video,
                epoch,
                op,
                arg,
            } => {
                if op == video_op::CLEANUP {
                    self.players.remove(&video);
                    self.previews.remove(&video);
                    if self.camera.is_some() {
                        self.refresh_camera(messages);
                    }
                    messages.push(Message::Status(
                        video,
                        epoch,
                        video_status::RELEASED,
                        String::new(),
                    ));
                    return;
                }
                if let Some(preview) = self.previews.get_mut(&video) {
                    if preview.epoch == epoch {
                        match op {
                            video_op::PLAY | video_op::RESUME => preview.active = true,
                            video_op::PAUSE => preview.active = false,
                            _ => {}
                        }
                        self.refresh_camera(messages);
                    }
                    return;
                }
                let Some((_, p)) = self.players.get_mut(&video).filter(|(e, _)| *e == epoch) else {
                    return;
                };
                match op {
                    video_op::PLAY => p.play(),
                    video_op::PAUSE => p.pause(),
                    video_op::RESUME => p.resume(),
                    video_op::MUTE => p.mute(),
                    video_op::UNMUTE => p.unmute(),
                    video_op::SEEK => p.seek_to(arg),
                    video_op::VOLUME => p.set_volume(f32::from_bits(arg as u32) as f64),
                    video_op::RATE => p.set_playback_rate(f32::from_bits(arg as u32) as f64),
                    video_op::SELECT_VIDEO_TRACK => {
                        p.select_video_track(arg as usize);
                    }
                    video_op::SELECT_AUDIO_TRACK => {
                        p.select_audio_track(arg as usize);
                    }
                    _ => eprintln!("[hal-video] unknown control {op}"),
                }
            }
            Request::Dialog {
                req,
                kind,
                title,
                location,
                filename,
                filters,
            } => match file_dialog::Dialog::new(req, kind, title, location, filename, filters) {
                Ok(dialog) => self.dialogs.push(dialog),
                Err(e) => {
                    eprintln!("[hal-dialog] {e}");
                    messages.push(Message::Dialog(req, Vec::new()));
                }
            },
        }
    }
    fn step(&mut self, messages: &mut Vec<Message>) {
        let audio_changed = self.audio_change.take();
        if self.enumerated.elapsed() >= Duration::from_secs(1) || audio_changed {
            self.enumerated = Instant::now();
            if let Some(a) = &mut self.audio {
                let mut devices = a.get_updated_descs();
                if let Some(p) = &mut self.pulse {
                    devices.extend(p.get_updated_descs());
                }
                if devices != self.audio_descs {
                    self.audio_descs = devices;
                    messages.push(Message::Audio(self.audio_descs.clone()));
                }
            }
            if let Some(m) = &mut self.midi {
                let descs = m.enumerate();
                if descs != self.midi_descs {
                    self.midi_descs = descs;
                    messages.push(Message::MidiPorts(self.midi_descs.clone()));
                }
            }
            if self.camera.as_ref().is_some_and(|c| c.unserved(&self.wanted_cameras())) {
                self.refresh_camera(messages);
            }
            if let Some(c) = &mut self.camera {
                let descs = c.enumerate();
                if descs != self.camera_descs {
                    self.camera_descs = descs;
                    messages.push(Message::Inputs(self.camera_descs.clone()));
                }
            }
        }
        if let Some(m) = &mut self.midi {
            for (port, data) in m.poll() {
                messages.push(Message::MidiData(port, data));
            }
        }
        if let Some(c) = &mut self.camera {
            for (key, bytes) in c.poll() {
                let bytes: Arc<[u8]> = bytes.into();
                let slots = self
                    .inputs
                    .iter()
                    .take(32)
                    .enumerate()
                    .filter(|(_, pair)| **pair == key)
                    .fold(0u32, |mask, (i, _)| mask | (1 << i))
                    & self.consumers;
                if slots != 0 {
                    messages.push(Message::Camera(self.camera_epoch, slots, bytes.clone()));
                }
                let previews: Vec<_> = self
                    .previews
                    .iter()
                    .filter(|(_, p)| p.key == key && p.active)
                    .map(|(id, p)| (*id, p.epoch))
                    .collect();
                if !previews.is_empty() {
                    if let Some(frame) = video::camera_preview(&bytes) {
                        let frame = Arc::new(frame);
                        for (id, epoch) in previews {
                            messages.push(Message::Frame(id, epoch, 0, frame.clone()));
                        }
                    }
                }
            }
        }
        for (&id, (epoch, p)) in &mut self.players {
            if let Some(prepared) = p.check_prepared() {
                match prepared {
                    Ok(info) => {
                        messages.push(Message::Prepared(id, *epoch, info));
                        messages.push(Message::Ranges(
                            id,
                            *epoch,
                            ranges::SEEKABLE,
                            p.seekable_ranges(),
                        ));
                    }
                    Err(e) => messages.push(Message::Status(id, *epoch, video_status::ERROR, e)),
                }
            }
            for event in p.poll_runtime() {
                match event {
                    linux_video_playback::GstRuntimeEvent::Error(e) => {
                        messages.push(Message::Status(id, *epoch, video_status::ERROR, e))
                    }
                    linux_video_playback::GstRuntimeEvent::Eos => messages.push(Message::Status(
                        id,
                        *epoch,
                        video_status::COMPLETED,
                        String::new(),
                    )),
                    linux_video_playback::GstRuntimeEvent::TracksChanged {
                        video_tracks,
                        audio_tracks,
                    } => messages.push(Message::Prepared(
                        id,
                        *epoch,
                        p.prepared_info(video_tracks, audio_tracks),
                    )),
                }
            }
            if let Some(frame) = p.poll_frame() {
                messages.push(Message::Frame(
                    id,
                    *epoch,
                    p.current_position_ms() as u64,
                    Arc::new(frame),
                ));
            }
            if let Some(ranges) = p.take_buffered_ranges_if_changed() {
                messages.push(Message::Ranges(id, *epoch, ranges::BUFFERED, ranges));
            }
        }
        let mut i = 0;
        while i < self.dialogs.len() {
            if let Some(paths) = self.dialogs[i].poll() {
                let dialog = self.dialogs.remove(i);
                messages.push(Message::Dialog(dialog.request, paths));
            } else {
                i += 1;
            }
        }
    }
}
