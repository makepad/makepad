use super::{jni, surface::{AndroidSurfaces, Registry}};
use crate::{backend::{exec::{Gpu, OsCmd}, vulkan::{NativeImages, Shell, Vulkan}, Backend, Pixels}, buffers::{CmdView, EventQueue}, session::{World, HOST}, *};
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
    pub kernel_version: String,
    /// the system's dark mode (at start, kept current by `onNightModeChanged`)
    pub night_mode: bool,
    /// `Display.getRotation()` in quarter turns (camera orientation; the surface itself arrives upright)
    pub rotation: u32,
}
impl Default for AndroidParams {
    fn default() -> Self { Self { cache_path: String::new(), data_path: String::new(), density: 1., emulator: false, android_version: String::new(), build_number: String::new(), kernel_version: String::new(), night_mode: false, rotation: 0 } }
}

pub(super) struct State {
    pub recipient: Option<(u32, Id)>,
    pub events: Vec<(Option<u32>, EventQueue)>,
    pub ime_session: Option<u32>,
    pub params: AndroidParams,
    pub refresh: f32,
    pub insets: [f32; 4],
    pub pending: HashSet<(u32, Id)>,
    pub active: bool,
    pub focused: bool,
    pub physical_keyboard: Option<bool>,
    pub ime: Option<(i32, bool)>,
    pub wake: Option<Arc<dyn Fn() + Send + Sync>>,
}
pub(super) struct Shared {
    pub registry: Mutex<Registry>,
    pub state: Mutex<State>,
}
impl Shared {
    pub fn global() -> &'static Self {
        static VALUE: OnceLock<Shared> = OnceLock::new();
        VALUE.get_or_init(|| Self { registry: Mutex::new(Registry::default()), state: Mutex::new(State {
            recipient: None, ime_session: None, events: Vec::new(), params: AndroidParams::default(), refresh: 60., insets: [0.; 4], pending: HashSet::new(), active: false, focused: false, physical_keyboard: None, ime: None, wake: None,
        }) })
    }
    pub fn dpi(&self) -> f32 { lock(&self.state).params.density }
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
        let (dpi, refresh, insets) = { let s = lock(&self.state); (s.params.density, s.refresh, s.insets) };
        self.emit(clock_ns(), |q, id| {
            q.display(id, dpi, refresh);
            q.geom(id, 0., 0., size[0] as f64 / dpi as f64, size[1] as f64 / dpi as f64, dpi, window_state::RESTORE);
            // the Activity reports the insets in logical points already (MakepadSystemInsets divides by the density)
            q.safe_area(id, insets[0] as f64, insets[1] as f64, insets[2] as f64, insets[3] as f64);
        });
    }
    pub fn frame(&self, time: u64) {
        let (pending, wake, period) = {
            let mut s = lock(&self.state);
            if !s.active { return; }
            (std::mem::take(&mut s.pending), s.wake.clone(), (1_000_000_000. / s.refresh as f64) as u64)
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
            OsCmd::WindowGeom { id, .. } if lock(&Shared::global().state).recipient == Some((sid, id)) => {
                // The Activity determines its geometry; application resize requests cannot
                // change the canonical host window independently of the Android surface.
                Shared::global().display(); true
            }
            OsCmd::FrameRequest { id } => { lock(&Shared::global().state).pending.insert((sid, id)); jni::request_frame(); true }
            cmd @ (OsCmd::ImeShow { .. } | OsCmd::ImeHide { .. } | OsCmd::ImeSync { .. }) => {
                if lock(&Shared::global().state).ime_session != Some(sid) { return false; }
                jni::ime_command(cmd)
            },
            OsCmd::BackUnhandled => jni::back_unhandled(),
            _ => false,
        }
    }
}

pub struct AndroidBackend { core: Vulkan, host: Option<Id>, offscreen: bool }
impl AndroidBackend {
    /// `notify` makes every pump look (the Activity's events and frame callbacks); `sessions` the
    /// session a GPU result is for.
    pub fn new(flags: u32, sessions: crate::backend::SessionNotify, notify: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        let mut core = Vulkan::with_sources(flags, sessions, Box::new(AndroidSurfaces), Box::new(NativeImages::default()))?;
        let offscreen = flags & open_flags::OFFSCREEN_WINDOWS != 0;
        if !offscreen { core.set_shell(Box::new(AndroidShell)); }
        lock(&Shared::global().state).wake = Some(notify);
        Ok(Self { core, host: None, offscreen })
    }
    pub fn params() -> AndroidParams { lock(&Shared::global().state).params.clone() }
    fn ensure_host(&mut self, world: &mut World) {
        if self.offscreen || self.host.is_some() { return; }
        let id = world.host_window([0.; 4]);
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
                if let Some(HalEvent::Geom { window, x, y, w, h, dpi, state }) = e.decode(payload) {
                    world.window_geom(sid, window, [x as f32, y as f32, w as f32, h as f32], dpi, state);
                }
                world.to_session(sid, |q| q.push_with_payload(e, payload));
            });
        }
    }
}
impl Backend for AndroidBackend {
    fn info(&self) -> HalInfo { HalInfo { dpi: Shared::global().dpi(), ..self.core.info() } }
    fn submit(&mut self, world: &mut World, sid: u32, view: &CmdView) {
        self.ensure_host(world);
        self.drain(world);
        let owner = lock(&Shared::global().state).recipient.map(|(sid, _)| sid);
        let allowed = owner == Some(sid) || world.session(sid).is_some_and(|s| s.focused && s.parent != 0 && Some(s.parent) == owner);
        lock(&Shared::global().state).ime_session = allowed.then_some(sid);
        self.core.submit(world, sid, view);
        self.drain(world);
    }
    fn step(&mut self, world: &mut World, sid: u32) { self.ensure_host(world); self.drain(world); self.core.step(world, sid); }
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
    }
}
