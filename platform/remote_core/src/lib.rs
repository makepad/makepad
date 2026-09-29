//! The Cx-free half of `--remote` (see `makepad_platform::remote`): the
//! HTTP accept loop and request parsing, the routes, the command queue the
//! HTTP threads hand to the UI thread, the grab sinks and the PNG encode
//! worker. None of it touches `Cx`; platform's `remote` module drains the
//! queue from the event loop, answers the commands and delivers grabs.
//!
//! It lives in its own crate so it compiles in parallel with the script
//! crate instead of inside platform, which everything waits for. Platform
//! fills [`PlatformHooks`] when the bridge starts: the event-loop wake and
//! the routes that need platform state (`/cap/start`, `/cap/stop`,
//! `/cursor`, `/midi`, `/log`).
#![allow(clippy::too_many_arguments)]

use makepad_math::{dvec2, Vec2d};
use makepad_studio_protocol::{KeyCode, PinchPhase, RemoteKeyModifiers};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

// ------------------------------------------------------------------
// platform hooks
// ------------------------------------------------------------------

/// What the HTTP side needs from platform, installed once by
/// `remote::start_if_requested` before the accept loop starts.
pub struct PlatformHooks {
    /// Wake the event loop so it drains the command queue.
    pub wake: fn(),
    /// Routes answered with platform state; `None` for an unknown path.
    pub route: fn(&str, &Params) -> Option<Out>,
}

static HOOKS: OnceLock<PlatformHooks> = OnceLock::new();

pub fn set_platform_hooks(hooks: PlatformHooks) {
    let _ = HOOKS.set(hooks);
}

fn hooks() -> &'static PlatformHooks {
    fn no_wake() {}
    fn no_route(_: &str, _: &Params) -> Option<Out> {
        None
    }
    HOOKS.get_or_init(|| PlatformHooks { wake: no_wake, route: no_route })
}

/// Serve `listener` on a background thread: one thread per connection, at
/// most [`MAX_LIVE_CONNS`] at once.
pub fn serve(listener: TcpListener) {
    std::thread::Builder::new()
        .name("makepad-remote".to_string())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                if LIVE_CONNS.load(Ordering::Relaxed) >= MAX_LIVE_CONNS {
                    let mut stream = stream;
                    let _ = respond(&mut stream, 503, "application/json", b"{\"err\":\"busy\"}");
                    continue;
                }
                LIVE_CONNS.fetch_add(1, Ordering::Relaxed);
                let _ = std::thread::Builder::new()
                    .name("makepad-remote-conn".to_string())
                    .spawn(move || {
                        handle_conn(stream);
                        LIVE_CONNS.fetch_sub(1, Ordering::Relaxed);
                    });
            }
        })
        .ok();
}

/// Channel order of raw grab pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrabOrder {
    Rgba,
    Bgra,
}

/// Which row raw grab pixels start at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrabOrigin {
    TopLeft,
    BottomLeft,
}

pub fn encode_rgba_as_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    use makepad_zune_png::{
        makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
        PngEncoder,
    };
    let options = EncoderOptions::default()
        .set_width(width as usize)
        .set_height(height as usize)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::RGBA);
    let mut encoder = PngEncoder::new(rgba, options);
    let mut out = Vec::new();
    encoder
        .encode(&mut out)
        .map_err(|err| format!("png encode failed: {err:?}"))?;
    Ok(out)
}

thread_local! {
    // Each connection owns its request context; it never crosses threads
    // implicitly. Queued commands carry the explicit expected user epoch.
    /// The `if_user_seq` a request carried, if any: a driver that names
    /// the sequence it started from is refused once the person has
    /// intervened; one that sends nothing only meets the quiet-period gate.
    pub static REQUEST_USER_SEQ: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    pub static REQUEST_START_USER_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

// ------------------------------------------------------------------
// global state (the HTTP threads' only view of the app)
// ------------------------------------------------------------------

pub static ACTIVE: AtomicBool = AtomicBool::new(false);

/// When the bridge last injected input, in milliseconds since it came
/// up; zero until it has. A window may wear a marker while this is
/// recent, so a person watching a scripted run can see that the pointer
/// and the keyboard are spoken for and keep their hands off.
pub static INJECTED_AT_MS: AtomicU64 = AtomicU64::new(0);

/// `/handsoff?on=1` holds the marker up regardless of recency, for a run
/// that thinks between its inputs; `?on=0` lets it go.
pub static HANDS_OFF: AtomicBool = AtomicBool::new(false);

/// How long hands-off stays true after the last injected input.
pub const HANDS_OFF_LINGER_MS: u64 = 3000;

pub fn uptime_ms() -> u64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Called when the bridge applies injected input.
pub fn note_injected_input() {
    INJECTED_AT_MS.store(uptime_ms().max(1), Ordering::Relaxed);
}

/// True when this process asked for the remote bridge, in any of the forms
/// [`requested_bind`] accepts — including `MAKEPAD_REMOTE`, which a plain
/// argv scan used to miss, so `MAKEPAD_REMOTE=1` started the bridge while
/// everything keyed off this said no. Pure argv + env, usable before the
/// bridge itself is up: the platform's focus policy reads it while the
/// first window is being created.
pub fn requested() -> bool {
    requested_bind().is_some()
}

pub static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub static LIVE_CONNS: AtomicUsize = AtomicUsize::new(0);

/// Grab request ids live in the top half of the id space, like the file
/// sinks in `cx_shared.rs`, so they can never collide with studio ids.
pub const GRAB_ID_BASE: u64 = 1 << 62;

pub const MAX_LIVE_CONNS: usize = 24;

pub const MAX_HEAD_BYTES: usize = 32 * 1024;

pub const MAX_BODY_BYTES: usize = 1 << 20;

pub const GRABS_KEPT_PER_WINDOW: usize = 64;

pub const MAX_PENDING_GRABS: usize = 64;

pub const MAX_GRAB_BYTES: usize = 64 * 1024 * 1024;

pub static PENDING_GRABS: AtomicUsize = AtomicUsize::new(0);

pub static GRAB_BYTES: AtomicUsize = AtomicUsize::new(0);

pub fn queue() -> &'static Mutex<Vec<QueuedCmd>> {
    static Q: OnceLock<Mutex<Vec<QueuedCmd>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn status_cell() -> &'static Mutex<Status> {
    static S: OnceLock<Mutex<Status>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Status::default()))
}

pub fn grab_sinks() -> &'static Mutex<HashMap<u64, GrabSink>> {
    static G: OnceLock<Mutex<HashMap<u64, GrabSink>>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn grab_dir() -> &'static Mutex<PathBuf> {
    static D: OnceLock<Mutex<PathBuf>> = OnceLock::new();
    D.get_or_init(|| Mutex::new(PathBuf::new()))
}

/// Windows the *human* dismissed, by id → title. Kept so a later request for
/// that window can say why it is gone instead of "no window N", which reads
/// like a crash.
pub fn closed_windows() -> &'static Mutex<Vec<(usize, String)>> {
    static C: OnceLock<Mutex<Vec<(usize, String)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Vec::new()))
}

#[derive(Default)]
pub struct Status {
    pub app: String,
    pub pid: u32,
    pub windows: Vec<WinInfo>,
    /// The `Cx`'s user sequence, shared when the service starts so the
    /// request threads can stamp headers without asking the UI thread.
    pub user_seq: Arc<AtomicU64>,
}

/// The user sequence as the request threads see it.
pub fn user_seq_now() -> u64 {
    status_cell().lock().unwrap().user_seq.load(Ordering::Acquire)
}

#[derive(Clone, PartialEq)]
pub struct WinInfo {
    pub id: usize,
    pub title: String,
    pub w: f64,
    pub h: f64,
    pub dpi: f64,
    pub x: f64,
    pub y: f64,
}

pub struct GrabSink {
    pub window: Option<usize>,
    pub requested_at: Instant,
    pub scale: f64,
    pub cancelled: Arc<AtomicBool>,
    pub tx: SyncSender<Result<Grabbed, String>>,
}

impl Drop for GrabSink {
    fn drop(&mut self) {
        PENDING_GRABS.fetch_sub(1, Ordering::Relaxed);
    }
}

pub struct EncodeJob {
    pub sink: GrabSink,
    pub width: u32,
    pub height: u32,
    pub pixels: Arc<[u8]>,
    pub stride: usize,
    pub order: GrabOrder,
    pub origin: GrabOrigin,
    pub capture_ms: f64,
    pub backend_png: bool,
    pub bytes: Option<GrabBytes>,
}

pub struct GrabBytes(pub usize);

impl Drop for GrabBytes {
    fn drop(&mut self) {
        GRAB_BYTES.fetch_sub(self.0, Ordering::Relaxed);
    }
}

pub fn encode_queue() -> &'static Result<SyncSender<EncodeJob>, String> {
    static ENCODER: OnceLock<Result<SyncSender<EncodeJob>, String>> = OnceLock::new();
    ENCODER.get_or_init(|| {
        let (tx, rx) = sync_channel::<EncodeJob>(MAX_PENDING_GRABS);
        std::thread::Builder::new()
            .name("makepad-remote-png".into())
            .spawn(move || {
                while let Ok(mut job) = rx.recv() {
                    if job.sink.cancelled.load(Ordering::Relaxed) {
                        continue;
                    }
                    let started = Instant::now();
                    let result = encode_grab(&job).map(|(width, height, png)| Grabbed {
                        window_id: job.sink.window.unwrap_or(0),
                        width,
                        height,
                        png,
                        capture_ms: job.capture_ms,
                        encode_ms: started.elapsed().as_secs_f64() * 1000.0,
                        backend_png: job.backend_png,
                        _bytes: job.bytes.take(),
                    });
                    let _ = job.sink.tx.try_send(result);
                }
            })
            .map_err(|err| format!("grab worker: {err}"))?;
        Ok(tx)
    })
}

/// 2x2 box-filter a raw readback in place of the job's pixels (same
/// channel order and origin, tightly packed); the sink's scale doubles
/// so the requested output size is kept where it still can be.
pub fn halve_grab(job: &mut EncodeJob) -> bool {
    let (w, h, stride) = (job.width as usize, job.height as usize, job.stride);
    if stride < w * 4 || job.pixels.len() < stride * h {
        return false;
    }
    let (w2, h2) = (w / 2, h / 2);
    let mut out = vec![0u8; w2 * h2 * 4];
    for y in 0..h2 {
        let (r0, r1) = (&job.pixels[2 * y * stride..], &job.pixels[(2 * y + 1) * stride..]);
        for x in 0..w2 {
            for c in 0..4 {
                let i = 8 * x + c;
                let sum = r0[i] as u32 + r0[i + 4] as u32 + r1[i] as u32 + r1[i + 4] as u32;
                out[(y * w2 + x) * 4 + c] = ((sum + 2) / 4) as u8;
            }
        }
    }
    job.pixels = Arc::from(out.into_boxed_slice());
    job.width = w2 as u32;
    job.height = h2 as u32;
    job.stride = w2 * 4;
    job.sink.scale = (job.sink.scale * 2.0).min(1.0);
    true
}

pub fn submit_encode(mut job: EncodeJob, raw: bool) {
    if job.sink.cancelled.load(Ordering::Relaxed) {
        return;
    }
    // Over the pixel budget (a fast /gseq of a large window outruns the
    // encoder): halve the frame until it fits instead of failing, so the
    // sequence degrades to smaller frames. A requested scale <= 0.5
    // loses nothing, since the encoder would have scaled down anyway.
    let len = loop {
        let len = job.pixels.len();
        // Keep fetch_update for older stable toolchains without try_update.
        #[allow(deprecated)]
        let fits = GRAB_BYTES
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(len)
                    .filter(|total| *total <= MAX_GRAB_BYTES)
            })
            .is_ok();
        if fits {
            break len;
        }
        if !raw || job.width < 2 || job.height < 2 || !halve_grab(&mut job) {
            let _ = job.sink.tx.try_send(Err(
                "grab pixel budget full; retry at a slower cadence".into()
            ));
            return;
        }
    };
    job.bytes = Some(GrabBytes(len));
    job.backend_png = !raw;
    match encode_queue() {
        Ok(tx) => {
            if let Err(err) = tx.try_send(job) {
                let (std::sync::mpsc::TrySendError::Full(job)
                | std::sync::mpsc::TrySendError::Disconnected(job)) = err;
                let _ = job.sink.tx.try_send(Err("grab encoder queue full".into()));
            }
        }
        Err(err) => {
            let _ = job.sink.tx.try_send(Err(err.clone()));
        }
    }
}

pub struct QueuedCmd {
    pub cmd: Cmd,
    pub user_seq: Option<u64>,
    pub deadline: Instant,
}

pub enum Cmd {
    Activity(Sender<Reply>),
    Input {
        window: Option<usize>,
        inputs: Vec<Input>,
        wait: bool,
        tx: Sender<Reply>,
    },
    Grab {
        window: Option<usize>,
        ids: Vec<u64>,
        started: Instant,
        every: Duration,
        cancelled: Arc<AtomicBool>,
        tx: Sender<Reply>,
    },
    CancelGrabs(Vec<u64>),
    Dump {
        /// One line per widget with its id path instead of the tree.
        paths: bool,
        tx: Sender<Reply>,
    },
    /// The task pool's one-line summary (workers, jobs, queue waits).
    PoolSummary(Sender<Reply>),
    Snap {
        window: Option<usize>,
        needle: String,
        /// `q=path:a.b` / `path=a.b`: match id paths, exactly or as a
        /// suffix of whole segments, instead of the substring search.
        path: Option<String>,
        all: bool,
        tx: Sender<Reply>,
    },
    /// Run `frames` frames of the virtual clock at `fps`; answered after
    /// the last is presented (and captured, while a capture runs).
    Step {
        window: Option<usize>,
        frames: u64,
        fps: u32,
        /// `wait_loads=1`: no frame opens while asynchronous loads are in
        /// flight (bounded; the step fails naming them).
        wait_loads: bool,
        tx: Sender<Reply>,
    },
    /// Is anything still moving: a pending NextFrame, redraw, repaint or
    /// virtual timer due within the next frame?
    Settled(Sender<Reply>),
    CapStop(Sender<Reply>),
    /// `/cap/pause` (true) and `/cap/resume` (false).
    CapPause(bool, Sender<Reply>),
    /// A command built by a platform-side route (`/cap/start`, `/cursor`):
    /// its payload names platform types, so only platform's `apply` reads it.
    Platform(Box<dyn std::any::Any + Send>),
    Close {
        window: Option<usize>,
        tx: Sender<Reply>,
    },
    /// Give a window a new inner size, in layout points, the way a
    /// person dragging its edge would. Answered after the frame that
    /// follows, so a grab right after sees the new layout.
    Resize {
        window: Option<usize>,
        size: Vec2d,
        tx: Sender<Reply>,
    },
    /// A tweaker-overlay operation. The route only parses; the whole
    /// answer comes from `Cx::tweak_callback` (registered by the widgets
    /// crate), so platform stays below widgets in the dependency order.
    Tweak {
        op: String,
        args: Vec<(String, String)>,
        /// Answer only after the next frame is drawn, so a following
        /// grab sees the applied change on screen.
        wait: bool,
        tx: Sender<Reply>,
    },
    /// An AI-overlay operation (`/ai`, `/ai/transcript`): parsed here,
    /// answered by `Cx::ai_callback` (registered by the aichat crate).
    Ai {
        op: String,
        args: Vec<(String, String)>,
        wait: bool,
        tx: Sender<Reply>,
    },
    Quit(Sender<Reply>),
    /// The hot-patchable constant tables of the compiled shaders (one
    /// shader, or all that have entries).
    ShaderConsts {
        shader: Option<usize>,
        tx: Sender<Reply>,
    },
    /// Patch (or, with `value` None, reset) one table constant. Lands on
    /// the GPU next frame with no recompile.
    ShaderConstPatch {
        shader: usize,
        index: usize,
        value: Option<f32>,
        tx: Sender<Reply>,
    },
}

impl Cmd {
    pub fn mutation_reply(&self) -> Option<&Sender<Reply>> {
        match self {
            Self::Input { tx, .. } | Self::Close { tx, .. }
            | Self::Quit(tx) | Self::ShaderConstPatch { tx, .. } => Some(tx),
            Self::Tweak { op, tx, .. }
                if !matches!(
                    op.as_str(),
                    "state"
                        | "diff"
                        | "final"
                        | "design_state"
                        | "design_palette"
                        | "design_patch"
                        | "design_commit"
                        | "design_verify"
                ) =>
            {
                Some(tx)
            }
            Self::Ai { op, tx, .. } if op != "transcript" => Some(tx),
            _ => None,
        }
    }
}

pub enum Input {
    Mouse {
        kind: MouseKind,
        x: f64,
        y: f64,
        button: u32,
        dx: f64,
        dy: f64,
        mods: RemoteKeyModifiers,
        /// Hardware-faithful: route through the same pointer-lock/pin
        /// transform physical mouse events take (`/m?hw=1`).
        hw: bool,
        /// The event's timestamp on the app clock (`/m?time=`), or now:
        /// drag samples with their own times drive velocity-dependent
        /// gestures (a flick's momentum) the same way every run.
        time: Option<f64>,
    },
    /// One step of a trackpad pinch (`/m?k=pinch&scale=&phase=`).
    Pinch {
        x: f64,
        y: f64,
        scale: f64,
        phase: PinchPhase,
        mods: RemoteKeyModifiers,
        time: Option<f64>,
    },
    Key {
        down: bool,
        code: KeyCode,
        mods: RemoteKeyModifiers,
    },
    Text(String),
    DropFile {
        path: String,
        x: f64,
        y: f64,
    },
    /// The window's own maximise (macOS: toggleFullScreen) — the test
    /// hook for the maximise/occlusion proof.
    Maximize,
    /// Resize the window's inner size (points): responsive-layout checks
    /// without relaunching.
    Resize(f64, f64),
    /// Ask the window what the OS asks before a native press: is this
    /// point a window drag (Caption) or the app's (Client)? Remote
    /// clicks skip that question, so this is how a check sees it.
    DragQuery(f64, f64),
}

#[derive(Clone, Copy, PartialEq)]
pub enum MouseKind {
    Move,
    Down,
    Up,
    Scroll,
}

/// What `poll` hands back to a waiting HTTP thread.
pub enum Reply {
    Ok,
    /// Deferred until the next rendered frame; the sender is re-armed by
    /// `poll` in a later tick.
    Text(String),
    Err(String),
    Conflict(String),
}

/// `--remote`, `--remote=PORT`, `--remote PORT`, `--remote=HOST:PORT`,
/// `--remote HOST:PORT`, or `MAKEPAD_REMOTE=1|PORT|HOST:PORT`.
///
/// The default host is loopback. Naming a host (`0.0.0.0:8399`,
/// `10.0.0.5:8399`) binds that interface instead so another machine can
/// drive this app — the fleet-box case, where the controlling agent sits
/// on a different computer. Only do that on a trusted network: this
/// surface injects real mouse/keyboard input and serves screen grabs.
/// IPv4 or hostname only.
pub fn requested_bind() -> Option<(String, u16)> {
    fn parse(value: &str) -> (String, u16) {
        let value = value.trim();
        if let Some((host, port)) = value.rsplit_once(':') {
            if let Ok(port) = port.parse::<u16>() {
                if !host.is_empty() {
                    return (host.to_string(), port);
                }
            }
        }
        ("127.0.0.1".to_string(), value.parse::<u16>().unwrap_or(0))
    }
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg == "--remote" {
            // an immediately following bare port or host:port is the bind
            if let Some(next) = args.next() {
                let next = next.trim().to_string();
                if next.parse::<u16>().is_ok() || next.contains(':') {
                    return Some(parse(&next));
                }
            }
            return Some(("127.0.0.1".to_string(), 0));
        }
        if let Some(value) = arg.strip_prefix("--remote=") {
            return Some(parse(value));
        }
    }
    match std::env::var("MAKEPAD_REMOTE") {
        Ok(v) => {
            let v = v.trim().to_string();
            let lower = v.to_ascii_lowercase();
            if lower.is_empty()
                || lower == "0"
                || lower == "off"
                || lower == "false"
                || lower == "no"
            {
                None
            } else if v.parse::<u16>().is_ok() || v.contains(':') {
                Some(parse(&v))
            } else {
                Some(("127.0.0.1".to_string(), 0))
            }
        }
        Err(_) => None,
    }
}

/// `--remote-title-tag=NAME` overrides the suffix; `off`/`none`/empty
/// disables it. Default is `[remote]`.
pub fn title_tag() -> Option<String> {
    for arg in std::env::args() {
        if let Some(value) = arg.strip_prefix("--remote-title-tag=") {
            let value = value.trim();
            if value.is_empty() || value == "off" || value == "none" {
                return None;
            }
            return Some(format!("[{value}]"));
        }
    }
    Some("[remote]".to_string())
}

/// Mark a `--remote` instance's windows so a human who finds one lingering
/// on screen can tell it belongs to an agent and close it guilt-free.
/// Idempotent: re-titling a window does not stack tags.
pub fn tag_window_title(title: String) -> String {
    if !ACTIVE.load(Ordering::Relaxed) {
        return title;
    }
    let Some(tag) = title_tag() else {
        return title;
    };
    if title.ends_with(&tag) {
        return title;
    }
    if title.is_empty() {
        return tag;
    }
    format!("{title} {tag}")
}

pub fn app_name() -> String {
    std::env::args_os()
        .next()
        .as_ref()
        .and_then(|a| std::path::Path::new(a).file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("makepad")
        .to_string()
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// The OS asked whether a window may close and the app said yes. Only the
/// native close button / Cmd-W reach that delegate — an app closing its own
/// window does not — so this is the precise "the human dismissed it" signal.
pub fn note_window_close_requested(window_id: usize) {
    if let Ok(mut pending) = close_requested().lock() {
        if !pending.contains(&window_id) {
            pending.push(window_id);
        }
    }
}

/// Consume the flag set by [`note_window_close_requested`].
pub fn take_window_close_requested(window_id: usize) -> bool {
    match close_requested().lock() {
        Ok(mut pending) => match pending.iter().position(|id| *id == window_id) {
            Some(index) => {
                pending.remove(index);
                true
            }
            None => false,
        },
        Err(_) => false,
    }
}

pub fn close_requested() -> &'static Mutex<Vec<usize>> {
    static R: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn window_gone_reason(window_id: usize) -> String {
    let closed = closed_windows().lock().ok();
    let hit = closed
        .as_ref()
        .and_then(|c| c.iter().find(|(id, _)| *id == window_id));
    match hit {
        Some(_) => format!("window {window_id} closed by user"),
        None => format!("no window {window_id}"),
    }
}

pub fn is_grab_id(id: u64) -> bool {
    // File sinks occupy 1<<63 and probes use 1<<40; preserve both.
    id >= GRAB_ID_BASE && id < (1 << 63)
}

pub fn handle_conn(mut stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(index) = find_head_end(&buf) {
            break index;
        }
        if buf.len() > MAX_HEAD_BYTES {
            let _ = respond(&mut stream, 431, "application/json", b"{\"err\":\"head\"}");
            return;
        }
        match stream.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return,
        }
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let Some(request_line) = lines.next() else {
        return;
    };
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();

    let mut content_length = 0usize;
    let mut from_browser = false;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            // Browsers send Origin on cross-site requests; an agent's
            // curl does not.
            if name.trim().eq_ignore_ascii_case("origin") {
                from_browser = true;
            }
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value
                    .trim()
                    .parse::<usize>()
                    .unwrap_or(0)
                    .min(MAX_BODY_BYTES);
            }
        }
    }

    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < content_length {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    body.truncate(content_length);

    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path.to_string(), query.to_string()),
        None => (target.clone(), String::new()),
    };
    // `/cap/start` writes a file the caller names: never on behalf of a
    // web page (the bridge answers any origin).
    if from_browser && path.starts_with("/cap/") {
        let _ = respond(
            &mut stream,
            403,
            "application/json",
            b"{\"err\":\"capture routes refuse browser (Origin) requests\"}",
        );
        return;
    }
    let mut params = parse_query(&query);
    if !body.is_empty() {
        params.extend(parse_flat_json(&String::from_utf8_lossy(&body)));
    }
    let params = Params(params);

    let response = route(&method, &path, &params);
    let _ = match response {
        Out::Json(status, text) => {
            respond(&mut stream, status, "application/json", text.as_bytes())
        }
        Out::Text(status, text) => respond(
            &mut stream,
            status,
            "text/plain; charset=utf-8",
            text.as_bytes(),
        ),
        Out::Png(bytes) => respond(&mut stream, 200, "image/png", &bytes),
    };
}

pub fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

pub fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        408 => "Request Timeout",
        403 => "Forbidden",
        409 => "Conflict",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let mut out = Vec::with_capacity(body.len() + 256);
    out.extend_from_slice(
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Expose-Headers: X-Makepad-User-Seq, X-Makepad-User-Seq-Start\r\nX-Makepad-User-Seq: {}\r\nX-Makepad-User-Seq-Start: {}\r\nConnection: close\r\n\r\n",
            body.len(), user_seq_now(), REQUEST_START_USER_SEQ.get()
        )
        .as_bytes(),
    );
    out.extend_from_slice(body);
    stream.write_all(&out)?;
    stream.flush()
}

pub enum Out {
    Json(u16, String),
    Text(u16, String),
    Png(Vec<u8>),
}

pub fn err(msg: &str) -> Out {
    Out::Json(404, format!("{{\"err\":{}}}", json_str(msg)))
}

pub fn route(method: &str, path: &str, p: &Params) -> Out {
    REQUEST_START_USER_SEQ.set(user_seq_now());
    let expected = match p.get(&["if_user_seq"]) {
        Some(value) => match value.parse::<u64>() {
            Ok(value) => Some(value),
            Err(_) => return Out::Json(400, "{\"err\":\"if_user_seq must be an unsigned integer\"}".into()),
        },
        // A script that does not track the sequence is gated by the quiet
        // period alone; one that does is refused once the person has
        // intervened since the sequence it named.
        None => None,
    };
    REQUEST_USER_SEQ.set(expected);
    if method != "GET" && method != "POST" && method != "HEAD" {
        return Out::Json(400, "{\"err\":\"method\"}".to_string());
    }
    match path {
        "/" | "/help" => Out::Text(200, cheat_sheet()),
        "/s" | "/status" => route_status(p),
        "/activity" => reply_to_out(ask(Cmd::Activity, ACTIVITY_WAIT_SECS)),
        "/g" | "/grab" => route_grab(p),
        "/gseq" => route_grab_sequence(p),
        "/gq" => route_grab_quit(p),
        "/m" | "/mouse" => route_mouse(p, None),
        "/click" => route_mouse(p, Some("click")),
        "/k" | "/key" => route_key(p),
        "/t" | "/text" => route_text(p),
        "/drop" => route_drop(p),
        "/trace" => route_trace(p),
        "/d" | "/dump" => {
            let paths = p.flag(&["paths"]);
            match ask(move |tx| Cmd::Dump { paths, tx }, 4) {
                Reply::Text(text) => Out::Text(200, text),
                other => reply_to_out(other),
            }
        }
        "/snap" => {
            let window = p.window();
            let mut needle = p.get(&["q", "query"]).unwrap_or_default().to_string();
            let mut path = p.get(&["path"]).map(str::to_string);
            if let Some(query) = needle.strip_prefix("path:") {
                path = Some(query.to_string());
                needle.clear();
            }
            let all = p.flag(&["all"]);
            reply_to_out(ask(
                move |tx| Cmd::Snap {
                    window,
                    needle,
                    path,
                    all,
                    tx,
                },
                4,
            ))
        }
        "/step" => route_step(p),
        "/settled" => reply_to_out(ask(Cmd::Settled, 4)),
        "/cap/pause" => reply_to_out(ask(|tx| Cmd::CapPause(true, tx), 4)),
        "/cap/resume" => reply_to_out(ask(|tx| Cmd::CapPause(false, tx), 4)),
        "/close" => {
            let window = p.window();
            reply_to_out(ask(move |tx| Cmd::Close { window, tx }, 4))
        }
        // A responsive layout is checked by resizing, so the bridge can:
        // `/resize?w=&h=[&window=]` in layout points, answered after the
        // frame that shows it; a following `/s` reports the size taken.
        "/resize" => {
            // `w` is the width here, so the window goes by `window=`.
            let window = p.get(&["window"]).and_then(|v| v.parse::<usize>().ok());
            let (w, h) = (p.f64(&["w"], 0.0), p.f64(&["h"], 0.0));
            let sane = |v: f64| v.is_finite() && (1.0..=16384.0).contains(&v);
            if !sane(w) || !sane(h) {
                return Out::Text(400, "give w= and h= in layout points, 1 to 16384".to_string());
            }
            reply_to_out(ask(move |tx| Cmd::Resize { window, size: dvec2(w, h), tx }, 6))
        }
        // `/w?k=maximize`: the window's own maximise, answered after the
        // next drawn frame with `wait` (the maximise/occlusion proof).
        "/w" | "/window" => match p.get(&["k", "kind"]) {
            Some("maximize") | Some("max") => send_input(p.window(), vec![Input::Maximize], p.flag(&["wait"])),
            Some("dragquery") => match (p.get(&["x"]).and_then(|v| v.parse::<f64>().ok()), p.get(&["y"]).and_then(|v| v.parse::<f64>().ok())) {
                (Some(x), Some(y)) => send_input(p.window(), vec![Input::DragQuery(x, y)], p.flag(&["wait"])),
                _ => err("dragquery needs x= and y= (window points)"),
            },
            Some("resize") => match (p.get(&["width"]).and_then(|v| v.parse::<f64>().ok()), p.get(&["height"]).and_then(|v| v.parse::<f64>().ok())) {
                (Some(w), Some(h)) if w >= 1.0 && h >= 1.0 => send_input(p.window(), vec![Input::Resize(w, h)], p.flag(&["wait"])),
                _ => err("resize needs width= and height= (points)"),
            },
            other => err(&format!("unknown window op {other:?}; k=maximize|resize|dragquery")),
        },
        // The tweaker overlay (design feedback). Thin: parse here, decide
        // in the widgets-side callback. `wait` answers after the next
        // drawn frame so a following grab sees the change.
        // The AI chat overlay (apps/aichat, seated in every Window on F10):
        // `/ai?on=1|0` toggles, `/ai?say=TEXT` types a line, `/ai/transcript`
        // reads the conversation. Answered by `Cx::ai_callback`.
        "/ai" => route_ai(
            if p.get(&["say"]).is_some() {
                "say"
            } else {
                "toggle"
            },
            p,
            true,
        ),
        "/ai/transcript" => route_ai("transcript", p, false),
        // The red hands-off frame, held up or let go by hand. It lights
        // by itself for a few seconds after any injected input, and
        // shows from the next frame the app draws.
        "/handsoff" => {
            let on = p.get(&["on"]).map(|v| v.to_string()).unwrap_or_else(|| "1".to_string()) != "0";
            HANDS_OFF.store(on, Ordering::Relaxed);
            Out::Json(200, format!("{{\"handsoff\":{}}}", on as u8))
        }
        "/tweak" => route_tweak("toggle", p, true),
        "/tweak/state" => route_tweak("state", p, false),
        "/tweak/apply" => route_tweak("apply", p, true),
        "/tweak/diff" => route_tweak("diff", p, false),
        "/tweak/clear" => route_tweak("clear", p, false),
        "/tweak/final" => route_tweak_final(p),
        // Escape hatch for tweaker ops that don't have (or need) a named
        // route yet: /tweak/op?op=NAME&... — the callback decides.
        "/tweak/op" => match p.get(&["op"]) {
            Some(op) => route_tweak(&op.to_string(), p, false),
            None => err("need op="),
        },
        // The designer: structural edits to the Splash source under
        // design, previewed live, never written by the app. The route
        // only names the op; the tweaker answers it.
        design if design.starts_with("/design/") => route_design(&design["/design/".len()..], p),
        // The window PNG with the overlay's outlines/annotations in it:
        // the overlay draws inside the window's own pass, so the ordinary
        // grab pipeline already composites it.
        "/tweak/grab" => route_grab(p),
        // The shader constant tables: annotated literals compiled into
        // a hot-patchable uniform buffer (see Cx::shader_const_patch).
        "/shader/consts" => {
            let shader = p
                .get(&["shader", "s"])
                .and_then(|v| v.parse::<usize>().ok());
            reply_to_out(ask(move |tx| Cmd::ShaderConsts { shader, tx }, 4))
        }
        "/shader/const" => {
            let Some(shader) = p
                .get(&["shader", "s"])
                .and_then(|v| v.parse::<usize>().ok())
            else {
                return err("need shader=ID");
            };
            let Some(index) = p.get(&["i", "index"]).and_then(|v| v.parse::<usize>().ok())
            else {
                return err("need i=INDEX");
            };
            let value = match p.get(&["v", "value"]) {
                Some(v) => match v.parse::<f32>() {
                    Ok(v) => Some(v),
                    Err(_) => return err("v= must be a number"),
                },
                None => {
                    if p.flag(&["reset"]) {
                        None
                    } else {
                        return err("need v=VALUE or reset=1");
                    }
                }
            };
            reply_to_out(ask(
                move |tx| Cmd::ShaderConstPatch {
                    shader,
                    index,
                    value,
                    tx,
                },
                4,
            ))
        }
        "/quit" => reply_to_out(ask(|tx| Cmd::Quit(tx), 4)),
        // `/cap/start`, `/cap/stop`, `/cursor`, `/midi`, `/log`: routes that
        // need platform state are answered by platform.
        _ => (hooks().route)(path, p).unwrap_or_else(|| err("no route")),
    }
}

pub fn cheat_sheet() -> String {
    let status = status_cell().lock().unwrap();
    let dir = grab_dir().lock().unwrap().display().to_string();
    format!(
        "makepad-remote  app={} pid={}  windows={}  grabs={}\n\
         all routes are GET; every answer is one line of JSON; x/y are layout points, window-local, y down\n\
         /                 this sheet\n\
         /s[?w=ID]         {{\"app\":..,\"pid\":..,\"w\":[{{\"i\":id,\"t\":title,\"sz\":[w,h],\"px\":[w,h],\"dpi\":f,\"pos\":[x,y]}}]}}\n\
         /activity         native user activity: user_active, user_seq, idle_ms, quiet_ms, held, last_input, window (also in /s)\n\
         \x20                 native input increments user_seq; injected input does not. No input contents are recorded\n\
         \x20                 mutations need 2 seconds without native input; with if_user_seq=N they are also refused once the person intervened after N; held pointer/touch input stays active\n\
         \x20                 HTTP 409 user_interacting/user_intervened rejects this sequence; inspect applied/state before retrying with a fresh activity counter\n\
         \x20                 all replies include X-Makepad-User-Seq[-Start]; changed epochs invalidate test/capture attribution\n\
         \x20                 a window the HUMAN closed is reported as {{\"err\":\"window N closed by user\"}} — a normal window close, not a crash\n\
         /g?w=&scale=&raw= grab window w (default: first). returns {{\"png\":path,\"w\":id,\"sz\":[w,h],\"capture_ms\":ms,\"encode_ms\":ms}}; raw=1 sends image/png bytes\n\
         \x20                 standalone macOS: pending Draw + immediate present at UI arming, before later input; no animation tick. Other backends: next render\n\
         /gseq?n=8&every_ms=50&scale=1  a separate present per deadline; n=1..64, every_ms>=8, span<=60s; {{\"png\":[paths],\"frames\":[per-frame timings]}}\n\
         \x20                 scheduled_ms and pixels_ms share the request origin; capture_ms = pixels_ms - scheduled_ms; cadence never waits for PNG encoding\n\
         \x20                 commands follow UI queue order (concurrent sockets have no client-time order); macOS wait=1 input replies after submitting its applied frame\n\
         /m?k=&x=&y=&w=    mouse. k=move|down|up|click|scroll|pinch  b=0 left,1 right,2 middle  scroll: dx=,dy=  pinch: scale= (relative to the previous step), phase=begin|update|end\n\
                           time= stamps the event in app-clock seconds (default: now) so drag samples carry their own timing\n\
                           add hw=1 to take the hardware pointer path (pointer-lock/pin transform included)\n\
                           shift=1 ctrl=1 alt=1 cmd=1 hold modifiers down for the press: a\n\
                           gesture that only exists under a modifier cannot be driven\n\
                           without them\n\
         /click?x=&y=      alias for /m?k=click\n\
         /k?t=TEXT         type text. or /k?k=down|up&c=KeyA (Escape ReturnKey Tab Backspace ArrowLeft F1 Key1 ..)\n\
         /t?t=TEXT         same as /k?t=\n\
         /drop?path=&x=&y= drop one absolute file path through Drag/Drop/DragEnd; optional w= and wait=1; app validates/loads it\n\
         /log?n=50         {{\"n\":lastseq,\"l\":[lines]}}; /log?since=N for everything after seq N\n\
         /midi?a=&b=&c=    inject a MIDI message as if a device sent it (three bytes; p= port id or name)\n\
         \x20                 /midi?k=ports&n=NAME[,NAME] declares ports for the app to adopt (NAME:i / NAME:o for one end)
                           /midi?k=out reads back what the app SENT (its LED writes); /midi?k=reset forgets both
                           it enters at the app's own receive, so enumeration and the OS handles are NOT proved
         /trace             get topics; ?topics=gpu.pass,wm sets them; ?off=1 clears them\n\
         /snap?q=&w=&all=  widget rects, ready to click: {{\"s\":[{{\"i\":id,\"ty\":type,\"r\":[x,y,w,h],\"w\":win,\"t\":text,\"p\":path}}]}}\n\
         \x20                 q= filters id/type/text (substring); default lists only visible, sized widgets\n\
         \x20                 q=path:a.b (or path=a.b) matches the id path p exactly or as a suffix of whole segments\n\
         /d                whole widget tree as indented text (id, type, x y w h); /d?paths=1 one line per widget: path type x y w h\n\
         /step?frames=K&fps=60  --virtual-clock only: run K frames of 1/fps (timers, NextFrame, draw, present); answers {{\"frame\",\"time\"}} after the K-th\n\
         \x20                 wait_loads=1: no frame opens while async loads (image decodes, glyph rasters, resources) are in flight; fails after 30 s naming them\n\
         /settled          {{\"settled\":bool,\"reasons\":[next_frame|redraw|repaint|timer|step|shaders|loads],\"loads\":{{kind:n}}}}\n\
         /cap/start?path=/ABS.mp4&fps=60&audio=1&overwrite=1  record the window in process (native pixels, H.264); virtual clock: one frame per /step frame (step at the same fps), pts=n/fps\n\
         \x20                 the directory must exist; an existing file needs overwrite=1; quitting with a capture open finishes the file\n\
         /cap/stop[?hashes=1]  finalize; {{\"frames\",\"sz\",\"missing\",\"hash\"}}\n\
         /cap/pause, /cap/resume  stop / restart writing without stopping the clock; the file stays gapless (pts count written frames); {{\"paused\"|\"resumed\":1,\"frames\":N}}\n\
         /cursor?show=1|0&style=arrow|hand|text|crosshair|app&x=&y=  a pointer drawn into the frame, moved by injected mouse input, ripple on press\n\
         \x20                 launch flags: --virtual-clock --window WxH@scale --seed N (MAKEPAD_VIRTUAL_CLOCK=1, MAKEPAD_SEED=N)\n\
         /tweak?on=1|0     the TWEAKER design-feedback overlay (also Shift+F10 in-app). hover outlines widgets; click pins; buttons never fire\n\
         /handsoff?on=1|0  a red frame round the window: the bridge is driving, hands off. it lights by itself for 3s after any /m /k /t\n\
         /tweak/state      selection + its editable properties + diff log + annotations + asks waiting on the source (renames; converts: grid, flex, dock with its \"do\"), one JSON\n\
         /tweak/apply      POST {{\"path\":\"a.b.c\",\"splash\":\"{{padding: 20}}\"}} or {{\"path\":..,\"prop\":\"padding\",\"value\":\"20\"}} — live-apply + relayout\n\
         /tweak/diff       the raw edit log; POST /tweak/clear resets it\n\
         /tweak/final      coalesced end state per widget (original -> final); adds \"png\" when the user drew\n\
         /tweak/grab       window grab with the overlay composited (same as /g while tweaking)\n\
         /design/open?path=  start a design session on the file that declares path (default: the pinned widget)\n\
         /design/state     session: file, edits, status, can_undo/redo, landing, hunks; /design/palette the insertable types\n\
         /design/insert?path=&place=before|after|inside&type=Button[&body={{text: \"Hi\"}}]  add a widget; answers the new path in \"select\"\n\
         /design/move?path=&to=&place=  move path next to / into to.  /design/delete|duplicate|wrap|up|down|out?path=\n\
         /design/rename?path=&name=  (empty name drops it)   /design/set?path=&key=&value=  write a property into the literal\n\
         /design/bake      write the value ledger's tweaks into the source; /design/undo|redo|reset step the source edits\n\
         /design/patch     the unified diff; /design/commit {{file,base_hash,new_hash,base_matches_disk,hunks,diff}} (the app never writes)\n\
         /design/verify    every widget of the file under design: found in the source or not; /design/close writes local/design/<file>.patch\n\
         /shader/consts    the compiled shaders' hot-patchable constants (annotated literals): {{\"shaders\":[{{\"id\",\"consts\":[{{\"i\",\"name\",\"value\",\"min\",\"max\",\"step\",\"file\",\"line\"}}]}}]}}; shader=ID for one\n\
         /shader/const     ?shader=ID&i=N&v=VALUE patches one constant on the GPU (no recompile, source untouched); reset=1 puts the literal back\n\
         /close?w=ID       close one window the normal way\n\
         /resize?w=&h=     give the window a new inner size, in layout points (like dragging its edge); answers after the frame that shows it\n\
         /gq[?scale=&w=]   FINISH HERE: grab every window, then quit. {{\"png\":[paths],\"quit\":1}}\n\
         /quit             shut the app down gracefully (no final grab)\n\
         finish owned tests with /gq (or /quit); a conflict invalidates the sequence evidence, not authorization for the active workflow\n\
         add &wait=1 to any input route to answer only after the next frame is drawn (so a following /g sees it)\n\
         add &w=ID to target a window; omit for the first one. ordinary errors are {{\"err\":\"...\"}} with status 404; interaction conflicts use 409\n\
         POST the same routes with a flat JSON body ({{\"x\":10,\"y\":20}}) when quoting query strings is painful\n",
        status.app,
        status.pid,
        status.windows.len(),
        dir,
    )
}

/// `/step?frames=k[&fps=60][&w=]`: answered after the k-th frame.
pub fn route_step(p: &Params) -> Out {
    let frames = match p.get(&["frames", "n"]).unwrap_or("1").parse::<u64>() {
        Ok(frames) if frames >= 1 => frames,
        _ => return err("step frames must be a positive integer"),
    };
    let fps = match p.get(&["fps"]).unwrap_or("60").parse::<u32>() {
        Ok(fps @ 1..=1000) => fps,
        _ => return err("step fps must be an integer in 1..=1000"),
    };
    let window = p.window();
    let wait_loads = p.flag(&["wait_loads"]);
    // A frame normally takes well under a second; the bound only keeps
    // a wedged app from pinning this request thread. Waiting on loads adds
    // their own bound (30 s) once per wait.
    let timeout = 30 + frames + if wait_loads { 30 } else { 0 };
    reply_to_out(ask(
        move |tx| Cmd::Step {
            window,
            frames,
            fps,
            wait_loads,
            tx,
        },
        timeout,
    ))
}

pub fn route_trace(p: &Params) -> Out {
    if p.flag(&["off"]) {
        makepad_error_log::set_trace_topics("");
    } else if let Some(topics) = p.get(&["topics"]) {
        makepad_error_log::set_trace_topics(topics);
    }
    Out::Json(
        200,
        format!(
            "{{\"topics\":{}}}",
            json_str(&makepad_error_log::trace_topics())
        ),
    )
}

pub fn route_ai(op: &str, p: &Params, wait: bool) -> Out {
    let op = op.to_string();
    let args = p.0.clone();
    let timeout = if wait { 6 } else { 4 };
    reply_to_out(ask(move |tx| Cmd::Ai { op, args, wait, tx }, timeout))
}

/// `/design/<op>`: reads answer at once; edits answer after the frame
/// that shows them, so a grab right after sees the change.
pub fn route_design(op: &str, p: &Params) -> Out {
    if op.is_empty() || !op.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
        return Out::Json(404, "{\"err\":\"unknown design op\"}".to_string());
    }
    let read = matches!(op, "state" | "palette" | "patch" | "commit" | "verify");
    route_tweak(&format!("design_{op}"), p, !read)
}

pub fn route_tweak(op: &str, p: &Params, wait: bool) -> Out {
    let op = op.to_string();
    let args = p.0.clone();
    // A `wait` op only resolves on the next drawn frame; give it the
    // same slack as an input wait.
    let timeout = if wait { 6 } else { 4 };
    reply_to_out(ask(move |tx| Cmd::Tweak { op, args, wait, tx }, timeout))
}

/// `/tweak/final` — the coalesced end state. When the answer says the
/// user drew (`"drew":1`), grab the window too and name the composited
/// PNG in the same JSON, so the caller sees what the drawings mean
/// without a second round trip.
pub fn route_tweak_final(p: &Params) -> Out {
    let args = p.0.clone();
    let reply = ask(
        move |tx| Cmd::Tweak {
            op: "final".to_string(),
            args,
            wait: false,
            tx,
        },
        4,
    );
    let mut json = match reply {
        Reply::Text(text) => text,
        other => return reply_to_out(other),
    };
    if json.contains("\"drew\":1") && json.ends_with('}') {
        match grab_one(p.window(), p.f64(&["scale"], 1.0))
            .and_then(|grabbed| write_grab(grabbed.window_id, &grabbed.png))
        {
            Ok(path) => {
                json.pop();
                json.push_str(&format!(
                    ",\"png\":{}}}",
                    json_str(&path.display().to_string())
                ));
            }
            Err(msg) => {
                json.pop();
                json.push_str(&format!(",\"png_err\":{}}}", json_str(&msg)));
            }
        }
    }
    Out::Json(200, json)
}

pub fn route_status(p: &Params) -> Out {
    let status = status_cell().lock().unwrap();
    let want = p.window();
    if let Some(want) = want {
        if !status.windows.iter().any(|w| w.id == want) {
            return err(&window_gone_reason(want));
        }
    }
    let mut out = format!(
        "{{\"app\":{},\"pid\":{},\"w\":[",
        json_str(&status.app),
        status.pid
    );
    let mut index = 0;
    for window in status.windows.iter() {
        if want.is_some_and(|want| want != window.id) {
            continue;
        }
        if index > 0 {
            out.push(',');
        }
        index += 1;
        out.push_str(&format!(
            "{{\"i\":{},\"t\":{},\"sz\":[{},{}],\"px\":[{},{}],\"dpi\":{},\"pos\":[{},{}]}}",
            window.id,
            json_str(&window.title),
            num(window.w),
            num(window.h),
            num((window.w * window.dpi).round()),
            num((window.h * window.dpi).round()),
            num(window.dpi),
            num(window.x),
            num(window.y),
        ));
    }
    out.push(']');
    // Windows the human dismissed. Their absence is intentional, not a crash.
    if let Ok(closed) = closed_windows().lock() {
        if !closed.is_empty() {
            out.push_str(",\"closed\":[");
            for (index, (id, title)) in closed.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&format!(
                    "{{\"i\":{},\"t\":{},\"by\":\"user\"}}",
                    id,
                    json_str(title)
                ));
            }
            out.push(']');
        }
    }
    drop(status);
    match ask(Cmd::Activity, ACTIVITY_WAIT_SECS) {
        Reply::Text(activity) => out.push_str(&format!(",\"activity\":{activity}}}")),
        other => return reply_to_out(other),
    }
    Out::Json(200, out)
}

pub fn route_mouse(p: &Params, force_kind: Option<&str>) -> Out {
    note_injected_input();
    let window = p.window();
    let kind = force_kind
        .map(str::to_string)
        .or_else(|| p.get(&["k", "kind"]).map(str::to_string))
        .unwrap_or_else(|| "move".to_string());
    let x = p.f64(&["x"], 0.0);
    let y = p.f64(&["y"], 0.0);
    let button = p.f64(&["b", "button"], 0.0).max(0.0) as u32;
    let dx = p.f64(&["dx", "sx"], 0.0);
    let dy = p.f64(&["dy", "sy"], 0.0);
    let mods = p.mods();
    let hw = p.flag(&["hw"]);
    let time = p.get(&["time"]).and_then(|v| v.parse::<f64>().ok());
    let mouse = |kind| Input::Mouse {
        kind,
        x,
        y,
        button,
        dx,
        dy,
        mods,
        hw,
        time,
    };
    let inputs = match kind.as_str() {
        "move" => vec![mouse(MouseKind::Move)],
        "down" => vec![mouse(MouseKind::Move), mouse(MouseKind::Down)],
        "up" => vec![mouse(MouseKind::Up)],
        "click" | "tap" => vec![
            mouse(MouseKind::Move),
            mouse(MouseKind::Down),
            mouse(MouseKind::Up),
        ],
        "scroll" | "wheel" => vec![mouse(MouseKind::Scroll)],
        "pinch" => {
            let phase = match p.get(&["phase"]).unwrap_or("update") {
                "begin" => PinchPhase::Begin,
                "update" => PinchPhase::Update,
                "end" => PinchPhase::End,
                other => return err(&format!("bad pinch phase {other}")),
            };
            vec![Input::Pinch {
                x,
                y,
                scale: p.f64(&["scale", "s"], 1.0),
                phase,
                mods,
                time,
            }]
        }
        other => return err(&format!("bad kind {other}")),
    };
    send_input(window, inputs, p.flag(&["wait"]))
}

pub fn route_key(p: &Params) -> Out {
    note_injected_input();
    let window = p.window();
    let mods = p.mods();
    if let Some(text) = p.get(&["t", "text"]) {
        return send_input(
            window,
            vec![Input::Text(text.to_string())],
            p.flag(&["wait"]),
        );
    }
    let Some(name) = p.get(&["c", "code", "key", "key_code"]) else {
        return err("need t= (text) or c= (key code)");
    };
    let Some(code) = parse_key_code(name) else {
        return err(&format!("bad key code {name}"));
    };
    let kind = p.get(&["k", "kind"]).unwrap_or("press");
    let inputs = match kind {
        "down" => vec![Input::Key {
            down: true,
            code,
            mods,
        }],
        "up" => vec![Input::Key {
            down: false,
            code,
            mods,
        }],
        "press" | "tap" => vec![
            Input::Key {
                down: true,
                code,
                mods,
            },
            Input::Key {
                down: false,
                code,
                mods,
            },
        ],
        other => return err(&format!("bad kind {other}")),
    };
    send_input(window, inputs, p.flag(&["wait"]))
}

pub fn route_text(p: &Params) -> Out {
    note_injected_input();
    let Some(text) = p.get(&["t", "text"]) else {
        return err("need t=");
    };
    send_input(
        p.window(),
        vec![Input::Text(text.to_string())],
        p.flag(&["wait"]),
    )
}

pub fn send_input(window: Option<usize>, inputs: Vec<Input>, wait: bool) -> Out {
    let timeout = if wait { 5 } else { 4 };
    reply_to_out(ask(
        move |tx| Cmd::Input {
            window,
            inputs,
            wait,
            tx,
        },
        timeout,
    ))
}

pub fn parse_drop(p: &Params) -> Result<(Option<usize>, Input, bool), &'static str> {
    // Unlike permissive mouse aliases, file input must not silently
    // ignore a typo or choose between duplicate parameters.
    let mut seen = Vec::new();
    for (key, _) in &p.0 {
        let key = match key.as_str() {
            "path" | "x" | "y" | "wait" | "if_user_seq" => key.as_str(),
            "w" | "window" => "w",
            _ => return Err("unknown drop parameter"),
        };
        if seen.contains(&key) {
            return Err("duplicate drop parameter");
        }
        seen.push(key);
    }
    let path = p.get(&["path"]).ok_or("drop requires path=")?;
    if path.is_empty()
        || path.len() > 4096
        || path.chars().any(char::is_control)
        || !std::path::Path::new(path).is_absolute()
    {
        return Err(
            "drop path must be absolute, at most 4096 bytes, with no control characters",
        );
    }
    let coordinate = |key| {
        p.get(&[key])
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or("drop requires finite, nonnegative x= and y=")
    };
    let x = coordinate("x")?;
    let y = coordinate("y")?;
    let window = p
        .get(&["w", "window"])
        .map(|value| {
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("drop window must be a nonnegative integer");
            }
            value
                .parse::<usize>()
                .map_err(|_| "drop window is out of range")
        })
        .transpose()?;
    let wait = match p.get(&["wait"]) {
        None | Some("0" | "false") => false,
        Some("1" | "true") => true,
        _ => return Err("drop wait must be 0 or 1"),
    };
    Ok((
        window,
        Input::DropFile {
            path: path.to_string(),
            x,
            y,
        },
        wait,
    ))
}

pub fn route_drop(p: &Params) -> Out {
    match parse_drop(p) {
        Ok((window, input, wait)) => send_input(window, vec![input], wait),
        Err(message) => Out::Json(400, format!("{{\"err\":{}}}", json_str(message))),
    }
}

pub struct Grabbed {
    pub window_id: usize,
    pub width: u32,
    pub height: u32,
    pub png: Vec<u8>,
    pub capture_ms: f64,
    pub encode_ms: f64,
    pub backend_png: bool,
    pub _bytes: Option<GrabBytes>,
}

pub struct PendingGrabs {
    pub ids: Vec<u64>,
    pub replies: Vec<Receiver<Result<Grabbed, String>>>,
    pub cancelled: Arc<AtomicBool>,
    pub deadline: Instant,
    pub paths: Vec<PathBuf>,
}

impl Drop for PendingGrabs {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        let mut sinks = grab_sinks().lock().unwrap();
        for id in &self.ids {
            sinks.remove(id);
        }
        drop(sinks);
        let mut pins = pinned_grabs().lock().unwrap();
        for path in &self.paths {
            pins.remove(path);
        }
        drop(pins);
        queue()
            .lock()
            .unwrap()
            .push(QueuedCmd {
                cmd: Cmd::CancelGrabs(self.ids.clone()),
                user_seq: None,
                deadline: Instant::now(),
            });
        (hooks().wake)();
    }
}

pub fn arm_grabs(
    window: Option<usize>,
    scale: f64,
    n: usize,
    every: Duration,
) -> Result<PendingGrabs, String> {
    let started = Instant::now();
    // Start the long-lived worker on this HTTP thread, never from the UI
    // or a GPU callback. No temporary worker is spawned for a frame.
    encode_queue().as_ref().map_err(Clone::clone)?;
    if !scale.is_finite() || scale <= 0.0 {
        return Err("grab scale must be positive and finite".into());
    }
    let window = Some(
        window
            .or_else(|| status_cell().lock().unwrap().windows.first().map(|w| w.id))
            .ok_or("no windows")?,
    );
    // Keep fetch_update for older stable toolchains without try_update.
    #[allow(deprecated)]
    PENDING_GRABS
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
            used.checked_add(n)
                .filter(|total| *total <= MAX_PENDING_GRABS)
        })
        .map_err(|_| "grab request limit reached (64); retry after pending grabs finish")?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut pending = PendingGrabs {
        ids: Vec::with_capacity(n),
        replies: Vec::with_capacity(n),
        cancelled: cancelled.clone(),
        deadline: started + every * (n as u32 - 1) + Duration::from_secs(10),
        paths: Vec::new(),
    };
    {
        let mut sinks = grab_sinks().lock().unwrap();
        for index in 0..n {
            let id = GRAB_ID_BASE + NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = sync_channel(1);
            sinks.insert(
                id,
                GrabSink {
                    window,
                    requested_at: started + every * index as u32,
                    scale,
                    cancelled: cancelled.clone(),
                    tx,
                },
            );
            pending.ids.push(id);
            pending.replies.push(rx);
        }
    }
    match ask(
        |tx| Cmd::Grab {
            window,
            ids: pending.ids.clone(),
            started,
            every,
            cancelled,
            tx,
        },
        4,
    ) {
        Reply::Ok => Ok(pending),
        Reply::Err(msg) => Err(msg),
        Reply::Text(_) | Reply::Conflict(_) => Err("unexpected grab reply".into()),
    }
}

pub fn receive_grab(pending: &PendingGrabs, index: usize) -> Result<Grabbed, String> {
    pending.replies[index]
        .recv_timeout(pending.deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| "grab timeout (is this backend rendering?)".to_string())?
}

pub fn grab_one(window: Option<usize>, scale: f64) -> Result<Grabbed, String> {
    let pending = arm_grabs(window, scale, 1, Duration::ZERO)?;
    receive_grab(&pending, 0)
}

pub fn grab_json(grabbed: &Grabbed, path: &std::path::Path) -> String {
    format!(
        "{{\"png\":{},\"w\":{},\"sz\":[{},{}],\"capture_ms\":{:.3},\"encode_ms\":{:.3},\"capture_kind\":{}}}",
        json_str(&path.display().to_string()), grabbed.window_id, grabbed.width, grabbed.height,
        grabbed.capture_ms, grabbed.encode_ms,
        json_str(if grabbed.backend_png { "backend_png" } else { "pixels" }),
    )
}

pub fn route_grab(p: &Params) -> Out {
    let raw = p.flag(&["raw"]);
    let grabbed = match grab_one(p.window(), p.f64(&["scale"], 1.0)) {
        Ok(grabbed) => grabbed,
        Err(msg) => return err(&msg),
    };
    if raw {
        return Out::Png(grabbed.png);
    }
    match write_grab(grabbed.window_id, &grabbed.png) {
        Ok(path) => Out::Json(200, grab_json(&grabbed, &path)),
        Err(msg) => err(&msg),
    }
}

pub fn route_grab_sequence(p: &Params) -> Out {
    let n = match p.get(&["n"]).unwrap_or("8").parse::<usize>() {
        Ok(n @ 1..=64) => n,
        _ => return err("gseq n must be an integer in 1..=64"),
    };
    let every_ms = match p.get(&["every_ms"]).unwrap_or("50").parse::<u64>() {
        Ok(ms @ 8..=60_000) if ms * (n as u64 - 1) <= 60_000 => ms,
        _ => {
            return err(
                "gseq every_ms must be 8..=60000; total cadence span must not exceed 60s",
            )
        }
    };
    let mut pending = match arm_grabs(
        p.window(),
        p.f64(&["scale"], 1.0),
        n,
        Duration::from_millis(every_ms),
    ) {
        Ok(pending) => pending,
        Err(msg) => return err(&msg),
    };
    // Every capture is already scheduled. Waiting for encodes here cannot
    // retime the UI's requests, even when encoding is slower than cadence.
    let mut paths = Vec::with_capacity(n);
    let mut frames = Vec::with_capacity(n);
    for index in 0..n {
        let grabbed = match receive_grab(&pending, index) {
            Ok(grabbed) => grabbed,
            Err(msg) => return err(&format!("gseq frame {index}: {msg}")),
        };
        let path = match write_grab_file(grabbed.window_id, &grabbed.png, true) {
            Ok(path) => path,
            Err(msg) => return err(&msg),
        };
        paths.push(json_str(&path.display().to_string()));
        let mut frame = grab_json(&grabbed, &path);
        frame.pop();
        frame.push_str(&format!(
            ",\"scheduled_ms\":{},\"pixels_ms\":{:.3}}}",
            index as u64 * every_ms,
            index as f64 * every_ms as f64 + grabbed.capture_ms
        ));
        frames.push(frame);
        pending.paths.push(path);
    }
    Out::Json(
        200,
        format!(
            "{{\"png\":[{}],\"n\":{n},\"every_ms\":{every_ms},\"frames\":[{}]}}",
            paths.join(","),
            frames.join(",")
        ),
    )
}

/// The canonical last call of an agent session: final evidence for every
/// window, then a graceful shutdown, in one request.
pub fn route_grab_quit(p: &Params) -> Out {
    let scale = p.f64(&["scale"], 1.0);
    let targets: Vec<usize> = match p.window() {
        Some(want) => vec![want],
        None => status_cell()
            .lock()
            .unwrap()
            .windows
            .iter()
            .map(|w| w.id)
            .collect(),
    };
    let mut paths = Vec::new();
    let mut problems = Vec::new();
    for window_id in targets {
        match grab_one(Some(window_id), scale)
            .and_then(|grabbed| write_grab(grabbed.window_id, &grabbed.png))
        {
            Ok(path) => paths.push(path.display().to_string()),
            Err(msg) => problems.push(format!("w{window_id}: {msg}")),
        }
    }
    // Capture failure still permits cleanup, but human intervention does
    // not. Check on the UI thread after all the potentially slow grabs.
    let quit = match ask(Cmd::Quit, 4) {
        Reply::Ok => true,
        Reply::Conflict(json) => return Out::Json(409, json),
        other => return reply_to_out(other),
    };
    let mut out = String::from("{\"png\":[");
    for (index, path) in paths.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&json_str(path));
    }
    out.push_str(&format!("],\"quit\":{}", if quit { 1 } else { 0 }));
    if !problems.is_empty() {
        out.push_str(&format!(",\"err\":{}", json_str(&problems.join("; "))));
    }
    out.push('}');
    Out::Json(200, out)
}

/// Write the PNG into the per-run grab dir and prune old ones so a long
/// session can't fill the disk.
pub fn pinned_grabs() -> &'static Mutex<std::collections::HashSet<PathBuf>> {
    static PINS: OnceLock<Mutex<std::collections::HashSet<PathBuf>>> = OnceLock::new();
    PINS.get_or_init(Default::default)
}

pub fn write_grab(window_id: usize, png: &[u8]) -> Result<PathBuf, String> {
    write_grab_file(window_id, png, false)
}

pub fn write_grab_file(window_id: usize, png: &[u8], pin: bool) -> Result<PathBuf, String> {
    // Serialize writers/pruning on HTTP threads only. A concurrent /g
    // cannot delete the first paths of a sequence still being returned.
    let mut pins = pinned_grabs().lock().unwrap();
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = grab_dir().lock().unwrap().clone();
    if dir.as_os_str().is_empty() {
        return Err("no grab dir".to_string());
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("grab dir: {e}"))?;
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let prefix = format!("grab-w{window_id}-");
    let path = dir.join(format!("{prefix}{seq:05}.png"));
    std::fs::write(&path, png).map_err(|e| format!("grab write: {e}"))?;
    if pin {
        pins.insert(path.clone());
    }

    let mut mine: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    !pins.contains(path)
                        && path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.starts_with(&prefix))
                })
                .collect()
        })
        .unwrap_or_default();
    if mine.len() > GRABS_KEPT_PER_WINDOW {
        mine.sort();
        let drop_count = mine.len() - GRABS_KEPT_PER_WINDOW;
        for old in mine.into_iter().take(drop_count) {
            let _ = std::fs::remove_file(old);
        }
    }
    Ok(path)
}

pub fn encode_grab(job: &EncodeJob) -> Result<(u32, u32, Vec<u8>), String> {
    use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
    use makepad_zune_png::makepad_zune_core::colorspace::ColorSpace;
    use makepad_zune_png::PngDecoder;

    let width = job.width as usize;
    let height = job.height as usize;
    if width == 0 || height == 0 || job.pixels.is_empty() {
        return Err("grab backend returned no pixels".into());
    }
    let target_w = ((width as f64) * job.sink.scale).round().max(1.0) as usize;
    let target_h = ((height as f64) * job.sink.scale).round().max(1.0) as usize;
    let target_bytes = target_w
        .checked_mul(target_h)
        .and_then(|area| area.checked_mul(4))
        .filter(|bytes| *bytes <= MAX_GRAB_BYTES)
        .ok_or("scaled grab exceeds the 64 MiB pixel limit")?;
    if job.backend_png && target_w == width && target_h == height {
        return Ok((job.width, job.height, job.pixels.to_vec()));
    }
    let decoded;
    let (pixels, stride, order, origin) = if job.backend_png {
        let mut decoder = PngDecoder::new(ZCursor::new(&*job.pixels));
        decoded = decoder
            .decode_raw()
            .map_err(|err| format!("grab decode failed: {err:?}"))?;
        if decoder.colorspace() != Some(ColorSpace::RGBA) {
            return Err("grab decode: not rgba".into());
        }
        (
            &decoded[..],
            width * 4,
            GrabOrder::Rgba,
            GrabOrigin::TopLeft,
        )
    } else {
        (&job.pixels[..], job.stride, job.order, job.origin)
    };
    if width == 0
        || height == 0
        || stride < width * 4
        || stride
            .checked_mul(height)
            .is_none_or(|len| len > pixels.len())
    {
        return Err("grab readback has invalid dimensions or stride".into());
    }
    // Sample first, then convert only the retained pixels. In particular,
    // scale=0.5 encodes one quarter as many pixels, with no full-size PNG.
    let mut out = vec![0u8; target_bytes];
    for y in 0..target_h {
        let src_y = y * height / target_h;
        let src_y = if origin == GrabOrigin::BottomLeft {
            height - 1 - src_y
        } else {
            src_y
        };
        for x in 0..target_w {
            let src = src_y * stride + (x * width / target_w) * 4;
            let dst = (y * target_w + x) * 4;
            out[dst..dst + 4].copy_from_slice(&pixels[src..src + 4]);
            if order == GrabOrder::Bgra {
                out.swap(dst, dst + 2);
            }
        }
    }
    let bytes = encode_rgba_as_png(target_w as u32, target_h as u32, &out)?;
    Ok((target_w as u32, target_h as u32, bytes))
}

/// Queue a command and block this HTTP thread until the event loop answers.
/// A status or activity read waits for the UI thread through a stall
/// instead of answering 408 from a snapshot; the bound only keeps a
/// wedged app from pinning the request thread forever.
pub const ACTIVITY_WAIT_SECS: u64 = 600;

pub fn ask<F>(make: F, timeout_secs: u64) -> Reply
where
    F: FnOnce(Sender<Reply>) -> Cmd,
{
    let (tx, rx) = channel();
    queue().lock().unwrap().push(QueuedCmd {
        cmd: make(tx),
        user_seq: REQUEST_USER_SEQ.get(),
        deadline: Instant::now() + Duration::from_secs(timeout_secs),
    });
    (hooks().wake)();
    match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
        Ok(reply) => reply,
        Err(_) => Reply::Err("timeout (app busy or not running its event loop)".to_string()),
    }
}

pub fn reply_to_out(reply: Reply) -> Out {
    match reply {
        Reply::Ok => Out::Json(200, "{\"ok\":1}".to_string()),
        Reply::Text(text) => Out::Json(200, text),
        Reply::Err(msg) => err(&msg),
        Reply::Conflict(json) => Out::Json(409, json),
    }
}

pub struct Params(pub Vec<(String, String)>);

impl Params {
    pub fn get(&self, keys: &[&str]) -> Option<&str> {
        for key in keys {
            if let Some((_, value)) = self.0.iter().find(|(k, _)| k == key) {
                return Some(value.as_str());
            }
        }
        None
    }
    pub fn f64(&self, keys: &[&str], default: f64) -> f64 {
        self.get(keys)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(default)
    }
    pub fn flag(&self, keys: &[&str]) -> bool {
        match self.get(keys) {
            None => false,
            Some(v) => !matches!(v, "0" | "false" | "no" | "off"),
        }
    }
    pub fn window(&self) -> Option<usize> {
        self.get(&["w", "window"])
            .and_then(|v| v.parse::<usize>().ok())
    }
    pub fn mods(&self) -> RemoteKeyModifiers {
        RemoteKeyModifiers {
            shift: self.flag(&["shift"]),
            control: self.flag(&["ctrl", "control"]),
            alt: self.flag(&["alt", "option"]),
            logo: self.flag(&["cmd", "logo", "meta", "super"]),
        }
    }
}

pub fn parse_query(query: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, "1"));
        out.push((percent_decode(key), percent_decode(value)));
    }
    out
}

pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Flat `{"k":v}` JSON: strings, numbers, bools. Nested values are skipped —
/// the protocol has no use for them and this stays 40 lines instead of 400.
pub fn parse_flat_json(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let chars: Vec<char> = body.chars().collect();
    let mut index = 0;
    let read_string = |chars: &[char], index: &mut usize| -> Option<String> {
        if chars.get(*index) != Some(&'"') {
            return None;
        }
        *index += 1;
        let mut value = String::new();
        while let Some(&c) = chars.get(*index) {
            *index += 1;
            match c {
                '"' => return Some(value),
                '\\' => {
                    let escape = *chars.get(*index)?;
                    *index += 1;
                    value.push(match escape {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        'b' => '\u{8}',
                        'f' => '\u{c}',
                        'u' => {
                            let hex: String = chars.get(*index..*index + 4)?.iter().collect();
                            *index += 4;
                            char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?
                        }
                        other => other,
                    });
                }
                other => value.push(other),
            }
        }
        None
    };
    while index < chars.len() {
        if chars[index] != '"' {
            index += 1;
            continue;
        }
        let Some(key) = read_string(&chars, &mut index) else {
            break;
        };
        while matches!(
            chars.get(index),
            Some(' ') | Some('\n') | Some('\t') | Some('\r')
        ) {
            index += 1;
        }
        if chars.get(index) != Some(&':') {
            continue;
        }
        index += 1;
        while matches!(
            chars.get(index),
            Some(' ') | Some('\n') | Some('\t') | Some('\r')
        ) {
            index += 1;
        }
        match chars.get(index) {
            Some('"') => {
                if let Some(value) = read_string(&chars, &mut index) {
                    out.push((key, value));
                }
            }
            Some(_) => {
                let start = index;
                while let Some(&c) = chars.get(index) {
                    if c == ',' || c == '}' || c == ']' {
                        break;
                    }
                    index += 1;
                }
                let value: String = chars[start..index].iter().collect();
                out.push((key, value.trim().to_string()));
            }
            None => break,
        }
    }
    out
}

pub fn json_str(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 2);
    out.push('"');
    for c in input.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Compact number: integers print without a trailing `.0`.
pub fn num(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1.0e15 {
        format!("{}", value as i64)
    } else {
        let rounded = (value * 1000.0).round() / 1000.0;
        format!("{rounded}")
    }
}

pub fn parse_key_code(name: &str) -> Option<KeyCode> {
    let lower = name.to_ascii_lowercase();
    // single letter / digit shorthands: "a" -> KeyA, "1" -> Key1
    let normalized = if lower.len() == 1 {
        let c = lower.chars().next().unwrap();
        if c.is_ascii_alphanumeric() {
            format!("key{c}")
        } else {
            lower.clone()
        }
    } else {
        lower.clone()
    };
    Some(match normalized.as_str() {
        "escape" | "esc" => KeyCode::Escape,
        "back" => KeyCode::Back,
        "backtick" | "`" => KeyCode::Backtick,
        "key0" => KeyCode::Key0,
        "key1" => KeyCode::Key1,
        "key2" => KeyCode::Key2,
        "key3" => KeyCode::Key3,
        "key4" => KeyCode::Key4,
        "key5" => KeyCode::Key5,
        "key6" => KeyCode::Key6,
        "key7" => KeyCode::Key7,
        "key8" => KeyCode::Key8,
        "key9" => KeyCode::Key9,
        "minus" | "-" => KeyCode::Minus,
        "equals" | "=" => KeyCode::Equals,
        "backspace" => KeyCode::Backspace,
        "tab" => KeyCode::Tab,
        "keyq" => KeyCode::KeyQ,
        "keyw" => KeyCode::KeyW,
        "keye" => KeyCode::KeyE,
        "keyr" => KeyCode::KeyR,
        "keyt" => KeyCode::KeyT,
        "keyy" => KeyCode::KeyY,
        "keyu" => KeyCode::KeyU,
        "keyi" => KeyCode::KeyI,
        "keyo" => KeyCode::KeyO,
        "keyp" => KeyCode::KeyP,
        "lbracket" | "[" => KeyCode::LBracket,
        "rbracket" | "]" => KeyCode::RBracket,
        "return" | "returnkey" | "enter" => KeyCode::ReturnKey,
        "keya" => KeyCode::KeyA,
        "keys" => KeyCode::KeyS,
        "keyd" => KeyCode::KeyD,
        "keyf" => KeyCode::KeyF,
        "keyg" => KeyCode::KeyG,
        "keyh" => KeyCode::KeyH,
        "keyj" => KeyCode::KeyJ,
        "keyk" => KeyCode::KeyK,
        "keyl" => KeyCode::KeyL,
        "semicolon" | ";" => KeyCode::Semicolon,
        "quote" | "'" => KeyCode::Quote,
        "backslash" => KeyCode::Backslash,
        "keyz" => KeyCode::KeyZ,
        "keyx" => KeyCode::KeyX,
        "keyc" => KeyCode::KeyC,
        "keyv" => KeyCode::KeyV,
        "keyb" => KeyCode::KeyB,
        "keyn" => KeyCode::KeyN,
        "keym" => KeyCode::KeyM,
        "comma" | "," => KeyCode::Comma,
        "period" | "." => KeyCode::Period,
        "slash" | "/" => KeyCode::Slash,
        "control" | "ctrl" => KeyCode::Control,
        "alt" | "option" => KeyCode::Alt,
        "shift" => KeyCode::Shift,
        "logo" | "cmd" | "command" | "meta" => KeyCode::Logo,
        "space" => KeyCode::Space,
        "capslock" => KeyCode::Capslock,
        "f1" => KeyCode::F1,
        "f2" => KeyCode::F2,
        "f3" => KeyCode::F3,
        "f4" => KeyCode::F4,
        "f5" => KeyCode::F5,
        "f6" => KeyCode::F6,
        "f7" => KeyCode::F7,
        "f8" => KeyCode::F8,
        "f9" => KeyCode::F9,
        "f10" => KeyCode::F10,
        "f11" => KeyCode::F11,
        "f12" => KeyCode::F12,
        "printscreen" => KeyCode::PrintScreen,
        "scrolllock" => KeyCode::ScrollLock,
        "pause" => KeyCode::Pause,
        "insert" => KeyCode::Insert,
        "delete" | "del" => KeyCode::Delete,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "numpadenter" => KeyCode::NumpadEnter,
        "arrowup" | "up" => KeyCode::ArrowUp,
        "arrowdown" | "down" => KeyCode::ArrowDown,
        "arrowleft" | "left" => KeyCode::ArrowLeft,
        "arrowright" | "right" => KeyCode::ArrowRight,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_parsing_decodes_and_defaults() {
        let p = Params(parse_query("x=10&y=20.5&t=hello%20world&wait"));
        assert_eq!(p.f64(&["x"], 0.0), 10.0);
        assert_eq!(p.f64(&["y"], 0.0), 20.5);
        assert_eq!(p.get(&["t", "text"]), Some("hello world"));
        assert!(p.flag(&["wait"]));
        assert!(!p.flag(&["raw"]));
    }

    #[test]
    fn flat_json_body_feeds_the_same_params() {
        let p = Params(parse_flat_json(
            "{\"window\":2,\"kind\":\"click\",\"x\":100,\"y\":-3.5,\"text\":\"a\\\"b\"}",
        ));
        assert_eq!(p.window(), Some(2));
        assert_eq!(p.get(&["k", "kind"]), Some("click"));
        assert_eq!(p.f64(&["x"], 0.0), 100.0);
        assert_eq!(p.f64(&["y"], 0.0), -3.5);
        assert_eq!(p.get(&["t", "text"]), Some("a\"b"));
    }

    #[test]
    fn file_drop_requires_unambiguous_path_coordinates_and_options() {
        let path = std::env::temp_dir()
            .join("reference car.png")
            .to_string_lossy()
            .into_owned();
        let fields = vec![
            ("path".into(), path.clone()),
            ("x".into(), "12.5".into()),
            ("y".into(), "20".into()),
        ];
        let mut valid = fields.clone();
        valid.extend([("w".into(), "2".into()), ("wait".into(), "1".into())]);
        assert!(
            matches!(parse_drop(&Params(valid)), Ok((Some(2), Input::DropFile { path: p, x: 12.5, y: 20.0 }, true)) if p == path)
        );
        for (key, value) in [
            ("path", "relative.png"),
            ("path", ""),
            ("path", "/tmp/invalid\0.png"),
            ("x", "NaN"),
            ("x", "inf"),
            ("x", "-1"),
            ("y", ""),
            ("w", "two"),
            ("w", "-1"),
            ("wait", "sometimes"),
            ("paths", "ignored.png"),
        ] {
            let mut invalid = fields.clone();
            invalid.retain(|(name, _)| name != key);
            invalid.push((key.into(), value.into()));
            assert!(
                parse_drop(&Params(invalid)).is_err(),
                "{key} accepted invalid value"
            );
        }
        let mut duplicate = fields.clone();
        duplicate.push(("path".into(), path.clone()));
        assert!(parse_drop(&Params(duplicate)).is_err());
        let mut aliases = fields.clone();
        aliases.extend([("w".into(), "1".into()), ("window".into(), "2".into())]);
        assert!(parse_drop(&Params(aliases)).is_err());
        let mut long_path = fields;
        long_path[0].1 = format!("{path}{}", "a".repeat(4096));
        assert!(parse_drop(&Params(long_path)).is_err());
    }

    #[test]
    fn key_codes_accept_short_and_long_names() {
        assert_eq!(parse_key_code("a"), Some(KeyCode::KeyA));
        assert_eq!(parse_key_code("KeyA"), Some(KeyCode::KeyA));
        assert_eq!(parse_key_code("enter"), Some(KeyCode::ReturnKey));
        assert_eq!(parse_key_code("ArrowLeft"), Some(KeyCode::ArrowLeft));
        assert_eq!(parse_key_code("nope"), None);
    }

    #[test]
    fn numbers_stay_compact_and_strings_escape() {
        assert_eq!(num(1680.0), "1680");
        assert_eq!(num(2.0), "2");
        assert_eq!(num(1.5), "1.5");
        assert_eq!(json_str("a\"b\n"), "\"a\\\"b\\n\"");
    }

    #[test]
    fn head_terminator_is_found_across_chunks() {
        // the index is where the terminator starts, so head = buf[..14]
        // and body = buf[14 + 4..]
        assert_eq!(find_head_end(b"GET / HTTP/1.1\r\n\r\nbody"), Some(14));
        assert_eq!(
            find_head_end(b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n"),
            Some(24)
        );
        assert_eq!(find_head_end(b"GET / HTTP/1.1\r\n"), None);
    }
}
