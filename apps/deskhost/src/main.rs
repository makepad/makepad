//! Desk Host: Desk Link's server for a plain desktop: the logged-in user's macOS or Windows
//! session, seen by a Desk Link viewer (apps/desklink) on the LAN as hardware H.264 and driven
//! with its keyboard, mouse and wheel as local events.
//!
//! It runs in the user's session (a LaunchAgent / an at-logon task) as a tray app (`app`): a
//! menu-bar / notification-area icon says whether it serves and who watches, and its dropdown
//! (`view`) switches serving on and off, lists the viewers with their stream, and quits. It
//! serves only while `~/.makepad/deskhost.conf` says `enabled = on` (the switch writes that key;
//! the other keys as the window manager's `desk-link.conf`). Everything network-facing is the
//! shared server of `libs/desk_link` (LAN-only bind, the fleet certificate, admin-only viewing:
//! tools/fleet-identity.md).
//!
//! The screen comes from the OS's capture service as GPU frames, only when it changed (frames
//! carry their own damage); each goes to the platform's hardware encoder
//! (`makepad_video::VideoStreamEncoder::push_surface`) while a viewer watches. The cursor
//! goes apart from the video. `--check` prints the settings and the permissions and exits.

// no console window in the user's session (the at-logon task starts it there); output of the
// command-line modes still goes to redirected handles
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod settings;
// the tray app (`app_main!` also makes a `main` there, which this file's replaces)
#[allow(dead_code)]
mod app;
mod view;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

// the engine (capture, encode, the server, the viewers) is the desk_link library's
#[cfg(any(target_os = "macos", target_os = "windows"))]
use makepad_desk_link::host::{self, clock_ns, default_kbps, Desktop, Frame, HostControl, Picture, Timing};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use makepad_video::{GpuCompletion, StreamVideoCodec, VideoStreamEncoder, VideoStreamEncoderOptions};
use settings::Settings;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::sync::mpsc::Receiver;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::time::{Duration, Instant};

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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cli = ["--check", "--ask", "--encode-test", "--capture-test"];
    if !args.iter().any(|a| cli.contains(&a.as_str())) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        makepad_widgets::run_app(app::app_main);
        // Linux desktops are served by the window manager itself (its desk_link)
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        log("this desktop has no capture backend here; on Linux the window manager serves Desk Link");
        return;
    }
    let settings = Settings::load();
    #[cfg(target_os = "macos")]
    macos::cli_app();
    #[cfg(target_os = "macos")]
    let mut desktop = macos::MacDesktop::new();
    #[cfg(target_os = "windows")]
    let mut desktop = windows::WinDesktop::new();
    if args.iter().any(|a| a == "--check") {
        println!("settings {}: {settings:?}", Settings::path().display());
        println!("{}: listen {:?}, fleet identity in {}", settings::host_name(), settings.listen_addr(), makepad_network::fleet::id::dir().display());
        #[cfg(target_os = "macos")]
        println!("{}", desktop.permissions(false));
        #[cfg(target_os = "windows")]
        println!("{}", desktop.grants());
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
    }
}

/// The host thread: the library's engine on this desktop, its settings from
/// `~/.makepad/deskhost.conf`, its one-line status in `~/.makepad/deskhost.status`.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn host_main(desktop: &mut dyn Desktop, ctl: &HostControl, wake_rx: &Receiver<()>) {
    let log: makepad_desk_link::server::Log = std::sync::Arc::new(|t: &str| log(t));
    host::host_main(desktop, &|| Settings::load().host(), ctl, wake_rx, log, Some(settings::home().join("deskhost.status")));
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
    if let Err(e) = desktop.prepare_watch().and_then(|_| desktop.capture(0, 120, tx)) {
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
        let pushed = match &frame.picture {
            Picture::Shared(surface) => e.push_surface(surface, &GpuCompletion::done(), n),
            Picture::Nv12(nv12) => e.push_frame_nv12(nv12, n),
        };
        if let Err(err) = pushed {
            return say(format!("capture-test: picture {n}: {err:?}"));
        }
        timing.add(if fresh_frame { t.saturating_sub(frame.captured_ns) } else { 0 }, clock_ns() - t, e.timings());
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
