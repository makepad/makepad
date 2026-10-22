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
    Cmd(u32, OsCmd),
    CloseSession(u32),
    Lend { host: Id, session: u32, alias: Id },
    Reveal,
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
        let native = Arc::new(Native { hwnd: hwnd.0 as usize, pixels: AtomicU64::new(0), token, popup, alive: AtomicBool::new(true) });
        lock(&self.natives).insert(key, native.clone());
        native
    }

    fn unregister(&self, key: Key) {
        if let Some(n) = lock(&self.natives).remove(&key) {
            n.alive.store(false, Ordering::Release);
        }
        lock(&self.pacer.wins).remove(&key);
    }

    /// The pacer thread: waits for the credits of the windows that asked for a frame.
    fn pace(&'static self) {
        // WaitForMultipleObjects takes 64 handles; one is the control event
        const MAX_WAIT: usize = 63;
        loop {
            let mut ready = Vec::new();
            let mut waits: Vec<(Key, Arc<Waitable>)> = Vec::new();
            let now = std::time::Instant::now();
            let mut timeout: Option<std::time::Duration> = None;
            {
                let mut wins = lock(&self.pacer.wins);
                for (key, w) in wins.iter_mut() {
                    if !w.demand {
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
            let next_ns = crate::clock_ns() + 16_666_667;
            for key in ready {
                self.push(key, |q, id| q.frame_ready(id, next_ns));
            }
            if waits.len() > MAX_WAIT {
                // more paced windows than one wait takes: the rest are served on later turns
                eprintln!("[hal-win32] {} windows wait for frames at once; pacing the first {MAX_WAIT} per turn", waits.len());
                waits.truncate(MAX_WAIT);
            }
            let mut handles: Vec<HANDLE> = waits.iter().map(|(_, h)| h.0).collect();
            handles.push(self.pacer.event);
            let ms = timeout.map_or(u32::MAX, |t| t.as_millis().clamp(1, u32::MAX as u128 - 1) as u32);
            let r = unsafe { WaitForMultipleObjects(&handles, false, ms) };
            if r == WAIT_TIMEOUT {
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
                let next_ns = crate::clock_ns() + 16_666_667;
                self.push(key, |q, id| q.frame_ready(id, next_ns));
            } else if index > waits.len() {
                eprintln!("[hal-win32] pacer wait failed: {r:?}");
                std::thread::sleep(NOMINAL_FRAME);
            }
        }
    }
}

/// The system DPI factor, with the process made per-monitor-v2 aware first (before any HWND):
/// offscreen windows render at the scale real ones would have.
pub fn system_dpi() -> f32 {
    static DPI: OnceLock<f32> = OnceLock::new();
    *DPI.get_or_init(window::dpi_awareness)
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
