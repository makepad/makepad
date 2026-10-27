//! A remote headset as this process's XR display (`MAKEPAD_XR_REMOTE=1`): the shell side of
//! remote rendering. The app sees what a native headset gives it (`info_flags::XR`, a
//! `window_kind::XR` window paced by `XrFrame`s) with `info_flags::XR_OFFSCREEN`: the window has
//! no display, its XR passes render into their own textures and the app hands the frames to the
//! encoder itself (`makepad_xr_stream`). The poses come from the network
//! (`crate::xr_remote::hal_xr_remote_pose`, one per frame the headset wants); this slot only paces
//! the window with them. Reprojection is the headset's (it shows each picture with the eyes it
//! was rendered with).
//!
//! One frame is asked at a time: a pose that arrives while the app builds a frame waits (the
//! newest one wins) and is asked for once the app says it built the frame
//! (`hal_xr_remote_drawn`): the app says so when it has a picture slot for the next one, so
//! the frames in flight to the encoder stay bounded by the app's slots.

use super::{lock, Key};
use crate::buffers::EventQueue;
use crate::backend::exec::OsCmd;
use crate::session::World;
use crate::types::window_kind;
use std::collections::HashMap;
use crate::types::window_state;
use crate::Id;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// A frame asked for and not built within this is asked again (its batch was dropped).
const REASK: Duration = Duration::from_millis(250);
/// The eye size until the headset says its own.
const DEFAULT_EYE: [u32; 2] = [1680, 1760];

/// `MAKEPAD_XR_REMOTE=1`: this process offers a remote headset slot.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_XR_REMOTE").is_some_and(|v| v == "1"))
}

#[derive(Default)]
struct State {
    window: Option<Key>,
    eye: Option<[u32; 2]>,
    refresh: f32,
    /// The pose the app draws with now, and when it was asked.
    asked: Option<(u64, Instant)>,
    /// The newest pose that came while a frame was being built: (seq, display_ns, XrFrame bytes).
    waiting: Option<(u64, u64, Vec<u8>)>,
    /// The XR window's events, by session, until the backend's next step delivers them.
    inbox: HashMap<u32, EventQueue>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
}

struct Remote {
    state: Mutex<State>,
}

fn remote() -> &'static Remote {
    static R: OnceLock<Remote> = OnceLock::new();
    R.get_or_init(|| Remote { state: Mutex::new(State { refresh: 72.0, ..Default::default() }) })
}

/// Queues events for the XR window and wakes the HAL (its backend's step delivers them).
fn push(s: &mut State, key: Key, f: impl FnOnce(&mut EventQueue, Id)) {
    let q = s.inbox.entry(key.0).or_default();
    q.set_time(crate::clock_ns());
    f(q, key.1);
    if let Some(wake) = s.wake.clone() {
        wake();
    }
}

/// What wakes the HAL for the XR window's events (the backend's notify).
pub(crate) fn set_wake(wake: Arc<dyn Fn() + Send + Sync>) {
    lock(&remote().state).wake = Some(wake);
}

/// Delivers the XR window's queued events of session `sid` (every session for HOST).
pub(crate) fn drain_events(sid: u32, world: &mut World) {
    let queues = {
        let mut s = lock(&remote().state);
        if s.inbox.is_empty() {
            return;
        }
        if sid == crate::session::HOST { std::mem::take(&mut s.inbox) } else { s.inbox.remove(&sid).map(|q| HashMap::from([(sid, q)])).unwrap_or_default() }
    };
    for (recipient, mut q) in queues {
        q.drain_with(|e, p| world.window_event(recipient, e, p));
    }
}

/// A window command the remote headset answers itself (its XR window is no window of the window
/// system); true when taken.
pub(crate) fn command(sid: u32, cmd: &OsCmd) -> bool {
    if !enabled() {
        return false;
    }
    match cmd {
        OsCmd::WindowCreate { id, kind, .. } if kind & 0xff == window_kind::XR => {
            create_window((sid, *id));
            true
        }
        OsCmd::WindowClose { id } if close_window((sid, *id)) => {
            let mut s = lock(&remote().state);
            push(&mut s, (sid, *id), |q, id| q.closed(id));
            true
        }
        OsCmd::WindowClose { id } | OsCmd::FrameRequest { id } | OsCmd::WindowVisuals { id, .. } | OsCmd::WindowGeom { id, .. }
        | OsCmd::WindowState { id, .. } | OsCmd::WindowTitle { id, .. } => is_window((sid, *id)),
        _ => false,
    }
}

fn eye(s: &State) -> [u32; 2] {
    s.eye.unwrap_or(DEFAULT_EYE)
}

/// `window_kind::XR` window `key` is made: the stereo view, two eyes side by side.
fn create_window(key: Key) {
    let mut s = lock(&remote().state);
    if let Some(old) = s.window.replace(key) {
        eprintln!("[hal-xr-remote] a second XR window {key:?} replaces {old:?}");
    }
    s.asked = None;
    let (e, refresh) = (eye(&s), s.refresh);
    push(&mut s, key, |q, id| {
        q.display(id, 1., refresh);
        q.geom(id, 0., 0., 2. * e[0] as f64, e[1] as f64, 1., window_state::RESTORE);
    });
    eprintln!("[hal-xr-remote] XR window {key:?}: {}x{} px", 2 * e[0], e[1]);
}

fn close_window(key: Key) -> bool {
    let mut s = lock(&remote().state);
    if s.window != Some(key) {
        return false;
    }
    s.window = None;
    s.asked = None;
    s.waiting = None;
    true
}

fn is_window(key: Key) -> bool {
    lock(&remote().state).window == Some(key)
}

/// The headset's eye size and refresh: the XR window takes them.
pub(crate) fn attach(eye_px: [u32; 2], refresh: f32) {
    let mut s = lock(&remote().state);
    s.eye = Some([eye_px[0].max(16) & !1, eye_px[1].max(16) & !1]);
    s.refresh = refresh;
    let (window, e) = (s.window, eye(&s));
    if let Some(key) = window {
        push(&mut s, key, |q, id| {
            q.display(id, 1., refresh);
            q.geom(id, 0., 0., 2. * e[0] as f64, e[1] as f64, 1., window_state::RESTORE);
        });
    }
}

/// A pose for the next frame. `false` while there is no XR window to draw it.
pub(crate) fn pose(seq: u64, display_ns: u64, frame: &[u8]) -> bool {
    let mut s = lock(&remote().state);
    let Some(key) = s.window else { return false };
    if s.asked.is_some_and(|(_, at)| at.elapsed() < REASK) {
        s.waiting = Some((seq, display_ns, frame.to_vec()));
        return true;
    }
    ask(&mut s, key, seq, display_ns, frame);
    true
}

fn ask(s: &mut State, key: Key, seq: u64, display_ns: u64, frame: &[u8]) {
    s.asked = Some((seq, Instant::now()));
    let period = (1e9 / s.refresh.max(1.0) as f64) as u64;
    push(s, key, |q, id| {
        q.xr_frame(id, display_ns, frame);
        q.frame_ready(id, display_ns + period);
    });
}

/// The app built the frame of the pose it was asked for and can take another: the newest pose
/// that waited is asked for now (else the next one that comes is).
pub(crate) fn drawn() {
    let mut s = lock(&remote().state);
    s.asked = None;
    if let (Some(key), Some((seq, display_ns, frame))) = (s.window, s.waiting.take()) {
        ask(&mut s, key, seq, display_ns, &frame);
    }
}
