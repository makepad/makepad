//! The Win32 shell of the HAL. The HAL owns the process main thread and runs its message loop
//! ([`run_main_loop`]); every session's Cx runs on its own thread and talks to the shell only
//! through the HAL: window and OS commands become [`Op`]s posted to the main thread's dispatch
//! window, input and window events become `RawEvent`s the pumping thread collects
//! ([`Shared::drain_events`]). The main thread never takes the HAL's state lock and never waits
//! for a session; a WndProc never waits for Cx (hit tests answer from the last published state).
//!
//! Frame pacing: each paced window's DXGI frame-latency waitable is a credit semaphore refilled
//! when a present retires. The pacer thread waits for the credits of the windows that asked for
//! a frame (`FrameRequest`) and answers `FrameReady`; a credit taken and not yet spent on a
//! present is kept for the next request (DXGI gives no way to return it).
//!
//! Moved from os/windows/{win32_app, win32_window, win32_event, win32_screen}.rs; the Cx-facing
//! callback enum is gone, producers write HAL events. The shell services live beside the windows:
//! `ime` (IMM composition), `menu` (the app menu), `dialog` (file dialogs on a persistent STA
//! worker), `drag` (OLE drag and drop), `clipboard` (text, images, file lists) and `rawinput`
//! (raw mouse motion and the pointer lock).

pub(crate) mod a11y;
mod clipboard;
mod dialog;
mod drag;
mod identity;
mod ime;
mod keys;
mod process;
pub use process::{activate_process, open_url};
mod menu;
mod rawinput;
mod tray;
mod window;

use super::super::exec::OsCmd;
use crate::buffers::EventQueue;
use crate::types::*;
use crate::Id;
use makepad_windows_sys::Win32::Foundation::*;
use makepad_windows_sys::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};
use makepad_windows_sys::Win32::UI::WindowsAndMessaging::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};

pub(crate) type Key = (u32, Id);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `MAKEPAD_HAL_LOG=1`: the shell says what it does.
pub(crate) fn log_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_HAL_LOG").is_some())
}

/// The dispatch window's message: ops are queued.
const WM_HAL_OPS: u32 = WM_APP + 1;

pub(crate) enum Op {
    A11yNotify(u64, a11y::Update),
    Cmd(u32, OsCmd),
    CloseSession(u32),
    Lend { host: Id, session: u32, alias: Id },
    Reveal,
    /// A web layer's content: its URL and the visual it renders into.
    SystemWebViewCreate { key: Key, url: Vec<u8>, visual: WebVisual },
    SystemWebViewOp { key: Key, op: u32, arg: u64, data: Vec<u8> },
    /// Where a frame placed a web layer: its window, client origin and content size in pixels
    /// and visibility.
    SystemWebViewPlace { key: Key, hwnd: usize, origin: [i32; 2], size: [i32; 2], visible: bool },
    /// The composition device was made again: the layer's content renders into a new visual.
    SystemWebViewRetarget { key: Key, visual: WebVisual },
    SystemWebViewDestroy(Key),
}

/// The DirectComposition visual a web layer's content renders into.
pub(crate) type WebVisual = crate::backend::windows::d3d11::dcomp::Agile<makepad_windows_sys::composition::Win32::Graphics::DirectComposition::IDCompositionVisual2>;

/// A native layer's input region in one window, as its last committed frame placed it (window
/// pixels): the clip before the affine transform `[m11, m12, m21, m22, dx, dy]`, minus the
/// occluders. The WndProc routes pointer input inside it to the layer's content.
#[derive(Clone, Debug)]
pub(crate) struct LayerRegion {
    pub key: Key,
    pub rect: [f32; 4],
    pub clip: [f32; 4],
    pub radius: [f32; 4],
    pub transform: [f32; 6],
    pub flags: u32,
    pub occluders: Vec<[f32; 4]>,
}

/// How client pixels map into a layer's content: the inverse of its affine transform, then
/// the rect's origin. Kept for a press or contact whose layer region goes away meanwhile.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LayerMap {
    transform: [f32; 6],
    origin: [f32; 2],
}

impl LayerMap {
    /// The layer-local pixel of a client pixel, inside the region or not.
    pub fn local(&self, x: f32, y: f32) -> (f32, f32) {
        let [a, b, c, d, dx, dy] = self.transform;
        let det = a * d - b * c;
        let (ux, uy) = if det.abs() < f32::EPSILON { (x, y) } else { (((x - dx) * d - (y - dy) * c) / det, ((y - dy) * a - (x - dx) * b) / det) };
        (ux - self.origin[0], uy - self.origin[1])
    }
}

impl LayerRegion {
    pub fn map(&self) -> LayerMap {
        LayerMap { transform: self.transform, origin: [self.rect[0], self.rect[1]] }
    }

    /// The layer-local pixel of a client pixel when it is inside the input region.
    pub fn local(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let inside = |r: [f32; 4], x: f32, y: f32| x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3];
        if self.flags & crate::native_layer::layer_flags::INPUT_PASSTHROUGH != 0 || self.flags & crate::native_layer::layer_flags::VISIBLE == 0
            || self.occluders.iter().any(|r| inside(*r, x, y)) {
            return None;
        }
        let [a, b, c, d, dx, dy] = self.transform;
        let det = a * d - b * c;
        if det.abs() < f32::EPSILON { return None; }
        let ux = ((x - dx) * d - (y - dy) * c) / det;
        let uy = ((y - dy) * a - (x - dx) * b) / det;
        if !inside(self.clip, ux, uy) { return None; }
        let (lx, ly) = (ux - self.rect[0], uy - self.rect[1]);
        // outside a rounded corner
        let (w, h) = (self.rect[2], self.rect[3]);
        let r = self.radius.map(|r| r.max(0.0).min(w.min(h) * 0.5));
        let corner = |cx: f32, cy: f32, r: f32| r > 0.0 && ((lx - cx).powi(2) + (ly - cy).powi(2)) > r * r;
        if (lx < r[0] && ly < r[0] && corner(r[0], r[0], r[0]))
            || (lx > w - r[1] && ly < r[1] && corner(w - r[1], r[1], r[1]))
            || (lx > w - r[2] && ly > h - r[2] && corner(w - r[2], h - r[2], r[2]))
            || (lx < r[3] && ly > h - r[3] && corner(r[3], h - r[3], r[3])) {
            return None;
        }
        Some((lx, ly))
    }
}

#[derive(Default)]
struct Run {
    wanted: bool,
    ready: Option<Result<(), String>>,
    stopped: bool,
}

/// What the D3D11 backend needs of a window: its HWND and client pixels, published by the main
/// thread. The HWND outlives every swapchain made for it: the backend holds this record.
pub struct Native {
    pub hwnd: usize,
    /// client size in pixels, w << 32 | h
    pub pixels: AtomicU64,
    /// changes when the window is made again (a new HWND: a new swapchain)
    pub token: u64,
    /// the window is a popup: presented without frame-latency pacing
    pub popup: bool,
    /// the window still exists (false after WM_DESTROY)
    pub alive: AtomicBool,
    /// transparent or with a system backdrop: its pixels' alpha shows what is behind
    pub see_through: AtomicBool,
}

impl Native {
    pub fn size(&self) -> [u32; 2] {
        let p = self.pixels.load(Ordering::Acquire);
        [(p >> 32) as u32, p as u32]
    }
}

/// A swapchain's frame-latency waitable, closed when the last holder lets go.
pub struct Waitable(pub HANDLE);
// SAFETY: a kernel object handle is usable from any thread
unsafe impl Send for Waitable {}
unsafe impl Sync for Waitable {}
impl Drop for Waitable {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[derive(Default)]
struct PaceWin {
    waitable: Option<Arc<Waitable>>,
    /// its frames are composition surfaces: paced by the compositor's clock
    compositor: bool,
    demand: bool,
    /// a credit taken from the waitable and not yet spent on a present
    credit: bool,
    /// when the window last heard FrameReady: a window without a swapchain yet (its first frame
    /// is still being prepared) is paced at the display's nominal rate, not as fast as it asks
    last_ready: Option<std::time::Instant>,
}

/// The nominal frame interval of a window that has no frame-latency waitable yet.
const NOMINAL_FRAME: std::time::Duration = std::time::Duration::from_nanos(16_666_667);

struct Pacer {
    wins: Mutex<HashMap<Key, PaceWin>>,
    /// wakes the pacer thread when demand or the set of waitables changed
    event: HANDLE,
    started: AtomicBool,
}
// SAFETY: the event handle is a process-wide kernel object
unsafe impl Send for Pacer {}
unsafe impl Sync for Pacer {}

pub struct Shared {
    ops: Mutex<Vec<Op>>,
    inbox: Mutex<HashMap<u32, EventQueue>>,
    natives: Mutex<HashMap<Key, Arc<Native>>>,
    wake: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
    run: Mutex<Run>,
    ready: Condvar,
    announced: AtomicBool,
    flags: AtomicU32,
    revealed: AtomicBool,
    dpi: AtomicU32,
    /// the message-only window ops are posted to (0 until the loop runs)
    dispatch: AtomicUsize,
    next_token: AtomicU64,
    screens: Mutex<Vec<crate::ScreenInfo>>,
    pacer: Pacer,
    /// native layer input regions by HWND, topmost last
    regions: Mutex<HashMap<usize, Arc<Vec<LayerRegion>>>>,
}

impl Shared {
    pub fn global() -> &'static Shared {
        static SHARED: OnceLock<Shared> = OnceLock::new();
        SHARED.get_or_init(|| Shared {
            ops: Mutex::new(Vec::new()),
            inbox: Mutex::new(HashMap::new()),
            natives: Mutex::new(HashMap::new()),
            wake: Mutex::new(Vec::new()),
            run: Mutex::new(Run::default()),
            ready: Condvar::new(),
            announced: AtomicBool::new(false),
            flags: AtomicU32::new(0),
            revealed: AtomicBool::new(false),
            dpi: AtomicU32::new(1f32.to_bits()),
            dispatch: AtomicUsize::new(0),
            next_token: AtomicU64::new(1),
            screens: Mutex::new(Vec::new()),
            regions: Mutex::new(HashMap::new()),
            pacer: Pacer {
                wins: Mutex::new(HashMap::new()),
                event: unsafe { CreateEventW(None, false, false, None) }.expect("pacer event"),
                started: AtomicBool::new(false),
            },
        })
    }

    // ---- the backend side (session threads)

    /// A backend wants windows: the main loop makes its dispatch window. Fails at once when the
    /// host never announced a main loop.
    pub fn start(&'static self, flags: u32, wake: Arc<dyn Fn() + Send + Sync>) -> Result<(), String> {
        if !self.announced.load(Ordering::Acquire) {
            return Err("Windows windows require expect_main_loop/run_main_loop on the main thread".into());
        }
        self.flags.fetch_or(flags, Ordering::Release);
        lock(&self.wake).push(wake);
        if !self.pacer.started.swap(true, Ordering::AcqRel) {
            std::thread::Builder::new()
                .name("makepad-win32-pacer".into())
                .spawn(move || self.pace())
                .map_err(|e| format!("pacer thread: {e}"))?;
        }
        let mut run = lock(&self.run);
        run.wanted = true;
        self.ready.notify_all();
        while run.ready.is_none() && !run.stopped {
            run = self.ready.wait(run).unwrap_or_else(|e| e.into_inner());
        }
        run.ready.clone().unwrap_or_else(|| Err("Windows shell stopped during startup".into()))
    }

    pub fn dpi(&self) -> f32 {
        f32::from_bits(self.dpi.load(Ordering::Acquire))
    }

    fn flags(&self) -> u32 {
        self.flags.load(Ordering::Acquire)
    }

    /// Takes a validated command; false when the shell does not implement it (the executor
    /// then answers it as a backend without a window system would).
    pub fn command(&self, sid: u32, cmd: OsCmd) -> bool {
        if !window::handles(&cmd) {
            return false;
        }
        if let OsCmd::FrameRequest { id } = cmd {
            self.request_frame((sid, id));
            return true;
        }
        self.post(Op::Cmd(sid, cmd));
        true
    }

    pub fn session_closed(&self, sid: u32) {
        lock(&self.inbox).remove(&sid);
        lock(&self.pacer.wins).retain(|k, _| k.0 != sid);
        self.post(Op::CloseSession(sid));
    }

    /// A web layer's content, made on the shell thread.
    pub(crate) fn web_view_create(&self, key: Key, url: Vec<u8>, visual: WebVisual) {
        self.post(Op::SystemWebViewCreate { key, url, visual });
    }

    pub(crate) fn web_view_op(&self, key: Key, op: u32, arg: u64, data: Vec<u8>) {
        self.post(Op::SystemWebViewOp { key, op, arg, data });
    }

    pub(crate) fn web_view_destroy(&self, key: Key) {
        self.post(Op::SystemWebViewDestroy(key));
    }

    /// A committed frame moved, resized, showed or hid a web layer.
    pub(crate) fn web_view_place(&self, key: Key, hwnd: usize, origin: [i32; 2], size: [i32; 2], visible: bool) {
        self.post(Op::SystemWebViewPlace { key, hwnd, origin, size, visible });
    }

    pub(crate) fn web_view_retarget(&self, key: Key, visual: WebVisual) {
        self.post(Op::SystemWebViewRetarget { key, visual });
    }

    /// Wakes every session's pumping thread (the HAL looks at the composition device).
    pub(crate) fn wake_all(&self) {
        let wakes = lock(&self.wake).clone();
        for wake in wakes {
            wake();
        }
    }

    /// The input regions of `hwnd`'s native layers, as its last committed frame placed them.
    pub(crate) fn set_layer_regions(&self, hwnd: usize, regions: Vec<LayerRegion>) {
        let mut map = lock(&self.regions);
        if regions.is_empty() { map.remove(&hwnd); } else { map.insert(hwnd, Arc::new(regions)); }
    }

    pub(crate) fn layer_regions(&self, hwnd: usize) -> Option<Arc<Vec<LayerRegion>>> {
        lock(&self.regions).get(&hwnd).cloned()
    }

    pub fn lend(&self, host: Id, session: u32, alias: Id) {
        self.post(Op::Lend { host, session, alias });
    }

    /// The native window of a key, if the main thread has made it.
    pub fn native(&self, key: Key) -> Option<Arc<Native>> {
        lock(&self.natives).get(&key).cloned()
    }

    /// A frame of `key` was accepted by its swapchain: the window's held credit is spent, and the
    /// launcher-hidden windows show.
    pub fn presented(&self, key: Key) {
        if let Some(w) = lock(&self.pacer.wins).get_mut(&key) {
            w.credit = false;
        }
        if self.flags() & open_flags::HIDDEN_UNTIL_PRESENT != 0 && !self.revealed.swap(true, Ordering::AcqRel) {
            self.post(Op::Reveal);
        }
    }

    /// The swapchain of `key` paces by `waitable` (None: an unpaced popup or a retired chain).
    pub fn set_waitable(&self, key: Key, waitable: Option<Arc<Waitable>>) {
        let mut wins = lock(&self.pacer.wins);
        let w = wins.entry(key).or_default();
        w.waitable = waitable;
        w.credit = false;
        drop(wins);
        unsafe {
            let _ = SetEvent(self.pacer.event);
        }
    }

    /// The window's frames are composition surfaces from now on (no swapchain credits): the
    /// pacer serves it on the compositor's clock.
    pub fn set_compositor_paced(&self, key: Key, on: bool) {
        lock(&self.pacer.wins).entry(key).or_default().compositor = on;
        unsafe {
            let _ = SetEvent(self.pacer.event);
        }
    }

    fn request_frame(&self, key: Key) {
        lock(&self.pacer.wins).entry(key).or_default().demand = true;
        unsafe {
            let _ = SetEvent(self.pacer.event);
        }
    }

    /// Moves what the main thread queued into the sessions' queues; returns the windows that closed.
    pub fn drain_events(&self, sid: u32, world: &mut crate::session::World) -> Vec<Key> {
        let queues: Vec<(u32, EventQueue)> = if sid == crate::session::HOST {
            std::mem::take(&mut *lock(&self.inbox)).into_iter().collect()
        } else {
            lock(&self.inbox).remove(&sid).map(|q| vec![(sid, q)]).unwrap_or_default()
        };
        let mut closed = Vec::new();
        for (recipient, mut q) in queues {
            q.drain_with(|e, p| {
                if e.tag == crate::events::event_tag::Closed {
                    closed.push((recipient, e.window));
                }
                world.window_event(recipient, e, p);
            });
        }
        closed
    }

    // ---- shared by the main thread and the pacer

    fn post(&self, op: Op) {
        lock(&self.ops).push(op);
        let hwnd = self.dispatch.load(Ordering::Acquire);
        if hwnd != 0 {
            unsafe {
                let _ = PostMessageW(Some(HWND(hwnd as _)), WM_HAL_OPS, WPARAM(0), LPARAM(0));
            }
        }
    }

    fn take_ops(&self) -> Vec<Op> {
        std::mem::take(&mut *lock(&self.ops))
    }

    /// An event for a session's pumping thread.
    pub(crate) fn push(&self, key: Key, f: impl FnOnce(&mut EventQueue, Id)) {
        {
            let mut inbox = lock(&self.inbox);
            let q = inbox.entry(key.0).or_default();
            q.set_time(crate::clock_ns());
            f(q, key.1);
        }
        let wakes = lock(&self.wake).clone();
        for wake in wakes {
            wake();
        }
    }

    fn register(&self, key: Key, hwnd: HWND, popup: bool) -> Arc<Native> {
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        let native = Arc::new(Native { hwnd: hwnd.0 as usize, pixels: AtomicU64::new(0), token, popup, alive: AtomicBool::new(true), see_through: AtomicBool::new(false) });
        lock(&self.natives).insert(key, native.clone());
        native
    }

    fn unregister(&self, key: Key) {
        if let Some(n) = lock(&self.natives).remove(&key) {
            n.alive.store(false, Ordering::Release);
        }
        lock(&self.pacer.wins).remove(&key);
    }

    /// The pacer thread: waits for the credits of the windows that asked for a frame. Windows
    /// whose frames are composition surfaces (no swapchain credits) are served on the
    /// compositor's clock: `DCompositionWaitForCompositorClock` where the system has it,
    /// otherwise the DWM-flush ticker (`Ticker`). Either way the control event and the
    /// swapchain waitables stay in the same wait.
    fn pace(&'static self) {
        // WaitForMultipleObjects takes 64 handles; one is the control event, one the ticker's
        const MAX_WAIT: usize = 62;
        let clock = compositor_clock();
        let ticker = if clock.is_none() { Ticker::start() } else { None };
        // the compositor clock answered with this status instead of a tick (the display is off,
        // or the clock failed): clocked windows are served at the nominal rate meanwhile
        let mut fallback: Option<u32> = None;
        // in that fallback: when the clocked windows are due next (wakes for other handles do not
        // push it back)
        let mut fallback_due = std::time::Instant::now();
        let mut period = Period::default();
        // clocked windows last served without a tick (the compositor was quiet)
        let mut kicked: Option<std::time::Instant> = None;
        loop {
            let mut ready = Vec::new();
            let mut waits: Vec<(Key, Arc<Waitable>)> = Vec::new();
            let mut clocked: Vec<Key> = Vec::new();
            let now = std::time::Instant::now();
            let mut timeout: Option<std::time::Duration> = None;
            {
                let mut wins = lock(&self.pacer.wins);
                for (key, w) in wins.iter_mut() {
                    if !w.demand {
                        continue;
                    }
                    if w.compositor && w.waitable.is_none() && (clock.is_some() || ticker.is_some()) {
                        clocked.push(*key);
                        continue;
                    }
                    match &w.waitable {
                        // a credit still held: the compositor is ready for this window
                        Some(_) if w.credit => {
                            w.demand = false;
                            w.last_ready = Some(now);
                            ready.push(*key);
                        }
                        Some(h) => waits.push((*key, h.clone())),
                        // no swapchain yet: at the nominal rate
                        None => {
                            let due = w.last_ready.map_or(now, |t| t + NOMINAL_FRAME);
                            if due <= now {
                                w.demand = false;
                                w.last_ready = Some(now);
                                ready.push(*key);
                            } else {
                                let left = due - now;
                                timeout = Some(timeout.map_or(left, |t| t.min(left)));
                            }
                        }
                    }
                }
            }
            let next_ns = crate::clock_ns() + period.ns();
            for key in ready {
                self.push(key, |q, id| q.frame_ready(id, next_ns));
            }
            let limit = if clocked.is_empty() || ticker.is_some() { MAX_WAIT } else { COMPOSITOR_CLOCK_HANDLES - 1 };
            if waits.len() > limit {
                // more paced windows than one wait takes: the rest are served on later turns
                eprintln!("[hal-win32] {} windows wait for frames at once; pacing the first {limit} per turn", waits.len());
                waits.truncate(limit);
            }
            let mut handles: Vec<HANDLE> = waits.iter().map(|(_, h)| h.0).collect();
            handles.push(self.pacer.event);
            if let Some(t) = ticker {
                t.want(!clocked.is_empty());
                if !clocked.is_empty() { handles.push(t.tick); }
            }
            if let (Some(wait), Some(status)) = (clock, fallback.filter(|_| !clocked.is_empty())) {
                // still no clock? a zero wait answers at once
                let r = unsafe { wait(0, std::ptr::null(), 0) };
                if r == WAIT_TIMEOUT.0 || r == WAIT_OBJECT_0.0 {
                    eprintln!("[hal-win32] compositor clock back (was {status:#x}); composed windows follow it again");
                    fallback = None;
                } else if r != status {
                    eprintln!("[hal-win32] compositor clock still unavailable, now {r:#x}{}; nominal rate", occluded(r));
                    fallback = Some(r);
                }
            }
            if fallback.is_some() && !clocked.is_empty() {
                // nominal rate, still waking for the control event and the waitables
                let now = std::time::Instant::now();
                if fallback_due <= now {
                    fallback_due = now + NOMINAL_FRAME;
                    self.serve_clocked(&clocked, NOMINAL_FRAME.as_nanos() as u64);
                    continue;
                }
                let left = fallback_due - now;
                timeout = Some(timeout.map_or(left, |t| t.min(left)));
            }
            if fallback.is_none() && !clocked.is_empty() {
                // the clock (and DwmFlush) only ticks while something is being composed: clocked
                // windows asking after a quiet stretch draw at once (their frame is what the
                // compositor composes next), and a clock silent for two frames does not hold them
                let now = std::time::Instant::now();
                let due = period.last.max(kicked).map_or(now, |t| t + std::time::Duration::from_nanos(2 * period.ns()));
                if due <= now {
                    kicked = Some(now);
                    self.serve_clocked(&clocked, period.ns());
                    continue;
                }
                let left = due - now;
                timeout = Some(timeout.map_or(left, |t| t.min(left)));
            }
            let ms = timeout.map_or(u32::MAX, |t| t.as_millis().clamp(1, u32::MAX as u128 - 1) as u32);
            let r = match clock.filter(|_| !clocked.is_empty() && fallback.is_none()) {
                Some(wait) => {
                    let r = unsafe { wait(handles.len() as u32, handles.as_ptr(), ms) };
                    if r == WAIT_OBJECT_0.0 + handles.len() as u32 {
                        // a composition: the clocked windows draw their next frame
                        self.serve_clocked(&clocked, period.tick());
                        continue;
                    }
                    if r != WAIT_TIMEOUT.0 && r >= WAIT_OBJECT_0.0 + handles.len() as u32 {
                        // the display is off (occluded) or the wait failed
                        eprintln!("[hal-win32] compositor clock unavailable ({r:#x}{}); composed windows at the nominal rate until it returns", occluded(r));
                        fallback = Some(r);
                        fallback_due = std::time::Instant::now() + NOMINAL_FRAME;
                        period.reset();
                        continue;
                    }
                    WAIT_EVENT(r)
                }
                None => unsafe { WaitForMultipleObjects(&handles, false, ms) },
            };
            if r == WAIT_TIMEOUT {
                // due clocked windows are served at the top of the next turn
                continue;
            }
            let index = r.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
            if index < waits.len() {
                let key = waits[index].0;
                let mut wins = lock(&self.pacer.wins);
                if let Some(w) = wins.get_mut(&key) {
                    w.demand = false;
                    w.credit = true;
                    w.last_ready = Some(std::time::Instant::now());
                }
                drop(wins);
                let next_ns = crate::clock_ns() + period.ns();
                self.push(key, |q, id| q.frame_ready(id, next_ns));
            } else if index == waits.len() {
                // the control event: demand or the set of windows changed
            } else if ticker.is_some() && !clocked.is_empty() && index == waits.len() + 1 {
                self.serve_clocked(&clocked, period.tick());
            } else {
                eprintln!("[hal-win32] pacer wait failed: {r:?}");
                std::thread::sleep(NOMINAL_FRAME);
            }
        }
    }

    /// The compositor ticked (or the nominal interval passed): the clocked windows draw.
    fn serve_clocked(&self, keys: &[Key], period_ns: u64) {
        let next_ns = crate::clock_ns() + period_ns;
        let mut wins = lock(&self.pacer.wins);
        for key in keys {
            if let Some(w) = wins.get_mut(key) {
                w.demand = false;
                w.last_ready = Some(std::time::Instant::now());
            }
        }
        drop(wins);
        for key in keys {
            self.push(*key, |q, id| q.frame_ready(id, next_ns));
        }
    }
}

/// STATUS_GRAPHICS_PRESENT_OCCLUDED: the compositor clock is off while the display is.
const PRESENT_OCCLUDED: u32 = 0xC01E_0006;

fn occluded(status: u32) -> &'static str {
    if status == PRESENT_OCCLUDED { ": the display is off or occluded" } else { "" }
}

/// The measured interval of the compositor's ticks, for the next frame's predicted time.
#[derive(Default)]
struct Period {
    last: Option<std::time::Instant>,
    ns: Option<u64>,
}

impl Period {
    fn ns(&self) -> u64 {
        self.ns.unwrap_or(NOMINAL_FRAME.as_nanos() as u64)
    }
    /// A tick now: the interval since the last one (when it is a plausible frame) joins the
    /// average. Answers the predicted interval.
    fn tick(&mut self) -> u64 {
        let now = std::time::Instant::now();
        if let Some(last) = self.last.replace(now) {
            let dt = now.duration_since(last).as_nanos() as u64;
            if (2_000_000..=50_000_000).contains(&dt) {
                self.ns = Some(self.ns.map_or(dt, |p| (p * 7 + dt) / 8));
            }
        }
        self.ns()
    }
    fn reset(&mut self) {
        self.last = None;
    }
}

/// Without a compositor clock (before Windows 11): a thread that waits in `DwmFlush` (which
/// returns when the desktop compositor has composed its next frame) and signals `tick` after
/// each composition, while clocked windows want frames. When DWM composes nothing (the
/// display is off) the flush returns at once; then it ticks at the nominal rate instead of
/// spinning. Every change between the two is logged.
struct Ticker {
    tick: HANDLE,
    arm: HANDLE,
    wanted: AtomicBool,
}
// SAFETY: event handles are process-wide kernel objects
unsafe impl Send for Ticker {}
unsafe impl Sync for Ticker {}

impl Ticker {
    fn start() -> Option<&'static Ticker> {
        let tick = unsafe { CreateEventW(None, false, false, None) }.ok()?;
        let arm = unsafe { CreateEventW(None, false, false, None) }.ok()?;
        let ticker: &'static Ticker = Box::leak(Box::new(Ticker { tick, arm, wanted: AtomicBool::new(false) }));
        match std::thread::Builder::new().name("makepad-win32-dwm-ticker".into()).spawn(move || ticker.run()) {
            Ok(_) => {
                eprintln!("[hal-win32] composed windows follow DWM compositions (DwmFlush)");
                Some(ticker)
            }
            Err(e) => {
                eprintln!("[hal-win32] DWM ticker thread: {e}; composed windows at the nominal rate");
                None
            }
        }
    }

    fn want(&self, on: bool) {
        if self.wanted.swap(on, Ordering::AcqRel) != on && on {
            unsafe {
                let _ = SetEvent(self.arm);
            }
        }
    }

    fn run(&self) {
        use makepad_windows_sys::Win32::Graphics::Dwm::DwmFlush;
        use makepad_windows_sys::Win32::System::Threading::WaitForSingleObject;
        let mut nominal: Option<String> = None;
        let mut fast = 0u32;
        loop {
            if !self.wanted.load(Ordering::Acquire) {
                unsafe { WaitForSingleObject(self.arm, u32::MAX) };
                continue;
            }
            let start = std::time::Instant::now();
            let why = match unsafe { DwmFlush() } {
                Err(e) => Some(format!("DwmFlush failed: {e}")),
                Ok(()) if start.elapsed() < std::time::Duration::from_micros(500) => {
                    fast += 1;
                    (fast >= 3).then(|| String::from("DWM composes nothing (the display is off or occluded)"))
                }
                Ok(()) => {
                    fast = 0;
                    None
                }
            };
            if why != nominal {
                match &why {
                    Some(w) => eprintln!("[hal-win32] {w}; composed windows at the nominal rate"),
                    None => eprintln!("[hal-win32] DWM compositions again; composed windows follow them"),
                }
                nominal = why;
            }
            if nominal.is_some() {
                std::thread::sleep(NOMINAL_FRAME);
            }
            unsafe {
                let _ = SetEvent(self.tick);
            }
        }
    }
}

/// `DCompositionWaitForCompositorClock` takes at most this many handles.
const COMPOSITOR_CLOCK_HANDLES: usize = 32;

/// The compositor clock wait (Windows 11 and later), looked up once; `None` before that, where
/// windows composed through surfaces follow the DWM-flush ticker.
fn compositor_clock() -> Option<makepad_windows_sys::composition::Win32::Graphics::DirectComposition::DCompositionWaitForCompositorClock> {
    use makepad_windows_sys::composition::Win32::Graphics::DirectComposition::{DCompositionWaitForCompositorClock, DCompositionWaitForCompositorClock_EXPORT};
    use makepad_windows_sys::system_web_view::Win32::System::LibraryLoader::LOAD_LIBRARY_SEARCH_DEFAULT_DIRS;
    use makepad_windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW};
    static CLOCK: OnceLock<Option<DCompositionWaitForCompositorClock>> = OnceLock::new();
    *CLOCK.get_or_init(|| unsafe {
        // `MAKEPAD_WIN_COMPOSITOR_CLOCK=0` takes the path of systems without one
        if std::env::var("MAKEPAD_WIN_COMPOSITOR_CLOCK").ok().as_deref() == Some("0") {
            eprintln!("[hal-win32] compositor clock not used (MAKEPAD_WIN_COMPOSITOR_CLOCK=0)");
            return None;
        }
        let (library, export) = DCompositionWaitForCompositorClock_EXPORT;
        let name: Vec<u16> = library.encode_utf16().chain(Some(0)).collect();
        let module = match LoadLibraryExW(makepad_windows_sys::core::PCWSTR(name.as_ptr()), None, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[hal-win32] no compositor clock ({library}: {e})");
                return None;
            }
        };
        let entry = GetProcAddress(module, makepad_windows_sys::core::PCSTR(export.as_ptr()));
        if entry.is_none() {
            eprintln!("[hal-win32] no compositor clock (before Windows 11)");
        }
        // the generated type is the export's metadata signature
        entry.map(|f| std::mem::transmute::<_, DCompositionWaitForCompositorClock>(f))
    })
}

/// The system DPI factor, with the process made per-monitor-v2 aware first (before any HWND):
/// offscreen windows render at the scale real ones would have.
pub fn system_dpi() -> f32 {
    static DPI: OnceLock<f32> = OnceLock::new();
    *DPI.get_or_init(window::dpi_awareness)
}

/// The HWND of a session's main window (shell thread only).
pub(crate) fn session_hwnd(sid: u32) -> Option<usize> {
    window::session_hwnd(sid)
}

/// Gives the keyboard focus back to window `hwnd` from a hosted native view (shell thread only).
pub(crate) fn focus_window(hwnd: usize, tab: Option<bool>) {
    window::focus_window(hwnd, tab)
}

pub fn expect_main_loop() {
    Shared::global().announced.store(true, Ordering::Release);
}

pub fn screens() -> Vec<crate::ScreenInfo> {
    lock(&Shared::global().screens).clone()
}

pub fn host_dark() -> Option<bool> {
    window::system_dark()
}

pub fn stop_main_loop() {
    let s = Shared::global();
    lock(&s.run).stopped = true;
    s.ready.notify_all();
    let hwnd = s.dispatch.load(Ordering::Acquire);
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as _)), WM_HAL_OPS, WPARAM(1), LPARAM(0));
        }
    }
}

/// The main thread's loop. Waits (without a message loop) until a backend asks for windows,
/// then serves the window system until [`stop_main_loop`].
pub fn run_main_loop() {
    let s = Shared::global();
    {
        let mut run = lock(&s.run);
        while !run.wanted && !run.stopped {
            run = s.ready.wait(run).unwrap_or_else(|e| e.into_inner());
        }
        if run.stopped {
            return;
        }
    }
    let result = window::start(s);
    let failed = result.is_err();
    if let Err(e) = &result {
        eprintln!("[hal-win32] shell startup failed: {e}");
    }
    lock(&s.run).ready = Some(result);
    s.ready.notify_all();
    if failed {
        return;
    }
    window::message_loop(s);
    s.dispatch.store(0, Ordering::Release);
}
