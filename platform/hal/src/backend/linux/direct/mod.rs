//! DRM/KMS direct-display shell. One HAL window is the native wide desktop.
//! evdev/VT stay on the shell thread; display WSI and scanout retire on one persistent worker.
mod direct_event;
mod drm_sys;
mod dma_buf;
mod gpu_bridge;
mod hotplug;
mod kms;
mod preferences;
mod raw_input;
mod render;
mod sys;
mod terminal;
mod touchpad;

use super::{io, lock, Key, Op, OsCmd, Protocol, Shared, Shell};
use crate::{backend::vulkan::{surface::{FramePresenter, SurfaceState, VkSurfaceSource}, Device, Image},
    buffers::EventQueue, events::event_tag, types::*, Id};
use crate::direct::{DisplayCommand, LinuxDisplaySnapshot};
use makepad_key_code::KeyCode;
use makepad_vulkan::vk;
use std::{collections::{HashMap, VecDeque}, ffi::CStr, io::{Read, Write}, os::unix::net::UnixStream,
    sync::{Arc, OnceLock, atomic::{AtomicBool, AtomicU64, Ordering}}, time::{Duration, Instant}};

type Presented = Box<dyn FnOnce(u64) + Send>;
struct Frame { key: Key, image: Arc<Image>, presented: Option<Presented>, generation: u64, arrived: u64, taken: u64 }
enum Command {
    Window(Option<Key>),
    Request,
    Frame(Frame),
    Pause,
    Resume,
    Stop,
    Mode(String, Option<String>),
    Order(Vec<String>),
    Primary(String),
    Arrangement(crate::direct::DisplayArrangement),
    Pacing(crate::direct::DisplayPacing),
    /// The HAL replaced its lost GPU device: the presenter renders with this one from now on.
    Device(Arc<Device>),
}

#[derive(Default)]
struct Snapshot {
    generation: u64,
    displays: LinuxDisplaySnapshot,
    size: [u32; 2],
    timings: HashMap<String, (u64, f64)>,
}

struct State {
    shared: &'static Shared,
    ops: std::sync::Mutex<VecDeque<Command>>,
    writer: UnixStream,
    reader: UnixStream,
    snapshot: std::sync::Mutex<Snapshot>,
    window: std::sync::Mutex<Option<Key>>,
    generation: AtomicU64,
    started: AtomicBool,
    stopped: AtomicBool,
    released: AtomicBool,
    ready: AtomicU64,
    timings_changed: AtomicBool,
    /// At least one display is driven (DRM master taken, mode set): input may open.
    acquired: AtomicBool,
    dpi: f64,
}
static STATE: OnceLock<Arc<State>> = OnceLock::new();

impl State {
    fn post(&self, command: Command) -> bool {
        {
            let mut ops = lock(&self.ops);
            if self.stopped.load(Ordering::Acquire) { return false; }
            ops.push_back(command);
        }
        if let Err(e) = (&self.writer).write(&[1]) {
            if e.kind() != std::io::ErrorKind::WouldBlock { eprintln!("[hal-direct] presenter wake: {e}"); }
        }
        true
    }
    fn take(&self) -> VecDeque<Command> {
        let mut bytes = [0; 256];
        while (&self.reader).read(&mut bytes).is_ok_and(|n| n > 0) {}
        std::mem::take(&mut *lock(&self.ops))
    }
    fn changed(&self, snapshot: &mut Snapshot) {
        snapshot.generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.shared.signal();
    }
}

struct Presenter(Arc<State>);
impl FramePresenter for Presenter {
    fn submit(&self, session: u32, window: Id, image: Arc<Image>, presented: Presented) {
        crate::profile::since(crate::profile::Metric::SubmitToRendered, crate::profile::submitted_at());
        self.0.post(Command::Frame(Frame { key: (session, window), image, presented: Some(presented), generation: 0, arrived: crate::clock_ns(), taken: 0 }));
    }
}

pub(super) struct Source(Arc<State>);
impl Source {
    pub(super) fn new() -> Self { Self(STATE.get().expect("direct shell connected").clone()) }
}
impl VkSurfaceSource for Source {
    fn instance_extensions(&self) -> Vec<&'static CStr> {
        vec![vk::KHR_DISPLAY_NAME, vk::EXT_DIRECT_MODE_DISPLAY_NAME, vk::EXT_ACQUIRE_DRM_DISPLAY_NAME]
    }
    fn order_devices(&mut self, instance: &makepad_vulkan::Instance, devices: &mut Vec<vk::PhysicalDevice>) -> Result<(), String> {
        render::order_devices(instance, devices)
    }
    fn device_ready(&mut self, device: Arc<Device>) -> Result<(), String> {
        if !device.formats[format::BGRA8 as usize].optimal_tiling_features.contains(vk::FormatFeatureFlags::BLIT_SRC) {
            return Err("direct display requires BGRA8 BLIT_SRC on the compositor GPU".into());
        }
        // A replacement device after a loss: the presenter thread swaps it in, keeping the
        // displays it holds (`render::Context::replace_device`).
        if self.0.started.swap(true, Ordering::AcqRel) {
            return if self.0.post(Command::Device(device)) { Ok(()) } else { Err("the display presenter stopped".into()) };
        }
        render::export_device(&device);
        let state = self.0.clone();
        if let Err(e) = std::thread::Builder::new().name("makepad-kms-present".into()).spawn(move || render::run(state, device)) {
            self.0.started.store(false,Ordering::Release); return Err(e.to_string());
        }
        Ok(())
    }
    fn state(&mut self, session: u32, window: Id, _: [u32; 2]) -> SurfaceState {
        if *lock(&self.0.window) == Some((session,window)) { SurfaceState::Managed } else { SurfaceState::Offscreen }
    }
    fn presenter(&self, session: u32, window: Id) -> Option<Arc<dyn FramePresenter>> {
        if *lock(&self.0.window) == Some((session,window)) { Some(Arc::new(Presenter(self.0.clone()))) } else { None }
    }
}

pub(super) struct App {
    /// The per-user direct-display lock, held for the process's life (see `claim_displays`).
    _owner: std::fs::File,
    state: Arc<State>,
    window: Option<Key>,
    recipient: Option<Key>,
    input: Option<raw_input::RawInput>,
    terminal: Option<terminal::DirectTerminal>,
    generation: u64,
    active: bool,
    releasing: bool,
    stopping: bool,
    clipboard: crate::clipboard::ClipboardData,
    selection: crate::clipboard::ClipboardData,
}

impl App {
    pub(super) fn connect(shared: &'static Shared) -> Result<Self, String> {
        let mut dpi = 1.0f64;
        for arg in std::env::args() {
            if let Some(scale) = arg.strip_prefix("-scale=") {
                dpi = scale.parse().ok().filter(|v: &f64| v.is_finite() && *v > 0.0).ok_or("-scale must be a positive finite number")?;
            }
        }
        // Before the console, the displays or any input device: one direct shell per user.
        let owner = claim_displays()?;
        let terminal = terminal::DirectTerminal::enter()?;
        if terminal.is_none() { eprintln!("[hal-direct] no local VT; DRM access still requires an owned active console"); }
        let (reader, writer) = UnixStream::pair().map_err(|e| e.to_string())?;
        reader.set_nonblocking(true).map_err(|e| e.to_string())?;
        writer.set_nonblocking(true).map_err(|e| e.to_string())?;
        let state = Arc::new(State { shared, reader, writer, ops: Default::default(), snapshot: Default::default(), window: Default::default(),
            generation: AtomicU64::new(0), started: AtomicBool::new(false), stopped: AtomicBool::new(false),
            released: AtomicBool::new(false), ready: AtomicU64::new(0), timings_changed: AtomicBool::new(false), acquired: AtomicBool::new(false), dpi });
        STATE.set(state.clone()).map_err(|_| "direct display shell already initialized")?;
        shared.dpi.store((dpi as f32).to_bits(), Ordering::Release);
        eprintln!("[hal-direct] Ctrl+Alt+Backspace exits; Ctrl+Alt+F1..F12 switches VT");
        // Input devices open once a display is driven (`dispatch`): a shell that gets no
        // display must not take the keyboard and pointer from whoever has them.
        Ok(Self { _owner: owner, state, window: None, recipient: None, input: None,
            terminal, generation: 0, active: true, releasing: false, stopping: false, clipboard: Default::default(), selection: Default::default() })
    }
    fn publish(&mut self) {
        let (size, displays, generation) = {
            let snapshot = lock(&self.state.snapshot);
            (snapshot.size, snapshot.displays.clone(), snapshot.generation)
        };
        let mut screens: Vec<_> = displays.outputs.iter().filter_map(|o| {
            let (x,y) = o.desktop_position?;
            let bounds = [x as f64 / self.state.dpi, y as f64 / self.state.dpi, o.width as f64 / self.state.dpi, o.height as f64 / self.state.dpi];
            Some(crate::ScreenInfo { bounds, work_area: bounds, is_primary: o.primary })
        }).collect();
        screens.sort_by(|a,b| a.bounds[0].total_cmp(&b.bounds[0]));
        let refresh = displays.outputs.iter().find(|o| o.primary).map_or(0.0, |o| o.refresh_hz as f32);
        self.generation = generation;
        if let Some(input) = &mut self.input {
            input.set_bounds(size[0] as f64 / self.state.dpi, size[1] as f64 / self.state.dpi, self.state.dpi);
            input.set_screen_rects(screens.iter().map(|s| s.bounds).collect());
        }
        *lock(&self.state.shared.screens) = screens;
        if let Some(key) = self.recipient {
            let dpi = self.state.dpi;
            let inventory = displays.pack(dpi);
            self.state.shared.push(key, |q, id| {
                q.payload_size(inventory.len() as u32);
                q.direct_displays(id, generation, &inventory);
                q.chrome(id, 0.0, 0.0, 0.0, 0.0, false);
                q.geom(id, 0.0, 0.0, size[0] as f64 / dpi, size[1] as f64 / dpi, dpi as f32, window_state::FULLSCREEN);
                q.display(id, dpi as f32, refresh);
            });
            self.publish_timings();
        }
    }
    fn publish_timings(&self) {
        let Some(key) = self.recipient else { return; };
        let (generation, timings) = {
            let snapshot = lock(&self.state.snapshot);
            let timings: Vec<_> = snapshot.displays.outputs.iter().enumerate().map(|(i, o)| {
                let (time, interval) = snapshot.timings.get(&o.name).copied().unwrap_or_default();
                (i as u32, time, interval)
            }).collect();
            (snapshot.generation, timings)
        };
        self.state.shared.push(key, |q, id| {
            for (output, time, interval) in timings { q.direct_frame_timing(id, generation, output, time, interval); }
        });
    }
    fn publish_input(&self) {
        if let Some(key) = self.recipient {
            self.state.shared.push(key, |q, id| q.direct_input(id, preferences::pointer_speed(false),
                preferences::pointer_speed(true), preferences::natural_scroll(), preferences::tap_to_click()));
        }
    }
    fn owns_display(&self, sid: u32) -> bool {
        self.recipient.is_some_and(|key| key.0 == sid) && !self.state.stopped.load(Ordering::Acquire)
    }
    fn display_command(&self, sid: u32, command: DisplayCommand) -> Result<(), String> {
        if !self.owns_display(sid) { return Err("this session does not own direct displays".into()); }
        command.validate()?;
        let connected = |name: &str| {
            if lock(&self.state.snapshot).displays.outputs.iter().any(|o| o.name == name) { Ok(()) }
            else { Err(format!("{name} is not connected")) }
        };
        let command = match command {
            DisplayCommand::Source(name) => { connected(&name)?; Command::Primary(name) }
            DisplayCommand::Order(order) => Command::Order(order),
            DisplayCommand::Arrangement(arrangement) => {
                if let crate::direct::DisplayArrangement::Mirror { optimise_for } = &arrangement { connected(optimise_for)?; }
                Command::Arrangement(arrangement)
            }
            DisplayCommand::Pacing(pacing) => {
                if let crate::direct::DisplayPacing::Screen(name) = &pacing { connected(name)?; }
                Command::Pacing(pacing)
            }
            DisplayCommand::Mode { name, mode } => {
                let mode = mode.as_deref().map(str::trim).filter(|mode| !mode.is_empty());
                connected(&name)?;
                Command::Mode(name, mode.map(str::to_owned))
            }
        };
        if self.state.post(command) { Ok(()) }
        else { Err("direct display shell stopped".into()) }
    }
    fn revoke_inventory(&self, key: Key) {
        let empty = LinuxDisplaySnapshot::default().pack(self.state.dpi);
        self.state.shared.push(key, |q, _| q.direct_displays(Id::NONE, 0, &empty));
    }
    fn close(&mut self) {
        if let Some(key) = self.window.take() {
            *lock(&self.state.window) = None;
            self.state.ready.store(0, Ordering::Release);
            self.state.post(Command::Window(None));
            let recipient = self.recipient.take().unwrap_or(key);
            self.revoke_inventory(recipient);
            self.state.shared.push(recipient, |q, id| q.closed(id));
        }
    }
    fn input(&mut self) {
        let Some(input) = &mut self.input else { return; };
        let mut events = input.poll_raw_input(crate::clock_ns() as f64 * 1e-9, self.recipient.map_or(Id::NONE, |key| key.1));
        let Some(key) = self.recipient else { return; };
        let mut delivered = EventQueue::default();
        delivered.set_time(crate::clock_ns());
        events.drain_with(|event, payload| {
            if event.tag == event_tag::KeyDown {
                if let Some(crate::events::HalEvent::KeyDown { key: code, repeat, mods, .. }) = event.decode(payload) {
                    let code = KeyCode::from_index(code);
                    if mods & (input::MOD_CONTROL | input::MOD_ALT) == input::MOD_CONTROL | input::MOD_ALT {
                        if code == KeyCode::Backspace {
                            delivered.lifecycle(lifecycle::QUIT_REQUESTED); return;
                        }
                        if !repeat {
                            if let Some(vt) = [KeyCode::F1,KeyCode::F2,KeyCode::F3,KeyCode::F4,KeyCode::F5,KeyCode::F6,
                                KeyCode::F7,KeyCode::F8,KeyCode::F9,KeyCode::F10,KeyCode::F11,KeyCode::F12].iter().position(|k| *k == code) {
                                if let Some(terminal) = &self.terminal { terminal.switch(vt as u32 + 1); } return;
                            }
                        }
                    }
                    if !repeat && mods & (input::MOD_CONTROL | input::MOD_LOGO) != 0 && mods & input::MOD_ALT == 0 && code == KeyCode::KeyV && !self.clipboard.text.is_empty() {
                        delivered.text(key.1, input::text::PASTE, self.clipboard.text.as_bytes());
                    }
                }
            }
            // Only typed text has a payload in the native evdev path.
            if event.tag == event_tag::Text { delivered.text(key.1, input::text::TYPED, payload); }
            else { delivered.push(event); }
        });
        if !delivered.is_empty() { self.state.shared.push(key, |q,_| q.append(&mut delivered)); }
    }
}

/// Where a user's direct shell holds its lock: the runtime dir, else /tmp by uid.
fn owner_lock_path() -> std::path::PathBuf {
    extern "C" { fn getuid() -> u32; }
    match crate::wm_socket::runtime_dir() {
        Some(dir) => dir.join("makepad-direct.lock"),
        // SAFETY: getuid has no failure mode.
        None => std::env::temp_dir().join(format!("makepad-direct-{}.lock", unsafe { getuid() })),
    }
}

/// One direct shell per user: an exclusive lock taken before the console, the displays or any
/// input device is touched, released by the kernel when the process ends. A second start (a
/// process that inherited the WM's `MAKEPAD=linux_direct`, a second WM) is refused here, before
/// it can take the touchpad or fight for DRM master.
fn claim_displays() -> Result<std::fs::File, String> {
    use std::os::fd::AsRawFd;
    extern "C" { fn flock(fd: i32, operation: i32) -> i32; }
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    let path = owner_lock_path();
    let mut file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path)
        .map_err(|e| format!("direct display lock {}: {e}", path.display()))?;
    // SAFETY: flock on a descriptor this function owns.
    if unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) } != 0 {
        let mut holder = String::new();
        let _ = file.read_to_string(&mut holder);
        let holder = holder.trim();
        return Err(format!("another makepad direct-display shell{} owns the displays and input devices ({}); \
            a process started inside the window manager runs as its client, never on the direct display",
            if holder.is_empty() { String::new() } else { format!(" (pid {holder})") }, path.display()));
    }
    file.set_len(0).and_then(|_| file.write_all(format!("{}\n", std::process::id()).as_bytes()))
        .map_err(|e| format!("direct display lock {}: {e}", path.display()))?;
    Ok(file)
}

impl Shell for App {
    fn protocol(&self) -> Protocol { Protocol::Direct }
    fn command(&mut self, op: Op) {
        match op {
            Op::Cmd(sid, command) => match command {
                OsCmd::WindowCreate { id, .. } => {
                    if self.window.is_some() { self.state.shared.window_failed((sid,id), "direct display owns one wide-desktop window"); return; }
                    self.window = Some((sid,id)); self.recipient = self.window;
                    *lock(&self.state.window) = self.window;
                    self.state.post(Command::Window(self.window));
                    self.publish();
                    self.publish_input();
                    self.state.shared.push((sid,id), |q,id| q.focus(id, self.active));
                }
                OsCmd::WindowClose { id } if self.window == Some((sid,id)) || self.recipient == Some((sid,id)) => self.close(),
                OsCmd::FrameRequest { .. } => { if self.recipient.is_some() { self.state.post(Command::Request); } }
                OsCmd::DirectDisplay { req, command } => {
                    let tag = command.tag() as u32;
                    let result = self.display_command(sid, command);
                    self.state.shared.push((sid, Id::NONE), |q, _| {
                        q.direct_display_result(req, tag, result.is_ok(), result.as_ref().err().map_or(&[][..], |e| e.as_bytes()));
                    });
                }
                OsCmd::DirectPointerSpeed { touchpad, hundredths } => {
                    if self.owns_display(sid) { preferences::set_pointer_speed(touchpad, hundredths); self.publish_input(); }
                    else { eprintln!("[hal-direct] session={sid} cannot set pointer speed: it does not own the desktop"); }
                }
                OsCmd::DirectNaturalScroll { on } => {
                    if self.owns_display(sid) { preferences::set_natural_scroll(on); self.publish_input(); }
                    else { eprintln!("[hal-direct] session={sid} cannot set natural scrolling: it does not own the desktop"); }
                }
                OsCmd::DirectTapToClick { on } => {
                    if self.owns_display(sid) { preferences::set_tap_to_click(on); self.publish_input(); }
                    else { eprintln!("[hal-direct] session={sid} cannot set tap to click: it does not own the desktop"); }
                }
                OsCmd::ClipSet { text } => self.clipboard.text = text,
                OsCmd::SelectionSet { text } => self.selection.text = text,
                OsCmd::ClipRead { req, primary } => {
                    let bytes = if primary { &self.selection } else { &self.clipboard }.pack();
                    self.state.shared.push((sid,Id::NONE), |q,_| q.clipboard(req,true,&bytes));
                }
                OsCmd::FileDialog { req, .. } => self.state.shared.push((sid,Id::NONE), |q,_| q.dialog_result(req,0,&[])),
                // There are no native caption buttons to reposition on a direct display.
                OsCmd::WindowDpiOverride { .. } => {}
                // The native cursor is an overlay in widgets::Window. Its shape changes
                // already redraw through the HAL client, without a second hardware cursor.
                _ => {}
            },
            Op::CloseSession(sid) => {
                if self.window.is_some_and(|k| k.0 == sid) { self.close(); }
                else if self.recipient.is_some_and(|k| k.0 == sid) {
                    self.recipient = self.window;
                    self.publish();
                    self.publish_input();
                }
                lock(&self.state.shared.inbox).remove(&sid);
            }
            Op::Lend { host, session, alias } => {
                if self.window == Some((crate::session::HOST,host)) {
                    if let Some(previous) = self.recipient.filter(|key| *key != (session, alias)) { self.revoke_inventory(previous); }
                    self.recipient = Some((session,alias)); self.publish(); self.publish_input();
                }
            }
            Op::Retire(_) | Op::Reveal => {}
        }
    }
    fn dispatch(&mut self) -> Result<(), String> {
        if self.stopping { return Ok(()); }
        if self.state.stopped.load(Ordering::Acquire) {
            if let Some(key) = self.recipient { self.state.shared.push(key, |q,_| q.lifecycle(lifecycle::QUIT_REQUESTED)); }
            return Err("KMS presentation worker stopped".into());
        }
        let signals = self.terminal.as_mut().map_or(0, |t| t.signals());
        if signals & 1 != 0 && self.active {
            self.active = false; self.releasing = true;
            if let Some(input) = &mut self.input { input.suspend(); }
            self.state.released.store(false, Ordering::Release);
            self.state.post(Command::Pause);
            if let Some(key) = self.recipient { self.state.shared.push(key, |q,id| { q.focus(id,false); q.lifecycle(lifecycle::PAUSE); }); }
        }
        if self.releasing && self.state.released.load(Ordering::Acquire) {
            if let Some(terminal) = &self.terminal { terminal.release()?; }
            self.releasing = false;
        }
        if signals & 2 != 0 {
            if let Some(terminal) = &self.terminal { terminal.acquire()?; }
            self.active = true; self.state.post(Command::Resume); self.publish();
            if let Some(key) = self.recipient { self.state.shared.push(key, |q,id| { q.lifecycle(lifecycle::RESUME); q.focus(id,true); }); }
        }
        let timings_changed = self.state.timings_changed.swap(false, Ordering::AcqRel);
        let generation = self.state.generation.load(Ordering::Acquire);
        if generation != self.generation { self.publish(); }
        else if timings_changed { self.publish_timings(); }
        if self.active {
            let next = self.state.ready.swap(0, Ordering::AcqRel);
            if next != 0 { if let Some(key) = self.recipient { self.state.shared.push(key, |q,id| q.frame_ready(id,next)); } }
        }
        if self.input.is_none() && self.active && self.state.acquired.load(Ordering::Acquire) {
            eprintln!("[hal-direct] a display is driven; opening input devices");
            self.input = Some(raw_input::RawInput::new(0.0, 0.0, self.state.dpi));
            self.publish();
        }
        if self.active { self.input(); }
        Ok(())
    }
    fn wait(&mut self, wake: i32) -> Result<(), String> {
        let mut fds: Vec<_> = self.input.iter().flat_map(|input| input.fds()).map(|fd| io::PollFd { fd, events: io::READ, revents: 0 }).collect();
        if let Some(terminal) = &self.terminal { fds.push(io::PollFd { fd: terminal.fd(), events: io::READ, revents: 0 }); }
        let interval = if self.input.as_ref().is_some_and(|i| i.coasting()) { Duration::from_millis(8) } else { Duration::from_secs(1) };
        io::wait(wake, &mut fds, (!self.stopping).then(|| Instant::now() + interval))
    }
    fn shutdown(&mut self) {
        self.close(); self.input = None; self.stopping = true;
        if self.state.started.load(Ordering::Acquire) { self.state.post(Command::Stop); }
        else { self.state.stopped.store(true,Ordering::Release); }
    }
    fn empty(&self) -> bool { self.stopping && self.state.stopped.load(Ordering::Acquire) }
}
