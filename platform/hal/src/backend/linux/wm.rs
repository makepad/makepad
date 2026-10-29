//! The WM-hosted shell: a makepad window manager hosts this process's windows. Chosen when a live
//! WM answers its socket (`wm_socket`, the Linux backend order); the WM adopts the process as a
//! hosted client and the shell speaks the same hub protocol as a hosted Cx: each window is
//! announced with `CreateWindow`, the WM sends its geometry, a ring of shared images
//! (`Swapchain`, descriptors over `hosted_ipc`), `Tick`s and input, and every frame renders
//! straight into a free ring image (`VkSurfaceSource::target`, the host's timeline orders the
//! writes), which `DrawCompleteAndFlip` hands back. Input, geometry, focus and close arrive as
//! shell events like any other shell's. A process that hosts other sessions (`makepad run`: the
//! boot document and the app domains share one window) is one WM tile; lending is unchanged.

pub(crate) mod a11y;
pub mod native;

use super::{io, lock, log_enabled, Key, Op, OsCmd, Protocol, Shared, Shell};
use crate::backend::vulkan::share::import_hosted;
use crate::backend::vulkan::surface::{FramePresenter, Offscreen, SurfaceState, VkSurfaceSource};
use crate::backend::vulkan::{Device, Image};
use crate::dmabuf::DmaBuf;
use crate::hosted_ipc::{self, ClientEndpoint};
use crate::types::*;
use crate::{session, wm_socket, Id};
use makepad_micro_serde::SerBin;
use makepad_studio_protocol::{
    AppToStudio, AppToStudioVec, ChildRelay, HostRelay, MouseCursor, PinchPhase, PresentableDraw, PresentableImageId, RelayTray, RelayTrayItem, RemoteKeyModifiers, ScrollPhase,
    drag_answer, drag_double, window_op, HostedAppearance, HostedCaption, HostedDragAnswer, HostedDragQuery, HostedDropdown, HostedVisible, HostedWindowOp, dropdown_op, SharedSwapchain, StudioToApp, StudioToAppVec, WIRE,
};
use std::collections::{HashMap, VecDeque};
use std::ffi::CStr;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static STATE: OnceLock<Arc<State>> = OnceLock::new();

/// What the shell thread, the hub reader and the renderer share.
struct State {
    shared: &'static Shared,
    /// Host messages off the hub socket, in order, for the shell thread.
    inbox: Mutex<VecDeque<StudioToApp>>,
    /// The hub socket closed: the WM let go of this process.
    hung_up: AtomicBool,
    /// The WM went away while this process has a tray: it waits for the next WM (`detach`), and
    /// a ticker wakes the shell once a second to look for it.
    detached: AtomicBool,
    /// Each window's ring of shared images (the WM's swapchain for it), keyed by the host window.
    rings: Mutex<HashMap<Key, Ring>>,
    /// Frames the GPU finished (or that missed the ring), for the shell thread to announce.
    finished: Mutex<Vec<(Key, Option<PresentableDraw>)>>,
    /// Frames of each window encoded and not finished yet (`FramePresenter::scheduled`, ended by
    /// their `finished` entry): a Tick grants a window no frame while it has MAX_IN_FLIGHT.
    in_flight: Mutex<HashMap<Key, u32>>,
    device: Mutex<Option<Arc<Device>>>,
}

/// Frames one window may have encoded and not finished. Without a bound an app that asks for
/// its next frame as soon as it submitted one (a game) was granted a frame every WM Tick
/// (240 Hz) however far the GPU fell behind: batches queued without limit (RSS growth), the
/// driver spun in vkQueueSubmit, and the tile showed frames seconds old, then none.
const MAX_IN_FLIGHT: u32 = 2;
/// A ring whose every image stays with the WM this long is said once (frames meanwhile render
/// offscreen and are not shown).
const STARVED_SAY: std::time::Duration = std::time::Duration::from_secs(1);

struct Ring {
    window_id: usize,
    /// The allocation of every image.
    alloc: [u32; 2],
    /// The window's size in pixels: the part of an image a frame covers.
    pixels: [u32; 2],
    images: Vec<RingImage>,
    next: usize,
    /// Since when no image of the ring was free (all imported, none released by the WM), and
    /// whether that was said.
    starved: Option<(std::time::Instant, bool)>,
}

struct RingImage {
    id: PresentableImageId,
    native: Arc<DmaBuf>,
    /// Imported on the render device at first use.
    image: Option<Arc<Image>>,
    failed: bool,
}

impl State {
    fn global() -> Arc<State> {
        STATE.get().expect("WM-hosted shell connected").clone()
    }
}

pub(super) struct Source(Arc<State>);
impl Source {
    pub(super) fn new() -> Self {
        Self(State::global())
    }
}

impl VkSurfaceSource for Source {
    fn instance_extensions(&self) -> Vec<&'static CStr> {
        Vec::new()
    }
    /// The WM's render GPU (its direct shell exports `MAKEPAD_VULKAN_DEVICE_UUID`): the shared
    /// images are that device's memory.
    fn order_devices(&mut self, instance: &makepad_vulkan::Instance, devices: &mut Vec<makepad_vulkan::vk::PhysicalDevice>) -> Result<(), String> {
        Offscreen.order_devices(instance, devices)
    }
    fn device_ready(&mut self, device: Arc<Device>) -> Result<(), String> {
        // a replacement device (the old one was lost) imports the shared images again
        if lock(&self.0.device).replace(device).is_some() {
            for ring in lock(&self.0.rings).values_mut() {
                for slot in &mut ring.images { slot.image = None; slot.failed = false; }
            }
        }
        Ok(())
    }
    fn presenter(&self, _: u32, _: Id) -> Option<Arc<dyn FramePresenter>> {
        Some(Arc::new(Presenter(self.0.clone())))
    }
    fn state(&mut self, _: u32, _: Id, _: [u32; 2]) -> SurfaceState {
        SurfaceState::Managed
    }
    fn target(&mut self, session: u32, window: Id, size: [u32; 2]) -> Option<Arc<Image>> {
        let device = lock(&self.0.device).clone()?;
        let mut rings = lock(&self.0.rings);
        let Some(ring) = rings.get_mut(&(session, window)) else {
            if log_enabled() { eprintln!("[hal-wm] frame for {:?} before its shared images", (session, window)); }
            return None;
        };
        if size[0] > ring.alloc[0] || size[1] > ring.alloc[1] {
            if log_enabled() { eprintln!("[hal-wm] frame {size:?} larger than the shared images {:?}", ring.alloc); }
            return None;
        }
        let count = ring.images.len();
        for offset in 0..count {
            let index = (ring.next + offset) % count;
            let slot = &mut ring.images[index];
            if slot.image.is_none() && !slot.failed {
                match import_hosted(device.clone(), slot.native.clone()) {
                    Ok(image) => slot.image = Some(image),
                    Err(e) => {
                        slot.failed = true;
                        eprintln!("[hal-wm] shared image {:?}: {e}", slot.id);
                    }
                }
            }
            if let Some(image) = slot.image.clone().filter(|i| i.hosted_writable()) {
                ring.next = (index + 1) % count;
                if let Some((since, true)) = ring.starved.take() {
                    eprintln!("[hal-wm] window {}: the WM released a shared image again after {} ms", ring.window_id, since.elapsed().as_millis());
                }
                return Some(image);
            }
        }
        if ring.images.iter().all(|s| s.image.is_some() || s.failed) {
            crate::profile::count("hal-wm: no free shared image");
            let (since, said) = ring.starved.get_or_insert((std::time::Instant::now(), false));
            if !*said && since.elapsed() >= STARVED_SAY {
                *said = true;
                let states: Vec<String> = ring.images.iter().map(|s| match s.image.as_ref().and_then(|i| i.hosted_state()) {
                    Some((value, next)) => format!("{:?}: timeline {value}, free at {next}", s.id),
                    None => format!("{:?}: {}", s.id, if s.failed { "import failed" } else { "no timeline" }),
                }).collect();
                eprintln!("[hal-wm] window {}: none of its {count} shared images came back from the WM for {} ms; frames render offscreen and are not shown [{}]",
                    ring.window_id, since.elapsed().as_millis(), states.join("; "));
            }
        }
        None
    }
    fn is_target(&self, image: &Arc<Image>) -> bool {
        lock(&self.0.rings).values().any(|r| r.images.iter().any(|s| s.image.as_ref().is_some_and(|i| Arc::ptr_eq(i, image))))
    }
}

struct Presenter(Arc<State>);
impl FramePresenter for Presenter {
    fn submit(&self, session: u32, window: Id, image: Arc<Image>, presented: Box<dyn FnOnce(u64) + Send>) {
        let draw = lock(&self.0.rings).get(&(session, window)).and_then(|ring| {
            let slot = ring.images.iter().find(|s| s.image.as_ref().is_some_and(|i| Arc::ptr_eq(i, &image)))?;
            Some(PresentableDraw { window_id: ring.window_id, target_id: slot.id, width: ring.pixels[0], height: ring.pixels[1],
                sequence: image.hosted_sequence()? })
        });
        if log_enabled() { eprintln!("[hal-wm] frame done {:?}: {draw:?}", (session, window)); }
        let shown = draw.is_some();
        lock(&self.0.finished).push(((session, window), draw));
        self.0.shared.signal();
        if shown { presented(crate::clock_ns()); }
    }
    fn scheduled(&self, session: u32, window: Id) {
        *lock(&self.0.in_flight).entry((session, window)).or_default() += 1;
    }
    fn dropped(&self, session: u32, window: Id) {
        if log_enabled() { eprintln!("[hal-wm] frame dropped {:?}: its batch did not run", (session, window)); }
        lock(&self.0.finished).push(((session, window), None));
        self.0.shared.signal();
    }
}

/// The hub socket: a websocket client to the WM's hub (the frames hosted apps speak).
struct Hub {
    stream: Mutex<TcpStream>,
    /// A send failed (the WM went away): said once.
    broken: AtomicBool,
}

impl Hub {
    fn connect(port: u16, client: u64, name: &str, state: Arc<State>) -> Result<Arc<Hub>, String> {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("hub 127.0.0.1:{port}: {e}"))?;
        let _ = stream.set_nodelay(true);
        let request = format!(
            "GET /app?build={client}&crate={name}&wire={WIRE:016x} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: bWFrZXBhZC13bS1zaGVsbA==\r\nSec-WebSocket-Version: 13\r\n\r\n");
        stream.write_all(request.as_bytes()).map_err(|e| format!("hub handshake: {e}"))?;
        let mut head = Vec::new();
        let mut byte = [0u8];
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() > 8192 || stream.read(&mut byte).map_err(|e| format!("hub handshake: {e}"))? == 0 {
                return Err("the WM's hub closed the handshake".into());
            }
            head.push(byte[0]);
        }
        if !head.starts_with(b"HTTP/1.1 101") {
            return Err(format!("the WM's hub refused this client: {}", String::from_utf8_lossy(&head).lines().next().unwrap_or("")));
        }
        let reader = stream.try_clone().map_err(|e| e.to_string())?;
        let hub = Arc::new(Hub { stream: Mutex::new(stream), broken: AtomicBool::new(false) });
        let pong = hub.clone();
        std::thread::Builder::new().name("wm-hub-reader".into()).spawn(move || {
            read_frames(reader, &pong, &state);
            state.hung_up.store(true, Ordering::Release);
            state.shared.signal();
        }).map_err(|e| e.to_string())?;
        Ok(hub)
    }

    fn frame(&self, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
        let mut out = Vec::with_capacity(payload.len() + 14);
        out.push(0x80 | opcode);
        match payload.len() {
            n if n < 126 => out.push(0x80 | n as u8),
            n if n <= u16::MAX as usize => { out.push(0x80 | 126); out.extend_from_slice(&(n as u16).to_be_bytes()); }
            n => { out.push(0x80 | 127); out.extend_from_slice(&(n as u64).to_be_bytes()); }
        }
        // A client masks every frame; the key only has to differ between frames in a browser.
        let mask = (crate::clock_ns() as u32).to_le_bytes();
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i & 3]));
        lock(&self.stream).write_all(&out)
    }

    fn send(&self, msgs: Vec<AppToStudio>) {
        if msgs.is_empty() { return; }
        if self.broken.load(Ordering::Acquire) { return; }
        if let Err(e) = self.frame(0x2, &AppToStudioVec(msgs).serialize_bin()) {
            self.broken.store(true, Ordering::Release);
            eprintln!("[hal-wm] the WM's hub socket failed: {e}");
        }
    }

    fn close(&self) {
        let _ = self.frame(0x8, &[]);
        let _ = lock(&self.stream).shutdown(std::net::Shutdown::Both);
    }
}

fn read_frames(mut stream: TcpStream, hub: &Hub, state: &State) {
    let mut message = Vec::new();
    loop {
        let mut head = [0u8; 2];
        if stream.read_exact(&mut head).is_err() { return; }
        let (fin, opcode, masked) = (head[0] & 0x80 != 0, head[0] & 0x0f, head[1] & 0x80 != 0);
        let len = match head[1] & 0x7f {
            126 => { let mut b = [0u8; 2]; if stream.read_exact(&mut b).is_err() { return; } u16::from_be_bytes(b) as usize }
            127 => { let mut b = [0u8; 8]; if stream.read_exact(&mut b).is_err() { return; } u64::from_be_bytes(b) as usize }
            n => n as usize,
        };
        let mut mask = [0u8; 4];
        if masked && stream.read_exact(&mut mask).is_err() { return; }
        if len > 256 << 20 { eprintln!("[hal-wm] hub frame of {len} bytes refused"); return; }
        let mut payload = vec![0u8; len];
        if stream.read_exact(&mut payload).is_err() { return; }
        if masked { for (i, b) in payload.iter_mut().enumerate() { *b ^= mask[i & 3]; } }
        match opcode {
            0x8 => return,
            0x9 => { let _ = hub.frame(0xA, &payload); }
            0xA => {}
            0x0 | 0x1 | 0x2 => {
                message.extend_from_slice(&payload);
                if !fin { continue; }
                let data = std::mem::take(&mut message);
                let msgs = match StudioToAppVec::decode_bin(&data) {
                    Ok((batch, errors)) => {
                        for e in errors { eprintln!("[hal-wm] {e}"); }
                        batch.0
                    }
                    Err(_) => match makepad_micro_serde::DeBin::deserialize_bin(&data) {
                        Ok(msg) => vec![msg],
                        Err(e) => { eprintln!("[hal-wm] undecodable host message: {e:?}"); continue; }
                    },
                };
                lock(&state.inbox).extend(msgs);
                state.shared.signal();
            }
            _ => {}
        }
    }
}

struct Win {
    window_id: usize,
    /// Whose window it is now (the host session, or the domain it is lent to).
    recipient: Key,
    /// The session asked for a frame and has not had one since.
    requested: bool,
    /// A FrameReady went out; its frame has not come back yet.
    awaiting: bool,
    /// Frames already on the GPU when that FrameReady went out: their completions come back
    /// first and are not its frame (they must not end the Tick before its frame is drawn).
    ahead: u32,
    geom: Option<([f64; 4], f64, Option<(f64, f64)>)>,
    /// A tray dropdown (`window_kind::DROPDOWN`): the WM shows it as a flyout under the tray's
    /// bar item, never as a tile.
    dropdown: bool,
}

pub(super) struct App {
    state: Arc<State>,
    hub: Arc<Hub>,
    aux_path: std::path::PathBuf,
    aux: Option<ClientEndpoint>,
    /// Swapchains whose descriptors could not be read yet (the channel was not connected).
    pending: Vec<SharedSwapchain>,
    windows: HashMap<Key, Win>,
    /// Geometry the WM sent for a window this process has not made yet (it sizes the tile's
    /// first window at adoption): applied when the window is made.
    early: HashMap<usize, ([f64; 4], f64, Option<(f64, f64)>)>,
    next_window: usize,
    /// A Tick is being answered: TickDone goes out with its frame (or at once without one).
    ticking: bool,
    asked_frame: bool,
    /// The WM shows none of the tile (`HostedVisible`: minimised, on another space, covered by
    /// the windows above it): no frames are asked for or granted until it shows again, as a
    /// compositor holds a covered window's frame callbacks. The sessions' timers, signals and
    /// input go on.
    hidden: bool,
    /// The clipboard is the WM's (shared with every app and the Wayland clients): reads go to the
    /// WM as relays; relay id -> (session, its request id).
    clip_reads: HashMap<u64, (u32, u64)>,
    next_clip_read: u64,
    /// The last primary selection this process made (also sent to the WM).
    selection: crate::clipboard::ClipboardData,
    /// The WM's drag queries (`HostedDragQuery` ids) the app has not answered yet, oldest first:
    /// its `DragReply`s come back in the order the queries went in.
    drag_queries: VecDeque<u32>,
    /// The WM's latest state messages (its style sheet, dark/light, a window's fullscreen), keyed
    /// by their JSON name: the session a window is lent to later hears them too, as an app the WM
    /// launched hears them at its start.
    wm_state: Vec<(String, String)>,
    /// The WM's last activation of each window (`WindowFocus`), by its window id.
    wm_focus: HashMap<usize, bool>,
    /// The trays this process shows in the WM's tray area (`TraySet`), by tray id: the session
    /// that set it and what was sent, so a menu choice reaches that session and the next WM
    /// gets them again.
    trays: HashMap<u64, (u32, RelayTray)>,
    /// Windows the app hid (`WindowState` HIDE: a tray app's closed window): the tile stays
    /// minimised, also on the next WM.
    hidden_windows: std::collections::HashSet<Key>,
    /// The WM went away and this process waits for the next one: when it last looked.
    detached_at: Option<std::time::Instant>,
    /// The last reason a reattach attempt failed (logged once per distinct reason).
    reattach_error: Option<String>,
    quit_sent: bool,
    stopping: bool,
}

impl App {
    pub(super) fn connect(shared: &'static Shared) -> Result<Self, String> {
        let path = wm_socket::discovery_path().ok_or("no WM socket path (XDG_RUNTIME_DIR unset)")?;
        let name = wm_socket::client_name();
        let admission = wm_socket::adopt(&path, WIRE, std::process::id(), &name)
            .map_err(|probe| format!("the makepad WM at {} did not adopt this process: {probe:?}", path.display()))?;
        let state = Arc::new(State { shared, inbox: Default::default(), hung_up: AtomicBool::new(false), detached: AtomicBool::new(false), rings: Default::default(),
            finished: Default::default(), in_flight: Default::default(), device: Default::default() });
        STATE.set(state.clone()).map_err(|_| "WM-hosted shell already connected")?;
        let hub = Hub::connect(admission.port, admission.client, &name, state.clone())?;
        a11y::start(hub.clone());
        // Under the WM a makepad window draws its own caption (the WM draws title bars only for
        // foreign Wayland windows): the app's windows get client-side chrome as on a Wayland
        // compositor without server decorations. Its caption's drag query and its window
        // buttons go to the WM through this shell (`HostedDragQuery`, `HostedWindowOp`).
        shared.client_chrome.store(true, Ordering::Release);
        eprintln!("[hal-wm] the makepad WM at {} hosts this process as client {} ({name})", path.display(), admission.client);
        Ok(Self { state, hub, aux_path: hosted_ipc::aux_path(admission.port, &admission.client.to_string()), aux: None,
            pending: Vec::new(), windows: HashMap::new(), early: HashMap::new(), next_window: 0, ticking: false, asked_frame: false, hidden: false,
            clip_reads: HashMap::new(), next_clip_read: 0, selection: Default::default(), drag_queries: VecDeque::new(), wm_state: Vec::new(), wm_focus: HashMap::new(), trays: HashMap::new(),
            hidden_windows: Default::default(), detached_at: None, reattach_error: None, quit_sent: false, stopping: false })
    }

    fn key_of(&self, window_id: usize) -> Option<Key> {
        self.windows.iter().find(|(_, w)| w.window_id == window_id).map(|(k, _)| *k)
    }

    /// The host window a command names: by its own key, or by the alias of the session it is
    /// lent to (`makepad run`'s app domain draws into the boot window).
    fn resolve(&self, key: Key) -> Option<Key> {
        self.windows.iter().find(|(k, w)| **k == key || w.recipient == key).map(|(k, _)| *k)
    }

    /// The WM's state messages so far, to a session that has just got a window.
    fn replay_wm_state(&self, recipient: Key) {
        for (_, json) in &self.wm_state {
            self.state.shared.push(recipient, |q, id| q.app_message(id, json.as_bytes()));
        }
    }

    /// Ask the WM for a window op (`HostedWindowOp`): the WM owns the tile.
    fn window_op(&self, op: u8) {
        self.hub.send(vec![AppToStudio::Custom(HostedWindowOp { op }.to_json())]);
    }

    fn connect_aux(&mut self) {
        if self.aux.is_some() { return; }
        match ClientEndpoint::try_connect(&self.aux_path) {
            Ok(Some(endpoint)) => {
                // The host writes a swapchain's descriptors before its message: a read that
                // stalls is a broken channel, not a slow GPU.
                let _ = endpoint.set_read_timeout(Some(std::time::Duration::from_secs(1)));
                self.aux = Some(endpoint);
            }
            Ok(None) => {}
            Err(e) => eprintln!("[hal-wm] descriptor channel {}: {e}", self.aux_path.display()),
        }
    }

    fn install(&mut self, swapchain: SharedSwapchain) {
        self.connect_aux();
        let Some(aux) = &self.aux else { self.pending.push(swapchain); return };
        let Some(key) = self.key_of(swapchain.window_id) else { return };
        let images: Result<Vec<_>, _> = swapchain.presentable_images.iter().map(|image| {
            hosted_ipc::recv_image(aux, *image, swapchain.alloc_width, swapchain.alloc_height)
                .map(|native| RingImage { id: image.id, native: Arc::new(native), image: None, failed: false })
        }).collect();
        match images {
            Ok(images) => {
                if log_enabled() { eprintln!("[hal-wm] window {} has {} shared images of {}x{}", swapchain.window_id, images.len(), swapchain.alloc_width, swapchain.alloc_height); }
                let pixels = self.windows.get(&key).and_then(|w| w.geom).map_or([swapchain.alloc_width, swapchain.alloc_height], |(g, dpi, _)| pixels(g, dpi));
                lock(&self.state.rings).insert(key, Ring { window_id: swapchain.window_id, alloc: [swapchain.alloc_width, swapchain.alloc_height],
                    pixels, images, next: 0, starved: None });
                // A frame asked for before there was anywhere to draw it.
                if self.windows.get(&key).is_some_and(|w| w.requested) {
                    self.ask_frame();
                }
            }
            Err(e) => eprintln!("[hal-wm] swapchain for window {}: {e}", swapchain.window_id),
        }
    }

    fn push_geom(&self, key: Key) {
        let Some(w) = self.windows.get(&key) else { return };
        let Some((g, dpi, origin)) = w.geom else { return };
        crate::a11y::set_screen_origin(crate::a11y::window_key(w.recipient.0, w.recipient.1.index), origin.map(|(x,y)| (x as f32,y as f32)));
        self.state.shared.push(w.recipient, |q, id| {
            q.display(id, dpi as f32, 60.0);
            q.geom(id, g[0], g[1], g[2], g[3], dpi as f32, window_state::RESTORE);
        });
    }

    /// The window the WM shows as this process's tile: its first (a tray dropdown is a flyout,
    /// never the tile).
    fn tile_window(&self) -> Option<Key> {
        self.windows.iter().filter(|(_, w)| !w.dropdown).min_by_key(|(_, w)| w.window_id).map(|(k, _)| *k)
    }

    /// The first window: input without a window id lands there.
    fn input_recipient(&self) -> Option<Key> {
        self.windows.values().min_by_key(|w| w.window_id).map(|w| w.recipient)
    }

    fn message(&mut self, msg: StudioToApp) {
        if a11y::request(&msg) { return; }
        if log_enabled() && !matches!(msg, StudioToApp::MouseMove(_)) {
            eprintln!("[hal-wm] host: {}", format!("{msg:?}").chars().take(160).collect::<String>());
        }
        match msg {
            StudioToApp::NativeApp(event) => self.native_message(event),
            StudioToApp::Swapchain(swapchain) => self.install(swapchain),
            StudioToApp::WindowGeomChange { window_id, dpi_factor, left, top, width, height, screen_origin } => {
                if ![left, top, width, height, dpi_factor].iter().all(|v| v.is_finite()) || dpi_factor <= 0.0 || width < 0.0 || height < 0.0 { return; }
                let geom = [left, top, width, height];
                let Some(key) = self.key_of(window_id) else {
                    self.early.insert(window_id, (geom, dpi_factor, screen_origin));
                    return;
                };
                let resized = self.windows.get(&key).and_then(|w| w.geom).is_none_or(|(old, dpi, _)| old != geom || dpi != dpi_factor);
                if let Some(w) = self.windows.get_mut(&key) {
                    w.geom = Some((geom, dpi_factor, screen_origin));
                    crate::a11y::set_screen_origin(crate::a11y::window_key(w.recipient.0, w.recipient.1.index), screen_origin.map(|(x,y)| (x as f32,y as f32)));
                }
                if let Some(ring) = lock(&self.state.rings).get_mut(&key) { ring.pixels = pixels(geom, dpi_factor); }
                if resized { self.push_geom(key); }
            }
            StudioToApp::Tick => {
                self.asked_frame = false;
                self.ticking = true;
                let mut any = false;
                // A hidden tile is granted nothing: its requests wait for it to show again.
                let hidden = self.hidden;
                let in_flight = lock(&self.state.in_flight);
                for (key, w) in self.windows.iter_mut().filter(|_| !hidden) {
                    // a window with MAX_IN_FLIGHT frames on the GPU waits for one to finish
                    // (`dispatch` asks for the next Tick then)
                    if w.requested && !w.awaiting && in_flight.get(key).copied().unwrap_or(0) < MAX_IN_FLIGHT {
                        w.requested = false;
                        w.awaiting = true;
                        w.ahead = in_flight.get(key).copied().unwrap_or(0);
                        any = true;
                        self.state.shared.push(w.recipient, |q, id| q.frame_ready(id, crate::clock_ns()));
                    }
                }
                drop(in_flight);
                if !any {
                    self.ticking = false;
                    self.hub.send(vec![AppToStudio::TickDone]);
                }
            }
            StudioToApp::MouseDown(e) => self.pointer(|q, id| q.mouse_down(id, e.x, e.y, e.button_raw_bits, mods(&e.modifiers))),
            StudioToApp::MouseMove(e) => self.pointer(|q, id| q.mouse_move(id, e.x, e.y, 0.0, 0.0, mods(&e.modifiers))),
            StudioToApp::MouseUp(e) => self.pointer(|q, id| q.mouse_up(id, e.x, e.y, e.button_raw_bits, mods(&e.modifiers))),
            StudioToApp::MouseCancel(e) => self.pointer(|q, id| q.mouse_cancel(id, e.x, e.y, e.button_raw_bits, mods(&e.modifiers))),
            StudioToApp::Scroll(e) => self.pointer(|q, id| q.scroll(id, e.x, e.y, e.sx, e.sy, mods(&e.modifiers), !e.is_mouse, phase(e.phase))),
            StudioToApp::Pinch(e) => {
                let phase = match e.phase { PinchPhase::Begin => input::pinch::BEGIN, PinchPhase::Update => input::pinch::UPDATE, PinchPhase::End => input::pinch::END };
                self.pointer(|q, id| q.pinch(id, e.x, e.y, e.scale, phase));
            }
            StudioToApp::KeyDown(e) => {
                let m = key_mods(e.modifiers.shift, e.modifiers.control, e.modifiers.alt, e.modifiers.logo);
                self.pointer(|q, id| q.key_down(id, e.key_code.to_index(), e.is_repeat, m));
            }
            StudioToApp::KeyUp(e) => {
                let m = key_mods(e.modifiers.shift, e.modifiers.control, e.modifiers.alt, e.modifiers.logo);
                self.pointer(|q, id| q.key_up(id, e.key_code.to_index(), m));
            }
            StudioToApp::TextInput(e) => {
                let kind = if e.was_paste { input::text::PASTE } else { input::text::TYPED };
                self.pointer(|q, id| q.text(id, kind, e.input.as_bytes()));
            }
            // The WM closed the tile: each window is asked to close, as a window manager's
            // close button asks (the app may save first; `makepad run` ends the run).
            // With no window left to ask (the app closed its own), the process is asked to quit.
            StudioToApp::Kill => {
                if self.windows.is_empty() {
                    self.quit();
                }
                for key in self.windows.keys() {
                    self.state.shared.push(*key, |q, id| q.close_requested(id));
                }
            }
            StudioToApp::Custom(json) => {
                // A press in the window: the app's drag query says whether it is its caption.
                if let Some(query) = HostedDragQuery::parse(&json) {
                    if let Some(key) = self.input_recipient() {
                        self.drag_queries.push_back(query.id);
                        self.state.shared.push(key, |q, id| q.drag_query(id, query.x, query.y));
                    }
                    return;
                }
                // The WM dismissed a tray dropdown (a click elsewhere, Escape, the item again): the
                // app hears it as the window system's `PopupDismissed` and hides the window.
                if let Some(window) = HostedDropdown::parse(&json).filter(|w| w.op == dropdown_op::DISMISS) {
                    if let Some(w) = self.key_of(window.window_id).and_then(|key| self.windows.get(&key)) {
                        self.state.shared.push(w.recipient, |q, id| q.popup_dismissed(id));
                    }
                    return;
                }
                // The tile's visibility is this shell's pacing (no frames while nothing shows).
                if let Some(shown) = HostedVisible::parse(&json) {
                    self.hidden = !shown.visible;
                    // Frames asked for meanwhile are owed now.
                    if !self.hidden && self.windows.values().any(|w| w.requested) {
                        self.ask_frame();
                    }
                    return;
                }
                // The WM's dark/light: what draws without a stylesheet (the run's load screen)
                // reads it (`host_dark`) on its next turn, which this wakes. The app hears it too.
                if let Some(appearance) = HostedAppearance::parse(&json) {
                    if super::host_dark() != Some(appearance.dark) {
                        super::set_host_dark(appearance.dark);
                        self.state.shared.push((session::HOST, Id::NONE), |q, _| q.wake());
                    }
                }
                // Every other WM message is the app's, as a WM-hosted Cx hears it (`Event::Custom`):
                // the WM's style sheet (its window controls' family), fullscreen, parked, ...
                if let Some(name) = wm_state_name(&json) {
                    // The style sheet's family gives the window controls; the run's load screen
                    // draws its caption by them (`host_window_controls`) on its next turn.
                    if name == "makepad_style" && crate::window_controls::set_host_sheet(&json) {
                        self.state.shared.push((session::HOST, Id::NONE), |q, _| q.wake());
                    }
                    // The sheet's caption colours, as the app's caption will draw them.
                    if let Some(c) = (name == "makepad_caption").then(|| HostedCaption::parse(&json)).flatten() {
                        let caption = crate::window_controls::HostCaption { bar: c.bar, bar_unfocused: c.bar_unfocused, ink: c.ink, ink_unfocused: c.ink_unfocused, size: c.size, button: c.button };
                        if crate::window_controls::set_host_caption(caption) {
                            self.state.shared.push((session::HOST, Id::NONE), |q, _| q.wake());
                        }
                    }
                    match self.wm_state.iter_mut().find(|(n, _)| n == name) {
                        Some(entry) => entry.1 = json.clone(),
                        None => self.wm_state.push((name.to_string(), json.clone())),
                    }
                }
                if let Some(key) = self.input_recipient() {
                    self.state.shared.push(key, |q, id| q.app_message(id, json.as_bytes()));
                }
            }
            // The WM's activation of a tile: the window gains or loses focus as on any shell.
            StudioToApp::WindowFocus { window_id, focused } => {
                self.wm_focus.insert(window_id, focused);
                if let Some(recipient) = self.key_of(window_id).and_then(|key| self.windows.get(&key)).map(|w| w.recipient) {
                    self.state.shared.push(recipient, |q, id| q.focus(id, focused));
                }
            }
            // The WM's tray area: a menu choice or a click on one of this process's trays.
            StudioToApp::Relay(HostRelay::TrayMenu { tray, item }) => {
                if let Some((sid, _)) = self.trays.get(&tray) {
                    self.state.shared.push((*sid, Id::NONE), |q, _| q.tray_menu(tray, item));
                }
            }
            StudioToApp::Relay(HostRelay::TrayClick { tray, anchor, screen }) => {
                if let Some((sid, _)) = self.trays.get(&tray) {
                    let screen = [screen[0] as f32, screen[1] as f32, screen[2] as f32, screen[3] as f32];
                    self.state.shared.push((*sid, Id::NONE), |q, _| q.tray_click(tray, anchor[0], anchor[1], anchor[2], anchor[3], screen));
                }
            }
            StudioToApp::Relay(HostRelay::Clipboard { request_id, text, png }) => {
                if let Some((sid, req)) = self.clip_reads.remove(&request_id) {
                    let bytes = crate::clipboard::ClipboardData { text, png, files: Vec::new() }.pack();
                    self.state.shared.push((sid, Id::NONE), |q, _| q.clipboard(req, true, &bytes));
                }
            }
            _ => {}
        }
    }

    fn pointer(&self, f: impl FnOnce(&mut crate::buffers::EventQueue, Id)) {
        if let Some(key) = self.input_recipient() {
            self.state.shared.push(key, f);
        }
    }

    /// The WM is gone: the process quits. Frames asked for from now on are granted at once, so
    /// nothing on the way out waits for a Tick that will not come.
    fn quit(&mut self) {
        if std::mem::replace(&mut self.quit_sent, true) { return; }
        let owners: std::collections::HashSet<u32> = self.windows.keys().map(|k| k.0).collect();
        for sid in owners.into_iter().chain((self.windows.is_empty()).then_some(session::HOST)) {
            self.state.shared.push((sid, Id::NONE), |q, _| q.lifecycle(lifecycle::QUIT_REQUESTED));
        }
        for w in self.windows.values_mut() {
            w.awaiting = true;
            self.state.shared.push(w.recipient, |q, id| q.frame_ready(id, crate::clock_ns()));
        }
    }

    /// One Tick asked for at a time, and none while the WM shows nothing of the tile.
    fn ask_frame(&mut self) {
        if self.hidden || self.asked_frame { return; }
        self.asked_frame = true;
        self.hub.send(vec![AppToStudio::RequestAnimationFrame]);
    }

    fn finish_tick(&mut self) {
        // The Tick stays open while a granted frame is awaited: its TickDone goes out with that
        // frame (taking `ticking` first lost the TickDone, and the WM paced the app at its 50 ms
        // fallback).
        if self.ticking && !self.windows.values().any(|w| w.awaiting) {
            self.ticking = false;
            self.hub.send(vec![AppToStudio::TickDone]);
        }
    }
}

/// The WM messages that are state, not events (the latest one holds): its style sheet
/// (`makepad_style`) and its resolved caption (`makepad_caption`), dark/light (`makepad_appearance`) and a window's fullscreen
/// (`makepad_fullscreen`). Their name is the message's one top-level JSON key.
fn wm_state_name(json: &str) -> Option<&str> {
    const STATE: [&str; 4] = ["makepad_style", "makepad_caption", "makepad_appearance", "makepad_fullscreen"];
    let name = json.trim_start().strip_prefix('{')?.trim_start().strip_prefix('"')?;
    let name = &name[..name.find('"')?];
    STATE.contains(&name).then_some(name)
}

/// A window's size in pixels at `dpi`.
fn pixels(geom: [f64; 4], dpi: f64) -> [u32; 2] {
    [(geom[2] * dpi).round().max(1.0) as u32, (geom[3] * dpi).round().max(1.0) as u32]
}

fn key_mods(shift: bool, control: bool, alt: bool, logo: bool) -> u32 {
    (shift as u32 * input::MOD_SHIFT) | (control as u32 * input::MOD_CONTROL) | (alt as u32 * input::MOD_ALT) | (logo as u32 * input::MOD_LOGO)
}

fn mods(m: &RemoteKeyModifiers) -> u32 {
    key_mods(m.shift, m.control, m.alt, m.logo)
}

fn phase(p: ScrollPhase) -> u32 {
    match p {
        ScrollPhase::None => input::scroll::NONE,
        ScrollPhase::Began => input::scroll::BEGAN,
        ScrollPhase::Touched => input::scroll::TOUCHED,
        ScrollPhase::Changed => input::scroll::CHANGED,
        ScrollPhase::Ended => input::scroll::ENDED,
        ScrollPhase::Momentum => input::scroll::MOMENTUM,
        ScrollPhase::MomentumEnded => input::scroll::MOMENTUM_ENDED,
    }
}

/// The HAL's cursor ids (`cursor::*`) as the hosted protocol's cursors.
fn mouse_cursor(id: u32) -> MouseCursor {
    use MouseCursor::*;
    const BY_ID: [MouseCursor; 26] = [Hidden, Default, Crosshair, Hand, Arrow, Move, Text, Wait, Help, NotAllowed, Grab, Grabbing,
        NResize, NeResize, EResize, SeResize, SResize, SwResize, WResize, NwResize, NsResize, NeswResize, EwResize, NwseResize,
        ColResize, RowResize];
    BY_ID.get(id as usize).copied().unwrap_or(Default)
}

/// A tray menu row as the relay carries it.
fn tray_item(item: &crate::types::MenuItem, label: String) -> RelayTrayItem {
    use crate::types::menu_kind;
    if item.kind & 0xff == menu_kind::LINE {
        return RelayTrayItem { line: true, ..Default::default() };
    }
    RelayTrayItem {
        id: item.cmd,
        label,
        enabled: item.kind & menu_kind::DISABLED == 0,
        checked: (item.kind & menu_kind::CHECKABLE != 0).then_some(item.kind & menu_kind::CHECKED != 0),
        line: false,
    }
}

/// How often a detached process looks for the next WM.
const REATTACH_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

impl App {
    /// The WM went away and this process has a tray (its windows can come back, its service
    /// keeps running): it lets go of the old WM's shared images and waits for the next WM at
    /// the same socket. Nothing draws meanwhile; timers, signals and the app's work go on.
    fn detach(&mut self) {
        a11y::detached();
        eprintln!("[hal-wm] the makepad WM went away; this process keeps its tray and waits for the next WM");
        self.detached_at = Some(std::time::Instant::now());
        lock(&self.state.rings).clear();
        lock(&self.state.in_flight).clear();
        self.pending.clear();
        self.aux = None;
        self.ticking = false;
        self.asked_frame = false;
        self.hidden = true;
        self.drag_queries.clear();
        self.early.clear();
        self.wm_focus.clear();
        for w in self.windows.values_mut() {
            w.awaiting = false;
            w.ahead = 0;
        }
        if !self.state.detached.swap(true, Ordering::AcqRel) {
            let state = self.state.clone();
            let _ = std::thread::Builder::new().name("wm-reattach".into()).spawn(move || {
                while state.detached.load(Ordering::Acquire) {
                    std::thread::sleep(REATTACH_EVERY);
                    state.shared.signal();
                }
            });
        }
    }

    /// A detached process asks the WM socket again; a live WM adopts it as a new client: its
    /// windows are announced again (a hidden one stays minimised) and its trays shown again.
    fn reattach(&mut self) {
        if self.detached_at.is_some_and(|at| at.elapsed() < REATTACH_EVERY) { return; }
        self.detached_at = Some(std::time::Instant::now());
        let name = wm_socket::client_name();
        let mut found = None;
        let mut last = String::from("no WM socket to ask");
        for path in wm_socket::reattach_paths() {
            match wm_socket::adopt(&path, WIRE, std::process::id(), &name) {
                Ok(admission) => { found = Some((path, admission)); break; }
                Err(e) => last = format!("{}: {e:?}", path.display()),
            }
        }
        let Some((path, admission)) = found else {
            // once per distinct reason, so a long wait is visible without a line a second
            if self.reattach_error.as_deref() != Some(last.as_str()) {
                eprintln!("[hal-wm] still waiting for the next WM: {last}");
                self.reattach_error = Some(last);
            }
            return;
        };
        self.reattach_error = None;
        self.state.hung_up.store(false, Ordering::Release);
        let hub = match Hub::connect(admission.port, admission.client, &name, self.state.clone()) {
            Ok(hub) => hub,
            Err(e) => {
                eprintln!("[hal-wm] the next WM admitted this process but its hub did not take it: {e}");
                return;
            }
        };
        eprintln!("[hal-wm] the makepad WM at {} hosts this process again as client {} ({name})", path.display(), admission.client);
        a11y::start(hub.clone());
        self.hub = hub;
        self.aux_path = hosted_ipc::aux_path(admission.port, &admission.client.to_string());
        self.detached_at = None;
        self.state.detached.store(false, Ordering::Release);
        self.hidden = false;
        let mut windows: Vec<(Key, usize, bool)> = self.windows.iter().map(|(k, w)| (*k, w.window_id, w.dropdown)).collect();
        windows.sort_by_key(|(_, id, _)| *id);
        for (key, window_id, dropdown) in windows {
            self.hub.send(vec![AppToStudio::CreateWindow { window_id, kind_id: if dropdown { window_kind::DROPDOWN as usize } else { 0 } }]);
            if self.hidden_windows.contains(&key) {
                self.window_op(window_op::MINIMIZE);
            }
            if let Some(w) = self.windows.get_mut(&key) { w.requested = true; }
        }
        let trays: Vec<AppToStudio> = self.trays.values().map(|(_, t)| AppToStudio::Relay(ChildRelay::SetTray(t.clone()))).collect();
        self.hub.send(trays);
        self.ask_frame();
    }
}

impl Shell for App {
    fn protocol(&self) -> Protocol {
        Protocol::Wm
    }
    fn command(&mut self, op: Op) {
        match op {
            Op::Cmd(sid, command) => match command {
                OsCmd::WindowCreate { id, kind, .. } => {
                    let window_id = self.next_window;
                    self.next_window += 1;
                    let geom = self.early.remove(&window_id);
                    let dropdown = kind & 0xff == window_kind::DROPDOWN;
                    self.windows.insert((sid, id), Win { window_id, recipient: (sid, id), requested: false, awaiting: false, ahead: 0, geom, dropdown });
                    self.hub.send(vec![AppToStudio::CreateWindow { window_id, kind_id: if dropdown { window_kind::DROPDOWN as usize } else { 0 } }]);
                    a11y::window(window_id, Some((sid, id)));
                    let focused = self.wm_focus.get(&window_id).copied().unwrap_or(true);
                    self.state.shared.push((sid, id), |q, id| q.focus(id, focused));
                    self.replay_wm_state((sid, id));
                    self.push_geom((sid, id));
                }
                OsCmd::WindowClose { id } => {
                    let Some(key) = self.resolve((sid, id)) else { return };
                    let tile = self.tile_window() == Some(key);
                    if let Some(w) = self.windows.remove(&key) {
                        a11y::window(w.window_id, None);
                        lock(&self.state.rings).remove(&key);
                        lock(&self.state.in_flight).remove(&key);
                        self.state.shared.push(w.recipient, |q, id| q.closed(id));
                        self.finish_tick();
                        // The app closed the window the WM shows as its tile (its caption's close
                        // said yes): the WM closes the tile, as a native window's close takes the
                        // window off the screen. It asks the process to end with it (`Kill`).
                        if tile {
                            self.window_op(window_op::CLOSE);
                        }
                    }
                }
                // The app's caption buttons: the WM minimises, maximises or closes the tile.
                OsCmd::WindowState { id, state } => {
                    let Some(key) = self.resolve((sid, id)) else { return };
                    // A dropdown is hidden and shown by its own ops; it is never the tile.
                    if self.windows.get(&key).is_some_and(|w| w.dropdown) {
                        if state == window_state::HIDE {
                            let window_id = self.windows[&key].window_id;
                            self.hub.send(vec![AppToStudio::Custom(HostedDropdown::new(window_id, dropdown_op::HIDE).to_json())]);
                        }
                        return;
                    }
                    match state {
                        // A hidden window (a tray app's closed one) is a minimised tile.
                        window_state::HIDE => {
                            self.hidden_windows.insert(key);
                            self.window_op(window_op::MINIMIZE);
                        }
                        window_state::MINIMIZE => self.window_op(window_op::MINIMIZE),
                        window_state::SHOW => {
                            self.hidden_windows.remove(&key);
                            self.window_op(window_op::SHOW);
                        }
                        window_state::MAXIMIZE => self.window_op(window_op::MAXIMIZE),
                        window_state::RESTORE => self.window_op(window_op::RESTORE),
                        // The app-drawn caption's close: the app is asked first (a tray app hides
                        // its window); the WM's own close of a main tile would end the process.
                        window_state::CLOSE => {
                            if let Some(w) = self.windows.get(&key) {
                                self.state.shared.push(w.recipient, |q, id| q.close_requested(id));
                            }
                        }
                        _ => {}
                    }
                }
                // The WM's tray area shows the tray; its menu choices and (a dropdown's) primary
                // clicks come back as relays.
                OsCmd::TraySet { tray, w, h, alpha, tooltip, items, flags } => {
                    let items = items.into_iter().map(|(item, label)| tray_item(&item, label)).collect();
                    let relay = RelayTray { tray, width: w, height: h, alpha, tooltip, items, dropdown: flags & tray_flags::DROPDOWN != 0 };
                    self.hub.send(vec![AppToStudio::Relay(ChildRelay::SetTray(relay.clone()))]);
                    self.trays.insert(tray, (sid, relay));
                }
                OsCmd::TrayRemove { tray } => {
                    if self.trays.remove(&tray).is_some() {
                        self.hub.send(vec![AppToStudio::Relay(ChildRelay::RemoveTray { tray })]);
                    }
                }
                // A tray dropdown: the WM places the window under the tray's bar item.
                OsCmd::WindowAtTray { id, tray, w, h } => {
                    let Some(key) = self.resolve((sid, id)) else { return };
                    let window_id = self.windows[&key].window_id;
                    let show = HostedDropdown { tray, w: w as f64, h: h as f64, ..HostedDropdown::new(window_id, dropdown_op::SHOW) };
                    self.hub.send(vec![AppToStudio::Custom(show.to_json())]);
                }
                OsCmd::DragReply { to: input::reply::QUERY, answer, .. } => {
                    if let Some(id) = self.drag_queries.pop_front() {
                        let response = u8::try_from(answer).unwrap_or(drag_answer::NO_ANSWER);
                        self.hub.send(vec![AppToStudio::Custom(HostedDragAnswer { id, response, double_click: drag_double::MAXIMISE }.to_json())]);
                    }
                }
                OsCmd::FrameRequest { id } => {
                    let Some(key) = self.resolve((sid, id)) else { return };
                    if self.quit_sent {
                        let w = &self.windows[&key];
                        self.state.shared.push(w.recipient, |q, id| q.frame_ready(id, crate::clock_ns()));
                        return;
                    }
                    let w = self.windows.get_mut(&key).unwrap();
                    // A request after a FrameReady: that beat went unused (no submit), its credit is spent.
                    let spent = std::mem::replace(&mut w.awaiting, false);
                    w.requested = true;
                    if spent { self.finish_tick(); }
                    self.ask_frame();
                }
                OsCmd::Cursor { cursor, .. } => self.hub.send(vec![AppToStudio::SetCursor(mouse_cursor(cursor))]),
                OsCmd::ClipSet { text } => self.hub.send(vec![AppToStudio::SetClipboard(text)]),
                // A drag out of the window: the WM carries the files from here (`StartFileDrag`),
                // so the app's own gesture ends now, as when a desktop drag session ends.
                OsCmd::DragStart { id, items, .. } => {
                    let paths: Vec<String> = crate::drag_promise::to_files(items)
                        .into_iter()
                        .filter(|(kind, _)| *kind == input::drag_item::FILE)
                        .map(|(_, path)| path)
                        .collect();
                    if !paths.is_empty() {
                        self.hub.send(vec![AppToStudio::Relay(ChildRelay::StartFileDrag { paths })]);
                    }
                    let recipient = self.resolve((sid, id)).and_then(|k| self.windows.get(&k)).map_or((sid, id), |w| w.recipient);
                    self.state.shared.push(recipient, |q, id| q.drag_drop(id, -100000.0, -100000.0, false, 0, &[]));
                }
                OsCmd::SelectionSet { text } => {
                    self.hub.send(vec![AppToStudio::Relay(ChildRelay::SetPrimarySelection { text: text.clone() })]);
                    self.selection.text = text;
                }
                OsCmd::ClipRead { req, primary: true } => {
                    let bytes = self.selection.pack();
                    self.state.shared.push((sid, Id::NONE), |q, _| q.clipboard(req, true, &bytes));
                }
                OsCmd::ClipRead { req, primary: false } => {
                    self.next_clip_read += 1;
                    self.clip_reads.insert(self.next_clip_read, (sid, req));
                    self.hub.send(vec![AppToStudio::Relay(ChildRelay::ReadClipboard { request_id: self.next_clip_read })]);
                }
                OsCmd::FileDialog { req, .. } => self.state.shared.push((sid, Id::NONE), |q, _| q.dialog_result(req, 0, &[])),
                OsCmd::DirectDisplay { req, command } => self.state.shared.push((sid, Id::NONE), |q, _| {
                    q.direct_display_result(req, command.tag() as u32, false, b"a WM-hosted window does not own direct displays");
                }),
                _ => {}
            },
            Op::CloseSession(sid) => {
                self.clip_reads.retain(|_, (s, _)| *s != sid);
                let gone: Vec<u64> = self.trays.iter().filter(|(_, (s, _))| *s == sid).map(|(t, _)| *t).collect();
                for tray in gone {
                    self.trays.remove(&tray);
                    self.hub.send(vec![AppToStudio::Relay(ChildRelay::RemoveTray { tray })]);
                }
                let closing: Vec<Key> = self.windows.keys().filter(|k| k.0 == sid).copied().collect();
                for key in closing {
                    self.windows.remove(&key);
                    lock(&self.state.rings).remove(&key);
                    lock(&self.state.in_flight).remove(&key);
                }
                // A domain that borrowed a window gives it back to the host.
                let lent: Vec<Key> = self.windows.iter().filter(|(k, w)| w.recipient.0 == sid && k.0 != sid).map(|(k, _)| *k).collect();
                for key in lent {
                    if let Some(w) = self.windows.get_mut(&key) { w.recipient = key; w.awaiting = false; }
                    self.push_geom(key);
                }
                self.finish_tick();
                lock(&self.state.shared.inbox).remove(&sid);
            }
            Op::Lend { host, session, alias } => {
                let key = (session::HOST, host);
                if let Some(w) = self.windows.get_mut(&key) {
                    w.recipient = (session, alias);
                    a11y::window(w.window_id, Some(w.recipient));
                    w.awaiting = false;
                    let focused = self.wm_focus.get(&w.window_id).copied().unwrap_or(true);
                    self.push_geom(key);
                    self.state.shared.push((session, alias), |q, id| q.focus(id, focused));
                    self.replay_wm_state((session, alias));
                    self.finish_tick();
                }
            }
            Op::Retire(_) | Op::Reveal => {}
        }
    }
    fn dispatch(&mut self) -> Result<(), String> {
        self.native_outgoing();
        if self.stopping { return Ok(()); }
        // The WM shares a swapchain only once this end of the descriptor channel is connected;
        // its listener exists from the adoption on.
        self.connect_aux();
        if !self.pending.is_empty() {
            if self.aux.is_some() {
                for swapchain in std::mem::take(&mut self.pending) { self.install(swapchain); }
            }
        }
        let finished = std::mem::take(&mut *lock(&self.state.finished));
        let mut flips = Vec::new();
        let mut redraw = false;
        for (key, draw) in finished {
            if let Some(n) = lock(&self.state.in_flight).get_mut(&key) { *n = n.saturating_sub(1); }
            if let Some(w) = self.windows.get_mut(&key) {
                // an earlier frame's completion leaves the granted frame awaited
                if w.ahead > 0 { w.ahead -= 1; } else { w.awaiting = false; }
                // A frame that found no free shared image is drawn again on the next Tick.
                if draw.is_none() { w.requested = true; }
            }
            match draw {
                Some(draw) => flips.push(AppToStudio::DrawCompleteAndFlip(draw)),
                None => redraw = true,
            }
        }
        self.hub.send(flips);
        // a frame finished: a window held back by MAX_IN_FLIGHT may be granted the next one
        if redraw || self.windows.values().any(|w| w.requested && !w.awaiting) { self.ask_frame(); }
        self.finish_tick();
        let msgs = std::mem::take(&mut *lock(&self.state.inbox));
        for msg in msgs { self.message(msg); }
        if self.detached_at.is_some() {
            self.reattach();
        } else if self.state.hung_up.load(Ordering::Acquire) {
            // A process with a tray outlives its WM (a WM restart): it waits for the next one.
            if self.trays.is_empty() { self.quit(); } else { self.detach(); }
        }
        Ok(())
    }
    fn wait(&mut self, wake: i32) -> Result<(), String> {
        io::wait(wake, &mut [], None)
    }
    fn shutdown(&mut self) {
        self.stopping = true;
        a11y::stop();
        self.hub.close();
    }
    fn empty(&self) -> bool {
        self.stopping
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_micro_serde::DeBin;
    use std::net::TcpListener;

    /// One frame as the hub reads it: unmasks a client frame.
    fn read_client_frame(stream: &mut TcpStream) -> (u8, Vec<u8>) {
        let mut head = [0u8; 2];
        stream.read_exact(&mut head).unwrap();
        assert!(head[1] & 0x80 != 0, "a client frame is masked");
        let len = match head[1] & 0x7f {
            126 => { let mut b = [0u8; 2]; stream.read_exact(&mut b).unwrap(); u16::from_be_bytes(b) as usize }
            127 => { let mut b = [0u8; 8]; stream.read_exact(&mut b).unwrap(); u64::from_be_bytes(b) as usize }
            n => n as usize,
        };
        let mut mask = [0u8; 4];
        stream.read_exact(&mut mask).unwrap();
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).unwrap();
        for (i, b) in payload.iter_mut().enumerate() { *b ^= mask[i & 3]; }
        (head[0] & 0x0f, payload)
    }

    #[test]
    fn hub_handshake_batches_both_ways_and_hang_up() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8];
            while !request.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).unwrap(); request.push(byte[0]); }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with(&format!("GET /app?build=77&crate=makepad-app-amp&wire={WIRE:016x} ")), "{request}");
            stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").unwrap();
            // a batch, split over a continuation frame, then a ping
            let batch = StudioToAppVec(vec![StudioToApp::Tick, StudioToApp::Kill]).serialize_bin();
            let (a, b) = batch.split_at(5);
            stream.write_all(&[0x02, a.len() as u8]).unwrap();
            stream.write_all(a).unwrap();
            stream.write_all(&[0x80, 126]).unwrap();
            stream.write_all(&(b.len() as u16).to_be_bytes()).unwrap();
            stream.write_all(b).unwrap();
            stream.write_all(&[0x89, 2, b'h', b'i']).unwrap();
            // the pong (reader thread) and the batch (caller) race: either order
            let mut frames = [read_client_frame(&mut stream), read_client_frame(&mut stream)];
            frames.sort_by_key(|(opcode, _)| *opcode);
            let [(data, sent), (pong, hi)] = frames;
            assert_eq!((pong, hi.as_slice()), (0xA, &b"hi"[..]));
            assert_eq!(data, 0x2);
            let sent = AppToStudioVec::deserialize_bin(&sent).unwrap();
            assert!(matches!(sent.0.as_slice(), [AppToStudio::CreateWindow { window_id: 3, kind_id: 0 }, AppToStudio::TickDone]));
            // the WM goes away
        });
        let state = Arc::new(State { shared: Shared::global(), inbox: Default::default(), hung_up: AtomicBool::new(false), detached: AtomicBool::new(false),
            rings: Default::default(), finished: Default::default(), in_flight: Default::default(), device: Default::default() });
        let hub = Hub::connect(port, 77, "makepad-app-amp", state.clone()).unwrap();
        hub.send(vec![AppToStudio::CreateWindow { window_id: 3, kind_id: 0 }, AppToStudio::TickDone]);
        server.join().unwrap();
        for _ in 0..200 {
            if state.hung_up.load(Ordering::Acquire) { break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(state.hung_up.load(Ordering::Acquire), "a closed hub socket is a hang-up");
        let got: Vec<_> = lock(&state.inbox).drain(..).collect();
        assert!(matches!(got.as_slice(), [StudioToApp::Tick, StudioToApp::Kill]), "{got:?}");
    }

    #[test]
    fn a_hidden_tile_asks_for_no_frames_and_is_granted_none() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8];
            while !request.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).unwrap(); request.push(byte[0]); }
            stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").unwrap();
            (0..2).map(|_| AppToStudioVec::deserialize_bin(&read_client_frame(&mut stream).1).unwrap().0).collect::<Vec<_>>()
        });
        let state = Arc::new(State { shared: Shared::global(), inbox: Default::default(), hung_up: AtomicBool::new(false), detached: AtomicBool::new(false),
            rings: Default::default(), finished: Default::default(), in_flight: Default::default(), device: Default::default() });
        let hub = Hub::connect(port, 78, "makepad-app-hidden", state.clone()).unwrap();
        let mut app = App { state, hub, aux_path: Default::default(), aux: None, pending: Vec::new(), windows: HashMap::new(),
            early: HashMap::new(), next_window: 1, ticking: false, asked_frame: false, hidden: false, clip_reads: HashMap::new(),
            next_clip_read: 0, selection: Default::default(), drag_queries: VecDeque::new(), wm_state: Vec::new(), wm_focus: HashMap::new(),
            trays: HashMap::new(), hidden_windows: Default::default(), detached_at: None, reattach_error: None, quit_sent: false, stopping: false };
        let key = (9_001, Id::new(1, 1));
        app.windows.insert(key, Win { window_id: 0, recipient: key, requested: false, awaiting: false, ahead: 0, geom: None, dropdown: false });
        // covered: the app's frame request is held, a Tick grants nothing and is answered at once
        app.message(StudioToApp::Custom(HostedVisible { visible: false }.to_json()));
        app.command(Op::Cmd(key.0, OsCmd::FrameRequest { id: key.1 }));
        app.message(StudioToApp::Tick);
        assert!(app.windows[&key].requested && !app.windows[&key].awaiting, "no frame while hidden");
        // shown again: the held request is asked for, and the next Tick grants it
        app.message(StudioToApp::Custom(HostedVisible { visible: true }.to_json()));
        app.message(StudioToApp::Tick);
        assert!(!app.windows[&key].requested && app.windows[&key].awaiting, "the owed frame once shown");
        let sent = server.join().unwrap();
        assert!(matches!(sent[0].as_slice(), [AppToStudio::TickDone]), "{:?}", sent[0]);
        assert!(matches!(sent[1].as_slice(), [AppToStudio::RequestAnimationFrame]), "{:?}", sent[1]);
    }

    #[test]
    /// A tray goes to the WM as a relay (items, check marks, lines, the glyph), a menu choice
    /// comes back to the session that set it, a closed session's tray is removed, and a process
    /// with a tray waits for the next WM when its WM goes away instead of quitting.
    fn trays_relay_to_the_wm_and_outlive_it() {
        use crate::types::{menu_kind, MenuItem};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8];
            while !request.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).unwrap(); request.push(byte[0]); }
            stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").unwrap();
            (0..2).map(|_| AppToStudioVec::deserialize_bin(&read_client_frame(&mut stream).1).unwrap().0).collect::<Vec<_>>()
        });
        let state = Arc::new(State { shared: Shared::global(), inbox: Default::default(), hung_up: AtomicBool::new(false), detached: AtomicBool::new(false),
            rings: Default::default(), finished: Default::default(), in_flight: Default::default(), device: Default::default() });
        let hub = Hub::connect(port, 79, "makepad-app-aihub", state.clone()).unwrap();
        let mut app = App { state, hub, aux_path: Default::default(), aux: None, pending: Vec::new(), windows: HashMap::new(),
            early: HashMap::new(), next_window: 1, ticking: false, asked_frame: false, hidden: false, clip_reads: HashMap::new(),
            next_clip_read: 0, selection: Default::default(), drag_queries: VecDeque::new(), wm_state: Vec::new(), wm_focus: HashMap::new(),
            trays: HashMap::new(), hidden_windows: Default::default(), detached_at: None, reattach_error: None, quit_sent: false, stopping: false };
        let item = |kind, cmd| MenuItem { parent: 0, kind, cmd, key: 0, str: Default::default() };
        let items = vec![
            (item(menu_kind::ITEM | menu_kind::DISABLED, 1), "Serving".to_string()),
            (item(menu_kind::LINE, 0), String::new()),
            (item(menu_kind::ITEM | menu_kind::CHECKABLE | menu_kind::CHECKED, 2), "Enabled".to_string()),
            (item(menu_kind::ITEM, 3), "Open AI Hub".to_string()),
        ];
        app.command(Op::Cmd(4, OsCmd::TraySet { tray: 77, w: 2, h: 1, flags: tray_flags::DROPDOWN, alpha: vec![0, 255], tooltip: "AI Hub".into(), items }));
        assert_eq!(app.trays.get(&77).map(|(sid, _)| *sid), Some(4), "the choice goes to the session that set it");
        app.command(Op::CloseSession(4));
        assert!(app.trays.is_empty(), "a closed session's tray goes");
        let sent = server.join().unwrap();
        match sent[0].as_slice() {
            [AppToStudio::Relay(ChildRelay::SetTray(t))] => {
                assert_eq!((t.tray, t.width, t.height, t.alpha.as_slice(), t.tooltip.as_str()), (77, 2, 1, &[0u8, 255][..], "AI Hub"));
                assert_eq!(t.items[0], RelayTrayItem { id: 1, label: "Serving".into(), enabled: false, checked: None, line: false });
                assert!(t.items[1].line);
                assert_eq!(t.items[2], RelayTrayItem { id: 2, label: "Enabled".into(), enabled: true, checked: Some(true), line: false });
                assert_eq!(t.items[3].checked, None);
                assert!(t.dropdown, "the dropdown flag reaches the WM");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(sent[1].as_slice(), [AppToStudio::Relay(ChildRelay::RemoveTray { tray: 77 })]), "{:?}", sent[1]);
        // The WM goes away with a tray up: detached, not quitting.
        app.trays.insert(78, (0, RelayTray::default()));
        app.state.hung_up.store(true, Ordering::Release);
        app.dispatch().unwrap();
        assert!(app.detached_at.is_some() && !app.quit_sent, "a tray app waits for the next WM");
        app.state.detached.store(false, Ordering::Release);
    }

    #[test]
    /// A tray dropdown is announced to the WM as such, asks to be shown at its tray with its size,
    /// says when it is hidden (not as the tile's minimise), and the WM's primary click on the
    /// tray reaches the session that set it.
    fn a_dropdown_window_is_announced_shown_at_its_tray_and_hidden() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8];
            while !request.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).unwrap(); request.push(byte[0]); }
            stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").unwrap();
            (0..3).map(|_| AppToStudioVec::deserialize_bin(&read_client_frame(&mut stream).1).unwrap().0).collect::<Vec<_>>()
        });
        let state = Arc::new(State { shared: Shared::global(), inbox: Default::default(), hung_up: AtomicBool::new(false), detached: AtomicBool::new(false),
            rings: Default::default(), finished: Default::default(), in_flight: Default::default(), device: Default::default() });
        let hub = Hub::connect(port, 81, "makepad-app-aihub", state.clone()).unwrap();
        let mut app = App { state, hub, aux_path: Default::default(), aux: None, pending: Vec::new(), windows: HashMap::new(),
            early: HashMap::new(), next_window: 0, ticking: false, asked_frame: false, hidden: false, clip_reads: HashMap::new(),
            next_clip_read: 0, selection: Default::default(), drag_queries: VecDeque::new(), wm_state: Vec::new(), wm_focus: HashMap::new(),
            trays: HashMap::new(), hidden_windows: Default::default(), detached_at: None, reattach_error: None, quit_sent: false, stopping: false };
        let id = Id::new(5, 1);
        app.command(Op::Cmd(6, OsCmd::WindowCreate { id, kind: window_kind::DROPDOWN, parent: Id::NONE, geom: [0.0; 4] }));
        assert!(app.windows[&(6, id)].dropdown);
        app.command(Op::Cmd(6, OsCmd::WindowAtTray { id, tray: 77, w: 460.0, h: 600.0 }));
        app.command(Op::Cmd(6, OsCmd::WindowState { id, state: window_state::HIDE }));
        assert!(app.hidden_windows.is_empty(), "a hidden dropdown is not a minimised tile");
        let sent = server.join().unwrap();
        assert!(matches!(sent[0].as_slice(), [AppToStudio::CreateWindow { window_id: 0, kind_id }] if *kind_id == window_kind::DROPDOWN as usize), "{:?}", sent[0]);
        match sent[1].as_slice() {
            [AppToStudio::Custom(json)] => {
                let d = HostedDropdown::parse(json).unwrap();
                assert_eq!((d.op, d.window_id, d.tray, d.w, d.h), (dropdown_op::SHOW, 0, 77, 460.0, 600.0));
            }
            other => panic!("{other:?}"),
        }
        match sent[2].as_slice() {
            [AppToStudio::Custom(json)] => assert_eq!(HostedDropdown::parse(json).unwrap().op, dropdown_op::HIDE),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    /// A Tick granted while an earlier frame is still on the GPU stays open until its own frame
    /// comes back: the earlier frame's completion does not answer it, and a finish_tick while the
    /// frame is awaited does not drop the answer (the WM then paced the app at its 50 ms fallback).
    fn tick_done_waits_for_the_granted_frame() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8];
            while !request.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).unwrap(); request.push(byte[0]); }
            stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").unwrap();
            loop { if read_client_frame(&mut stream).0 == 0x8 { break; } }
        });
        let state = Arc::new(State { shared: Shared::global(), inbox: Default::default(), hung_up: AtomicBool::new(false), detached: AtomicBool::new(false),
            rings: Default::default(), finished: Default::default(), in_flight: Default::default(), device: Default::default() });
        let hub = Hub::connect(port, 80, "makepad-app-game", state.clone()).unwrap();
        let mut app = App { state: state.clone(), hub, aux_path: Default::default(), aux: None, pending: Vec::new(), windows: HashMap::new(),
            early: HashMap::new(), next_window: 1, ticking: false, asked_frame: false, hidden: false, clip_reads: HashMap::new(),
            next_clip_read: 0, selection: Default::default(), drag_queries: VecDeque::new(), wm_state: Vec::new(), wm_focus: HashMap::new(),
            trays: HashMap::new(), hidden_windows: Default::default(), detached_at: None, reattach_error: None, quit_sent: false, stopping: false };
        let key = (9_003, Id::new(1, 1));
        app.windows.insert(key, Win { window_id: 0, recipient: key, requested: false, awaiting: false, ahead: 0, geom: None, dropdown: false });
        let presenter = Presenter(state.clone());
        // frame 1 is on the GPU; the app asks again and the next Tick grants frame 2
        app.command(Op::Cmd(key.0, OsCmd::FrameRequest { id: key.1 }));
        app.message(StudioToApp::Tick);
        presenter.scheduled(key.0, key.1);
        app.command(Op::Cmd(key.0, OsCmd::FrameRequest { id: key.1 }));
        app.message(StudioToApp::Tick);
        assert!(app.ticking && app.windows[&key].awaiting, "frame 2 granted");
        app.finish_tick();
        assert!(app.ticking, "a finish_tick while the frame is awaited keeps the Tick open");
        // frame 1 completes: not frame 2
        presenter.dropped(key.0, key.1);
        app.dispatch().unwrap();
        assert!(app.ticking && app.windows[&key].awaiting, "an earlier frame's completion does not answer the Tick");
        // frame 2 is drawn and completes: the Tick is answered
        presenter.scheduled(key.0, key.1);
        presenter.dropped(key.0, key.1);
        app.dispatch().unwrap();
        assert!(!app.ticking && !app.windows[&key].awaiting, "the granted frame answers the Tick");
        app.shutdown();
        server.join().unwrap();
    }

    /// A game asks for its next frame right after submitting one. The WM's Ticks (240 Hz) grant
    /// it frames only while fewer than MAX_IN_FLIGHT of them are on the GPU; a finished frame
    /// (shown, missed the ring, or dropped) lets the next one go.
    #[test]
    fn frames_in_flight_are_bounded_per_window() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8];
            while !request.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).unwrap(); request.push(byte[0]); }
            stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").unwrap();
            loop { if read_client_frame(&mut stream).0 == 0x8 { break; } }
        });
        let state = Arc::new(State { shared: Shared::global(), inbox: Default::default(), hung_up: AtomicBool::new(false), detached: AtomicBool::new(false),
            rings: Default::default(), finished: Default::default(), in_flight: Default::default(), device: Default::default() });
        let hub = Hub::connect(port, 79, "makepad-app-game", state.clone()).unwrap();
        let mut app = App { state: state.clone(), hub, aux_path: Default::default(), aux: None, pending: Vec::new(), windows: HashMap::new(),
            early: HashMap::new(), next_window: 1, ticking: false, asked_frame: false, hidden: false, clip_reads: HashMap::new(),
            next_clip_read: 0, selection: Default::default(), drag_queries: VecDeque::new(), wm_state: Vec::new(), wm_focus: HashMap::new(),
            trays: HashMap::new(), hidden_windows: Default::default(), detached_at: None, reattach_error: None, quit_sent: false, stopping: false };
        let key = (9_002, Id::new(1, 1));
        app.windows.insert(key, Win { window_id: 0, recipient: key, requested: false, awaiting: false, ahead: 0, geom: None, dropdown: false });
        let presenter = Presenter(state.clone());
        // each granted beat submits a frame and asks for the next one at once
        for _ in 0..MAX_IN_FLIGHT {
            app.command(Op::Cmd(key.0, OsCmd::FrameRequest { id: key.1 }));
            app.message(StudioToApp::Tick);
            assert!(app.windows[&key].awaiting, "granted while under the bound");
            presenter.scheduled(key.0, key.1);
        }
        app.command(Op::Cmd(key.0, OsCmd::FrameRequest { id: key.1 }));
        app.message(StudioToApp::Tick);
        assert!(app.windows[&key].requested && !app.windows[&key].awaiting, "no frame while {MAX_IN_FLIGHT} are on the GPU");
        // one finishes (its batch failed: dropped): the held request goes on the next Tick
        presenter.dropped(key.0, key.1);
        app.dispatch().unwrap();
        assert_eq!(state.in_flight.lock().unwrap()[&key], MAX_IN_FLIGHT - 1);
        app.message(StudioToApp::Tick);
        assert!(!app.windows[&key].requested && app.windows[&key].awaiting, "granted once a frame finished");
        app.shutdown();
        server.join().unwrap();
    }

    /// `makepad run` under the WM draws its own caption: the WM's drag query reaches the app's
    /// window (lent to the app's domain), the app's answer goes back under the WM's id, and the
    /// caption buttons of the lent window become the WM's window ops; the app closing that window closes the tile.
    #[test]
    fn the_caption_of_a_lent_window_answers_the_wm_and_the_wm_appearance_reaches_host_dark() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8];
            while !request.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).unwrap(); request.push(byte[0]); }
            stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").unwrap();
            (0..5).map(|_| AppToStudioVec::deserialize_bin(&read_client_frame(&mut stream).1).unwrap().0).collect::<Vec<_>>()
        });
        let state = Arc::new(State { shared: Shared::global(), inbox: Default::default(), hung_up: AtomicBool::new(false), detached: AtomicBool::new(false),
            rings: Default::default(), finished: Default::default(), in_flight: Default::default(), device: Default::default() });
        let hub = Hub::connect(port, 79, "makepad-app-clock", state.clone()).unwrap();
        let mut app = App { state, hub, aux_path: Default::default(), aux: None, pending: Vec::new(), windows: HashMap::new(),
            early: HashMap::new(), next_window: 1, ticking: false, asked_frame: false, hidden: false, clip_reads: HashMap::new(),
            next_clip_read: 0, selection: Default::default(), drag_queries: VecDeque::new(), wm_state: Vec::new(), wm_focus: HashMap::new(),
            trays: HashMap::new(), hidden_windows: Default::default(), detached_at: None, reattach_error: None, quit_sent: false, stopping: false };
        let host = (session::HOST, Id::new(1, 1));
        let lent = (9_002, Id::new(4, 1));
        app.windows.insert(host, Win { window_id: 0, recipient: lent, requested: false, awaiting: false, ahead: 0, geom: None, dropdown: false });
        // two presses (a double click) before the app answers: the answers keep the WM's ids
        app.message(StudioToApp::Custom(HostedDragQuery { id: 11, x: 40.0, y: 8.0 }.to_json()));
        app.message(StudioToApp::Custom(HostedDragQuery { id: 12, x: 41.0, y: 8.0 }.to_json()));
        assert_eq!(app.drag_queries, [11, 12]);
        assert!(lock(&app.state.shared.inbox).contains_key(&lent.0), "the query goes to the session the window is lent to");
        let reply = |answer| OsCmd::DragReply { id: lent.1, to: input::reply::QUERY, answer };
        app.command(Op::Cmd(lent.0, reply(input::drag_query::CAPTION)));
        app.command(Op::Cmd(lent.0, reply(input::drag_query::CLIENT)));
        assert!(app.drag_queries.is_empty());
        // the caption's buttons, sent by the app's domain on its alias of the window
        app.command(Op::Cmd(lent.0, OsCmd::WindowState { id: lent.1, state: window_state::MAXIMIZE }));
        app.command(Op::Cmd(lent.0, OsCmd::WindowState { id: lent.1, state: window_state::MINIMIZE }));
        // a window this shell does not have asks nothing
        app.command(Op::Cmd(lent.0, OsCmd::WindowState { id: Id::new(9, 1), state: window_state::MAXIMIZE }));
        // the WM's dark/light reaches `host_dark` (the run's load screen reads it) and wakes it
        app.message(StudioToApp::Custom(HostedAppearance { dark: true }.to_json()));
        assert_eq!(super::super::host_dark(), Some(true));
        assert!(lock(&app.state.shared.inbox).contains_key(&session::HOST), "the load screen is woken");
        app.message(StudioToApp::Custom(HostedAppearance { dark: false }.to_json()));
        assert_eq!(super::super::host_dark(), Some(false));
        // the app's caption close said yes and the app closed its window: the WM closes the tile,
        // and its Kill then finds no window to ask, so the process is asked to quit
        app.command(Op::Cmd(lent.0, OsCmd::WindowClose { id: lent.1 }));
        assert!(app.windows.is_empty());
        assert!(!app.quit_sent);
        app.message(StudioToApp::Kill);
        assert!(app.quit_sent, "a Kill with no window left quits the process");
        lock(&app.state.shared.inbox).remove(&session::HOST);
        lock(&app.state.shared.inbox).remove(&lent.0);
        let custom = |batch: &Vec<AppToStudio>| match batch.as_slice() {
            [AppToStudio::Custom(json)] => json.clone(),
            other => panic!("{other:?}"),
        };
        let sent: Vec<String> = server.join().unwrap().iter().map(custom).collect();
        assert_eq!(HostedDragAnswer::parse(&sent[0]), Some(HostedDragAnswer { id: 11, response: drag_answer::CAPTION, double_click: drag_double::MAXIMISE }));
        assert_eq!(HostedDragAnswer::parse(&sent[1]), Some(HostedDragAnswer { id: 12, response: drag_answer::CLIENT, double_click: drag_double::MAXIMISE }));
        assert_eq!(HostedWindowOp::parse(&sent[2]), Some(HostedWindowOp { op: window_op::MAXIMIZE }));
        assert_eq!(HostedWindowOp::parse(&sent[3]), Some(HostedWindowOp { op: window_op::MINIMIZE }));
        assert_eq!(HostedWindowOp::parse(&sent[4]), Some(HostedWindowOp { op: window_op::CLOSE }));
    }

    /// `makepad run` under the WM: the WM's style sheet and its activation of the tile arrive while
    /// the run's own boot window has it (the app's domain is not up yet); the session the window is
    /// lent to then hears the latest of each, as an app the WM launched hears them at its start.
    /// Events that are no state (a drag query, a back press) are not replayed.
    #[test]
    fn a_lent_window_hears_the_wm_state_sent_before_the_lend() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8];
            while !request.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).unwrap(); request.push(byte[0]); }
            stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").unwrap();
        });
        let state = Arc::new(State { shared: Shared::global(), inbox: Default::default(), hung_up: AtomicBool::new(false), detached: AtomicBool::new(false),
            rings: Default::default(), finished: Default::default(), in_flight: Default::default(), device: Default::default() });
        let hub = Hub::connect(port, 80, "makepad-app-lend", state.clone()).unwrap();
        server.join().unwrap();
        let mut app = App { state, hub, aux_path: Default::default(), aux: None, pending: Vec::new(), windows: HashMap::new(),
            early: HashMap::new(), next_window: 1, ticking: false, asked_frame: false, hidden: false, clip_reads: HashMap::new(),
            next_clip_read: 0, selection: Default::default(), drag_queries: VecDeque::new(), wm_state: Vec::new(), wm_focus: HashMap::new(),
            trays: HashMap::new(), hidden_windows: Default::default(), detached_at: None, reattach_error: None, quit_sent: false, stopping: false };
        let host = (session::HOST, Id::new(77, 1));
        app.windows.insert(host, Win { window_id: 0, recipient: host, requested: false, awaiting: false, ahead: 0, geom: None, dropdown: false });
        let old_style = r#"{"makepad_style":{"name":"windows"}}"#;
        let style = r#"{"makepad_style":{"name":"macos"}}"#;
        let back = r#"{"makepad_back":{}}"#;
        for json in [old_style, style, back] {
            app.message(StudioToApp::Custom(json.to_string()));
        }
        app.message(StudioToApp::WindowFocus { window_id: 0, focused: false });
        let drain = |app: &App, sid: u32| {
            let mut got = Vec::new();
            if let Some(mut q) = lock(&app.state.shared.inbox).remove(&sid) {
                q.drain_with(|e, payload| got.push((e.tag, String::from_utf8_lossy(payload).into_owned(), e.decode(payload).map(|e| format!("{e:?}")))));
            }
            got
        };
        // (the boot window heard them as they came: session::HOST's inbox is shared with other tests)
        // lent to the app's domain: the latest style, and the tile's focus as the WM last said it
        let lent = (9_101, Id::new(4, 1));
        app.command(Op::Lend { host: host.1, session: lent.0, alias: lent.1 });
        let heard = drain(&app, lent.0);
        let messages: Vec<&str> = heard.iter().filter(|(tag, ..)| *tag == crate::events::event_tag::AppMessage).map(|(_, json, _)| json.as_str()).collect();
        assert_eq!(messages, [style], "only the latest state, no events");
        let focus: Vec<&String> = heard.iter().filter(|(tag, ..)| *tag == crate::events::event_tag::Focus).filter_map(|(.., e)| e.as_ref()).collect();
        assert_eq!(focus.len(), 1);
        assert!(focus[0].contains("focused: false"), "{focus:?}");
    }

    #[test]
    fn cursors_and_phases_map_one_to_one() {
        assert_eq!(mouse_cursor(cursor::TEXT), MouseCursor::Text);
        assert_eq!(mouse_cursor(cursor::ROW_RESIZE), MouseCursor::RowResize);
        assert_eq!(mouse_cursor(999), MouseCursor::Default);
        assert_eq!(phase(ScrollPhase::Momentum), input::scroll::MOMENTUM);
        assert_eq!(pixels([0.0, 0.0, 100.5, 50.0], 2.0), [201, 100]);
        assert_eq!(key_mods(true, false, true, false), input::MOD_SHIFT | input::MOD_ALT);
    }
}
