//! makepad-hal: the interface right below Cx.
//!
//! Cx writes a [`CmdWriter`] (fire-and-forget commands, FramePackets inside) and submits it once
//! per loop turn; the HAL answers only through events ([`EventQueue`] -> [`EventReader`]). Ids are
//! chosen by Cx, the HAL keys its tables by (session, kind, index) and checks generations
//! ([`session`]). Four synchronous entries cross the boundary as `#[relax_api]` ops:
//! [`hal_open`], [`hal_submit`], [`hal_pump`], [`hal_wake`] (OS nested loops block the main thread
//! only, where the HAL runs; the sessions go on, so no call into the app is needed). Types are in
//! [`types`], the command and event tables in [`cmds`] and [`events`], the GPU ledger in
//! [`ledger`]. This crate ships the headless backend; the platform backends come with their phases.

pub mod backend;
#[cfg(target_os = "linux")]
pub mod dmabuf;
/// Shared textures outside any session on Windows (frame rings): a producer's copy or upload
/// into a slot a host shared, a slot read back.
#[cfg(windows)]
pub use backend::windows::d3d11::share_copy as shared_texture;
pub mod spirv;
pub mod hlsl;
pub mod glsl;
#[cfg(target_os = "linux")]
pub mod gl_interop;
pub mod automation;
pub mod buffers;
pub mod clock;
pub mod clipboard;
pub mod direct;
#[cfg(target_os = "linux")]
pub mod wide_desktop;
pub mod cmds;
pub mod codec;
pub mod events;
pub mod font;
pub mod ledger;
pub mod gpu_limits;
pub mod gpu_pressure;
#[cfg(not(target_arch = "wasm32"))]
pub mod inspect;
pub mod media;
pub mod profile;
pub mod session;
pub mod types;

#[cfg(target_vendor = "apple")]
pub use backend::apple::media::{can_play_type, video_h264_probe};
#[cfg(target_os = "linux")]
pub use backend::linux::media::can_play_type;
#[cfg(target_os = "android")]
pub use backend::android::media::can_play_type;
#[cfg(windows)]
pub use backend::windows::media::can_play_type;
pub use buffers::{CmdWriter, EventQueue, EventReader, EventStore};
pub use cmds::{cmd_tag, HalCmd};
pub use events::{event_tag, HalEvent};
pub use session::World;
pub use types::*;

use backend::{headless::Headless, Backend};
use makepad_relax_api::relax_api;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::Duration;

pub use session::{CloseAs, DomainLaunch, ExecRecord};

/// What the relax host provides so a privileged session can start, stop and relaunch apps
/// (`DomainStart` / `DomainStop` / `DomainRelaunch`): host-native code, registered with
/// [`Hal::set_domain_host`]. Called outside the HAL's lock, on the thread that submitted the
/// command, so an implementation may call back into the HAL. A started app opens its session with
/// `OpenReq { parent: launch.parent, app_key: launch.app_key, .. }`; a relaunch first stops the
/// old app (it was asked to quit with `stop(domain, QUIT)`, which returns once its run has ended,
/// and its session is freed before `relaunch`) and then starts the new one the same way. `stop`
/// with `KILL` is called after the session is freed (the app hears one quit request on its next pump)
/// and likewise returns once the run has ended. Progress and crashes are reported with [`Hal::report_domain`].
pub trait DomainHost: Send + Sync {
    fn start(&self, launch: DomainLaunch) -> Result<(), String>;
    /// `domain_stop::QUIT` (the app was asked to quit) or `KILL` (its session is already freed).
    fn stop(&self, domain: u32, mode: u32);
    fn relaunch(&self, launch: DomainLaunch) -> Result<(), String>;
}

/// What the host grants a session at creation; never claimable from a command or from JIT code.
#[derive(Clone, Copy, Debug, Default)]
pub struct Caps {
    /// may use the domain control commands
    pub privileged: bool,
}

struct State {
    world: World,
    /// THE backend: created by the first open that names one, kept for the life of the Hal so
    /// the device and the host-lent windows outlive app sessions
    backend: Option<(u32, Box<dyn Backend>)>,
    /// how many times a backend instance was created (stays 1 across relaunches)
    backend_inits: u32,
}

/// Who has something to pick up: one blocking queue per session, plus the all-sessions wake.
#[derive(Default)]
struct Signal {
    /// bumped by anything an `await_*` waits for
    epoch: u64,
    /// bumped when every session's pump must look again (global wake, clock change)
    all: u64,
    /// per session: its queue got an event or it was woken
    per: HashMap<u32, u64>,
    /// `hal_wake` count, and `wake_session` count per session
    wake_all: u64,
    wake_one: HashMap<u32, u64>,
    /// the wake counts each session's pump has turned into a `Wake` event
    taken: HashMap<u32, (u64, u64)>,
}

impl Signal {
    fn tick(&self, sid: u32) -> u64 {
        self.all + self.per.get(&sid).copied().unwrap_or(0)
    }

    /// Whether a wake arrived for `sid` since its pump last asked.
    fn take_wake(&mut self, sid: u32) -> bool {
        let now = (self.wake_all, self.wake_one.get(&sid).copied().unwrap_or(0));
        let seen = self.taken.entry(sid).or_insert((0, 0));
        let woken = *seen != now;
        *seen = now;
        woken
    }

    /// A new session starts with the wakes before it already seen.
    fn start(&mut self, sid: u32) {
        let now = (self.wake_all, self.wake_one.get(&sid).copied().unwrap_or(0));
        self.taken.insert(sid, now);
    }

    fn forget(&mut self, sid: u32) {
        self.per.remove(&sid);
        self.wake_one.remove(&sid);
        self.taken.remove(&sid);
    }
}

/// The wake signal and what waits on it; backends hold a handle to wake the pump from their
/// own threads.
struct Sig {
    signal: Mutex<Signal>,
    cv: Condvar,
}

impl Sig {
    /// Something happened for these sessions' queues (and for the host's `await_*`).
    fn notify(&self, sessions: &[u32]) {
        let mut s = lock(&self.signal);
        s.epoch += 1;
        for id in sessions {
            *s.per.entry(*id).or_default() += 1;
        }
        drop(s);
        self.cv.notify_all();
    }

    /// Every session's pump looks again; `wake` also delivers a `Wake` event to each.
    fn notify_all(&self, wake: bool) {
        let mut s = lock(&self.signal);
        s.epoch += 1;
        s.all += 1;
        if wake {
            s.wake_all += 1;
        }
        drop(s);
        self.cv.notify_all();
    }
}

/// One HAL instance: the session tables, the backends and the wake signal. The four ops use the
/// process-wide one ([`Hal::global`]); tests and the automation runner may own their own. Every
/// call is keyed by its session and may come from any thread.
pub struct Hal {
    /// this instance, for the work it hands to a thread of its own (domain stops and relaunches)
    me: Weak<Hal>,
    state: Mutex<State>,
    sig: Arc<Sig>,
    /// request ids of host queries
    next_req: AtomicU32,
    /// the automation runs by session (started by `hal_open` when `MAKEPAD_HAL_SCRIPT` is set)
    runners: Mutex<std::collections::HashMap<u32, automation::Running>>,
    domain_host: Mutex<Option<Arc<dyn DomainHost>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Hal {
    pub fn new() -> Arc<Hal> {
        Arc::new_cyclic(|me| Hal {
            me: me.clone(),
            state: Mutex::new(State { world: World::new(), backend: None, backend_inits: 0 }),
            sig: Arc::new(Sig { signal: Mutex::new(Signal::default()), cv: Condvar::new() }),
            next_req: AtomicU32::new(1),
            runners: Mutex::new(Default::default()),
            domain_host: Mutex::new(None),
        })
    }

    pub fn global() -> &'static Hal {
        static HAL: OnceLock<Arc<Hal>> = OnceLock::new();
        HAL.get_or_init(Hal::new)
    }

    // ---- the four ops

    /// Opens an unprivileged session on the requested backend; `session == 0` in the answer =
    /// refused. A non-zero `app_key` of an already open session closes that session first. A
    /// request naming a `parent` is accepted only as the opening of a domain that parent
    /// started with `DomainStart` (its `app_key` is the proof).
    pub fn open(&self, req: &OpenReq) -> HalInfo {
        self.open_as(req, Caps::default(), false, false)
    }

    /// The host's open: like [`Hal::open`] with capabilities, and `req.parent` taken as given.
    pub fn open_domain(&self, req: &OpenReq, caps: Caps) -> HalInfo {
        self.open_as(req, caps, true, false)
    }

    /// Opens the host's permanent session on the same backend its apps will use.
    /// Session 0 is never lent or closed; only its windows are lent.
    pub fn open_host(&self, req: &OpenReq) -> HalInfo {
        self.open_as(req, Caps { privileged: true }, true, true)
    }

    fn open_as(&self, req: &OpenReq, caps: Caps, trusted: bool, host_session: bool) -> HalInfo {
        let refused = HalInfo { backend: backend_id::NONE, ..Default::default() };
        let mut guard = lock(&self.state);
        let st = &mut *guard;
        let sig = self.sig.clone();
        #[allow(unused_variables)]
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || sig.notify_all(true));
        // the window system's events are the news themselves: the pumps wake without a `Wake`
        let sig = self.sig.clone();
        #[allow(unused_variables)]
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || sig.notify_all(false));
        let make = |id: u32| -> Option<Box<dyn Backend>> {
            match id {
                #[cfg(target_os = "linux")]
                backend_id::AUTO => match backend::linux::open(req.flags, wake.clone(), notify.clone()) {
                    Ok(b) => Some(b),
                    Err(e) => { eprintln!("[hal] {e}"); None }
                },
                #[cfg(target_os = "linux")]
                backend_id::GL => match backend::gl::GlBackend::new(req.flags, wake.clone(), notify.clone()) {
                    Ok(gl) => Some(Box::new(gl)),
                    Err(e) => {
                        eprintln!("[hal-gl] {e}");
                        if req.flags & open_flags::OFFSCREEN_WINDOWS == 0 { backend::linux::stop_main_loop(); }
                        None
                    }
                },
                backend_id::HEADLESS => Some(Box::new(Headless)),
                #[cfg(target_vendor = "apple")]
                backend_id::METAL => backend::apple::Metal::new(req.flags, wake.clone(), notify.clone()).map(|m| Box::new(m) as Box<dyn Backend>),
                #[cfg(windows)]
                backend_id::D3D11 => match backend::windows::D3d11::new(req.flags, wake.clone(), notify.clone()) {
                    Ok(d) => Some(Box::new(d)),
                    Err(e) => { eprintln!("[hal-d3d11] {e}"); None }
                },
                #[cfg(target_os = "linux")]
                backend_id::VULKAN => match backend::vulkan::Vulkan::new(req.flags, wake.clone(), notify.clone()) {
                    Ok(v) => Some(Box::new(v)),
                    Err(e) => { eprintln!("[hal-vulkan] {e}"); None }
                },
                #[cfg(target_os = "android")]
                backend_id::VULKAN => match backend::android::AndroidBackend::new(req.flags, wake.clone()) {
                    Ok(v) => Some(Box::new(v)),
                    Err(e) => { eprintln!("[hal-android] {e}"); None }
                },
                _ => None,
            }
        };
        match &st.backend {
            // AUTO takes the backend the first open selected
            Some((id, _)) if *id != req.backend && req.backend != backend_id::AUTO => return refused,
            Some(_) => {}
            None => match make(req.backend) {
                Some(b) => {
                    st.backend = Some((b.info().backend, b));
                    st.backend_inits += 1;
                }
                None => return refused,
            },
        }
        // the font service's workers wake the pumps when they have glyphs to deliver
        if st.world.font_notify.is_none() {
            st.world.font_notify = Some(notify.clone());
        }
        let mut info = st.backend.as_ref().expect("backend").1.info();
        if req.parent != 0 || req.flags & open_flags::OFFSCREEN_WINDOWS != 0 {
            info.flags &= !info_flags::DIRECT_DISPLAY;
        }
        let parent_dpi = st.world.domain_dpi(req.parent);
        if req.parent != 0 && parent_dpi.is_none() {
            return refused;
        }
        // A child's pixel density belongs to its compositor, not its own DPI hint.
        info.dpi = if req.parent != 0 { parent_dpi.expect("parent checked") }
            else if req.dpi_hint > 0.0 { req.dpi_hint } else { parent_dpi.unwrap_or(info.dpi) };
        let id = if host_session {
            session::HOST
        } else if let Some(domain) = st.world.domain_waiting(req.app_key, req.parent) {
            domain
        } else if req.parent != 0 {
            if !trusted { return refused; }
            st.world.open_session(req.app_key, req.parent, false, info.dpi)
        } else {
            // a relaunch of the same app: the old session goes first
            if let Some(old) = st.world.session_with_key(req.app_key) {
                st.world.schedule_close(old, CloseAs::Quit);
                for (closed, _) in Self::settle(st) {
                    lock(&self.sig.signal).forget(closed);
                }
            }
            st.world.open_session(req.app_key, 0, false, info.dpi)
        };
        if let Some(s) = st.world.session_mut(id) {
            s.privileged = caps.privileged;
        }
        let backend = &mut st.backend.as_mut().expect("backend").1;
        backend.session_opened(&mut st.world, id);
        lock(&self.sig.signal).start(id);
        if st.world.domain(id).is_some() || st.world.session(id).is_some_and(|s| s.parent != 0) {
            st.world.attach_domain(id, info.dpi);
        }
        info.session = id;
        info.flags &= !(info_flags::PRIVILEGED | info_flags::EMBEDDED);
        if caps.privileged { info.flags |= info_flags::PRIVILEGED; }
        if req.parent != 0 { info.flags |= info_flags::EMBEDDED; }
        if let Some(s) = st.world.session_mut(id) {
            s.dpi = info.dpi;
            s.events.startup();
        }
        self.finish(guard, &[id]);
        info
    }

    /// Runs the queued closes, the backend first so it can free its side; returns (session,
    /// made again by a relaunch).
    fn settle(st: &mut State) -> Vec<(u32, bool)> {
        let mut closed = Vec::new();
        while let Some((id, how)) = st.world.pop_close() {
            if let Some((_, b)) = st.backend.as_mut() {
                b.session_closed(&mut st.world, id);
            }
            let relaunched = matches!(how, CloseAs::Relaunch);
            st.world.finish_close(id, how);
            closed.push((id, relaunched));
        }
        closed
    }

    /// Ends an operation on the world: queued closes run, the sessions that got events are
    /// woken, the host is asked for what the commands requested, finished automation runs report.
    /// Nothing is held while the host is called.
    fn finish(&self, mut guard: MutexGuard<'_, State>, notify: &[u32]) {
        let st = &mut *guard;
        let closed = Self::settle(st);
        let mut targets = st.world.take_notified();
        targets.extend_from_slice(notify);
        let calls = st.world.take_host_calls();
        drop(guard);
        self.sig.notify(&targets);
        for (id, relaunched) in closed {
            if !relaunched {
                lock(&self.sig.signal).forget(id);
            }
            self.finish_runner(id);
        }
        // Stopping or relaunching an app waits for its threads and frees its heap: never on the caller's
        // thread (a window manager's render thread would stall for the whole teardown). Starts already
        // run on the host's loader threads.
        let off_thread = std::env::var_os("MAKEPAD_SYNC_STOP").is_none()
            && calls.iter().any(|c| matches!(c, session::HostCall::Stop { .. } | session::HostCall::Relaunch(_)));
        // the thread runs them on this instance (not the process-wide one: a test's or a runner's
        // own Hal has its own domain host)
        if let Some(hal) = self.me.upgrade().filter(|_| off_thread) {
            let _ = std::thread::Builder::new().name("hal-host-calls".into()).spawn(move || {
                let t0 = std::time::Instant::now();
                hal.run_host_calls(calls);
                if std::env::var_os("MAKEPAD_FRAME_LOG").is_some() { eprintln!("[hal] domain stop/relaunch host calls took {:.0} ms (off the caller's thread)", t0.elapsed().as_secs_f64() * 1e3); }
            });
            return;
        }
        let t0 = std::time::Instant::now();
        self.run_host_calls(calls);
        if std::env::var_os("MAKEPAD_FRAME_LOG").is_some() { eprintln!("[hal] host calls took {:.0} ms on the caller's thread", t0.elapsed().as_secs_f64() * 1e3); }
    }

    fn run_host_calls(&self, calls: Vec<session::HostCall>) {
        for call in calls {
            // a relaunch frees what is left of the old session (the app has stopped by now) and
            // makes it again, empty, under the same id
            if let session::HostCall::Relaunch(l) = &call {
                let mut guard = lock(&self.state);
                guard.world.relaunch_domain(l.domain);
                self.finish(guard, &[]);
            }
            let host = lock(&self.domain_host).clone();
            let (domain, result) = match (call, host) {
                (session::HostCall::Stop { domain, mode }, Some(h)) => {
                    h.stop(domain, mode);
                    continue;
                }
                (session::HostCall::Stop { .. }, None) => continue,
                (session::HostCall::Start(l), Some(h)) => (l.domain, h.start(l)),
                (session::HostCall::Relaunch(l), Some(h)) => (l.domain, h.relaunch(l)),
                (session::HostCall::Start(l) | session::HostCall::Relaunch(l), None) => (l.domain, Err("no domain host is registered".to_string())),
            };
            if let Err(msg) = result {
                self.report_domain(domain, domain_state::CRASHED, 0, 0.0, &msg);
            }
        }
    }

    /// Registers the host that starts, stops and relaunches domains' apps.
    pub fn set_domain_host(&self, host: Arc<dyn DomainHost>) {
        *lock(&self.domain_host) = Some(host);
    }

    /// The host reports a domain's progress (`domain_state::COMPILING` with a phase and percent),
    /// its crash or its exit; the parent hears `DomainState`. A crash or exit frees the session
    /// (its windows leave the parent, children go with it). False for an unknown domain.
    pub fn report_domain(&self, domain: u32, state: u32, phase: u32, pct: f32, msg: &str) -> bool {
        let mut guard = lock(&self.state);
        let st = &mut *guard;
        if !st.world.report_domain(domain, state, phase, pct, msg) {
            return false;
        }
        match state {
            domain_state::CRASHED => st.world.schedule_close(domain, CloseAs::Crash(msg.to_string())),
            domain_state::EXITED => st.world.schedule_close(domain, CloseAs::Exit(msg.to_string())),
            _ => {}
        }
        self.finish(guard, &[]);
        true
    }

    /// How many backend instances this Hal has created (1 once opened, however many relaunches).
    pub fn backend_inits(&self) -> u32 {
        lock(&self.state).backend_inits
    }

    /// Executes a batch. The slices in `buf` are read during this call only.
    pub fn submit(&self, buf: &CmdBuf) {
        let sid = buf.session;
        let mut guard = lock(&self.state);
        let st = &mut *guard;
        // SAFETY: the slices are borrowed for this call (hal_submit contract)
        let Some(view) = (unsafe { buf.view() }) else {
            if let Some(s) = st.world.session_mut(sid) {
                s.stats.bad_bufs += 1;
            }
            return;
        };
        match st.world.session_mut(sid) {
            None => return,
            // closing, or a domain whose app has not (re)opened it: nothing of this caller counts
            Some(s) if s.closing || !s.attached => {
                s.stats.denied += 1;
                return;
            }
            Some(_) => {}
        }
        let Some((_, b)) = st.backend.as_mut() else { return };
        b.submit(&mut st.world, sid, &view);
        // `Quit` ends the session: the same free path as `close`
        if st.world.session(sid).is_some_and(|s| s.quit) {
            st.world.schedule_close(sid, CloseAs::Quit);
        }
        self.finish(guard, &[sid]);
    }

    /// An automation run of the closed session ends here: its summary (what the HAL holds after
    /// the close) goes out, and a failed run ends the process with status 1.
    fn finish_runner(&self, session: u32) {
        let run = lock(&self.runners).remove(&session);
        if let Some(run) = run {
            if !run.finish(self) {
                std::process::exit(1);
            }
        }
    }

    /// Delivers events for `out.session`, waiting up to `timeout_ns` when there are none (woken
    /// early by [`Hal::wake`], an injected event, or a due timer). Returns the event count.
    pub fn pump(&self, timeout_ns: u64, out: &mut EventBuf) -> u32 {
        let sid = out.session;
        let t0 = std::time::Instant::now();
        let deadline = t0.checked_add(Duration::from_nanos(timeout_ns)).unwrap_or_else(|| t0 + Duration::from_secs(86400 * 365));
        loop {
            let seen = lock(&self.sig.signal).tick(sid);
            let wait = {
                let mut guard = lock(&self.state);
                let st = &mut *guard;
                if let Some((_, b)) = st.backend.as_mut() {
                    b.step(&mut st.world, sid);
                    // glyphs the font workers finished go to the atlas and to the sessions
                    if let Some(gpu) = b.host_gpu() {
                        font::service(gpu, &mut st.world);
                    }
                }
                // A completion or a lent window's geometry can notify another session while
                // this pump steps the backend. Wake it now, even when no later submit follows.
                self.sig.notify(&st.world.take_notified());
                let woken = lock(&self.sig.signal).take_wake(sid);
                let now = st.world.now();
                // embedded windows' frames (no display link of their own) at display rate
                let embedded_due = st.world.deliver_embedded_frames(sid, now);
                let due = st.backend.as_ref().and_then(|(_, b)| b.next_due(&st.world, sid));
                let due = match (due, embedded_due) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) };
                if st.world.session(sid).is_none() {
                    // the session was ended under its app (killed, crashed, its parent closed):
                    // the app hears one quit request and ends its run
                    if st.world.take_ended(sid) {
                        let mut q = EventQueue::new();
                        q.set_time(now);
                        q.lifecycle(lifecycle::QUIT_REQUESTED);
                        return q.drain_into(out);
                    }
                    out.arena_len = 0;
                    return 0;
                }
                let Some(s) = st.world.session_mut(sid) else { return 0 };
                if woken {
                    s.events.set_time(now);
                    s.events.wake();
                }
                if !s.events.is_empty() {
                    return s.events.drain_into(out);
                }
                // nothing yet: sleep until the deadline or the next timer (manual clocks do not
                // advance by sleeping, only by automation)
                let real = std::time::Instant::now();
                let mut wait = deadline.saturating_duration_since(real);
                if let (Some(d), None) = (due, st.world.clock.manual) {
                    wait = wait.min(Duration::from_nanos(d.saturating_sub(now)));
                }
                wait
            };
            if wait.is_zero() {
                out.arena_len = 0;
                return 0;
            }
            let sig = lock(&self.sig.signal);
            if sig.tick(sid) == seen {
                drop(self.sig.cv.wait_timeout(sig, wait).unwrap_or_else(|e| e.into_inner()));
            }
        }
    }

    /// Wakes every session's blocked [`Hal::pump`] (any thread, never waits for the pumping
    /// thread); each gets a `Wake` event.
    pub fn wake(&self) {
        self.sig.notify_all(true);
    }

    /// Tests and automation: the backend handles its device as lost on its next step and replaces
    /// it (sessions hear `DeviceLost`, then `DeviceReset`). No effect on a backend without a
    /// device reset (headless; Metal and Vulkan implement the two hooks after the HAL push).
    pub fn simulate_device_loss(&self) {
        {
            let mut guard = lock(&self.state);
            if let Some((_, b)) = guard.backend.as_mut() {
                b.simulate_device_loss();
            }
        }
        self.wake();
    }

    /// Wakes one session's pump (a `Wake` event for it alone).
    pub fn wake_session(&self, session: u32) {
        let mut s = lock(&self.sig.signal);
        s.epoch += 1;
        *s.wake_one.entry(session).or_default() += 1;
        *s.per.entry(session).or_default() += 1;
        drop(s);
        self.sig.cv.notify_all();
    }

    /// The pixels of a window of the session as the GPU holds them now (the newest finished
    /// frame), for the host side: automation snapshots. `None` without a raster or a frame.
    pub fn window_pixels(&self, session: u32, window: Option<Id>) -> Option<backend::Pixels> {
        let mut guard = lock(&self.state);
        let st = &mut *guard;
        let (_, b) = st.backend.as_mut()?;
        b.window_pixels(&mut st.world, session, window)
    }

    // ---- host / automation API (plain Rust, not relax ops)

    /// Puts an event into the session's queue, as if the OS had delivered it.
    pub fn inject(&self, session: u32, e: RawEvent) {
        if let Some(s) = lock(&self.state).world.session_mut(session) {
            s.events.push(e);
        }
        self.sig.notify(&[session]);
    }

    /// Sets a manual clock: HAL time stops following the real one until [`Hal::clock_real`].
    pub fn clock_set(&self, ns: u64) {
        lock(&self.state).world.clock.manual = Some(ns);
        self.sig.notify_all(false);
    }

    /// Advances the manual clock (starting from now when it was real).
    pub fn clock_advance(&self, ns: u64) {
        let mut st = lock(&self.state);
        let now = st.world.now();
        st.world.clock.manual = Some(now + ns);
        drop(st);
        self.sig.notify_all(false);
    }

    pub fn clock_real(&self) {
        lock(&self.state).world.clock.manual = None;
        self.sig.notify_all(false);
    }

    pub fn now(&self) -> u64 {
        lock(&self.state).world.now()
    }

    /// Pushes events built with the queue's own constructors (those with payloads need its arena).
    pub fn with_events(&self, session: u32, f: impl FnOnce(&mut EventQueue)) {
        if let Some(s) = lock(&self.state).world.session_mut(session) {
            f(&mut s.events);
        }
        self.sig.notify(&[session]);
    }

    /// Blocks until `done` holds for the world (checked again at every frame, injected event and
    /// clock change); false on timeout.
    pub fn wait_until(&self, timeout: Duration, mut done: impl FnMut(&World) -> bool) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let seen = lock(&self.sig.signal).epoch;
            if done(&lock(&self.state).world) {
                return true;
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            let sig = lock(&self.sig.signal);
            if sig.epoch == seen {
                drop(self.sig.cv.wait_timeout(sig, left).unwrap_or_else(|e| e.into_inner()));
            }
        }
    }

    /// Blocks until the session has finished a frame with id >= `frame`; false on timeout.
    pub fn await_frame_done(&self, session: u32, frame: u64, timeout: Duration) -> bool {
        self.wait_until(timeout, |w| w.session(session).is_some_and(|s| s.last_frame_done >= frame))
    }

    /// Blocks until the session has answered `count` more frames than it had at the call.
    pub fn await_frames(&self, session: u32, count: u64, timeout: Duration) -> bool {
        let start = self.with_world(|w| w.session(session).map_or(0, |s| s.stats.frames));
        self.wait_until(timeout, |w| w.session(session).is_some_and(|s| s.stats.frames >= start + count))
    }

    /// Asks the app: a `Query` event carrying `request`, answered by a `QueryReply` command.
    /// Returns the reply bytes, or `None` when the app did not answer within `timeout`.
    pub fn query(&self, session: u32, request: &[u8], timeout: Duration) -> Option<Vec<u8>> {
        let req = self.next_req.fetch_add(1, Ordering::Relaxed);
        self.with_events(session, |q| q.query(req, request));
        self.wait_until(timeout, |w| w.session(session).is_none_or(|s| s.replies.iter().any(|r| r.0 == req)));
        let mut st = lock(&self.state);
        let s = st.world.session_mut(session)?;
        let i = s.replies.iter().position(|r| r.0 == req)?;
        Some(s.replies.swap_remove(i).1)
    }

    /// Closes a session (frees what it owns, returns lent objects to the host, closes its
    /// children first).
    pub fn close(&self, session: u32) -> bool {
        let mut guard = lock(&self.state);
        let st = &mut *guard;
        if session == session::HOST || st.world.session(session).is_none_or(|s| s.closing) {
            return false;
        }
        st.world.schedule_close(session, CloseAs::Exit("closed".into()));
        self.finish(guard, &[]);
        true
    }

    /// Runs `f` on the world (tests, the runner's queries).
    pub fn with_world<R>(&self, f: impl FnOnce(&mut World) -> R) -> R {
        f(&mut lock(&self.state).world)
    }

    /// What the font service did so far (zeros before the first font command).
    pub fn font_stats(&self) -> font::FontStats {
        lock(&self.state).world.fonts.as_ref().map(|f| f.stats()).unwrap_or_default()
    }

    /// What one session used of the font service.
    pub fn font_session_stats(&self, session: u32) -> font::SessionFontStats {
        lock(&self.state).world.fonts.as_ref().map(|f| f.session_stats(session)).unwrap_or_default()
    }

    /// The ledger report of one session; groups idle after `idle_frames` frames.
    pub fn report(&self, session: u32, idle_frames: u64) -> Option<ledger::Report> {
        let st = lock(&self.state);
        let s = st.world.session(session)?;
        Some(s.ledger.report(session, &s.strings, s.frame, idle_frames))
    }

    /// The reports of every session, merged.
    pub fn report_all(&self, idle_frames: u64) -> ledger::Report {
        let st = lock(&self.state);
        let mut all = ledger::Report::default();
        for id in st.world.session_ids() {
            if let Some(s) = st.world.session(id) {
                all.merge(s.ledger.report(id, &s.strings, s.frame, idle_frames));
            }
        }
        all
    }
}

// ------------------------------------------------------------------ the main thread

/// Schedules native host work on the process main thread without blocking the caller.
///
/// macOS integrations (for example an embedded browser) use this for initialization, commands
/// and their message pump. Call `expect_main_loop` before scheduling startup work. The HAL
/// starts AppKit for queued work even before a session opens, and serves it in common modes
/// during live resize and modal loops. Closures run once, in submission order, outside the HAL
/// state lock; they must not wait on a session. Results belong in a non-blocking mailbox.
/// Returns false without running the closure if this host has no main loop or it has stopped.
#[cfg(target_vendor = "apple")]
pub fn dispatch_main<F: FnOnce() + Send + 'static>(f: F) -> bool {
    // Generic over the caller's closure, so it is compiled where the caller is (relax compiles
    // it for JIT code): the closure is boxed here and crosses to the native op as a pointer and
    // two thunks, whichever side compiled it.
    let data = Box::into_raw(Box::new(f)) as usize;
    backend::apple::shell::dispatch_main_raw(data, main_run::<F> as extern "C" fn(usize) as usize, main_discard::<F> as extern "C" fn(usize) as usize)
}

#[cfg(target_vendor = "apple")]
extern "C" fn main_run<F: FnOnce()>(data: usize) {
    let f = unsafe { Box::from_raw(data as *mut F) };
    f()
}

#[cfg(target_vendor = "apple")]
extern "C" fn main_discard<F>(data: usize) {
    drop(unsafe { Box::from_raw(data as *mut F) })
}

/// Keeps the main loop serving native callbacks until asynchronous teardown has completed.
/// Acquire before queuing work, then drop on its final completion. Returns `None` after stop.
#[cfg(target_vendor = "apple")]
pub use backend::apple::shell::{keep_main_loop_alive, MainLoopGuard};

/// Gives the calling thread, which must be the process's main thread, to the HAL: it becomes the
/// window system's thread (on macOS `NSApp`'s event loop) and does not return until
/// [`stop_main_loop`]. Every session's Cx runs on its own thread. A host API, not a relax op.
/// Until a backend asks for windows (`hal_open` without `OFFSCREEN_WINDOWS`) the thread only
/// waits.
pub fn run_main_loop() {
    main_loop::run()
}

/// A display: its extent and what is left of it after the menu bar and Dock, as x, y, w, h in the
/// window system's global points (macOS: bottom-left origin, the space window frames are in).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenInfo {
    pub bounds: [f64; 4],
    pub work_area: [f64; 4],
    pub is_primary: bool,
}

/// The displays attached right now, as the window system last saw them: readable from any
/// thread. Empty where there is no window system, or before it is up.
pub fn screens() -> Vec<ScreenInfo> {
    main_loop::screens()
}

/// Whether the system is in dark mode, as the window system last saw it: readable from any
/// thread. `None` where there is no window system, or before it is up.
#[relax_api]
pub fn host_dark() -> Option<bool> {
    main_loop::host_dark()
}

/// Announces that the host will run [`run_main_loop`] on its main thread (call it before the
/// threads that open sessions start): a backend that wants windows without it fails at once
/// instead of waiting for a main thread that never comes.
pub fn expect_main_loop() {
    main_loop::expect()
}

/// Ends [`run_main_loop`] (any thread), as the app does when it is done.
pub fn stop_main_loop() {
    main_loop::stop()
}

#[cfg(target_vendor = "apple")]
mod main_loop {
    pub use super::backend::apple::shell::{expect_main_loop as expect, screens};
    #[cfg(target_os = "macos")]
    pub use super::backend::apple::appkit::{host_dark, run_main_loop as run, stop_main_loop as stop};
    #[cfg(any(target_os = "ios", target_os = "tvos"))]
    pub use super::backend::apple::uikit::{host_dark, run_main_loop as run, stop_main_loop as stop};
}

/// The Linux shell owns the display connection and all native window objects.
#[cfg(target_os = "linux")]
mod main_loop {
    pub use super::backend::linux::{expect_main_loop as expect, run_main_loop as run, stop_main_loop as stop};
    pub fn host_dark() -> Option<bool> { None }
    pub fn screens() -> Vec<super::ScreenInfo> { super::backend::linux::screens() }
}

/// The Win32 shell runs the message loop on the main thread and owns every HWND.
#[cfg(windows)]
mod main_loop {
    pub use super::backend::windows::win32::{expect_main_loop as expect, host_dark, run_main_loop as run, screens, stop_main_loop as stop};
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux", windows)))]
mod main_loop {
    use std::sync::{Condvar, Mutex};

    static STOP: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

    pub fn expect() {}

    pub fn host_dark() -> Option<bool> {
        None
    }

    pub fn screens() -> Vec<super::ScreenInfo> {
        Vec::new()
    }

    pub fn run() {
        let mut stopped = STOP.0.lock().unwrap_or_else(|e| e.into_inner());
        while !*stopped {
            stopped = STOP.1.wait(stopped).unwrap_or_else(|e| e.into_inner());
        }
    }

    pub fn stop() {
        *STOP.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
        STOP.1.notify_all();
    }
}

// ------------------------------------------------------------------ the synchronous ops

thread_local! {
    static CAPS: std::cell::Cell<Caps> = const { std::cell::Cell::new(Caps { privileged: false }) };
    static LAUNCH: std::cell::Cell<Option<(u32, u64)>> = const { std::cell::Cell::new(None) };
}

/// Runs `f` (the whole run of the window manager app, on this thread) with the capabilities the
/// host grants it: its `hal_open` opens a session that has them. Host code only; the op itself
/// takes none from its caller.
pub fn with_caps<R>(caps: Caps, f: impl FnOnce() -> R) -> R {
    let before = CAPS.with(|c| c.replace(caps));
    let out = f();
    CAPS.with(|c| c.set(before));
    out
}

/// Runs `f` (an app's whole run, on this thread) as the domain `launch` names: its `hal_open`
/// opens the session the parent asked for with `DomainStart`, whatever the app puts in its
/// request. The host calls this around the app it starts in [`DomainHost::start`] / `relaunch`.
pub fn with_launch<R>(launch: &DomainLaunch, f: impl FnOnce() -> R) -> R {
    let before = LAUNCH.with(|l| l.replace(Some((launch.parent, launch.app_key))));
    let out = f();
    LAUNCH.with(|l| l.set(before));
    out
}

/// Opens a session: its id and the backend's capabilities (shader dialect, formats, MSAA, dpi,
/// limits), which select the shader codegen before the first CmdBuf. Once per session.
#[relax_api]
pub fn hal_open(req: &OpenReq) -> HalInfo {
    let hal = Hal::global();
    // a domain's app opens with the parent and key its host launched it with (`with_launch`),
    // so the app itself never names a parent
    let mut req = *req;
    if req.parent == 0 {
        if let Some((parent, app_key)) = LAUNCH.with(|l| l.get()) {
            req.parent = parent;
            req.app_key = app_key;
        }
    }
    let req = &req;
    // `MAKEPAD_HAL_SCRIPT=<file>`: the HAL runs the automation script next to the app (native
    // and under relax alike); a script that does not parse ends the process with status 2
    let script = std::env::var_os("MAKEPAD_HAL_SCRIPT").map(|path| match automation::Script::load(path.as_ref()) {
        Ok(script) => script,
        Err(error) => {
            println!("{{\"summary\":true,\"ok\":false,\"error\":{}}}", automation::json(&error));
            std::process::exit(2);
        }
    });
    let info = hal.open_as(req, CAPS.with(|c| c.get()), false, false);
    // (the script drives the app the process runs; the domains it starts are driven through it)
    if let (Some(script), true) = (script, info.session != 0 && req.parent == 0) {
        let mut cfg = automation::Config::new(info.session, info.dpi);
        cfg.snapshot_dir = std::env::var_os("MAKEPAD_HAL_SNAPSHOTS").map(Into::into);
        lock(&hal.runners).insert(info.session, automation::Running::start(hal, script, cfg));
    }
    info
}

/// The transport: one batch of commands per loop turn. Nothing is returned; borrowed slices in
/// `buf` are valid for this call only and the HAL copies what it keeps before returning.
#[relax_api]
pub fn hal_submit(buf: &CmdBuf) {
    Hal::global().submit(buf)
}

/// The transport back: delivers events into `out`, blocking up to `timeout_ns` when there are
/// none. Returns the number of events; `out.arena_len` is the arena bytes they use. Called by the
/// session's own thread (the HAL has the main thread).
#[relax_api]
pub fn hal_pump(timeout_ns: u64, out: &mut EventBuf) -> u32 {
    Hal::global().pump(timeout_ns, out)
}

/// Wakes a blocked `hal_pump`. Callable from any thread; never waits for the main thread.
#[relax_api]
pub fn hal_wake() {
    Hal::global().wake()
}

/// The directory of the app bundle's resources (Foundation, any thread).
#[cfg(target_vendor = "apple")]
pub fn bundle_resource_dir() -> Option<String> {
    use makepad_apple_sys::*;
    unsafe {
        let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
        let bundle: ObjcId = msg_send![class!(NSBundle), mainBundle];
        let root: ObjcId = msg_send![bundle, resourcePath];
        let path = (root != nil).then(|| nsstring_to_string(root));
        let () = msg_send![pool, release];
        path
    }
}

/// The app's own cache directory (`Library/Caches` of its container), writable, kept across
/// launches but not backed up; the OS may empty it when space runs low (Foundation, any thread).
#[cfg(target_vendor = "apple")]
pub fn app_cache_dir() -> Option<String> {
    /// NSCachesDirectory, NSUserDomainMask
    const CACHES: u64 = 13;
    const USER_DOMAIN: u64 = 1;
    use makepad_apple_sys::*;
    unsafe {
        let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
        let manager: ObjcId = msg_send![class!(NSFileManager), defaultManager];
        let urls: ObjcId = msg_send![manager, URLsForDirectory: CACHES inDomains: USER_DOMAIN];
        let url: ObjcId = if urls != nil { msg_send![urls, firstObject] } else { nil };
        let path: ObjcId = if url != nil { msg_send![url, path] } else { nil };
        let path = (path != nil).then(|| nsstring_to_string(path));
        let () = msg_send![pool, release];
        path
    }
}

/// Reads a file relative to the application's bundle resources (Foundation, no AppKit / UIKit
/// state).
#[cfg(target_vendor = "apple")]
pub fn bundle_resource(path: &str) -> Option<Vec<u8>> {
    std::fs::read(std::path::Path::new(&bundle_resource_dir()?).join(path)).ok()
}

/// Queues activation of a running process. The OS may decline the request.
#[cfg(target_os = "macos")]
pub fn activate_process(pid: u32) -> bool {
    dispatch_main(move || unsafe {
        use makepad_apple_sys::*;
        let app: ObjcId = msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid as i32];
        if app != nil {
            let _: BOOL = msg_send![app, activateWithOptions: 2u64];
        }
    })
}

/// Brings a running process's main window to the front; false when it has none or the OS refuses.
#[cfg(windows)]
pub fn activate_process(pid: u32) -> bool {
    backend::windows::win32::activate_process(pid)
}

/// Opens an external URL with its registered handler.
#[cfg(windows)]
pub fn open_url(url: String) -> bool {
    backend::windows::win32::open_url(&url)
}

/// Opens an external URL on the process main thread. The OS handles the URL scheme.
#[cfg(target_os = "macos")]
pub fn open_url(url: String) -> bool {
    dispatch_main(move || unsafe {
        use makepad_apple_sys::*;
        let value: ObjcId = msg_send![class!(NSURL), URLWithString: str_to_nsstring(&url)];
        if value != nil {
            let workspace: ObjcId = msg_send![class!(NSWorkspace), sharedWorkspace];
            let _: BOOL = msg_send![workspace, openURL: value];
        }
    })
}

/// Reads an APK asset into caller-owned memory. Returns its size, or -1 on failure.
/// A short buffer only queries the size; APK assets are immutable between the two calls.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_read_asset(path: &str, out: &mut [u8]) -> i64 {
    backend::android::assets::read(path, out)
}

/// Socket streams for the app's network stack (Android has no libssl for native code, so TLS is the
/// Activity's `openSocketStream` family). A stream is named by an `id` the caller picks and keeps
/// unique; any thread may call. `flags` bit 0: TLS, bit 1: skip certificate validation.
#[cfg(target_os = "android")]
pub const SOCKET_TLS: u32 = 1;
#[cfg(target_os = "android")]
pub const SOCKET_IGNORE_CERT: u32 = 2;

/// Opens stream `id`. Returns 1 when open, 0 when Java refused.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_socket_open(id: i64, host: &str, port: i32, flags: u32) -> u32 {
    backend::android::socket::open(id, host, port, flags & SOCKET_TLS != 0, flags & SOCKET_IGNORE_CERT != 0) as u32
}

/// Reads into `out`, blocking until bytes arrive or the read timeout passes. Returns the byte count,
/// 0 when the read timed out, -1 when the stream is closed or failed.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_socket_read(id: i64, out: &mut [u8]) -> i64 {
    backend::android::socket::read_into(id, out).map_or(-1, |n| n as i64)
}

/// Writes `data`. Returns the bytes written, or -1.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_socket_write(id: i64, data: &[u8]) -> i64 {
    backend::android::socket::write(id, data) as i64
}

/// Sets the read (`read` != 0) or write timeout in milliseconds (0: none).
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_socket_timeout(id: i64, read: u32, ms: i32) {
    backend::android::socket::set_timeout(id, read != 0, ms)
}

/// Writes the SHA-256 of the server certificate (once handshaken) into `out` (32 bytes). Returns 32,
/// or -1 when there is none.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_socket_fingerprint(id: i64, out: &mut [u8]) -> i64 {
    match backend::android::socket::peer_fingerprint(id) {
        Some(fp) if fp.len() == 32 && out.len() >= 32 => { out[..32].copy_from_slice(&fp); 32 }
        _ => -1,
    }
}

/// Closes stream `id`.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_socket_close(id: i64) {
    backend::android::socket::close(id)
}

/// The path prefix naming a stored APK asset in a font open (`apk:<asset path>`).
#[cfg(target_os = "android")]
pub const APK_ASSET_PREFIX: &str = "apk:";

/// Opens a stored APK asset as a file range so the caller can map it without a heap copy: returns
/// an open descriptor the caller owns (-1 for a missing or compressed asset) and writes
/// `out = [start, length]`.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_map_asset(path: &str, out: &mut [i64]) -> i64 {
    backend::android::assets::range(path, out)
}

/// Opens an external URL on the process main thread. The OS handles the URL scheme.
#[cfg(any(target_os = "ios", target_os = "tvos"))]
pub fn open_url(url: String) -> bool {
    dispatch_main(move || unsafe {
        use makepad_apple_sys::*;
        let value: ObjcId = msg_send![class!(NSURL), URLWithString: str_to_nsstring(&url)];
        if value != nil {
            let app: ObjcId = msg_send![class!(UIApplication), sharedApplication];
            let options: ObjcId = msg_send![class!(NSDictionary), dictionary];
            let () = msg_send![app, openURL: value options: options completionHandler: nil];
        }
    })
}
