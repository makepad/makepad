//! The shell-neutral half of the Apple window systems: the process's main thread belongs to the
//! HAL ([`run_main_loop`](super::run_main_loop)) and runs the OS shell there (AppKit on macOS,
//! UIKit on iOS and tvOS); every session's Cx runs on its own thread and talks to the shell only
//! through the HAL: window and OS commands become [`Op`]s the main thread applies, input and
//! window events become `RawEvent`s the pumping thread collects ([`Shared::drain_events`]).
//! Nothing here waits for the other side: the main thread never takes the HAL's state lock, the
//! pumping thread never calls the OS shell.
//!
//! One `CAMetalLayer` per window ([`Shared::layer`]): the Metal backend renders into the layer's
//! drawables and presents them. What differs per OS (how work reaches the main thread, how its
//! loop is woken, the native view of a window) is each shell's `native` module.

use makepad_relax_api::relax_api;

#[cfg(target_os = "macos")]
use super::appkit::native;
#[cfg(any(target_os = "ios", target_os = "tvos"))]
use super::uikit::native;

/// Runs `f` once on the main thread, in every run-loop mode (also inside the OS's nested loops),
/// for the media module's native objects (location, views) that must live on it. False, and `f`
/// dropped, when the window system is not running: a backend without windows (offscreen) has no
/// main thread work to offer.
pub fn on_main(f: impl FnOnce() + Send + 'static) -> bool {
    if !lock(&Shared::global().run).ready {
        return false;
    }
    let f = Mutex::new(Some(f));
    native::run_on_main(move || {
        if let Some(f) = lock(&f).take() {
            f();
        }
    });
    true
}

/// Queues host work on the HAL main thread, including before the first session opens.
/// Accepted closures run once in common run-loop modes. They must not wait for an app thread.
/// False means no host loop was announced, or it has stopped; the closure was not queued.
pub fn dispatch_main(f: impl FnOnce() + Send + 'static) -> bool {
    let shared = Shared::global();
    let mut run = lock(&shared.run);
    if !shared.host.load(Ordering::Acquire) || (run.stop && run.keep_alive == 0) {
        return false;
    }
    lock(&shared.ops).push(Op::Host(Box::new(f)));
    run.wanted = true;
    let ready = run.ready;
    shared.run_cv.notify_all();
    #[cfg(target_os = "macos")]
    super::appkit::wake_idle_main();
    drop(run);
    if ready && !shared.scheduled.swap(true, Ordering::AcqRel) {
        native::schedule_drain();
    }
    true
}

/// A closure of another crate crossing the relax boundary as plain values: `data` is the boxed
/// closure, `run` calls and frees it, `discard` only frees it (both are `extern "C"` thunks of
/// the closure's own crate, whichever side compiled it).
struct RawWork {
    data: usize,
    run: extern "C" fn(usize),
    discard: extern "C" fn(usize),
    pending: bool,
}

// The closure it carries is `Send` (`dispatch_main`'s bound).
unsafe impl Send for RawWork {}

impl RawWork {
    fn call(mut self) {
        self.pending = false;
        (self.run)(self.data);
    }
}

impl Drop for RawWork {
    fn drop(&mut self) {
        if self.pending {
            (self.discard)(self.data);
        }
    }
}

/// `dispatch_main` for a closure that is not nameable here (code relax compiles): the closure
/// is boxed on its own side and arrives as `data` with two thunks, the main thread calls `run`.
/// False: nothing was queued and `discard` has freed it.
#[relax_api]
pub fn dispatch_main_raw(data: usize, run: usize, discard: usize) -> bool {
    // SAFETY: the addresses are `extern "C" fn(usize)` thunks of the caller (`hal::dispatch_main`).
    let work = RawWork {
        data,
        run: unsafe { std::mem::transmute::<usize, extern "C" fn(usize)>(run) },
        discard: unsafe { std::mem::transmute::<usize, extern "C" fn(usize)>(discard) },
        pending: true,
    };
    dispatch_main(move || work.call())
}

/// Keeps the main loop serving asynchronous native teardown after the app thread returns.
/// Dropping the last guard lets an already requested stop finish. No thread waits on a guard.
#[relax_api(handle)]
pub struct MainLoopGuard {
    _private: (),
}

#[relax_api]
pub fn keep_main_loop_alive() -> Option<MainLoopGuard> {
    let shared = Shared::global();
    let mut run = lock(&shared.run);
    if !shared.host.load(Ordering::Acquire) || (run.stop && run.keep_alive == 0) {
        return None;
    }
    run.keep_alive += 1;
    run.wanted = true;
    shared.run_cv.notify_all();
    #[cfg(target_os = "macos")]
    super::appkit::wake_idle_main();
    Some(MainLoopGuard { _private: () })
}

impl Drop for MainLoopGuard {
    fn drop(&mut self) {
        let shared = Shared::global();
        let mut run = lock(&shared.run);
        run.keep_alive -= 1;
        let wake = run.stop && run.keep_alive == 0 && run.ready;
        drop(run);
        if wake { native::wake_event_loop(); }
    }
}

/// The content view of a window of a session, for the media module's native views. Main thread
/// only (it is an AppKit / UIKit object).
pub fn window_view(sid: u32, id: Id) -> Option<ObjcId> {
    native::window_view(sid, id)
}

/// The displays as the main thread last saw them.
pub fn screens() -> Vec<crate::ScreenInfo> {
    lock(&Shared::global().screens).clone()
}

/// The host will run the main loop (see `makepad_hal::expect_main_loop`).
pub fn expect_main_loop() {
    Shared::global().host.store(true, Ordering::Release);
}

use super::super::exec::OsCmd;
use crate::buffers::EventQueue;
use crate::types::Id;
use makepad_apple_sys::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

/// `MAKEPAD_HAL_LOG=1`: the shell says what it does (windows made, the first frame on screen).
pub(crate) fn log_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_HAL_LOG").is_some())
}

pub(super) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Work for the main thread.
pub enum Op {
    /// Host-native integrations whose OS work must execute on the main thread.
    Host(Box<dyn FnOnce() + Send>),
    /// A validated command of a session, with the number of the newest event the session had
    /// taken from the shell when it sent the command (see [`Shared::push`]): a command that
    /// answers an older state of the OS object than the shell knows of is stale (the text
    /// field's state while newer typing is still on its way to the app).
    Cmd(u32, OsCmd, u64),
    /// The session is gone: its windows close without telling anyone.
    CloseSession(u32),
    /// Change the borrower of a host window; its physical identity and layer stay fixed.
    Lend { host: Id, session: u32, alias: Id },
    /// The first frame of the session's windows is on glass: the launcher-hidden windows show.
    Reveal,
}

/// The window system's life: asked for by a backend, started by the main thread, stopped by the
/// host.
#[derive(Default)]
pub(super) struct Run {
    pub(super) wanted: bool,
    pub(super) ready: bool,
    pub(super) stop: bool,
    pub(super) keep_alive: usize,
}

/// What the main thread and the sessions' threads share: the one instance of the process.
pub struct Shared {
    /// events by session, written by the main thread, drained by `Hal::pump`
    pub(super) inbox: Mutex<HashMap<u32, EventQueue>>,
    /// events pushed so far (the number of the newest one), under the inbox lock
    pub(super) pushed: AtomicU64,
    /// per session: the number of the newest event its pump has taken, under the inbox lock
    pub(super) taken: Mutex<HashMap<u32, u64>>,
    pub(super) ops: Mutex<Vec<Op>>,
    /// a main-queue drain is pending
    pub(super) scheduled: AtomicBool,
    /// the CAMetalLayer of each window, by (session, window index) and generation
    pub(super) layers: Mutex<HashMap<(u32, u32), (u32, RcObjcId)>>,
    pub(super) wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    pub(super) run: Mutex<Run>,
    pub(super) run_cv: Condvar,
    /// the MTLDevice windows' layers draw with
    pub(super) device: AtomicUsize,
    /// the main screen's backing scale (f32 bits), 0 until the shell is up
    pub(super) dpi: AtomicU32,
    /// `open_flags` of the backend using the shell
    pub(super) flags: AtomicU32,
    /// set once the first presented frame revealed the launcher-hidden windows
    pub(super) revealed: AtomicBool,
    /// the host announced it runs the main loop
    pub(super) host: AtomicBool,
    pub(super) logged_present: AtomicBool,
    pub(super) screens: Mutex<Vec<crate::ScreenInfo>>,
}

impl Shared {
    fn new() -> Shared {
        Shared {
            inbox: Mutex::new(HashMap::new()),
            pushed: AtomicU64::new(0),
            taken: Mutex::new(HashMap::new()),
            ops: Mutex::new(Vec::new()),
            scheduled: AtomicBool::new(false),
            layers: Mutex::new(HashMap::new()),
            wake: Mutex::new(None),
            run: Mutex::new(Run::default()),
            run_cv: Condvar::new(),
            device: AtomicUsize::new(0),
            dpi: AtomicU32::new(0),
            flags: AtomicU32::new(0),
            revealed: AtomicBool::new(false),
            host: AtomicBool::new(false),
            logged_present: AtomicBool::new(false),
            screens: Mutex::new(Vec::new()),
        }
    }

    /// The process-wide shell.
    pub fn global() -> &'static Shared {
        static SHARED: OnceLock<Shared> = OnceLock::new();
        SHARED.get_or_init(Shared::new)
    }

    // ---- the backend side (any thread)

    /// A backend wants windows: the main loop starts. `wake` is called whenever the shell has
    /// events for the pumping thread. Waits (bounded) for the main thread to be up, so the
    /// first answer carries the screen's scale; false when the host never ran the main loop.
    pub fn start(&self, device: ObjcId, flags: u32, wake: Arc<dyn Fn() + Send + Sync>) -> bool {
        if !self.host.load(Ordering::Acquire) {
            return false;
        }
        self.device.store(device as usize, Ordering::Release);
        self.flags.store(flags, Ordering::Release);
        *lock(&self.wake) = Some(wake);
        let mut run = lock(&self.run);
        run.wanted = true;
        self.run_cv.notify_all();
        #[cfg(target_os = "macos")]
        super::appkit::wake_idle_main();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !run.ready && !run.stop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            run = self.run_cv.wait_timeout(run, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        run.ready
    }

    pub fn dpi(&self) -> f32 {
        let bits = self.dpi.load(Ordering::Acquire);
        if bits == 0 {
            1.0
        } else {
            f32::from_bits(bits)
        }
    }

    pub fn flags(&self) -> u32 {
        self.flags.load(Ordering::Acquire)
    }

    /// Hands a command to the main thread.
    pub fn command(&self, sid: u32, cmd: OsCmd) {
        let taken = lock(&self.taken).get(&sid).copied().unwrap_or(0);
        self.post(Op::Cmd(sid, cmd, taken));
    }

    /// The session's windows close (silently) and its events are dropped.
    pub fn session_closed(&self, sid: u32) {
        lock(&self.inbox).remove(&sid);
        lock(&self.taken).remove(&sid);
        lock(&self.layers).retain(|k, _| k.0 != sid);
        self.post(Op::CloseSession(sid));
    }

    pub fn lend(&self, host: Id, session: u32, alias: Id) {
        self.post(Op::Lend { host, session, alias });
    }

    /// The first frame of the app is on glass.
    pub fn first_present(&self) {
        if self.flags() & crate::types::open_flags::HIDDEN_UNTIL_PRESENT != 0 && !self.revealed.swap(true, Ordering::AcqRel) {
            self.post(Op::Reveal);
        }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn revealed(&self) -> bool {
        self.revealed.load(Ordering::Acquire)
    }

    /// A frame of a window reached the screen (any thread): the launcher-hidden windows show,
    /// and the live-resize test counts it.
    pub fn presented(&self) {
        if log_enabled() && !self.logged_present.swap(true, Ordering::AcqRel) {
            eprintln!("[hal-apple] the first frame reached the screen");
        }
        self.first_present();
        native::presented();
    }

    /// The window's layer (retained for the caller), if the main thread has made it.
    pub fn layer(&self, sid: u32, id: Id) -> Option<RcObjcId> {
        lock(&self.layers).get(&(sid, id.index)).filter(|(gen, _)| *gen == id.gen).map(|(_, l)| l.clone())
    }

    /// Moves what the main thread queued for the session into its event queue.
    pub fn drain_events(&self, sid: u32, world: &mut crate::session::World) {
        let mut inbox = lock(&self.inbox);
        let pushed = self.pushed.load(Ordering::Acquire);
        let mut taken = lock(&self.taken);
        if sid == crate::session::HOST {
            // The owner keeps pumping when a borrower is parked before its first
            // frame. Its visible run-state view must still receive native input.
            for (&recipient, q) in inbox.iter_mut() {
                q.drain_with(|event, payload| world.window_event(recipient, event, payload));
                taken.insert(recipient, pushed);
            }
        } else if let Some(q) = inbox.get_mut(&sid) {
            q.drain_with(|event, payload| world.window_event(sid, event, payload));
            taken.insert(sid, pushed);
        }
    }

    // ---- the main thread's side

    pub(super) fn post(&self, op: Op) {
        lock(&self.ops).push(op);
        if !self.scheduled.swap(true, Ordering::AcqRel) {
            native::schedule_drain();
        }
    }

    pub(super) fn take_ops(&self) -> Vec<Op> {
        self.scheduled.store(false, Ordering::Release);
        std::mem::take(&mut *lock(&self.ops))
    }

    /// An event for the session's pumping thread (from the main thread). Returns the event's
    /// number (see [`Op::Cmd`]).
    pub(super) fn push(&self, sid: u32, f: impl FnOnce(&mut EventQueue)) -> u64 {
        let number = {
            let mut inbox = lock(&self.inbox);
            let q = inbox.entry(sid).or_default();
            q.set_time(crate::clock_ns());
            f(q);
            self.pushed.fetch_add(1, Ordering::AcqRel) + 1
        };
        let wake = lock(&self.wake).clone();
        if let Some(wake) = wake {
            wake();
        }
        number
    }

    pub(super) fn register_layer(&self, sid: u32, id: Id, layer: RcObjcId) {
        lock(&self.layers).insert((sid, id.index), (id.gen, layer));
    }

    pub(super) fn unregister_layer(&self, sid: u32, id: Id) {
        let mut layers = lock(&self.layers);
        if layers.get(&(sid, id.index)).is_some_and(|(gen, _)| *gen == id.gen) {
            layers.remove(&(sid, id.index));
        }
    }

    pub(super) fn device(&self) -> ObjcId {
        self.device.load(Ordering::Acquire) as ObjcId
    }

    pub(super) fn publish_ready(&self, dpi: f32) {
        self.dpi.store(dpi.to_bits(), Ordering::Release);
        let mut run = lock(&self.run);
        run.ready = true;
        self.run_cv.notify_all();
    }
}
