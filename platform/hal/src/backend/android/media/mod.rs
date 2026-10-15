//! Android media services for the HAL: camera, playback, SAF file dialog, permissions,
//! location, haptics (workers `Worker<E>` per session) and AAudio / AMidi (two control
//! workers per session). Wired like Linux's media: Vulkan calls `command`, `drain`,
//! `next_due`, `session_closed`, and forwards FileDialog OsCmds to `dialog`.
//!
//! Playback uses the Java MediaPlayer/SurfaceTexture on a private EGL worker, reads RGBA
//! there, and stages it through Gpu texture hooks; GL names never enter Vulkan. Camera
//! capture copies strided YUV/JPEG on its worker; only texture staging and event packing
//! run in the HAL pump. Frame mailboxes retain the latest frame per consumer; control and
//! reply queues are lossless. The one-command worker pipe retries on pressure.
//!
//! Not ported (not representable in the HAL command tables, or no Java API): raw H.264
//! encode/decode codec services, SysEx (MidiData carries short messages), HTTP streaming
//! and transport abort (cancellation retires the response route), buffered ranges of the
//! native Java player. Permission CHECK/REQUEST and onPermissionResult are the common
//! permission service (including RECORD_AUDIO): selecting an input does not request it.
//! AAudio conversion is O(frames * channels) with 1024-frame scratch; the only synchronous
//! callback quiescence is the AudioEntry ABI fence before the client frees a context.

mod camera;
mod file_dialog;
mod haptics;
mod audio;
mod midi;
mod video;
mod mediacodec;
pub mod hevc;
pub mod video_file;

use crate::{
    backend::{exec::OsCmd, vulkan::Vulkan},
    cmds::HalCmd,
    media::*,
    session::Session,
    types::Id,
};
use makepad_jni_sys::*;

pub use audio::AUDIO_BLOCK_FRAMES;
use std::{
    collections::{HashMap, VecDeque},
    ffi::{c_void, CStr},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread::{self, Thread},
    time::Duration,
};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Java sees an incarnation token, never an app's reusable video id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ticket {
    pub video: u64,
    pub token: u64,
}
fn ticket(video: u64) -> Ticket {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    Ticket { video, token: NEXT.fetch_add(1, Ordering::Relaxed) }
}

enum Message {
    Inputs(Vec<VideoInputInfo>),
    Camera(u64, u32, Arc<[u8]>),
    Prepared(Ticket, u32, u32, u64, bool, Vec<String>, Vec<String>),
    Frame(Ticket, u64, Arc<video::Frame>),
    Status(Ticket, u32, String),
    Ranges(Ticket, u32, Vec<(f64, f64)>),
    Dialog(u32, Vec<String>),
    Permission(u32, u32, u32),
    Location(haptics::Fix),
}
#[derive(Default)]
struct Mailbox {
    events: Vec<Message>,
    frames: HashMap<(u8, u64, u64), Message>,
}
#[derive(Clone)]
struct Output {
    pub sid: u32,
    queue: Arc<Mutex<Mailbox>>,
    alive: Arc<AtomicBool>,
}
impl Output {
    fn new(sid: u32) -> Self {
        Self { sid, queue: Arc::new(Mutex::new(Mailbox::default())), alive: Arc::new(AtomicBool::new(true)) }
    }
    pub fn push(&self, message: Message) {
        if !self.alive.load(Ordering::Acquire) { return; }
        let key = match &message {
            Message::Frame(t, _, _) => Some((0, t.token, 0)),
            Message::Camera(epoch, slots, _) => Some((1, *epoch, *slots as u64)),
            _ => None,
        };
        // The replaced allocation drops outside the short mailbox lock.
        let old = if let Some(key) = key {
            lock(&self.queue).frames.insert(key, message)
        } else {
            lock(&self.queue).events.push(message);
            None
        };
        drop(old);
        crate::Hal::global().wake_session(self.sid);
    }
}

trait Engine: 'static {
    type Command: Send + 'static;
    fn new(output: Output) -> Self;
    fn command(&mut self, command: Self::Command);
    fn step(&mut self);
    fn active(&self) -> bool;
    fn interval(&self) -> Duration { Duration::from_millis(8) }
}
struct Worker<E: Engine> {
    sender: SyncSender<E::Command>,
    pending: VecDeque<E::Command>,
    stop: Arc<AtomicBool>,
    thread: Thread,
    pressured: bool,
}
impl<E: Engine> Worker<E> {
    fn new(output: Output, name: &str) -> Result<Self, String> {
        // One in transit; the HAL retains every request the worker cannot take yet.
        let (sender, receiver) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let handle = std::thread::Builder::new().name(format!("hal-android-{name}-{}", output.sid)).spawn(move || {
            let _jni_thread = JniThread::new();
            let sid = output.sid;
            let mut engine = E::new(output);
            while !stopped.load(Ordering::Acquire) {
                match receiver.try_recv() {
                    Ok(command) => {
                        crate::Hal::global().wake_session(sid);
                        engine.command(command);
                    }
                    Err(mpsc::TryRecvError::Disconnected) => break,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                engine.step();
                if stopped.load(Ordering::Acquire) { break; }
                if engine.active() {
                    std::thread::park_timeout(engine.interval());
                } else {
                    std::thread::park();
                }
            }
            // Native close, Java cleanup and payload destruction stay on this worker.
        }).map_err(|e| e.to_string())?;
        Ok(Self { sender, pending: VecDeque::new(), stop, thread: handle.thread().clone(), pressured: false })
    }
    fn send(&mut self, command: E::Command) {
        self.pending.push_back(command);
        self.flush();
    }
    fn flush(&mut self) {
        while let Some(command) = self.pending.pop_front() {
            match self.sender.try_send(command) {
                Ok(()) => self.thread.unpark(),
                Err(TrySendError::Full(command)) | Err(TrySendError::Disconnected(command)) => {
                    self.pending.push_front(command);
                    if !self.pressured {
                        eprintln!("[hal-android-media] worker busy; retaining control for the next pump");
                        self.pressured = true;
                    }
                    break;
                }
            }
        }
        if self.pending.is_empty() { self.pressured = false; }
    }
}
impl<E: Engine> Drop for Worker<E> {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.unpark();
    }
}

enum ControlRequest {
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
}
enum ControlMessage {
    Audio(Vec<u8>),
    Ports(Vec<u8>),
    Midi(u64, u32),
}
struct Control {
    sender: SyncSender<ControlRequest>,
    pending: VecDeque<ControlRequest>,
    receiver: Receiver<ControlMessage>,
    stop: Arc<AtomicBool>,
    thread: Thread,
    pressured: bool,
}
impl Control {
    fn new(session: u32, lane: usize) -> Result<Self, String> {
        // A capacity-one handoff bounds concurrent control work, not accepted commands.
        // Any overflow stays on the HAL side and is retried by the next scheduled pump.
        let (sender, requests) = mpsc::sync_channel(1);
        let (output, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let handle = thread::Builder::new()
            .name(format!("hal-android-media-{session}-{lane}"))
            .spawn(move || {
                let _jni_thread = crate::backend::android::jhelp::WorkerThread::new();
                let mut audio: Option<audio::Audio> = None;
                let mut midi: Option<midi::Midi> = None;
                while !stopped.load(Ordering::Acquire) {
                    let mut messages = Vec::new();
                    let request = requests.try_recv().ok();
                    let had_request = request.is_some();
                    if let Some(request) = request {
                        match request {
                            ControlRequest::Audio {
                                direction,
                                devices,
                                entries,
                            } => audio
                                .get_or_insert_with(audio::Audio::new)
                                .configure(direction, devices, entries),
                            ControlRequest::Midi {
                                op,
                                port,
                                data,
                                ports,
                            } => {
                                if midi.is_none() {
                                    match midi::Midi::new() {
                                        Ok(service) => midi = Some(service),
                                        Err(error) => {
                                            eprintln!("[hal-midi] {error}");
                                            messages.push(ControlMessage::Ports(Vec::new()));
                                        }
                                    }
                                }
                                if let Some(midi) = &mut midi {
                                    match op {
                                        midi_op::OPEN => {}
                                        midi_op::USE_INPUTS => midi.use_ports(false, ports),
                                        midi_op::USE_OUTPUTS => midi.use_ports(true, ports),
                                        midi_op::RESET => midi.reset(),
                                        midi_op::SEND => midi.send(port, data),
                                        _ => unreachable!("validated by command"),
                                    }
                                }
                            }
                        }
                    }
                    if let Some(audio) = &mut audio {
                        if let Some(devices) = audio.poll() {
                            messages.push(ControlMessage::Audio(pack_audio_devices(&devices)));
                        }
                    }
                    if let Some(midi) = &mut midi {
                        let (ports, data) = midi.poll();
                        if let Some(ports) = ports {
                            messages.push(ControlMessage::Ports(pack_midi_ports(&ports)));
                        }
                        messages.extend(
                            data.into_iter()
                                .map(|(port, data)| ControlMessage::Midi(port, data)),
                        );
                    }
                    if !messages.is_empty() {
                        for message in messages {
                            if output.send(message).is_err() {
                                return;
                            }
                        }
                        crate::Hal::global().wake_session(session);
                    }
                    if !had_request {
                        let delay = if midi.as_ref().is_some_and(midi::Midi::active) {
                            Duration::from_millis(1)
                        } else if audio.as_ref().is_some_and(audio::Audio::active) {
                            Duration::from_millis(8)
                        } else {
                            Duration::from_secs(1)
                        };
                        thread::park_timeout(delay);
                    }
                }
                // Streams, AMidi ports and JNI global references are destroyed on this worker.
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            sender,
            pending: VecDeque::new(),
            receiver,
            stop,
            thread: handle.thread().clone(),
            pressured: false,
        })
    }
    fn send(&mut self, request: ControlRequest) {
        self.pending.push_back(request);
        self.flush();
    }
    fn flush(&mut self) {
        while let Some(request) = self.pending.pop_front() {
            match self.sender.try_send(request) {
                Ok(()) => self.thread.unpark(),
                Err(TrySendError::Full(request)) => {
                    self.pending.push_front(request);
                    if !self.pressured {
                        eprintln!(
                            "[hal-android-media] worker busy; retaining commands for next pump"
                        );
                        self.pressured = true;
                    }
                    break;
                }
                Err(TrySendError::Disconnected(request)) => {
                    self.pending.push_front(request);
                    eprintln!("[hal-android-media] control worker stopped unexpectedly");
                    break;
                }
            }
        }
        if self.pending.is_empty() {
            self.pressured = false;
        }
    }
}
impl Drop for Control {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.unpark();
    }
}

struct Playback {
    ticket: Ticket,
    targets: [Id; 4],
    camera: bool,
    closing: bool,
}
struct SessionMedia {
    output: Output,
    camera: Worker<camera::Camera>,
    video: Option<Worker<video::Video>>,
    services: Option<Worker<haptics::Services>>,
    players: HashMap<u64, Playback>,
    camera_epoch: u64,
    /// Audio (0) and MIDI (1) control workers, started on first use.
    control: [Option<Control>; 2],
    entries: audio::Entries,
}
impl SessionMedia {
    fn new(sid: u32) -> Result<Self, String> {
        let output = Output::new(sid);
        Ok(Self { camera: Worker::new(output.clone(), "camera")?, output, video: None, services: None, players: HashMap::new(), camera_epoch: 0, control: [None, None], entries: audio::Entries::default() })
    }
    fn video(&mut self) -> Result<&mut Worker<video::Video>, String> {
        if self.video.is_none() { self.video = Some(Worker::new(self.output.clone(), "video")?); }
        Ok(self.video.as_mut().unwrap())
    }
    fn services(&mut self) -> Result<&mut Worker<haptics::Services>, String> {
        if self.services.is_none() { self.services = Some(Worker::new(self.output.clone(), "services")?); }
        Ok(self.services.as_mut().unwrap())
    }
    fn control(&mut self, t: Ticket, camera: bool, op: u32, arg: u64) -> Result<(), String> {
        if camera { self.camera.send(camera::Command::Control(t, op)); }
        else { self.video()?.send(video::Command::Control(t, op, arg)); }
        Ok(())
    }
    fn current(&self, t: Ticket) -> bool {
        self.players.get(&t.video).is_some_and(|p| p.ticket == t && !p.closing)
    }
}
impl Drop for SessionMedia {
    fn drop(&mut self) {
        self.output.alive.store(false, Ordering::Release);
        self.entries.clear();
    }
}

#[derive(Default)]
pub struct AndroidMedia {
    sessions: HashMap<u32, SessionMedia>,
}
impl AndroidMedia {
    pub fn new() -> Self { Self::default() }
    fn session(&mut self, sid: u32) -> Result<&mut SessionMedia, String> {
        if let std::collections::hash_map::Entry::Vacant(entry) = self.sessions.entry(sid) {
            entry.insert(SessionMedia::new(sid)?);
        }
        Ok(self.sessions.get_mut(&sid).unwrap())
    }
    pub fn command(&mut self, gpu: &mut Vulkan, s: &mut Session, c: &HalCmd) -> bool {
        if let HalCmd::Haptic { kind } = *c { return self.haptic(s.id, kind); }
        if matches!(c, HalCmd::AudioUse { .. } | HalCmd::AudioEntry { .. } | HalCmd::Midi { .. }) { return self.control_command(s, c); }
        if !matches!(c, HalCmd::VideoInput { .. } | HalCmd::VideoPrepare { .. } | HalCmd::VideoControl { .. } | HalCmd::Location { .. } | HalCmd::Permission { .. })
            && !matches!(c, HalCmd::ViewOp { op, .. } if op & 0xff00 == view_kind::CAMERA) { return false; }
        let result = (|| -> Result<(), String> {
            let sm = self.session(s.id)?;
            match *c {
                HalCmd::VideoInput { consumers, inputs } => {
                    if inputs.len() % 16 != 0 { return Err("invalid camera input pairs".into()); }
                    sm.camera_epoch += 1;
                    sm.camera.send(camera::Command::Inputs(sm.camera_epoch, consumers, read_u64s(inputs).chunks_exact(2).map(|p| (p[0], p[1])).collect()));
                }
                HalCmd::VideoPrepare { tex, mode, video, y, u, v, src } => {
                    if mode & video_mode::CAMERA != 0 && src.len() != 16 { return Err("invalid camera source".into()); }
                    if let Some(old) = sm.players.remove(&video) {
                        video::release(gpu, s, old.targets);
                        sm.control(old.ticket, old.camera, video_op::CLEANUP, 0)?;
                    }
                    let t = ticket(video);
                    let camera = mode & video_mode::CAMERA != 0;
                    let targets = [tex, y, u, v];
                    video::declare(s, targets)?;
                    sm.players.insert(video, Playback { ticket: t, targets, camera, closing: false });
                    if camera {
                        let ids = read_u64s(src);
                        sm.camera.send(camera::Command::Prepare(t, (ids[0], ids[1]), targets.iter().all(|i| i.is_none())));
                    } else { sm.video()?.send(video::Command::Prepare(t, mode, Arc::from(src))); }
                }
                HalCmd::VideoControl { video, op, arg } => {
                    if let Some(player) = sm.players.get_mut(&video) {
                        if op == video_op::CLEANUP {
                            player.closing = true;
                            video::release(gpu, s, player.targets);
                        }
                        let (t, camera) = (player.ticket, player.camera);
                        sm.control(t, camera, op, arg)?;
                    }
                }
                HalCmd::ViewOp { op, view, area, extra, .. } => {
                    if let Some(p) = sm.players.get(&view).filter(|p| p.camera && !p.closing) {
                        sm.camera.send(camera::Command::View(p.ticket, op & 0xff, area, extra != 0));
                    }
                }
                HalCmd::Location { op } => {
                    if op > location_op::STOP { return Err("invalid location operation".into()); }
                    sm.services()?.send(haptics::Command::Location(op));
                }
                HalCmd::Permission { op, kind, req } => {
                    if op > permission_op::REQUEST || kind > permission_kind::LOCATION { return Err("invalid permission operation/kind".into()); }
                    sm.services()?.send(haptics::Command::Permission(op, kind, req));
                }
                _ => unreachable!(),
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("[hal-android-media] {error}");
            s.bad();
            if let HalCmd::VideoPrepare { video, .. } = c { s.events.video_status(*video, video_status::ERROR, error.as_bytes()); }
        }
        true
    }
    /// AudioUse / AudioEntry (lane 0) and Midi (lane 1) go to their control worker.
    fn control_command(&mut self, s: &mut Session, c: &HalCmd) -> bool {
        let lane = if matches!(c, HalCmd::Midi { .. }) { 1 } else { 0 };
        let state = match self.session(s.id) {
            Ok(state) => state,
            Err(error) => {
                eprintln!("[hal-android-media] cannot start session media: {error}");
                if lane == 0 { s.events.audio_devices(&[]); } else { s.events.midi_ports(&[]); }
                return true;
            }
        };
        let request = match *c {
            HalCmd::AudioEntry { dir, index, entry, ctx } => {
                if dir > 1 { s.bad(); return true; }
                state.entries.slot(dir as usize, index as usize).replace((entry != 0).then(|| audio::Entry {
                    function: unsafe { std::mem::transmute::<usize, AudioFn>(entry as usize) },
                    context: ctx as usize,
                }));
                ControlRequest::Audio { direction: None, devices: Vec::new(), entries: state.entries.clone() }
            }
            HalCmd::AudioUse { dir, options: _, devices } => {
                if dir > 1 || devices.len() % 8 != 0 { s.bad(); return true; }
                let devices = read_u64s(devices);
                for index in 0..devices.len() { state.entries.slot(dir as usize, index); }
                ControlRequest::Audio { direction: Some(dir as usize), devices, entries: state.entries.clone() }
            }
            HalCmd::Midi { op, port, data, ports } => {
                if op > midi_op::SEND || ports.len() % 8 != 0 { s.bad(); return true; }
                ControlRequest::Midi { op, port, data, ports: read_u64s(ports) }
            }
            _ => unreachable!(),
        };
        if state.control[lane].is_none() {
            match Control::new(s.id, lane) {
                Ok(service) => state.control[lane] = Some(service),
                Err(error) => {
                    eprintln!("[hal-android-media] cannot start worker: {error}");
                    if lane == 0 { s.events.audio_devices(&[]); } else { s.events.midi_ports(&[]); }
                    return true;
                }
            }
        }
        state.control[lane].as_mut().unwrap().send(request);
        true
    }
    pub fn dialog(&mut self, sid: u32, command: OsCmd) -> bool {
        if !matches!(command, OsCmd::FileDialog { .. }) { return false; }
        match self.session(sid).and_then(SessionMedia::services) {
            Ok(worker) => { worker.send(haptics::Command::Dialog(command)); true }
            Err(e) => { eprintln!("[hal-android-dialog] {e}"); false }
        }
    }
    fn haptic(&mut self, sid: u32, kind: u32) -> bool {
        match self.session(sid).and_then(SessionMedia::services) {
            Ok(worker) => { worker.send(haptics::Command::Haptic(kind)); true }
            Err(e) => { eprintln!("[hal-android-haptic] {e}"); false }
        }
    }
    pub fn drain(&mut self, gpu: &mut Vulkan, s: &mut Session) {
        let sm = match self.session(s.id) { Ok(sm) => sm, Err(e) => { eprintln!("[hal-android-media] {e}"); return; } };
        sm.camera.flush();
        if let Some(w) = &mut sm.video { w.flush(); }
        if let Some(w) = &mut sm.services { w.flush(); }
        for control in sm.control.iter_mut().flatten() {
            control.flush();
            while let Ok(message) = control.receiver.try_recv() {
                match message {
                    ControlMessage::Audio(bytes) => s.events.audio_devices(&bytes),
                    ControlMessage::Ports(bytes) => s.events.midi_ports(&bytes),
                    ControlMessage::Midi(port, data) => s.events.midi_data(port, data),
                }
            }
        }
        let mailbox = std::mem::take(&mut *lock(&sm.output.queue));
        // Prepared and status messages precede the latest associated frame.
        for message in mailbox.events.into_iter().chain(mailbox.frames.into_values()) {
            match message {
                Message::Inputs(inputs) => s.events.video_inputs(&pack_video_inputs(&inputs)),
                Message::Camera(epoch, slots, bytes) if epoch == sm.camera_epoch => s.events.camera_frame(slots, &bytes),
                Message::Prepared(t, w, h, duration, seekable, vt, at) if sm.current(t) => {
                    s.events.video_prepared(t.video, [w as u16, h as u16], duration, seekable, &pack_tracks(&vt, &at));
                }
                Message::Frame(t, pos, frame) if sm.current(t) => {
                    let p = &sm.players[&t.video];
                    match video::present(gpu, s, p.targets, &frame) {
                        Ok(true) => s.events.video_frame(t.video, pos, frame.yuv, frame.matrix),
                        Ok(false) => {},
                        Err(e) => s.events.video_status(t.video, video_status::ERROR, e.as_bytes()),
                    }
                }
                Message::Status(t, kind, text) if sm.players.get(&t.video).is_some_and(|p| p.ticket == t) => {
                    if kind == video_status::RELEASED {
                        if let Some(player) = sm.players.remove(&t.video) { video::release(gpu, s, player.targets); }
                    }
                    s.events.video_status(t.video, kind, text.as_bytes());
                }
                Message::Ranges(t, kind, ranges) if sm.current(t) => s.events.video_ranges(t.video, kind, &pack_ranges(&ranges)),
                Message::Dialog(req, paths) => s.events.dialog_result(req, paths.len() as u32, &crate::buffers::pack_strs(&paths.iter().map(String::as_str).collect::<Vec<_>>())),
                Message::Permission(req, kind, status) => s.events.permission(req, kind, status),
                Message::Location(f) => s.events.location(f.kind, f.lon, f.lat, f.accuracy, f.time, f.alt, f.speed, f.heading, f.text.as_bytes()),
                _ => {},
            }
        }
    }
    pub fn next_due(&self, sid: u32, now: u64) -> Option<u64> {
        let sm = self.sessions.get(&sid)?;
        (!sm.camera.pending.is_empty() || sm.video.as_ref().is_some_and(|w| !w.pending.is_empty()) || sm.services.as_ref().is_some_and(|w| !w.pending.is_empty()) || sm.control.iter().flatten().any(|w| !w.pending.is_empty())).then_some(now + 1_000_000)
    }
    pub fn session_closed(&mut self, sid: u32) { self.sessions.remove(&sid); }
}

/// The old Android native player's MIME capability table.
pub fn can_play_type(mime: &str) -> &'static str {
    match mime.split(';').next().unwrap_or("").trim() {
        "video/mp4" | "video/x-m4v" | "audio/mp4" | "audio/x-m4a" | "audio/mpeg" | "audio/wav" | "audio/x-wav" => "probably",
        "video/webm" | "audio/webm" | "video/ogg" | "audio/ogg" => "maybe",
        other if other.starts_with("video/") || other.starts_with("audio/") => "maybe",
        _ => "",
    }
}

/// Every native this slice owns on MakepadNative; the shell registers them with its own.
pub fn native_methods() -> Vec<(&'static str, &'static str, *mut c_void)> {
    let mut methods = camera::native_methods();
    methods.extend(video::native_methods());
    methods.extend(file_dialog::native_methods());
    methods.extend(haptics::native_methods());
    methods.extend(midi::native_methods());
    methods
}
// Small JNI helpers shared only by this slice. Local references live in a frame;
// globals retain their JVM so cleanup also works after activityOnDestroy.
fn java<R>(f: impl FnOnce(*mut JNIEnv, jobject) -> Result<R, String>) -> Result<R, String> {
    crate::backend::android::jni::with_activity(|env, activity| unsafe {
        WORKER_VM.with(|slot| {
            if slot.get() == 0 {
                let mut vm = std::ptr::null_mut();
                ((**env).GetJavaVM.unwrap())(env, &mut vm);
                slot.set(vm as usize);
            }
        });
        if ((**env).PushLocalFrame.unwrap())(env, 32) < 0 { exception(env)?; return Err("JNI local frame unavailable".into()); }
        let result = f(env, activity);
        let ex = exception(env);
        ((**env).PopLocalFrame.unwrap())(env, std::ptr::null_mut());
        ex.and(result)
    }).unwrap_or_else(|| Err("Android Activity is unavailable".into()))
}
unsafe fn exception(env: *mut JNIEnv) -> Result<(), String> {
    if ((**env).ExceptionCheck.unwrap())(env) == 0 { return Ok(()); }
    ((**env).ExceptionDescribe.unwrap())(env);
    ((**env).ExceptionClear.unwrap())(env);
    Err("Android Java media call threw an exception (see logcat)".into())
}
unsafe fn method(env: *mut JNIEnv, object: jobject, name: &CStr, sig: &CStr) -> Result<jmethodID, String> {
    if object.is_null() { return Err("null Java media object".into()); }
    let cls = ((**env).GetObjectClass.unwrap())(env, object);
    exception(env)?;
    let id = ((**env).GetMethodID.unwrap())(env, cls, name.as_ptr(), sig.as_ptr());
    ((**env).DeleteLocalRef.unwrap())(env, cls);
    exception(env)?;
    if id.is_null() { Err(format!("missing Java method {}", name.to_string_lossy())) } else { Ok(id) }
}
unsafe fn void(env: *mut JNIEnv, object: jobject, name: &CStr, sig: &CStr, args: &[jvalue]) -> Result<(), String> {
    let id = method(env, object, name, sig)?;
    ((**env).CallVoidMethodA.unwrap())(env, object, id, args.as_ptr());
    exception(env)
}
unsafe fn string(env: *mut JNIEnv, text: &str) -> Result<jstring, String> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let len = i32::try_from(units.len()).map_err(|_| "Java string is too long")?;
    let s = ((**env).NewString.unwrap())(env, units.as_ptr(), len);
    exception(env)?;
    if s.is_null() { Err("Java string allocation failed".into()) } else { Ok(s) }
}
unsafe fn field(env: *mut JNIEnv, object: jobject, name: &CStr, sig: &CStr) -> Result<jobject, String> {
    let cls = ((**env).GetObjectClass.unwrap())(env, object);
    exception(env)?;
    let id = ((**env).GetFieldID.unwrap())(env, cls, name.as_ptr(), sig.as_ptr());
    ((**env).DeleteLocalRef.unwrap())(env, cls);
    exception(env)?;
    if id.is_null() { return Err(format!("missing Java field {}", name.to_string_lossy())); }
    let value = ((**env).GetObjectField.unwrap())(env, object, id);
    exception(env)?;
    if value.is_null() { Err(format!("null Java field {}", name.to_string_lossy())) } else { Ok(value) }
}
struct Global {
    object: usize,
    vm: usize,
}
impl Global {
    pub unsafe fn new(env: *mut JNIEnv, object: jobject) -> Result<Self, String> {
        let mut vm = std::ptr::null_mut();
        ((**env).GetJavaVM.unwrap())(env, &mut vm);
        let object = ((**env).NewGlobalRef.unwrap())(env, object);
        exception(env)?;
        if object.is_null() || vm.is_null() { return Err("Java global reference allocation failed".into()); }
        Ok(Self { object: object as usize, vm: vm as usize })
    }
    pub fn object(&self) -> jobject { self.object as _ }
}
impl Drop for Global {
    fn drop(&mut self) { unsafe {
        let vm = self.vm as *mut JavaVM;
        let mut env = std::ptr::null_mut();
        let attach = ((**vm).GetEnv.unwrap())(vm, &mut env as *mut _ as _, JNI_VERSION_1_6) != 0;
        if attach && ((**vm).AttachCurrentThread.unwrap())(vm, &mut env, std::ptr::null_mut()) != 0 { return; }
        ((**env).DeleteGlobalRef.unwrap())(env, self.object());
        if attach { ((**vm).DetachCurrentThread.unwrap())(vm); }
    }}
}
std::thread_local! {
    static WORKER_VM: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
struct JniThread;
impl JniThread {
    fn new() -> Self { Self }
}
impl Drop for JniThread {
    fn drop(&mut self) {
        // A worker may precede Activity creation and attach only much later.
        // Remember its first successful JNI call, not just its startup state.
        WORKER_VM.with(|slot| {
            let vm = slot.replace(0) as *mut JavaVM;
            if !vm.is_null() { unsafe { ((**vm).DetachCurrentThread.unwrap())(vm); } }
        });
    }
}
