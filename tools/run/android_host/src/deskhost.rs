//! The embedded Desk Link endpoint (the deskhost engine, `makepad_desk_link::host`): a viewer on
//! the admin's machine sees the host's own render and drives the app with its keyboard and mouse,
//! as the embedded devhub node (devhub.rs) serves the master. It runs natively in the host, next
//! to the HAL: frames come from the HAL's frame tap (`makepad_hal::frame_tap`, the finished
//! frames of the app's windows), input goes into the app through the HAL's injection
//! (`backend::android::inject`), and the phone's hardware encoder (MediaCodec, behind
//! `makepad_video::VideoStreamEncoder`) encodes. The app does not see it. Nothing is tapped or
//! encoded while no viewer watches.
//!
//! Sources (only those that exist): `panel` (the host window: the XR panel on a Quest; `window`
//! on a phone), `xr` (the XR stereo window, both eyes side by side) and `xr-left` (its left
//! eye). The pointer goes to the source's window; keys and text always go to the app.
//!
//! Identity: the fleet credential devhub.rs installs (`MAKEPAD_FLEET_DIR=files/fleet`); the
//! server serves only the fleet admin. Listens on 0.0.0.0:8790 as `makepad desk host <name>`.
//! Switched by the host conf (`desk_link=on|off`, `desk_link_fps=`), `files/deskhost.conf`
//! (`enabled=on|off`, `fps=`, `kbps=`, `key_every=`, `port=`; it wins over the host conf) and
//! devhub (`makepad devhub desk <node> on|off`, which writes `enabled=` there).

use crate::logcat;
use makepad_desk_link::host::{self, Cmd, Desktop, Frame, HostControl, HostSettings, Picture};
use makepad_desk_link::server::{Input, InputFactory, Log};
use makepad_desk_link::{GamepadState, ScreenInfo};
use makepad_hal::backend::android::inject::{self, AppWindow, Pointer};
use makepad_hal::frame_tap::{self, Region};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What the host conf says (`desk_link=on|off`, `desk_link_fps=`).
#[derive(Clone, Debug, Default)]
pub struct DeskConf {
    pub on: bool,
    pub fps: Option<u32>,
}

/// The stream's pictures a second: the default and the range the settings may ask for.
const DEFAULT_FPS: u32 = 36;
const FPS_RANGE: (u32, u32) = (30, 72);
/// The largest stream of each source kind (pixels; scaled down keeping the aspect).
const PANEL_MAX: [u32; 2] = [1920, 1920];
const XR_MAX: [u32; 2] = [2880, 1440];
const EYE_MAX: [u32; 2] = [1440, 1440];

fn log(text: &str) {
    logcat::log(&format!("deskhost: {text}"));
}

/// The settings file's `key=value` lines.
fn read_file(path: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let l = l.split('#').next()?.trim();
            let (k, v) = l.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

fn on_off(v: &str) -> Option<bool> {
    match v {
        "on" | "1" | "true" => Some(true),
        "off" | "0" | "false" => Some(false),
        _ => None,
    }
}

/// The endpoint's settings, from the host conf and the settings file.
struct Settings {
    file: PathBuf,
    conf: DeskConf,
    name: String,
}

impl Settings {
    fn now(&self) -> HostSettings {
        let file = read_file(&self.file);
        let get = |key: &str| file.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        let number = |key: &str| get(key).and_then(|v| v.parse::<u32>().ok());
        let enabled = get("enabled").and_then(on_off).unwrap_or(self.conf.on);
        let fps = number("fps").or(self.conf.fps).unwrap_or(DEFAULT_FPS).clamp(FPS_RANGE.0, FPS_RANGE.1);
        let port = number("port").filter(|p| *p > 0 && *p < 65536).unwrap_or(makepad_desk_link::DEFAULT_PORT as u32) as u16;
        HostSettings {
            enabled,
            listen: Ok(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))),
            fps,
            kbps: number("kbps").unwrap_or(0),
            key_every: number("key_every").unwrap_or(0),
            host_name: self.name.clone(),
            tls_name: format!("makepad desk host {}", self.name),
            source: format!("devhub `makepad devhub desk {} on`, or {}", self.name, self.file.display()),
        }
    }

    /// Writes `enabled=on|off` into the settings file, keeping its other lines.
    fn switch(&self, on: bool) -> Result<(), String> {
        let text = std::fs::read_to_string(&self.file).unwrap_or_default();
        let mut out: String = text.lines().filter(|l| l.split('=').next().map(str::trim) != Some("enabled")).map(|l| format!("{l}\n")).collect();
        out.push_str(if on { "enabled=on\n" } else { "enabled=off\n" });
        let tmp = self.file.with_extension("conf.new");
        std::fs::write(&tmp, out).and_then(|_| std::fs::rename(&tmp, &self.file)).map_err(|e| format!("{}: {e}", self.file.display()))
    }
}

/// A picture source: one of the app's windows (or its left half), its pixels, the stream size.
#[derive(Clone, Debug, PartialEq)]
struct Source {
    name: &'static str,
    window: AppWindow,
    region: Region,
    /// The region's pixels in the window.
    region_px: [u32; 2],
    stream: [u32; 2],
}

/// `px` scaled down to fit `max` (aspect kept), a width of a multiple of 4 and an even height.
fn fit(px: [u32; 2], max: [u32; 2]) -> [u32; 2] {
    let scale = (max[0] as f64 / px[0].max(1) as f64).min(max[1] as f64 / px[1].max(1) as f64).min(1.0);
    let w = ((px[0] as f64 * scale) as u32 / 4 * 4).max(4);
    let h = ((px[1] as f64 * scale) as u32 / 2 * 2).max(2);
    [w, h]
}

/// The sources that exist now.
fn sources() -> Vec<Source> {
    let mut out = Vec::new();
    if let Some(w) = inject::host_window() {
        let name = if inject::immersive() { "panel" } else { "window" };
        out.push(Source { name, window: w, region: Region::Whole, region_px: w.px, stream: fit(w.px, PANEL_MAX) });
    }
    if let Some(w) = inject::xr_window() {
        out.push(Source { name: "xr", window: w, region: Region::Whole, region_px: w.px, stream: fit(w.px, XR_MAX) });
        let eye = [w.px[0] / 2, w.px[1]];
        out.push(Source { name: "xr-left", window: w, region: Region::LeftHalf, region_px: eye, stream: fit(eye, EYE_MAX) });
    }
    out
}

/// The injected pointer: the source it is over (the one being watched) and where (0..=65535).
#[derive(Default)]
struct PointerState {
    source: Option<Source>,
    at: Option<(u16, u16)>,
    /// Window points of the last position.
    points: [f64; 2],
    /// Modifier keys held (`input::MOD_*`).
    mods: u32,
}

struct Capture {
    stop: Arc<AtomicBool>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

struct QuestDesktop {
    sources: Vec<Source>,
    capture: Option<Capture>,
    pointer: Arc<Mutex<PointerState>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Desktop for QuestDesktop {
    fn screens(&mut self) -> Vec<ScreenInfo> {
        self.sources = sources();
        let mut p = lock(&self.pointer);
        if p.source.as_ref().is_none_or(|s| !self.sources.iter().any(|n| n.name == s.name)) {
            p.source = self.sources.first().cloned();
            p.at = None;
        } else if let Some(s) = p.source.as_ref().and_then(|s| self.sources.iter().find(|n| n.name == s.name)) {
            // the window may have been made again (a new id) or resized
            p.source = Some(s.clone());
        }
        drop(p);
        let mut x = 0;
        self.sources
            .iter()
            .map(|s| {
                let info = ScreenInfo { name: s.name.to_string(), x, y: 0, width: s.stream[0], height: s.stream[1] };
                x += s.stream[0];
                info
            })
            .collect()
    }

    fn capture(&mut self, index: usize, fps: u32, frames: Sender<Frame>) -> Result<(), String> {
        self.capture = None;
        let source = self.sources.get(index).cloned().ok_or_else(|| format!("no source {index}"))?;
        let tap = frame_tap::subscribe(source.window.session, source.window.window, source.region, source.stream, fps)?;
        log(&format!("taps {} ({}x{} px of the window) at {}x{}, {fps} a second", source.name, source.region_px[0], source.region_px[1], source.stream[0], source.stream[1]));
        {
            let mut p = lock(&self.pointer);
            if p.source.as_ref() != Some(&source) {
                p.source = Some(source.clone());
                p.at = None;
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        std::thread::Builder::new()
            .name("deskhost-frames".into())
            .spawn(move || {
                // the tap's RGBA frames as NV12 (BT.709 limited), on the host's clock
                let mut nv12 = Vec::new();
                while !thread_stop.load(Ordering::Acquire) {
                    let f = match tap.frames.recv_timeout(Duration::from_millis(100)) {
                        Ok(f) => f,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    makepad_video::nv12::rgba8_to_nv12(&f.rgba, f.width, f.height, makepad_video::VideoColor::BT709, &mut nv12);
                    let age = makepad_hal::clock_ns().saturating_sub(f.captured_ns);
                    let frame = Frame {
                        picture: Picture::Nv12(std::mem::take(&mut nv12)),
                        width: f.width,
                        height: f.height,
                        captured_ns: host::clock_ns().saturating_sub(age),
                        _hold: Box::new(()),
                    };
                    if frames.send(frame).is_err() {
                        break;
                    }
                }
                // dropping the tap unsubscribes
                drop(tap);
            })
            .map_err(|e| format!("frame thread: {e}"))?;
        self.capture = Some(Capture { stop });
        Ok(())
    }

    fn stop_capture(&mut self) {
        if self.capture.take().is_some() {
            log("stops the tap (nobody watches)");
        }
    }

    fn cursor(&mut self, index: usize) -> (u16, u16, bool, u8) {
        let p = lock(&self.pointer);
        match (p.source.as_ref(), p.at, self.sources.get(index)) {
            (Some(s), Some((x, y)), Some(shown)) if s.name == shown.name => (x, y, true, 0),
            _ => (0, 0, false, 0),
        }
    }

    fn input(&self) -> InputFactory {
        let pointer = self.pointer.clone();
        Arc::new(move || Ok(Box::new(AppInput { pointer: pointer.clone() }) as Box<dyn Input>))
    }
}

/// A viewer's input into the app.
struct AppInput {
    pointer: Arc<Mutex<PointerState>>,
}

/// `input::MOD_*` of a modifier key (makepad `KeyCode` index), 0 for other keys.
fn modifier(keycode: u32) -> u32 {
    use makepad_hal::input::*;
    use makepad_key_code::KeyCode;
    match KeyCode::from_index(keycode) {
        KeyCode::Shift => MOD_SHIFT,
        KeyCode::Control => MOD_CONTROL,
        KeyCode::Alt => MOD_ALT,
        KeyCode::Logo => MOD_LOGO,
        _ => 0,
    }
}

impl Input for AppInput {
    fn pointer(&mut self, x: u16, y: u16) {
        let mut p = lock(&self.pointer);
        let Some(s) = p.source.clone() else { return };
        let dpi = s.window.dpi.max(0.01) as f64;
        let points = [x as f64 / 65535.0 * s.region_px[0] as f64 / dpi, y as f64 / 65535.0 * s.region_px[1] as f64 / dpi];
        p.at = Some((x, y));
        p.points = points;
        inject::pointer(&s.window, points[0], points[1], Pointer::Move, p.mods);
    }

    fn button(&mut self, down: bool, button: u8) {
        let p = lock(&self.pointer);
        let Some(s) = p.source.clone() else { return };
        // the wire's buttons (left, right, middle, back, forward) are Cx's mask bits in order
        let mask = 1u32 << button.min(7);
        inject::pointer(&s.window, p.points[0], p.points[1], if down { Pointer::Down(mask) } else { Pointer::Up(mask) }, p.mods);
    }

    fn scroll(&mut self, dx: i32, dy: i32) {
        let p = lock(&self.pointer);
        let Some(s) = p.source.clone() else { return };
        // 1/120 notch units (+dy: the wheel away from the user, the content up) to points (a
        // notch is 40; +y: the view moves down), as the WM's wheel
        let per_unit = 40.0 / makepad_desk_link::SCROLL_UNITS_PER_NOTCH as f64;
        inject::pointer(&s.window, p.points[0], p.points[1], Pointer::Scroll { dx: dx as f64 * per_unit, dy: -(dy as f64) * per_unit }, p.mods);
    }

    fn key(&mut self, down: bool, keycode: u32) {
        let mods = {
            let mut p = lock(&self.pointer);
            let m = modifier(keycode);
            if down {
                p.mods |= m;
            } else {
                p.mods &= !m;
            }
            p.mods
        };
        inject::key(down, keycode, false, mods);
    }

    fn gamepad(&mut self, _slot: u8, _state: &GamepadState) {}

    fn gamepad_gone(&mut self, _slot: u8) {}
}

/// Starts the endpoint on its own thread and registers its devhub switch. `name`: the device's
/// devhub name (the viewer's tab, the certificate's name).
pub fn start(files: &Path, name: String, conf: DeskConf) {
    let settings = Arc::new(Settings { file: files.join("deskhost.conf"), conf, name });
    let (ctl, wake_rx) = HostControl::new();
    let ctl = Arc::new(ctl);
    {
        let (set_settings, set_ctl, state_settings, state_ctl) = (settings.clone(), ctl.clone(), settings.clone(), ctl.clone());
        makepad_devhub::embed::set_desk_link_control(makepad_devhub::embed::DeskLinkControl {
            set: Box::new(move |on| {
                set_settings.switch(on)?;
                set_ctl.send(Cmd::Settings);
                log(&format!("switched {} by devhub", if on { "on" } else { "off" }));
                Ok(format!("switched {}", if on { "on" } else { "off" }))
            }),
            state: Box::new(move || {
                let r = state_ctl.report.lock().unwrap_or_else(|e| e.into_inner()).clone();
                makepad_devhub::embed::DeskLinkState { on: state_settings.now().enabled, port: r.port(), note: r.line() }
            }),
        });
    }
    let _ = std::thread::Builder::new().name("deskhost".into()).spawn(move || {
        let now = settings.now();
        log(&format!("{} (port {}, {} a second); fleet identity in {}", if now.enabled { "on" } else { "off" },
            now.listen.as_ref().map_or(0, |a| a.port()), now.fps, makepad_network::fleet::id::dir().display()));
        let mut desktop = QuestDesktop { sources: Vec::new(), capture: None, pointer: Default::default() };
        let line: Log = Arc::new(log);
        host::host_main(&mut desktop, &|| settings.now(), &ctl, &wake_rx, line, None);
        log("stopped");
    });
}

