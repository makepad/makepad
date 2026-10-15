//! The Cx-free half of the `--remote` command set (see `makepad_platform::remote`): the queue
//! the endpoint's command threads hand to the UI thread, the grab sinks and the encode worker,
//! the parameter parsing and the command bodies. None of it touches `Cx`; platform's `remote`
//! module drains the queue from the event loop, answers the commands and delivers grabs. The
//! transport is the process's makepad-remote endpoint (`cmd` and `frames` services).
#![allow(clippy::too_many_arguments)]

use crate::makepad_math::Vec2d;
pub use makepad_remote::json_str;
use makepad_studio_protocol::{KeyCode, PinchPhase, RemoteKeyModifiers, ScrollPhase};
pub use crate::remote_schema::{CmdRisk, Command};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

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
    /// The user sequence when the request on this thread started (the end line reports both).
    pub static REQUEST_START_USER_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    // Each connection owns its request context; it never crosses threads
    // implicitly. Queued commands carry the explicit expected user epoch.
    /// The `if_user_seq` a request carried, if any: a driver that names
    /// the sequence it started from is refused once the person has
    /// intervened; one that sends nothing only meets the quiet-period gate.
    pub static REQUEST_USER_SEQ: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

// ------------------------------------------------------------------
// global state (the command threads' only view of the app)
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

/// The fields the endpoint adds to the end line of every answer: the user sequence as the answer
/// leaves and as the request started, so a driver can tell that the person used the app during it.
pub fn end_fields(out: &mut String) {
    use std::fmt::Write as _;
    let _ = write!(out, ",\"user_seq\":{},\"user_seq_start\":{}", super::activity::user_seq_now(), REQUEST_START_USER_SEQ.get());
}

/// Wakes the event loop so it drains the command queue. SignalToUI coalesces wakes until
/// timer 0 clears its flag; a remote capture must also wake between those ticks (including
/// 200 ms idle).
pub fn wake() {
    crate::thread::SignalToUI::set_internal_signal();
}

pub static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Grab request ids live in the top half of the id space, like the file
/// sinks in `cx_shared.rs`, so they can never collide with studio ids.
pub const GRAB_ID_BASE: u64 = 1 << 62;

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
    /// Deliver tightly packed RGBA bytes (top row first) instead of a PNG.
    pub raw_output: bool,
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

/// The encoder worker's queue: `None` until a grab command starts it, and again after the
/// app ended it ([`stop_encoder`]).
fn encoder() -> &'static Mutex<Option<SyncSender<EncodeJob>>> {
    static ENCODER: Mutex<Option<SyncSender<EncodeJob>>> = Mutex::new(None);
    &ENCODER
}

/// Starts the encoder worker if it is not running (a grab command's thread, never the UI or a
/// GPU callback). It runs until [`stop_encoder`].
pub fn start_encoder() -> Result<(), String> {
    let mut encoder = encoder().lock().unwrap_or_else(|e| e.into_inner());
    if encoder.is_some() {
        return Ok(());
    }
    let (tx, rx) = sync_channel::<EncodeJob>(MAX_PENDING_GRABS);
    std::thread::Builder::new()
        .name("makepad-remote-png".into())
        .spawn(move || {
            while let Ok(mut job) = rx.recv() {
                if job.sink.cancelled.load(Ordering::Relaxed) {
                    continue;
                }
                let started = Instant::now();
                let result = encode_grab(&job).map(|(width, height, data)| Grabbed {
                    window_id: job.sink.window.unwrap_or(0),
                    width,
                    height,
                    data,
                    capture_ms: job.capture_ms,
                    encode_ms: started.elapsed().as_secs_f64() * 1000.0,
                    backend_png: job.backend_png,
                    _bytes: job.bytes.take(),
                });
                let _ = job.sink.tx.try_send(result);
            }
        })
        .map_err(|err| format!("grab worker: {err}"))?;
    *encoder = Some(tx);
    Ok(())
}

/// The app ends: the encoder worker finishes the jobs it has and ends. A hosted app's
/// domain waits for every thread it started (restart::stop_workers); the worker waited on its
/// queue forever, so every quit of a grabbed app under `makepad run` gave up on it as leaked
/// after the stop's 5 s, with the thread still running.
pub(crate) fn stop_encoder() {
    encoder().lock().unwrap_or_else(|e| e.into_inner()).take();
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
    let tx = encoder().lock().unwrap_or_else(|e| e.into_inner()).clone();
    match tx {
        Some(tx) => {
            if let Err(err) = tx.try_send(job) {
                let (std::sync::mpsc::TrySendError::Full(job)
                | std::sync::mpsc::TrySendError::Disconnected(job)) = err;
                let _ = job.sink.tx.try_send(Err("grab encoder queue full".into()));
            }
        }
        None => {
            let _ = job.sink.tx.try_send(Err("the grab encoder has ended (the app is quitting)".into()));
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
    /// One step of a two-finger touchpad scroll (`/m?k=scroll&phase=`):
    /// is_mouse false, with the gesture phase.
    PadScroll {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
        phase: ScrollPhase,
        mods: RemoteKeyModifiers,
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

/// What `poll` hands back to a waiting command thread.
pub enum Reply {
    Ok,
    /// Deferred until the next rendered frame; the sender is re-armed by
    /// `poll` in a later tick.
    Text(String),
    Err(String),
    Conflict(String),
}

/// What the command line and the environment ask of the remote endpoint.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    /// `MAKEPAD_REMOTE=0`: no endpoint at all.
    Off,
    /// Nothing asked: the endpoint is open (same user only, nothing on the network), so a local
    /// agent reaches every app the person started; the app behaves as an ordinary launch.
    Local,
    /// `--remote`, `MAKEPAD_REMOTE=1` or `MAKEPAD_INSPECT=1`: launched for an agent, a test or an
    /// inspector. The endpoint is open and the instance behaves as one (no focus stealing, a tagged title, its own instance).
    On,
    /// A form that no longer exists (a TCP port or address); the message says what to use.
    Refused(String),
}

fn refused(form: &str) -> Request {
    Request::Refused(format!(
        "{form}: a port or address is not supported any more; --remote (no value) or MAKEPAD_REMOTE=1 opens a local \
         Unix-socket endpoint (see the startup line, `makepad-remote app <pid> status`); other machines are reached \
         through the hub (makepad hub)"
    ))
}

/// `host:port` or a bare port.
fn looks_like_bind(value: &str) -> bool {
    let value = value.trim();
    value.parse::<u16>().is_ok()
        || value.rsplit_once(':').is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok())
}

/// The request of a command line (`--remote`; the first argument is the program) and of
/// `MAKEPAD_REMOTE` (`1`/`on`/`true`/`yes` on, `0`/`off`/`false`/`no` off, unset or empty local).
pub fn request_from(args: impl Iterator<Item = String>, env: Option<&str>) -> Request {
    let mut args = args.skip(1).peekable();
    while let Some(arg) = args.next() {
        if arg == "--remote" {
            if let Some(next) = args.peek().filter(|n| looks_like_bind(n)) {
                return refused(&format!("--remote {next}"));
            }
            return Request::On;
        }
        if let Some(value) = arg.strip_prefix("--remote=") {
            return refused(&format!("--remote={value}"));
        }
    }
    let Some(value) = env else { return Request::Local };
    match value.trim().to_ascii_lowercase().as_str() {
        "" => Request::Local,
        "0" | "off" | "false" | "no" => Request::Off,
        "1" | "on" | "true" | "yes" => Request::On,
        _ => refused(&format!("MAKEPAD_REMOTE={value}")),
    }
}

pub fn request() -> Request {
    match request_from(std::env::args(), std::env::var("MAKEPAD_REMOTE").ok().as_deref()) {
        Request::Off | Request::Local if std::env::var("MAKEPAD_INSPECT").as_deref() == Ok("1") => Request::On,
        request => request,
    }
}

/// True when this process was launched for an agent or a test (`--remote`,
/// `MAKEPAD_REMOTE=1` or `MAKEPAD_INSPECT=1`); the endpoint itself is open in every process unless `MAKEPAD_REMOTE=0`.
/// Pure argv + env, usable before the endpoint itself is up: the platform's focus policy reads
/// it while the first window is being created.
pub fn requested() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| request() == Request::On)
}

/// `--remote-title-tag=NAME` overrides the suffix; `off`/`none`/empty
/// disables it. Default is `[remote]`.
pub fn title_tag() -> Option<&'static str> {
    static TAG: OnceLock<Option<String>> = OnceLock::new();
    TAG.get_or_init(|| {
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
    }).as_deref()
}

/// Mark a `--remote` instance's windows so a human who finds one lingering
/// on screen can tell it belongs to an agent and close it guilt-free.
/// Idempotent: re-titling a window does not stack tags.
pub fn tag_window_title(mut title: String) -> String {
    if !requested() {
        return title;
    }
    let Some(tag) = title_tag() else {
        return title;
    };
    if title.ends_with(tag) {
        return title;
    }
    title.reserve(tag.len() + usize::from(!title.is_empty()));
    if !title.is_empty() {
        title.push(' ');
    }
    title.push_str(tag);
    title
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

/// One answer: JSON (one line of it, or several) or plain text. A refusal is `{"err":..}`.
pub enum Answer {
    Json(String),
    Text(String),
}

pub fn err(msg: &str) -> Answer {
    Answer::Json(format!("{{\"err\":{}}}", json_str(msg)))
}

/// Every command, with its arguments and what it answers (`help`). Sub-commands are words
/// (`cap pause`, `tweak apply`); `design <op>` takes the designer op as its second word.
/// `grab`, `gseq`, `gq` and `tweak grab` are client side: they open the `frames` service.
pub const COMMANDS: &[Command] = &[
    Command { words: "status", args: "[w=ID]", answers: "{\"app\",\"pid\",\"w\":[{\"i\":id,\"t\":title,\"sz\":[w,h],\"px\":[w,h],\"dpi\",\"pos\":[x,y]}],\"activity\"}", service: "ui", risk: CmdRisk::Read, params: "\"w\":{\"type\":\"integer\",\"description\":\"window id (status lists them)\"}", required: &[], positional: &[] },
    Command { words: "activity", args: "", answers: "native user activity: user_active, user_seq, idle_ms, quiet_ms, held, last_input, window (also in status)", service: "ui", risk: CmdRisk::Read, params: "", required: &[], positional: &[] },
    Command { words: "grab", args: "[w=ID] [scale=1] [out=FILE]", answers: "client side (frames service): the window as a PNG file; prints {\"png\":path,\"w\",\"sz\",\"capture_ms\",\"encode_ms\",\"capture_kind\"}", service: "ui", risk: CmdRisk::Read, params: "\"w\":{\"type\":\"integer\"},\"scale\":{\"type\":\"number\"},\"out\":{\"type\":\"string\"}", required: &[], positional: &[] },
    Command { words: "gseq", args: "[n=8] [every_ms=50] [scale=1] [w=ID] [out=DIR]", answers: "client side (frames service): n frames, a separate present per deadline; n=1..64, every_ms>=8, span<=60s", service: "ui", risk: CmdRisk::Read, params: "\"n\":{\"type\":\"integer\"},\"every_ms\":{\"type\":\"integer\"},\"scale\":{\"type\":\"number\"},\"w\":{\"type\":\"integer\"},\"out\":{\"type\":\"string\"}", required: &[], positional: &[] },
    Command { words: "rec", args: "[out=DIR] [seconds=5] [fps=30] [codec=png|raw] [w=ID]", answers: "client side (frames cap=1): capture by streaming -- every presented frame is read back on the device and written here as frame_<slot>.png; prints {\"dir\",\"frames\",\"missing\",\"slots\",\"sz\"}; no video encoder on the device", service: "ui", risk: CmdRisk::Read, params: "\"seconds\":{\"type\":\"number\"},\"fps\":{\"type\":\"number\"},\"w\":{\"type\":\"integer\"},\"out\":{\"type\":\"string\"}", required: &[], positional: &[] },
    Command { words: "gq", args: "[scale=1] [w=ID]", answers: "client side: FINISH HERE: grab every window, then quit. {\"png\":[paths],\"quit\":1}", service: "ui", risk: CmdRisk::Destructive, params: "\"scale\":{\"type\":\"number\"},\"w\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "mouse", args: "k=move|down|up|click|scroll|pinch x= y= [b=0] [dx= dy=] [scale= phase=begin|update|end] [scroll phase=touched|began|changed|ended|momentum|momentum_ended: a touchpad scroll] [time=] [hw=1] [shift=1 ctrl=1 alt=1 cmd=1] [w=ID] [wait=1]", answers: "inject a mouse event through the real event path", service: "ui", risk: CmdRisk::Act, params: "\"k\":{\"type\":\"string\",\"enum\":[\"move\",\"down\",\"up\",\"click\",\"scroll\",\"pinch\"]},\"x\":{\"type\":\"number\"},\"y\":{\"type\":\"number\"},\"b\":{\"type\":\"integer\"},\"dx\":{\"type\":\"number\"},\"dy\":{\"type\":\"number\"},\"scale\":{\"type\":\"number\"},\"phase\":{\"type\":\"string\",\"enum\":[\"begin\",\"update\",\"end\",\"touched\",\"began\",\"changed\",\"ended\",\"momentum\",\"momentum_ended\"]},\"time\":{\"type\":\"number\"},\"hw\":{\"type\":\"boolean\"},\"shift\":{\"type\":\"boolean\"},\"ctrl\":{\"type\":\"boolean\"},\"alt\":{\"type\":\"boolean\"},\"logo\":{\"type\":\"boolean\"},\"w\":{\"type\":\"integer\"},\"wait\":{\"type\":\"boolean\"}", required: &["k"], positional: &[] },
    Command { words: "click", args: "x= y= [b=0] [w=ID] [wait=1]", answers: "mouse k=click", service: "ui", risk: CmdRisk::Act, params: "\"x\":{\"type\":\"number\"},\"y\":{\"type\":\"number\"},\"b\":{\"type\":\"integer\"},\"w\":{\"type\":\"integer\"},\"wait\":{\"type\":\"boolean\"}", required: &["x", "y"], positional: &[] },
    Command { words: "key", args: "t=TEXT | c=KeyA [k=down|up|press] [shift=1 ...] [w=ID] [wait=1]", answers: "type text or press a key (Escape ReturnKey Tab Backspace ArrowLeft F1 Key1 ..)", service: "ui", risk: CmdRisk::Act, params: "\"t\":{\"type\":\"string\",\"description\":\"text to type\"},\"c\":{\"type\":\"string\",\"description\":\"a key code: Escape ReturnKey Tab Backspace ArrowLeft F1 KeyA Key1 ..\"},\"k\":{\"type\":\"string\",\"enum\":[\"down\",\"up\",\"press\"]},\"shift\":{\"type\":\"boolean\"},\"ctrl\":{\"type\":\"boolean\"},\"alt\":{\"type\":\"boolean\"},\"logo\":{\"type\":\"boolean\"},\"w\":{\"type\":\"integer\"},\"wait\":{\"type\":\"boolean\"}", required: &[], positional: &[] },
    Command { words: "text", args: "t=TEXT [w=ID] [wait=1]", answers: "type text", service: "ui", risk: CmdRisk::Act, params: "\"t\":{\"type\":\"string\"},\"w\":{\"type\":\"integer\"},\"wait\":{\"type\":\"boolean\"}", required: &["t"], positional: &[] },
    Command { words: "drop", args: "path=ABS x= y= [w=ID] [wait=1]", answers: "drop one absolute file path through Drag/Drop/DragEnd", service: "ui", risk: CmdRisk::Act, params: "\"path\":{\"type\":\"string\"},\"x\":{\"type\":\"number\"},\"y\":{\"type\":\"number\"},\"w\":{\"type\":\"integer\"},\"wait\":{\"type\":\"boolean\"}", required: &["path"], positional: &[] },
    Command { words: "window", args: "k=maximize | k=resize width= height= | k=dragquery x= y= [w=ID] [wait=1]", answers: "window operations (the maximise/occlusion proof, the caption hit test)", service: "ui", risk: CmdRisk::Act, params: "\"k\":{\"type\":\"string\",\"enum\":[\"maximize\",\"resize\",\"dragquery\"]},\"width\":{\"type\":\"number\"},\"height\":{\"type\":\"number\"},\"x\":{\"type\":\"number\"},\"y\":{\"type\":\"number\"},\"w\":{\"type\":\"integer\"},\"wait\":{\"type\":\"boolean\"}", required: &["k"], positional: &[] },
    Command { words: "log", args: "[n=50] [since=N]", answers: "{\"n\":lastseq,\"pool\",\"l\":[lines]}", service: "logs", risk: CmdRisk::Read, params: "\"n\":{\"type\":\"integer\"},\"since\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "midi", args: "a= b= c= [p=PORT] | k=ports n=NAME[,NAME] | k=out | k=reset", answers: "inject a MIDI message as if a device sent it; declare ports; read back what the app sent", service: "ui", risk: CmdRisk::Act, params: "\"a\":{\"type\":\"integer\"},\"b\":{\"type\":\"integer\"},\"c\":{\"type\":\"integer\"},\"p\":{\"type\":\"string\"},\"k\":{\"type\":\"string\",\"enum\":[\"ports\",\"out\",\"reset\"]},\"n\":{\"type\":\"string\"}", required: &[], positional: &[] },
    Command { words: "trace", args: "[topics=a,b] [off=1]", answers: "get / set the trace topics", service: "logs", risk: CmdRisk::Act, params: "\"topics\":{\"type\":\"string\"},\"off\":{\"type\":\"boolean\"}", required: &[], positional: &[] },
    Command { words: "snap", args: "[q=TEXT|path:a.b] [w=ID] [all=1]", answers: "widget rects ready to click: {\"s\":[{\"i\",\"ty\",\"r\":[x,y,w,h],\"w\",\"t\",\"p\"}]}", service: "ui", risk: CmdRisk::Read, params: "\"q\":{\"type\":\"string\",\"description\":\"text to find, or path:a.b\"},\"w\":{\"type\":\"integer\"},\"all\":{\"type\":\"boolean\"}", required: &[], positional: &[] },
    Command { words: "dump", args: "[paths=1]", answers: "the widget tree as indented text (id, type, x y w h); paths=1 one line per widget", service: "ui", risk: CmdRisk::Read, params: "\"paths\":{\"type\":\"boolean\"}", required: &[], positional: &[] },
    Command { words: "step", args: "[frames=1] [fps=60] [wait_loads=1] [w=ID]", answers: "--virtual-clock only: run frames of 1/fps; answers {\"frame\",\"time\"} after the last", service: "ui", risk: CmdRisk::Act, params: "\"frames\":{\"type\":\"integer\"},\"fps\":{\"type\":\"number\"},\"wait_loads\":{\"type\":\"boolean\"},\"w\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "settled", args: "", answers: "{\"settled\":bool,\"reasons\":[next_frame|redraw|repaint|timer|step|shaders|loads],\"loads\":{kind:n}}", service: "ui", risk: CmdRisk::Read, params: "", required: &[], positional: &[] },
    Command { words: "cap start", args: "path=ABS.mp4 [fps=60] [audio=1] [overwrite=1] [w=ID]", answers: "record the window in process (native pixels, H.264)", service: "ui", risk: CmdRisk::Act, params: "\"path\":{\"type\":\"string\"},\"fps\":{\"type\":\"number\"},\"audio\":{\"type\":\"boolean\"},\"overwrite\":{\"type\":\"boolean\"},\"w\":{\"type\":\"integer\"}", required: &["path"], positional: &[] },
    Command { words: "cap stop", args: "[hashes=1]", answers: "finalize the recording: {\"frames\",\"sz\",\"missing\",\"hash\"}", service: "ui", risk: CmdRisk::Act, params: "\"hashes\":{\"type\":\"boolean\"}", required: &[], positional: &[] },
    Command { words: "cap pause", args: "", answers: "stop writing without stopping the clock", service: "ui", risk: CmdRisk::Act, params: "", required: &[], positional: &[] },
    Command { words: "cap resume", args: "", answers: "restart writing", service: "ui", risk: CmdRisk::Act, params: "", required: &[], positional: &[] },
    Command { words: "cursor", args: "[show=1|0] [style=arrow|hand|text|crosshair|app] [x= y=] [w=ID]", answers: "a pointer drawn into the frame, moved by injected mouse input", service: "ui", risk: CmdRisk::Act, params: "\"show\":{\"type\":\"boolean\"},\"style\":{\"type\":\"string\",\"enum\":[\"arrow\",\"hand\",\"text\",\"crosshair\",\"app\"]},\"x\":{\"type\":\"number\"},\"y\":{\"type\":\"number\"},\"w\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "close", args: "[w=ID]", answers: "close one window the normal way", service: "ui", risk: CmdRisk::Destructive, params: "\"w\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "resize", args: "w= h= [window=ID]", answers: "give the window a new inner size in layout points; answers after the frame that shows it", service: "ui", risk: CmdRisk::Act, params: "\"w\":{\"type\":\"number\"},\"h\":{\"type\":\"number\"},\"window\":{\"type\":\"integer\"}", required: &["w", "h"], positional: &[] },
    Command { words: "handsoff", args: "[on=1|0]", answers: "the red hands-off frame round the window", service: "ui", risk: CmdRisk::Act, params: "\"on\":{\"type\":\"boolean\"}", required: &[], positional: &[] },
    Command { words: "ai", args: "[on=1|0] | say=TEXT", answers: "the AI chat overlay: toggle or type a line", service: "ui", risk: CmdRisk::Act, params: "\"on\":{\"type\":\"boolean\"},\"say\":{\"type\":\"string\"}", required: &[], positional: &[] },
    Command { words: "ai transcript", args: "", answers: "the conversation", service: "ui", risk: CmdRisk::Read, params: "", required: &[], positional: &[] },
    Command { words: "tweak", args: "[on=1|0]", answers: "the TWEAKER design-feedback overlay", service: "edit", risk: CmdRisk::Act, params: "\"on\":{\"type\":\"boolean\"}", required: &[], positional: &[] },
    Command { words: "tweak state", args: "", answers: "selection, editable properties, diff log, annotations, asks waiting on the source", service: "edit", risk: CmdRisk::Read, params: "", required: &[], positional: &[] },
    Command { words: "tweak apply", args: "path=a.b.c splash={padding: 20} | prop= value=", answers: "live-apply and relayout", service: "edit", risk: CmdRisk::Act, params: "\"path\":{\"type\":\"string\"},\"splash\":{\"type\":\"string\"},\"prop\":{\"type\":\"string\"},\"value\":{\"type\":\"string\"}", required: &[], positional: &[] },
    Command { words: "tweak diff", args: "", answers: "the raw edit log", service: "edit", risk: CmdRisk::Read, params: "", required: &[], positional: &[] },
    Command { words: "tweak clear", args: "", answers: "reset the edit log", service: "edit", risk: CmdRisk::Act, params: "", required: &[], positional: &[] },
    Command { words: "tweak final", args: "", answers: "coalesced end state per widget; with \"drew\":1 the client adds \"png\" (the composited window)", service: "edit", risk: CmdRisk::Read, params: "", required: &[], positional: &[] },
    Command { words: "tweak op", args: "op=NAME ...", answers: "a tweaker op that has no named command", service: "edit", risk: CmdRisk::Act, params: "\"op\":{\"type\":\"string\"}", required: &["op"], positional: &[] },
    Command { words: "tweak grab", args: "[w=ID] [scale=1] [out=FILE]", answers: "client side: grab with the overlay composited", service: "edit", risk: CmdRisk::Read, params: "\"w\":{\"type\":\"integer\"},\"scale\":{\"type\":\"number\"},\"out\":{\"type\":\"string\"}", required: &[], positional: &[] },
    Command { words: "design", args: "<op> ...", answers: "structural edits to the Splash source under design (open state palette insert move delete duplicate wrap up down out rename set bake undo redo reset patch commit verify close)", service: "edit", risk: CmdRisk::Act, params: "\"op\":{\"type\":\"string\",\"enum\":[\"open\",\"state\",\"palette\",\"insert\",\"move\",\"delete\",\"duplicate\",\"wrap\",\"up\",\"down\",\"out\",\"rename\",\"set\",\"bake\",\"undo\",\"redo\",\"reset\",\"patch\",\"commit\",\"verify\",\"close\"]},\"path\":{\"type\":\"string\"}", required: &["op"], positional: &["op"] },
    Command { words: "shader consts", args: "[shader=ID]", answers: "the compiled shaders' hot-patchable constants", service: "edit", risk: CmdRisk::Read, params: "\"shader\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "shader const", args: "shader=ID i=N v=VALUE | reset=1", answers: "patch one constant on the GPU (no recompile)", service: "edit", risk: CmdRisk::Act, params: "\"shader\":{\"type\":\"integer\"},\"i\":{\"type\":\"integer\"},\"v\":{\"type\":\"number\"},\"reset\":{\"type\":\"boolean\"}", required: &[], positional: &[] },
    Command { words: "quit", args: "", answers: "shut the app down gracefully (no final grab)", service: "ui", risk: CmdRisk::Destructive, params: "", required: &[], positional: &[] },
];

/// The commands the client side answers by opening the `frames` service.
pub const CLIENT_SIDE: &[&str] = &["grab", "gseq", "rec", "gq", "tweak grab"];

/// A command word (or two) resolved: its key in [`COMMANDS`], the designer op for `design`, and
/// how many words it took.
#[derive(Debug, PartialEq, Eq)]
pub struct Resolved {
    pub key: &'static str,
    pub sub: Option<String>,
    pub used: usize,
}

pub fn resolve(words: &[String]) -> Result<Resolved, String> {
    let first = words.first().map(String::as_str).unwrap_or("");
    if first == "design" {
        return match words.get(1) {
            Some(op) => Ok(Resolved { key: "design", sub: Some(op.clone()), used: 2 }),
            None => Err("design <op>: open state palette insert move delete duplicate wrap up down out rename set bake undo redo reset patch commit verify close".into()),
        };
    }
    if let Some(second) = words.get(1) {
        let both = format!("{first} {second}");
        if CLIENT_SIDE.contains(&both.as_str()) {
            return Err(format!("`{both}` is a client-side command (it reads the frames service): use `makepad-remote app <pid|app> {both}` or `makepad inspect <pid> {both}`"));
        }
        if let Some(c) = COMMANDS.iter().find(|c| c.words == both) {
            return Ok(Resolved { key: c.words, sub: None, used: 2 });
        }
    }
    if CLIENT_SIDE.contains(&first) {
        return Err(format!("`{first}` is a client-side command (it reads the frames service): use `makepad-remote app <pid|app> {first}` or `makepad inspect <pid> {first}`"));
    }
    match COMMANDS.iter().find(|c| c.words == first) {
        Some(c) => Ok(Resolved { key: c.words, sub: None, used: 1 }),
        None => Err(format!("unknown command `{first}` (try `help`)")),
    }
}

/// `/step?frames=k[&fps=60][&w=]`: answered after the k-th frame.
pub fn route_step(p: &Params) -> Answer {
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
    reply_to_answer(ask(
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

pub fn route_trace(p: &Params) -> Answer {
    if p.flag(&["off"]) {
        crate::makepad_error_log::set_trace_topics("");
    } else if let Some(topics) = p.get(&["topics"]) {
        crate::makepad_error_log::set_trace_topics(topics);
    }
    #[cfg(not(target_arch = "wasm32"))]
    makepad_hal::profile::set(crate::makepad_error_log::trace_enabled("frames"));
    Answer::Json(format!(
            "{{\"topics\":{}}}",
            json_str(&crate::makepad_error_log::trace_topics())
        ),
    )
}

pub fn route_ai(op: &str, p: &Params, wait: bool) -> Answer {
    let op = op.to_string();
    let args = p.0.clone();
    let timeout = if wait { 6 } else { 4 };
    reply_to_answer(ask(move |tx| Cmd::Ai { op, args, wait, tx }, timeout))
}

/// `/design/<op>`: reads answer at once; edits answer after the frame
/// that shows them, so a grab right after sees the change.
pub fn route_design(op: &str, p: &Params) -> Answer {
    if op.is_empty() || !op.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
        return Answer::Json("{\"err\":\"unknown design op\"}".to_string());
    }
    let read = matches!(op, "state" | "palette" | "patch" | "commit" | "verify");
    route_tweak(&format!("design_{op}"), p, !read)
}

pub fn route_tweak(op: &str, p: &Params, wait: bool) -> Answer {
    let op = op.to_string();
    let args = p.0.clone();
    // A `wait` op only resolves on the next drawn frame; give it the
    // same slack as an input wait.
    let timeout = if wait { 6 } else { 4 };
    reply_to_answer(ask(move |tx| Cmd::Tweak { op, args, wait, tx }, timeout))
}

pub fn route_status(p: &Params) -> Answer {
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
        other => return reply_to_answer(other),
    }
    Answer::Json(out)
}

pub fn route_mouse(p: &Params, force_kind: Option<&str>) -> Answer {
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
        "scroll" | "wheel" => match p.get(&["phase"]) {
            None => vec![mouse(MouseKind::Scroll)],
            Some(name) => {
                let Some(phase) = pad_scroll_phase(name) else {
                    return err(&format!("bad scroll phase {name}"));
                };
                vec![Input::PadScroll { x, y, dx, dy, phase, mods, time }]
            }
        },
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

/// A touchpad scroll phase by name (`/m?k=scroll&phase=`).
fn pad_scroll_phase(name: &str) -> Option<ScrollPhase> {
    Some(match name {
        "touched" => ScrollPhase::Touched,
        "began" | "begin" => ScrollPhase::Began,
        "changed" | "update" => ScrollPhase::Changed,
        "ended" | "end" => ScrollPhase::Ended,
        "momentum" => ScrollPhase::Momentum,
        "momentum_ended" => ScrollPhase::MomentumEnded,
        _ => return None,
    })
}

pub fn route_key(p: &Params) -> Answer {
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
    let Some(code) = KeyCode::from_name(name) else {
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

pub fn route_text(p: &Params) -> Answer {
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

pub fn send_input(window: Option<usize>, inputs: Vec<Input>, wait: bool) -> Answer {
    let timeout = if wait { 5 } else { 4 };
    reply_to_answer(ask(
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

pub fn route_drop(p: &Params) -> Answer {
    match parse_drop(p) {
        Ok((window, input, wait)) => send_input(window, vec![input], wait),
        Err(message) => Answer::Json(format!("{{\"err\":{}}}", json_str(message))),
    }
}

pub struct Grabbed {
    pub window_id: usize,
    pub width: u32,
    pub height: u32,
    /// A PNG, or tightly packed RGBA when the sink asked for `raw_output`.
    pub data: Vec<u8>,
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
}

impl Drop for PendingGrabs {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        let mut sinks = grab_sinks().lock().unwrap();
        for id in &self.ids {
            sinks.remove(id);
        }
        drop(sinks);
        queue()
            .lock()
            .unwrap()
            .push(QueuedCmd {
                cmd: Cmd::CancelGrabs(self.ids.clone()),
                user_seq: None,
                deadline: Instant::now(),
            });
        wake();
    }
}

pub fn arm_grabs(
    window: Option<usize>,
    scale: f64,
    n: usize,
    every: Duration,
    raw_output: bool,
) -> Result<PendingGrabs, String> {
    let started = Instant::now();
    // Start the long-lived worker on this command thread, never from the UI
    // or a GPU callback. No temporary worker is spawned for a frame.
    start_encoder()?;
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
                    raw_output,
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
    if job.backend_png && !job.sink.raw_output && target_w == width && target_h == height {
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
    if job.sink.raw_output {
        return Ok((target_w as u32, target_h as u32, out));
    }
    let bytes = encode_rgba_as_png(target_w as u32, target_h as u32, &out)?;
    Ok((target_w as u32, target_h as u32, bytes))
}

// ------------------------------------------------------------------
// the `frames` service
// ------------------------------------------------------------------

/// What an OPEN of the `frames` service asked for (`window= scale= codec=png|raw [n=] [fps=]
/// [once=1]`).
#[derive(Debug, PartialEq)]
pub struct FramesArgs {
    pub window: Option<usize>,
    pub scale: f64,
    pub raw: bool,
    /// a fixed number of frames; none = until the client closes the channel
    pub n: Option<usize>,
    /// the cadence of a series, as the gap between presents
    pub every: Duration,
    /// `cap=1`: every presented frame of the window, streamed (capture without any encoder on the device)
    pub capture: bool,
    /// `codec=rle`: run-length coded RGBA pixels (cheap on the device, small for flat UI)
    pub rle: bool,
}

pub fn parse_frames_args(args: &str) -> Result<FramesArgs, String> {
    let (mut window, mut scale, mut raw, mut n, mut fps, mut capture, mut rle) = (None, 1.0f64, false, None, None, false, false);
    for word in args.split_whitespace() {
        let (key, value) = word.split_once('=').ok_or_else(|| format!("frames: bad argument `{word}`"))?;
        let bad = |what: &str| format!("frames: {key}= must be {what}");
        match key {
            "window" | "w" => window = Some(value.parse::<usize>().map_err(|_| bad("a window id"))?),
            "scale" => scale = value.parse::<f64>().map_err(|_| bad("a number"))?,
            "codec" => {
                raw = match value {
                    "png" => false,
                    "raw" => true,
                    "rle" => { rle = true; false }
                    _ => return Err(bad("png, raw or rle")),
                }
            }
            "n" => n = Some(value.parse::<usize>().map_err(|_| bad("an integer"))?),
            "once" => {
                if value != "0" {
                    n = Some(1);
                }
            }
            "fps" => fps = Some(value.parse::<f64>().map_err(|_| bad("a number"))?),
            "cap" => capture = value != "0",
            other => return Err(format!("frames: unknown argument `{other}`")),
        }
    }
    if capture {
        let fps = fps.unwrap_or(30.0);
        if !(fps.is_finite() && fps > 0.0 && fps <= 240.0) { return Err("frames: cap=1 fps must be 1..=240".into()); }
        return Ok(FramesArgs { window, scale, raw, n: None, every: Duration::from_secs_f64(1.0 / fps), capture: true, rle });
    }
    if let Some(n) = n {
        if !(1..=MAX_PENDING_GRABS).contains(&n) {
            return Err(format!("frames n must be an integer in 1..={MAX_PENDING_GRABS}"));
        }
    }
    // a series is paced by the gap between presents; a single frame has none
    let every_ms = match (n, fps) {
        (Some(1), _) => 0,
        (_, Some(fps)) if fps.is_finite() && fps > 0.0 => (1000.0 / fps).round() as u64,
        (_, Some(_)) => return Err("frames fps must be positive".into()),
        (Some(_), None) => 50,
        (None, None) => 100,
    };
    if n != Some(1) && (!(8..=60_000).contains(&every_ms) || every_ms * (n.unwrap_or(1) as u64 - 1) > 60_000) {
        return Err("frames: the gap between frames must be 8..=60000 ms; a series' total span must not exceed 60s".into());
    }
    Ok(FramesArgs { window, scale, raw, n, every: Duration::from_millis(every_ms), capture: false, rle: false })
}

/// The header of one `frames` record (see `makepad_remote::frames`).
pub fn frame_header(seq: u64, g: &Grabbed, raw: bool) -> String {
    format!(
        "{{\"seq\":{seq},\"w\":{},\"h\":{},\"window\":{},\"t_ns\":{},\"codec\":\"{}\",\"capture_ms\":{:.3},\"encode_ms\":{:.3},\"capture_kind\":{}}}",
        g.width,
        g.height,
        g.window_id,
        makepad_remote::conn::mono_ns(),
        if raw { "raw" } else { "png" },
        g.capture_ms,
        g.encode_ms,
        json_str(if g.backend_png { "backend_png" } else { "pixels" }),
    )
}

/// Where a `frames` producer puts its records: the endpoint's channel, or a Relax host's sink.
pub struct FramesOut {
    /// Queues a record; `wait` waits for room (a fixed series must arrive whole), otherwise a
    /// full queue drops the record (latest wins). `false`: the client is gone.
    pub send: Box<dyn Fn(Vec<u8>, bool) -> bool + Send>,
    /// Ends the stream (the first call counts).
    pub close: Box<dyn Fn(u16, &str) + Send>,
    /// The client closed the stream: stop producing.
    pub closed: Box<dyn Fn() -> bool + Send>,
}

/// Produces the frames `args` asks for into `out`, then ends the stream. Grabs go through the UI
/// thread (the same readback and encode worker the bridge always had); a fixed series is armed at
/// once with its cadence, so encoding never retimes the presents; an open-ended one (no `n`) is
/// latest-wins.
pub fn run_frames(args: FramesArgs, out: FramesOut) {
    use makepad_remote::conn::{CLOSE_NORMAL, CLOSE_REFUSED};
    let fail = |why: String| (out.close)(CLOSE_REFUSED, &why);
    if args.capture {
        return run_capture_stream(args, out);
    }
    match args.n {
        Some(n) => {
            let pending = match arm_grabs(args.window, args.scale, n, args.every, args.raw) {
                Ok(p) => p,
                Err(why) => return fail(why),
            };
            for index in 0..n {
                let grabbed = match receive_grab(&pending, index) {
                    Ok(g) => g,
                    Err(why) => return fail(format!("frame {index}: {why}")),
                };
                let record = makepad_remote::frames::encode_record(&frame_header(index as u64, &grabbed, args.raw), &grabbed.data);
                drop(grabbed);
                if !(out.send)(record, true) {
                    return;
                }
            }
            (out.close)(CLOSE_NORMAL, "");
        }
        None => {
            let mut seq = 0u64;
            let mut next = Instant::now();
            loop {
                if (out.closed)() {
                    return;
                }
                let pending = match arm_grabs(args.window, args.scale, 1, Duration::ZERO, args.raw) {
                    Ok(p) => p,
                    Err(why) => return fail(why),
                };
                let grabbed = match receive_grab(&pending, 0) {
                    Ok(g) => g,
                    Err(why) => return fail(why),
                };
                let record = makepad_remote::frames::encode_record(&frame_header(seq, &grabbed, args.raw), &grabbed.data);
                drop(grabbed);
                seq += 1;
                if !(out.send)(record, false) {
                    return;
                }
                next += args.every;
                std::thread::sleep(next.saturating_duration_since(Instant::now()));
            }
        }
    }
}

/// `frames cap=1 [fps=30] [codec=png|raw] [w=ID]`: every presented frame of the window (at most `fps` a
/// second), read back and handed to the client as records whose `seq` is the frame's slot on the wall
/// clock (`round(t * fps)`): the client writes the file (or shows it live) and counts the gaps, so
/// capture needs no video encoder on the device and `frames N, missing M` is checkable on the far
/// side. A client that is behind drops frames (their slots show as missing); the device never queues
/// without bound.
fn run_capture_stream(args: FramesArgs, out: FramesOut) {
    use makepad_remote::conn::CLOSE_NORMAL;
    use std::sync::mpsc::{sync_channel, TrySendError};
    let fps = (1.0 / args.every.as_secs_f64()).round().max(1.0) as u32;
    let (tx, rx) = sync_channel::<(u64, u32, u32, Vec<u8>, u64)>(3);
    let dropped = Arc::new(AtomicU64::new(0));
    let (d2, mut first_ns) = (dropped.clone(), None::<u64>);
    let id = crate::screen_capture::add_screen_capture(
        crate::screen_capture::ScreenCaptureOptions { window_id: args.window, max_fps: fps as f64 },
        move |frame| {
                let first = *first_ns.get_or_insert(frame.time_ns);
            let slot = ((frame.time_ns - first) as u128 * fps as u128 + 500_000_000) / 1_000_000_000;
            if let Err(TrySendError::Full(_)) = tx.try_send((slot as u64, frame.width, frame.height, frame.rgba.to_vec(), frame.time_ns)) {
                d2.fetch_add(1, Ordering::Relaxed);
            }
        },
    );
    loop {
        if (out.closed)() { break; }
        let Ok((slot, w, h, rgba, t_ns)) = rx.recv_timeout(Duration::from_millis(200)) else { continue };
        // a phone's window is 10 MB a frame: `scale=0.5` sends a quarter of it (integer box average)
        let k = if args.scale > 0.0 && args.scale < 0.999 { (1.0 / args.scale).round().max(1.0) as usize } else { 1 };
        let (w, h, rgba) = if k > 1 { box_down(w, h, &rgba, k) } else { (w, h, rgba) };
        let (codec, data) = if args.raw {
            ("raw", rgba)
        } else if args.rle {
            ("rle", rle_encode(&rgba))
        } else {
            match encode_rgba_as_png(w, h, &rgba) { Ok(p) => ("png", p), Err(e) => { (out.close)(makepad_remote::conn::CLOSE_REFUSED, &e); break; } }
        };
        let header = format!("{{\"seq\":{slot},\"w\":{w},\"h\":{h},\"window\":{},\"t_ns\":{t_ns},\"codec\":\"{codec}\",\"dropped\":{}}}", args.window.unwrap_or(0), dropped.load(Ordering::Relaxed));
        // latest wins: a client that has not taken the last record loses this frame (its slot shows as missing)
        if !(out.send)(makepad_remote::frames::encode_record(&header, &data), false) { break; }
    }
    crate::screen_capture::remove_screen_capture(id);
    (out.close)(CLOSE_NORMAL, "");
}

/// Run-length coding of RGBA pixels: `[run: u16 le][r g b a]` per run (a run is 1..=65535 equal pixels).
pub fn rle_encode(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 8 + 64);
    let mut i = 0;
    let n = rgba.len() / 4;
    while i < n {
        let px = &rgba[i * 4..i * 4 + 4];
        let mut run = 1usize;
        while i + run < n && run < 65535 && rgba[(i + run) * 4..(i + run) * 4 + 4] == *px { run += 1; }
        out.extend_from_slice(&(run as u16).to_le_bytes());
        out.extend_from_slice(px);
        i += run;
    }
    out
}

/// RGBA averaged over k x k blocks (the edges that do not fill a block are cut).
fn box_down(w: u32, h: u32, rgba: &[u8], k: usize) -> (u32, u32, Vec<u8>) {
    let (nw, nh) = (w as usize / k, h as usize / k);
    let mut out = vec![0u8; nw * nh * 4];
    let div = (k * k) as u32;
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0u32; 4];
            for dy in 0..k {
                let row = ((y * k + dy) * w as usize + x * k) * 4;
                for dx in 0..k {
                    for c in 0..4 { acc[c] += rgba[row + dx * 4 + c] as u32; }
                }
            }
            let o = (y * nw + x) * 4;
            for c in 0..4 { out[o + c] = (acc[c] / div) as u8; }
        }
    }
    (nw as u32, nh as u32, out)
}

/// The `frames` service of the endpoint: each OPEN becomes a producer thread ([`run_frames`]).
pub fn frames_service() -> makepad_remote::endpoint::Factory {
    use makepad_remote::conn::{Event, SendError, CLOSE_REFUSED};
    Arc::new(|_conn, open| {
        let args = parse_frames_args(&open.args).map_err(|why| (CLOSE_REFUSED, why))?;
        std::thread::Builder::new()
            .name("makepad-remote-frames".into())
            .spawn(move || {
                let ch = open.channel;
                let (tx, tx2) = (ch.tx(), ch.tx());
                run_frames(
                    args,
                    FramesOut {
                        send: Box::new(move |record, wait| {
                            if wait {
                                tx.send(record).is_ok()
                            } else {
                                // the client has not taken the last one: this one is dropped
                                !matches!(tx.try_send(record), Err(SendError::Closed))
                            }
                        }),
                        close: Box::new(move |code, why| tx2.close(code, why)),
                        closed: Box::new(move || matches!(ch.try_recv(), Ok(Event::Closed(..)) | Err(false))),
                    },
                );
            })
            .map(|_| ())
            .map_err(|e| (CLOSE_REFUSED, format!("cannot start the frames thread: {e}")))
    })
}

/// Queue a command and block this command thread until the event loop answers.
/// A status or activity read waits for the UI thread through a stall
/// instead of answering 408 from a snapshot; the bound only keeps a
/// wedged app from pinning the command thread forever.
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
    wake();
    match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
        Ok(reply) => reply,
        Err(_) => Reply::Err("timeout (app busy or not running its event loop)".to_string()),
    }
}

pub fn reply_to_answer(reply: Reply) -> Answer {
    match reply {
        Reply::Ok => Answer::Json("{\"ok\":1}".to_string()),
        Reply::Text(text) => Answer::Json(text),
        Reply::Err(msg) => err(&msg),
        Reply::Conflict(json) => Answer::Json(json),
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

/// Compact number: integers print without a trailing `.0`.
pub fn num(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1.0e15 {
        format!("{}", value as i64)
    } else {
        let rounded = (value * 1000.0).round() / 1000.0;
        format!("{rounded}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_phase_names() {
        assert_eq!(pad_scroll_phase("changed"), Some(ScrollPhase::Changed));
        assert_eq!(pad_scroll_phase("momentum"), Some(ScrollPhase::Momentum));
        assert_eq!(pad_scroll_phase("momentum_ended"), Some(ScrollPhase::MomentumEnded));
        assert_eq!(pad_scroll_phase("wobble"), None);
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
        assert_eq!(KeyCode::from_name("a"), Some(KeyCode::KeyA));
        assert_eq!(KeyCode::from_name("KeyA"), Some(KeyCode::KeyA));
        assert_eq!(KeyCode::from_name("enter"), Some(KeyCode::ReturnKey));
        assert_eq!(KeyCode::from_name("ArrowLeft"), Some(KeyCode::ArrowLeft));
        assert_eq!(KeyCode::from_name("nope"), None);
    }

    #[test]
    fn numbers_stay_compact_and_strings_escape() {
        assert_eq!(num(1680.0), "1680");
        assert_eq!(num(2.0), "2");
        assert_eq!(num(1.5), "1.5");
        assert_eq!(json_str("a\"b\n"), "\"a\\\"b\\n\"");
    }

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_endpoint_is_opened_by_a_bare_flag_or_the_env_one() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>().into_iter();
        assert_eq!(request_from(args(&["app"]), None), Request::Local);
        assert_eq!(request_from(args(&["app", "--remote"]), None), Request::On);
        assert_eq!(request_from(args(&["app", "--remote", "a.png"]), None), Request::On);
        assert_eq!(request_from(args(&["app", "--remote-title-tag=x"]), None), Request::Local);
        assert_eq!(request_from(args(&["app"]), Some("1")), Request::On);
        assert_eq!(request_from(args(&["app"]), Some("on")), Request::On);
        assert_eq!(request_from(args(&["app"]), Some("0")), Request::Off);
        assert_eq!(request_from(args(&["app"]), Some("")), Request::Local);
        assert_eq!(request_from(args(&["app"]), Some("off")), Request::Off);
        // the HTTP bridge's forms are gone, and say so
        for refused in [
            request_from(args(&["app", "--remote=5000"]), None),
            request_from(args(&["app", "--remote", "5000"]), None),
            request_from(args(&["app", "--remote", "0.0.0.0:8399"]), None),
            request_from(args(&["app"]), Some("8399")),
            request_from(args(&["app"]), Some("127.0.0.1:8399")),
        ] {
            match refused {
                Request::Refused(msg) => assert!(msg.contains("no value") && msg.contains("hub"), "{msg}"),
                other => panic!("expected a refusal, got {other:?}"),
            }
        }
    }

    /// Every route of the HTTP bridge is a command of the endpoint, by its route name.
    #[test]
    fn every_former_route_resolves_to_a_command() {
        let routes = [
            ("status", 1),
            ("activity", 1),
            ("mouse", 1),
            ("click", 1),
            ("key", 1),
            ("text", 1),
            ("drop", 1),
            ("trace", 1),
            ("dump", 1),
            ("snap", 1),
            ("step", 1),
            ("settled", 1),
            ("cap pause", 2),
            ("cap resume", 2),
            ("cap start", 2),
            ("cap stop", 2),
            ("close", 1),
            ("resize", 1),
            ("window", 1),
            ("ai", 1),
            ("ai transcript", 2),
            ("handsoff", 1),
            ("tweak", 1),
            ("tweak state", 2),
            ("tweak apply", 2),
            ("tweak diff", 2),
            ("tweak clear", 2),
            ("tweak final", 2),
            ("tweak op", 2),
            ("shader consts", 2),
            ("shader const", 2),
            ("quit", 1),
            ("log", 1),
            ("midi", 1),
            ("cursor", 1),
        ];
        for (line, used) in routes {
            let r = resolve(&words(line)).unwrap_or_else(|e| panic!("{line}: {e}"));
            assert_eq!((r.key, r.used), (line, used), "{line}");
        }
        let design = resolve(&words("design insert path=a")).unwrap();
        assert_eq!((design.key, design.sub.as_deref(), design.used), ("design", Some("insert"), 2));
        assert!(resolve(&words("design")).is_err());
        // a sub-command word that is not one is a flag of the one-word command
        assert_eq!(resolve(&words("snap all")).unwrap().used, 1);
        assert!(resolve(&words("nope")).unwrap_err().contains("unknown command"));
        // the table has no command the test above does not know of
        // design, and the four client-side ones
        assert_eq!(COMMANDS.len(), routes.len() + 1 + CLIENT_SIDE.len(), "COMMANDS and the route list disagree");
    }

    #[test]
    fn grabs_are_client_side_and_say_where() {
        for line in ["grab", "gseq n=2", "gq", "tweak grab"] {
            let msg = resolve(&words(line)).unwrap_err();
            assert!(msg.contains("client-side") && msg.contains("frames"), "{line}: {msg}");
        }
        for name in CLIENT_SIDE {
            assert!(COMMANDS.iter().any(|c| c.words == *name), "{name} is documented in help");
        }
    }
}
