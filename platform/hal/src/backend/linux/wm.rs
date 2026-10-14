//! The WM-hosted shell: a makepad window manager hosts this process's windows. Chosen when a live
//! WM answers its socket (`wm_socket`, the Linux backend order); the WM adopts the process as a
//! hosted client and the shell speaks the same hub protocol as a hosted Cx: each window is
//! announced with `CreateWindow`, the WM sends its geometry, a ring of shared images
//! (`Swapchain`, descriptors over `hosted_ipc`), `Tick`s and input, and every frame renders
//! straight into a free ring image (`VkSurfaceSource::target`, the host's timeline orders the
//! writes), which `DrawCompleteAndFlip` hands back. Input, geometry, focus and close arrive as
//! shell events like any other shell's. A process that hosts other sessions (`makepad run`: the
//! boot document and the app domains share one window) is one WM tile; lending is unchanged.

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
    AppToStudio, AppToStudioVec, ChildRelay, HostRelay, MouseCursor, PinchPhase, PresentableDraw, PresentableImageId, RemoteKeyModifiers, ScrollPhase,
    drag_answer, drag_double, window_op, HostedAppearance, HostedDragAnswer, HostedDragQuery, HostedVisible, HostedWindowOp, SharedSwapchain, StudioToApp, StudioToAppVec, WIRE,
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
    /// Each window's ring of shared images (the WM's swapchain for it), keyed by the host window.
    rings: Mutex<HashMap<Key, Ring>>,
    /// Frames the GPU finished (or that missed the ring), for the shell thread to announce.
    finished: Mutex<Vec<(Key, Option<PresentableDraw>)>>,
    device: Mutex<Option<Arc<Device>>>,
}

struct Ring {
    window_id: usize,
    /// The allocation of every image.
    alloc: [u32; 2],
    /// The window's size in pixels: the part of an image a frame covers.
    pixels: [u32; 2],
    images: Vec<RingImage>,
    next: usize,
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
                return Some(image);
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
        lock(&self.0.finished).push(((session, window), draw));
        self.0.shared.signal();
        presented(crate::clock_ns());
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
    geom: Option<([f64; 4], f64)>,
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
    early: HashMap<usize, ([f64; 4], f64)>,
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
    quit_sent: bool,
    stopping: bool,
}

impl App {
    pub(super) fn connect(shared: &'static Shared) -> Result<Self, String> {
        let path = wm_socket::discovery_path().ok_or("no WM socket path (XDG_RUNTIME_DIR unset)")?;
        let name = wm_socket::client_name();
        let admission = wm_socket::adopt(&path, WIRE, std::process::id(), &name)
            .map_err(|probe| format!("the makepad WM at {} did not adopt this process: {probe:?}", path.display()))?;
        let state = Arc::new(State { shared, inbox: Default::default(), hung_up: AtomicBool::new(false), rings: Default::default(),
            finished: Default::default(), device: Default::default() });
        STATE.set(state.clone()).map_err(|_| "WM-hosted shell already connected")?;
        let hub = Hub::connect(admission.port, admission.client, &name, state.clone())?;
        // Under the WM a makepad window draws its own caption (the WM draws title bars only for
        // foreign Wayland windows): the app's windows get client-side chrome as on a Wayland
        // compositor without server decorations. Its caption's drag query and its window
        // buttons go to the WM through this shell (`HostedDragQuery`, `HostedWindowOp`).
        shared.client_chrome.store(true, Ordering::Release);
        eprintln!("[hal-wm] the makepad WM at {} hosts this process as client {} ({name})", path.display(), admission.client);
        Ok(Self { state, hub, aux_path: hosted_ipc::aux_path(admission.port, &admission.client.to_string()), aux: None,
            pending: Vec::new(), windows: HashMap::new(), early: HashMap::new(), next_window: 0, ticking: false, asked_frame: false, hidden: false,
            clip_reads: HashMap::new(), next_clip_read: 0, selection: Default::default(), drag_queries: VecDeque::new(), quit_sent: false, stopping: false })
    }

    fn key_of(&self, window_id: usize) -> Option<Key> {
        self.windows.iter().find(|(_, w)| w.window_id == window_id).map(|(k, _)| *k)
    }

    /// The host window a command names: by its own key, or by the alias of the session it is
    /// lent to (`makepad run`'s app domain draws into the boot window).
    fn resolve(&self, key: Key) -> Option<Key> {
        self.windows.iter().find(|(k, w)| **k == key || w.recipient == key).map(|(k, _)| *k)
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
                let pixels = self.windows.get(&key).and_then(|w| w.geom).map_or([swapchain.alloc_width, swapchain.alloc_height], |(g, dpi)| pixels(g, dpi));
                lock(&self.state.rings).insert(key, Ring { window_id: swapchain.window_id, alloc: [swapchain.alloc_width, swapchain.alloc_height],
                    pixels, images, next: 0 });
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
        let Some((g, dpi)) = w.geom else { return };
        self.state.shared.push(w.recipient, |q, id| {
            q.display(id, dpi as f32, 60.0);
            q.geom(id, g[0], g[1], g[2], g[3], dpi as f32, window_state::RESTORE);
        });
    }

    /// The first window: input without a window id lands there.
    fn input_recipient(&self) -> Option<Key> {
        self.windows.values().min_by_key(|w| w.window_id).map(|w| w.recipient)
    }

    fn message(&mut self, msg: StudioToApp) {
        if log_enabled() && !matches!(msg, StudioToApp::MouseMove(_)) {
            eprintln!("[hal-wm] host: {}", format!("{msg:?}").chars().take(160).collect::<String>());
        }
        match msg {
            StudioToApp::Swapchain(swapchain) => self.install(swapchain),
            StudioToApp::WindowGeomChange { window_id, dpi_factor, left, top, width, height } => {
                if ![left, top, width, height, dpi_factor].iter().all(|v| v.is_finite()) || dpi_factor <= 0.0 || width < 0.0 || height < 0.0 { return; }
                let geom = [left, top, width, height];
                let Some(key) = self.key_of(window_id) else {
                    self.early.insert(window_id, (geom, dpi_factor));
                    return;
                };
                if let Some(w) = self.windows.get_mut(&key) { w.geom = Some((geom, dpi_factor)); }
                if let Some(ring) = lock(&self.state.rings).get_mut(&key) { ring.pixels = pixels(geom, dpi_factor); }
                self.push_geom(key);
            }
            StudioToApp::Tick => {
                self.asked_frame = false;
                self.ticking = true;
                let mut any = false;
                // A hidden tile is granted nothing: its requests wait for it to show again.
                let hidden = self.hidden;
                for w in self.windows.values_mut().filter(|_| !hidden) {
                    if w.requested && !w.awaiting {
                        w.requested = false;
                        w.awaiting = true;
                        any = true;
                        self.state.shared.push(w.recipient, |q, id| q.frame_ready(id, crate::clock_ns()));
                    }
                }
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
            StudioToApp::Kill => {
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
                // The WM's dark/light: what draws without a stylesheet (the run's load screen)
                // reads it (`host_dark`) on its next turn, which this wakes.
                if let Some(appearance) = HostedAppearance::parse(&json) {
                    if super::host_dark() != Some(appearance.dark) {
                        super::set_host_dark(appearance.dark);
                        self.state.shared.push((session::HOST, Id::NONE), |q, _| q.wake());
                    }
                    return;
                }
                let Some(shown) = HostedVisible::parse(&json) else { return };
                self.hidden = !shown.visible;
                // Frames asked for meanwhile are owed now.
                if !self.hidden && self.windows.values().any(|w| w.requested) {
                    self.ask_frame();
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
        if std::mem::take(&mut self.ticking) && !self.windows.values().any(|w| w.awaiting) {
            self.hub.send(vec![AppToStudio::TickDone]);
        }
    }
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

impl Shell for App {
    fn protocol(&self) -> Protocol {
        Protocol::Wm
    }
    fn command(&mut self, op: Op) {
        match op {
            Op::Cmd(sid, command) => match command {
                OsCmd::WindowCreate { id, .. } => {
                    let window_id = self.next_window;
                    self.next_window += 1;
                    let geom = self.early.remove(&window_id);
                    self.windows.insert((sid, id), Win { window_id, recipient: (sid, id), requested: false, awaiting: false, geom });
                    self.hub.send(vec![AppToStudio::CreateWindow { window_id, kind_id: 0 }]);
                    self.state.shared.push((sid, id), |q, id| q.focus(id, true));
                    self.push_geom((sid, id));
                }
                OsCmd::WindowClose { id } => {
                    let Some(key) = self.resolve((sid, id)) else { return };
                    if let Some(w) = self.windows.remove(&key) {
                        lock(&self.state.rings).remove(&key);
                        self.state.shared.push(w.recipient, |q, id| q.closed(id));
                        self.finish_tick();
                    }
                }
                // The app's caption buttons: the WM minimises, maximises or closes the tile.
                OsCmd::WindowState { id, state } => {
                    if self.resolve((sid, id)).is_none() { return }
                    match state {
                        window_state::MINIMIZE | window_state::HIDE => self.window_op(window_op::MINIMIZE),
                        window_state::MAXIMIZE => self.window_op(window_op::MAXIMIZE),
                        window_state::RESTORE => self.window_op(window_op::RESTORE),
                        window_state::CLOSE => self.window_op(window_op::CLOSE),
                        _ => {}
                    }
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
                let closing: Vec<Key> = self.windows.keys().filter(|k| k.0 == sid).copied().collect();
                for key in closing {
                    self.windows.remove(&key);
                    lock(&self.state.rings).remove(&key);
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
                    w.awaiting = false;
                    self.push_geom(key);
                    self.state.shared.push((session, alias), |q, id| q.focus(id, true));
                    self.finish_tick();
                }
            }
            Op::Retire(_) | Op::Reveal => {}
        }
    }
    fn dispatch(&mut self) -> Result<(), String> {
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
            if let Some(w) = self.windows.get_mut(&key) {
                w.awaiting = false;
                // A frame that found no free shared image is drawn again on the next Tick.
                if draw.is_none() { w.requested = true; }
            }
            match draw {
                Some(draw) => flips.push(AppToStudio::DrawCompleteAndFlip(draw)),
                None => redraw = true,
            }
        }
        self.hub.send(flips);
        if redraw { self.ask_frame(); }
        self.finish_tick();
        let msgs = std::mem::take(&mut *lock(&self.state.inbox));
        for msg in msgs { self.message(msg); }
        if self.state.hung_up.load(Ordering::Acquire) {
            self.quit();
        }
        Ok(())
    }
    fn wait(&mut self, wake: i32) -> Result<(), String> {
        io::wait(wake, &mut [], None)
    }
    fn shutdown(&mut self) {
        self.stopping = true;
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
        let state = Arc::new(State { shared: Shared::global(), inbox: Default::default(), hung_up: AtomicBool::new(false),
            rings: Default::default(), finished: Default::default(), device: Default::default() });
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
        let state = Arc::new(State { shared: Shared::global(), inbox: Default::default(), hung_up: AtomicBool::new(false),
            rings: Default::default(), finished: Default::default(), device: Default::default() });
        let hub = Hub::connect(port, 78, "makepad-app-hidden", state.clone()).unwrap();
        let mut app = App { state, hub, aux_path: Default::default(), aux: None, pending: Vec::new(), windows: HashMap::new(),
            early: HashMap::new(), next_window: 1, ticking: false, asked_frame: false, hidden: false, clip_reads: HashMap::new(),
            next_clip_read: 0, selection: Default::default(), drag_queries: VecDeque::new(), quit_sent: false, stopping: false };
        let key = (9_001, Id::new(1, 1));
        app.windows.insert(key, Win { window_id: 0, recipient: key, requested: false, awaiting: false, geom: None });
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

    /// `makepad run` under the WM draws its own caption: the WM's drag query reaches the app's
    /// window (lent to the app's domain), the app's answer goes back under the WM's id, and the
    /// caption buttons of the lent window become the WM's window ops.
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
            (0..4).map(|_| AppToStudioVec::deserialize_bin(&read_client_frame(&mut stream).1).unwrap().0).collect::<Vec<_>>()
        });
        let state = Arc::new(State { shared: Shared::global(), inbox: Default::default(), hung_up: AtomicBool::new(false),
            rings: Default::default(), finished: Default::default(), device: Default::default() });
        let hub = Hub::connect(port, 79, "makepad-app-clock", state.clone()).unwrap();
        let mut app = App { state, hub, aux_path: Default::default(), aux: None, pending: Vec::new(), windows: HashMap::new(),
            early: HashMap::new(), next_window: 1, ticking: false, asked_frame: false, hidden: false, clip_reads: HashMap::new(),
            next_clip_read: 0, selection: Default::default(), drag_queries: VecDeque::new(), quit_sent: false, stopping: false };
        let host = (session::HOST, Id::new(1, 1));
        let lent = (9_002, Id::new(4, 1));
        app.windows.insert(host, Win { window_id: 0, recipient: lent, requested: false, awaiting: false, geom: None });
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
