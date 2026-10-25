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

/// H.264's highest level the desktop encoders take (5.2): 2 073 600 macroblocks a second. More
/// is not encoded at all (Apple silicon's encoder drops every 4K picture asked for at 90 or 120 a
/// second; at 60 it encodes them), so the rate of a big screen stays under it.
#[cfg(any(target_os = "macos", target_os = "windows"))]
const H264_MAX_MBPS: u64 = 2_073_600;

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
    let mut fps = 0u32;
    let mut watching = false;
    // the newest captured frame not yet encoded: held while a viewer is behind, so the last
    // change of a screen that then goes still is still sent (frames come only on change)
    let mut pending: Option<Frame> = None;
    let mut key_wanted = true;
    let mut frame_no: u32 = 0;
    let mut cursor: Option<(u16, u16, bool, u8)> = None;
    let mut last_cursor = Instant::now();
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
                    viewers.push(Viewer { session, name, out, backlog, paused: Arc::new(AtomicBool::new(paused)), refresh: 0, told_rate: false });
                    key_wanted = true;
                    cursor = None;
                }
                ToUi::Control { session, control } => {
                    let Some(v) = viewers.iter_mut().find(|v| v.session == session) else { continue };
                    match control {
                        Control::Pause => v.paused.store(true, Ordering::Release),
                        Control::Resume => {
                            v.paused.store(false, Ordering::Release);
                            key_wanted = true;
                            cursor = None;
                        }
                        Control::Keyframe => key_wanted = true,
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
                match desktop.capture(screen, fps, frame_tx.clone()) {
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
        if std::mem::take(&mut key_wanted) {
            enc.request_keyframe();
        }
        // the encode is synchronous: the frame (its surface) is held until it returns
        match enc.push_shared_texture(frame.handle, GpuReady::Done, frame_no as i64) {
            Ok(packets) => {
                if packets.is_empty() {
                    dropped += 1;
                    if dropped == 1 || dropped % 1000 == 0 {
                        log(&format!("encoder: gave nothing for {dropped} pictures so far ({}x{} at {fps} a second)", size[0], size[1]));
                    }
                }
                let encoded_ns = clock_ns();
                for p in packets {
                    let bytes = proto::encode_video(frame_no, p.is_key, frame.captured_ns, encoded_ns, 0, (0, 1, 0), &p.data);
                    for v in &viewers {
                        if v.paused.load(Ordering::Acquire) {
                            continue;
                        }
                        v.backlog.fetch_add(1, Ordering::AcqRel);
                        let _ = v.out.send(Out::Video(bytes.clone()));
                    }
                }
                frame_no = frame_no.wrapping_add(1);
            }
            Err(e) => {
                log(&format!("picture {frame_no}: {e:?}"));
                key_wanted = true;
            }
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn stream_size(s: &ScreenInfo) -> [u32; 2] {
    [s.width / 4 * 4, s.height / 2 * 2]
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
/// 0.12 bit per pixel and picture.
fn default_kbps(size: [u32; 2], fps: u32) -> u32 {
    ((size[0] as u64 * size[1] as u64 * fps.max(1) as u64 * 12 / 100) / 1000).clamp(2_000, 200_000) as u32
}

#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
mod tests {
    use super::*;

    #[test]
    fn rate_is_the_lowest_limit() {
        // 4K: H.264 level 5.2 (64 a second); 1080p: the cap; the screen's refresh below both
        assert_eq!(stream_fps(120, 240, [3840, 2160], &[]), 64);
        assert_eq!(stream_fps(120, 240, [1920, 1080], &[]), 120);
        assert_eq!(stream_fps(120, 60, [1920, 1080], &[]), 60);
        assert_eq!(stream_fps(120, 0, [2560, 1440], &[]), 120);
    }
}
