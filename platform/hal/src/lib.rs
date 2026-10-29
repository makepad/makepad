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

/// The HAL's maps of objects, keyed by small ids ((session, index) pairs, string ids): hashed
/// with the in-tree FxHash. The per-draw path looks textures, buffers and pipelines up in them
/// many times a frame, where std's SipHash was ~4% of the main thread's encode samples.
pub type FastMap<K, V> = std::collections::HashMap<K, V, fxhash::FxBuildHasher>;

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
pub mod call_scope;
pub mod completion;
pub mod clock;
#[cfg_attr(not(any(test, target_os = "android")), allow(dead_code))]
mod xr_carry;
pub mod fake_camera;
pub mod clipboard;
pub mod drag_promise;
pub mod direct;
#[cfg(target_os = "linux")]
pub mod wide_desktop;
#[cfg(target_os = "linux")]
pub mod wm_socket;
/// Descriptor I/O over Unix sockets: hosted clients and hosts, and frame rings (app_host).
#[cfg(target_os = "linux")]
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
pub mod linux_ipc;
#[cfg(target_os = "linux")]
pub mod hosted_ipc;
/// The native-layer bridge used by process-hosted Cx; the WM HAL shell uses it directly.
#[cfg(target_os = "linux")]
pub use backend::linux::wm::native as hosted_native;
pub mod cmds;
pub mod codec;
pub mod events;
pub mod font;
pub mod ledger;
pub mod gpu_limits;
#[cfg(not(target_arch = "wasm32"))]
pub mod gpu_prof;
pub mod gpu_pressure;
pub mod h264_bits;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod stream_encode;
#[cfg(not(target_arch = "wasm32"))]
pub mod stream_decode;
pub mod stream_pack;
pub mod xr_remote;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod frame_tap;
#[cfg(not(target_arch = "wasm32"))]
pub mod framecap;
#[cfg(not(target_arch = "wasm32"))]
pub mod inspect;
pub mod media;
pub mod native_layer;
pub mod profile;
pub mod threadcpu;
pub mod session;
pub mod types;
pub mod window_controls;
pub use window_controls::{host_caption, host_window_controls, HostCaption, WindowControls};

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

pub use session::{AppCaption, CAPTION_BADGE_ROOM, CloseAs, DomainLaunch, ExecRecord};

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
    /// `before_exit` ran: the process is ending, no backend is made again
    exited: bool,
}

/// Who has something to pick up: one blocking queue per session, plus the all-sessions wake.
#[derive(Default)]
struct Signal {
    /// bumped by anything an `await_*` waits for
    epoch: u64,
    /// bumped when every session's pump must look again (global wake, clock change)
    all: u64,
    /// per session: its queue got an event or it was woken (never reset: a pump compares it)
    per: HashMap<u32, u64>,
    /// `hal_wake` count, and `wake_session` count per session
    wake_all: u64,
    wake_one: HashMap<u32, u64>,
    /// the wake counts each session's pump has turned into a `Wake` event
    taken: HashMap<u32, (u64, u64)>,
    /// each session's pump blocks on its own condvar: news for one session wakes that pump alone
    cvs: HashMap<u32, Arc<Condvar>>,
    /// what each session's pump did, by cause (`hal pump`)
    counts: HashMap<u32, PumpCounts>,
}

/// One session's pump loop, counted: every turn steps the backend under the state lock, so an
/// idle system must show none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PumpCounts {
    /// `hal_pump` calls
    pub calls: u64,
    /// loop turns (each steps the backend under the state lock)
    pub turns: u64,
    /// calls that returned events
    pub delivered: u64,
    /// blocked waits that ended because of: events for this session, a wake (`hal_wake` or
    /// `wake_session`), every pump asked to look (window system news, clock, font glyphs),
    /// the timeout (a due timer, the deadline)
    pub by_event: u64,
    pub by_wake: u64,
    pub by_all: u64,
    pub by_timeout: u64,
}

/// The counters a pump compares across its wait.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Ticks {
    all: u64,
    per: u64,
    wake_all: u64,
    wake_one: u64,
}

impl Signal {
    fn ticks(&self, sid: u32) -> Ticks {
        Ticks {
            all: self.all,
            per: self.per.get(&sid).copied().unwrap_or(0),
            wake_all: self.wake_all,
            wake_one: self.wake_one.get(&sid).copied().unwrap_or(0),
        }
    }

    /// The condvar `sid`'s pump blocks on.
    fn cv(&mut self, sid: u32) -> Arc<Condvar> {
        self.cvs.entry(sid).or_default().clone()
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

    /// The session is gone: a pump still blocked for it looks once more (and hears its end).
    fn forget(&mut self, sid: u32) {
        *self.per.entry(sid).or_default() += 1;
        if let Some(cv) = self.cvs.remove(&sid) {
            cv.notify_all();
        }
        self.wake_one.remove(&sid);
        self.taken.remove(&sid);
        self.counts.remove(&sid);
    }
}

/// The wake signal and what waits on it; backends hold a handle to wake the pump from their
/// own threads.
struct Sig {
    signal: Mutex<Signal>,
    /// the host's `await_*` waits (on `epoch`); pumps wait on their session's own condvar
    cv: Condvar,
}

impl Sig {
    /// Something happened for these sessions' queues (and for the host's `await_*`): only
    /// their pumps wake.
    fn notify(&self, sessions: &[u32]) {
        let mut s = lock(&self.signal);
        s.epoch += 1;
        for id in sessions {
            *s.per.entry(*id).or_default() += 1;
            if let Some(cv) = s.cvs.get(id) {
                cv.notify_all();
            }
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
        for cv in s.cvs.values() {
            cv.notify_all();
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

/// Lifts this process's soft open-file limit to its hard limit (Linux, Android): every process
/// that makes a HAL (`Hal::new`), the WM and its hosted apps alike. NVIDIA's Linux driver holds
/// one file descriptor on /dev/nvidiaN per memory allocation, and the usual soft limit is 1024:
/// a game with a few hundred buffers, textures and upload arenas reached it, and from then on every create on the device -- fences, command pools, upload
/// arenas, descriptor pools -- failed with OUT_OF_HOST/DEVICE_MEMORY while the GPU had 16 GB free
/// (149, Sandbox at ~1 fps). The hard limit is the system's real bound (524288 on 149).
/// Hosted children inherit the raised limit from their host.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn raise_open_file_limit() {
    #[repr(C)]
    struct Rlimit { current: std::ffi::c_ulong, max: std::ffi::c_ulong }
    extern "C" {
        fn getrlimit(resource: std::ffi::c_int, limit: *mut Rlimit) -> std::ffi::c_int;
        fn setrlimit(resource: std::ffi::c_int, limit: *const Rlimit) -> std::ffi::c_int;
    }
    const RLIMIT_NOFILE: std::ffi::c_int = 7;
    let mut limit = Rlimit { current: 0, max: 0 };
    if unsafe { getrlimit(RLIMIT_NOFILE, &mut limit) } != 0 || limit.current >= limit.max { return; }
    let raised = Rlimit { current: limit.max, max: limit.max };
    if unsafe { setrlimit(RLIMIT_NOFILE, &raised) } != 0 {
        eprintln!("[hal] open-file limit stays at {} (raising it to {} failed: {})", limit.current, limit.max, std::io::Error::last_os_error());
    }
}
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn raise_open_file_limit() {}

/// The process's open file descriptors and its soft limit, for an out-of-memory report: at the
/// limit the driver answers every create with out of memory (`raise_open_file_limit`).
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn open_files() -> Option<(usize, u64)> {
    let open = std::fs::read_dir("/proc/self/fd").ok()?.count();
    let limits = std::fs::read_to_string("/proc/self/limits").ok()?;
    let soft = limits.lines().find(|l| l.starts_with("Max open files"))?.split_whitespace().nth(3)?.parse().ok()?;
    Some((open, soft))
}
/// Whether `open` descriptors are at the soft limit `soft` for the driver's purposes (a few kept
/// for the sockets and files a create may open besides its allocation).
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn at_file_limit(open: usize, soft: u64) -> bool { open as u64 + 16 >= soft }

#[cfg(all(test, any(target_os = "linux", target_os = "android")))]
mod open_file_limit_tests {
    #[test]
    fn a_hal_lifts_the_soft_open_file_limit_to_the_hard_one() {
        let _hal = super::Hal::new();
        {
            let limits = std::fs::read_to_string("/proc/self/limits").unwrap();
            let line = limits.lines().find(|l| l.starts_with("Max open files")).unwrap();
            let v: Vec<&str> = line.split_whitespace().collect();
            assert_eq!(v[3], v[4], "soft limit not raised: {line}");
            let (open, soft) = super::open_files().unwrap();
            assert!(open > 0 && soft.to_string() == v[3]);
        }
    }
    #[test]
    fn the_file_limit_check_keeps_a_margin() {
        assert!(!super::at_file_limit(200, 1024));
        assert!(super::at_file_limit(1008, 1024), "16 left counts as the limit");
        assert!(super::at_file_limit(1024, 1024));
        assert!(!super::at_file_limit(1024, 524288));
    }
}

/// The backend's capabilities with what the HAL itself adds: a remote headset slot
/// (`xr_remote`) under any GPU backend.
fn backend_info_of(b: &dyn Backend) -> HalInfo {
    let mut info = b.info();
    info.flags |= xr_remote::info_flags();
    info
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Hal {
    pub fn new() -> Arc<Hal> {
        raise_open_file_limit();
        Arc::new_cyclic(|me| Hal {
            me: me.clone(),
            state: Mutex::new(State { world: World::new(), backend: None, backend_inits: 0, exited: false }),
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
        // a GPU result is news for the session it belongs to, and a window frame also for the
        // sessions that hear of that session's window frames
        let sig = self.sig.clone();
        let listeners = st.world.listeners.clone();
        #[allow(unused_variables)]
        let sessions: backend::SessionNotify = Arc::new(move |sid, window| {
            if !window {
                return sig.notify(&[sid]);
            }
            let l = lock(&listeners);
            let mut ids = [sid; 3];
            let mut n = 1;
            for id in l.get(&sid).into_iter().flatten().take(2) {
                ids[n] = *id;
                n += 1;
            }
            sig.notify(&ids[..n]);
        });
        let make = |id: u32| -> Option<Box<dyn Backend>> {
            match id {
                #[cfg(target_os = "linux")]
                backend_id::AUTO => match backend::linux::open(req.flags, wake.clone(), sessions.clone(), notify.clone()) {
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
                backend_id::METAL => backend::apple::Metal::new(req.flags, sessions.clone(), notify.clone()).map(|m| Box::new(m) as Box<dyn Backend>),
                #[cfg(windows)]
                backend_id::D3D11 => match backend::windows::D3d11::new(req.flags, wake.clone(), notify.clone()) {
                    Ok(d) => Some(Box::new(d)),
                    Err(e) => { eprintln!("[hal-d3d11] {e}"); None }
                },
                #[cfg(target_os = "linux")]
                backend_id::VULKAN => match backend::vulkan::Vulkan::new(req.flags, sessions.clone(), notify.clone()) {
                    Ok(v) => Some(Box::new(v)),
                    Err(e) => { eprintln!("[hal-vulkan] {e}"); None }
                },
                #[cfg(target_os = "android")]
                // the Activity's events and frame callbacks make the pumps look (no `Wake` for every
                // app); a GPU result its own session's
                backend_id::VULKAN => match backend::android::AndroidBackend::new(req.flags, sessions.clone(), notify.clone()) {
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
            None if st.exited => return refused,
            None => match make(req.backend) {
                Some(b) => {
                    st.backend = Some((b.info().backend, b));
                    st.backend_inits += 1;
                    // a remote headset slot's XR window events wake the pumps like the backend's
                    xr_remote::set_wake(notify.clone());
                }
                None => return refused,
            },
        }
        // the font service's workers wake the pumps when they have glyphs to deliver
        if st.world.font_notify.is_none() {
            st.world.font_notify = Some(notify.clone());
        }
        let mut info = backend_info_of(&*st.backend.as_ref().expect("backend").1);
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

    /// The backend this Hal runs (`backend_id`) and its shader dialect, once one is open.
    pub fn backend_info(&self) -> Option<HalInfo> {
        lock(&self.state).backend.as_ref().map(|(_, b)| backend_info_of(&**b))
    }

    /// Executes a batch. The slices in `buf` are read during this call only.
    pub fn submit(&self, buf: &CmdBuf) {
        let sid = buf.session;
        // SAFETY: the slices are borrowed for this call (hal_submit contract)
        let Some(view) = (unsafe { buf.view() }) else {
            if let Some(s) = lock(&self.state).world.session_mut(sid) {
                s.stats.bad_bufs += 1;
            }
            return;
        };
        // the batch's long driver work (pipeline variants its frames use first) runs on this
        // session's thread before the batch, without the lock: only this session waits for it
        let ahead = {
            let mut guard = lock(&self.state);
            let st = &mut *guard;
            st.backend.as_mut().and_then(|(_, b)| b.work_ahead(&st.world, sid, &view))
        };
        if let Some(work) = ahead {
            work();
        }
        let mut guard = lock(&self.state);
        let st = &mut *guard;
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
        // its windows may have changed: who hears of its window frames
        st.world.update_listeners(sid);
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
    /// early by [`Hal::wake`], an event for this session, or a due timer). Returns the event
    /// count. A turn steps the backend under the state lock; the pump blocks until there is news
    /// for its own session, so other sessions' traffic costs it no turn and an idle system none.
    pub fn pump(&self, timeout_ns: u64, out: &mut EventBuf) -> u32 {
        let sid = out.session;
        let t0 = std::time::Instant::now();
        let deadline = t0.checked_add(Duration::from_nanos(timeout_ns)).unwrap_or_else(|| t0 + Duration::from_secs(86400 * 365));
        let cv = {
            let mut sig = lock(&self.sig.signal);
            sig.counts.entry(sid).or_default().calls += 1;
            sig.cv(sid)
        };
        loop {
            let seen = {
                let mut sig = lock(&self.sig.signal);
                sig.counts.entry(sid).or_default().turns += 1;
                sig.ticks(sid)
            };
            let wait = {
                let mut guard = lock(&self.state);
                let st = &mut *guard;
                if let Some((_, b)) = st.backend.as_mut() {
                    b.step(&mut st.world, sid);
                    xr_remote::drain_events(sid, &mut st.world);
                    // glyphs the font workers finished go to the atlas and to the sessions
                    if let Some(gpu) = b.host_gpu() {
                        font::service(gpu, &mut st.world);
                    }
                }
                // A completion or a lent window's geometry can notify another session while
                // this pump steps the backend. Wake it now, even when no later submit follows.
                let notified = st.world.take_notified();
                if !notified.is_empty() {
                    self.sig.notify(&notified);
                }
                let woken = lock(&self.sig.signal).take_wake(sid);
                let now = st.world.now();
                // embedded windows' frames (no display link of their own) at display rate
                let embedded_due = st.world.deliver_embedded_frames(sid, now);
                let due = st.backend.as_ref().and_then(|(_, b)| b.next_due(&st.world, sid));
                let due = match (due, embedded_due) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) };
                if st.world.session(sid).is_none() {
                    // the session was ended under its app (killed, crashed, its parent closed):
                    // the app hears one quit request and ends its run
                    let mut sig = lock(&self.sig.signal);
                    sig.cvs.remove(&sid);
                    sig.counts.remove(&sid);
                    drop(sig);
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
                    lock(&self.sig.signal).counts.entry(sid).or_default().delivered += 1;
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
            // Block until this session has news or the wait ends. Only its own events, a wake,
            // or an all-sessions look end the wait early; a spurious condvar return waits on.
            let until = std::time::Instant::now() + wait;
            let mut sig = lock(&self.sig.signal);
            loop {
                let now = sig.ticks(sid);
                if now != seen {
                    let c = sig.counts.entry(sid).or_default();
                    if now.wake_all != seen.wake_all || now.wake_one != seen.wake_one {
                        c.by_wake += 1;
                    } else if now.per != seen.per {
                        c.by_event += 1;
                    } else {
                        c.by_all += 1;
                    }
                    break;
                }
                let left = until.saturating_duration_since(std::time::Instant::now());
                if left.is_zero() {
                    sig.counts.entry(sid).or_default().by_timeout += 1;
                    break;
                }
                sig = cv.wait_timeout(sig, left).unwrap_or_else(|e| e.into_inner()).0;
            }
        }
    }

    /// What each open session's pump did so far, by cause (`hal pump`; tests).
    pub fn pump_counts(&self) -> Vec<(u32, PumpCounts)> {
        let sig = lock(&self.sig.signal);
        let mut v: Vec<_> = sig.counts.iter().map(|(id, c)| (*id, *c)).collect();
        v.sort_by_key(|(id, _)| *id);
        v
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

    /// The host ends the process next (its sessions are done, its app domains stopped): the
    /// backend leaves the Hal -- no call reaches it any more, no backend is made again -- and its
    /// driver threads stop and are joined (work in flight completes or is abandoned), so exit()
    /// never runs the driver's exit handlers under a thread inside the driver. Returns once
    /// they are gone. A host API, the last HAL call of every way the host ends; once is enough.
    pub fn before_exit(&self) {
        let backend = {
            let mut guard = lock(&self.state);
            guard.exited = true;
            guard.backend.take()
        };
        // outside the state lock: a worker's last jobs may still deliver to the sessions
        if let Some((_, mut b)) = backend {
            b.before_exit();
            // the process ends: its GPU objects go with it (destroying them now would only be
            // more driver calls on the way out)
            std::mem::forget(b);
        }
    }

    /// Wakes one session's pump (a `Wake` event for it alone).
    pub fn wake_session(&self, session: u32) {
        let mut s = lock(&self.sig.signal);
        s.epoch += 1;
        *s.wake_one.entry(session).or_default() += 1;
        *s.per.entry(session).or_default() += 1;
        if let Some(cv) = s.cvs.get(&session) {
            cv.notify_all();
        }
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
/// serves its run loop (on macOS the main queue the system frameworks finish their work on).
pub fn run_main_loop() {
    main_loop::run()
}

/// The app lives in the menu bar (macOS): it starts as an accessory app (no Dock icon, no menu
/// bar) like a bundle whose Info.plist says `LSUIElement`, for a binary that is not such a bundle
/// (a host a launcher bundle starts). Set before the window system starts. A host API.
#[cfg(target_os = "macos")]
pub fn set_menu_bar_app(on: bool) {
    backend::apple::appkit::MENU_BAR_APP.store(on, std::sync::atomic::Ordering::Release);
}

/// A display: its extent and what is left of it after the menu bar and Dock, as x, y, w, h in the
/// window system's global points (macOS: bottom-left origin, the space window frames are in).
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScreenInfo {
    pub bounds: [f64; 4],
    pub work_area: [f64; 4],
    pub is_primary: bool,
}

/// The identity a launcher gave this process (MAKEPAD_APP_ID, set by `makepad app <id>` for the
/// OS entry of an app: its .desktop file names the same id): `nl.makepad.<id>`, which a Linux
/// window takes as its Wayland app_id and X11 WM_CLASS so the desktop groups it with its entry.
/// None without one (the window is named after the executable).
pub fn app_identity() -> Option<String> {
    app_id().map(|id| format!("nl.makepad.{id}"))
}

/// The launcher's app id itself (MAKEPAD_APP_ID when it is a valid one): Windows names the
/// process and its windows `Makepad.<id>`, the AppUserModelID of the app's Start menu entry.
pub fn app_id() -> Option<String> {
    let id = std::env::var("MAKEPAD_APP_ID").ok()?;
    let ok = !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    ok.then_some(id)
}

/// The displays attached right now, as the window system last saw them (readable from any
/// thread): the first `out.len()` of them are written to `out`, the number attached is returned
/// (a larger number: call again with room for all). None where there is no window system, or
/// before it is up. An op: the list lives in the shell's state.
#[relax_api]
pub fn hal_screens(out: &mut [ScreenInfo]) -> u32 {
    let all = main_loop::screens();
    for (o, s) in out.iter_mut().zip(&all) {
        *o = *s;
    }
    all.len() as u32
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
    pub use super::backend::linux::host_dark;
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

/// Wakes every session's blocked `hal_pump`. Callable from any thread; never waits for the main
/// thread.
#[relax_api]
pub fn hal_wake() {
    Hal::global().wake()
}

/// Wakes one session's blocked `hal_pump` (a `Wake` event for it alone): what an app's own
/// threads call, so its signals do not turn every other app's loop. Any thread.
#[relax_api]
pub fn hal_wake_session(session: u32) {
    Hal::global().wake_session(session)
}

/// One texture or buffer of a session's GPU ledger (`hal_allocations`).
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Allocation {
    /// a texture; else a buffer
    pub texture: bool,
    /// a vertex or index buffer (the ledger's "geometry")
    pub geometry: bool,
    pub index: u32,
    pub gen: u32,
    /// the backend's allocated bytes
    pub bytes: u64,
}

/// What `hal_allocations` found besides the list.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Allocations {
    /// the session is open
    pub open: bool,
    /// the textures and buffers it has (more than `out` held: call again with room for all)
    pub count: u32,
    /// the backend reports the device's own allocated total, `device_allocated`
    pub has_device: bool,
    pub device_allocated: u64,
}

/// The textures and buffers of session `session` as its GPU ledger has them (any thread): the
/// first `out.len()` are written to `out`. An op: the ledger is the HAL's state.
#[relax_api]
pub fn hal_allocations(session: u32, out: &mut [Allocation]) -> Allocations {
    let Some(report) = Hal::global().report(session, 0) else {
        return Allocations::default();
    };
    let geometry = backend::exec::buffer_detail(buffer_kind::VERTEX);
    let mut count = 0;
    for item in report.items.iter().filter(|i| matches!(i.kind, ledger::Kind::Texture | ledger::Kind::Buffer)) {
        if let Some(o) = out.get_mut(count) {
            *o = Allocation { texture: item.kind == ledger::Kind::Texture, geometry: item.kind == ledger::Kind::Buffer && item.detail == geometry, index: item.index, gen: item.gen, bytes: item.bytes };
        }
        count += 1;
    }
    Allocations { open: true, count: count as u32, has_device: report.device_allocated.is_some(), device_allocated: report.device_allocated.unwrap_or(0) }
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
#[relax_api]
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
#[relax_api]
pub fn activate_process(pid: u32) -> bool {
    backend::windows::win32::activate_process(pid)
}

/// Opens an external URL with its registered handler.
#[cfg(windows)]
#[relax_api]
pub fn open_url(url: &str) -> bool {
    backend::windows::win32::open_url(url)
}

/// Opens an external URL on the process main thread. The OS handles the URL scheme.
#[cfg(target_os = "macos")]
#[relax_api]
pub fn open_url(url: &str) -> bool {
    let url = url.to_string();
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

/// Copies the server's leaf certificate DER into `out` when it fits. Returns the required
/// byte length (call with an empty buffer to query it), or -1 when there is no certificate.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_socket_certificate(id: i64, out: &mut [u8]) -> i64 {
    match backend::android::socket::peer_certificate(id) {
        Some(der) => {
            if out.len() >= der.len() {
                out[..der.len()].copy_from_slice(&der);
            }
            der.len() as i64
        }
        None => -1,
    }
}

/// Closes stream `id`.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_socket_close(id: i64) {
    backend::android::socket::close(id)
}

/// The phone's codecs for makepad_video: HEVC stills (the image tile libraries' frames) and
/// the file decoder. A call is named by an `id` the caller picks and keeps unique (a file
/// decoder keeps its open id for its life); its result waits under that id until
/// `hal_media_take`. Any thread may call.
#[cfg(target_os = "android")]
fn media_results() -> &'static std::sync::Mutex<std::collections::HashMap<i64, Result<Vec<u8>, String>>> {
    static RESULTS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<i64, Result<Vec<u8>, String>>>> = std::sync::OnceLock::new();
    RESULTS.get_or_init(Default::default)
}

/// Keeps `result` under `id`: its length, or -1 - the length of the error text.
#[cfg(target_os = "android")]
fn media_keep(id: i64, result: Result<Vec<u8>, String>) -> i64 {
    let len = match &result {
        Ok(bytes) => bytes.len() as i64,
        Err(e) => -1 - e.len() as i64,
    };
    media_results().lock().unwrap_or_else(|e| e.into_inner()).insert(id, result);
    len
}

/// The headset's pose for a renderer elsewhere, once per display frame (Android XR: published by
/// the XR thread right after xrBeginFrame, located `XrPoseLead` past the display time, whether
/// or not the app draws that frame): waits at most `timeout_ms` for one newer than the last
/// taken, writes its `XrFrame` (`buffers::Wire`) into `out` and returns its display time (ns);
/// 0 when none came (or elsewhere). A thread of the app's own sends it on, so the uplink keeps
/// the display's rate and spacing whatever the app's frames do.
#[relax_api]
pub fn hal_xr_uplink_next(timeout_ms: u32, out: &mut [u8]) -> u64 {
    #[cfg(target_os = "android")]
    return backend::android::xr::uplink_next(timeout_ms, out);
    #[cfg(not(target_os = "android"))]
    {
        let _ = (timeout_ms, out);
        0
    }
}

/// The XR tracking log (Android XR, while the uplink runs): what went upstream (kind 0: the
/// sample time, the display time it was located for, its `XrFrame`) and the poses as tracked
/// (kind 1: a display time already past, located after it, the time it was located, the
/// `XrFrame` without controllers). Entries from absolute index `cursor` on (the oldest kept when
/// older) are written into `out` after an 8-byte count of them (u64 LE), each kind, time, aux
/// (u64 LE) then the `XrFrame` ([`hal_xr_track_entry_size`] bytes each), as many whole ones as
/// fit; returns the index after the last written. Elsewhere nothing (count 0).
#[relax_api]
pub fn hal_xr_track_log(cursor: u64, out: &mut [u8]) -> u64 {
    if out.len() < 8 {
        return cursor;
    }
    #[cfg(target_os = "android")]
    {
        let (next, bytes) = backend::android::xr::track_log(cursor, &mut out[8..]);
        let n = (bytes / backend::android::xr::track_entry_size().max(1)) as u64;
        out[..8].copy_from_slice(&n.to_le_bytes());
        return next;
    }
    #[cfg(not(target_os = "android"))]
    {
        out[..8].copy_from_slice(&0u64.to_le_bytes());
        cursor
    }
}

/// The size of one [`hal_xr_track_log`] entry in bytes.
#[relax_api]
pub fn hal_xr_track_entry_size() -> usize {
    #[cfg(target_os = "android")]
    return backend::android::xr::track_entry_size();
    #[cfg(not(target_os = "android"))]
    0
}

/// Plays a recorded tracking log (entries as [`hal_xr_track_log`] writes them) instead of the
/// runtime's poses, looping: the uplink gets the recorded predictions (moved to the replay's
/// display times along the tracked path), the XR window's frames the tracked path. Empty: the
/// runtime's poses again. Whether a replay runs (Android XR; elsewhere false).
#[relax_api]
pub fn hal_xr_replay(data: &[u8]) -> bool {
    #[cfg(target_os = "android")]
    return backend::android::xr::replay(data);
    #[cfg(not(target_os = "android"))]
    {
        let _ = data;
        false
    }
}

/// The XR window's frames write linear colour (`on`): its projection chains become sRGB images
/// without the mutable format (the GPU keeps their framebuffer compression: the stream pass's
/// fixed cost 1.7 -> 0.1 ms on Quest 3) and the hardware encodes on write; the app's shaders then
/// output linear colour into it ([`hal_xr_linear_frames_active`] says from which frame on).
/// Off: sRGB-encoded colour written through UNORM views, as every app's frames. Android XR;
/// elsewhere nothing.
#[relax_api]
pub fn hal_xr_linear_frames(on: bool) {
    #[cfg(target_os = "android")]
    backend::android::xr::linear_frames(on);
    #[cfg(not(target_os = "android"))]
    let _ = on;
}

/// The XR window's stereo view paused (`on`): no frame is asked of the app and none is shown
/// (only the room: passthrough); the frame loop, the input, the uplink's poses and the depth
/// provider go on. A streamed view whose stream is off. Android XR; elsewhere nothing.
#[relax_api]
pub fn hal_xr_stereo_paused(on: bool) {
    #[cfg(target_os = "android")]
    backend::android::xr::stereo_paused(on);
    #[cfg(not(target_os = "android"))]
    let _ = on;
}

/// The XR panel (the host window as a quad layer in the room): `show` 0 the default rule (shown
/// without an XR window; over one only after a thumbstick click), 1 always, 2 never; it hangs
/// `distance_m` away and is `width_m` wide; `follow`: it stays in front of the person, following
/// the head lazily (it recentres smoothly once the view drifts off it), display only (no
/// pointer). The image is the window's own frames: a window that draws nothing new costs no
/// GPU work. Android XR; elsewhere nothing.
#[relax_api]
pub fn hal_xr_panel(show: u32, distance_m: f32, width_m: f32, follow: bool) {
    #[cfg(target_os = "android")]
    backend::android::xr::panel(show, distance_m, width_m, follow);
    #[cfg(not(target_os = "android"))]
    let _ = (show, distance_m, width_m, follow);
}

/// The near and far planes of every XR eye projection (metres in the room): the app's eye
/// passes, a remote headset's stream (its depth band decoded with them) and its warp. The one
/// definition.
pub const XR_EYE_NEAR_M: f32 = 0.05;
pub const XR_EYE_FAR_M: f32 = 2000.0;

/// The XR window's fixed foveation (Android XR: the runtime's foveation profile in the
/// projection chains' fragment density maps; 0 none, 1 low, 2 medium, 3 high), applied from the
/// next display frame. Elsewhere nothing.
#[relax_api]
pub fn hal_xr_foveation(level: u32) {
    #[cfg(target_os = "android")]
    backend::android::xr::foveation(level);
    #[cfg(not(target_os = "android"))]
    let _ = level;
}

/// The headset's environment depth runs (`on`, the default): the occlusion and the room field
/// read it; off stops the runtime's depth provider (its GPU and camera work). Android XR;
/// elsewhere nothing.
#[relax_api]
pub fn hal_xr_environment_depth(on: bool) {
    #[cfg(target_os = "android")]
    backend::android::xr::environment_depth(on);
    #[cfg(not(target_os = "android"))]
    let _ = on;
}

/// The environment depth images are read back to the app for its room field (`on`; off at
/// start: the app gets only the GPU image, no copy and no CPU work). Android XR; elsewhere
/// nothing.
#[relax_api]
pub fn hal_xr_depth_readback(on: bool) {
    #[cfg(target_os = "android")]
    backend::android::xr::depth_readback(on);
    #[cfg(not(target_os = "android"))]
    let _ = on;
}

/// Hands and controllers tracked at once (`on`, the default) or only whichever is in use: with
/// both, the Quest's cameras are shared with the controllers and its hand tracker runs at about
/// 25 Hz instead of its high-frequency mode. Android XR; elsewhere nothing.
#[relax_api]
pub fn hal_xr_hands_and_controllers(on: bool) {
    #[cfg(target_os = "android")]
    backend::android::xr::hands_and_controllers(on);
    #[cfg(not(target_os = "android"))]
    let _ = on;
}

/// The XR window's eye buffers move between `min_percent` and `max_percent` of their start size
/// (40..100; equal: fixed) on the runtime's measured app GPU frame time: a step down (100, 90,
/// 80, 70) when a frame's GPU time nears the display period, back up with headroom, changing
/// rarely; the window's geometry follows (the app renders at the new size). Default 100..100.
/// Android XR (XR_META_performance_metrics); elsewhere nothing.
#[relax_api]
pub fn hal_xr_eye_scale(min_percent: u32, max_percent: u32) {
    #[cfg(target_os = "android")]
    backend::android::xr::eye_scale(min_percent, max_percent);
    #[cfg(not(target_os = "android"))]
    let _ = (min_percent, max_percent);
}

/// The XR window's eye buffer size now (percent of the start size, the high 32 bits) and the app
/// GPU frame time the dynamic scale last measured (µs, the low 32 bits; 0 none). Android XR;
/// elsewhere 100 and 0.
#[relax_api]
pub fn hal_xr_eye_scale_now() -> u64 {
    #[cfg(target_os = "android")]
    return { let (p, us) = backend::android::xr::eye_scale_now(); ((p as u64) << 32) | us as u64 };
    #[cfg(not(target_os = "android"))]
    { 100 << 32 }
}

/// The uplink's poses are located `frames` display frames past the headset's newest one (the
/// newest is the runtime's own pipeline ahead of now: a frame located that many past it has
/// `frames` periods from the pose's sample to its frame's start, whatever that pipeline depth
/// does); 0: from the pose lead (`XrPoseLead`) instead. Android XR; elsewhere nothing.
#[relax_api]
pub fn hal_xr_pose_frames_ahead(frames: u32) {
    #[cfg(target_os = "android")]
    backend::android::xr::pose_frames_ahead(frames);
    #[cfg(not(target_os = "android"))]
    let _ = frames;
}

/// Whether the XR window's frames render into linear-output (sRGB-encoding) targets now.
#[relax_api]
pub fn hal_xr_linear_frames_active() -> bool {
    #[cfg(target_os = "android")]
    return backend::android::xr::linear_frames_active();
    #[cfg(not(target_os = "android"))]
    false
}

/// The device's network in its low-latency mode while `on` (Android: a low-latency Wi-Fi lock,
/// no power save or background scans); elsewhere nothing. Whether it was applied.
#[relax_api]
pub fn hal_network_low_latency(on: bool) -> bool {
    #[cfg(target_os = "android")]
    return backend::android::media::network_low_latency(on).map_err(|e| eprintln!("[hal-android] low-latency network: {e}")).is_ok();
    #[cfg(not(target_os = "android"))]
    {
        let _ = on;
        false
    }
}

/// Writes the largest frame the HEVC encoder takes into `out` (width, height: little-endian
/// u32s). Returns 8, or -1 when the phone has no HEVC encoder.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_hevc_encoder_max(out: &mut [u8]) -> i64 {
    match backend::android::media::hevc::encoder_max_size() {
        Some((w, h)) if out.len() >= 8 => {
            out[..4].copy_from_slice(&w.to_le_bytes());
            out[4..8].copy_from_slice(&h.to_le_bytes());
            8
        }
        _ => -1,
    }
}

/// Encodes an NV12 still. The result (`hal_media_take`) is the parameter sets' length (u32 LE),
/// the parameter sets, then the frame, both Annex-B. Returns its length, or -1 - the error's.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_hevc_encode(id: i64, nv12: &[u8], width: u32, height: u32, bitrate: u32) -> i64 {
    let result = backend::android::media::hevc::encode(nv12, width, height, bitrate).map(|(config, frame)| {
        let mut out = Vec::with_capacity(4 + config.len() + frame.len());
        out.extend_from_slice(&(config.len() as u32).to_le_bytes());
        out.extend_from_slice(&config);
        out.extend_from_slice(&frame);
        out
    });
    media_keep(id, result)
}

/// Decodes one Annex-B access unit of a `width` x `height` picture. The result is its NV12.
/// Returns its length, or -1 - the error's.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_hevc_decode(id: i64, annex_b: &[u8], width: u32, height: u32) -> i64 {
    media_keep(id, backend::android::media::hevc::decode(annex_b, width, height))
}

/// Copies the result (or the error text) of call `id` into `out` and forgets it. Returns the
/// bytes copied, -1 when there is none.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_media_take(id: i64, out: &mut [u8]) -> i64 {
    let Some(result) = media_results().lock().unwrap_or_else(|e| e.into_inner()).remove(&id) else { return -1 };
    let bytes = match &result {
        Ok(bytes) => bytes.as_slice(),
        Err(e) => e.as_bytes(),
    };
    let n = bytes.len().min(out.len());
    out[..n].copy_from_slice(&bytes[..n]);
    n as i64
}

/// The open video files, by the id they were opened under. A decoder is locked by the one call
/// using it, never the table, so files decode side by side.
#[cfg(target_os = "android")]
fn video_files() -> &'static std::sync::Mutex<std::collections::HashMap<i64, std::sync::Arc<std::sync::Mutex<backend::android::media::video_file::FileDecoder>>>> {
    static FILES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<i64, std::sync::Arc<std::sync::Mutex<backend::android::media::video_file::FileDecoder>>>>> = std::sync::OnceLock::new();
    FILES.get_or_init(Default::default)
}

#[cfg(target_os = "android")]
fn video_file(id: i64) -> Result<std::sync::Arc<std::sync::Mutex<backend::android::media::video_file::FileDecoder>>, String> {
    video_files().lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned().ok_or_else(|| format!("no open video file {id}"))
}

/// Opens the video file at `path` (`audio_only` != 0: its sound alone) as decoder `id`. The result
/// (`hal_media_take`) is its info: width, height, fps_num, fps_den (u32 LE), duration in
/// microseconds (i64 LE), the picture's MIME type's length (u32 LE) and text, has_audio (u8),
/// sample_rate (u32 LE), channels (u16 LE), then the picture track's MediaFormat colour keys as
/// i32 LE: KEY_COLOR_STANDARD, KEY_COLOR_RANGE, KEY_COLOR_TRANSFER (0: not stated). Returns its
/// length, or -1 - the error's.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_video_file_open(id: i64, path: &str, audio_only: u32) -> i64 {
    let result = backend::android::media::video_file::FileDecoder::open(path, audio_only != 0).map(|decoder| {
        let info = decoder.info().clone();
        video_files().lock().unwrap_or_else(|e| e.into_inner()).insert(id, std::sync::Arc::new(std::sync::Mutex::new(decoder)));
        let mime = info.video_mime.unwrap_or_default();
        let mut out = Vec::new();
        for v in [info.width, info.height, info.fps_num, info.fps_den] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&info.duration_us.to_le_bytes());
        out.extend_from_slice(&(mime.len() as u32).to_le_bytes());
        out.extend_from_slice(mime.as_bytes());
        out.push(info.has_audio as u8);
        out.extend_from_slice(&info.sample_rate.to_le_bytes());
        out.extend_from_slice(&info.channels.to_le_bytes());
        for v in [info.color_standard, info.color_range, info.color_transfer] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    });
    media_keep(id, result)
}

/// The next picture of decoder `id`. The result is its time in microseconds (i64 LE), the
/// KEY_COLOR_RANGE it is delivered in (i32 LE, 0: not stated), then its NV12 (the info's width x
/// height). Returns its length, 0 at the end, or -1 - the error's.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_video_file_next_frame(id: i64) -> i64 {
    let result = video_file(id).and_then(|d| d.lock().unwrap_or_else(|e| e.into_inner()).next_frame());
    match result {
        Ok(None) => 0,
        Ok(Some((time, range, nv12))) => {
            let mut out = Vec::with_capacity(12 + nv12.len());
            out.extend_from_slice(&time.to_le_bytes());
            out.extend_from_slice(&range.to_le_bytes());
            out.extend_from_slice(&nv12);
            media_keep(id, Ok(out))
        }
        Err(e) => media_keep(id, Err(e)),
    }
}

/// The next chunk of sound of decoder `id`. The result is its time in microseconds (i64 LE),
/// sample rate (u32 LE), channels (u16 LE) and interleaved 16-bit samples (LE). Returns its
/// length, 0 at the end, or -1 - the error's.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_video_file_next_audio(id: i64) -> i64 {
    let result = video_file(id).and_then(|d| d.lock().unwrap_or_else(|e| e.into_inner()).next_audio());
    match result {
        Ok(None) => 0,
        Ok(Some((time, rate, channels, samples))) => {
            let mut out = Vec::with_capacity(14 + samples.len() * 2);
            out.extend_from_slice(&time.to_le_bytes());
            out.extend_from_slice(&rate.to_le_bytes());
            out.extend_from_slice(&channels.to_le_bytes());
            for s in samples {
                out.extend_from_slice(&s.to_le_bytes());
            }
            media_keep(id, Ok(out))
        }
        Err(e) => media_keep(id, Err(e)),
    }
}

/// Puts decoder `id` at the sync sample at or before `time_us`. Returns 0, or -1 - the error's
/// length (the text waits for `hal_media_take`).
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_video_file_seek(id: i64, time_us: i64) -> i64 {
    match video_file(id).and_then(|d| d.lock().unwrap_or_else(|e| e.into_inner()).seek(time_us)) {
        Ok(()) => 0,
        Err(e) => media_keep(id, Err(e)),
    }
}

/// Closes decoder `id`.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_video_file_close(id: i64) {
    video_files().lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    media_results().lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
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
#[relax_api]
pub fn open_url(url: &str) -> bool {
    let url = url.to_string();
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

/// Fleet identity crypto on Android (native code has no libcrypto/libssl; the platform's Java
/// security providers do the work: backend/android/fleet.rs). Keys are X9.63 (97 bytes), points
/// 65 bytes. Byte results go into `out`; the return is the byte count, or -1 on failure.
#[cfg(target_os = "android")]
fn fleet_out(r: Result<Vec<u8>, String>, out: &mut [u8]) -> i64 {
    match r {
        Ok(v) if v.len() <= out.len() => {
            out[..v.len()].copy_from_slice(&v);
            v.len() as i64
        }
        _ => -1,
    }
}

/// A fresh P-256 key into `out` (97 bytes).
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_fleet_generate(out: &mut [u8]) -> i64 {
    fleet_out(backend::android::fleet::generate_p256(), out)
}

/// DER ECDSA-SHA256 of `msg` by `key` into `out` (at most 72 bytes).
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_fleet_sign(key: &[u8], msg: &[u8], out: &mut [u8]) -> i64 {
    fleet_out(backend::android::fleet::sign(key, msg), out)
}

/// 1 when `sig` (DER) is `point`'s ECDSA-SHA256 of `msg`.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_fleet_verify(point: &[u8], msg: &[u8], sig: &[u8]) -> u32 {
    backend::android::fleet::verify(point, msg, sig) as u32
}

/// The raw P-256 ECDH secret of `key` and `point` into `out` (32 bytes).
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_fleet_ecdh(key: &[u8], point: &[u8], out: &mut [u8]) -> i64 {
    fleet_out(backend::android::fleet::ecdh(key, point), out)
}

/// A TLS server context (certificate DER + key): a handle, or -1.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_fleet_tls_server(cert: &[u8], key: &[u8]) -> i64 {
    backend::android::fleet::server(cert, key).unwrap_or(-1)
}

/// A new server session of a context: a handle, or -1.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_fleet_tls_session(server: i64) -> i64 {
    backend::android::fleet::session(server).unwrap_or(-1)
}

/// Feeds a session ciphertext from the peer (empty: advance the handshake). Returns bit 0 =
/// handshaking, bit 1 = closed, or -1 on failure. Output waits for `hal_fleet_tls_take`.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_fleet_tls_feed(session: i64, data: &[u8]) -> i64 {
    match backend::android::fleet::feed(session, data) {
        Ok((hs, closed)) => hs as i64 | (closed as i64) << 1,
        Err(_) => -1,
    }
}

/// Encrypts plaintext (`close` != 0: stages the close_notify instead). 0, or -1 on failure.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_fleet_tls_wrap(session: i64, data: &[u8], close: u32) -> i64 {
    if close != 0 {
        backend::android::fleet::close(session);
        return 0;
    }
    backend::android::fleet::wrap(session, data).map_or(-1, |_| 0)
}

/// Moves waiting bytes (`app` != 0: plaintext, else ciphertext to send) into `out`: the count
/// (0: none), -1 for an unknown handle.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_fleet_tls_take(session: i64, app: u32, out: &mut [u8]) -> i64 {
    backend::android::fleet::take(session, app != 0, out)
}

/// Frees a server or session handle.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_fleet_tls_free(handle: i64) {
    backend::android::fleet::free(handle)
}

/// The battery: level 0..=100 in bits 0..8, bit 8 = charging; -1 when unknown.
#[cfg(target_os = "android")]
#[relax_api]
pub fn hal_battery() -> i64 {
    backend::android::fleet::battery().map_or(-1, |(level, charging)| level as i64 | (charging as i64) << 8)
}

/// The host's hook for the Android BACKGROUND lifecycle (the activity stopped): an Android process
/// is killed, never exited, so the makepad run host writes its compile cache from here (the loader's
/// quit write) instead of at an exit that never comes. Set once, before the first stop.
static BACKGROUND_HOOK: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();
pub fn set_background_hook(f: fn()) {
    let _ = BACKGROUND_HOOK.set(f);
}
pub(crate) fn background_hook() {
    if let Some(f) = BACKGROUND_HOOK.get() {
        f();
    }
}
