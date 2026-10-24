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
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
struct Viewer {
    session: u32,
    name: String,
    out: Sender<Out>,
    backlog: Arc<AtomicUsize>,
    paused: Arc<AtomicBool>,
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
    if !settings.enabled {
        log(&format!("off ({}); nothing listens", Settings::path().display()));
        return;
    }
    #[cfg(target_os = "macos")]
    {
        // the one-time grants: asks (opens System Settings) when missing
        log(&desktop.permissions(true));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Linux desktops are served by the window manager itself (its desk_link)
        log("this desktop has no capture backend here; on Linux the window manager serves Desk Link");
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    run(settings, &mut desktop);
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn run(settings: Settings, desktop: &mut dyn Desktop) {
    let addr = match settings.listen_addr() {
        Ok(a) => a,
        Err(e) => {
            log(&e);
            return;
        }
    };
    let (wake_tx, wake_rx) = std::sync::mpsc::channel::<()>();
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
    let server = match Server::start(config, Arc::new(move || {
        let _ = wake_tx.send(());
    })) {
        Ok(s) => s,
        Err(e) => {
            log(&e);
            return;
        }
    };
    log(&format!("listens on {} (certificate {})", server.addr, server.fingerprint));

    let (frame_tx, frame_rx) = std::sync::mpsc::channel::<Frame>();
    let mut viewers: Vec<Viewer> = Vec::new();
    let mut screen = 0usize;
    let mut capturing: Option<usize> = None;
    let mut encoder: Option<(VideoStreamEncoder, [u32; 2])> = None;
    let mut key_wanted = true;
    let mut frame_no: u32 = 0;
    let mut cursor: Option<(u16, u16, bool, u8)> = None;
    let mut last_cursor = Instant::now();
    loop {
        // whatever arrived from the sessions
        let _ = wake_rx.try_recv();
        for m in server.rx.try_iter() {
            match m {
                ToUi::Opened { session, peer, client, name, screen: wanted, paused, out, backlog, .. } => {
                    let screens = desktop.screens();
                    if viewers.is_empty() && wanted >= 0 && (wanted as usize) < screens.len() {
                        screen = wanted as usize;
                    }
                    let stream = screens.get(screen).map(stream_size).unwrap_or([0, 0]);
                    let note = server.input_note.lock().unwrap().clone();
                    log(&format!("{client} ({name}) from {peer} watches screen {screen}{}", if paused { " (paused)" } else { "" }));
                    let welcome = ServerMsg::Welcome {
                        version: proto::VERSION,
                        session,
                        screens,
                        screen: screen as u32,
                        stream,
                        input: note.is_empty(),
                        input_note: note,
                        server: settings::host_name(),
                    };
                    let _ = out.send(Out::Msg(welcome.encode()));
                    viewers.push(Viewer { session, name, out, backlog, paused: Arc::new(AtomicBool::new(paused)) });
                    key_wanted = true;
                    cursor = None;
                }
                ToUi::Control { session, control } => {
                    let Some(v) = viewers.iter().find(|v| v.session == session) else { continue };
                    match control {
                        Control::Pause => v.paused.store(true, Ordering::Release),
                        Control::Resume => {
                            v.paused.store(false, Ordering::Release);
                            key_wanted = true;
                            cursor = None;
                        }
                        Control::Keyframe => key_wanted = true,
                        Control::SelectScreen(index) => {
                            let screens = desktop.screens();
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
        // the capture follows the viewers: on while one watches, on the screen they watch
        let watching = viewers.iter().any(|v| !v.paused.load(Ordering::Acquire));
        let screens = desktop.screens();
        if let Some(s) = screens.get(screen) {
            *server.geometry.lock().unwrap() = Geometry { desktop: [s.width, s.height], screen: [0, 0, s.width, s.height] };
        }
        match (watching, capturing) {
            (true, c) if c != Some(screen) => match desktop.capture(screen, settings.fps, frame_tx.clone()) {
                Ok(()) => {
                    capturing = Some(screen);
                    key_wanted = true;
                }
                Err(e) => {
                    log(&format!("capture: {e}"));
                    for v in &viewers {
                        let _ = v.out.send(Out::Close(format!("the desktop cannot capture its screen: {e}")));
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            },
            (false, Some(_)) => {
                desktop.stop_capture();
                capturing = None;
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
        // the newest frame (older ones are given back unencoded: they were never sent, so
        // nothing references them)
        let mut frame = match frame_rx.recv_timeout(Duration::from_millis(4)) {
            Ok(f) => f,
            Err(_) => continue,
        };
        while let Ok(newer) = frame_rx.try_recv() {
            frame = newer;
        }
        let congested = viewers.iter().any(|v| !v.paused.load(Ordering::Acquire) && v.backlog.load(Ordering::Acquire) > BACKLOG);
        if !watching || congested {
            continue;
        }
        let size = [frame.width / 4 * 4, frame.height / 2 * 2];
        if encoder.as_ref().is_none_or(|(_, s)| *s != size) {
            let options = VideoStreamEncoderOptions {
                codec: StreamVideoCodec::H264,
                width: size[0],
                height: size[1],
                fps: settings.fps,
                bitrate_kbps: if settings.kbps > 0 { settings.kbps } else { default_kbps(size, settings.fps) },
                keyint: 0,
                low_latency: true, full_chroma: false,
            };
            match VideoStreamEncoder::new(options) {
                Ok(e) => {
                    log(&format!("encoder: {}", e.description()));
                    encoder = Some((e, size));
                    key_wanted = true;
                }
                Err(e) => {
                    log(&format!("encoder: {e:?}"));
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
            }
        }
        let (enc, _) = encoder.as_mut().unwrap();
        if std::mem::take(&mut key_wanted) {
            enc.request_keyframe();
        }
        match enc.push_shared_texture(frame.handle, GpuReady::Done, frame_no as i64) {
            Ok(packets) => {
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
fn default_kbps(size: [u32; 2], fps: u32) -> u32 {
    ((size[0] as u64 * size[1] as u64 * fps.max(1) as u64 * 12 / 100) / 1000).clamp(2_000, 200_000) as u32
}
