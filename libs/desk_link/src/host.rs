//! The deskhost engine (feature `host`): one picture source served to Desk Link viewers. The
//! host thread ([`host_main`]) serves while its settings say on and waits while they say off; it
//! captures and encodes only while a viewer watches, at the lowest rate any limit asks for, and
//! sends the cursor apart from the video. Everything network-facing is [`crate::server`].
//!
//! What it serves comes from a [`Desktop`]: the macOS / Windows Desk Host's screens
//! (apps/deskhost, pictures as shared textures), or a JIT host's own render (its window, its XR
//! eye buffers; pictures as NV12 from its GPU readback), with the input injected into the app
//! the viewer sees. Settings come in through a closure ([`HostSettings`]): a settings file, a
//! devhub switch. The app talks to the host thread through [`HostControl`].

use crate::server::{self, Control, Feed, Geometry, InputFactory, Log, Out, Server, ServerConfig, ToUi};
use crate::{self as proto, ScreenInfo, ServerMsg};
use makepad_video::{VideoStreamEncoder, VideoStreamEncoderOptions};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use makepad_video::StreamVideoCodec;

/// This process's clock (ns): every server time on the wire, and a [`Frame`]'s `captured_ns`.
pub fn clock_ns() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// One picture of the source, held until it is encoded (dropping it gives it back).
pub struct Frame {
    pub picture: Picture,
    pub width: u32,
    pub height: u32,
    pub captured_ns: u64,
    pub _hold: Box<dyn Send>,
}

pub enum Picture {
    /// A GPU surface (macOS: a captured NV12 IOSurface; Windows: a keyed-mutex BGRA shared
    /// texture), complete when captured: `VideoStreamEncoder::push_surface`.
    Shared(makepad_video::SurfaceHandle),
    /// Tightly packed NV12 (`width` x `height`, BT.709 limited range): a JIT host's GPU
    /// readback, `VideoStreamEncoder::push_frame_nv12`.
    Nv12(Vec<u8>),
}

/// What a desktop or a JIT host gives the deskhost.
pub trait Desktop {
    /// The sources a viewer picks from (a desktop's screens; a JIT host's window, XR eyes).
    fn screens(&mut self) -> Vec<ScreenInfo>;
    /// Starts (or moves) the capture of screen `index` at `fps`; frames come to `frames`.
    fn capture(&mut self, index: usize, fps: u32, frames: Sender<Frame>) -> Result<(), String>;
    fn stop_capture(&mut self);
    /// The pointer over screen `index` (0..=65535 each axis), whether it is on it, its shape
    /// (`proto::CURSOR_SHAPES` index).
    fn cursor(&mut self, index: usize) -> (u16, u16, bool, u8);
    /// The input devices for the server (`Err`: why viewers can only watch): a desktop's
    /// session, a JIT host's injection into the app it shows.
    fn input(&self) -> InputFactory;
    /// Screen `index`'s refresh rate (Hz; 0 = not known).
    fn refresh(&mut self, _index: usize) -> u32 {
        0
    }
    /// One line on the OS grants the host needs (empty where there are none).
    fn grants(&mut self) -> String {
        String::new()
    }
    /// A capture/session status for viewers, empty when ready. Called on connection
    /// and with the five-second status refresh, never per video frame.
    fn watch_note(&mut self, _preparing: bool) -> String {
        String::new()
    }
    /// Makes the screens capturable for a viewer who wants to watch, where that needs a step
    /// first (Windows: a session shown over Remote Desktop moves to the console). `Err`: why
    /// it cannot be watched now. `Ok(true)` invalidates an existing capture (e.g.
    /// after a session move). Called on every viewer connection, including paused ones.
    fn prepare_watch(&mut self) -> Result<bool, String> {
        Ok(false)
    }
    /// The codec this host encodes: H.264 everywhere today; a host may offer H.265 (streamed
    /// while every viewer decodes it, H.264 otherwise).
    fn codec(&self) -> StreamVideoCodec {
        StreamVideoCodec::H264
    }
    /// Held over one turn of the host loop and dropped at its end (macOS: an autorelease pool,
    /// so the capture's and the encoder's Objective-C objects go every turn).
    fn turn(&mut self) -> Option<Box<dyn std::any::Any>> {
        None
    }
}

/// The host's settings, read again whenever it starts serving or is told they changed
/// ([`Cmd::Settings`]).
#[derive(Clone, Debug)]
pub struct HostSettings {
    /// Off: nothing listens.
    pub enabled: bool,
    /// Where to listen (`Err`: why it cannot, e.g. a host that is not a LAN address).
    pub listen: Result<SocketAddr, String>,
    /// The cap of pictures a second (the stream runs at the lowest of it, the source's refresh,
    /// the watching viewers' displays and H.264's limit for the size).
    pub fps: u32,
    /// The stream's rate (0: by picture size, [`default_kbps`]).
    pub kbps: u32,
    /// Seconds between scheduled key frames while pictures flow (0: only on a new viewer, a
    /// request or a lost picture).
    pub key_every: u32,
    /// This machine's name, as the viewers see it (`Welcome::server`).
    pub host_name: String,
    /// The certificate's common name.
    pub tls_name: String,
    /// Where these settings are switched (the status line names it while off: a conf file, a
    /// devhub switch).
    pub source: String,
}

/// One viewer, as the host's owner shows it (apps/deskhost maps it to the panel's row).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewerRow {
    pub session: u32,
    /// What the viewer calls itself (`desklink <user>`).
    pub name: String,
    /// Its address (IP only).
    pub peer: String,
    /// Not watching right now (the viewer shows another desktop); still connected.
    pub paused: bool,
    /// Seconds since it connected.
    pub secs: u64,
    /// The stream: its rate, H.265 or H.264.
    pub fps: u32,
    pub hevc: bool,
    /// Pictures and bytes it got over the last second (a still source sends nothing).
    pub pictures: u32,
    pub bytes: u64,
}

/// What the host does, for its owner (the host thread writes it, the owner reads it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub state: HostState,
    /// `host:port` it listens on (while serving).
    pub addr: String,
    pub viewers: Vec<ViewerRow>,
    /// The OS grants it is missing (macOS), empty when it has them.
    pub grants: String,
}

impl Report {
    /// The port it listens on (0: not listening): what a JIT host reports to devhub.
    pub fn port(&self) -> u16 {
        self.addr.parse::<SocketAddr>().map_or(0, |a| a.port())
    }

    /// One line on what it does (`listens on ..`, `waiting: ..`, `off`), with the viewers.
    pub fn line(&self) -> String {
        let mut line = match &self.state {
            HostState::Off => "off".to_string(),
            HostState::Waiting(why) => format!("waiting: {}", why.lines().next().unwrap_or("")),
            HostState::Serving => format!("listens on {}", self.addr),
        };
        let watching = self.viewers.iter().filter(|v| !v.paused).count();
        if !self.viewers.is_empty() {
            line.push_str(&format!(", {} viewers ({watching} watching)", self.viewers.len()));
        }
        line
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum HostState {
    /// `enabled` off: nothing listens.
    #[default]
    Off,
    /// On, but it cannot listen yet (no fleet key, no allowed peer, a bad host), and why.
    Waiting(String),
    Serving,
}

/// What the owner asks of the host thread.
pub enum Cmd {
    /// The settings changed (a switch): read them again.
    Settings,
    Disconnect(u32),
    Quit,
}

/// The owner and the host thread: the report one way, commands the other, `wake` to make the
/// host look now.
pub struct HostControl {
    pub report: Mutex<Report>,
    pub cmds: Mutex<Vec<Cmd>>,
    pub wake: Sender<()>,
}

impl HostControl {
    /// A control and the receiver [`host_main`] waits on.
    pub fn new() -> (HostControl, Receiver<()>) {
        let (wake, wake_rx) = std::sync::mpsc::channel();
        (HostControl { report: Default::default(), cmds: Default::default(), wake }, wake_rx)
    }

    pub fn send(&self, cmd: Cmd) {
        self.cmds.lock().unwrap().push(cmd);
        let _ = self.wake.send(());
    }

    fn update(&self, f: impl FnOnce(&mut Report)) {
        f(&mut self.report.lock().unwrap());
    }
}

/// One line on what the host does now (listens, waits and why, its grants), rewritten in the
/// status file (when there is one) as it changes; the log says the same as it happens.
struct Status {
    state: String,
    grants: String,
    written: String,
    path: Option<PathBuf>,
    log: Log,
}

impl Status {
    fn set(&mut self, state: Option<String>, grants: Option<String>) {
        if let Some(g) = grants {
            if g != self.grants {
                (self.log)(&g);
                self.grants = g;
            }
        }
        if let Some(s) = state {
            self.state = s;
        }
        let line = if self.grants.is_empty() { self.state.clone() } else { format!("{}; {}", self.state, self.grants) };
        if line != self.written {
            if let Some(path) = &self.path {
                let _ = std::fs::write(path, format!("{line}\n"));
            }
            self.written = line;
        }
    }
}

struct Viewer {
    session: u32,
    name: String,
    out: Sender<Out>,
    /// Its pictures: backlog, pause, whether it follows the stream (`server::Feed`).
    feed: Arc<Feed>,
    /// What it decodes (`proto::caps`).
    caps: u32,
    /// The viewer's display refresh (Hz, 0 = not known) and whether it was told the stream's rate.
    refresh: u16,
    told_rate: bool,
    /// The viewer decodes from a key frame it got on: until then (a new viewer, a resume) it
    /// gets no picture that refers to one it never had.
    synced: bool,
    peer: String,
    since: Instant,
    /// Pictures and bytes sent this second, and over the last one (the owner's stream line).
    sent: (u32, u64),
    sent_rate: (u32, u64),
}

/// The host thread: serves while the settings say `enabled`, waits while they say off, until
/// the owner quits. Commands and settings changes wake it. `status_file`: where the one-line
/// status goes (none: the log only).
pub fn host_main(desktop: &mut dyn Desktop, settings: &dyn Fn() -> HostSettings, ctl: &HostControl, wake_rx: &Receiver<()>, log: Log, status_file: Option<PathBuf>) {
    let mut status = Status { state: String::new(), grants: String::new(), written: String::new(), path: status_file, log: log.clone() };
    loop {
        let now = settings();
        if take_quit(ctl) {
            return;
        }
        if !now.enabled {
            ctl.update(|r| {
                r.state = HostState::Off;
                r.addr.clear();
                r.viewers.clear();
            });
            status.set(Some(format!("off ({})", now.source)), Some(desktop.grants()));
            let _ = wake_rx.recv_timeout(Duration::from_secs(5));
            continue;
        }
        let Some((now, server)) = start(desktop, settings, &mut status, ctl, wake_rx, &log) else { continue };
        if !run(desktop, settings, &mut status, ctl, now, server, wake_rx, &log) {
            return;
        }
        log("stopped serving");
    }
}

/// Takes the commands of an idle host; true for Quit (settings changes are read anyway).
fn take_quit(ctl: &HostControl) -> bool {
    ctl.cmds.lock().unwrap().drain(..).any(|c| matches!(c, Cmd::Quit))
}

/// Whether a waiting host stops trying: Quit (kept queued for [`host_main`]); a settings change
/// is dropped (the next try reads them).
fn take_quit_or_settings(ctl: &HostControl) -> bool {
    let mut cmds = ctl.cmds.lock().unwrap();
    let quit = cmds.iter().any(|c| matches!(c, Cmd::Quit));
    cmds.retain(|c| matches!(c, Cmd::Quit));
    quit
}

/// The server, once it can listen: until then (no fleet identity installed yet, a conf being
/// edited) it says why and tries again every 10 s, reading the settings anew each time.
fn start(desktop: &mut dyn Desktop, settings: &dyn Fn() -> HostSettings, status: &mut Status, ctl: &HostControl, wake_rx: &Receiver<()>, log: &Log) -> Option<(HostSettings, Server)> {
    let mut said = String::new();
    loop {
        let now = settings();
        if !now.enabled || take_quit_or_settings(ctl) {
            return None;
        }
        let wake_tx = ctl.wake.clone();
        let started = now.listen.clone().and_then(|addr| {
            let config = ServerConfig { addr, tls_name: now.tls_name.clone(), log: log.clone(), clock: clock_ns, input: desktop.input() };
            Server::start(config, Arc::new(move || {
                let _ = wake_tx.send(());
            }))
        });
        match started {
            Ok(server) => {
                log(&format!("listens on {} as {} (the fleet admin only)", server.addr, server.name));
                status.set(Some(format!("listens on {}", server.addr)), Some(desktop.grants()));
                ctl.update(|r| {
                    r.state = HostState::Serving;
                    r.addr = server.addr.to_string();
                    r.grants = status.grants.clone();
                });
                return Some((now, server));
            }
            Err(e) => {
                if e != said {
                    log(&format!("{e} (tries again every 10 s)"));
                    said = e.clone();
                }
                status.set(Some(format!("waiting: {e}")), Some(desktop.grants()));
                ctl.update(|r| {
                    r.state = HostState::Waiting(e.clone());
                    r.addr.clear();
                    r.grants = status.grants.clone();
                });
                // a changed setting or a command tries again at once
                let _ = wake_rx.recv_timeout(Duration::from_secs(10));
            }
        }
    }
}

/// The highest H.264 level the platform's encoder takes, in macroblocks a second. More is not
/// encoded at all on macOS (5.2, 2 073 600: Apple silicon's encoder drops every 4K picture asked
/// for at 90 or 120 a second; at 60 it encodes them), so the rate of a big screen stays under it.
/// Windows' GPU encoders (NVENC, AMF, QSV) and Linux's Vulkan Video take level 6.2 (16 711 680:
/// 4K at 120). Android's MediaCodec encoders are held to 5.2 like Apple's.
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub const H264_MAX_MBPS: u64 = 16_711_680;
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub const H264_MAX_MBPS: u64 = 2_073_600;

/// Pictures a second: the settings' cap, the source's refresh, the watching viewers' displays,
/// H.264's limit for the stream's size.
fn stream_fps(cap: u32, screen_hz: u32, stream: [u32; 2], viewers: &[Viewer]) -> u32 {
    let mut fps = cap.max(1);
    let mbs = (stream[0].div_ceil(16) as u64 * stream[1].div_ceil(16) as u64).max(1);
    fps = fps.min((H264_MAX_MBPS / mbs).max(1) as u32);
    if screen_hz > 0 {
        fps = fps.min(screen_hz);
    }
    for v in viewers.iter().filter(|v| v.refresh > 0 && !v.feed.paused.load(Ordering::Acquire)) {
        fps = fps.min(v.refresh as u32);
    }
    fps
}

/// The codec streamed: the host's, while every viewer decodes it.
fn stream_codec(host: StreamVideoCodec, viewers: &[Viewer]) -> StreamVideoCodec {
    match host {
        StreamVideoCodec::Hevc if viewers.iter().all(|v| v.caps & proto::caps::HEVC != 0) => StreamVideoCodec::Hevc,
        _ => StreamVideoCodec::H264,
    }
}

fn wire_codec(codec: StreamVideoCodec) -> u8 {
    match codec {
        StreamVideoCodec::H264 => proto::pref::H264,
        StreamVideoCodec::Hevc => proto::pref::HEVC,
    }
}

/// Serves until the settings say off (false: the owner quit). Dropping the server says goodbye
/// to every viewer.
#[allow(clippy::too_many_arguments)]
fn run(desktop: &mut dyn Desktop, settings_now: &dyn Fn() -> HostSettings, status: &mut Status, ctl: &HostControl, settings: HostSettings, server: Server, wake_rx: &Receiver<()>, log: &Log) -> bool {
    let (frame_tx, frame_rx) = std::sync::mpsc::channel::<Frame>();
    let mut viewers: Vec<Viewer> = Vec::new();
    let mut screen = 0usize;
    let mut screens = desktop.screens();
    let mut screens_at = Instant::now();
    let mut grants_at = Instant::now();
    let mut watch_note = desktop.watch_note(false);
    let mut capturing: Option<(usize, u32)> = None;
    let mut encoder: Option<(VideoStreamEncoder, [u32; 2], u32, StreamVideoCodec)> = None;
    // pictures the encoder took and gave nothing back for (it drops what it cannot do)
    let mut dropped = 0u64;
    // pictures in a row the encoder failed
    let mut failed = 0u64;
    let mut timing = Timing::default();
    let mut fps = 0u32;
    let mut codec = stream_codec(desktop.codec(), &viewers);
    let mut watching = false;
    // the newest captured frame not yet encoded: held while a viewer is behind, so the last
    // change of a screen that then goes still is still sent (frames come only on change)
    let mut pending: Option<Frame> = None;
    // the last frame encoded, and when it was last encoded again: a still screen sends no
    // frames, so a viewer that came, resumed or asked for a key frame gets it again (a key
    // frame of what the screen shows) instead of waiting for the screen to change
    let mut shown: Option<Frame> = None;
    let mut reshown = Instant::now();
    let mut key_wanted = true;
    let mut frame_no: u32 = 0;
    let mut cursor: Option<(u16, u16, bool, u8)> = None;
    let mut last_cursor = Instant::now();
    // the screen and stream size the viewers were told
    let mut told_size: Option<(usize, [u32; 2])> = None;
    // the last key frame sent (scheduled ones follow `key_every` while pictures flow)
    let mut last_key = Instant::now();
    let mut reported = Instant::now();
    loop {
        let _turn = desktop.turn();
        // the owner's commands: off, quit, a viewer to disconnect
        let cmds: Vec<Cmd> = ctl.cmds.lock().unwrap().drain(..).collect();
        for cmd in cmds {
            match cmd {
                Cmd::Quit => {
                    if capturing.is_some() {
                        desktop.stop_capture();
                    }
                    return false;
                }
                Cmd::Settings => {
                    if !settings_now().enabled {
                        log("switched off");
                        if capturing.is_some() {
                            desktop.stop_capture();
                        }
                        return true;
                    }
                }
                Cmd::Disconnect(session) => {
                    if let Some(v) = viewers.iter().find(|v| v.session == session) {
                        log(&format!("disconnects {} ({})", v.name, v.peer));
                        let _ = v.out.send(Out::Close("the desktop disconnected you".into()));
                    }
                }
            }
        }
        // the owner's view of the viewers, once a second (and when one comes or goes)
        if reported.elapsed() >= Duration::from_secs(1) {
            reported = Instant::now();
            for v in viewers.iter_mut() {
                v.sent_rate = std::mem::take(&mut v.sent);
            }
            report_viewers(ctl, &viewers, fps, codec);
        }
        // nobody watches: sleep until a session says something (no polling while idle)
        if viewers.is_empty() {
            let _ = wake_rx.recv_timeout(Duration::from_secs(1));
        } else if !watching {
            let _ = wake_rx.recv_timeout(Duration::from_millis(20));
        }
        while wake_rx.try_recv().is_ok() {}
        if grants_at.elapsed() >= Duration::from_secs(5) {
            grants_at = Instant::now();
            status.set(None, Some(desktop.grants()));
            ctl.update(|r| r.grants = status.grants.clone());
            let note = desktop.watch_note(false);
            if note != watch_note {
                watch_note = note;
                viewers.iter_mut().for_each(|v| v.told_rate = false);
            }
        }
        if screens_at.elapsed() >= Duration::from_secs(1) {
            screens_at = Instant::now();
            screens = desktop.screens();
        }
        // whatever arrived from the sessions
        for m in server.rx.try_iter() {
            match m {
                ToUi::Opened { session, peer, client, name, screen: wanted, paused, caps, out, feed } => {
                    // The connection has already been authenticated. Report preparation before
                    // waiting for the desktop; even a paused/info-only viewer checks its setup.
                    let preparing = desktop.watch_note(true);
                    if !preparing.is_empty() {
                        status.set(None, Some(preparing.clone()));
                        ctl.update(|r| r.grants = status.grants.clone());
                        let _ = out.send(Out::Msg(ServerMsg::Rate { fps: 0, kbps: settings.kbps, limit: proto::limit::CAP, full_chroma: false, codec: wire_codec(codec), preset: proto::pref::FAST, streams: 1, why: preparing }.encode()));
                    }
                    match desktop.prepare_watch() {
                        Ok(true) => {
                            desktop.stop_capture();
                            capturing = None;
                            pending = None;
                            shown = None;
                        }
                        Ok(false) => {}
                        Err(e) => {
                            status.set(None, Some(e.clone()));
                            ctl.update(|r| r.grants = status.grants.clone());
                            let _ = out.send(Out::Close(format!("the desktop cannot prepare its screen: {e}")));
                            continue;
                        }
                    }
                    status.set(None, Some(desktop.grants()));
                    ctl.update(|r| r.grants = status.grants.clone());
                    watch_note = desktop.watch_note(false);
                    viewers.iter_mut().for_each(|v| v.told_rate = false);
                    screens = desktop.screens();
                    if viewers.is_empty() && wanted >= 0 && (wanted as usize) < screens.len() {
                        screen = wanted as usize;
                    }
                    let stream = screens.get(screen).map(stream_size).unwrap_or([0, 0]);
                    let note = server.input_note.lock().unwrap().clone();
                    log(&format!("{client} ({name}) from {peer} watches screen {screen}{}", if paused { " (paused)" } else { "" }));
                    let welcome = ServerMsg::Welcome {
                        version: proto::VERSION,
                        session,
                        screens: screens.clone(),
                        screen: screen as u32,
                        stream,
                        input: note.is_empty(),
                        input_note: note,
                        server: settings.host_name.clone(),
                    };
                    let _ = out.send(Out::Msg(welcome.encode()));
                    viewers.push(Viewer { session, name, out, feed, caps, refresh: 0, told_rate: false, synced: false, peer, since: Instant::now(), sent: (0, 0), sent_rate: (0, 0) });
                    report_viewers(ctl, &viewers, fps, codec);
                    key_wanted = true;
                    cursor = None;
                }
                ToUi::Control { session, control } => {
                    let Some(v) = viewers.iter_mut().find(|v| v.session == session) else { continue };
                    match control {
                        Control::Pause => v.feed.paused.store(true, Ordering::Release),
                        Control::Resume => {
                            v.feed.paused.store(false, Ordering::Release);
                            v.synced = false;
                            key_wanted = true;
                            cursor = None;
                        }
                        Control::Keyframe => {
                            log(&format!("{} asks for a key frame", v.name));
                            v.synced = false;
                            key_wanted = true;
                        }
                        Control::Refresh(hz) => {
                            log(&format!("{} shows {hz} Hz", v.name));
                            v.refresh = hz;
                        }
                        Control::SelectScreen(index) => {
                            screens = desktop.screens();
                            if (index as usize) < screens.len() && index as usize != screen {
                                screen = index as usize;
                                let msg = ServerMsg::Stream { screen: index, stream: stream_size(&screens[screen]) }.encode();
                                for v in &viewers {
                                    let _ = v.out.send(Out::Msg(msg.clone()));
                                }
                                key_wanted = true;
                            }
                        }
                        // a source streams its real size: the viewer's window size (v5, for
                        // desktops that follow their viewer) and who drives last change nothing
                        Control::Window { .. } => {}
                        Control::Clipboard(_) | Control::Stats { .. } | Control::Encoding { .. } => {}
                    }
                }
                ToUi::Closed { session, reason } => {
                    if let Some(i) = viewers.iter().position(|v| v.session == session) {
                        let v = viewers.remove(i);
                        log(&format!("{} left ({reason})", v.name));
                        report_viewers(ctl, &viewers, fps, codec);
                    }
                }
            }
        }
        if let Some(s) = screens.get(screen) {
            *server.geometry.lock().unwrap() = Geometry { desktop: [s.width, s.height], screen: [0, 0, s.width, s.height] };
            // the screen changed its size (a mode change, the session moved to another display):
            // the viewers are told, the stream starts again with a key frame
            let size = stream_size(s);
            if told_size != Some((screen, size)) {
                if told_size.is_some_and(|(i, _)| i == screen) {
                    log(&format!("screen {screen} is {}x{} now", size[0], size[1]));
                    let msg = ServerMsg::Stream { screen: screen as u32, stream: size }.encode();
                    for v in viewers.iter_mut() {
                        let _ = v.out.send(Out::Msg(msg.clone()));
                        v.synced = false;
                    }
                    key_wanted = true;
                }
                told_size = Some((screen, size));
            }
        }
        // the stream's rate and codec, told to every viewer (anew when they change)
        let now_fps = stream_fps(settings.fps, desktop.refresh(screen), screens.get(screen).map(stream_size).unwrap_or([0, 0]), &viewers);
        let now_codec = stream_codec(desktop.codec(), &viewers);
        if now_fps != fps || now_codec != codec {
            if fps != 0 && now_fps != fps {
                log(&format!("{now_fps} pictures a second (was {fps})"));
            }
            fps = now_fps;
            codec = now_codec;
            viewers.iter_mut().for_each(|v| v.told_rate = false);
        }
        for v in viewers.iter_mut().filter(|v| !v.told_rate) {
            v.told_rate = true;
            // a plain desktop's rate does not adapt: its setting, or by picture size (kbps 0)
            let limit = if settings.kbps > 0 { proto::limit::FIXED } else { proto::limit::CAP };
            let _ = v.out.send(Out::Msg(ServerMsg::Rate { fps: if watch_note.is_empty() { fps as u16 } else { 0 }, kbps: settings.kbps, limit, full_chroma: false, codec: wire_codec(codec), preset: proto::pref::FAST, streams: 1, why: watch_note.clone() }.encode()));
        }
        // the capture follows the viewers: on while one watches, on the screen they watch, at the stream's rate
        watching = viewers.iter().any(|v| !v.feed.paused.load(Ordering::Acquire));
        match (watching, capturing) {
            (true, c) if c != Some((screen, fps)) => {
                pending = None;
                match desktop.prepare_watch().and_then(|_| desktop.capture(screen, fps, frame_tx.clone())) {
                    Ok(()) => {
                        // frames of the capture before are dropped: they are in its format
                        while frame_rx.try_recv().is_ok() {}
                        shown = None;
                        capturing = Some((screen, fps));
                        key_wanted = true;
                    }
                    Err(e) => {
                        log(&format!("capture: {e}"));
                        status.set(None, Some(desktop.grants()));
                        for v in &viewers {
                            let _ = v.out.send(Out::Close(format!("the desktop cannot capture its screen: {e}")));
                        }
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            }
            (false, Some(_)) => {
                desktop.stop_capture();
                capturing = None;
                pending = None;
                shown = None;
            }
            _ => {}
        }
        // the cursor, apart from the video
        if !viewers.is_empty() && last_cursor.elapsed() >= Duration::from_millis(8) {
            last_cursor = Instant::now();
            let now = desktop.cursor(screen);
            if cursor != Some(now) {
                cursor = Some(now);
                let msg = ServerMsg::Cursor { x: now.0, y: now.1, shape: now.3, visible: now.2 }.encode();
                for v in &viewers {
                    let _ = v.out.send(Out::Msg(msg.clone()));
                }
            }
        }
        if !watching {
            continue;
        }
        // the newest frame (older ones are given back unencoded: they were never sent, so
        // nothing references them)
        if let Ok(f) = frame_rx.recv_timeout(Duration::from_millis(4)) {
            pending = Some(f);
        }
        while let Ok(newer) = frame_rx.try_recv() {
            pending = Some(newer);
        }
        // a viewer let go of the stream that caught up starts again at a key frame
        if server::resync_due(viewers.iter().map(|v| &*v.feed)) {
            key_wanted = true;
        }
        // a viewer behind: the frame waits (no teardown, no key frame) and goes when it caught up;
        // one behind while another keeps up is let go of the stream instead (`server::hold`)
        let congested = server::hold(viewers.iter().map(|v| &*v.feed));
        if congested {
            continue;
        }
        let unsynced = viewers.iter().any(|v| !v.synced && !v.feed.paused.load(Ordering::Acquire));
        if pending.is_none() && unsynced && reshown.elapsed() >= Duration::from_millis(250) {
            if let Some(mut again) = shown.take() {
                // a time of its own: the encoder's times only go forward
                again.captured_ns = clock_ns();
                pending = Some(again);
                reshown = Instant::now();
            }
        }
        let Some(frame) = pending.take() else { continue };
        let size = [frame.width / 4 * 4, frame.height / 2 * 2];
        if encoder.as_ref().is_none_or(|(_, s, f, c)| *s != size || *f != fps || *c != codec) {
            let options = VideoStreamEncoderOptions {
                codec,
                width: size[0],
                height: size[1],
                fps,
                bitrate_kbps: if settings.kbps > 0 { settings.kbps } else { default_kbps(size, fps) },
                keyint: 0,
                low_latency: true,
                full_chroma: false,
            };
            match VideoStreamEncoder::new(options) {
                Ok(e) => {
                    log(&format!("encoder: {} at {fps} pictures a second", e.description()));
                    encoder = Some((e, size, fps, codec));
                    key_wanted = true;
                }
                Err(e) => {
                    log(&format!("encoder: {e:?}"));
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
            }
        }
        let (enc, _, _, _) = encoder.as_mut().unwrap();
        // a key frame: asked for, a viewer not in step, or the scheduled one
        if settings.key_every > 0 && last_key.elapsed() >= Duration::from_secs(settings.key_every as u64) {
            key_wanted = true;
        }
        if std::mem::take(&mut key_wanted) || unsynced {
            enc.request_keyframe();
            last_key = Instant::now();
        }
        // the encode is synchronous: the frame (its surface) is held until it returns
        let push_ns = clock_ns();
        let pushed = push(enc, &frame, size);
        let captured_ns = frame.captured_ns;
        shown = Some(frame);
        timing.add(push_ns.saturating_sub(captured_ns), clock_ns() - push_ns, enc.timings());
        if timing.since.elapsed() >= Duration::from_secs(10) {
            log(&timing.line());
            timing = Timing::default();
        }
        match pushed {
            Ok(packets) => {
                failed = 0;
                if packets.is_empty() {
                    // a picture lost: whatever comes next must not refer to it
                    dropped += 1;
                    key_wanted = true;
                    if dropped == 1 || dropped % 1000 == 0 {
                        log(&format!("encoder: gave nothing for {dropped} pictures so far ({}x{} at {fps} a second); a key frame follows", size[0], size[1]));
                    }
                    continue;
                }
                let encoded_ns = clock_ns();
                for p in packets {
                    if p.is_key {
                        last_key = Instant::now();
                    }
                    let bytes = proto::encode_video(frame_no, p.is_key, captured_ns, encoded_ns, 0, (0, 1, 0, 0), wire_codec(codec), &p.data);
                    for v in viewers.iter_mut() {
                        if !v.feed.admit(frame_no, p.is_key) {
                            continue;
                        }
                        // a viewer starts at a key frame
                        v.synced |= p.is_key;
                        if !v.synced {
                            continue;
                        }
                        v.feed.backlog.fetch_add(1, Ordering::AcqRel);
                        v.sent.0 += 1;
                        v.sent.1 += bytes.len() as u64;
                        if v.out.send(Out::Video(bytes.clone())).is_err() {
                            v.synced = false;
                        }
                    }
                }
                frame_no = frame_no.wrapping_add(1);
            }
            Err(e) => {
                // said once per run of failures (each picture fails the same way)
                failed += 1;
                if failed == 1 || failed % 1000 == 0 {
                    log(&format!("picture {frame_no}: {e:?} ({failed} so far)"));
                }
                key_wanted = true;
                // a broken encoder is opened again
                encoder = None;
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

/// One picture into the encoder, by its kind. Its time is the capture's (100 ns units): the
/// encoder's rate control spends its bits by it (picture numbers as times read as ten million
/// pictures a second: starved pictures).
fn push(enc: &mut VideoStreamEncoder, frame: &Frame, size: [u32; 2]) -> Result<Vec<makepad_video::EncodedPacket>, makepad_video::VideoFileError> {
    let pts = (frame.captured_ns / 100) as i64;
    match &frame.picture {
        Picture::Shared(surface) => enc.push_surface(surface, &makepad_video::GpuCompletion::done(), pts),
        Picture::Nv12(nv12) if [frame.width, frame.height] == size => enc.push_frame_nv12(nv12, pts),
        Picture::Nv12(nv12) => enc.push_frame_nv12(&crop_nv12(nv12, frame.width, frame.height, size), pts),
    }
}

/// The top-left `size` of a tightly packed `width` x `height` NV12 picture (a source whose size
/// is not a multiple of 4 x 2).
pub fn crop_nv12(nv12: &[u8], width: u32, height: u32, size: [u32; 2]) -> Vec<u8> {
    let (w, h, cw, ch) = (width as usize, height as usize, size[0] as usize, size[1] as usize);
    let mut out = Vec::with_capacity(cw * ch * 3 / 2);
    for y in 0..ch {
        out.extend_from_slice(&nv12[y * w..y * w + cw]);
    }
    // the interleaved chroma rows (one per two luma rows, `width` bytes each: odd widths round up)
    let chroma = h * w;
    let cstride = w.div_ceil(2) * 2;
    for y in 0..ch / 2 {
        let at = chroma + y * cstride;
        out.extend_from_slice(&nv12[at..at + cw]);
    }
    out
}

/// The viewers as the owner shows them: who, watching or not, since when, what they get.
fn report_viewers(ctl: &HostControl, viewers: &[Viewer], fps: u32, codec: StreamVideoCodec) {
    let rows = viewers
        .iter()
        .map(|v| ViewerRow {
            session: v.session,
            name: v.name.clone(),
            peer: v.peer.clone(),
            paused: v.feed.paused.load(Ordering::Acquire),
            secs: v.since.elapsed().as_secs(),
            fps,
            hevc: codec == StreamVideoCodec::Hevc,
            pictures: v.sent_rate.0,
            bytes: v.sent_rate.1,
        })
        .collect();
    ctl.update(|r| r.viewers = rows);
}

/// Where a picture's time goes, said every 10 s while pictures flow: captured -> the encoder
/// takes it (the host's own wait), and the encode call.
pub struct Timing {
    pub since: Instant,
    wait_us: Vec<u32>,
    encode_us: Vec<u32>,
    /// The platform's stages of the encode call (`VideoStreamEncoder::timings`), summed.
    stages: [u64; 3],
}

impl Default for Timing {
    fn default() -> Self {
        Timing { since: Instant::now(), wait_us: Vec::new(), encode_us: Vec::new(), stages: [0; 3] }
    }
}

impl Timing {
    pub fn add(&mut self, wait_ns: u64, encode_ns: u64, t: makepad_video::EncodeTimings) {
        for (sum, s) in self.stages.iter_mut().zip([t.copy_us, t.convert_us, t.encode_us]) {
            *sum += s as u64;
        }
        self.wait_us.push((wait_ns / 1000).min(u32::MAX as u64) as u32);
        self.encode_us.push((encode_ns / 1000).min(u32::MAX as u64) as u32);
    }

    pub fn line(&mut self) -> String {
        let stat = |v: &mut Vec<u32>| {
            v.sort_unstable();
            let at = |q: usize| v.get((v.len() * q / 100).min(v.len().saturating_sub(1))).copied().unwrap_or(0) as f64 / 1000.0;
            format!("median {:.1} p90 {:.1} max {:.1} ms", at(50), at(90), v.last().copied().unwrap_or(0) as f64 / 1000.0)
        };
        let n = self.wait_us.len();
        let mut line = format!("timing: {n} pictures in {:.0} s; captured to encode {}; encode {}", self.since.elapsed().as_secs_f64(), stat(&mut self.wait_us), stat(&mut self.encode_us));
        if self.stages.iter().any(|s| *s > 0) {
            let avg = |s: u64| s as f64 / n.max(1) as f64 / 1000.0;
            line += &format!(" (average copy {:.1}, convert {:.1}, encoder {:.1} ms)", avg(self.stages[0]), avg(self.stages[1]), avg(self.stages[2]));
        }
        line
    }
}

/// The size a source streams at: 4:2:0 chroma and the encoders want a width of a multiple of 4
/// and an even height.
pub fn stream_size(s: &ScreenInfo) -> [u32; 2] {
    [s.width / 4 * 4, s.height / 2 * 2]
}

/// 0.25 bit per pixel and picture (a LAN: sharp text; 4K at 120 is 250 Mbit/s), at most 400 Mbit/s.
pub fn default_kbps(size: [u32; 2], fps: u32) -> u32 {
    ((size[0] as u64 * size[1] as u64 * fps.max(1) as u64 / 4) / 1000).clamp(2_000, 400_000) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_is_the_lowest_limit() {
        // 4K: H.264 level 5.2 on macOS and Android (64 a second), 6.2 on Windows and Linux;
        // 1080p: the cap; the screen's refresh below both
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        assert_eq!(stream_fps(120, 240, [3840, 2160], &[]), 64);
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        assert_eq!(stream_fps(120, 240, [3840, 2160], &[]), 120);
        assert_eq!(stream_fps(120, 240, [1920, 1080], &[]), 120);
        assert_eq!(stream_fps(120, 60, [1920, 1080], &[]), 60);
        assert_eq!(stream_fps(120, 0, [2560, 1440], &[]), 120);
    }

    #[test]
    fn hevc_only_while_every_viewer_decodes_it() {
        let (out, _rx) = std::sync::mpsc::channel();
        let viewer = |caps: u32| Viewer { session: 1, name: String::new(), out: out.clone(), feed: Arc::new(Feed::new(false)), caps, refresh: 0, told_rate: false, synced: false, peer: String::new(), since: Instant::now(), sent: (0, 0), sent_rate: (0, 0) };
        assert_eq!(stream_codec(StreamVideoCodec::H264, &[viewer(proto::caps::HEVC)]), StreamVideoCodec::H264);
        assert_eq!(stream_codec(StreamVideoCodec::Hevc, &[viewer(proto::caps::HEVC)]), StreamVideoCodec::Hevc);
        assert_eq!(stream_codec(StreamVideoCodec::Hevc, &[viewer(proto::caps::HEVC), viewer(0)]), StreamVideoCodec::H264);
    }

    #[test]
    fn report_says_its_port_and_state() {
        let mut r = Report::default();
        assert_eq!((r.port(), r.line()), (0, "off".to_string()));
        r.state = HostState::Serving;
        r.addr = "10.0.0.201:8790".into();
        r.viewers.push(ViewerRow { session: 1, paused: true, ..Default::default() });
        assert_eq!((r.port(), r.line()), (8790, "listens on 10.0.0.201:8790, 1 viewers (0 watching)".to_string()));
    }

    #[test]
    fn nv12_crops_to_the_stream_size() {
        // 6x3: luma rows 0..3, chroma one row of 6 (for luma rows 0-1; the third has none)
        let (w, h) = (6u32, 3u32);
        let mut nv12: Vec<u8> = (0..(w * h) as u8).collect();
        nv12.extend([100, 101, 102, 103, 104, 105]);
        nv12.extend([110, 111, 112, 113, 114, 115]);
        let out = crop_nv12(&nv12, w, h, [4, 2]);
        assert_eq!(out, vec![0, 1, 2, 3, 6, 7, 8, 9, 100, 101, 102, 103]);
    }
}
