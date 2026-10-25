//! Desk Host: Desk Link's server for a plain desktop: the logged-in user's macOS or Windows
//! session, seen by a Desk Link viewer (apps/desklink) on the LAN as hardware H.264 and driven
//! with its keyboard, mouse and wheel as local events.
//!
//! It runs in the user's session (a LaunchAgent / an at-logon task), has no window, and does
//! nothing unless `~/.makepad/deskhost.conf` enables it (keys as the window manager's
//! `desk-link.conf`). Everything network-facing is the shared server of `libs/desk_link`
//! (LAN-only bind and peers, TLS, fleet credentials of role `lan`).
//!
//! The screen comes from the OS's capture service as GPU frames, only when it changed (frames
//! carry their own damage); each goes to the platform's hardware encoder
//! (`makepad_video::VideoStreamEncoder::push_shared_texture`) while a viewer watches. The cursor
//! goes apart from the video. `--check` prints the settings and the permissions and exits.

// no console window in the user's session (the at-logon task starts it there); output of the
// command-line modes still goes to redirected handles
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod settings;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use makepad_desk_link::server::{Control, Geometry, Out, Server, ServerConfig, ToUi};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use makepad_desk_link::{self as proto, ServerMsg};
use makepad_desk_link::ScreenInfo;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use makepad_video::{GpuReady, StreamVideoCodec, VideoStreamEncoder, VideoStreamEncoderOptions};
use settings::Settings;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::sync::Arc;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::time::Duration;
use std::time::Instant;

#[cfg(any(target_os = "macos", target_os = "windows"))]
/// More pictures than this queued for a viewer: no picture is encoded until it catches up.
const BACKLOG: usize = 2;

pub fn log(text: &str) {
    use std::io::Write;
    eprintln!("deskhost: {text}");
    if cfg!(test) {
        return;
    }
    let path = settings::home().join("deskhost.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let _ = writeln!(f, "{secs:.3} {text}");
    }
}

/// This process's clock (ns): every server time on the wire.
pub fn clock_ns() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// One captured screen frame, held until it is encoded (dropping it gives it back).
pub struct Frame {
    /// The platform's shared-texture handle (macOS: the IOSurface id).
    pub handle: u64,
    pub width: u32,
    pub height: u32,
    pub captured_ns: u64,
    pub _hold: Box<dyn Send>,
}

/// What a desktop platform gives the host.
pub trait Desktop {
    fn screens(&mut self) -> Vec<ScreenInfo>;
    /// Starts (or moves) the capture of screen `index` at `fps`; frames come to `frames`.
    fn capture(&mut self, index: usize, fps: u32, frames: Sender<Frame>) -> Result<(), String>;
    fn stop_capture(&mut self);
    /// The pointer over screen `index` (0..=65535 each axis), whether it is on it, its shape
    /// (`proto::CURSOR_SHAPES` index).
    fn cursor(&mut self, index: usize) -> (u16, u16, bool, u8);
    /// The session's input devices for the server (`Err`: why viewers can only watch).
    fn input(&self) -> makepad_desk_link::server::InputFactory;
    /// Screen `index`'s refresh rate (Hz; 0 = not known).
    fn refresh(&mut self, _index: usize) -> u32 {
        0
    }
    /// One line on the OS grants the host needs (empty where there are none).
    fn grants(&mut self) -> String {
        String::new()
    }
    /// Makes the screens capturable for a viewer who wants to watch, where that needs a step
    /// first (Windows: a session shown over Remote Desktop moves to the console). `Err`: why
    /// it cannot be watched now.
    fn prepare_watch(&mut self) -> Result<(), String> {
        Ok(())
    }
}

/// `~/.makepad/deskhost.status`: one line on what the host does now (listens, waits and why,
/// its grants), rewritten when it changes; the log says the same as it happens.
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Default)]
struct Status {
    state: String,
    grants: String,
    written: String,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Status {
    fn set(&mut self, state: Option<String>, grants: Option<String>) {
        if let Some(g) = grants {
            if g != self.grants {
                log(&g);
                self.grants = g;
            }
        }
        if let Some(s) = state {
            self.state = s;
        }
        let line = if self.grants.is_empty() { self.state.clone() } else { format!("{}; {}", self.state, self.grants) };
        if line != self.written {
            let _ = std::fs::write(settings::home().join("deskhost.status"), format!("{line}\n"));
            self.written = line;
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
struct Viewer {
    session: u32,
    name: String,
    out: Sender<Out>,
    backlog: Arc<AtomicUsize>,
    paused: Arc<AtomicBool>,
    /// The viewer's display refresh (Hz, 0 = not known) and whether it was told the stream's rate.
    refresh: u16,
    told_rate: bool,
    /// The viewer decodes from a key frame it got on: until then (a new viewer, a resume) it
    /// gets no picture that refers to one it never had.
    synced: bool,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let settings = Settings::load();
    #[cfg(target_os = "macos")]
    let mut desktop = macos::MacDesktop::new();
    #[cfg(target_os = "windows")]
    let mut desktop = windows::WinDesktop::new();
    if args.iter().any(|a| a == "--check") {
        println!("settings {}: {settings:?}", Settings::path().display());
        println!("{}: listen {:?}, fleet key in {}", settings::host_name(), settings.listen_addr(), settings.fleet_dir().display());
        #[cfg(target_os = "macos")]
        println!("{}", desktop.permissions(false));
        return;
    }
    // macOS's own grant prompts, only when the user asks for them (the agent never does)
    #[cfg(target_os = "macos")]
    if args.iter().any(|a| a == "--ask") {
        println!("{}", desktop.permissions(true));
        return;
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if let Some(i) = args.iter().position(|a| a == "--encode-test") {
        encode_test(args.get(i + 1).map(String::as_str).unwrap_or("3840x2160@120"));
        return;
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if let Some(i) = args.iter().position(|a| a == "--capture-test") {
        let secs = args.get(i + 1).and_then(|a| a.parse().ok()).unwrap_or(10);
        let repeat = args.iter().any(|a| a == "--busy");
        capture_test(&mut desktop, secs, repeat);
        return;
    }
    if !settings.enabled {
        log(&format!("off ({}); nothing listens", Settings::path().display()));
        return;
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Linux desktops are served by the window manager itself (its desk_link)
        log("this desktop has no capture backend here; on Linux the window manager serves Desk Link");
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    run(&mut desktop);
}

/// The server, once it can listen: until then (no fleet key installed yet, a conf being
/// edited) it says why and tries again every 10 s, reading the settings anew each time.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn start(desktop: &mut dyn Desktop, status: &mut Status) -> (Settings, Server, std::sync::mpsc::Receiver<()>) {
    let mut said = String::new();
    loop {
        let settings = Settings::load();
        let (wake_tx, wake_rx) = std::sync::mpsc::channel::<()>();
        let started = settings.listen_addr().and_then(|addr| {
            let config = ServerConfig {
                addr,
                allow_peers: settings.allow_peers.clone(),
                tls_dir: settings::home().join("deskhost/tls"),
                tls_name: format!("makepad desk host {}", settings::host_name()),
                fleet_dir: settings.fleet_dir(),
                log: Arc::new(|t: &str| log(t)),
                clock: clock_ns,
                input: desktop.input(),
            };
            Server::start(config, Arc::new(move || {
                let _ = wake_tx.send(());
            }))
        });
        match started {
            Ok(server) => {
                log(&format!("listens on {} for {:?} (certificate {})", server.addr, settings.allow_peers, server.fingerprint));
                status.set(Some(format!("listens on {}", server.addr)), Some(desktop.grants()));
                return (settings, server, wake_rx);
            }
            Err(e) => {
                if e != said {
                    log(&format!("{e} (tries again every 10 s)"));
                    said = e.clone();
                }
                status.set(Some(format!("waiting: {e}")), Some(desktop.grants()));
                std::thread::sleep(Duration::from_secs(10));
            }
        }
    }
}

/// The highest H.264 level the desktop's encoder takes, in macroblocks a second. More is not
/// encoded at all on macOS (5.2, 2 073 600: Apple silicon's encoder drops every 4K picture asked
/// for at 90 or 120 a second; at 60 it encodes them), so the rate of a big screen stays under it.
/// Windows' GPU encoders (NVENC, AMF, QSV) take level 6.2 (16 711 680: 4K at 120).
#[cfg(target_os = "macos")]
const H264_MAX_MBPS: u64 = 2_073_600;
#[cfg(target_os = "windows")]
const H264_MAX_MBPS: u64 = 16_711_680;

/// Pictures a second: the settings' cap, the screen's refresh, the watching viewers' displays,
/// H.264's limit for the stream's size.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn stream_fps(cap: u32, screen_hz: u32, stream: [u32; 2], viewers: &[Viewer]) -> u32 {
    let mut fps = cap.max(1);
    let mbs = (stream[0].div_ceil(16) as u64 * stream[1].div_ceil(16) as u64).max(1);
    fps = fps.min((H264_MAX_MBPS / mbs).max(1) as u32);
    if screen_hz > 0 {
        fps = fps.min(screen_hz);
    }
    for v in viewers.iter().filter(|v| v.refresh > 0 && !v.paused.load(Ordering::Acquire)) {
        fps = fps.min(v.refresh as u32);
    }
    fps
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn run(desktop: &mut dyn Desktop) {
    let mut status = Status::default();
    let (settings, server, wake_rx) = start(desktop, &mut status);
    let (frame_tx, frame_rx) = std::sync::mpsc::channel::<Frame>();
    let mut viewers: Vec<Viewer> = Vec::new();
    let mut screen = 0usize;
    let mut screens = desktop.screens();
    let mut screens_at = Instant::now();
    let mut grants_at = Instant::now();
    let mut capturing: Option<(usize, u32)> = None;
    let mut encoder: Option<(VideoStreamEncoder, [u32; 2], u32)> = None;
    // pictures the encoder took and gave nothing back for (it drops what it cannot do)
    let mut dropped = 0u64;
    // pictures in a row the encoder failed
    let mut failed = 0u64;
    let mut timing = Timing::default();
    let mut fps = 0u32;
    let mut watching = false;
    // the newest captured frame not yet encoded: held while a viewer is behind, so the last
    // change of a screen that then goes still is still sent (frames come only on change)
    let mut pending: Option<Frame> = None;
    let mut key_wanted = true;
    let mut frame_no: u32 = 0;
    let mut cursor: Option<(u16, u16, bool, u8)> = None;
    let mut last_cursor = Instant::now();
    // the screen and stream size the viewers were told
    let mut told_size: Option<(usize, [u32; 2])> = None;
    // the last key frame sent (scheduled ones follow `key_every` while pictures flow)
    let mut last_key = Instant::now();
    loop {
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
        }
        if screens_at.elapsed() >= Duration::from_secs(1) {
            screens_at = Instant::now();
            screens = desktop.screens();
        }
        // whatever arrived from the sessions
        for m in server.rx.try_iter() {
            match m {
                ToUi::Opened { session, peer, client, name, screen: wanted, paused, out, backlog, .. } => {
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
                        server: settings::host_name(),
                    };
                    let _ = out.send(Out::Msg(welcome.encode()));
                    viewers.push(Viewer { session, name, out, backlog, paused: Arc::new(AtomicBool::new(paused)), refresh: 0, told_rate: false, synced: false });
                    key_wanted = true;
                    cursor = None;
                }
                ToUi::Control { session, control } => {
                    let Some(v) = viewers.iter_mut().find(|v| v.session == session) else { continue };
                    match control {
                        Control::Pause => v.paused.store(true, Ordering::Release),
                        Control::Resume => {
                            v.paused.store(false, Ordering::Release);
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
                        // a physical screen streams its real size: the viewer's window size (v5,
                        // for desktops that follow their viewer) and who drives last change nothing
                        Control::Window { .. } | Control::Active => {}
                        Control::Clipboard(_) | Control::Stats { .. } | Control::Split(_) => {}
                    }
                }
                ToUi::Closed { session, reason } => {
                    if let Some(i) = viewers.iter().position(|v| v.session == session) {
                        let v = viewers.remove(i);
                        log(&format!("{} left ({reason})", v.name));
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
        // the stream's rate, told to every viewer (anew when it changes)
        let now_fps = stream_fps(settings.fps, desktop.refresh(screen), screens.get(screen).map(stream_size).unwrap_or([0, 0]), &viewers);
        if now_fps != fps {
            if fps != 0 {
                log(&format!("{now_fps} pictures a second (was {fps})"));
            }
            fps = now_fps;
            viewers.iter_mut().for_each(|v| v.told_rate = false);
        }
        for v in viewers.iter_mut().filter(|v| !v.told_rate) {
            v.told_rate = true;
            let _ = v.out.send(Out::Msg(ServerMsg::Rate { fps: fps as u16 }.encode()));
        }
        // the capture follows the viewers: on while one watches, on the screen they watch, at the stream's rate
        watching = viewers.iter().any(|v| !v.paused.load(Ordering::Acquire));
        match (watching, capturing) {
            (true, c) if c != Some((screen, fps)) => {
                pending = None;
                match desktop.prepare_watch().and_then(|()| desktop.capture(screen, fps, frame_tx.clone())) {
                    Ok(()) => {
                        // frames of the capture before are dropped: they are in its format
                        while frame_rx.try_recv().is_ok() {}
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
        // a viewer behind: the frame waits (no teardown, no key frame) and goes when it caught up
        let congested = viewers.iter().any(|v| !v.paused.load(Ordering::Acquire) && v.backlog.load(Ordering::Acquire) > BACKLOG);
        if congested {
            continue;
        }
        let Some(frame) = pending.take() else { continue };
        let size = [frame.width / 4 * 4, frame.height / 2 * 2];
        if encoder.as_ref().is_none_or(|(_, s, f)| *s != size || *f != fps) {
            let options = VideoStreamEncoderOptions {
                codec: StreamVideoCodec::H264,
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
                    encoder = Some((e, size, fps));
                    key_wanted = true;
                }
                Err(e) => {
                    log(&format!("encoder: {e:?}"));
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
            }
        }
        let (enc, _, _) = encoder.as_mut().unwrap();
        // a key frame: asked for, a viewer not in step, or the scheduled one
        let unsynced = viewers.iter().any(|v| !v.synced && !v.paused.load(Ordering::Acquire));
        if settings.key_every > 0 && last_key.elapsed() >= Duration::from_secs(settings.key_every as u64) {
            key_wanted = true;
        }
        if std::mem::take(&mut key_wanted) || unsynced {
            enc.request_keyframe();
        }
        // the encode is synchronous: the frame (its surface) is held until it returns
        let push_ns = clock_ns();
        let pushed = enc.push_shared_texture(frame.handle, GpuReady::Done, frame_no as i64);
        timing.add(push_ns.saturating_sub(frame.captured_ns), clock_ns() - push_ns, enc.stages());
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
                }
                let encoded_ns = clock_ns();
                for p in packets {
                    if p.is_key {
                        last_key = Instant::now();
                    }
                    let bytes = proto::encode_video(frame_no, p.is_key, frame.captured_ns, encoded_ns, 0, (0, 1, 0, 0), &p.data);
                    for v in viewers.iter_mut() {
                        if v.paused.load(Ordering::Acquire) {
                            continue;
                        }
                        // a viewer starts at a key frame
                        v.synced |= p.is_key;
                        if !v.synced {
                            continue;
                        }
                        v.backlog.fetch_add(1, Ordering::AcqRel);
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

/// `--capture-test SECS [--busy]`: screen 0 captured and encoded as a viewer would get it, for
/// SECS seconds, the timing line every 2 s (in ~/.makepad/deskhost-encode-test.txt too). With
/// `--busy` the newest picture is encoded again every 8 ms when the screen did not change (a
/// desktop that moves all the time).
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn capture_test(desktop: &mut dyn Desktop, secs: u64, busy: bool) {
    use std::io::Write;
    let report = settings::home().join("deskhost-encode-test.txt");
    let mut out = std::fs::File::create(&report).ok();
    let mut say = |line: String| {
        println!("{line}");
        if let Some(f) = out.as_mut() {
            let _ = writeln!(f, "{line}");
        }
    };
    let (tx, rx) = std::sync::mpsc::channel::<Frame>();
    if let Err(e) = desktop.prepare_watch().and_then(|()| desktop.capture(0, 120, tx)) {
        return say(format!("capture-test: {e}"));
    }
    let mut enc: Option<VideoStreamEncoder> = None;
    let mut last: Option<Frame> = None;
    let mut timing = Timing::default();
    let start = Instant::now();
    let mut n = 0i64;
    while start.elapsed() < Duration::from_secs(secs) {
        let fresh = match rx.recv_timeout(Duration::from_millis(8)) {
            Ok(f) => Some(f),
            Err(_) => None,
        };
        let fresh_frame = fresh.is_some();
        if let Some(f) = fresh {
            last = Some(f);
        }
        if !fresh_frame && !busy {
            continue;
        }
        let Some(frame) = last.as_ref() else { continue };
        let size = [frame.width / 4 * 4, frame.height / 2 * 2];
        if enc.is_none() {
            let options = VideoStreamEncoderOptions { codec: StreamVideoCodec::H264, width: size[0], height: size[1], fps: 120, bitrate_kbps: default_kbps(size, 120), keyint: 0, low_latency: true, full_chroma: false };
            match VideoStreamEncoder::new(options) {
                Ok(e) => {
                    say(format!("capture-test: {}", e.description()));
                    enc = Some(e);
                }
                Err(e) => return say(format!("capture-test: {e:?}")),
            }
        }
        let e = enc.as_mut().unwrap();
        let t = clock_ns();
        if let Err(err) = e.push_shared_texture(frame.handle, GpuReady::Done, n) {
            return say(format!("capture-test: picture {n}: {err:?}"));
        }
        timing.add(if fresh_frame { t.saturating_sub(frame.captured_ns) } else { 0 }, clock_ns() - t, e.stages());
        n += 1;
        if timing.since.elapsed() >= Duration::from_secs(2) {
            say(timing.line());
            timing = Timing::default();
        }
    }
    desktop.stop_capture();
}

/// `--encode-test WxH@FPS`: the platform's stream encoder as the host opens it, fed 240 moving
/// CPU pictures (a key frame asked for at 60 and 61, a dropped-picture style request at 150):
/// per picture the packets' NAL types and sizes, the encode time, and the first SPS. Needs no
/// desktop, no viewer, no settings.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn encode_test(spec: &str) {
    // to stdout and to ~/.makepad/deskhost-encode-test.txt (a run without a console)
    let report = settings::home().join("deskhost-encode-test.txt");
    let _ = std::fs::write(&report, "");
    macro_rules! say {
        ($($t:tt)*) => {{
            use std::io::Write;
            let line = format!($($t)*);
            println!("{line}");
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&report) {
                let _ = writeln!(f, "{line}");
            }
        }};
    }
    // WxH@FPS[/MS]: MS ms between pictures (a desktop that changes now and then)
    let (spec, pause_ms) = match spec.split_once('/') {
        Some((s, ms)) => (s, ms.parse::<u64>().unwrap_or(0)),
        None => (spec, 0),
    };
    let parse = || -> Option<(u32, u32, u32)> {
        let (size, fps) = spec.split_once('@')?;
        let (w, h) = size.split_once('x')?;
        Some((w.parse().ok()?, h.parse().ok()?, fps.parse().ok()?))
    };
    let Some((w, h, fps)) = parse() else {
        say!("encode-test: WxH@FPS[/MS], e.g. 3840x2160@120");
        return;
    };
    let size = [w / 4 * 4, h / 2 * 2];
    let options = VideoStreamEncoderOptions { codec: StreamVideoCodec::H264, width: size[0], height: size[1], fps, bitrate_kbps: default_kbps(size, fps), keyint: 0, low_latency: true, full_chroma: false };
    let mut enc = match VideoStreamEncoder::new(options) {
        Ok(e) => e,
        Err(e) => return say!("encode-test: {e:?}"),
    };
    say!("encode-test: {}", enc.description());
    let (w, h) = (size[0] as usize, size[1] as usize);
    let mut nv12 = vec![128u8; w * h * 3 / 2];
    let (mut empty, mut keys, mut bytes, mut times) = (0u32, 0u32, 0u64, Vec::new());
    let count = if pause_ms > 0 { 40u32 } else { 240 };
    for i in 0..count {
        if pause_ms > 0 {
            std::thread::sleep(Duration::from_millis(pause_ms));
        }
        // a bar that moves and a gradient: every picture changes
        for y in 0..h {
            let row = &mut nv12[y * w..(y + 1) * w];
            for (x, p) in row.iter_mut().enumerate() {
                *p = if (x + i as usize * 16) % w < 64 { 235 } else { (16 + (x * 200 / w + y * 19 / h) as u8).min(235) };
            }
        }
        if i == 60 || i == 61 || i == 150 {
            enc.request_keyframe();
        }
        let t = Instant::now();
        let packets = match enc.push_frame_nv12(&nv12, i as i64) {
            Ok(p) => p,
            Err(e) => return say!("encode-test: picture {i}: {e:?}"),
        };
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        if packets.is_empty() {
            empty += 1;
        }
        for p in &packets {
            keys += p.is_key as u32;
            bytes += p.data.len() as u64;
            let types = nal_types(&p.data);
            if p.is_key || i < 3 || (58..64).contains(&i) || (148..154).contains(&i) {
                say!("encode-test: picture {i}: {} bytes key={} NAL types {types:?}", p.data.len(), p.is_key);
            }
            if p.is_key && keys == 1 {
                if let Some(sps) = sps_of(&p.data) {
                    say!("encode-test: SPS profile_idc {} constraints {:#04x} level_idc {}", sps[0], sps[1], sps[2]);
                }
            }
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = times.len();
    say!(
        "encode-test: {n} pictures, {keys} key, {empty} gave nothing; encode median {:.2} ms p90 {:.2} ms max {:.2} ms; {:.1} Mbit/s at {fps} a second",
        times[n / 2],
        times[n * 9 / 10],
        times[n - 1],
        bytes as f64 * 8.0 / (n as f64 / fps as f64) / 1e6
    );
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn nal_types(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            out.push(data[i + 3] & 0x1f);
            i += 3;
        } else {
            i += 1;
        }
    }
    out
}

/// profile_idc, constraint flags, level_idc of the first SPS in `data`.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn sps_of(data: &[u8]) -> Option<[u8; 3]> {
    (0..data.len().saturating_sub(6)).find(|&i| data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 && data[i + 3] & 0x1f == 7).map(|i| [data[i + 4], data[i + 5], data[i + 6]])
}

/// Where a picture's time goes, said every 10 s while pictures flow: captured -> the encoder
/// takes it (the host's own wait), and the encode call.
#[cfg(any(target_os = "macos", target_os = "windows"))]
struct Timing {
    since: Instant,
    wait_us: Vec<u32>,
    encode_us: Vec<u32>,
    /// The platform's stages of the encode call (see `VideoStreamEncoder::stages`), summed.
    stages: [u64; 3],
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Default for Timing {
    fn default() -> Self {
        Timing { since: Instant::now(), wait_us: Vec::new(), encode_us: Vec::new(), stages: [0; 3] }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Timing {
    fn add(&mut self, wait_ns: u64, encode_ns: u64, stages: [u32; 3]) {
        for (sum, s) in self.stages.iter_mut().zip(stages) {
            *sum += s as u64;
        }
        self.wait_us.push((wait_ns / 1000).min(u32::MAX as u64) as u32);
        self.encode_us.push((encode_ns / 1000).min(u32::MAX as u64) as u32);
    }

    fn line(&mut self) -> String {
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

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn stream_size(s: &ScreenInfo) -> [u32; 2] {
    [s.width / 4 * 4, s.height / 2 * 2]
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
/// 0.25 bit per pixel and picture (a LAN: sharp text; 4K at 120 is 250 Mbit/s), at most 400 Mbit/s.
fn default_kbps(size: [u32; 2], fps: u32) -> u32 {
    ((size[0] as u64 * size[1] as u64 * fps.max(1) as u64 / 4) / 1000).clamp(2_000, 400_000) as u32
}

#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
mod tests {
    use super::*;

    #[test]
    fn rate_is_the_lowest_limit() {
        // 4K: H.264 level 5.2 on macOS (64 a second), 6.2 on Windows; 1080p: the cap; the
        // screen's refresh below both
        #[cfg(target_os = "macos")]
        assert_eq!(stream_fps(120, 240, [3840, 2160], &[]), 64);
        #[cfg(target_os = "windows")]
        assert_eq!(stream_fps(120, 240, [3840, 2160], &[]), 120);
        assert_eq!(stream_fps(120, 240, [1920, 1080], &[]), 120);
        assert_eq!(stream_fps(120, 60, [1920, 1080], &[]), 60);
        assert_eq!(stream_fps(120, 0, [2560, 1440], &[]), 120);
    }
}
