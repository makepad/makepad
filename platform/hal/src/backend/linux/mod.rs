//! Linux desktop shell. Only this main-thread loop owns Xlib/Wayland window objects.
//! Session threads exchange commands/events with it; there is no Cx or HAL state lock here.
//! Vulkan leases native handles until its presentation worker has retired their surfaces.

mod input;
pub mod media;
pub(crate) mod io;
mod wayland;
mod x11;
pub mod direct;
mod wm;

use super::exec::OsCmd;
use super::log_enabled;
use super::vulkan::surface::{NativeSurface, SurfaceState, VkSurfaceSource};
use crate::{buffers::EventQueue, session::World, types::*, Id};
use makepad_vulkan::vk;
use std::collections::HashMap;
use std::ffi::CStr;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};

type Key = (u32, Id);
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Protocol {
    X11,
    Wayland,
    Direct,
    /// A makepad window manager hosts the windows (wm.rs).
    Wm,
}

enum Op {
    Cmd(u32, OsCmd),
    CloseSession(u32),
    Lend { host: Id, session: u32, alias: Id },
    Retire(u64),
    Reveal,
}

trait Shell {
    fn protocol(&self) -> Protocol;
    fn command(&mut self, op: Op);
    fn dispatch(&mut self) -> Result<(), String>;
    fn wait(&mut self, wake: i32) -> Result<(), String>;
    fn shutdown(&mut self);
    fn empty(&self) -> bool;
}

#[derive(Default)]
struct Run {
    wanted: bool,
    ready: Option<Result<Protocol, String>>,
    stopped: bool,
}

/// Shared data locks cover only queue/map operations; native calls run after releasing them.
pub struct Shared {
    ops: Mutex<Vec<Op>>,
    inbox: Mutex<HashMap<u32, EventQueue>>,
    surfaces: Mutex<HashMap<Key, Arc<Native>>>,
    wake: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
    run: Mutex<Run>,
    ready: Condvar,
    announced: AtomicBool,
    flags: AtomicU32,
    client_chrome: AtomicBool,
    revealed: AtomicBool,
    dpi: AtomicU32,
    next: AtomicU64,
    screens: Mutex<Vec<crate::ScreenInfo>>,
    egl_display: Mutex<Option<super::gl::surface::Display>>,
    reader: UnixStream,
    writer: UnixStream,
}

struct Native {
    shared: &'static Shared,
    token: u64,
    protocol: Protocol,
    display: usize,
    window: usize,
    pixels: AtomicU64,
    available: AtomicBool,
    presented: AtomicBool,
    transparent: AtomicBool,
}

impl Drop for Native {
    fn drop(&mut self) {
        if log_enabled() {
            eprintln!("[hal-linux] {:?} native token={} last Vulkan/window lease released", self.protocol, self.token);
        }
        self.shared.post(Op::Retire(self.token));
    }
}

impl Shared {
    pub fn global() -> &'static Self {
        static SHARED: OnceLock<Shared> = OnceLock::new();
        SHARED.get_or_init(|| {
            let (reader, writer) = UnixStream::pair().expect("HAL shell wake socket");
            reader
                .set_nonblocking(true)
                .expect("nonblocking shell reader");
            writer
                .set_nonblocking(true)
                .expect("nonblocking shell writer");
            Self {
                ops: Mutex::new(Vec::new()),
                inbox: Mutex::new(HashMap::new()),
                surfaces: Mutex::new(HashMap::new()),
                wake: Mutex::new(Vec::new()),
                run: Mutex::new(Run::default()),
                ready: Condvar::new(),
                announced: AtomicBool::new(false),
                flags: AtomicU32::new(0),
                client_chrome: AtomicBool::new(false),
                revealed: AtomicBool::new(false),
                dpi: AtomicU32::new(1f32.to_bits()),
                next: AtomicU64::new(1),
                screens: Mutex::new(Vec::new()),
                egl_display: Mutex::new(None),
                reader,
                writer,
            }
        })
    }

    /// Called by the session thread, never the main thread. Startup discovers the WSI extension
    /// before creating the process's Vulkan instance. Failure is explicit, not an offscreen app.
    pub fn start(
        &'static self,
        flags: u32,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), String> {
        if log_enabled() {
            eprintln!("[hal-linux] shell startup requested flags={flags:#x}");
        }
        if !self.announced.load(Ordering::Acquire) {
            return Err(
                "Linux windows require expect_main_loop/run_main_loop on the main thread".into(),
            );
        }
        self.flags.fetch_or(flags, Ordering::Release);
        lock(&self.wake).push(wake);
        let mut run = lock(&self.run);
        run.wanted = true;
        self.signal();
        while run.ready.is_none() && !run.stopped {
            run = self.ready.wait(run).unwrap_or_else(|e| e.into_inner());
        }
        run.ready
            .clone()
            .unwrap_or_else(|| Err("Linux shell stopped during startup".into()))
            .map(|_| ())
    }

    fn signal(&self) {
        match (&self.writer).write(&[1]) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {} // the fd is already readable
            Err(e) => eprintln!("[hal-linux] wake: {e}"),
        }
    }
    fn post(&self, op: Op) {
        lock(&self.ops).push(op);
        self.signal();
    }
    fn take(&self) -> Vec<Op> {
        let mut bytes = [0; 256];
        loop {
            match (&self.reader).read(&mut bytes) {
                Ok(0) => {
                    eprintln!("[hal-linux] wake socket unexpectedly closed");
                    break;
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    eprintln!("[hal-linux] wake socket read: {e}");
                    break;
                }
            }
        }
        std::mem::take(&mut *lock(&self.ops))
    }
    pub fn command(&self, sid: u32, cmd: OsCmd) {
        if log_enabled() {
            match &cmd {
                OsCmd::WindowCreate { id, kind, parent, geom } => eprintln!(
                    "[hal-linux] OsCmd::WindowCreate session={sid} window={id:?} kind={kind:#x} parent={parent:?} geom={geom:?}"
                ),
                OsCmd::WindowClose { id } => eprintln!(
                    "[hal-linux] OsCmd::WindowClose session={sid} window={id:?} (app command)"
                ),
                OsCmd::FrameRequest { id } => eprintln!(
                    "[hal-linux] OsCmd::FrameRequest session={sid} window={id:?}"
                ),
                _ => {}
            }
        }
        self.post(Op::Cmd(sid, cmd));
    }
    pub fn session_closed(&self, sid: u32) {
        if log_enabled() {
            eprintln!("[hal-linux] session={sid} closed; queuing native window teardown");
        }
        self.post(Op::CloseSession(sid));
    }
    pub fn lend(&self, host: Id, session: u32, alias: Id) {
        self.post(Op::Lend {
            host,
            session,
            alias,
        });
    }
    pub fn presented(&self, key: Key) {
        let first = lock(&self.surfaces).get(&key).is_some_and(|n| {
            !n.presented.load(Ordering::Acquire) && !n.presented.swap(true, Ordering::AcqRel)
        });
        if first && log_enabled() {
            eprintln!("[hal-linux] first presentation completed key={key:?}");
        }
        if self.flags() & open_flags::HIDDEN_UNTIL_PRESENT != 0
            && !self.revealed.swap(true, Ordering::AcqRel)
        {
            self.post(Op::Reveal);
        }
    }
    pub fn dpi(&self) -> f32 {
        f32::from_bits(self.dpi.load(Ordering::Acquire))
    }
    pub fn client_chrome(&self) -> bool {
        self.client_chrome.load(Ordering::Acquire)
    }
    pub fn direct_display(&self) -> bool {
        matches!(lock(&self.run).ready.as_ref(), Some(Ok(Protocol::Direct)))
    }
    fn flags(&self) -> u32 {
        self.flags.load(Ordering::Acquire)
    }
    fn push(&self, key: Key, f: impl FnOnce(&mut EventQueue, Id)) {
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
    fn window_failed(&self, key: Key, reason: &str) {
        eprintln!("[hal-linux] window create failed key={key:?}: {reason}; emitting Closed");
        self.push(key, |q, id| q.closed(id));
    }
    pub fn drain_events(&self, sid: u32, world: &mut World) -> Vec<Key> {
        let queues = if sid == crate::session::HOST {
            std::mem::take(&mut *lock(&self.inbox))
        } else {
            lock(&self.inbox)
                .remove(&sid)
                .map(|q| HashMap::from([(sid, q)]))
                .unwrap_or_default()
        };
        let mut closed = Vec::new();
        for (recipient, mut q) in queues {
            q.drain_with(|e, p| {
                if log_enabled() && matches!(e.tag, crate::events::event_tag::Geom | crate::events::event_tag::FrameReady) {
                    eprintln!("[hal-linux] delivering to session={recipient}: {:?}", e.decode(p));
                }
                if e.tag == crate::events::event_tag::Closed {
                    closed.push((recipient, e.window));
                }
                world.window_event(recipient, e, p);
            });
        }
        closed
    }
    fn register(&'static self, key: Key, protocol: Protocol, display: usize, window: usize) -> u64 {
        let token = self.next.fetch_add(1, Ordering::Relaxed);
        let native = Arc::new(Native {
            shared: self,
            token,
            protocol,
            display,
            window,
            pixels: AtomicU64::new(0),
            available: AtomicBool::new(false),
            presented: AtomicBool::new(false),
            transparent: AtomicBool::new(false),
        });
        let old = lock(&self.surfaces).insert(key, native);
        if log_enabled() {
            eprintln!("[hal-linux] {protocol:?} native window registered key={key:?} token={token} display={display:#x} window={window:#x}; waiting for geometry");
        }
        drop(old);
        token
    }
    fn geometry(&self, key: Key, size: [u32; 2], available: bool) {
        let token = lock(&self.surfaces).get(&key).map(|n| {
            n.pixels
                .store((size[0] as u64) << 32 | size[1] as u64, Ordering::Release);
            n.available.store(available, Ordering::Release);
            n.token
        });
        if log_enabled() {
            eprintln!("[hal-linux] native geometry key={key:?} token={token:?} pixels={size:?} available={available}");
        }
    }
    fn unregister(&self, key: Key) {
        let old = lock(&self.surfaces).remove(&key);
        if log_enabled() {
            if let Some(native) = &old {
                eprintln!("[hal-linux] unregister key={key:?} token={} remaining Vulkan leases={}", native.token, Arc::strong_count(native) - 1);
            }
        }
        drop(old); // final lease schedules destruction, never performs it on a Vulkan worker
    }
    fn visuals(&self, key: Key, transparent: bool) {
        if let Some(native) = lock(&self.surfaces).get(&key) {
            native.transparent.store(transparent, Ordering::Release);
        }
    }
}

pub struct SurfaceSource(pub &'static Shared);
/// The GL backend's view of the same native-window leases Vulkan uses; no EGL call runs here.
pub struct EglSurfaceSource(pub &'static Shared);
impl super::gl::surface::EglSurfaceSource for EglSurfaceSource {
    fn display(&self) -> Result<super::gl::surface::Display, String> {
        (*lock(&self.0.egl_display)).ok_or_else(|| "the DRM/KMS direct shell presents through Vulkan only".into())
    }
    fn state(&mut self, sid: u32, id: Id) -> super::gl::surface::SurfaceState {
        use super::gl::surface::{NativeSurface, SurfaceState};
        let native = lock(&self.0.surfaces).get(&(sid, id)).filter(|n| n.available.load(Ordering::Acquire)).cloned();
        let Some(n) = native else { return SurfaceState::Lost };
        let size = n.pixels.load(Ordering::Acquire);
        SurfaceState::Native(NativeSurface { generation: n.token, window: n.window, size: [(size >> 32) as u32, size as u32], keep: n })
    }
}
impl VkSurfaceSource for SurfaceSource {
    fn transparent(&self, sid: u32, window: Id) -> bool {
        lock(&self.0.surfaces)
            .get(&(sid, window))
            .is_some_and(|n| n.transparent.load(Ordering::Acquire))
    }
    fn instance_extensions(&self) -> Vec<&'static CStr> {
        match lock(&self.0.run)
            .ready
            .as_ref()
            .and_then(|r| r.as_ref().ok())
        {
            Some(Protocol::Wayland) => vec![vk::KHR_WAYLAND_SURFACE_NAME],
            Some(Protocol::X11) => vec![vk::KHR_XLIB_SURFACE_NAME],
            Some(Protocol::Direct) | Some(Protocol::Wm) => Vec::new(),
            None => Vec::new(),
        }
    }
    fn state(&mut self, sid: u32, id: Id, _: [u32; 2]) -> SurfaceState {
        let registry = lock(&self.0.surfaces);
        let Some(n) = registry
            .get(&(sid, id))
            .filter(|n| n.available.load(Ordering::Acquire))
        else {
            return SurfaceState::Lost;
        };
        let size = n.pixels.load(Ordering::Acquire);
        SurfaceState::Native {
            generation: n.token,
            size: [(size >> 32) as u32, size as u32],
        }
    }
    unsafe fn create(
        &mut self,
        instance: &makepad_vulkan::Instance,
        sid: u32,
        id: Id,
    ) -> Result<NativeSurface, String> {
        let native = lock(&self.0.surfaces)
            .get(&(sid, id))
            .cloned()
            .ok_or_else(|| format!("Vulkan surface key={:?}: native window was closed", (sid, id)))?;
        let call = match native.protocol {
            Protocol::Wayland => "vkCreateWaylandSurfaceKHR",
            Protocol::X11 => "vkCreateXlibSurfaceKHR",
            Protocol::Direct | Protocol::Wm => return Err("this shell presents through its own provider, not a window-system surface".into()),
        };
        if native.display == 0 || native.window == 0 {
            return Err(format!("{call} key={:?}: null native display/window handle", (sid, id)));
        }
        if log_enabled() {
            eprintln!("[hal-linux] {call} key={:?} token={} display={:#x} window={:#x}", (sid, id), native.token, native.display, native.window);
        }
        let handle = match native.protocol {
            Protocol::Wayland => instance.create_wayland_surface(
                &vk::WaylandSurfaceCreateInfoKHR::default()
                    .display(native.display as _)
                    .surface(native.window as _),
                None,
            ),
            Protocol::X11 => instance.create_xlib_surface(
                &vk::XlibSurfaceCreateInfoKHR::default()
                    .dpy(native.display as _)
                    .window(native.window as _),
                None,
            ),
            Protocol::Direct | Protocol::Wm => return Err("this shell presents through its own provider, not a window-system surface".into()),
        }
        .map_err(|e| format!("{call} key={:?} token={}: {e:?}", (sid, id), native.token))?;
        if log_enabled() {
            eprintln!("[hal-linux] {call} key={:?} succeeded: surface={handle:?}", (sid, id));
        }
        Ok(NativeSurface {
            handle,
            keep: native,
        })
    }
}

pub fn expect_main_loop() {
    Shared::global().announced.store(true, Ordering::Release);
}
pub fn screens() -> Vec<crate::ScreenInfo> {
    lock(&Shared::global().screens).clone()
}
pub fn surface_source(shared: &'static Shared) -> Box<dyn VkSurfaceSource> {
    match lock(&shared.run).ready.as_ref() {
        Some(Ok(Protocol::Direct)) => Box::new(direct::Source::new()),
        Some(Ok(Protocol::Wm)) => Box::new(wm::Source::new()),
        _ => Box::new(SurfaceSource(shared)),
    }
}
/// How the default selection ranks a backend that opened, lower first: hardware Vulkan, hardware
/// GL, software Vulkan, software GL (a CPU Vulkan such as lavapipe is slower than a GPU's GL).
pub fn preference(vulkan: bool, software: bool) -> u8 {
    (software as u8) * 2 + (!vulkan) as u8
}

/// The preferred of the backends that opened. GL is tried only when Vulkan gave no hardware device.
fn prefer(vulkan: Result<super::vulkan::Vulkan, String>, gl: impl FnOnce() -> Result<super::gl::GlBackend, String>) -> Result<Box<dyn super::Backend>, String> {
    use super::Backend;
    let vulkan = match vulkan {
        Ok(v) if !v.software() => return Ok(Box::new(v)),
        other => other,
    };
    let gl = gl();
    let mut opened: Vec<(u8, Box<dyn Backend>)> = Vec::new();
    let vk_error = match vulkan {
        Ok(v) => { opened.push((preference(true, true), Box::new(v))); None }
        Err(e) => Some(e),
    };
    let gl_error = match gl {
        Ok(g) => { opened.push((preference(false, g.software()), Box::new(g))); None }
        Err(e) => Some(e),
    };
    opened.sort_by_key(|(rank, _)| *rank);
    let errors = format!("Vulkan: {}; GLES: {}", vk_error.as_deref().unwrap_or("a CPU device"), gl_error.as_deref().unwrap_or("opened"));
    match opened.into_iter().next() {
        Some((_, chosen)) => {
            eprintln!("[hal] no hardware Vulkan device ({errors}); rendering with {}", if chosen.info().backend == backend_id::GL { "GLES" } else { "the CPU Vulkan device" });
            Ok(chosen)
        }
        None => Err(errors),
    }
}

/// Linux desktop's backend when none is forced, both on the one shell start; see [`preference`].
pub fn open(flags: u32, wake: Arc<dyn Fn() + Send + Sync>, sessions: super::SessionNotify, notify: Arc<dyn Fn() + Send + Sync>) -> Result<Box<dyn super::Backend>, String> {
    use super::{gl::GlBackend, vulkan::Vulkan};
    if flags & open_flags::OFFSCREEN_WINDOWS != 0 {
        return prefer(Vulkan::new(flags, sessions, notify.clone()), || GlBackend::new(flags, wake, notify));
    }
    let shell = Shared::global();
    shell.start(flags, notify)?;
    let opened = prefer(Vulkan::on_shell(flags, sessions, shell), || GlBackend::on_shell(wake, shell));
    if opened.is_err() { stop_main_loop(); }
    opened
}

#[cfg(test)]
mod tests {
    use super::preference;

    #[test]
    fn hardware_gl_comes_before_a_cpu_vulkan_device() {
        let mut order = [(false, true), (true, true), (false, false), (true, false)];
        order.sort_by_key(|(vulkan, software)| preference(*vulkan, *software));
        // hardware Vulkan > hardware GL > software Vulkan > software GL
        assert_eq!(order, [(true, false), (false, false), (true, true), (false, true)]);
    }
}

pub fn stop_main_loop() {
    let s = Shared::global();
    lock(&s.run).stopped = true;
    if log_enabled() {
        eprintln!("[hal-linux] stop_main_loop requested; waking the shell");
    }
    s.ready.notify_all();
    s.signal();
}

/// Whether the window system is dark: 0 unknown, 1 light, 2 dark. The WM-hosted shell sets it
/// from the WM's `HostedAppearance`; Wayland and X11 have no appearance of their own here.
static HOST_DARK: AtomicU8 = AtomicU8::new(0);

pub fn host_dark() -> Option<bool> {
    match HOST_DARK.load(Ordering::Acquire) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

fn set_host_dark(dark: bool) {
    HOST_DARK.store(if dark { 2 } else { 1 }, Ordering::Release);
}

fn connect(shared: &'static Shared) -> Result<Box<dyn Shell>, String> {
    use crate::wm_socket::{self, Backend};
    let forced = wm_socket::forced(std::env::args(), std::env::var("MAKEPAD").ok().as_deref());
    let window_manager = shared.flags.load(Ordering::Acquire) & open_flags::WINDOW_MANAGER != 0;
    let wayland_display = std::env::var_os("WAYLAND_DISPLAY").filter(|s| !s.is_empty());
    let wayland_socket = std::env::var_os("WAYLAND_SOCKET").filter(|s| !s.is_empty());
    let display = std::env::var_os("DISPLAY").filter(|s| !s.is_empty());
    let (backend, reason) = wm_socket::choose(forced, window_manager, wayland_display.is_some() || wayland_socket.is_some(),
        || wm_socket::discovered(makepad_studio_protocol::WIRE).clone());
    if log_enabled() || matches!(backend, Backend::Wm | Backend::Direct) {
        eprintln!("[hal-linux] backend={backend:?} reason={reason}; WAYLAND_DISPLAY={wayland_display:?} WAYLAND_SOCKET={wayland_socket:?} DISPLAY={display:?}; XDG_RUNTIME_DIR={:?}",
            std::env::var_os("XDG_RUNTIME_DIR"));
    }
    match backend {
        Backend::Direct => direct::App::connect(shared).map(|app| Box::new(app) as Box<dyn Shell>),
        Backend::Wm => wm::App::connect(shared).map(|app| Box::new(app) as Box<dyn Shell>),
        Backend::Wayland => match wayland::App::connect(shared) {
            Ok(app) => Ok(Box::new(app)),
            Err(e) => {
                if forced.is_some() || display.is_none() {
                    return Err(format!("Wayland connection failed: {e}; no X11 fallback ({})",
                        if forced.is_some() { "Wayland explicitly selected" } else { "DISPLAY unset/empty" }));
                }
                eprintln!("[hal-linux] Wayland connection failed: {e}; backend=x11 fallback because DISPLAY is set");
                x11::App::connect(shared)
                    .map(|app| Box::new(app) as Box<dyn Shell>)
                    .map_err(|x| format!("Wayland failed: {e}; X11 fallback failed: {x}"))
            }
        },
        Backend::X11 => x11::App::connect(shared).map(|app| Box::new(app) as Box<dyn Shell>),
    }
}

pub fn run_main_loop() {
    let s = Shared::global();
    if log_enabled() {
        eprintln!("[hal-linux] main loop entered; waiting for shell startup or app exit");
    }
    // Startup waits on the wake fd, not a channel/Condvar or the HAL's state mutex.
    loop {
        let run = lock(&s.run);
        if run.stopped {
            if log_enabled() {
                eprintln!("[hal-linux] main loop exited before shell startup");
            }
            return;
        }
        if run.wanted {
            break;
        }
        drop(run);
        if let Err(e) = io::wait(s.reader.as_raw_fd(), &mut [], None) {
            eprintln!("[hal-linux] startup wake wait: {e}");
            lock(&s.run).ready = Some(Err(format!("startup wake wait: {e}")));
            stop_main_loop();
            return;
        }
        s.take();
    }
    let result = connect(s);
    lock(&s.run).ready = Some(result.as_ref().map(|a| a.protocol()).map_err(Clone::clone));
    s.ready.notify_all();
    let mut app = match result {
        Ok(app) => app,
        Err(e) => {
            eprintln!("[hal-linux] shell startup failed: {e}");
            return;
        }
    };
    if log_enabled() {
        eprintln!("[hal-linux] {:?} connection ready; serving window commands", app.protocol());
    }
    // MAKEPAD_HAL_MAIN_LAG: the loop's own wake fd is signalled every 4 ms, so an idle loop comes
    // around at that rate and every longer gap is time the window system waited for this thread
    let pinger = super::main_lag::wanted().then(|| {
        super::main_lag::start();
        std::thread::Builder::new().name("hal-main-lag".into()).spawn(move || {
            while !lock(&s.run).stopped {
                std::thread::sleep(super::main_lag::TICK);
                s.signal();
            }
        })
    });
    let mut stopping = false;
    let mut disconnected = false;
    loop {
        super::main_lag::tick();
        for op in s.take() {
            app.command(op);
        }
        if lock(&s.run).stopped && !stopping {
            stopping = true;
            // the pings end with the run: teardown waits are not the window system's
            super::main_lag::finish();
            if log_enabled() {
                eprintln!("[hal-linux] main loop stopping; closing windows and draining native leases");
            }
            app.shutdown();
        }
        if stopping && app.empty() {
            break;
        }
        if disconnected {
            if let Err(e) = io::wait(s.reader.as_raw_fd(), &mut [], None) {
                eprintln!("[hal-linux] teardown wake: {e}");
            }
            continue;
        }
        let dispatched = app.dispatch();
        // Dispatch may trigger app exit. Observe it before entering another display wait.
        if dispatched.is_ok() && !stopping && lock(&s.run).stopped {
            continue;
        }
        if let Err(e) = dispatched.and_then(|_| app.wait(s.reader.as_raw_fd())) {
            eprintln!("[hal-linux] display connection: {e}");
            // Stop sessions before native connection teardown. A dead display is not a new
            // offscreen surface; queued Vulkan references still own their native leases.
            let keys: Vec<_> = lock(&s.surfaces).keys().copied().collect();
            for key in keys {
                s.push(key, |q, _| {
                    q.lifecycle(crate::types::lifecycle::QUIT_REQUESTED)
                });
            }
            stop_main_loop();
            disconnected = true;
        }
    }
    super::main_lag::finish();
    if let Some(Ok(pinger)) = pinger {
        let _ = pinger.join();
    }
    if log_enabled() {
        eprintln!("[hal-linux] main loop exited; native windows retired");
    }
}
