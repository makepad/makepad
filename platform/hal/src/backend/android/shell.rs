use super::{jni, surface::{AndroidEglSurfaces, AndroidSurfaces, Registry}};
use crate::{backend::{exec::{self, Gpu, OsCmd}, gl::GlBackend, vulkan::Vulkan, Backend, Pixels, Shell}, buffers::{CmdView, EventQueue}, session::{World, HOST}, *};
use std::{collections::HashSet, sync::{Arc, Mutex, MutexGuard, OnceLock}};

pub(super) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }

#[derive(Clone, Debug)]
pub struct AndroidParams {
    pub cache_path: String,
    pub data_path: String,
    pub density: f32,
    pub emulator: bool,
    pub android_version: String,
    pub build_number: String,
    /// the package's version and last update time, which every new or reinstalled APK changes
    pub app_build: String,
    /// the system's dark mode (at start, kept current by `onNightModeChanged`)
    pub night_mode: bool,
    /// `Display.getRotation()` in quarter turns (camera orientation; the surface itself arrives upright)
    pub rotation: u32,
}
impl Default for AndroidParams {
    fn default() -> Self { Self { cache_path: String::new(), data_path: String::new(), density: 1., emulator: false, android_version: String::new(), build_number: String::new(), app_build: String::new(), night_mode: false, rotation: 0 } }
}

pub(super) struct State {
    pub recipient: Option<(u32, Id)>,
    pub events: Vec<(Option<u32>, EventQueue)>,
    pub ime_session: Option<u32>,
    pub params: AndroidParams,
    /// the display's frame rate for this app, which each `FrameReady` predicts the next vsync from
    pub refresh: f32,
    pub insets: [f32; 4],
    pub pending: HashSet<(u32, Id)>,
    pub active: bool,
    pub focused: bool,
    pub physical_keyboard: Option<bool>,
    pub ime: Option<(i32, bool)>,
    /// The session that asked for fullscreen: the system bars stay hidden, also after a resume,
    /// until it restores its window or closes.
    pub fullscreen: Option<u32>,
    pub wake: Option<Arc<dyn Fn() + Send + Sync>>,
    /// An immersive (OpenXR) build: the host window is the XR panel, this many pixels at this
    /// density, whatever the Activity's own surface is.
    pub xr: Option<([u32; 2], f32)>,
}
pub(super) struct Shared {
    pub registry: Mutex<Registry>,
    pub state: Mutex<State>,
    /// Lets go of the Activity's window before `surfaceDestroyed` returns (the GLES core's
    /// EGL surfaces; Vulkan's swapchains keep a lease and fail their next present instead).
    pub release_window: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}
impl Shared {
    pub fn global() -> &'static Self {
        static VALUE: OnceLock<Shared> = OnceLock::new();
        VALUE.get_or_init(|| Self { registry: Mutex::new(Registry::default()), state: Mutex::new(State {
            recipient: None, ime_session: None, events: Vec::new(), params: AndroidParams::default(), refresh: 60., insets: [0.; 4], pending: HashSet::new(), active: false, focused: false, physical_keyboard: None, ime: None, fullscreen: None, wake: None, xr: None,
        }), release_window: Mutex::new(None) })
    }
    pub fn dpi(&self) -> f32 { let s = lock(&self.state); s.xr.map_or(s.params.density, |(_, dpi)| dpi) }
    pub fn emit(&self, time: u64, f: impl FnOnce(&mut EventQueue, Id)) {
        let mut q = EventQueue::new();
        q.set_time(time);
        let recipient = lock(&self.state).recipient;
        let id = recipient.map_or(Id::default(), |(_, id)| id);
        f(&mut q, id);
        let wake = {
            let mut s = lock(&self.state);
            s.events.push((recipient.map(|(sid, _)| sid), q));
            s.wake.clone()
        };
        if let Some(wake) = wake { wake(); }
    }
    pub fn display(&self) {
        let size = lock(&self.registry).state.size;
        let (size, dpi, refresh, insets) = { let s = lock(&self.state); match s.xr {
            Some((size, dpi)) => (size, dpi, s.refresh, [0.; 4]),
            None => (size, s.params.density, s.refresh, s.insets),
        } };
        let state = if lock(&self.state).fullscreen.is_some() { window_state::FULLSCREEN } else { window_state::RESTORE };
        self.emit(clock_ns(), |q, id| {
            q.display(id, dpi, refresh);
            q.geom(id, 0., 0., size[0] as f64 / dpi as f64, size[1] as f64 / dpi as f64, dpi, state);
            // the Activity reports the insets in logical points already (MakepadSystemInsets divides by the density)
            q.safe_area(id, insets[0] as f64, insets[1] as f64, insets[2] as f64, insets[3] as f64);
        });
    }
    /// Sends `f`'s events to window `id` of session `sid`.
    pub fn emit_to(&self, sid: u32, id: Id, time: u64, f: impl FnOnce(&mut EventQueue, Id)) {
        let mut q = EventQueue::new();
        q.set_time(time);
        f(&mut q, id);
        let wake = { let mut s = lock(&self.state); s.events.push((Some(sid), q)); s.wake.clone() };
        if let Some(wake) = wake { wake(); }
    }
    /// FrameReady for the windows that asked; an XR window stays pending for the XR thread.
    pub fn frame(&self, time: u64) {
        let xr_window = lock(&self.registry).xr_window;
        let (pending, wake, period) = {
            let mut s = lock(&self.state);
            if !s.active { return; }
            let pending = std::mem::take(&mut s.pending);
            let (xr, pending): (HashSet<_>, HashSet<_>) = pending.into_iter().partition(|w| Some(*w) == xr_window);
            s.pending.extend(xr);
            (pending, s.wake.clone(), (1_000_000_000. / s.refresh as f64) as u64)
        };
        let mut events = Vec::with_capacity(pending.len());
        for (sid, id) in pending {
            let mut q = EventQueue::new(); q.set_time(time); q.frame_ready(id, time.saturating_add(period));
            events.push((Some(sid), q));
        }
        let any = !events.is_empty();
        lock(&self.state).events.extend(events);
        if any { if let Some(wake) = wake { wake(); } }
    }
}

/// The backend the app picked in its Cargo.toml (`[package.metadata.makepad.android] hal_backend`,
/// `gl` or `vulkan`), which cargo-makepad writes into the manifest. `vulkan` keeps the default.
static APP_HAL_BACKEND: OnceLock<String> = OnceLock::new();

pub(super) unsafe extern "system" fn app_hal_backend(env: *mut makepad_jni_sys::JNIEnv, _: makepad_jni_sys::jclass, backend: makepad_jni_sys::jstring) {
    let _ = APP_HAL_BACKEND.set(makepad_jni_sys::registration::string(env, backend));
}

struct AndroidShell;
impl Shell for AndroidShell {
    fn auto_adopt(&self) -> bool { true }
    fn lent(&mut self, sid: u32, alias: Id, host: Id, attach: bool) {
        let shared = Shared::global();
        if lock(&shared.registry).host != Some(host) { return; }
        lock(&shared.state).recipient = Some(if attach { (sid, alias) } else { (HOST, host) });
        shared.display();
        let (active, focused, keyboard, ime, dpi) = { let s = lock(&shared.state); (s.active, s.focused, s.physical_keyboard, s.ime, s.params.density) };
        shared.emit(clock_ns(), |q, id| {
            q.focus(id, focused);
            if let Some(connected) = keyboard { q.physical_keyboard(id, connected); }
            if let Some((height, open)) = ime { q.ime_resize(id, height as f64 / dpi as f64, open); }
            q.lifecycle(if active { lifecycle::RESUME } else { lifecycle::PAUSE });
        });
    }
    fn command(&mut self, sid: u32, cmd: OsCmd) -> bool {
        match cmd {
            // The executor immediately answers unsupported requests.
            OsCmd::PrintPdf { .. } => false,
            OsCmd::WindowGeom { id, .. } if lock(&Shared::global().state).recipient == Some((sid, id)) => {
                // The Activity determines its geometry; application resize requests cannot
                // change the canonical host window independently of the Android surface.
                Shared::global().display(); true
            }
            OsCmd::WindowState { id, state: state @ (window_state::FULLSCREEN | window_state::RESTORE) } if lock(&Shared::global().state).recipient == Some((sid, id)) => {
                let fullscreen = state == window_state::FULLSCREEN;
                lock(&Shared::global().state).fullscreen = fullscreen.then_some(sid);
                jni::set_full_screen(fullscreen);
                Shared::global().display(); true
            }
            OsCmd::WindowCreate { id, kind, .. } if kind & 0xff == window_kind::XR => {
                // XR frames never reach the Activity's surface, so no present would lift its cover.
                jni::hide_surface_cover();
                super::xr::create_window(sid, id)
            }
            OsCmd::WindowClose { id } if lock(&Shared::global().registry).xr_window == Some((sid, id)) => {
                let mut r = lock(&Shared::global().registry);
                r.xr_window = None; r.xr_see_through = false; true
            }
            OsCmd::WindowVisuals { id, visuals, .. } if lock(&Shared::global().registry).xr_window == Some((sid, id)) => {
                lock(&Shared::global().registry).xr_see_through = visuals & 0xff != 0; true
            }
            OsCmd::XrFrameEyes { id, eyes, focus_m } if lock(&Shared::global().registry).xr_window == Some((sid, id)) => { super::xr::frame_eyes(eyes, focus_m); true }
            OsCmd::XrPoseLead { id, lead_ns } if lock(&Shared::global().registry).xr_window == Some((sid, id)) => { super::xr::pose_lead(lead_ns); true }
            // (the display's, whichever window asks: it may come before the XR window is registered)
            OsCmd::XrRefreshRate { millihertz, .. } => { super::xr::refresh_rate(millihertz); true }
            OsCmd::XrHaptic { hand, amplitude, duration_us, .. } => { super::xr::haptic(hand, amplitude, duration_us); true }
            OsCmd::FrameRequest { id } => { lock(&Shared::global().state).pending.insert((sid, id)); jni::request_frame(); true }
            cmd @ (OsCmd::ImeShow { .. } | OsCmd::ImeHide { .. } | OsCmd::ImeSync { .. }) => {
                if lock(&Shared::global().state).ime_session != Some(sid) { return false; }
                jni::ime_command(cmd)
            },
            OsCmd::ClipSet { data } => jni::copy_to_clipboard(&data),
            OsCmd::ClipRead { req, .. } => {
                let Some(content) = jni::read_clipboard() else { return false; };
                let content = content.pack();
                Shared::global().emit_to(sid, Id::NONE, clock_ns(), |q, _| {
                    q.payload_size(content.len() as u32);
                    q.clipboard(req, true, &content);
                });
                true
            }
            // they sit at the asking window's points, so only the Activity's own window places them (a hide always goes)
            OsCmd::SelectionHandles { id, op: action, start, end } if action == selection_handles::HIDE || lock(&Shared::global().state).recipient == Some((sid, id)) => jni::selection_handles(action, start, end),
            OsCmd::ClipboardActions { id, show, area, selection } if !show || lock(&Shared::global().state).recipient == Some((sid, id)) => jni::clipboard_actions(show, area, selection),
            OsCmd::BackUnhandled => jni::back_unhandled(),
            OsCmd::NativeAppearance { value } => jni::set_native_appearance(value),
            OsCmd::MobileUi { op: mobile_ui::STATUS_BAR_DARK_ICONS, value } => jni::set_system_bar_appearance(value != 0),
            OsCmd::MobileUi { op: mobile_ui::ORIENTATION, value } => jni::set_screen_orientation(value),
            OsCmd::MobileUi { op: mobile_ui::KEEP_SCREEN_ON, value } => jni::set_keep_screen_on(value != 0),
            OsCmd::MobileUi { op: mobile_ui::APP_BACKGROUND, value } => jni::set_app_background(value),
            OsCmd::MobileUi { op: mobile_ui::SHADER_COMPILE_SCREEN, .. } => jni::show_shader_compile_screen(),
            _ => false,
        }
    }
}

/// Vulkan where the device has Vulkan 1.1 and its driver takes our pipelines, else GLES 3.
/// `MAKEPAD_HAL_BACKEND` or `debug.makepad.hal_backend` (`gl`, `vulkan`) picks one for good.
pub fn open(backend: u32, flags: u32, sessions: crate::backend::SessionNotify, wake: Arc<dyn Fn() + Send + Sync>, notify: Arc<dyn Fn() + Send + Sync>) -> Result<Box<dyn Backend>, String> {
    let backend = match (backend, super::xr::system_property("debug.makepad.hal_backend").as_deref()) {
        (backend_id::AUTO, Some("gl")) => backend_id::GL,
        (backend_id::AUTO, Some("vulkan")) => backend_id::VULKAN,
        (backend_id::AUTO, _) if APP_HAL_BACKEND.get().is_some_and(|b| b == "gl") => backend_id::GL,
        (backend, _) => backend,
    };
    if backend == backend_id::AUTO && super::gles_fallback::remembered() {
        match AndroidBackend::with_gl(flags, wake.clone(), notify.clone()) {
            Ok(gl) => {
                eprintln!("[hal-android] this Vulkan driver refused our pipelines before, drawing with GLES");
                return Ok(Box::new(gl));
            }
            Err(e) => eprintln!("[hal-android] this Vulkan driver refused our pipelines before, but GLES failed ({e})"),
        }
    }
    if backend != backend_id::GL {
        let fallback = (backend == backend_id::AUTO).then(|| wake.clone());
        match AndroidBackend::new(flags, sessions, notify.clone(), fallback) {
            Ok(vulkan) => return Ok(Box::new(vulkan)),
            Err(e) if backend == backend_id::VULKAN => return Err(e),
            Err(e) => eprintln!("[hal-android] no Vulkan 1.1 ({e}), drawing with GLES"),
        }
    }
    Ok(Box::new(AndroidBackend::with_gl(flags, wake, notify)?))
}

/// What makes the GLES core if the Vulkan driver refuses one of our pipelines.
struct GlesFallback { flags: u32, wake: Arc<dyn Fn() + Send + Sync>, notify: Arc<dyn Fn() + Send + Sync>, driver: [u32; 3] }

pub struct AndroidBackend { core: Box<dyn Backend>, host: Option<Id>, offscreen: bool, gles_fallback: Option<GlesFallback> }
impl AndroidBackend {
    /// `notify` makes every pump look (the Activity's events and frame callbacks); `sessions` the
    /// session a GPU result is for. With `wake`, a driver that refuses a pipeline gets GLES instead.
    pub fn new(flags: u32, sessions: crate::backend::SessionNotify, notify: Arc<dyn Fn() + Send + Sync>, wake: Option<Arc<dyn Fn() + Send + Sync>>) -> Result<Self, String> {
        let offscreen = flags & open_flags::OFFSCREEN_WINDOWS != 0;
        // A Quest build carries the OpenXR loader: the host window is shown in an XR session.
        let (source, xr): (Box<dyn crate::backend::vulkan::VkSurfaceSource>, bool) = match (!offscreen).then(super::xr::XrSurfaces::try_new).flatten() {
            Some(xr) => (Box::new(xr?), true),
            None => (Box::new(AndroidSurfaces), false),
        };
        let mut core = Vulkan::with_sources(flags, sessions, source, Box::new(crate::backend::vulkan::shared_images()))?;
        if !offscreen { core.set_shell(Box::new(AndroidShell)); }
        // an XR session needs Vulkan, so there's nothing to fall back to
        let gles_fallback = wake.filter(|_| !xr).map(|wake| GlesFallback { flags, wake, notify: notify.clone(), driver: core.driver() });
        lock(&Shared::global().state).wake = Some(notify);
        Ok(Self { core: Box::new(core), host: None, offscreen, gles_fallback })
    }

    /// GLES 3 through EGL on the Activity's `ANativeWindow`, for devices without Vulkan 1.1.
    pub fn with_gl(flags: u32, wake: Arc<dyn Fn() + Send + Sync>, notify: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        let offscreen = flags & open_flags::OFFSCREEN_WINDOWS != 0;
        let shell: Option<Box<dyn Shell>> = if offscreen { None } else { Some(Box::new(AndroidShell)) };
        let core = GlBackend::with_source(wake, Box::new(AndroidEglSurfaces { offscreen }), shell)?;
        if !offscreen { *lock(&Shared::global().release_window) = Some(core.window_release()); }
        lock(&Shared::global().state).wake = Some(notify);
        Ok(Self { core: Box::new(core), host: None, offscreen, gles_fallback: None })
    }

    /// The Vulkan driver refused one of our pipelines: we move every session to a GLES core, much
    /// like after a device loss, and note it for this driver and system build.
    fn fall_back_if_refused(&mut self, world: &mut World) {
        if self.gles_fallback.is_none() { return; }
        let Some(why) = crate::backend::vulkan::refused_pipeline() else { return };
        let Some(fallback) = self.gles_fallback.take() else { return };
        let gl = match Self::with_gl(fallback.flags, fallback.wake, fallback.notify) {
            Ok(gl) => gl,
            Err(e) => return eprintln!("[hal-android] the Vulkan driver refused a pipeline ({why}), and GLES failed ({e})"),
        };
        eprintln!("[hal-android] the Vulkan driver refused a pipeline ({why}), drawing with GLES");
        super::gles_fallback::remember(fallback.driver, why);
        self.core.retire(world);
        self.core = gl.core;
        let info = self.info();
        if let Some(gpu) = self.core.host_gpu() { exec::backend_changed(gpu, world, info); }
    }
    pub fn params() -> AndroidParams { lock(&Shared::global().state).params.clone() }
    fn ensure_host(&mut self, world: &mut World) {
        if self.offscreen || self.host.is_some() { return; }
        let id = world.host_window([0.; 4], true);
        self.host = Some(id);
        let shared = Shared::global();
        lock(&shared.registry).host = Some(id);
        lock(&shared.state).recipient = Some((HOST, id));
        shared.display();
    }
    fn drain(&mut self, world: &mut World) {
        let events = std::mem::take(&mut lock(&Shared::global().state).events);
        for (sid, mut q) in events {
            let recipient = lock(&Shared::global().state).recipient;
            let remap = sid.is_none();
            let sid = sid.unwrap_or_else(|| recipient.map_or(HOST, |(sid, _)| sid));
            // Input is addressed to the Activity's borrower, including before its first frame.
            // Child domains receive input only through their parent's Forward commands.
            q.drain_with(|mut e, payload| {
                if remap { e.window = recipient.map_or(Id::default(), |(_, id)| id); }
                match e.decode(payload) {
                    Some(HalEvent::Geom { window, x, y, w, h, dpi, state }) => world.window_geom(sid, window, [x as f32, y as f32, w as f32, h as f32], dpi, state),
                    Some(HalEvent::Lifecycle { kind }) => world.note_lifecycle(kind),
                    _ => {}
                }
                world.to_session(sid, |q| q.push_with_payload(e, payload));
            });
        }
    }
}
impl Backend for AndroidBackend {
    fn info(&self) -> HalInfo {
        let info = self.core.info();
        // the Choreographer (or an XR session's frame loop) answers frame requests at each vsync
        let display_clock = if self.offscreen { 0 } else { info_flags::DISPLAY_CLOCK };
        HalInfo { dpi: Shared::global().dpi(), flags: info.flags | display_clock | if super::xr::active() { info_flags::XR } else { 0 }, ..info }
    }
    fn submit(&mut self, world: &mut World, sid: u32, view: &CmdView) {
        self.ensure_host(world);
        self.drain(world);
        let owner = lock(&Shared::global().state).recipient.map(|(sid, _)| sid);
        let allowed = owner == Some(sid) || world.session(sid).is_some_and(|s| s.focused && s.parent != 0 && Some(s.parent) == owner);
        lock(&Shared::global().state).ime_session = allowed.then_some(sid);
        self.core.submit(world, sid, view);
        self.fall_back_if_refused(world);
        self.drain(world);
    }
    fn step(&mut self, world: &mut World, sid: u32) { self.ensure_host(world); self.drain(world); self.core.step(world, sid); self.fall_back_if_refused(world); }
    fn next_due(&self, world: &World, sid: u32) -> Option<u64> { self.core.next_due(world, sid) }
    fn window_pixels(&mut self, world: &mut World, sid: u32, window: Option<Id>) -> Option<Pixels> { self.core.window_pixels(world, sid, window) }
    fn host_gpu(&mut self) -> Option<&mut dyn Gpu> { self.core.host_gpu() }
    fn session_opened(&mut self, world: &mut World, sid: u32) { self.ensure_host(world); self.core.session_opened(world, sid); }
    fn session_closed(&mut self, world: &mut World, sid: u32) {
        self.core.session_closed(world, sid);
        let shared = Shared::global();
        let mut state = lock(&shared.state);
        state.events.retain(|(s, _)| *s != Some(sid));
        state.pending.retain(|(s, _)| *s != sid);
        if state.recipient.is_some_and(|(s, _)| s == sid) { state.recipient = self.host.map(|id| (HOST, id)); }
        let restore = state.fullscreen == Some(sid);
        if restore { state.fullscreen = None; }
        drop(state);
        // its fullscreen ends with it, so the next app gets the system bars back
        if restore { jni::set_full_screen(false); shared.display(); }
        let mut registry = lock(&shared.registry);
        if registry.xr_window.is_some_and(|(s, _)| s == sid) { registry.xr_window = None; }
    }
    fn before_exit(&mut self) { self.core.before_exit(); }
}
