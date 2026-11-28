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
use crate::backend::shell;
use crate::backend::vulkan::share::import_hosted;
use crate::backend::vulkan::surface::{FramePresenter, Offscreen, SurfaceState, VkSurfaceSource};
use crate::backend::vulkan::{Device, Image};
use crate::dmabuf::DmaBuf;
use crate::hosted_ipc::{self, ClientEndpoint};
use crate::types::*;
use crate::window_controls::{hosted_window_op, HostedAppFrame, HostedClipboard, HostedResize, HostedWindow, WINDOW_OP_FULLSCREEN};
use crate::{session, wm_socket, Id};
use makepad_micro_serde::SerBin;
use makepad_studio_protocol::{
    AppToStudio, AppToStudioVec, ChildRelay, HostRelay, MouseCursor, PinchPhase, PresentableDraw, PresentableImageId, RelayFileDialog, RelayTray, RelayTrayItem, RemoteKeyModifiers, ScrollPhase,
    drag_answer, drag_double, window_op, HostedAppearance, HostedCaption, HostedDragAnswer, HostedDragQuery, HostedDropdown, HostedWindowOp, dropdown_op, SharedSwapchain, StudioToApp, StudioToAppVec, WIRE,
};
use std::collections::{HashMap, VecDeque};
use std::ffi::CStr;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

relax_api::option_layouts! {
    (Key, f64, f64),
}

const POINTER_CAPS_JSON: &str = r#"{"makepad_pointer_caps":{"mouse_cancel":true}}"#;

static STATE: OnceLock<Arc<State>> = OnceLock::new();

#[derive(Clone)]
struct Pace {
    started: bool,
    asked_since_ns: u64,
    tick_open_since_ns: u64,
    last_tick_ns: u64,
    last_tick_done_ns: u64,
    last_raf_ns: u64,
    ticks: u64,
    tick_dones: u64,
    rafs: u64,
    rafs_in_tick: u64,
    hidden: bool,
    windows: Vec<(usize, bool, bool, u32, u32)>,
}

static PACE: Mutex<Pace> = Mutex::new(Pace {
    started: false, asked_since_ns: 0, tick_open_since_ns: 0, last_tick_ns: 0,
    last_tick_done_ns: 0, last_raf_ns: 0, ticks: 0, tick_dones: 0, rafs: 0,
    rafs_in_tick: 0, hidden: false, windows: Vec::new(),
});

/// (`linux::end_run`) The ticks and frames of the ended app go; the connection stays.
pub(crate) fn end_run() {
    {
        let mut p = lock(&PACE);
        *p = Pace { started: p.started, asked_since_ns: 0, tick_open_since_ns: 0, last_tick_ns: 0,
            last_tick_done_ns: 0, last_raf_ns: 0, ticks: 0, tick_dones: 0, rafs: 0,
            rafs_in_tick: 0, hidden: false, windows: Vec::new() };
    }
    native::end_run();
}

impl Pace {
    fn tick_done(&mut self) {
        self.tick_open_since_ns = 0;
        self.last_tick_done_ns = crate::clock_ns();
        self.tick_dones += 1;
    }
}

/// A snapshot avoids asking the shell or UI thread why a frame never came.
pub fn pace_lines() -> Vec<String> {
    let p = lock(&PACE).clone();
    if !p.started { return Vec::new(); }
    let now = crate::clock_ns();
    let age = |at| (at != 0).then(|| now.saturating_sub(at) / 1_000_000);
    let raf_age = age(p.last_raf_ns);
    let owed = p.windows.iter().any(|&(_, requested, awaiting, _, _)| requested && !awaiting)
        && p.tick_open_since_ns == 0 && p.last_tick_ns < p.last_raf_ns
        && raf_age.is_some_and(|ms| ms >= 1000);
    let why = if owed {
        format!("the app asked the WM for a frame {} ms ago and no Tick came ({} requests were sent while a Tick was open)", raf_age.unwrap(), p.rafs_in_tick)
    } else { String::new() };
    let number = |at| age(at).map(|n| n.to_string()).unwrap_or_else(|| "null".into());
    let mut lines = vec![format!(
        "{{\"verdict\":\"{}\",\"why\":{},\"asked_ms_ago\":{},\"tick_open_ms\":{},\"last_tick_ms_ago\":{},\"last_tick_done_ms_ago\":{},\"last_raf_ms_ago\":{},\"ticks\":{},\"tick_dones\":{},\"rafs\":{},\"rafs_in_tick\":{},\"hidden\":{}}}",
        if owed { "OWED" } else { "OK" }, makepad_remote::json_str(&why), number(p.asked_since_ns),
        number(p.tick_open_since_ns), number(p.last_tick_ns), number(p.last_tick_done_ns), number(p.last_raf_ns),
        p.ticks, p.tick_dones, p.rafs, p.rafs_in_tick, p.hidden)];
    for (id, requested, awaiting, ahead, in_flight) in p.windows {
        lines.push(format!("{{\"window\":{id},\"requested\":{requested},\"awaiting\":{awaiting},\"ahead\":{ahead},\"in_flight\":{in_flight}}}"));
    }
    lines
}

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
    /// Frames the GPU finished (or that missed the ring), with their originating session.
    finished: Mutex<Vec<(Key, Option<PresentableDraw>, u32)>>,
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
                if let Some((since, said)) = ring.starved.take() {
                    if let Some(w) = shell::snapshot().windows.get_mut(&ring.window_id) { w.starved = None; }
                    if said { eprintln!("[hal-wm] window {}: the WM released a shared image again after {} ms", ring.window_id, since.elapsed().as_millis()); }
                }
                return Some(image);
            }
        }
        if ring.images.iter().all(|s| s.image.is_some() || s.failed) {
            crate::profile::count("hal-wm: no free shared image");
            let window_id = ring.window_id;
            let (since, said) = ring.starved.get_or_insert_with(|| {
                let now = std::time::Instant::now();
                if let Some(w) = shell::snapshot().windows.get_mut(&window_id) { w.starved = Some(now); }
                (now, false)
            });
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
    fn submit(&self, session: u32, window: Id, from: u32, image: Arc<Image>, presented: Box<dyn FnOnce(u64) + Send>) {
        let draw = lock(&self.0.rings).get(&(session, window)).and_then(|ring| {
            let slot = ring.images.iter().find(|s| s.image.as_ref().is_some_and(|i| Arc::ptr_eq(i, &image)))?;
            Some(PresentableDraw { window_id: ring.window_id, target_id: slot.id, width: ring.pixels[0], height: ring.pixels[1],
                sequence: image.hosted_sequence()? })
        });
        if log_enabled() { eprintln!("[hal-wm] frame done {:?}: {draw:?}", (session, window)); }
        let shown = draw.is_some();
        lock(&self.0.finished).push(((session, window), draw, from));
        self.0.shared.signal();
        if shown { presented(crate::clock_ns()); }
    }
    fn scheduled(&self, session: u32, window: Id) {
        *lock(&self.0.in_flight).entry((session, window)).or_default() += 1;
    }
    fn dropped(&self, session: u32, window: Id) {
        if log_enabled() { eprintln!("[hal-wm] frame dropped {:?}: its batch did not run", (session, window)); }
        lock(&self.0.finished).push(((session, window), None, session));
        self.0.shared.signal();
    }
}

/// The hub socket: a websocket client to the WM's hub (the frames hosted apps speak).
struct Hub {
    stream: Mutex<TcpStream>,
    /// A send failed (the WM went away): said once.
    broken: AtomicBool,
    /// An exec hands this connection to the next image: the reader stops at a frame boundary
    /// (`paused`), and no frame is written any more (`frozen`), so the stream stays whole.
    pause: AtomicBool,
    paused: AtomicBool,
    frozen: AtomicBool,
}

/// The connection an exec hands over (`prepare_exec`): the hub, its port and this client's id.
static HANDOVER: Mutex<Option<(Arc<Hub>, u16, u64)>> = Mutex::new(None);
/// The handed-over connection: `<fd> <port> <client> [protocol [wire]]` (older images omit them).
const RESUME_ENV: &str = "MAKEPAD_WM_RESUME";

/// Before an exec of this process (a restart into a rebuilt makepad): the WM connection stays
/// open for the next image, which resumes on it as the same client, so the WM keeps the app's
/// tile and window (no close, no reopen: its content refreshes). The reader stops at a frame
/// boundary and writes stop, so neither side sees half a frame. Empty: nothing to hand over
/// (not WM-hosted, or the reader did not stop in time); the new image then asks the WM socket
/// again with the same pid and gets its client back.
#[relax_api::relax_api(loader = "the loader prepares its exec (the WM's connection crosses it): JIT code has no use for it")]
pub fn prepare_exec() -> Vec<(String, String)> {
    use std::os::fd::AsRawFd;
    let Some((hub, port, client)) = lock(&HANDOVER).clone() else { return Vec::new() };
    if hub.broken.load(Ordering::Acquire) {
        return Vec::new();
    }
    hub.pause.store(true, Ordering::Release);
    let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while !hub.paused.load(Ordering::Acquire) && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    if !hub.paused.load(Ordering::Acquire) {
        hub.pause.store(false, Ordering::Release);
        return Vec::new();
    }
    // a frame being written ends first; none starts after
    let stream = lock(&hub.stream);
    hub.frozen.store(true, Ordering::Release);
    let fd = stream.as_raw_fd();
    if io::close_on_exec(fd, false).is_err() {
        hub.frozen.store(false, Ordering::Release);
        hub.pause.store(false, Ordering::Release);
        return Vec::new();
    }
    let s = shell::snapshot();
    let protocol = s.protocol.map_or(String::new(), |v| {
        let wire = s.wire.map_or(String::new(), |wire| format!(" {wire}"));
        format!(" {v}{wire}")
    });
    vec![(RESUME_ENV.to_string(), format!("{fd} {port} {client}{protocol}"))]
}

/// The exec after `prepare_exec` failed: this process goes on with its connection.
#[relax_api::relax_api(loader = "the runner undoes its exec preparation: JIT code has no use for it")]
pub fn undo_exec() {
    use std::os::fd::AsRawFd;
    let Some((hub, ..)) = lock(&HANDOVER).clone() else { return };
    let _ = io::close_on_exec(lock(&hub.stream).as_raw_fd(), true);
    hub.frozen.store(false, Ordering::Release);
    hub.pause.store(false, Ordering::Release);
}

impl Hub {
    fn publish_link(&self, link: &'static str) {
        let mut s = shell::snapshot();
        if s.hub_id == self as *const Self as usize { s.hub = link; }
    }

    fn connect(port: u16, client: u64, wire: u64, version: u32, name: &str, state: Arc<State>) -> Result<Arc<Hub>, String> {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("hub 127.0.0.1:{port}: {e}"))?;
        let _ = stream.set_nodelay(true);
        let request = format!(
            "GET /app?build={client}&crate={name}&wire={wire:016x}&proto={version} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
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
        let hub = Self::start(stream, state)?;
        *lock(&HANDOVER) = Some((hub.clone(), port, client));
        Ok(hub)
    }

    /// The connection an earlier image of this process handed over (`prepare_exec`): the same
    /// client of the same WM, past its handshake.
    fn resume(state: Arc<State>) -> Option<(Arc<Hub>, u16, u64)> {
        use std::os::fd::FromRawFd;
        let text = std::env::var(RESUME_ENV).ok()?;
        // not for this process's children
        std::env::remove_var(RESUME_ENV);
        let mut w = text.split_whitespace().map(|x| x.parse::<u64>().ok());
        let (Some(Some(fd)), Some(Some(port)), Some(Some(client))) = (w.next(), w.next(), w.next()) else { return None };
        let fd = i32::try_from(fd).ok()?;
        let protocol = w.next().flatten().and_then(|v| u32::try_from(v).ok());
        let wire = w.next().flatten();
        // the descriptor must still be a connected socket (else: ask the WM socket)
        let stream = unsafe { TcpStream::from_raw_fd(fd) };
        if stream.peer_addr().is_err() {
            std::mem::forget(stream);
            return None;
        }
        let _ = io::close_on_exec(fd, true);
        let port = u16::try_from(port).ok()?;
        let hub = Self::start(stream, state).ok()?;
        {
            let mut s = shell::snapshot();
            s.admitted = Some(client);
            s.protocol = protocol;
            s.wire = wire;
        }
        *lock(&HANDOVER) = Some((hub.clone(), port, client));
        Some((hub, port, client))
    }

    fn start(stream: TcpStream, state: Arc<State>) -> Result<Arc<Hub>, String> {
        let reader = stream.try_clone().map_err(|e| e.to_string())?;
        let hub = Arc::new(Hub { stream: Mutex::new(stream), broken: AtomicBool::new(false), pause: AtomicBool::new(false), paused: AtomicBool::new(false), frozen: AtomicBool::new(false) });
        {
            let mut s = shell::snapshot();
            s.hub_id = Arc::as_ptr(&hub) as usize;
            s.hub = "connected";
        }
        let pong = hub.clone();
        std::thread::Builder::new().name("wm-hub-reader".into()).spawn(move || {
            read_frames(reader, &pong, &state);
            pong.publish_link("broken");
            state.hung_up.store(true, Ordering::Release);
            state.shared.signal();
        }).map_err(|e| { hub.publish_link("broken"); e.to_string() })?;
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
        let mut stream = lock(&self.stream);
        // handed to the next image: nothing more from this one
        if self.frozen.load(Ordering::Acquire) {
            return Ok(());
        }
        stream.write_all(&out)
    }

    fn send(&self, msgs: Vec<AppToStudio>) -> bool {
        if msgs.is_empty() { return true; }
        if self.broken.load(Ordering::Acquire) { return false; }
        if let Err(e) = self.frame(0x2, &AppToStudioVec(msgs).serialize_bin()) {
            self.broken.store(true, Ordering::Release);
            self.publish_link("broken");
            eprintln!("[hal-wm] the WM's hub socket failed: {e}");
            return false;
        }
        true
    }

    fn close(&self) {
        let _ = self.frame(0x8, &[]);
        let _ = lock(&self.stream).shutdown(std::net::Shutdown::Both);
        self.publish_link("broken");
    }
}

fn read_frames(mut stream: TcpStream, hub: &Hub, state: &State) {
    let mut message = Vec::new();
    // between frames the reader looks up now and then: an exec may want the stream whole
    let idle = Some(std::time::Duration::from_millis(50));
    let _ = stream.set_read_timeout(idle);
    loop {
        if hub.pause.load(Ordering::Acquire) && message.is_empty() {
            hub.paused.store(true, Ordering::Release);
            while hub.pause.load(Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            hub.paused.store(false, Ordering::Release);
        }
        let mut first = [0u8; 1];
        match stream.peek(&mut first) {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => continue,
            Err(_) => return,
        }
        // a frame has begun: read it whole
        let _ = stream.set_read_timeout(None);
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
                if !fin { let _ = stream.set_read_timeout(idle); continue; }
                let data = std::mem::take(&mut message);
                let msgs = match StudioToAppVec::decode_bin(&data) {
                    Ok((batch, errors)) => {
                        for e in errors { eprintln!("[hal-wm] {e}"); }
                        batch.0
                    }
                    Err(_) => match makepad_micro_serde::DeBin::deserialize_bin(&data) {
                        Ok(msg) => vec![msg],
                        Err(e) => { eprintln!("[hal-wm] undecodable host message: {e:?}"); let _ = stream.set_read_timeout(idle); continue; }
                    },
                };
                lock(&state.inbox).extend(msgs);
                state.shared.signal();
            }
            _ => {}
        }
        let _ = stream.set_read_timeout(idle);
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
    /// The app shows none of its own window buttons in it now (`OsCmd::WindowButtons`): the WM
    /// gives it its own (`hosted_window_op::HOST_BUTTONS`), said again to a WM that hosts this
    /// process anew.
    host_buttons: bool,
}

struct HeldButtons {
    bits: u32,
    /// The recipient's origin in the WM's pointer coordinates.
    origin: (f64, f64),
}

#[derive(Default)]
struct HeldInput {
    keys: Vec<(Key, u32)>,
    buttons: HashMap<Key, HeldButtons>,
    pointer: (f64, f64),
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
    /// An app session's first frame reached the WM; replayed when a new WM hosts this process.
    app_drawn: bool,
    /// The WM is gone (detached): no frames are asked for or granted until the next one. Whether
    /// the WM shows the tile (`HostedVisible`) is the app's pacing, not this flag's.
    hidden: bool,
    /// The clipboard is the WM's (shared with every app and the Wayland clients): reads go to the
    /// WM as relays; relay id -> (session, its request id).
    clip_reads: HashMap<u64, (u32, u64)>,
    next_clip_read: u64,
    /// The WM echoes the relay id; each session numbers its own dialog requests.
    dialogs: HashMap<u64, (u32, u32)>,
    next_dialog: u64,
    /// Application messages waiting for the next WM, in send order across sessions.
    host_messages: VecDeque<(u32, Vec<u8>)>,
    /// Latest title and folder per session, in the order they were set.
    host_state: Vec<(u32, &'static str, Vec<u8>)>,
    /// The sessions that sent the WM an application message: until an app session holds the input
    /// window (none is made yet, or a run's boot window is not lent yet), the WM's messages that
    /// are no state reach them as they come, as the answer to what they sent.
    speakers: Vec<u32>,
    /// A read's richer flavours the WM sent just ahead of its answer (`HostedClipboard`).
    clip_flavours: Option<HostedClipboard>,
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
    /// The socket notice of the WM this process is (or was last) attached to, read as it attached
    /// (a WM's clean quit removes it): a reattach joins only a WM of its kind ([`may_reattach`]).
    host_notice: Option<String>,
    /// The WMs a reattach passed over, with why (each logged once per detach).
    passed_over: Vec<String>,
    /// Keys and buttons down at their recipients: a detach releases them.
    held: HeldInput,
    /// This WM asked the process to close its windows.
    killed: bool,
    quit_sent: bool,
    stopping: bool,
}

impl App {
    pub(super) fn connect(shared: &'static Shared) -> Result<Self, String> {
        let path = wm_socket::discovery_path().ok_or("no WM socket path (XDG_RUNTIME_DIR unset)")?;
        let name = wm_socket::client_name();
        let state = Arc::new(State { shared, inbox: Default::default(), hung_up: AtomicBool::new(false), detached: AtomicBool::new(false), rings: Default::default(),
            finished: Default::default(), in_flight: Default::default(), device: Default::default() });
        STATE.set(state.clone()).map_err(|_| "WM-hosted shell already connected")?;
        // a restart of this process (exec) handed its WM connection over: the same client, tile
        // and window, without asking the socket
        let (hub, port, client) = match Hub::resume(state.clone()) {
            Some((hub, port, client)) => {
                eprintln!("[hal-wm] resumed the WM connection of this process's previous image as client {client} ({name})");
                (hub, port, client)
            }
            None => {
                let admission = wm_socket::adopt(&path, WIRE, std::process::id(), &name)
                    .map_err(|probe| format!("the makepad WM at {} did not adopt this process: {probe:?}", path.display()))?;
                {
                    let mut s = shell::snapshot();
                    s.protocol = Some(admission.version);
                    s.wire = Some(admission.wire);
                    s.admitted = Some(admission.client);
                }
                let hub = Hub::connect(admission.port, admission.client, admission.wire, admission.version, &name, state.clone())?;
                eprintln!("[hal-wm] the makepad WM at {} hosts this process as client {} ({name}, hosted protocol {})", path.display(), admission.client, admission.version);
                (hub, admission.port, admission.client)
            }
        };
        a11y::start(hub.clone());
        // Under the WM a makepad window draws its own caption (the WM draws title bars only for
        // foreign Wayland windows): the app's windows get client-side chrome as on a Wayland
        // compositor without server decorations. Its caption's drag query and its window
        // buttons go to the WM through this shell (`HostedDragQuery`, `HostedWindowOp`).
        shared.client_chrome.store(true, Ordering::Release);
        lock(&PACE).started = true;
        Ok(Self { state, hub, aux_path: hosted_ipc::aux_path(port, &client.to_string()), aux: None,
            pending: Vec::new(), windows: HashMap::new(), early: HashMap::new(), next_window: 0, ticking: false, asked_frame: false, app_drawn: false, hidden: false,
            clip_reads: HashMap::new(), next_clip_read: 0, clip_flavours: None, selection: Default::default(), drag_queries: VecDeque::new(), wm_state: Vec::new(), wm_focus: HashMap::new(), trays: HashMap::new(),
            dialogs: HashMap::new(), next_dialog: 0, host_messages: VecDeque::new(), host_state: Vec::new(), speakers: Vec::new(),
            hidden_windows: Default::default(), detached_at: None, reattach_error: None, host_notice: read_notice(&path), passed_over: Vec::new(), held: HeldInput::default(), killed: false, quit_sent: false, stopping: false })
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
        let Some(aux) = &self.aux else {
            if let Some(w) = shell::snapshot().windows.get_mut(&swapchain.window_id) { w.pending_swapchain = true; }
            self.pending.push(swapchain);
            return;
        };
        let Some(key) = self.key_of(swapchain.window_id) else { return };
        if let Some(w) = shell::snapshot().windows.get_mut(&swapchain.window_id) {
            w.pending_swapchain = self.pending.iter().any(|s| s.window_id == swapchain.window_id);
        }
        let images: Result<Vec<_>, _> = swapchain.presentable_images.iter().map(|image| {
            hosted_ipc::recv_image(aux, *image, swapchain.alloc_width, swapchain.alloc_height)
                .map(|native| RingImage { id: image.id, native: Arc::new(native), image: None, failed: false })
        }).collect();
        match images {
            Ok(images) => {
                if log_enabled() { eprintln!("[hal-wm] window {} has {} shared images of {}x{}", swapchain.window_id, images.len(), swapchain.alloc_width, swapchain.alloc_height); }
                let pixels = self.windows.get(&key).and_then(|w| w.geom).map_or([swapchain.alloc_width, swapchain.alloc_height], |(g, dpi, _)| crate::window_pixels([g[2], g[3]], dpi));
                let mut rings = lock(&self.state.rings);
                // The host says a swapchain again until a frame confirms it: the same allocations
                // stay as they are. A new ring would strand the frames in flight: they no longer
                // find their image, never reach the WM, and the image they took never comes back,
                // so a few such frames in a row leave a window that draws and never presents.
                let same = rings.get(&key).is_some_and(|ring| ring.images.len() == images.len()
                    && ring.images.iter().zip(&images).all(|(old, new)| old.id == new.id));
                if !same {
                    rings.insert(key, Ring { window_id: swapchain.window_id, alloc: [swapchain.alloc_width, swapchain.alloc_height],
                        pixels, images, next: 0, starved: None });
                }
                if let Some(w) = shell::snapshot().windows.get_mut(&swapchain.window_id) {
                    w.pixels = pixels;
                    w.starved = None;
                }
                drop(rings);
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
                if let Some(ring) = lock(&self.state.rings).get_mut(&key) { ring.pixels = crate::window_pixels([geom[2], geom[3]], dpi_factor); }
                if let Some(w) = shell::snapshot().windows.get_mut(&window_id) { w.pixels = crate::window_pixels([geom[2], geom[3]], dpi_factor); }
                if resized { self.push_geom(key); }
            }
            StudioToApp::Tick => {
                {
                    let now = crate::clock_ns();
                    let mut pace = lock(&PACE);
                    pace.asked_since_ns = 0;
                    if pace.tick_open_since_ns == 0 { pace.tick_open_since_ns = now; }
                    pace.last_tick_ns = now;
                    pace.ticks += 1;
                }
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
                    lock(&PACE).tick_done();
                    self.hub.send(vec![AppToStudio::TickDone]);
                }
            }
            StudioToApp::MouseDown(e) => {
                let Some((recipient, x, y)) = self.pointer_at(e.x, e.y) else { return };
                // A repeated down after a lost release transfers this bit to its new recipient.
                self.held.buttons.retain(|owner, held| {
                    if *owner != recipient { held.bits &= !e.button_raw_bits; }
                    held.bits != 0
                });
                let held = self.held.buttons.entry(recipient).or_insert(HeldButtons { bits: 0, origin: (e.x - x, e.y - y) });
                held.bits |= e.button_raw_bits;
                self.state.shared.push(recipient, |q, id| q.mouse_down(id, x, y, e.button_raw_bits, mods(&e.modifiers)));
            }
            StudioToApp::MouseMove(e) => self.pointer_to(e.x, e.y, |q, id, x, y| q.mouse_move(id, x, y, 0.0, 0.0, mods(&e.modifiers))),
            StudioToApp::MouseUp(e) | StudioToApp::MouseCancel(e) => {
                let owner = self.held.buttons.iter_mut().find(|(_, held)| held.bits & e.button_raw_bits != 0);
                let (recipient, x, y) = if let Some((&recipient, held)) = owner {
                    let (left, top) = held.origin;
                    held.bits &= !e.button_raw_bits;
                    if held.bits == 0 { self.held.buttons.remove(&recipient); }
                    self.held.pointer = (e.x, e.y);
                    (recipient, e.x - left, e.y - top)
                } else if let Some(point) = self.pointer_at(e.x, e.y) {
                    point
                } else {
                    return;
                };
                let release = if matches!(msg, StudioToApp::MouseCancel(_)) {
                    crate::buffers::EventQueue::mouse_cancel
                } else {
                    crate::buffers::EventQueue::mouse_up
                };
                self.state.shared.push(recipient, |q, id| release(q, id, x, y, e.button_raw_bits, mods(&e.modifiers)));
            }
            StudioToApp::Scroll(e) => self.pointer_to(e.x, e.y, |q, id, x, y| q.scroll(id, x, y, e.sx, e.sy, mods(&e.modifiers), !e.is_mouse, phase(e.phase))),
            StudioToApp::Pinch(e) => {
                let phase = match e.phase { PinchPhase::Begin => input::pinch::BEGIN, PinchPhase::Update => input::pinch::UPDATE, PinchPhase::End => input::pinch::END };
                self.pointer_to(e.x, e.y, |q, id, x, y| q.pinch(id, x, y, e.scale, phase));
            }
            StudioToApp::KeyDown(e) | StudioToApp::KeyUp(e) => {
                let Some(recipient) = self.input_recipient() else { return };
                let key = e.key_code.to_index();
                let m = key_mods(e.modifiers.shift, e.modifiers.control, e.modifiers.alt, e.modifiers.logo);
                if matches!(msg, StudioToApp::KeyDown(_)) {
                    if !self.held.keys.contains(&(recipient, key)) { self.held.keys.push((recipient, key)); }
                    self.state.shared.push(recipient, |q, id| q.key_down(id, key, e.is_repeat, m));
                } else {
                    self.held.keys.retain(|entry| *entry != (recipient, key));
                    self.state.shared.push(recipient, |q, id| q.key_up(id, key, m));
                }
            }
            StudioToApp::TextInput(e) => {
                let kind = if e.was_paste { input::text::PASTE } else { input::text::TYPED };
                self.pointer(|q, id| q.text(id, kind, e.input.as_bytes()));
            }
            // The WM closed the tile: each window is asked to close, as a window manager's
            // close button asks (the app may save first; `makepad run` ends the run).
            // With no window left to ask (the app closed its own), the process is asked to quit.
            StudioToApp::Kill => {
                self.killed = true;
                if self.windows.is_empty() {
                    self.quit();
                }
                for key in self.windows.keys() {
                    self.state.shared.push(*key, |q, id| q.close_requested(id));
                }
            }
            StudioToApp::Custom(json) => {
                // A clipboard read's flavours, just ahead of its answer.
                if let Some(flavours) = HostedClipboard::parse(&json) {
                    self.clip_flavours = Some(flavours);
                    return;
                }
                // A press in the window: the app's drag query says whether it is its caption.
                if let Some(query) = HostedDragQuery::parse(&json) {
                    if let Some((key, x, y)) = self.pointer_at(query.x, query.y) {
                        self.drag_queries.push_back(query.id);
                        self.state.shared.push(key, |q, id| q.drag_query(id, x, y));
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
                // The tile's visibility is the app's pacing, not this shell's: the app holds its
                // frames while nothing shows, except those a remote request (a grab, a step) needs,
                // so a covered window still answers a grab. It goes on to the app below.
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
                let recipient = self.input_recipient();
                if let Some(key) = recipient {
                    self.state.shared.push(key, |q, id| q.app_message(id, json.as_bytes()));
                }
                // Until an app session holds the input window (none is made yet, or a run's boot
                // window is not lent yet), the sessions that spoke to the WM hear its other
                // messages too: the answer to what they sent (the AI bus's `Registered` to a port
                // opened at startup). State waits for the window, which hears its latest value.
                if recipient.is_none_or(|(s, _)| s == session::HOST) && wm_state_name(&json).is_none() {
                    for &sid in &self.speakers {
                        self.state.shared.push((sid, Id::NONE), |q, id| q.app_message(id, json.as_bytes()));
                    }
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
                // the flavours the host sent just ahead, when they go with this text
                let flavours = self.clip_flavours.take().filter(|f| f.request == request_id && f.text == text).unwrap_or_default();
                if let Some((sid, req)) = self.clip_reads.remove(&request_id) {
                    let (html, markdown) = (flavours.html, flavours.markdown);
                    let bytes = crate::clipboard::ClipboardData { text, html, markdown, png, files: Vec::new() }.pack();
                    self.state.shared.push((sid, Id::NONE), |q, _| q.clipboard(req, true, &bytes));
                }
            }
            StudioToApp::Relay(HostRelay::FileDialog(result)) => {
                if let Some((sid, req)) = self.dialogs.remove(&result.id) {
                    let paths: Vec<&str> = result.paths.iter().map(String::as_str).collect();
                    let bytes = crate::buffers::pack_strs(&paths);
                    self.state.shared.push((sid, Id::NONE), |q, _| q.dialog_result(req, paths.len() as u32, &bytes));
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

    /// The window a pointer message at `x, y` (the child's window space, where a later window
    /// sits at its own origin: its geometry's left and top) is for, and the point in it. A point
    /// in no window is the first window's, as the native hosted path does (`window_id_contains`).
    fn pointer_at(&mut self, x: f64, y: f64) -> Option<(Key, f64, f64)> {
        self.held.pointer = (x, y);
        let inside = self.windows.values().filter(|w| !w.dropdown).find_map(|w| {
            let ([left, top, width, height], _, _) = w.geom?;
            (x >= left && y >= top && x <= left + width && y <= top + height).then_some((w.recipient, x - left, y - top))
        });
        let (recipient, local_x, local_y) = inside.or_else(|| self.input_recipient().map(|key| (key, x, y)))?;
        if let Some(held) = self.held.buttons.get_mut(&recipient) {
            held.origin = (x - local_x, y - local_y);
        }
        Some((recipient, local_x, local_y))
    }

    /// A pointer message at `x, y` to the window it is in, with the point in that window.
    fn pointer_to(&mut self, x: f64, y: f64, f: impl FnOnce(&mut crate::buffers::EventQueue, Id, f64, f64)) {
        if let Some((key, x, y)) = self.pointer_at(x, y) {
            self.state.shared.push(key, |q, id| f(q, id, x, y));
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
        // A request during a Tick goes out with its TickDone: the WM takes a request
        // that arrives while its Tick is open as served by that Tick.
        if self.hidden || self.asked_frame || self.ticking { return; }
        self.asked_frame = true;
        {
            let now = crate::clock_ns();
            let mut pace = lock(&PACE);
            pace.asked_since_ns = now;
            pace.last_raf_ns = now;
            pace.rafs += 1;
            if self.ticking { pace.rafs_in_tick += 1; }
        }
        self.hub.send(vec![AppToStudio::RequestAnimationFrame]);
    }

    fn finish_tick(&mut self) {
        // The Tick stays open while a granted frame is awaited: its TickDone goes out with that
        // frame (taking `ticking` first lost the TickDone, and the WM paced the app at its 50 ms
        // fallback).
        if self.ticking && !self.windows.values().any(|w| w.awaiting) {
            self.ticking = false;
            lock(&PACE).tick_done();
            self.hub.send(vec![AppToStudio::TickDone]);
            let in_flight = lock(&self.state.in_flight);
            let wanted = self.windows.iter().any(|(key, w)| {
                w.requested && !w.awaiting && in_flight.get(key).copied().unwrap_or(0) < MAX_IN_FLIGHT
            });
            drop(in_flight);
            if wanted { self.ask_frame(); }
        }
    }

    /// Reuse the window storage so observing pacing adds no allocation to a steady turn.
    fn publish_pace(&self) {
        let in_flight = lock(&self.state.in_flight);
        let mut pace = lock(&PACE);
        pace.started = true;
        pace.hidden = self.hidden;
        // Detaching abandons the old WM's open Tick and unanswered request.
        if !self.ticking { pace.tick_open_since_ns = 0; }
        if !self.asked_frame { pace.asked_since_ns = 0; }
        pace.windows.clear();
        pace.windows.extend(self.windows.iter().map(|(key, w)| {
            (w.window_id, w.requested, w.awaiting, w.ahead, in_flight.get(key).copied().unwrap_or(0))
        }));
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

/// Only the title and folder in the app's WM request envelope survive the connection.
fn wm_request_state(message: &[u8]) -> Option<&'static str> {
    use makepad_micro_serde::{DeJson, JsonValue};
    let key = message.trim_ascii_start().strip_prefix(b"{")?.trim_ascii_start();
    if !key.starts_with(b"\"wm\"") { return None; }
    let envelope = JsonValue::deserialize_json(std::str::from_utf8(message).ok()?).ok()?;
    let request = envelope.get("wm")?.object()?;
    if request.len() != 1 { return None; }
    for (kind, field) in [("Title", "title"), ("Cwd", "path")] {
        if request.get(kind).and_then(|body| body.get(field)).and_then(JsonValue::as_str).is_some() {
            return Some(kind);
        }
    }
    None
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

/// Whether the WM's state file (`<socket>.state`) asks its apps to outlive it: a `keep=1` line.
fn state_keeps_apps(state: &str) -> bool {
    state.lines().any(|line| line == "keep=1")
}

/// The kind of WM a socket notice names (its `lane=` line): empty for a node's own WM, else the
/// lane of a test desktop.
fn notice_lane(state: &str) -> &str {
    state.lines().find_map(|line| line.strip_prefix("lane=")).unwrap_or("")
}

/// Whether a process that left the WM whose notice was `left` may join the WM whose notice is
/// `next`: only one of the same kind, so a node's apps never land in a lane's test WM (and die
/// with it) nor a test WM's apps on the node's; and one that keeps its apps when the old one
/// kept this process. A process whose old WM published none joins no WM (it quits at the
/// hang-up instead, [`detach_on_hangup`]).
fn may_reattach(left: Option<&str>, next: Option<&str>) -> Result<(), String> {
    let Some(left) = left else { return Err("the WM this process left published no host notice (an older build)".into()) };
    let Some(next) = next else { return Err("it publishes no host notice (an older WM)".into()) };
    let kind = |lane: &str| if lane.is_empty() { "the node's own WM".to_string() } else { format!("a test WM of lane {lane}") };
    let (was, is) = (notice_lane(left), notice_lane(next));
    if was != is {
        return Err(format!("it is {}, and this process's was {}", kind(is), kind(was)));
    }
    if state_keeps_apps(left) && !state_keeps_apps(next) {
        return Err("it does not keep its apps, and this process was kept".into());
    }
    Ok(())
}

/// Whether a process whose WM hung up waits for the next one: it has a tray, or its WM asked
/// for its apps to be kept and did not Kill this one; and that WM published its notice as this
/// process attached (`known`), without which no next WM can be told to be of its kind
/// ([`may_reattach`]). Otherwise it quits.
fn detach_on_hangup(has_tray: bool, killed: bool, keep: bool, known: bool) -> bool {
    known && (has_tray || (!killed && keep))
}

/// The notice a WM publishes beside its socket (`<socket>.state`), if any.
fn read_notice(socket: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(socket.with_added_extension("state")).ok()
}

/// How often a detached process looks for the next WM.
const REATTACH_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

impl App {
    fn send_host_message(&self, message: &[u8]) -> bool {
        if self.detached_at.is_some() || self.state.hung_up.load(Ordering::Acquire) || self.hub.broken.load(Ordering::Acquire) {
            return false;
        }
        self.hub.send(vec![AppToStudio::Custom(String::from_utf8_lossy(message).into_owned())])
    }

    fn send_file_dialog(&mut self, kind: u32, title: String, location: String, filename: String, filters: Vec<String>) -> Option<u64> {
        if self.detached_at.is_some() || self.hub.broken.load(Ordering::Acquire) { return None; }
        let name = match kind & 0xff {
            dialog_kind::SELECT_FILE => "select_file",
            dialog_kind::SAVE_FILE => "save_file",
            dialog_kind::SELECT_FOLDER => "select_folder",
            dialog_kind::SAVE_FOLDER => "save_folder",
            _ => return None,
        };
        self.next_dialog += 1;
        let id = self.next_dialog;
        let filters = filters.chunks(2).map(|pair| {
            let extensions = pair.get(1).map(|s| s.split(',').map(str::to_owned).collect()).unwrap_or_default();
            (pair[0].clone(), extensions)
        }).collect();
        let dialog = RelayFileDialog {
            id, kind: name.into(), multiple: kind & dialog_kind::MULTIPLE != 0, filters,
            title: (!title.is_empty()).then_some(title),
            location: (!location.is_empty()).then_some(location),
            filename: (!filename.is_empty()).then_some(filename),
        };
        self.hub.send(vec![AppToStudio::Relay(ChildRelay::FileDialog(dialog))]);
        (!self.hub.broken.load(Ordering::Acquire)).then_some(id)
    }

    /// Release the old WM's shared images and wait for the next WM. While drawing is
    /// suspended, timers, signals and the app's work go on.
    fn detach(&mut self) {
        for (_, (sid, req)) in self.dialogs.drain() {
            self.state.shared.push((sid, Id::NONE), |q, _| q.dialog_result(req, 0, &[]));
        }
        for (recipient, key) in self.held.keys.drain(..) {
            self.state.shared.push(recipient, |q, id| q.key_up(id, key, 0));
        }
        let (x, y) = self.held.pointer;
        for (recipient, held) in self.held.buttons.drain() {
            let mut bits = held.bits;
            while bits != 0 {
                let button = 1u32 << bits.trailing_zeros();
                self.state.shared.push(recipient, |q, id| q.mouse_up(id, x - held.origin.0, y - held.origin.1, button, 0));
                bits &= !button;
            }
        }
        a11y::detached();
        eprintln!("[hal-wm] the makepad WM went away; this process waits for the next WM");
        self.detached_at = Some(std::time::Instant::now());
        self.passed_over.clear();
        lock(&self.state.rings).clear();
        {
            let mut s = shell::snapshot();
            s.detached = true;
            for w in s.windows.values_mut() { w.starved = None; w.pending_swapchain = false; }
        }
        lock(&self.state.in_flight).clear();
        self.pending.clear();
        self.aux = None;
        self.ticking = false;
        self.asked_frame = false;
        self.hidden = true;
        self.drag_queries.clear();
        self.early.clear();
        for w in self.windows.values() {
            self.state.shared.push(w.recipient, |q, id| q.focus(id, false));
        }
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

    /// A detached process asks the WM sockets again, passing over WMs of another kind than the one
    /// it left; a live WM adopts it as a new client: its windows are announced again (a hidden one stays minimised) and its trays shown again.
    fn reattach(&mut self) {
        if self.detached_at.is_some_and(|at| at.elapsed() < REATTACH_EVERY) { return; }
        self.detached_at = Some(std::time::Instant::now());
        let name = wm_socket::client_name();
        let mut found = None;
        let mut last = String::from("no WM socket to ask");
        for path in wm_socket::reattach_paths() {
            let notice = read_notice(&path);
            if let Err(why) = may_reattach(self.host_notice.as_deref(), notice.as_deref()) {
                last = format!("{}: {why}", path.display());
                if !self.passed_over.contains(&last) {
                    eprintln!("[hal-wm] passed over the WM at {last}");
                    self.passed_over.push(last.clone());
                }
                continue;
            }
            {
                let mut s = shell::snapshot();
                s.wm_socket = Some(path.to_string_lossy().into_owned());
                s.protocol = None;
                s.wire = None;
                s.admitted = None;
            }
            match wm_socket::adopt(&path, WIRE, std::process::id(), &name) {
                Ok(admission) => { found = Some((path, admission, notice)); break; }
                Err(e) => last = format!("{}: {e:?}", path.display()),
            }
        }
        let Some((path, admission, notice)) = found else {
            shell::snapshot().refused = Some(last.clone());
            // once per distinct reason, so a long wait is visible without a line a second
            if self.reattach_error.as_deref() != Some(last.as_str()) {
                eprintln!("[hal-wm] still waiting for the next WM: {last}");
                self.reattach_error = Some(last);
            }
            return;
        };
        self.reattach_error = None;
        self.host_notice = notice;
        {
            let mut s = shell::snapshot();
            s.protocol = Some(admission.version);
            s.wire = Some(admission.wire);
            s.admitted = Some(admission.client);
            s.refused = None;
        }
        self.state.hung_up.store(false, Ordering::Release);
        let hub = match Hub::connect(admission.port, admission.client, admission.wire, admission.version, &name, self.state.clone()) {
            Ok(hub) => hub,
            Err(e) => {
                shell::snapshot().refused = Some(e.clone());
                eprintln!("[hal-wm] the next WM admitted this process but its hub did not take it: {e}");
                return;
            }
        };
        eprintln!("[hal-wm] the makepad WM at {} hosts this process again as client {} ({name}, hosted protocol {})", path.display(), admission.client, admission.version);
        a11y::start(hub.clone());
        self.aux_path = hosted_ipc::aux_path(admission.port, &admission.client.to_string());
        let sessions = crate::Hal::global().with_world(|world| world.session_ids());
        self.finish_reattach(hub, &sessions);
    }

    fn announce_window(&self, window_id: usize, dropdown: bool) {
        self.hub.send(vec![
            AppToStudio::CreateWindow { window_id, kind_id: if dropdown { window_kind::DROPDOWN as usize } else { 0 } },
            AppToStudio::Custom(POINTER_CAPS_JSON.into()),
        ]);
    }

    /// Announce this process to the replacement hub before replaying its apps' messages.
    fn finish_reattach(&mut self, hub: Arc<Hub>, sessions: &[u32]) {
        self.hub = hub;
        self.killed = false;
        self.detached_at = None;
        self.state.detached.store(false, Ordering::Release);
        self.hidden = false;
        shell::snapshot().detached = false;
        let mut windows: Vec<(Key, usize, bool)> = self.windows.iter().map(|(k, w)| (*k, w.window_id, w.dropdown)).collect();
        windows.sort_by_key(|(_, id, _)| *id);
        for (key, window_id, dropdown) in windows {
            self.announce_window(window_id, dropdown);
            if self.hidden_windows.contains(&key) {
                self.window_op(window_op::MINIMIZE);
            }
            if let Some(w) = self.windows.get_mut(&key) { w.requested = true; }
        }
        let buttons: Vec<AppToStudio> = self.windows.values().filter(|w| w.host_buttons).map(|w| AppToStudio::Custom(HostedWindow { x: 1.0, ..HostedWindow::new(w.window_id, hosted_window_op::HOST_BUTTONS) }.to_json())).collect();
        self.hub.send(buttons);
        let trays: Vec<AppToStudio> = self.trays.values().map(|(_, t)| AppToStudio::Relay(ChildRelay::SetTray(t.clone()))).collect();
        self.hub.send(trays);
        for (_, _, message) in &self.host_state {
            self.send_host_message(message);
        }
        if !self.host_messages.is_empty() {
            eprintln!("[hal-wm] kept {} app messages while detached", self.host_messages.len());
            while let Some((_, message)) = self.host_messages.front() {
                if !self.send_host_message(message) { break; }
                self.host_messages.pop_front();
            }
        }
        if self.app_drawn { self.hub.send(vec![AppToStudio::Custom(HostedAppFrame.to_json())]); }
        for &sid in sessions {
            self.state.shared.push((sid, Id::NONE), |q, id| q.app_message(id, wm_socket::HOST_ATTACHED.as_bytes()));
        }
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
                    let old = self.windows.insert((sid, id), Win { window_id, recipient: (sid, id), requested: false, awaiting: false, ahead: 0, geom, dropdown, host_buttons: false });
                    if let Some(w) = &old {
                        self.held.buttons.remove(&w.recipient);
                        self.state.shared.push(w.recipient, |q, id| q.focus(id, false));
                    }
                    {
                        let mut s = shell::snapshot();
                        if let Some(w) = old { s.windows.remove(&w.window_id); }
                        s.windows.insert(window_id, shell::Window {
                            pixels: geom.map_or([0, 0], |(g, dpi, _)| crate::window_pixels([g[2], g[3]], dpi)),
                            hidden: self.hidden_windows.contains(&(sid, id)),
                            pending_swapchain: self.pending.iter().any(|s| s.window_id == window_id),
                            ..Default::default()
                        });
                    }
                    shell::window_made();
                    self.announce_window(window_id, dropdown);
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
                        self.held.buttons.remove(&w.recipient);
                        shell::snapshot().windows.remove(&w.window_id);
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
                        } else {
                            // A later window (an output window) is a tile of its own: the WM
                            // takes it off the desk, or it stays there with its last frame.
                            self.hub.send(vec![AppToStudio::Custom(HostedWindow::new(w.window_id, hosted_window_op::CLOSED).to_json())]);
                        }
                    }
                }
                // Whether the app shows window buttons of its own in the window now: the WM gives
                // it its own where it does not (its plain caption over a full-screen window).
                OsCmd::WindowButtons { id, show } => {
                    let Some(key) = self.resolve((sid, id)) else { return };
                    let Some(w) = self.windows.get_mut(&key).filter(|w| !w.dropdown && w.host_buttons != show) else { return };
                    w.host_buttons = show;
                    let window_id = w.window_id;
                    self.hub.send(vec![AppToStudio::Custom(HostedWindow { x: show as u8 as f64, ..HostedWindow::new(window_id, hosted_window_op::HOST_BUTTONS) }.to_json())]);
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
                    // A later window (an output window) is a tile of its own on the desk: what
                    // the app asks of it names it (`HostedWindow`), the tile's own ops do not.
                    if self.tile_window() != Some(key) {
                        let op = match state {
                            window_state::MINIMIZE => Some(hosted_window_op::MINIMIZE),
                            window_state::MAXIMIZE => Some(hosted_window_op::MAXIMIZE),
                            window_state::RESTORE => Some(hosted_window_op::RESTORE),
                            window_state::FULLSCREEN => Some(hosted_window_op::FULLSCREEN),
                            _ => None,
                        };
                        if let Some(op) = op {
                            let window_id = self.windows[&key].window_id;
                            self.hub.send(vec![AppToStudio::Custom(HostedWindow::new(window_id, op).to_json())]);
                            return;
                        }
                    }
                    match state {
                        // A hidden window (a tray app's closed one) is a minimised tile.
                        window_state::HIDE => {
                            self.hidden_windows.insert(key);
                            if let Some(w) = shell::snapshot().windows.get_mut(&self.windows[&key].window_id) { w.hidden = true; }
                            self.window_op(window_op::MINIMIZE);
                        }
                        window_state::MINIMIZE => self.window_op(window_op::MINIMIZE),
                        window_state::SHOW => {
                            self.hidden_windows.remove(&key);
                            if let Some(w) = shell::snapshot().windows.get_mut(&self.windows[&key].window_id) { w.hidden = false; }
                            self.window_op(window_op::SHOW);
                        }
                        window_state::MAXIMIZE => self.window_op(window_op::MAXIMIZE),
                        window_state::RESTORE => self.window_op(window_op::RESTORE),
                        window_state::FULLSCREEN => self.window_op(WINDOW_OP_FULLSCREEN),
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
                OsCmd::NotifyPost { .. } | OsCmd::NotifyRemove { .. } => {}
                // A tray dropdown: the WM places the window under the tray's bar item.
                OsCmd::WindowAtTray { id, tray, w, h } => {
                    let Some(key) = self.resolve((sid, id)) else { return };
                    let window_id = self.windows[&key].window_id;
                    let show = HostedDropdown { tray, w: w as f64, h: h as f64, ..HostedDropdown::new(window_id, dropdown_op::SHOW) };
                    self.hub.send(vec![AppToStudio::Custom(show.to_json())]);
                }
                OsCmd::DragReply { to: input::reply::QUERY, answer, fullscreen, .. } => {
                    if let Some(id) = self.drag_queries.pop_front() {
                        let response = u8::try_from(answer).unwrap_or(drag_answer::NO_ANSWER);
                        // what a double click there does is the window's own declaration
                        let double_click = if fullscreen { drag_double::FULLSCREEN } else { drag_double::MAXIMISE };
                        self.hub.send(vec![AppToStudio::Custom(HostedDragAnswer { id, response, double_click }.to_json())]);
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
                // The app resizes a window (`Window::resize`, the remote `resize`): the WM owns
                // every tile's size, so the request goes to it. The tile asks with
                // `HostedResize`; a later window is a tile of its own and asks with its id.
                OsCmd::WindowGeom { id, geom } => {
                    let Some(key) = self.resolve((sid, id)) else { return };
                    let Some(w) = self.windows.get(&key).filter(|w| !w.dropdown) else { return };
                    if geom[2] < 1.0 || geom[3] < 1.0 { return }
                    let (width, height) = (geom[2] as f64, geom[3] as f64);
                    let ask = if self.tile_window() == Some(key) {
                        HostedResize { w: width, h: height }.to_json()
                    } else {
                        HostedWindow { x: width, y: height, ..HostedWindow::new(w.window_id, hosted_window_op::RESIZE) }.to_json()
                    };
                    self.hub.send(vec![AppToStudio::Custom(ask)]);
                }
                OsCmd::Cursor { cursor, .. } => { self.hub.send(vec![AppToStudio::SetCursor(mouse_cursor(cursor))]); }
                // the flavours go just ahead of the text the studio protocol carries
                OsCmd::ClipSet { data } => {
                    let flavours = HostedClipboard { request: 0, text: data.text.clone(), html: data.html, markdown: data.markdown };
                    let mut msgs = Vec::new();
                    if flavours.any() {
                        msgs.push(AppToStudio::Custom(flavours.to_json()));
                    }
                    msgs.push(AppToStudio::SetClipboard(data.text));
                    self.hub.send(msgs);
                }
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
                OsCmd::PrintPdf { req, .. } => self.state.shared.push((sid, Id::NONE), |q, _| q.print_result(req, crate::print_status::UNSUPPORTED, &[])),
                OsCmd::HostMessage { message } => {
                    if !self.speakers.contains(&sid) { self.speakers.push(sid); }
                    if let Some(kind) = wm_request_state(&message) {
                        self.send_host_message(&message);
                        self.host_state.retain(|(s, k, _)| (*s, *k) != (sid, kind));
                        self.host_state.push((sid, kind, message));
                    } else if !self.host_messages.is_empty() || !self.send_host_message(&message) {
                        // Behind any still queued, so one-shot messages keep their order.
                        self.host_messages.push_back((sid, message));
                    }
                    return; // An app message does not change frame pacing.
                }
                OsCmd::FileDialog { req, kind, title, location, filename, filters } => {
                    if let Some(id) = self.send_file_dialog(kind, title, location, filename, filters) {
                        self.dialogs.insert(id, (sid, req));
                    } else {
                        self.state.shared.push((sid, Id::NONE), |q, _| q.dialog_result(req, 0, &[]));
                    }
                }
                OsCmd::DirectDisplay { req, command } => self.state.shared.push((sid, Id::NONE), |q, _| {
                    q.direct_display_result(req, command.tag() as u32, false, b"a WM-hosted window does not own direct displays");
                }),
                _ => {}
            },
            Op::CloseSession(sid) => {
                self.held.buttons.retain(|recipient, _| recipient.0 != sid);
                self.clip_reads.retain(|_, (s, _)| *s != sid);
                self.dialogs.retain(|_, (s, _)| *s != sid);
                self.host_messages.retain(|(s, _)| *s != sid);
                self.host_state.retain(|(s, _, _)| *s != sid);
                self.speakers.retain(|s| *s != sid);
                let gone: Vec<u64> = self.trays.iter().filter(|(_, (s, _))| *s == sid).map(|(t, _)| *t).collect();
                for tray in gone {
                    self.trays.remove(&tray);
                    self.hub.send(vec![AppToStudio::Relay(ChildRelay::RemoveTray { tray })]);
                }
                let closing: Vec<Key> = self.windows.keys().filter(|k| k.0 == sid).copied().collect();
                for key in closing {
                    if let Some(w) = self.windows.remove(&key) {
                        self.held.buttons.remove(&w.recipient);
                        shell::snapshot().windows.remove(&w.window_id);
                    }
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
                    self.held.buttons.remove(&w.recipient);
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
        self.publish_pace();
    }
    fn dispatch(&mut self) -> Result<(), String> {
        self.native_outgoing();
        if self.stopping { self.publish_pace(); return Ok(()); }
        // The WM shares a swapchain only once this end of the descriptor channel is connected;
        // its listener exists from the adoption on.
        self.connect_aux();
        if !self.pending.is_empty() {
            if self.aux.is_some() {
                let pending = std::mem::take(&mut self.pending);
                {
                    let mut s = shell::snapshot();
                    for swapchain in &pending {
                        if let Some(w) = s.windows.get_mut(&swapchain.window_id) { w.pending_swapchain = false; }
                    }
                }
                for swapchain in pending { self.install(swapchain); }
            }
        }
        let finished = std::mem::take(&mut *lock(&self.state.finished));
        let mut flips = Vec::new();
        let mut redraw = false;
        for (key, draw, from) in finished {
            if let Some(n) = lock(&self.state.in_flight).get_mut(&key) { *n = n.saturating_sub(1); }
            if let Some(w) = self.windows.get_mut(&key) {
                // an earlier frame's completion leaves the granted frame awaited
                if w.ahead > 0 { w.ahead -= 1; } else { w.awaiting = false; }
                // A frame that found no free shared image is drawn again on the next Tick.
                if draw.is_none() { w.requested = true; }
            }
            match draw {
                Some(draw) => {
                    flips.push(AppToStudio::DrawCompleteAndFlip(draw));
                    if !self.app_drawn && from != session::HOST {
                        self.app_drawn = true;
                        {
                            let mut s = shell::snapshot();
                            s.app_frame_shown = Some(self.app_drawn);
                            s.app_frame_at = Some(crate::clock_ns());
                        }
                        flips.push(AppToStudio::Custom(HostedAppFrame.to_json()));
                    }
                }
                None => redraw = true,
            }
        }
        self.hub.send(flips);
        // a frame finished: a window held back by MAX_IN_FLIGHT may be granted the next one
        if redraw || self.windows.values().any(|w| w.requested && !w.awaiting) { self.ask_frame(); }
        self.finish_tick();
        // The reader publishes the hang-up after queuing its last messages. Sample it first
        // so a Kill queued during this dispatch is handled before its hang-up.
        let hung_up = self.state.hung_up.load(Ordering::Acquire);
        let msgs = std::mem::take(&mut *lock(&self.state.inbox));
        for msg in msgs { self.message(msg); }
        if self.detached_at.is_some() {
            self.reattach();
        } else if hung_up {
            self.state.hung_up.store(false, Ordering::Relaxed);
            let socket = shell::snapshot().wm_socket.clone();
            let keep = socket.and_then(|path| read_notice(std::path::Path::new(&path))).is_some_and(|state| state_keeps_apps(&state));
            let (tray, known) = (!self.trays.is_empty(), self.host_notice.is_some());
            if detach_on_hangup(tray, self.killed, keep, known) {
                self.detach();
            } else {
                if !known && detach_on_hangup(tray, self.killed, keep, true) {
                    eprintln!("[hal-wm] the makepad WM went away and this process quits: that WM published no host notice as it attached (an older build), so no next WM can be told to be of its kind");
                }
                self.quit();
            }
        }
        self.publish_pace();
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
    use makepad_studio_protocol::{HostedVisible, HOSTED_VERSIONS};
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
            assert!(request.starts_with(&format!("GET /app?build=77&crate=makepad-app-amp&wire={WIRE:016x}&proto={} ", HOSTED_VERSIONS.1)), "{request}");
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
        let hub = Hub::connect(port, 77, WIRE, HOSTED_VERSIONS.1, "makepad-app-amp", state.clone()).unwrap();
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
    fn a_covered_tile_still_gets_the_frames_its_app_asks_for() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8];
            while !request.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).unwrap(); request.push(byte[0]); }
            stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").unwrap();
            (0..1).map(|_| AppToStudioVec::deserialize_bin(&read_client_frame(&mut stream).1).unwrap().0).collect::<Vec<_>>()
        });
        let state = Arc::new(State { shared: Shared::global(), inbox: Default::default(), hung_up: AtomicBool::new(false), detached: AtomicBool::new(false),
            rings: Default::default(), finished: Default::default(), in_flight: Default::default(), device: Default::default() });
        let hub = Hub::connect(port, 78, WIRE, HOSTED_VERSIONS.1, "makepad-app-hidden", state.clone()).unwrap();
        let mut app = App { state, hub, aux_path: Default::default(), aux: None, pending: Vec::new(), windows: HashMap::new(),
            early: HashMap::new(), next_window: 1, ticking: false, asked_frame: false, app_drawn: false, hidden: false, clip_reads: HashMap::new(),
            dialogs: HashMap::new(), next_dialog: 0, host_messages: VecDeque::new(), host_state: Vec::new(), speakers: Vec::new(),
            next_clip_read: 0, clip_flavours: None, selection: Default::default(), drag_queries: VecDeque::new(), wm_state: Vec::new(), wm_focus: HashMap::new(),
            trays: HashMap::new(), hidden_windows: Default::default(), detached_at: None, reattach_error: None, host_notice: None, passed_over: Vec::new(), held: HeldInput::default(), killed: false, quit_sent: false, stopping: false };
        let key = (9_001, Id::new(1, 1));
        app.windows.insert(key, Win { window_id: 0, recipient: key, requested: false, awaiting: false, ahead: 0, geom: None, dropdown: false, host_buttons: false });
        // covered: the app holds its own frames, but one it asks for anyway (a grab's) is asked of
        // the WM and granted at the next Tick
        app.message(StudioToApp::Custom(HostedVisible { visible: false }.to_json()));
        app.command(Op::Cmd(key.0, OsCmd::FrameRequest { id: key.1 }));
        app.message(StudioToApp::Tick);
        assert!(!app.windows[&key].requested && app.windows[&key].awaiting, "the frame a covered app asked for");
        let sent = server.join().unwrap();
        assert!(matches!(sent[0].as_slice(), [AppToStudio::RequestAnimationFrame]), "{:?}", sent[0]);
    }

    #[test]
    fn an_apps_resize_goes_to_the_wm_named_by_its_window() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8];
            while !request.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).unwrap(); request.push(byte[0]); }
            stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").unwrap();
            let mut sent = Vec::new();
            while sent.len() < 2 { sent.extend(AppToStudioVec::deserialize_bin(&read_client_frame(&mut stream).1).unwrap().0); }
            sent
        });
        let state = Arc::new(State { shared: Shared::global(), inbox: Default::default(), hung_up: AtomicBool::new(false), detached: AtomicBool::new(false),
            rings: Default::default(), finished: Default::default(), in_flight: Default::default(), device: Default::default() });
        let hub = Hub::connect(port, 79, WIRE, HOSTED_VERSIONS.1, "makepad-app-resize", state.clone()).unwrap();
        let mut app = App { state, hub, aux_path: Default::default(), aux: None, pending: Vec::new(), windows: HashMap::new(),
            early: HashMap::new(), next_window: 1, ticking: false, asked_frame: false, app_drawn: false, hidden: false, clip_reads: HashMap::new(),
            dialogs: HashMap::new(), next_dialog: 0, host_messages: VecDeque::new(), host_state: Vec::new(), speakers: Vec::new(),
            next_clip_read: 0, clip_flavours: None, selection: Default::default(), drag_queries: VecDeque::new(), wm_state: Vec::new(), wm_focus: HashMap::new(),
            trays: HashMap::new(), hidden_windows: Default::default(), detached_at: None, reattach_error: None, host_notice: None, passed_over: Vec::new(), held: HeldInput::default(), killed: false, quit_sent: false, stopping: false };
        let (tile, later) = ((9_002, Id::new(1, 1)), (9_002, Id::new(2, 1)));
        app.windows.insert(tile, Win { window_id: 0, recipient: tile, requested: false, awaiting: false, ahead: 0, geom: None, dropdown: false, host_buttons: false });
        app.windows.insert(later, Win { window_id: 1, recipient: later, requested: false, awaiting: false, ahead: 0, geom: None, dropdown: false, host_buttons: false });
        // Every tile's size is the WM's: a later window asks with its own id (an older WM
        // ignores the op), so its request never resizes the app's first tile.
        app.command(Op::Cmd(later.0, OsCmd::WindowGeom { id: later.1, geom: [0.0, 0.0, 300.0, 200.0] }));
        app.command(Op::Cmd(tile.0, OsCmd::WindowGeom { id: tile.1, geom: [0.0, 0.0, 640.0, 480.0] }));
        let sent = server.join().unwrap();
        let [AppToStudio::Custom(later_ask), AppToStudio::Custom(tile_ask)] = &sent[..] else { panic!("{sent:?}") };
        assert_eq!(HostedResize::parse(later_ask), None, "a later window's resize never reads as the tile's");
        assert_eq!(HostedWindow::parse(later_ask), Some(HostedWindow { x: 300.0, y: 200.0, ..HostedWindow::new(1, hosted_window_op::RESIZE) }));
        assert_eq!(HostedResize::parse(tile_ask), Some(HostedResize { w: 640.0, h: 480.0 }));
    }

    fn wm_connection(state: &Arc<State>) -> (Arc<Hub>, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (wm, _) = listener.accept().unwrap();
        (Hub::start(client, state.clone()).unwrap(), wm)
    }

    fn received(hub: &Hub, wm: &mut TcpStream) -> Vec<Vec<AppToStudio>> {
        // A ping delimits the writes so the old socket can stay writable through detach.
        hub.frame(0x9, &[]).unwrap();
        let mut sent = Vec::new();
        loop {
            let (opcode, bytes) = read_client_frame(wm);
            if opcode == 0x9 { return sent; }
            sent.push(AppToStudioVec::deserialize_bin(&bytes).unwrap().0);
        }
    }

    fn connected_app(next_window: usize) -> (App, TcpStream) {
        let state = Arc::new(State { shared: Shared::global(), inbox: Default::default(), hung_up: AtomicBool::new(false), detached: AtomicBool::new(false),
            rings: Default::default(), finished: Default::default(), in_flight: Default::default(), device: Default::default() });
        let (hub, wm) = wm_connection(&state);
        let app = App { state, hub, aux_path: Default::default(), aux: None, pending: Vec::new(), windows: HashMap::new(),
            early: HashMap::new(), next_window, ticking: false, asked_frame: false, app_drawn: false, hidden: false, clip_reads: HashMap::new(),
            dialogs: HashMap::new(), next_dialog: 0, host_messages: VecDeque::new(), host_state: Vec::new(), speakers: Vec::new(),
            next_clip_read: 0, clip_flavours: None, selection: Default::default(), drag_queries: VecDeque::new(), wm_state: Vec::new(), wm_focus: HashMap::new(),
            trays: HashMap::new(), hidden_windows: Default::default(), detached_at: None, reattach_error: None, host_notice: None, passed_over: Vec::new(), held: HeldInput::default(), killed: false, quit_sent: false, stopping: false };
        (app, wm)
    }

    #[test]
    fn wm_shell_announces_pointer_caps_routes_outside_up_and_blurs_on_detach() {
        use crate::HalEvent;
        use makepad_studio_protocol::{MouseButton, RemoteMouseDown, RemoteMouseUp};

        let sid = 9_105;
        let (first, later) = (Id::new(0, 1), Id::new(1, 1));
        let (mut app, mut wm) = connected_app(0);
        for (id, left) in [(first, 0.0), (later, 100.0)] {
            let bounds = [left, 10.0, 80.0, 60.0];
            app.early.insert(id.index as usize, (bounds, 1.0, None));
            app.command(Op::Cmd(sid, OsCmd::WindowCreate { id, kind: 0, parent: Id::NONE, geom: [0.0; 4] }));
        }
        let check_announcements = |batches: &[Vec<AppToStudio>]| {
            let mut windows = Vec::new();
            for batch in batches {
                if let [AppToStudio::CreateWindow { window_id, .. }, AppToStudio::Custom(caps)] = batch.as_slice() {
                    assert_eq!(caps, r#"{"makepad_pointer_caps":{"mouse_cancel":true}}"#);
                    windows.push(*window_id);
                }
            }
            assert_eq!(windows, [0, 1], "both windows announce cancellation support");
        };
        check_announcements(&received(&app.hub, &mut wm));

        let button = MouseButton::PRIMARY.bits();
        let down = |x| StudioToApp::MouseDown(RemoteMouseDown { x, y: 15.0, button_raw_bits: button, ..Default::default() });
        for cancel in [false, true] {
            // A lost up in the first window cannot keep ownership after a new down.
            app.message(down(20.0));
            app.message(down(120.0));
            let release = RemoteMouseUp { x: 35.0, y: 22.0, button_raw_bits: button, ..Default::default() };
            app.message(if cancel { StudioToApp::MouseCancel(release) } else { StudioToApp::MouseUp(release) });
            let mut edges = Vec::new();
            lock(&app.state.shared.inbox).remove(&sid).unwrap().drain_with(|e, payload| match e.decode(payload) {
                Some(HalEvent::MouseUp { window, x, y, button, .. }) => edges.push((false, window, x, y, button)),
                Some(HalEvent::MouseCancel { window, x, y, button, .. }) => edges.push((true, window, x, y, button)),
                _ => {}
            });
            assert_eq!(edges, [(cancel, later, -65.0, 12.0, button)], "release follows the owning press");
            assert!(app.held.buttons.is_empty(), "the button had one owner");
            assert_eq!(app.held.pointer, (35.0, 22.0));
        }
        app.detach();
        let mut blurred = Vec::new();
        lock(&app.state.shared.inbox).remove(&sid).unwrap().drain_with(|e, payload| {
            if let Some(HalEvent::Focus { window, focused: false }) = e.decode(payload) {
                blurred.push(window);
            }
        });
        blurred.sort_by_key(|window| window.index);
        assert_eq!(blurred, [first, later]);
        let (hub, mut next_wm) = wm_connection(&app.state);
        app.finish_reattach(hub, &[sid]);
        check_announcements(&received(&app.hub, &mut next_wm));
        app.command(Op::CloseSession(sid));
    }

    #[test]
    /// A tray goes to the WM as a relay (items, check marks, lines, the glyph), a menu choice
    /// comes back to the session that set it, a closed session's tray is removed, and a process
    /// with a tray waits for the next WM when its WM goes away instead of quitting.
    /// File pickers return their paths to their session and detach cancels outstanding pickers.
    /// The replacement WM receives messages queued by sessions that survived the detach.
    /// A detached process joins only a WM of the kind it left (the node's own, or its lane's test WM).
    fn trays_relay_to_the_wm_and_outlive_it() {
        use crate::types::{menu_kind, MenuItem};
        use makepad_studio_protocol::RelayFileDialogResult;
        assert!(detach_on_hangup(true, true, false, true), "a tray outlives the WM even after Kill without keep");
        assert!(!detach_on_hangup(false, true, true, true), "Kill quits an app without a tray even with keep");
        assert!(detach_on_hangup(false, false, true, true), "keep detaches an app without a tray on hang-up");
        assert!(!detach_on_hangup(false, false, false, true), "a hang-up without keep quits an app without a tray");
        assert!(!detach_on_hangup(true, false, true, false), "no app waits when its WM published no notice: no next WM could be told to be of its kind");
        assert!(state_keeps_apps("epoch=ab12\nkeep=1\n"), "keep=1 is recognized alongside other keys");
        assert!(!state_keeps_apps("keep=0"), "keep=0 disables keep");
        assert!(!state_keeps_apps(""), "an empty state file does not keep apps");
        assert!(!state_keeps_apps("keep=1x"), "only an exact keep=1 line keeps apps");
        let node_wm = "epoch=ab12\nkeep=1\nlane=\n";
        let lane_wm = "epoch=cd34\nkeep=1\nlane=devhubfix\n";
        assert_eq!(may_reattach(Some(node_wm), Some("epoch=ef56\nkeep=1\nlane=\n")), Ok(()), "a kept app joins the node's next WM");
        let why = may_reattach(Some(node_wm), Some(lane_wm)).unwrap_err();
        assert!(why.contains("a test WM of lane devhubfix") && why.contains("the node's own WM"), "the node's apps never join a lane's test WM, and the skip says why: {why}");
        assert!(may_reattach(Some(lane_wm), Some(node_wm)).is_err(), "a test WM's apps never join the node's WM");
        assert!(may_reattach(Some(lane_wm), Some("epoch=0\nkeep=1\nlane=other\n")).is_err(), "nor another lane's test WM");
        assert_eq!(may_reattach(Some(lane_wm), Some("epoch=9\nkeep=1\nlane=devhubfix\n")), Ok(()), "a test WM's apps join the lane's next test WM");
        assert!(may_reattach(Some(node_wm), None).is_err(), "a kept app passes over a WM that publishes no notice");
        assert!(may_reattach(Some(node_wm), Some("keep=0\nlane=\n")).is_err(), "a kept app passes over a WM that would not keep it");
        assert_eq!(may_reattach(Some("keep=0\nlane=\n"), Some("keep=0\nlane=\n")), Ok(()), "a tray joins a WM of its kind that keeps nothing");
        let why = may_reattach(None, Some(node_wm)).unwrap_err();
        assert!(why.contains("no host notice"), "an app whose old WM published no notice rejoins no WM, and says why: {why}");
        let (mut app, mut old_wm) = connected_app(1);
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
        // Two sessions may use the same request number. Only the second picker gets a reply.
        let sid = 9_102;
        let answered_sid = 9_103;
        let req = 17;
        app.command(Op::Cmd(sid, OsCmd::FileDialog {
            req, kind: dialog_kind::SAVE_FOLDER, title: String::new(), location: String::new(), filename: String::new(), filters: Vec::new(),
        }));
        app.command(Op::Cmd(answered_sid, OsCmd::FileDialog {
            req, kind: dialog_kind::SELECT_FILE | dialog_kind::MULTIPLE, title: "Open documents".into(),
            location: "docs".into(), filename: "draft.md".into(),
            filters: vec!["Text".into(), "md,txt".into(), "Images".into(), "png".into()],
        }));
        let host_message = |sid, text: &str| Op::Cmd(sid, OsCmd::HostMessage { message: text.as_bytes().to_vec() });
        let title = r#"{"wm":{"Title":{"title":"café"}}}"#;
        let cwd = r#"{"wm":{"Cwd":{"path":"documents"}}}"#;
        let launch = r#"{"wm":{"Launch":{"app":"terminal","args":[]}}}"#;
        app.command(host_message(sid, title));
        app.command(host_message(sid, cwd));
        app.command(host_message(sid, launch));
        let sent = received(&app.hub, &mut old_wm);
        assert_eq!(sent.len(), 7, "the WM receives both tray relays, both dialog requests and all three app messages");
        assert!(matches!(sent[4].as_slice(), [AppToStudio::Custom(text)] if text == title), "the WM gets the exact app text: {:?}", sent[4]);
        assert!(matches!(sent[6].as_slice(), [AppToStudio::Custom(text)] if text == launch), "the original WM gets the launch action");
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
        let [AppToStudio::Relay(ChildRelay::FileDialog(pending))] = sent[2].as_slice() else { panic!("{:?}", sent[2]) };
        assert_eq!(pending, &RelayFileDialog { id: pending.id, kind: "save_folder".into(), ..Default::default() });
        let [AppToStudio::Relay(ChildRelay::FileDialog(open))] = sent[3].as_slice() else { panic!("{:?}", sent[3]) };
        assert_ne!(open.id, pending.id, "each outstanding dialog has its own relay id");
        assert_eq!(open, &RelayFileDialog {
            id: open.id, kind: "select_file".into(), multiple: true, title: Some("Open documents".into()),
            location: Some("docs".into()), filename: Some("draft.md".into()),
            filters: vec![("Text".into(), vec!["md".into(), "txt".into()]), ("Images".into(), vec!["png".into()])],
        });
        let chosen = ["docs/first draft.md", "docs/café.txt"];
        app.message(StudioToApp::Relay(HostRelay::FileDialog(RelayFileDialogResult {
            id: open.id, save: false, folder: false, paths: chosen.iter().map(|s| s.to_string()).collect(),
        })));
        let mut results = Vec::new();
        if let Some(mut queue) = lock(&app.state.shared.inbox).remove(&answered_sid) {
            queue.drain_with(|e, payload| if let Some(crate::HalEvent::DialogResult { req, count, paths }) = e.decode(payload) {
                results.push((req, count, crate::buffers::read_strs(paths, count)));
            });
        }
        assert_eq!(results, [(req, 2, chosen.map(String::from).to_vec())], "the requesting session gets its paths in order");
        assert!(!lock(&app.state.shared.inbox).contains_key(&sid), "the other session's dialog is still pending");
        use makepad_studio_protocol::{KeyCode, KeyEvent, MouseButton, RemoteMouseDown, RemoteMouseMove, RemoteMouseUp};
        let first = Id::new(1, 1);
        let later = Id::new(2, 1);
        for (window_id, id, left) in [(0, first, 0.0), (1, later, 100.0)] {
            app.windows.insert((sid, id), Win { window_id, recipient: (sid, id), requested: false, awaiting: false, ahead: 0,
                geom: Some(([left, 0.0, 80.0, 60.0], 1.0, None)), dropdown: false, host_buttons: false });
        }
        let key = KeyEvent { key_code: KeyCode::KeyA, ..Default::default() };
        app.message(StudioToApp::KeyDown(key));
        app.message(StudioToApp::KeyDown(KeyEvent { is_repeat: true, ..key }));
        let released = KeyEvent { key_code: KeyCode::KeyB, ..key };
        app.message(StudioToApp::KeyDown(released));
        app.message(StudioToApp::KeyUp(released));
        let primary = MouseButton::PRIMARY.bits();
        let secondary = MouseButton::SECONDARY.bits();
        app.message(StudioToApp::MouseDown(RemoteMouseDown { x: 110.0, y: 5.0, button_raw_bits: primary, ..Default::default() }));
        app.message(StudioToApp::MouseDown(RemoteMouseDown { x: 110.0, y: 5.0, button_raw_bits: secondary, ..Default::default() }));
        app.message(StudioToApp::MouseCancel(RemoteMouseUp { x: 110.0, y: 5.0, button_raw_bits: secondary, ..Default::default() }));
        app.message(StudioToApp::MouseMove(RemoteMouseMove { x: 70.0, y: 20.0, ..Default::default() }));
        lock(&app.state.shared.inbox).remove(&sid);

        // The WM goes away with a tray up: detached, not quitting, once it attached to a WM that
        // published its notice; without one (an older WM) it quits instead.
        app.trays.insert(78, (0, RelayTray::default()));
        app.state.hung_up.store(true, Ordering::Release);
        app.host_notice = Some("keep=0\nlane=\n".into());
        app.dispatch().unwrap();
        assert!(app.detached_at.is_some() && !app.quit_sent, "a tray app waits for the next WM");
        let mut keys = Vec::new();
        let mut buttons = Vec::new();
        let mut cancelled = Vec::new();
        if let Some(mut queue) = lock(&app.state.shared.inbox).remove(&sid) {
            queue.drain_with(|event, payload| match event.decode(payload) {
                Some(crate::HalEvent::KeyUp { window, key, .. }) => keys.push((window, key)),
                Some(crate::HalEvent::MouseUp { window, x, y, button, .. }) => buttons.push((window, x, y, button)),
                Some(crate::HalEvent::DialogResult { req, count, paths }) => cancelled.push((req, count, paths.to_vec())),
                _ => {}
            });
        }
        assert_eq!(cancelled, [(req, 0, Vec::new())], "detach cancels the outstanding dialog on its session");
        assert!(!lock(&app.state.shared.inbox).contains_key(&answered_sid), "detach leaves the completed dialog alone");
        assert_eq!(keys, [(first, key.key_code.to_index())], "detach releases each still-held key once to its recipient");
        assert_eq!(buttons, [(later, -30.0, 20.0, primary)], "detach releases only held buttons to their recipient at the last pointer position");

        let closing_sid = 9_104;
        let new_title = r#"{"wm":{"Title":{"title":"updated"}}}"#;
        let other_title = r#"{"wm":{"Title":{"title":"second session"}}}"#;
        app.command(host_message(sid, "first"));
        app.command(host_message(sid, new_title));
        app.command(host_message(answered_sid, other_title));
        app.command(host_message(closing_sid, cwd));
        app.command(host_message(closing_sid, "from a closing session"));
        app.command(host_message(answered_sid, "second"));
        app.command(Op::CloseSession(closing_sid));
        let during_detach = received(&app.hub, &mut old_wm);

        let (hub, mut next_wm) = wm_connection(&app.state);
        app.finish_reattach(hub, &[sid, answered_sid]);
        let replay: Vec<_> = received(&app.hub, &mut next_wm).into_iter().flatten().collect();
        assert!(matches!(replay.as_slice(), [
            AppToStudio::CreateWindow { window_id: 0, .. }, AppToStudio::Custom(first_caps),
            AppToStudio::CreateWindow { window_id: 1, .. }, AppToStudio::Custom(later_caps),
            AppToStudio::Relay(ChildRelay::SetTray(_)), AppToStudio::Custom(_), ..
        ] if first_caps == POINTER_CAPS_JSON && later_caps == POINTER_CAPS_JSON), "the new WM gets the windows and tray before the queued messages: {replay:?}");
        let texts: Vec<_> = replay.iter().filter_map(|msg| match msg { AppToStudio::Custom(text) => Some(text.as_str()), _ => None }).collect();
        assert!(!texts.contains(&title) && !texts.contains(&launch), "the old title and the previously delivered launch are never replayed");
        assert_eq!(texts, [POINTER_CAPS_JSON, POINTER_CAPS_JSON, cwd, new_title, other_title, "first", "second"],
            "each live session's latest title and folder reach the new WM once before the queued messages, in send order");
        assert!(during_detach.is_empty() && app.host_messages.is_empty(), "messages stayed off the old connection and the new WM drained the queue");
        for session in [sid, answered_sid] {
            let mut attached = Vec::new();
            if let Some(mut queue) = lock(&app.state.shared.inbox).remove(&session) {
                queue.drain_with(|event, payload| {
                    if let Some(crate::HalEvent::AppMessage { window, message }) = event.decode(payload) {
                        attached.push((window, message.to_vec()));
                    }
                });
            }
            assert_eq!(attached, [(Id::NONE, wm_socket::HOST_ATTACHED.as_bytes().to_vec())],
                "each live session hears the host-attached marker once, even without a window");
        }
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
        let hub = Hub::connect(port, 81, WIRE, HOSTED_VERSIONS.1, "makepad-app-aihub", state.clone()).unwrap();
        let mut app = App { state, hub, aux_path: Default::default(), aux: None, pending: Vec::new(), windows: HashMap::new(),
            early: HashMap::new(), next_window: 0, ticking: false, asked_frame: false, app_drawn: false, hidden: false, clip_reads: HashMap::new(),
            dialogs: HashMap::new(), next_dialog: 0, host_messages: VecDeque::new(), host_state: Vec::new(), speakers: Vec::new(),
            next_clip_read: 0, clip_flavours: None, selection: Default::default(), drag_queries: VecDeque::new(), wm_state: Vec::new(), wm_focus: HashMap::new(),
            trays: HashMap::new(), hidden_windows: Default::default(), detached_at: None, reattach_error: None, host_notice: None, passed_over: Vec::new(), held: HeldInput::default(), killed: false, quit_sent: false, stopping: false };
        let id = Id::new(5, 1);
        app.command(Op::Cmd(6, OsCmd::WindowCreate { id, kind: window_kind::DROPDOWN, parent: Id::NONE, geom: [0.0; 4] }));
        assert!(app.windows[&(6, id)].dropdown);
        app.command(Op::Cmd(6, OsCmd::WindowAtTray { id, tray: 77, w: 460.0, h: 600.0 }));
        app.command(Op::Cmd(6, OsCmd::WindowState { id, state: window_state::HIDE }));
        assert!(app.hidden_windows.is_empty(), "a hidden dropdown is not a minimised tile");
        let sent = server.join().unwrap();
        assert!(matches!(sent[0].as_slice(), [AppToStudio::CreateWindow { window_id: 0, kind_id }, AppToStudio::Custom(caps)] if *kind_id == window_kind::DROPDOWN as usize && caps == POINTER_CAPS_JSON), "{:?}", sent[0]);
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
        let hub = Hub::connect(port, 80, WIRE, HOSTED_VERSIONS.1, "makepad-app-game", state.clone()).unwrap();
        let mut app = App { state: state.clone(), hub, aux_path: Default::default(), aux: None, pending: Vec::new(), windows: HashMap::new(),
            early: HashMap::new(), next_window: 1, ticking: false, asked_frame: false, app_drawn: false, hidden: false, clip_reads: HashMap::new(),
            dialogs: HashMap::new(), next_dialog: 0, host_messages: VecDeque::new(), host_state: Vec::new(), speakers: Vec::new(),
            next_clip_read: 0, clip_flavours: None, selection: Default::default(), drag_queries: VecDeque::new(), wm_state: Vec::new(), wm_focus: HashMap::new(),
            trays: HashMap::new(), hidden_windows: Default::default(), detached_at: None, reattach_error: None, host_notice: None, passed_over: Vec::new(), held: HeldInput::default(), killed: false, quit_sent: false, stopping: false };
        let key = (9_003, Id::new(1, 1));
        app.windows.insert(key, Win { window_id: 0, recipient: key, requested: false, awaiting: false, ahead: 0, geom: None, dropdown: false, host_buttons: false });
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
        let hub = Hub::connect(port, 79, WIRE, HOSTED_VERSIONS.1, "makepad-app-game", state.clone()).unwrap();
        let mut app = App { state: state.clone(), hub, aux_path: Default::default(), aux: None, pending: Vec::new(), windows: HashMap::new(),
            early: HashMap::new(), next_window: 1, ticking: false, asked_frame: false, app_drawn: false, hidden: false, clip_reads: HashMap::new(),
            dialogs: HashMap::new(), next_dialog: 0, host_messages: VecDeque::new(), host_state: Vec::new(), speakers: Vec::new(),
            next_clip_read: 0, clip_flavours: None, selection: Default::default(), drag_queries: VecDeque::new(), wm_state: Vec::new(), wm_focus: HashMap::new(),
            trays: HashMap::new(), hidden_windows: Default::default(), detached_at: None, reattach_error: None, host_notice: None, passed_over: Vec::new(), held: HeldInput::default(), killed: false, quit_sent: false, stopping: false };
        let key = (9_002, Id::new(1, 1));
        app.windows.insert(key, Win { window_id: 0, recipient: key, requested: false, awaiting: false, ahead: 0, geom: None, dropdown: false, host_buttons: false });
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
    /// A later window gets the presses made in it, its answer carries its own double click, and
    /// one the app closes is said to the WM by its id, so its tile leaves the desk.
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
            (0..11).map(|_| AppToStudioVec::deserialize_bin(&read_client_frame(&mut stream).1).unwrap().0).collect::<Vec<_>>()
        });
        let state = Arc::new(State { shared: Shared::global(), inbox: Default::default(), hung_up: AtomicBool::new(false), detached: AtomicBool::new(false),
            rings: Default::default(), finished: Default::default(), in_flight: Default::default(), device: Default::default() });
        let hub = Hub::connect(port, 79, WIRE, HOSTED_VERSIONS.1, "makepad-app-clock", state.clone()).unwrap();
        let mut app = App { state, hub, aux_path: Default::default(), aux: None, pending: Vec::new(), windows: HashMap::new(),
            early: HashMap::new(), next_window: 1, ticking: false, asked_frame: false, app_drawn: false, hidden: false, clip_reads: HashMap::new(),
            dialogs: HashMap::new(), next_dialog: 0, host_messages: VecDeque::new(), host_state: Vec::new(), speakers: Vec::new(),
            next_clip_read: 0, clip_flavours: None, selection: Default::default(), drag_queries: VecDeque::new(), wm_state: Vec::new(), wm_focus: HashMap::new(),
            trays: HashMap::new(), hidden_windows: Default::default(), detached_at: None, reattach_error: None, host_notice: None, passed_over: Vec::new(), held: HeldInput::default(), killed: false, quit_sent: false, stopping: false };
        let host = (session::HOST, Id::new(1, 1));
        let lent = (9_002, Id::new(4, 1));
        app.windows.insert(host, Win { window_id: 0, recipient: lent, requested: false, awaiting: false, ahead: 0, geom: None, dropdown: false, host_buttons: false });
        // two presses (a double click) before the app answers: the answers keep the WM's ids
        app.message(StudioToApp::Custom(HostedDragQuery { id: 11, x: 40.0, y: 8.0 }.to_json()));
        app.message(StudioToApp::Custom(HostedDragQuery { id: 12, x: 41.0, y: 8.0 }.to_json()));
        assert_eq!(app.drag_queries, [11, 12]);
        assert!(lock(&app.state.shared.inbox).contains_key(&lent.0), "the query goes to the session the window is lent to");
        let reply = |answer| OsCmd::DragReply { id: lent.1, to: input::reply::QUERY, answer, fullscreen: false };
        app.command(Op::Cmd(lent.0, reply(input::drag_query::CAPTION)));
        app.command(Op::Cmd(lent.0, reply(input::drag_query::CLIENT)));
        assert!(app.drag_queries.is_empty());
        // the caption's buttons, sent by the app's domain on its alias of the window
        app.command(Op::Cmd(lent.0, OsCmd::WindowState { id: lent.1, state: window_state::MAXIMIZE }));
        app.command(Op::Cmd(lent.0, OsCmd::WindowState { id: lent.1, state: window_state::MINIMIZE }));
        app.command(Op::Cmd(lent.0, OsCmd::WindowState { id: lent.1, state: window_state::FULLSCREEN }));
        // a later window (an output window) is a tile of its own: what the app asks of it names it
        let later = (lent.0, Id::new(5, 1));
        let origin = ([100_000.0, 0.0, 300.0, 200.0], 1.0, None);
        app.windows.insert(later, Win { window_id: 3, recipient: later, requested: false, awaiting: false, ahead: 0, geom: Some(origin), dropdown: false, host_buttons: false });
        app.command(Op::Cmd(lent.0, OsCmd::WindowState { id: later.1, state: window_state::FULLSCREEN }));
        app.command(Op::Cmd(lent.0, OsCmd::WindowState { id: later.1, state: window_state::RESTORE }));
        // a press in it (at its origin in the child's window space) is asked of that window, at
        // its own point, so a window that drags by its content drags
        lock(&app.state.shared.inbox).remove(&lent.0);
        app.message(StudioToApp::Custom(HostedDragQuery { id: 13, x: 100_040.0, y: 8.0 }.to_json()));
        let mut asked = Vec::new();
        if let Some(mut q) = lock(&app.state.shared.inbox).remove(&lent.0) {
            q.drain_with(|e, payload| if let Some(crate::HalEvent::DragQuery { window, x, y }) = e.decode(payload) { asked.push((window, x, y)); });
        }
        assert_eq!(asked, [(later.1, 40.0, 8.0)]);
        // it drags by its content and a double click makes it fullscreen (its own declaration)
        app.command(Op::Cmd(lent.0, OsCmd::DragReply { id: later.1, to: input::reply::QUERY, answer: input::drag_query::CAPTION, fullscreen: true }));
        app.command(Op::Cmd(lent.0, OsCmd::WindowClose { id: later.1 }));
        assert!(!app.windows.contains_key(&later));
        // a window this shell does not have asks nothing
        app.command(Op::Cmd(lent.0, OsCmd::WindowState { id: Id::new(9, 1), state: window_state::MAXIMIZE }));
        // the WM's dark/light reaches `host_dark` (the run's load screen reads it) and wakes it
        app.message(StudioToApp::Custom(HostedAppearance { dark: true }.to_json()));
        assert_eq!(super::super::host_dark(), Some(true));
        assert!(lock(&app.state.shared.inbox).contains_key(&session::HOST), "the load screen is woken");
        app.message(StudioToApp::Custom(HostedAppearance { dark: false }.to_json()));
        assert_eq!(super::super::host_dark(), Some(false));
        // the app shows no window buttons of its own in the window (chromeless, or full screen
        // with no caption): the WM is told once, by the window's id, to give it its own
        app.command(Op::Cmd(lent.0, OsCmd::WindowButtons { id: lent.1, show: true }));
        app.command(Op::Cmd(lent.0, OsCmd::WindowButtons { id: lent.1, show: true }));
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
        assert_eq!(HostedWindowOp::parse(&sent[4]), Some(HostedWindowOp { op: WINDOW_OP_FULLSCREEN }));
        assert_eq!(HostedWindow::parse(&sent[5]), Some(HostedWindow::new(3, hosted_window_op::FULLSCREEN)));
        assert_eq!(HostedWindow::parse(&sent[6]), Some(HostedWindow::new(3, hosted_window_op::RESTORE)));
        assert_eq!(HostedDragAnswer::parse(&sent[7]), Some(HostedDragAnswer { id: 13, response: drag_answer::CAPTION, double_click: drag_double::FULLSCREEN }));
        assert_eq!(HostedWindow::parse(&sent[8]), Some(HostedWindow::new(3, hosted_window_op::CLOSED)));
        assert_eq!(HostedWindow::parse(&sent[9]), Some(HostedWindow { x: 1.0, ..HostedWindow::new(0, hosted_window_op::HOST_BUTTONS) }));
        assert_eq!(HostedWindowOp::parse(&sent[10]), Some(HostedWindowOp { op: window_op::CLOSE }));
    }

    /// `makepad run` under the WM: the WM's style sheet and its activation of the tile arrive while
    /// the run's own boot window has it (the app's domain is not up yet); the session the window is
    /// lent to then hears the latest of each, as an app the WM launched hears them at its start.
    /// Events that are no state (a drag query, a back press) are not replayed; an answer to what
    /// the app's domain sent the WM reaches it before the lend.
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
        let hub = Hub::connect(port, 80, WIRE, HOSTED_VERSIONS.1, "makepad-app-lend", state.clone()).unwrap();
        server.join().unwrap();
        let mut app = App { state, hub, aux_path: Default::default(), aux: None, pending: Vec::new(), windows: HashMap::new(),
            early: HashMap::new(), next_window: 1, ticking: false, asked_frame: false, app_drawn: false, hidden: false, clip_reads: HashMap::new(),
            dialogs: HashMap::new(), next_dialog: 0, host_messages: VecDeque::new(), host_state: Vec::new(), speakers: Vec::new(),
            next_clip_read: 0, clip_flavours: None, selection: Default::default(), drag_queries: VecDeque::new(), wm_state: Vec::new(), wm_focus: HashMap::new(),
            trays: HashMap::new(), hidden_windows: Default::default(), detached_at: None, reattach_error: None, host_notice: None, passed_over: Vec::new(), held: HeldInput::default(), killed: false, quit_sent: false, stopping: false };
        let host = (session::HOST, Id::new(77, 1));
        app.windows.insert(host, Win { window_id: 0, recipient: host, requested: false, awaiting: false, ahead: 0, geom: None, dropdown: false, host_buttons: false });
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
        let lent = (9_101, Id::new(4, 1));
        let app_messages = |heard: &[(u16, String, Option<String>)]| -> Vec<String> {
            heard.iter().filter(|(tag, ..)| *tag == crate::events::event_tag::AppMessage).map(|(_, json, _)| json.clone()).collect()
        };
        // the app's domain asks the WM before the window is lent to it (an AI port opened at its
        // start): the answer, no state, reaches it at once, not only the boot window
        app.command(Op::Cmd(lent.0, OsCmd::HostMessage { message: br#"{"makepad_ask":{}}"#.to_vec() }));
        let answer = r#"{"makepad_answer":{}}"#;
        app.message(StudioToApp::Custom(answer.to_string()));
        assert_eq!(app_messages(&drain(&app, lent.0)), [answer], "the session that asked hears the answer before its lend");
        // lent to the app's domain: the latest style, and the tile's focus as the WM last said it
        app.command(Op::Lend { host: host.1, session: lent.0, alias: lent.1 });
        let heard = drain(&app, lent.0);
        assert_eq!(app_messages(&heard), [style], "only the latest state, no events");
        let focus: Vec<&String> = heard.iter().filter(|(tag, ..)| *tag == crate::events::event_tag::Focus).filter_map(|(.., e)| e.as_ref()).collect();
        assert_eq!(focus.len(), 1);
        assert!(focus[0].contains("focused: false"), "{focus:?}");
        // the window is the app's now: it hears the next answer once, as the input recipient
        app.message(StudioToApp::Custom(answer.to_string()));
        assert_eq!(app_messages(&drain(&app, lent.0)), [answer], "an answer after the lend reaches the app once");
    }

    #[test]
    fn cursors_and_phases_map_one_to_one() {
        assert_eq!(mouse_cursor(cursor::TEXT), MouseCursor::Text);
        assert_eq!(mouse_cursor(cursor::ROW_RESIZE), MouseCursor::RowResize);
        assert_eq!(mouse_cursor(999), MouseCursor::Default);
        assert_eq!(phase(ScrollPhase::Momentum), input::scroll::MOMENTUM);
        assert_eq!(key_mods(true, false, true, false), input::MOD_SHIFT | input::MOD_ALT);
    }
}
