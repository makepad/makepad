//! The Cx-free half of the `--remote` command set (see `makepad_platform::remote`): the queue
//! the endpoint's command threads hand to the UI thread, the grab sinks and the encode worker,
//! the parameter parsing and the command bodies. None of it touches `Cx`; platform's `remote`
//! module drains the queue from the event loop, answers the commands and delivers grabs. The
//! transport is the process's makepad-remote endpoint (`cmd` and `frames` services).
#![allow(clippy::too_many_arguments)]

use crate::makepad_math::Vec2d;
use crate::ui_progress::{
    CmdStage, CommandLive, JitWaitSample, ProgressWatch, ThreadExecSample, ThreadExecState,
    UiProgressState, UiState, UiThreadProbe, WaitKind, WatchAction, WatchConfig, WatchSample,
    GLOBAL_UI_PROGRESS,
};
use makepad_video::{StreamVideoCodec, VideoStreamEncoder, VideoStreamEncoderOptions};
pub use makepad_remote::json_str;
use makepad_studio_protocol::{KeyCode, PinchPhase, RemoteKeyModifiers, ScrollPhase};
use makepad_zune_png::{
    makepad_zune_core::{
        bit_depth::BitDepth, bytestream::ZByteIoError, colorspace::ColorSpace,
        options::EncoderOptions,
    },
    PngStreamEncoder,
};
pub use crate::remote_schema::{CmdRisk, Command};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
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

fn png_encode_error(err: ZByteIoError) -> String {
    format!("png encode failed: {err:?}")
}

fn rgba_png_encoder(width: usize, height: usize) -> Result<PngStreamEncoder<Vec<u8>>, String> {
    let options = EncoderOptions::default()
        .set_width(width)
        .set_height(height)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::RGBA);
    PngStreamEncoder::new(Vec::new(), options).map_err(png_encode_error)
}

pub fn encode_rgba_as_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let (width, height) = (width as usize, height as usize);
    if width.checked_mul(4).and_then(|row| row.checked_mul(height)) != Some(rgba.len()) {
        return Err("png encode failed: RGBA length does not match dimensions".into());
    }
    let mut encoder = rgba_png_encoder(width, height)?;
    encoder.write_rows(rgba).map_err(png_encode_error)?;
    let (_, out) = encoder.finish().map_err(png_encode_error)?;
    Ok(out)
}

thread_local! {
    /// The user sequence when the request on this thread started (the end line reports both).
    pub static REQUEST_START_USER_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub static REQUEST_GONE: std::cell::RefCell<Option<makepad_remote::cmd::Gone>> =
        const { std::cell::RefCell::new(None) };
    // Each connection owns its request context; it never crosses threads
    // implicitly. Queued commands carry the explicit expected user epoch.
    /// The `if_user_seq` a request carried, if any: a driver that names
    /// the sequence it started from is refused once the person has
    /// intervened; one that sends nothing only meets the quiet-period gate.
    pub static REQUEST_USER_SEQ: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

#[must_use]
pub struct RequestGoneScope(Option<makepad_remote::cmd::Gone>);

impl RequestGoneScope {
    pub fn enter(gone: makepad_remote::cmd::Gone) -> Self {
        Self(REQUEST_GONE.replace(Some(gone)))
    }
}

impl Drop for RequestGoneScope {
    fn drop(&mut self) {
        REQUEST_GONE.set(self.0.take());
    }
}

pub fn request_gone() -> bool {
    REQUEST_GONE.with_borrow(|gone| gone.as_ref().is_some_and(|f| f()))
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
/// The app's own endpoint only: under the JIT its host owns the endpoint and its end fields.
#[cfg(not(relax))]
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

/// Grabs in flight across all clients (armed, in GPU readback or waiting for the PNG encoder).
/// A request past it never fails: it waits for a slot within its deadline ([`acquire_grab_slot`]).
pub const GRAB_IN_FLIGHT_LIMIT: usize = 64;
/// The grabs one fixed `frames n=N` series arms ahead of delivery: `n <= 8` (the default `gseq`)
/// is one `Cmd::Grab`, and 8 series share the global window without starving each other.
pub const GRAB_SERIES_WINDOW: usize = 8;
/// A series refills its window once this few of its grabs are still in flight.
const GRAB_SERIES_REFILL_AT: usize = GRAB_SERIES_WINDOW / 2;
/// A series arms ahead only the grabs due within this much time, so a slow cadence holds no
/// slots (and keeps no window ticking) long before its frames are due.
const GRAB_LOOKAHEAD: Duration = Duration::from_secs(2);
/// A series with nothing in flight sleeps until its next frame is this close.
const GRAB_ARM_LEAD: Duration = Duration::from_secs(1);
/// Extend a full-window wait while frames, encoder bytes or the JIT make progress.
pub const GRAB_SLOT_WAIT: Duration = Duration::from_secs(30);

/// Above this queued pixel budget, [`submit_encode`] pre-halves while the requested scale
/// allows and logs once when frames queue over budget at their requested size.
pub const GRAB_QUEUE_BUDGET_BYTES: usize = 64 * 1024 * 1024;

/// The grabs in flight, each owned by a [`GrabPermit`]: the one count the window keeps, read
/// lock-free by the event loop (`needs_ticks`).
pub static PENDING_GRABS: AtomicUsize = AtomicUsize::new(0);

/// Owns one reserved slot in `PENDING_GRABS`; dropping it releases the slot and wakes waiters.
pub struct GrabPermit(());

impl Drop for GrabPermit {
    fn drop(&mut self) {
        release_grab_slots(1);
    }
}

/// Where requests wait for a slot when the window is full. `lock` is a leaf lock: held only
/// around a slot check and the condvar, never across I/O, channels or callers' closures. The
/// UI thread never waits here; it only releases (and skips the lock when nobody waits).
struct GrabSlotGate {
    waiters: AtomicUsize,
    lock: Mutex<()>,
    ready: Condvar,
    saturated_logged: AtomicBool,
}

static GRAB_SLOTS: GrabSlotGate = GrabSlotGate {
    waiters: AtomicUsize::new(0),
    lock: Mutex::new(()),
    ready: Condvar::new(),
    saturated_logged: AtomicBool::new(false),
};

#[derive(Debug, PartialEq, Eq)]
enum AcquireError {
    TimedOut,
    Cancelled,
}

/// A slot now, if the window has one (no lock, no wait).
fn try_acquire_grab_slot() -> Option<GrabPermit> {
    PENDING_GRABS
        .try_update(Ordering::SeqCst, Ordering::SeqCst, |used| (used < GRAB_IN_FLIGHT_LIMIT).then_some(used + 1))
        .ok()
        .map(|_| GrabPermit(()))
}

/// A slot, waiting while the window is full until `deadline` or until `cancelled()` (a command
/// or `frames` thread, never the UI thread). Under the JIT the wait goes on while the process
/// still compiles (a compile may be what holds the other grabs).
fn acquire_grab_slot(deadline: Instant, cancelled: impl FnMut() -> bool) -> Result<GrabPermit, AcquireError> {
    let progress = || (GLOBAL_UI_PROGRESS.snapshot().repaints, GRAB_BYTES.load(Ordering::Relaxed));
    let mut previous = progress();
    #[cfg(relax)]
    let mut probe = RealProgressProbe::new();
    wait_for_grab_slot(deadline, cancelled, || {
        let now = progress();
        if std::mem::replace(&mut previous, now) != now { return true; }
        #[cfg(relax)]
        { probe.jit_busy() }
        #[cfg(not(relax))]
        { false }
    })
}

/// A slot, waiting while the window is full until `deadline` or until `cancelled()`. When the
/// deadline passes, `extend()` is asked: yes waits [`GRAB_SLOT_WAIT`] more. Logs once per
/// saturation episode.
///
/// No wakeup is lost: `PENDING_GRABS` and `waiters` are in one `SeqCst` order, so either this
/// waiter's check under `lock` sees a release, or the releaser sees `waiters > 0` and notifies
/// under `lock` after this waiter is in `wait_timeout`.
fn wait_for_grab_slot(
    mut deadline: Instant,
    mut cancelled: impl FnMut() -> bool,
    mut extend: impl FnMut() -> bool,
) -> Result<GrabPermit, AcquireError> {
    if let Some(permit) = try_acquire_grab_slot() {
        return Ok(permit);
    }
    let gate = &GRAB_SLOTS;
    if !gate.saturated_logged.swap(true, Ordering::Relaxed) {
        crate::log!(
            "[makepad-remote] grab window full ({GRAB_IN_FLIGHT_LIMIT} in flight); waiting for pending grabs to finish"
        );
    }
    struct Waiting<'a>(&'a AtomicUsize);
    impl Drop for Waiting<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    gate.waiters.fetch_add(1, Ordering::SeqCst);
    let _waiting = Waiting(&gate.waiters);
    loop {
        if cancelled() {
            return Err(AcquireError::Cancelled);
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()).filter(|left| !left.is_zero()) else {
            if extend() {
                deadline = Instant::now() + GRAB_SLOT_WAIT;
                continue;
            }
            return Err(AcquireError::TimedOut);
        };
        let guard = gate.lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(permit) = try_acquire_grab_slot() {
            return Ok(permit);
        }
        drop(gate.ready.wait_timeout(guard, remaining.min(Duration::from_millis(200))).unwrap_or_else(|e| e.into_inner()));
    }
}

/// Gives `count` slots back and wakes the requests waiting for one. Any thread, the UI thread
/// included: with nobody waiting it takes no lock.
pub(crate) fn release_grab_slots(count: usize) {
    let gate = &GRAB_SLOTS;
    let prev = PENDING_GRABS.fetch_sub(count, Ordering::SeqCst);
    // hysteresis: a window that hovers at the limit logs once, not once per frame
    if prev.saturating_sub(count) <= GRAB_IN_FLIGHT_LIMIT / 2 {
        gate.saturated_logged.store(false, Ordering::Relaxed);
    }
    if gate.waiters.load(Ordering::SeqCst) > 0 {
        let _guard = gate.lock.lock().unwrap_or_else(|e| e.into_inner());
        gate.ready.notify_all();
    }
}

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
    pub visible: bool,
    pub id: usize,
    pub title: String,
    pub w: f64,
    pub h: f64,
    pub dpi: f64,
    /// the scale the window's main pass last rasterised at (0 before its first frame): a pass
    /// at another scale than `dpi` draws stretched and takes input offset
    pub pass_dpi: f64,
    pub x: f64,
    pub y: f64,
    /// it has the keyboard focus
    pub key: bool,
}

pub struct GrabSink {
    pub id: u64,
    pub window: Option<usize>,
    pub requested_at: Instant,
    pub scale: f64,
    /// Deliver tightly packed RGBA bytes (top row first) instead of a PNG.
    pub raw_output: bool,
    pub cancelled: Arc<AtomicBool>,
    pub encoding: Arc<AtomicBool>,
    pub tx: SyncSender<Result<Grabbed, String>>,
    /// The grab's slot in the window, released after the sink's own drop (normally on the
    /// encoder worker, after the grab is delivered).
    pub _permit: GrabPermit,
}

/// A grab's frame: its number (`Cx::repaint_id`, counting drawn frames) and how many live patches
/// (`std::relax::patched`) the app had applied when it drew it. A grab after a patch shows the
/// patch's frame or a later one: the grab's command is served after the app took in every patch
/// installed before it (`Cx::poll_live_patch`), and its snapshot is a frame drawn after that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drawn {
    pub frame: u64,
    pub patch: u64,
}

/// The armed grabs whose snapshot a frame build took, until they are delivered.
static GRAB_DRAWN: Mutex<Vec<(u64, Drawn)>> = Mutex::new(Vec::new());

/// The frame build put a snapshot of `ids` in frame `frame` (the UI thread).
pub fn grabs_drawn(ids: &[u64], drawn: Drawn) {
    let mut d = GRAB_DRAWN.lock().unwrap_or_else(|e| e.into_inner());
    d.extend(ids.iter().filter(|id| is_grab_id(**id)).map(|id| (*id, drawn)));
}

/// The frame grab `id`'s snapshot was taken in (taken out: the grab is being delivered).
pub fn take_grab_drawn(id: u64) -> Option<Drawn> {
    let mut d = GRAB_DRAWN.lock().unwrap_or_else(|e| e.into_inner());
    let i = d.iter().position(|x| x.0 == id)?;
    Some(d.swap_remove(i).1)
}

impl Drop for GrabSink {
    fn drop(&mut self) {
        // a grab that failed or was cancelled after its frame was drawn; `_permit` then
        // releases the slot
        take_grab_drawn(self.id);
    }
}

pub struct EncodeJob {
    pub sink: GrabSink,
    pub width: u32,
    pub height: u32,
    pub pixels: crate::HostBuf,
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
/// app ended it ([`stop_encoder`]). Unbounded: the grab window ([`GRAB_IN_FLIGHT_LIMIT`]) bounds
/// what waits in it, and a send from the UI thread never blocks or refuses.
fn encoder() -> &'static Mutex<Option<Sender<EncodeJob>>> {
    static ENCODER: Mutex<Option<Sender<EncodeJob>>> = Mutex::new(None);
    &ENCODER
}

/// Starts the encoder worker if it is not running (a grab command's thread, never the UI or a
/// GPU callback). It runs until [`stop_encoder`].
pub fn start_encoder() -> Result<(), String> {
    let mut encoder = encoder().lock().unwrap_or_else(|e| e.into_inner());
    if encoder.is_some() {
        return Ok(());
    }
    let (tx, rx) = channel::<EncodeJob>();
    std::thread::Builder::new()
        .name("makepad-remote-png".into())
        .spawn(move || {
            while let Ok(mut job) = rx.recv() {
                if job.sink.cancelled.load(Ordering::Relaxed) {
                    continue;
                }
                let started = Instant::now();
                let result = encode_grab(&job);
                drop(job.pixels);
                let result = result.map(|(width, height, data)| Grabbed {
                    window_id: job.sink.window.unwrap_or(0),
                    width,
                    height,
                    data,
                    capture_ms: job.capture_ms,
                    encode_ms: started.elapsed().as_secs_f64() * 1000.0,
                    backend_png: job.backend_png,
                    drawn: take_grab_drawn(job.sink.id),
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

/// Box-filter a pair of four-byte pixel rows, dropping an odd trailing column.
fn halve_row_pair(r0: &[u8], r1: &[u8], w: usize, dst: &mut [u8]) {
    for x in 0..(w / 2) {
        for c in 0..4 {
            let i = 8 * x + c;
            let sum = r0[i] as u32 + r0[i + 4] as u32 + r1[i] as u32 + r1[i + 4] as u32;
            dst[x * 4 + c] = ((sum + 2) / 4) as u8;
        }
    }
}

/// 2x2 box filter of 4-byte pixels in rows of `stride` bytes (any channel order, rows in memory
/// order), tightly packed; an odd last row or column is dropped. The one downscale step of a
/// grab: the queue's halving ([`halve_grab`]) and the encoder's ([`encode_grab`]) are this, so
/// a frame comes out the same whichever of them halved it. None when the pixels are too few.
fn halve_pixels(pixels: &[u8], w: usize, h: usize, stride: usize) -> Option<Vec<u8>> {
    if w < 2 || h < 2 || stride < w.checked_mul(4)? || pixels.len() < stride.checked_mul(h)? {
        return None;
    }
    let (w2, h2) = (w / 2, h / 2);
    let mut out = vec![0u8; w2 * h2 * 4];
    for (y, dst) in out.chunks_exact_mut(w2 * 4).enumerate() {
        let (r0, r1) = (&pixels[2 * y * stride..], &pixels[(2 * y + 1) * stride..]);
        halve_row_pair(r0, r1, w, dst);
    }
    Some(out)
}

/// 2x2 box-filter a raw readback in place of the job's pixels (same
/// channel order and origin, tightly packed); the sink's scale doubles
/// so the requested output size is kept where it still can be.
pub fn halve_grab(job: &mut EncodeJob) -> bool {
    let (w, h) = (job.width as usize, job.height as usize);
    let Some(out) = halve_pixels(&job.pixels, w, h, job.stride) else {
        return false;
    };
    job.pixels = crate::HostBuf::from_vec(out);
    job.width = (w / 2) as u32;
    job.height = (h / 2) as u32;
    job.stride = (w / 2) * 4;
    job.sink.scale = (job.sink.scale * 2.0).min(1.0);
    true
}

pub fn submit_encode(mut job: EncodeJob, raw: bool) {
    if job.sink.cancelled.load(Ordering::Relaxed) {
        return;
    }
    // Over the pixel budget (a fast gseq of a large window outruns the encoder): halve the
    // frame while its requested output size allows (a requested scale <= 0.5 loses nothing,
    // the encoder would scale down anyway). Past that the frame queues as it is and the
    // encoder catches up: a sequence slows down, but no frame shrinks below the size asked
    // for and none is refused. The grab window (GRAB_IN_FLIGHT_LIMIT) bounds what waits.
    while raw
        && job.sink.scale <= 0.5
        && job.width >= 2
        && job.height >= 2
        && GRAB_BYTES.load(Ordering::Relaxed) + job.pixels.len() > GRAB_QUEUE_BUDGET_BYTES
        && halve_grab(&mut job)
    {}
    let len = job.pixels.len();
    let used = GRAB_BYTES.fetch_add(len, Ordering::Relaxed) + len;
    static OVER_BUDGET: AtomicBool = AtomicBool::new(false);
    let over = used > GRAB_QUEUE_BUDGET_BYTES;
    if over && !OVER_BUDGET.swap(true, Ordering::Relaxed) {
        crate::log!(
            "[makepad-remote] grab pixels waiting ({} MiB) exceed queue budget ({} MiB); frames queue at their requested size",
            used >> 20,
            GRAB_QUEUE_BUDGET_BYTES >> 20
        );
    } else if !over {
        OVER_BUDGET.store(false, Ordering::Relaxed);
    }
    job.bytes = Some(GrabBytes(len));
    job.backend_png = !raw;
    let tx = encoder().lock().unwrap_or_else(|e| e.into_inner()).clone();
    match tx {
        Some(tx) => {
            job.sink.encoding.store(true, Ordering::Release);
            // fails only when the worker has ended
            if let Err(std::sync::mpsc::SendError(job)) = tx.send(job) {
                let _ = job.sink.tx.try_send(Err("the grab encoder has ended (the app is quitting)".into()));
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
    pub live: CommandLive,
    /// who sent it ([`asked_by`]; empty: not known): the input it injects says it with what it
    /// reaches (a window's close or caption button)
    pub by: String,
}

/// Who sent the command being queued, on the endpoint's thread that answers it: the agent and its
/// process (`makepad_remote::cmd::asked_by`); empty where not known. Under the JIT the host's
/// endpoint answers, and its host says.
#[cfg(not(relax))]
pub fn asked_by() -> String {
    makepad_remote::cmd::asked_by().unwrap_or_default()
}

#[cfg(all(relax, any(unix, windows)))]
pub fn asked_by() -> String {
    let hook = crate::cx::relax_host_symbol(c"relax_inspect_asker");
    if hook.is_null() {
        return String::new();
    }
    // SAFETY: the host's `relax_inspect_asker(buf, cap) -> len`
    let asker = unsafe { std::mem::transmute::<*mut std::ffi::c_void, extern "C" fn(*mut u8, usize) -> usize>(hook) };
    let mut buf = vec![0u8; 512];
    let n = asker(buf.as_mut_ptr(), buf.len());
    buf.truncate(n.min(512));
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(all(relax, not(any(unix, windows))))]
pub fn asked_by() -> String {
    String::new()
}

pub enum Cmd {
    Activity(Sender<Reply>),
    Gamepads(Sender<Reply>),
    Input {
        window: Option<usize>,
        inputs: Vec<Input>,
        wait: bool,
        tx: Sender<Reply>,
    },
    Grab {
        window: Option<usize>,
        /// 0 for a standalone grab; non-zero identifies a `frames n=N` series so refilled
        /// chunks append to the same `GrabBatch` and preserve `last_request`.
        series: u64,
        /// `(grab_id, due_at)` in cadence order.
        grabs: Vec<(u64, Instant)>,
        cancelled: Arc<AtomicBool>,
        tx: Sender<Reply>,
    },
    CancelGrabs(Vec<u64>),
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
    /// The UI thread's own counters (`uicounts`): read on the thread that runs the event loop;
    /// with what the app's other threads waited for a core (`waited=1`).
    UiCounts(bool, Sender<Reply>),
    CapStop(Sender<Reply>),
    /// `/cap/pause` (true) and `/cap/resume` (false).
    CapPause(bool, Sender<Reply>),
    /// A command built by a platform-side route (`/cap/start`, `/cursor`):
    /// its payload names platform types, so only platform's `apply` reads it.
    Platform(Box<dyn std::any::Any + Send>),
    /// `front`: the person asked to see the app ([`crate::cx::Cx::show_to_person`]).
    Front(Sender<Reply>),
    /// Closes one window the way its close button does: the app is asked first
    /// (`WindowCloseRequested`) and may keep the window, e.g. hide it instead.
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
    /// The run ends either way (a hub stopping its lease's run): the app's exit, whatever the person
    /// is doing to it. Refused, the hub ended it on the spot, without its exit write.
    QuitForced(Sender<Reply>),
    /// The quit a logout asks: on macOS AppKit's `terminate:` (where a logout's quit event arrives),
    /// so AppKit asks `applicationShouldTerminate:` and the app's held quit runs on its real path.
    QuitNative { force: bool, tx: Sender<Reply> },
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
    /// `Cx::gpu_inventory()`: GPU + CPU-retained graphics memory across
    /// textures (including free pool slots), draw-list instances, and geometries.
    GpuInventory {
        n: usize,
        tx: Sender<Reply>,
    },
}

impl Cmd {
    /// Only observes the app (its state, its picture, its counters, a capture of it): what the
    /// UI thread runs applying it answers the asker and is none of the app's work (`uicounts`'
    /// `answered_ns`). Input, steps, tweaks, window changes and the app's own commands drive it.
    pub fn observes(&self) -> bool {
        matches!(
            self,
            Self::Activity(_) | Self::Gamepads(_) | Self::Grab { .. } | Self::CancelGrabs(_)
                | Self::Settled(_) | Self::UiCounts(..) | Self::CapStop(_) | Self::CapPause(..)
                | Self::ShaderConsts { .. } | Self::GpuInventory { .. }
        )
    }

    pub fn mutation_reply(&self) -> Option<&Sender<Reply>> {
        match self {
            Self::Input { tx, .. } | Self::Close { tx, .. }
            | Self::Quit(tx) | Self::QuitNative { force: false, tx } | Self::ShaderConstPatch { tx, .. } => Some(tx),
            Self::Tweak { op, args, tx, .. }
                if !matches!(op.as_str(), "state" | "diff" | "final")
                    && !op.strip_prefix("design_").is_some_and(|op| design_reads(op, args)) =>
            {
                Some(tx)
            }
            Self::Ai { op, tx, .. } if op != "transcript" => Some(tx),
            _ => None,
        }
    }
}

pub enum Input {
    /// AppKit hit testing and native delivery; Makepad hits return to the UI queue.
    NativeClick { x: f64, y: f64 },
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
    /// One step of a touch (`/m?k=touch&phase=start|move|stop|cancel&id=`).
    Touch {
        x: f64,
        y: f64,
        uid: u64,
        state: crate::event::TouchState,
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
    /// A file dropped at a point: through Drag, Drop and DragEnd, or Drop and DragEnd on the
    /// open drag of the same file (after a Drag there when the point moved).
    DropFile {
        path: String,
        x: f64,
        y: f64,
    },
    /// A file dragged over a point and held there (`drag`): one Drag event, the drag left open
    /// for the next `drag`, `drag_end` or `drop`.
    DragFile {
        path: String,
        x: f64,
        y: f64,
    },
    /// The open drag leaves the window without a drop (`drag_end`): DragEnd.
    DragFileEnd,
    /// The window's own maximise (macOS: toggleFullScreen) — the test
    /// hook for the maximise/occlusion proof.
    Maximize,
    /// The window's fullscreen (`CxOsOp::FullscreenWindow`): under a window manager the whole
    /// screen at its own size, where a maximise keeps the bar and the dock.
    Fullscreen,
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

/// What the command line, the environment and the per-user setting ask of the remote endpoint,
/// by the one rule a native build and `makepad run` share ([`makepad_remote::ask::request_from`]):
/// off by default; `--remote`, `MAKEPAD_REMOTE=1` or `MAKEPAD_INSPECT=1` for an agent;
/// `MAKEPAD_REMOTE=local` or the per-user setting (`~/.makepad/remote`: `local`) as an ordinary
/// launch; `MAKEPAD_REMOTE=0` off.
pub use makepad_remote::ask::Request;

pub fn request() -> Request {
    let asked = std::env::var("MAKEPAD_REMOTE").ok();
    // an Android app gets no environment of its own, so `adb shell setprop debug.makepad.remote 1` asks too
    #[cfg(target_os = "android")]
    let asked = asked.or_else(|| {
        let value = makepad_hal::android::system_property("debug.makepad.remote")?;
        // every makepad app on the device reads it, so a value it can't use is ignored instead of ending the app
        if let Request::Refused(_) = makepad_remote::ask::request_from(std::iter::empty(), Some(&value), None) {
            eprintln!("[makepad-remote] debug.makepad.remote={value} is ignored: 1 or local asks for the endpoint, 0 turns it off");
            return None;
        }
        Some(value)
    });
    match makepad_remote::ask::request_with(asked.as_deref()) {
        // nothing outside a release Android or iOS app can reach its socket, and the system kills such
        // an app without running exit handlers, so its socket and discovery file would only pile up
        Request::Local if cfg!(all(any(target_os = "android", target_os = "ios", target_os = "tvos"), not(debug_assertions), not(relax))) => Request::Off,
        request => request,
    }
}

/// True when this process was launched for an agent or a test (`--remote`,
/// `MAKEPAD_REMOTE=1` or `MAKEPAD_INSPECT=1`); the endpoint itself is open also as an ordinary
/// launch on a developer computer ([`Request::Local`]: `MAKEPAD_REMOTE=local` or the per-user
/// setting), and off otherwise (a release Android or iOS app opens it only when asked).
/// Argv, env and the per-user setting, usable before the endpoint itself is up: the platform's focus policy reads
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

/// The application title without decorations added when a window is created for remote control.
pub fn application_title(title: &str) -> &str {
    let title = if requested() {
        title_tag().and_then(|tag| title.strip_suffix(tag))
            .map(|base| base.strip_suffix(' ').unwrap_or(base)).unwrap_or(title)
    } else { title };
    if let Some(lane) = crate::window::test_lane() {
        if let Some(name) = title.strip_prefix("TEST: lane ").and_then(|s| s.strip_prefix(lane))
            .and_then(|s| s.strip_prefix(" · ")) { return name; }
    }
    title
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
    Command { words: "status", args: "[w=ID]", answers: "{\"app\",\"pid\",\"w\":[{\"i\":id,\"t\":title,\"sz\":[w,h],\"px\":[w,h],\"dpi\",\"pass_dpi\",\"pos\":[x,y],\"key\":has the keyboard focus}],\"activity\"}", service: "ui", risk: CmdRisk::Read, params: "\"w\":{\"type\":\"integer\",\"description\":\"window id (status lists them)\"}", required: &[], positional: &[] },
    Command { words: "gamepads", args: "", answers: "connected controllers and their current input states", service: "ui", risk: CmdRisk::Read, params: "", required: &[], positional: &[] },
    Command { words: "worker_violations", args: "", answers: "operations required to run on workers that ran on a UI thread, with their counts since run start", service: "ui", risk: CmdRisk::Read, params: "", required: &[], positional: &[] },
    Command { words: "activity", args: "", answers: "native user activity: user_active, user_seq, idle_ms, quiet_ms, held, last_input, window (also in status)", service: "ui", risk: CmdRisk::Read, params: "", required: &[], positional: &[] },
    Command { words: "grab", args: "[w=ID] [scale=1] [out=FILE]", answers: "client side (frames service): the window as a PNG file; prints {\"png\":path,\"w\",\"sz\",\"capture_ms\",\"encode_ms\",\"capture_kind\"}", service: "ui", risk: CmdRisk::Read, params: makepad_remote::params::GRAB, required: &[], positional: &[] },
    Command { words: "gseq", args: "[n=8] [every_ms=50] [scale=1] [w=ID] [out=DIR]", answers: "client side (frames service): n frames, a separate present per deadline; n>=1, every_ms=8..=60000", service: "ui", risk: CmdRisk::Read, params: makepad_remote::params::GSEQ, required: &[], positional: &[] },
    Command { words: "rec", args: "[out=DIR] [seconds=5] [fps=30] [codec=png|raw] [w=ID]", answers: "client side (frames cap=1): capture by streaming -- every presented frame is read back on the device and written here as frame_<slot>.png; prints {\"dir\",\"frames\",\"missing\",\"slots\",\"sz\"}; no video encoder on the device", service: "ui", risk: CmdRisk::Read, params: makepad_remote::params::REC, required: &[], positional: &[] },
    Command { words: "gq", args: "[scale=1] [w=ID]", answers: "client side: FINISH HERE: grab every window, then quit. {\"png\":[paths],\"quit\":1}", service: "ui", risk: CmdRisk::Destructive, params: makepad_remote::params::GQ, required: &[], positional: &[] },
    Command { words: "mouse", args: "k=move|down|up|click|dblclick|scroll|pinch|touch x= y= [b=0] [dx= dy=] [scale= phase=begin|update|end] [scroll phase=touched|began|changed|ended|momentum|momentum_ended: a touchpad scroll] [touch phase=start|move|stop|cancel id=N: one finger] [time=] [hw=1] [native=1: AppKit primary click] [shift=1 ctrl=1 alt=1 logo=1] [w=ID] [wait=1]", answers: "inject a mouse event through the real event path", service: "ui", risk: CmdRisk::Act, params: "\"k\":{\"type\":\"string\",\"enum\":[\"move\",\"down\",\"up\",\"click\",\"dblclick\",\"scroll\",\"pinch\",\"touch\"]},\"x\":{\"type\":\"number\"},\"y\":{\"type\":\"number\"},\"b\":{\"type\":\"integer\"},\"dx\":{\"type\":\"number\"},\"dy\":{\"type\":\"number\"},\"scale\":{\"type\":\"number\"},\"phase\":{\"type\":\"string\",\"enum\":[\"begin\",\"update\",\"end\",\"touched\",\"began\",\"changed\",\"ended\",\"momentum\",\"momentum_ended\",\"start\",\"move\",\"stop\",\"cancel\"]},\"id\":{\"type\":\"integer\"},\"time\":{\"type\":\"number\"},\"hw\":{\"type\":\"boolean\"},\"native\":{\"type\":\"boolean\"},\"shift\":{\"type\":\"boolean\"},\"ctrl\":{\"type\":\"boolean\"},\"alt\":{\"type\":\"boolean\"},\"logo\":{\"type\":\"boolean\"},\"w\":{\"type\":\"integer\"},\"wait\":{\"type\":\"boolean\"},\"x1\":{\"type\":\"number\"},\"y1\":{\"type\":\"number\"},\"steps\":{\"type\":\"integer\"},\"back\":{\"type\":\"boolean\"},\"if_user_seq\":{\"type\":\"integer\"}", required: &["k"], positional: &[] },
    Command { words: "click", args: "x= y= [b=0] [w=ID] [wait=1]", answers: "mouse k=click", service: "ui", risk: CmdRisk::Act, params: "\"x\":{\"type\":\"number\"},\"y\":{\"type\":\"number\"},\"b\":{\"type\":\"integer\"},\"w\":{\"type\":\"integer\"},\"wait\":{\"type\":\"boolean\"},\"shift\":{\"type\":\"boolean\"},\"ctrl\":{\"type\":\"boolean\"},\"alt\":{\"type\":\"boolean\"},\"logo\":{\"type\":\"boolean\"},\"hw\":{\"type\":\"boolean\"},\"native\":{\"type\":\"boolean\"},\"time\":{\"type\":\"number\"},\"if_user_seq\":{\"type\":\"integer\"}", required: &["x", "y"], positional: &[] },
    Command { words: "move", args: "x= y= [hw=1] [w=ID] [wait=1]", answers: "mouse k=move", service: "ui", risk: CmdRisk::Act, params: "\"x\":{\"type\":\"number\"},\"y\":{\"type\":\"number\"},\"w\":{\"type\":\"integer\"},\"wait\":{\"type\":\"boolean\"},\"shift\":{\"type\":\"boolean\"},\"ctrl\":{\"type\":\"boolean\"},\"alt\":{\"type\":\"boolean\"},\"logo\":{\"type\":\"boolean\"},\"hw\":{\"type\":\"boolean\"},\"time\":{\"type\":\"number\"},\"if_user_seq\":{\"type\":\"integer\"}", required: &["x", "y"], positional: &[] },
    Command { words: "key", args: "t=TEXT | c=KeyA [k=down|up|press] [shift=1 ...] [w=ID] [wait=1]", answers: "type text or press a key (Escape ReturnKey Tab Backspace ArrowLeft F1 Key1 ..)", service: "ui", risk: CmdRisk::Act, params: "\"t\":{\"type\":\"string\",\"description\":\"text to type\"},\"c\":{\"type\":\"string\",\"description\":\"a key code: Escape ReturnKey Tab Backspace ArrowLeft F1 KeyA Key1 ..\"},\"k\":{\"type\":\"string\",\"enum\":[\"down\",\"up\",\"press\"]},\"shift\":{\"type\":\"boolean\"},\"ctrl\":{\"type\":\"boolean\"},\"alt\":{\"type\":\"boolean\"},\"logo\":{\"type\":\"boolean\"},\"w\":{\"type\":\"integer\"},\"wait\":{\"type\":\"boolean\"},\"if_user_seq\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "text", args: "t=TEXT [w=ID] [wait=1]", answers: "type text", service: "ui", risk: CmdRisk::Act, params: "\"t\":{\"type\":\"string\"},\"w\":{\"type\":\"integer\"},\"wait\":{\"type\":\"boolean\"},\"if_user_seq\":{\"type\":\"integer\"}", required: &["t"], positional: &[] },
    Command { words: "drop", args: "path=ABS x= y= [w=ID] [wait=1]", answers: "drop one absolute file path through Drag/Drop/DragEnd; on an open drag of it, Drop and DragEnd there", service: "ui", risk: CmdRisk::Act, params: "\"path\":{\"type\":\"string\"},\"x\":{\"type\":\"number\"},\"y\":{\"type\":\"number\"},\"w\":{\"type\":\"integer\"},\"wait\":{\"type\":\"boolean\"},\"if_user_seq\":{\"type\":\"integer\"}", required: &["path"], positional: &[] },
    Command { words: "drag", args: "path=ABS x= y= [w=ID] [wait=1]", answers: "hold one absolute file path dragged over x,y: a Drag event (the first one enters, repeats move it); drag_end leaves, drop drops it", service: "ui", risk: CmdRisk::Act, params: "\"path\":{\"type\":\"string\"},\"x\":{\"type\":\"number\"},\"y\":{\"type\":\"number\"},\"w\":{\"type\":\"integer\"},\"wait\":{\"type\":\"boolean\"},\"if_user_seq\":{\"type\":\"integer\"}", required: &["path"], positional: &[] },
    Command { words: "drag_end", args: "[w=ID] [wait=1]", answers: "the open drag leaves the window without a drop (DragEnd)", service: "ui", risk: CmdRisk::Act, params: "\"w\":{\"type\":\"integer\"},\"wait\":{\"type\":\"boolean\"},\"if_user_seq\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "window", args: "k=maximize | k=fullscreen | k=resize width= height= | k=dragquery x= y= [w=ID] [wait=1]", answers: "window operations (the maximise/occlusion proof, fullscreen at the screen's size, the caption hit test)", service: "ui", risk: CmdRisk::Act, params: "\"k\":{\"type\":\"string\",\"enum\":[\"maximize\",\"fullscreen\",\"resize\",\"dragquery\"]},\"width\":{\"type\":\"number\"},\"height\":{\"type\":\"number\"},\"x\":{\"type\":\"number\"},\"y\":{\"type\":\"number\"},\"w\":{\"type\":\"integer\"},\"wait\":{\"type\":\"boolean\"},\"if_user_seq\":{\"type\":\"integer\"}", required: &["k"], positional: &[] },
    Command { words: "cpu", args: "", answers: "shared CPU budget and app pool summary", service: "platform", risk: CmdRisk::Read, params: "", required: &[], positional: &[] },
    Command { words: "log", args: "[n=50] [since=N]", answers: "{\"n\":lastseq,\"d\":dropped,\"l\":[lines]}", service: "logs", risk: CmdRisk::Read, params: "\"n\":{\"type\":\"integer\"},\"since\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "net", args: "", answers: "the app's HTTP requests and network event queue, answered off the UI thread: a verdict per runtime first (STUCK: an answer waits unread in the app's queue, with when the app last read it / WAITING: in flight with nothing from the backend / OK), then per request: id, method and URL, the backend's answer and when, whether the app read it", service: "logs", risk: CmdRisk::Read, params: "", required: &[], positional: &[] },
    Command { words: "loop", args: "", answers: "what the UI thread's event loop waits for, answered off the UI thread: one verdict line (STUCK: a frame asked for and never granted, or a raised signal the loop slept through / WAITING: the GPU / OK) with the pump, frame-request, GPU and signal ages", service: "logs", risk: CmdRisk::Read, params: "", required: &[], positional: &[] },
    Command { words: "midi", args: "a= b= c= [p=PORT] | k=ports n=NAME[,NAME] | k=out | k=reset", answers: "inject a MIDI message as if a device sent it; declare ports; read back what the app sent", service: "ui", risk: CmdRisk::Act, params: "\"a\":{\"type\":\"integer\"},\"b\":{\"type\":\"integer\"},\"c\":{\"type\":\"integer\"},\"p\":{\"type\":\"string\"},\"k\":{\"type\":\"string\",\"enum\":[\"ports\",\"out\",\"reset\"]},\"n\":{\"type\":\"string\"},\"if_user_seq\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "trace", args: "[topics=a,b] [off=1]", answers: "get / set the trace topics", service: "logs", risk: CmdRisk::Act, params: "\"topics\":{\"type\":\"string\"},\"off\":{\"type\":\"boolean\"}", required: &[], positional: &[] },
    Command { words: "step", args: "[frames=1] [fps=60] [wait_loads=1] [w=ID]", answers: "--virtual-clock only: run frames of 1/fps; answers {\"frame\",\"time\"} after the last", service: "ui", risk: CmdRisk::Act, params: "\"frames\":{\"type\":\"integer\"},\"fps\":{\"type\":\"number\"},\"wait_loads\":{\"type\":\"boolean\"},\"w\":{\"type\":\"integer\"},\"if_user_seq\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "settled", args: "", answers: "{\"settled\":bool,\"reasons\":[next_frame|redraw|repaint|timer|step|shaders|loads|pool],\"loads\":{kind:n},\"pool\":{\"queued\":[interactive,visible,background],\"running\":n,\"taken\":n}} (pool: the app pool's jobs waiting for a worker and its CPU permit, on its workers, and taken so far)", service: "ui", risk: CmdRisk::Read, params: "", required: &[], positional: &[] },
    Command { words: "uicounts", args: "[waited=1]", answers: "{\"ins\":n,\"cyc\":n,\"cpu_ns\":n,\"answered_ns\":n,\"process_cpu_ns\":n,\"served_cpu_ns\":n,\"runtime_cpu_ns\":n,\"faults\":n} of the UI thread so far (ins/cyc: thread_counters, user mode: macOS thread_selfcounts, Linux perf_event; else 0) and of its CPU what it ran answering remote peers (the commands it applied for them that only observe the app, the frames it drew only to build a tree a request asked for), the CPU of all the process's threads, and of that what serving remote peers cost it (its serving threads, and its UI thread answering their commands) and what the runtime's own threads ran (under makepad run, the JIT's; natively 0), and the process's major page faults so far (getrusage; 0 on Windows); waited=1 adds \"waited_ns\":n, what the app's threads that run now (not the UI thread, the serving ones or the runtime's) have spent runnable, waiting for a core (Linux schedstat run_delay; 0 where the OS keeps none per thread: macOS, Windows), and \"io_threads\":n, how many of its threads (not the serving ones) wait in the kernel uninterruptibly now, on the disk or a page (Linux state D, macOS TH_STATE_UNINTERRUPTIBLE; 0 on Windows), a file or two a thread: ask it seldom", service: "ui", risk: CmdRisk::Read, params: "\"waited\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "cap start", args: "path=ABS.mp4 [fps=60] [audio=1] [overwrite=1] [w=ID]", answers: "record the window in process (native pixels, H.264)", service: "ui", risk: CmdRisk::Act, params: "\"path\":{\"type\":\"string\"},\"fps\":{\"type\":\"number\"},\"audio\":{\"type\":\"boolean\"},\"overwrite\":{\"type\":\"boolean\"},\"w\":{\"type\":\"integer\"}", required: &["path"], positional: &[] },
    Command { words: "cap stop", args: "[hashes=1]", answers: "finalize the recording: {\"frames\",\"sz\",\"missing\",\"hash\"}", service: "ui", risk: CmdRisk::Act, params: "\"hashes\":{\"type\":\"boolean\"}", required: &[], positional: &[] },
    Command { words: "input record", args: "path=<file>", answers: "record the person's native pointer, wheel, key and text input as the commands that inject it (one line each: ms, presented frame, command)", service: "ui", risk: CmdRisk::Act, params: "\"path\":{\"type\":\"string\"}", required: &["path"], positional: &[] },
    Command { words: "input replay", args: "path=<file> [speed=1] [sync=time|frame]", answers: "play a recording back through the injecting commands, paced by time or by presented frames (same input on the same frame)", service: "ui", risk: CmdRisk::Act, params: "\"path\":{\"type\":\"string\"},\"speed\":{\"type\":\"number\"},\"sync\":{\"type\":\"string\",\"enum\":[\"time\",\"frame\"]}", required: &["path"], positional: &[] },
    Command { words: "input stop", args: "", answers: "end the recording (its event count) and any replay", service: "ui", risk: CmdRisk::Act, params: "", required: &[], positional: &[] },
    Command { words: "input status", args: "", answers: "recording on/off, events recorded, replay events left", service: "ui", risk: CmdRisk::Read, params: "", required: &[], positional: &[] },
    Command { words: "cap pause", args: "", answers: "stop writing without stopping the clock", service: "ui", risk: CmdRisk::Act, params: "", required: &[], positional: &[] },
    Command { words: "cap resume", args: "", answers: "restart writing", service: "ui", risk: CmdRisk::Act, params: "", required: &[], positional: &[] },
    Command { words: "cursor", args: "[show=1|0] [style=arrow|hand|text|crosshair|app] [x= y=] [w=ID]", answers: "a pointer drawn into the frame, moved by injected mouse input", service: "ui", risk: CmdRisk::Act, params: "\"show\":{\"type\":\"boolean\"},\"style\":{\"type\":\"string\",\"enum\":[\"arrow\",\"hand\",\"text\",\"crosshair\",\"app\"]},\"x\":{\"type\":\"number\"},\"y\":{\"type\":\"number\"},\"w\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "front", args: "", answers: "the person asked to see the app (a repeat `devhub show`): it comes to the front, and a tray app with no window open opens what a click on its icon opens; {\"front\":1}", service: "ui", risk: CmdRisk::Act, params: "", required: &[], positional: &[] },
    Command { words: "close", args: "[w=ID]", answers: "close one window like its close button: the app is asked first and may keep or hide the window", service: "ui", risk: CmdRisk::Destructive, params: "\"w\":{\"type\":\"integer\"},\"if_user_seq\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "resize", args: "w= h= [window=ID]", answers: "give the window a new inner size in layout points; answers after the frame that shows it", service: "ui", risk: CmdRisk::Act, params: "\"w\":{\"type\":\"number\"},\"h\":{\"type\":\"number\"},\"window\":{\"type\":\"integer\"},\"if_user_seq\":{\"type\":\"integer\"}", required: &["w", "h"], positional: &[] },
    Command { words: "handsoff", args: "[on=1|0]", answers: "the red hands-off frame round the window", service: "ui", risk: CmdRisk::Act, params: "\"on\":{\"type\":\"boolean\"}", required: &[], positional: &[] },
    Command { words: "ai", args: "[on=1|0] | say=TEXT", answers: "the AI chat overlay: toggle or type a line", service: "ui", risk: CmdRisk::Act, params: "\"on\":{\"type\":\"boolean\"},\"say\":{\"type\":\"string\"},\"confirm\":{\"type\":\"string\"},\"if_user_seq\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "ai transcript", args: "", answers: "the conversation", service: "ui", risk: CmdRisk::Read, params: "", required: &[], positional: &[] },
    Command { words: "tweak", args: "[on=1|0]", answers: "the TWEAKER design-feedback overlay", service: "edit", risk: CmdRisk::Act, params: makepad_remote::params::TWEAK_TOGGLE, required: &[], positional: &[] },
    Command { words: "tweak state", args: "", answers: "selection, editable properties, diff log, annotations, asks waiting on the source", service: "edit", risk: CmdRisk::Read, params: makepad_remote::params::TWEAK_STATE, required: &[], positional: &[] },
    Command { words: "tweak apply", args: "path=a.b.c splash={padding: 20} | prop= value=", answers: "live-apply and relayout", service: "edit", risk: CmdRisk::Act, params: makepad_remote::params::TWEAK_APPLY, required: &[], positional: &[] },
    Command { words: "tweak diff", args: "", answers: "the raw edit log", service: "edit", risk: CmdRisk::Read, params: makepad_remote::params::TWEAK_DIFF, required: &[], positional: &[] },
    Command { words: "tweak clear", args: "", answers: "reset the edit log", service: "edit", risk: CmdRisk::Act, params: makepad_remote::params::TWEAK_CLEAR, required: &[], positional: &[] },
    Command { words: "tweak final", args: "", answers: "coalesced end state per widget; with \"drew\":1 the client adds \"png\" (the composited window)", service: "edit", risk: CmdRisk::Read, params: makepad_remote::params::TWEAK_FINAL, required: &[], positional: &[] },
    Command { words: "tweak op", args: "op=NAME ...", answers: "a tweaker op that has no named command", service: "edit", risk: CmdRisk::Act, params: "\"op\":{\"type\":\"string\"}", required: &["op"], positional: &[] },
    Command { words: "tweak grab", args: "[w=ID] [scale=1] [out=FILE]", answers: "client side: grab with the overlay composited", service: "edit", risk: CmdRisk::Read, params: makepad_remote::params::GRAB, required: &[], positional: &[] },
    Command { words: "design", args: "tree [w=W] [path=P|reveal=P|query=Q] [depth=D offset=N limit=L] | pick at=x,y|path=P | props|locate|states path=P | apply path=P splash=...|prop=K value=V | typed path=P prop=K value=V | pose path=P layer=L state=S | theme get|set|install | preview file=F text=T|base_hash=H edits=E|upload=U | preview_chunk file=F id=U seq=N data=B64 | revert file=F | verify [file=F path=P] | drop at=x,y [exclude=P] | <structural op> ...", answers: "live design data (tree pick props apply typed locate theme states pose), and structural edits to the Splash source (preview preview_chunk revert drop open state palette insert move delete duplicate wrap up down out rename set bake undo redo reset patch commit verify close)", service: "edit", risk: CmdRisk::Act, params: "\"op\":{\"type\":\"string\"}", required: &["op"], positional: &["op"] },
    Command { words: "shader consts", args: "[shader=ID]", answers: "the compiled shaders' hot-patchable constants", service: "edit", risk: CmdRisk::Read, params: "\"shader\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "shader const", args: "shader=ID i=N v=VALUE | reset=1", answers: "patch one constant on the GPU (no recompile)", service: "edit", risk: CmdRisk::Act, params: "\"shader\":{\"type\":\"integer\"},\"i\":{\"type\":\"integer\"},\"v\":{\"type\":\"number\"},\"reset\":{\"type\":\"boolean\"},\"if_user_seq\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "gpu_inventory", args: "[n=15]", answers: "Cx GPU and CPU-retained graphics memory (textures, free pool slots, draw-list instances, geometries)", service: "inspect", risk: CmdRisk::Read, params: "\"n\":{\"type\":\"integer\"}", required: &[], positional: &[] },
    Command { words: "quit", args: "[force=1] [native=1]", answers: "shut the app down gracefully. force=1 bypasses the activity guard for an instance devhub or an agent started; never the user's own app. native=1 asks macOS AppKit to terminate as at logout", service: "ui", risk: CmdRisk::Destructive, params: "\"force\":{\"type\":\"integer\"},\"native\":{\"type\":\"boolean\"},\"if_user_seq\":{\"type\":\"integer\"}", required: &[], positional: &[] },
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
            None => {
                let ops: Vec<&str> = makepad_remote::params::DESIGN.iter().map(|(op, _)| *op).collect();
                Err(format!("design <op>: {}", ops.join(" ")))
            }
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
        None => {
            let mut names = Vec::new();
            for command in COMMANDS {
                if let Some(name) = command.words.split_whitespace().next() {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }
            Err(format!("unknown command `{first}`: the commands are {} (`help` describes each)", names.join(", ")))
        }
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
    reply_to_answer(ask("step", WaitKind::Turn,
        move |tx| Cmd::Step {
            window,
            frames,
            fps,
            wait_loads,
            tx,
        },
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
    let what = if op == "transcript" { "ai transcript" } else { "ai" };
    reply_to_answer(ask(what, frame_or_turn(wait), move |tx| Cmd::Ai { op, args, wait, tx }))
}

fn design_reads(op: &str, args: &[(String, String)]) -> bool {
    match op {
        "tree" | "pick" | "props" | "locate" | "states" | "drop" | "preview_chunk" | "state" | "palette" | "patch" | "commit" | "verify" => true,
        "theme" => args.iter().any(|(key, _)| key == "get")
            && !args.iter().any(|(key, _)| matches!(key.as_str(), "set" | "install")),
        _ => false,
    }
}

fn frame_or_turn(wait: bool) -> WaitKind {
    if wait { WaitKind::Frame } else { WaitKind::Turn }
}

/// Edits answer after the frame that shows them. Verification can wait for a reload.
pub fn route_design(op: &str, p: &Params) -> Answer {
    if op.is_empty() || !op.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
        return Answer::Json("{\"err\":\"unknown design op\"}".to_string());
    }
    let wait = !design_reads(op, &p.0);
    let kind = match op {
        "preview" | "revert" | "verify" => WaitKind::Reload,
        _ => frame_or_turn(wait),
    };
    route_tweak_with("design", &format!("design_{op}"), p, wait, kind)
}

pub fn route_tweak(op: &str, p: &Params, wait: bool) -> Answer {
    route_tweak_with("tweak", op, p, wait, frame_or_turn(wait))
}

fn route_tweak_with(what: &'static str, op: &str, p: &Params, wait: bool, kind: WaitKind) -> Answer {
    let op = op.to_string();
    let args = p.0.clone();
    reply_to_answer(ask(what, kind, move |tx| Cmd::Tweak { op, args, wait, tx }))
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
            "{{\"i\":{},\"t\":{},\"title\":{},\"visible\":{},\"sz\":[{},{}],\"px\":[{},{}],\"dpi\":{},\"pass_dpi\":{},\"pos\":[{},{}],\"key\":{}}}",
            window.id,
            json_str(&window.title),
            json_str(application_title(&window.title)),
            window.visible,
            num(window.w),
            num(window.h),
            num((window.w * window.dpi).round()),
            num((window.h * window.dpi).round()),
            num(window.dpi),
            num(window.pass_dpi),
            num(window.x),
            num(window.y),
            window.key,
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
    match ask("status", WaitKind::Turn, Cmd::Activity) {
        Reply::Text(activity) => out.push_str(&format!(",\"activity\":{activity}}}")),
        other => return reply_to_answer(other),
    }
    Answer::Json(out)
}

pub fn route_mouse(p: &Params, force_kind: Option<&str>) -> Answer {
    note_injected_input();
    match mouse_inputs(p, force_kind) {
        Ok(inputs) => {
            let what = match force_kind { Some("click") => "click", Some("move") => "move", _ => "mouse" };
            send_input(what, p.window(), inputs, p.flag(&["wait"]))
        }
        Err(message) => err(&message),
    }
}

fn mouse_inputs(p: &Params, force_kind: Option<&str>) -> Result<Vec<Input>, String> {
    let kind = force_kind.or_else(|| p.get(&["k", "kind"])).unwrap_or("move");
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
    if p.flag(&["native"]) {
        if !matches!(kind, "click" | "tap") || button != 0 || hw || mods != Default::default() || time.is_some() {
            return Err("native=1 supports a primary click without hw, modifiers or a timestamp".into());
        }
        return Ok(vec![Input::NativeClick { x, y }]);
    }
    let inputs = match kind {
        "move" => vec![mouse(MouseKind::Move)],
        "down" => vec![mouse(MouseKind::Move), mouse(MouseKind::Down)],
        "up" => vec![mouse(MouseKind::Up)],
        "click" | "tap" => vec![
            mouse(MouseKind::Move),
            mouse(MouseKind::Down),
            mouse(MouseKind::Up),
        ],
        "dblclick" => vec![
            mouse(MouseKind::Move),
            mouse(MouseKind::Down),
            mouse(MouseKind::Up),
            mouse(MouseKind::Down),
            mouse(MouseKind::Up),
        ],
        // a whole drag in one request: down at x,y, `steps` moves to x1,y1 (and back with
        // back=1), up; a burst like a fast pointer, so agents reproduce drag hitches alone
        "drag" => {
            let x1 = p.f64(&["x1"], x);
            let y1 = p.f64(&["y1"], y);
            let steps = (p.f64(&["steps"], 60.0).max(1.0)) as usize;
            let back = p.flag(&["back"]);
            let at = |px: f64, py: f64, kind| Input::Mouse { kind, x: px, y: py, button, dx: 0.0, dy: 0.0, mods, hw, time };
            let mut inputs = vec![at(x, y, MouseKind::Move), at(x, y, MouseKind::Down)];
            let legs: &[(f64, f64, f64, f64)] = if back { &[(x, y, x1, y1), (x1, y1, x, y)] } else { &[(x, y, x1, y1)] };
            for &(ax, ay, bx, by) in legs {
                for i in 1..=steps {
                    let t = i as f64 / steps as f64;
                    inputs.push(at(ax + (bx - ax) * t, ay + (by - ay) * t, MouseKind::Move));
                }
            }
            let (ex, ey) = if back { (x, y) } else { (x1, y1) };
            inputs.push(at(ex, ey, MouseKind::Up));
            inputs
        }
        "scroll" | "wheel" => match p.get(&["phase"]) {
            None => vec![mouse(MouseKind::Scroll)],
            Some(name) => {
                let Some(phase) = pad_scroll_phase(name) else {
                    return Err(format!("bad scroll phase {name}"));
                };
                vec![Input::PadScroll { x, y, dx, dy, phase, mods, time }]
            }
        },
        "touch" => {
            let state = match p.get(&["phase"]).unwrap_or("move") {
                "start" => crate::event::TouchState::Start,
                "move" => crate::event::TouchState::Move,
                "stop" => crate::event::TouchState::Stop,
                "cancel" => crate::event::TouchState::Cancel,
                other => return Err(format!("bad touch phase {other}")),
            };
            vec![Input::Touch {
                x,
                y,
                uid: p.f64(&["id"], 1.0).max(0.0) as u64,
                state,
                mods,
                time,
            }]
        }
        "pinch" => {
            let phase = match p.get(&["phase"]).unwrap_or("update") {
                "begin" => PinchPhase::Begin,
                "update" => PinchPhase::Update,
                "end" => PinchPhase::End,
                other => return Err(format!("bad pinch phase {other}")),
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
        other => return Err(format!("bad kind {other}")),
    };
    Ok(inputs)
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
            "key", window,
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
    // A real keyboard delivers a printable key's character as text input after its key down
    // (that is what a TextInput inserts); an injected one must too, or `key c=Space` types
    // nothing. Not with ctrl/alt/logo held: those are shortcuts.
    let text = (!mods.control && !mods.alt && !mods.logo)
        .then(|| key_text(code, mods.shift))
        .flatten()
        .map(|c| Input::Text(c.to_string()));
    let inputs = match kind {
        "down" => [Input::Key {
            down: true,
            code,
            mods,
        }]
        .into_iter()
        .chain(text)
        .collect(),
        "up" => vec![Input::Key {
            down: false,
            code,
            mods,
        }],
        "press" | "tap" => [Input::Key {
            down: true,
            code,
            mods,
        }]
        .into_iter()
        .chain(text)
        .chain([Input::Key {
            down: false,
            code,
            mods,
        }])
        .collect(),
        other => return err(&format!("bad kind {other}")),
    };
    send_input("key", window, inputs, p.flag(&["wait"]))
}

/// The character a key types on a US layout (letters, digits, space), as the OS sends it
/// with the key down. Shifted digits and punctuation depend on the layout: none.
fn key_text(code: KeyCode, shift: bool) -> Option<char> {
    const LETTERS: [(KeyCode, char); 26] = [
        (KeyCode::KeyA, 'a'), (KeyCode::KeyB, 'b'), (KeyCode::KeyC, 'c'), (KeyCode::KeyD, 'd'),
        (KeyCode::KeyE, 'e'), (KeyCode::KeyF, 'f'), (KeyCode::KeyG, 'g'), (KeyCode::KeyH, 'h'),
        (KeyCode::KeyI, 'i'), (KeyCode::KeyJ, 'j'), (KeyCode::KeyK, 'k'), (KeyCode::KeyL, 'l'),
        (KeyCode::KeyM, 'm'), (KeyCode::KeyN, 'n'), (KeyCode::KeyO, 'o'), (KeyCode::KeyP, 'p'),
        (KeyCode::KeyQ, 'q'), (KeyCode::KeyR, 'r'), (KeyCode::KeyS, 's'), (KeyCode::KeyT, 't'),
        (KeyCode::KeyU, 'u'), (KeyCode::KeyV, 'v'), (KeyCode::KeyW, 'w'), (KeyCode::KeyX, 'x'),
        (KeyCode::KeyY, 'y'), (KeyCode::KeyZ, 'z'),
    ];
    const DIGITS: [KeyCode; 10] = [
        KeyCode::Key0, KeyCode::Key1, KeyCode::Key2, KeyCode::Key3, KeyCode::Key4,
        KeyCode::Key5, KeyCode::Key6, KeyCode::Key7, KeyCode::Key8, KeyCode::Key9,
    ];
    if code == KeyCode::Space {
        return Some(' ');
    }
    if let Some(&(_, c)) = LETTERS.iter().find(|(k, _)| *k == code) {
        return Some(if shift { c.to_ascii_uppercase() } else { c });
    }
    let digit = DIGITS.iter().position(|k| *k == code)?;
    (!shift).then(|| char::from(b'0' + digit as u8))
}

pub fn route_text(p: &Params) -> Answer {
    note_injected_input();
    let Some(text) = p.get(&["t", "text"]) else {
        return err("need t=");
    };
    send_input(
        "text", p.window(),
        vec![Input::Text(text.to_string())],
        p.flag(&["wait"]),
    )
}

/// Injected pointer moves follow the hardware rule (HAL intake): a move directly followed by
/// another move with the same buttons and modifiers is dropped and its delta carried on.
pub fn coalesce_moves(inputs: Vec<Input>) -> Vec<Input> {
    let mut out: Vec<Input> = Vec::with_capacity(inputs.len());
    for input in inputs {
        if let Input::Mouse { kind: MouseKind::Move, button, mods, hw, dx, dy, .. } = &input {
            if let Some(Input::Mouse { kind: MouseKind::Move, button: b, mods: m, hw: h, dx: pdx, dy: pdy, .. }) = out.last() {
                if b == button && m == mods && h == hw {
                    let (cx, cy) = (pdx + dx, pdy + dy);
                    out.pop();
                    let mut input = input;
                    if let Input::Mouse { dx, dy, .. } = &mut input {
                        *dx = cx;
                        *dy = cy;
                    }
                    out.push(input);
                    continue;
                }
            }
        }
        out.push(input);
    }
    out
}

pub fn send_input(what: &'static str, window: Option<usize>, inputs: Vec<Input>, wait: bool) -> Answer {
    reply_to_answer(ask(what, frame_or_turn(wait),
        move |tx| Cmd::Input {
            window,
            inputs,
            wait,
            tx,
        },
    ))
}

/// The file commands (`drop`, `drag`, `drag_end`): what each takes.
#[derive(Clone, Copy, PartialEq)]
pub enum FileInput {
    Drop,
    Drag,
    DragEnd,
}

impl FileInput {
    fn word(self) -> &'static str {
        match self {
            FileInput::Drop => "drop",
            FileInput::Drag => "drag",
            FileInput::DragEnd => "drag_end",
        }
    }
}

/// A file command's window, input and wait. Unlike permissive mouse aliases, file input must
/// not silently ignore a typo or choose between duplicate parameters.
pub fn parse_file_input(p: &Params, what: FileInput) -> Result<(Option<usize>, Input, bool), String> {
    let word = what.word();
    let mut seen = Vec::new();
    for (key, _) in &p.0 {
        let key = match key.as_str() {
            "path" | "x" | "y" if what != FileInput::DragEnd => key.as_str(),
            "wait" | "if_user_seq" => key.as_str(),
            "w" | "window" => "w",
            _ => return Err(format!("unknown {word} parameter")),
        };
        if seen.contains(&key) {
            return Err(format!("duplicate {word} parameter"));
        }
        seen.push(key);
    }
    let window = p
        .get(&["w", "window"])
        .map(|value| {
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(format!("{word} window must be a nonnegative integer"));
            }
            value
                .parse::<usize>()
                .map_err(|_| format!("{word} window is out of range"))
        })
        .transpose()?;
    let wait = match p.get(&["wait"]) {
        None | Some("0" | "false") => false,
        Some("1" | "true") => true,
        _ => return Err(format!("{word} wait must be 0 or 1")),
    };
    if what == FileInput::DragEnd {
        return Ok((window, Input::DragFileEnd, wait));
    }
    let path = p.get(&["path"]).ok_or_else(|| format!("{word} requires path="))?;
    if path.is_empty()
        || path.len() > 4096
        || path.chars().any(char::is_control)
        || !std::path::Path::new(path).is_absolute()
    {
        return Err(format!(
            "{word} path must be absolute, at most 4096 bytes, with no control characters"
        ));
    }
    let coordinate = |key| {
        p.get(&[key])
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| format!("{word} requires finite, nonnegative x= and y="))
    };
    let (path, x, y) = (path.to_string(), coordinate("x")?, coordinate("y")?);
    let input = match what {
        FileInput::Drag => Input::DragFile { path, x, y },
        _ => Input::DropFile { path, x, y },
    };
    Ok((window, input, wait))
}

pub fn route_file_input(p: &Params, what: FileInput) -> Answer {
    match parse_file_input(p, what) {
        Ok((window, input, wait)) => send_input(what.word(), window, vec![input], wait),
        Err(message) => Answer::Json(format!("{{\"err\":{}}}", json_str(&message))),
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
    pub drawn: Option<Drawn>,
    pub _bytes: Option<GrabBytes>,
}

trait GrabSubmit: Clone + Send + Sync + 'static {
    fn ask_grab(&self, make: impl FnOnce(Sender<Reply>) -> Cmd) -> Reply;
    fn receive_grab(&self, rx: &Receiver<Result<Grabbed, String>>, due: Instant,
        encoding: &AtomicBool, gone: &dyn Fn() -> bool) -> Result<Grabbed, String>;
    fn cancel_grabs(&self, ids: Vec<u64>);
}

#[derive(Clone, Copy)]
struct UiQueue;

impl GrabSubmit for UiQueue {
    fn ask_grab(&self, make: impl FnOnce(Sender<Reply>) -> Cmd) -> Reply {
        ask("grab", WaitKind::Turn, make)
    }

    fn receive_grab(&self, rx: &Receiver<Result<Grabbed, String>>, due: Instant,
        encoding: &AtomicBool, gone: &dyn Fn() -> bool) -> Result<Grabbed, String>
    {
        let live = CommandLive::detached();
        live.set_stage(CmdStage::Waiting);
        let not_before_ms = due.checked_duration_since(Instant::now()).map(|d| d.as_millis() as u64);
        wait_for_reply("grab", WaitKind::Grab { not_before_ms }, rx, &live, &GLOBAL_UI_PROGRESS,
            WatchConfig::default(), &mut RealProgressProbe::new(), Some(encoding), gone, wake)
            .map_err(|e| e.message())?
    }

    fn cancel_grabs(&self, ids: Vec<u64>) {
        queue().lock().unwrap().push(QueuedCmd {
            cmd: Cmd::CancelGrabs(ids),
            user_seq: None,
            live: CommandLive::detached(),
            by: String::new(),
        });
        wake();
    }
}

/// One armed grab, its scheduled time and its handoff to the encoder.
struct ArmedGrab {
    id: u64,
    rx: Receiver<Result<Grabbed, String>>,
    due: Instant,
    encoding: Arc<AtomicBool>,
}

/// The grabs one request has armed and not yet received, in cadence order: at most
/// [`GRAB_SERIES_WINDOW`] for a fixed series, whatever its `n`. Dropping it cancels the rest.
struct GrabSeries<S: GrabSubmit> {
    series: u64,
    window: Option<usize>,
    scale: f64,
    raw_output: bool,
    cancelled: Arc<AtomicBool>,
    in_flight: std::collections::VecDeque<ArmedGrab>,
    submit: S,
}

impl<S: GrabSubmit> Drop for GrabSeries<S> {
    fn drop(&mut self) {
        // also ends the UI thread's batch of this series
        self.cancelled.store(true, Ordering::Relaxed);
        if self.in_flight.is_empty() {
            return;
        }
        let ids: Vec<u64> = self.in_flight.iter().map(|grab| grab.id).collect();
        let mut sinks = grab_sinks().lock().unwrap();
        for id in &ids {
            sinks.remove(id);
        }
        drop(sinks);
        self.submit.cancel_grabs(ids);
    }
}

/// When frame `index` of a series that started at `start` is due (no overflow at any `n`).
fn series_due(start: Instant, every: Duration, index: usize) -> Instant {
    const FAR: u64 = u32::MAX as u64 * 1_000_000_000;
    let nanos = every.as_nanos().saturating_mul(index as u128).min(FAR as u128) as u64;
    start.checked_add(Duration::from_nanos(nanos)).unwrap_or(start)
}

/// The window a grab of `window` (None: the first) goes to.
fn resolve_grab_window(window: Option<usize>) -> Result<Option<usize>, String> {
    let window = window.or_else(|| status_cell().lock().unwrap().windows.first().map(|w| w.id)).ok_or("no windows")?;
    Ok(Some(window))
}

const GRAB_SLOT_TIMEOUT: &str = "grab timeout waiting for a slot in the grab window (other grabs did not finish)";

impl<S: GrabSubmit> GrabSeries<S> {
    fn new(series: u64, window: Option<usize>, scale: f64, raw_output: bool, submit: S) -> Self {
        Self {
            series, window, scale, raw_output, cancelled: Arc::new(AtomicBool::new(false)),
            in_flight: Default::default(), submit,
        }
    }

    /// Arms one grab per permit, frames `first_index..` of the cadence, in one `Cmd::Grab`.
    fn arm_chunk(&mut self, permits: Vec<GrabPermit>, series_start: Instant, every: Duration, first_index: usize) -> Result<(), String> {
        let mut grabs = Vec::with_capacity(permits.len());
        {
            let mut sinks = grab_sinks().lock().unwrap();
            for (offset, permit) in permits.into_iter().enumerate() {
                let id = GRAB_ID_BASE + NEXT_ID.fetch_add(1, Ordering::Relaxed);
                let due = series_due(series_start, every, first_index + offset);
                let (tx, rx) = sync_channel(1);
                let encoding = Arc::new(AtomicBool::new(false));
                sinks.insert(
                    id,
                    GrabSink {
                        id,
                        window: self.window,
                        requested_at: due,
                        encoding: encoding.clone(),
                        scale: self.scale,
                        raw_output: self.raw_output,
                        cancelled: self.cancelled.clone(),
                        tx,
                        _permit: permit,
                    },
                );
                self.in_flight.push_back(ArmedGrab { id, rx, due, encoding });
                grabs.push((id, due));
            }
        }
        match self.submit.ask_grab(|tx| Cmd::Grab {
            window: self.window,
            series: self.series,
            grabs,
            cancelled: self.cancelled.clone(),
            tx,
        }) {
            Reply::Ok => Ok(()),
            Reply::Err(msg) => Err(msg),
            Reply::Text(_) | Reply::Conflict(_) => Err("unexpected grab reply".into()),
        }
    }

    /// Remove the oldest grab only after delivery. On error, Drop cancels it with the rest.
    fn receive_next(&mut self, gone: &dyn Fn() -> bool) -> Result<Grabbed, String> {
        let front = self.in_flight.front().ok_or("no grab armed")?;
        let grabbed = self.submit.receive_grab(&front.rx, front.due, &front.encoding, gone)?;
        self.in_flight.pop_front();
        Ok(grabbed)
    }
}

/// Iterated 2x2 box filtering in memory order. Each level holds one unpaired row at
/// its own width, so even the deepest reduction needs only O(width) scratch.
struct RowHalver {
    width: usize,
    halvings: u32,
    levels: Vec<Vec<u8>>,
    have: Vec<bool>,
    tmp: Vec<u8>,
    tmp2: Vec<u8>,
}

impl RowHalver {
    fn new(width: usize, halvings: u32) -> Self {
        debug_assert!(halvings >= 1 && width >> halvings >= 1);
        let levels = (1..=halvings).map(|n| vec![0; (width >> n) * 4]).collect();
        Self {
            width,
            halvings,
            levels,
            have: vec![false; halvings as usize],
            tmp: vec![0; (width / 2) * 4],
            tmp2: vec![0; (width / 2) * 4],
        }
    }

    fn row(&mut self, pixels: &[u8], stride: usize, mem_y: usize) -> &[u8] {
        let base = mem_y << self.halvings;
        for pair in 0..(1 << (self.halvings - 1)) {
            let start = (base + 2 * pair) * stride;
            let r0 = &pixels[start..];
            let r1 = &pixels[start + stride..];
            if !self.have[0] {
                halve_row_pair(r0, r1, self.width, &mut self.levels[0]);
                self.have[0] = true;
                continue;
            }
            halve_row_pair(r0, r1, self.width, &mut self.tmp);
            // Carry the new row up through its waiting siblings until a level is free.
            for lv in 1..self.halvings as usize {
                self.have[lv - 1] = false;
                let (lo, hi) = self.levels.split_at_mut(lv);
                let w = self.width >> lv;
                if !self.have[lv] {
                    halve_row_pair(&lo[lv - 1], &self.tmp, w, &mut hi[0]);
                    self.have[lv] = true;
                    break;
                }
                halve_row_pair(&lo[lv - 1], &self.tmp, w, &mut self.tmp2);
                std::mem::swap(&mut self.tmp, &mut self.tmp2);
            }
        }
        // A complete group of source rows leaves exactly one row at the last level.
        let last = self.halvings as usize - 1;
        self.have[last] = false;
        &self.levels[last]
    }
}

/// Point-sample and convert a source row into tightly packed RGBA.
fn write_target_row(src: &[u8], w: usize, target_w: usize, order: GrabOrder, dst: &mut [u8]) {
    if target_w == w {
        match order {
            GrabOrder::Rgba => dst.copy_from_slice(&src[..w * 4]),
            GrabOrder::Bgra => {
                for (pixel, out) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                    out.copy_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                }
            }
        }
        return;
    }
    for (x, out) in dst.chunks_exact_mut(4).enumerate() {
        let src_x = (x as u64 * w as u64 / target_w as u64) as usize * 4;
        out.copy_from_slice(&src[src_x..src_x + 4]);
        if order == GrabOrder::Bgra {
            out.swap(0, 2);
        }
    }
}

pub fn encode_grab(job: &EncodeJob) -> Result<(u32, u32, Vec<u8>), String> {
    use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
    use makepad_zune_png::PngDecoder;

    let width = job.width as usize;
    let height = job.height as usize;
    if width == 0 || height == 0 || job.pixels.is_empty() {
        return Err("grab backend returned no pixels".into());
    }
    // Every 2x of downscale is a 2x2 box filter (`halve_pixels`), the rest a point sample. The
    // queue may have taken some of those halvings already (`halve_grab`, over the pixel budget);
    // this takes the ones left, so a frame comes out the same however deep the queue was. A
    // gseq at scale=0.5 point-sampled its first frames and box-filtered the ones behind a
    // backlog: its unchanged text flipped between two looks, frame to frame (2026-11-17).
    let (mut halvings, mut scale, mut w, mut h) = (0, job.sink.scale, width, height);
    while scale <= 0.5 && w >= 2 && h >= 2 {
        (halvings, scale, w, h) = (halvings + 1, (scale * 2.0).min(1.0), w / 2, h / 2);
    }
    let scaled_dim = |n: usize| -> Result<usize, String> {
        let v = (n as f64 * scale).round();
        if !v.is_finite() || v > i32::MAX as f64 {
            return Err("scaled grab dimension exceeds PNG/u32 range".into());
        }
        Ok(v.max(1.0) as usize)
    };
    let target_w = scaled_dim(w)?;
    let target_h = scaled_dim(h)?;
    let row_bytes = target_w.checked_mul(4).ok_or("scaled grab row size overflow")?;
    let target_bytes = row_bytes.checked_mul(target_h).ok_or("scaled grab size overflow")?;
    if job.backend_png && !job.sink.raw_output && target_w == width && target_h == height {
        return Ok((job.width, job.height, job.pixels.to_vec()));
    }
    let source_row_bytes = width.checked_mul(4).ok_or("grab readback row size overflow")?;
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
            source_row_bytes,
            GrabOrder::Rgba,
            GrabOrigin::TopLeft,
        )
    } else {
        (&job.pixels[..], job.stride, job.order, job.origin)
    };
    if stride < source_row_bytes
        || stride
            .checked_mul(height)
            .is_none_or(|len| len > pixels.len())
    {
        return Err("grab readback has invalid dimensions or stride".into());
    }
    let mut halver = (halvings > 0).then(|| RowHalver::new(width, halvings));
    let mut out = Vec::new();
    let mut row_buf = Vec::new();
    let mut encoder = if job.sink.raw_output {
        out.try_reserve_exact(target_bytes)
            .map_err(|err| format!("cannot allocate {target_bytes} bytes for raw grab: {err}"))?;
        out.resize(target_bytes, 0);
        None
    } else {
        if target_w != w || order != GrabOrder::Rgba {
            row_buf.resize(row_bytes, 0);
        }
        Some(rgba_png_encoder(target_w, target_h)?)
    };
    for y in 0..target_h {
        let src_y = (y as u64 * h as u64 / target_h as u64) as usize;
        let mem_y = if origin == GrabOrigin::BottomLeft { h - 1 - src_y } else { src_y };
        let src_row = match &mut halver {
            Some(halver) => halver.row(pixels, stride, mem_y),
            None => &pixels[mem_y * stride..mem_y * stride + source_row_bytes],
        };
        if let Some(encoder) = &mut encoder {
            let row = if row_buf.is_empty() {
                src_row
            } else {
                write_target_row(src_row, w, target_w, order, &mut row_buf);
                &row_buf
            };
            encoder.write_rows(row).map_err(png_encode_error)?;
        } else {
            write_target_row(src_row, w, target_w, order, &mut out[y * row_bytes..(y + 1) * row_bytes]);
        }
    }
    let bytes = match encoder {
        Some(encoder) => encoder.finish().map_err(png_encode_error)?.1,
        None => out,
    };
    Ok((target_w as u32, target_h as u32, bytes))
}

// ------------------------------------------------------------------
// the `frames` service
// ------------------------------------------------------------------

/// What an OPEN of the `frames` service asked for.
#[derive(Debug, PartialEq)]
pub struct FramesArgs {
    pub window: Option<usize>,
    pub scale: f64,
    pub raw: bool,
    /// a fixed number of frames; none = until the client closes the channel
    pub n: Option<usize>,
    /// the cadence of a series, as the gap between presents
    pub every: Duration,
    /// `cap=1`: subscribe to presented frames.
    pub capture: bool,
    /// `codec=rle`: run-length coded RGBA pixels (cheap on the device, small for flat UI)
    pub rle: bool,
    pub video: Option<StreamVideoCodec>,
    pub full_chroma: bool,
    pub kbps: u32,
    pub max_qp: Option<u32>,
}

fn capture_interval(fps: f64) -> Result<Duration, String> {
    if fps > 0.0 && fps <= 240.0 {
        if let Ok(every) = Duration::try_from_secs_f64(1.0 / fps) {
            return Ok(every);
        }
    }
    Err("frames: cap=1 requires a positive fps up to 240".into())
}

fn bitrate(value: &str) -> Result<u32, String> {
    value.parse::<u32>().ok().filter(|&n| n > 0)
        .ok_or_else(|| "frames: kbps must be a positive integer".into())
}

pub fn parse_frames_args(args: &str) -> Result<FramesArgs, String> {
    let (mut window, mut scale, mut raw, mut n, mut fps, mut capture, mut rle) = (None, 1.0f64, false, None, None, false, false);
    let (mut video, mut kbps, mut max_qp) = (None, None, None);
    let mut chroma = None;
    for word in args.split_whitespace() {
        let (key, value) = word.split_once('=').ok_or_else(|| format!("frames: bad argument `{word}`"))?;
        let bad = |what: &str| format!("frames: {key}= must be {what}");
        match key {
            "window" | "w" => window = Some(value.parse::<usize>().map_err(|_| bad("a window id"))?),
            "scale" => scale = value.parse::<f64>().map_err(|_| bad("a number"))?,
            "codec" => {
                raw = value == "raw";
                rle = value == "rle";
                video = match value {
                    "png" | "raw" | "rle" => None,
                    "h264" => Some(StreamVideoCodec::H264),
                    "hevc" => Some(StreamVideoCodec::Hevc),
                    _ => return Err(bad("png, raw, rle, h264 or hevc")),
                };
            }
            "chroma" => chroma = Some(match value {
                "420" => false,
                "444" => true,
                _ => return Err(bad("420 or 444")),
            }),
            "kbps" => kbps = Some(bitrate(value)?),
            "maxqp" => max_qp = Some(value.parse::<u32>().map_err(|_| bad("an unsigned integer"))?),
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
    if video.is_some() && !(capture && scale.is_finite() && scale > 0.0) {
        return Err("frames: video requires cap=1 and a positive finite scale".into());
    }
    if video.is_none() && (kbps.is_some() || max_qp.is_some() || chroma.is_some()) {
        return Err("frames: kbps, maxqp and chroma require a video codec".into());
    }
    let full_chroma = chroma.unwrap_or(false);
    let kbps = kbps.unwrap_or_else(|| VideoStreamEncoderOptions::default().bitrate_kbps);
    if capture {
        let every = capture_interval(fps.unwrap_or(30.0))?;
        return Ok(FramesArgs { window, scale, raw, n: None, every, capture, rle, video, kbps, max_qp, full_chroma });
    }
    if n == Some(0) {
        return Err("frames n must be a positive integer".into());
    }
    // a series is paced by the gap between presents; a single frame has none
    let every_ms = match (n, fps) {
        (Some(1), _) => 0,
        (_, Some(fps)) if fps.is_finite() && fps > 0.0 => (1000.0 / fps).round() as u64,
        (_, Some(_)) => return Err("frames fps must be positive".into()),
        (Some(_), None) => 50,
        (None, None) => 100,
    };
    // any number of frames over any span: the series is paced through the grab window
    if n != Some(1) && !(8..=60_000).contains(&every_ms) {
        return Err("frames: the gap between frames must be 8..=60000 ms".into());
    }
    Ok(FramesArgs { window, scale, raw, n, every: Duration::from_millis(every_ms), capture, rle: false, video, kbps, max_qp, full_chroma })
}

/// The header of one `frames` record (see `makepad_remote::frames`).
pub fn frame_header(seq: u64, g: &Grabbed, raw: bool) -> String {
    let drawn = g.drawn.map_or(String::new(), |d| format!(",\"frame\":{},\"patch\":{}", d.frame, d.patch));
    format!(
        "{{\"seq\":{seq},\"w\":{},\"h\":{},\"window\":{},\"t_ns\":{},\"codec\":\"{}\",\"capture_ms\":{:.3},\"encode_ms\":{:.3},\"capture_kind\":{}{drawn}}}",
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
    /// Returns a video record to its producer if it must pause capture and retry.
    pub try_send: Box<dyn Fn(Vec<u8>) -> Result<(), makepad_remote::conn::SendError> + Send>,
    /// Ends the stream (the first call counts).
    pub close: Box<dyn Fn(u16, &str) + Send>,
    pub try_recv: Box<dyn Fn() -> Result<makepad_remote::Event, bool> + Send>,
}

impl FramesOut {
    /// Drain client DATA in wire order before the next frame. A disconnect also ends the stream.
    fn receive(&self, mut data: impl FnMut(&[u8]) -> Result<(), String>) -> Result<bool, String> {
        use makepad_remote::Event;
        loop {
            match (self.try_recv)() {
                Ok(Event::Data(bytes)) => data(&bytes)?,
                Ok(Event::Opened(_)) => {}
                Ok(Event::Closed(..)) | Err(false) => return Ok(false),
                Err(true) => return Ok(true),
            }
        }
    }
}

/// Produces the frames `args` asks for into `out`, then ends the stream. Grabs go through the UI
/// thread (the same readback and encode worker the bridge always had); a fixed series is armed
/// with its cadence through a sliding window ([`GRAB_SERIES_WINDOW`] ahead of delivery, refilled
/// as frames arrive), so encoding never retimes the presents and any `n` fits; an open-ended one
/// (no `n`) is latest-wins. A full grab window waits for a slot, never refuses.
#[cfg(relax)]
pub fn run_frames(args: FramesArgs, out: FramesOut) {
    run_frames_with(args, out, UiQueue);
}

fn run_frames_with<S: GrabSubmit>(args: FramesArgs, out: FramesOut, submit: S) {
    use makepad_remote::conn::{CLOSE_NORMAL, CLOSE_REFUSED};
    let fail = |why: String| (out.close)(CLOSE_REFUSED, &why);
    if args.capture {
        return run_capture_stream(args, out, submit);
    }
    // Start the long-lived worker on this thread, never from the UI or a GPU callback.
    if let Err(why) = start_encoder() {
        return fail(why);
    }
    if !args.scale.is_finite() || args.scale <= 0.0 {
        return fail("grab scale must be positive and finite".into());
    }
    let window = match resolve_grab_window(args.window) {
        Ok(window) => window,
        Err(why) => return fail(why),
    };
    let gone = || out.receive(|_| Ok(())) != Ok(true);
    match args.n {
        Some(n) => {
            let series_id = if n > 1 { NEXT_ID.fetch_add(1, Ordering::Relaxed) } else { 0 };
            let mut series = GrabSeries::new(series_id, window, args.scale, args.raw, submit);
            let series_start = Instant::now();
            let mut armed = 0usize;
            for index in 0..n {
                if gone() {
                    return;
                }
                if armed < n && series.in_flight.len() <= GRAB_SERIES_REFILL_AT {
                    let mut permits = Vec::new();
                    if series.in_flight.is_empty() {
                        // a slow cadence holds no slot until its next frame is near
                        loop {
                            let lead = series_due(series_start, args.every, armed).saturating_duration_since(Instant::now());
                            if lead <= GRAB_ARM_LEAD {
                                break;
                            }
                            std::thread::sleep((lead - GRAB_ARM_LEAD).min(Duration::from_millis(200)));
                            if gone() {
                                return;
                            }
                        }
                        let due = series_due(series_start, args.every, armed);
                        match acquire_grab_slot(due.max(Instant::now()) + GRAB_SLOT_WAIT, &gone) {
                            Ok(permit) => permits.push(permit),
                            Err(AcquireError::Cancelled) => return,
                            Err(AcquireError::TimedOut) => return fail(format!("frame {index}: {GRAB_SLOT_TIMEOUT}")),
                        }
                    }
                    // the rest of the window, as far as slots are free now: a full global window
                    // never keeps this series from draining what it has in flight
                    let want = (n - armed).min(GRAB_SERIES_WINDOW - series.in_flight.len());
                    while permits.len() < want
                        && series_due(series_start, args.every, armed + permits.len()) <= Instant::now() + GRAB_LOOKAHEAD
                    {
                        match try_acquire_grab_slot() {
                            Some(permit) => permits.push(permit),
                            None => break,
                        }
                    }
                    if !permits.is_empty() {
                        let count = permits.len();
                        if let Err(why) = series.arm_chunk(permits, series_start, args.every, armed) {
                            return fail(format!("frame {index}: {why}"));
                        }
                        armed += count;
                    }
                }
                let grabbed = match series.receive_next(&gone) {
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
                if gone() {
                    return;
                }
                let permit = match acquire_grab_slot(Instant::now() + GRAB_SLOT_WAIT, &gone) {
                    Ok(permit) => permit,
                    Err(AcquireError::Cancelled) => return,
                    Err(AcquireError::TimedOut) => return fail(GRAB_SLOT_TIMEOUT.into()),
                };
                let mut grab = GrabSeries::new(0, window, args.scale, args.raw, submit.clone());
                if let Err(why) = grab.arm_chunk(vec![permit], Instant::now(), Duration::ZERO, 0) {
                    return fail(why);
                }
                let grabbed = match grab.receive_next(&gone) {
                    Ok(g) => g,
                    Err(why) => return fail(why),
                };
                drop(grab);
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

/// Subscribe to presented frames. Image records use wall-clock slots so a receiver can count
/// missing images. Video records number encoded units consecutively and carry their encoder PTS.
fn run_capture_stream<S: GrabSubmit>(args: FramesArgs, out: FramesOut, submit: S) {
    use makepad_remote::conn::{CLOSE_NORMAL, CLOSE_REFUSED};
    use std::sync::mpsc::{sync_channel, TrySendError};
    if let Some(codec) = args.video {
        match run_video_capture(args, codec, &out, submit) {
            Ok(()) => (out.close)(CLOSE_NORMAL, ""),
            Err(why) => (out.close)(CLOSE_REFUSED, &why),
        }
        return;
    }
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
        if out.receive(|_| Ok(())) != Ok(true) { break; }
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

fn video_time_ns() -> u64 {
    (crate::Cx::monotonic_now() * 1_000_000_000.0) as u64
}

struct VideoFrame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    time_ns: u64,
}

/// One waiting RGBA frame. Removing the subscription also stops its render-side readbacks.
struct VideoInput {
    id: u64,
    frames: Receiver<VideoFrame>,
    occupied: Arc<AtomicBool>,
}

impl VideoInput {
    fn new(args: &FramesArgs, skipped: Arc<AtomicU64>) -> Self {
        let (tx, frames) = sync_channel(1);
        let occupied = Arc::new(AtomicBool::new(false));
        let slot = occupied.clone();
        let id = crate::screen_capture::add_screen_capture(
            crate::screen_capture::ScreenCaptureOptions {
                window_id: args.window,
                max_fps: 1.0 / args.every.as_secs_f64(),
            },
            move |frame| {
                if slot.swap(true, Ordering::AcqRel) {
                    skipped.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                let _ = tx.try_send(VideoFrame {
                    width: frame.width, height: frame.height, time_ns: frame.time_ns,
                    rgba: frame.rgba.to_vec(),
                });
            },
        );
        Self { id, frames, occupied }
    }

    fn next(&self, wait: Duration) -> Option<VideoFrame> {
        let frame = self.frames.recv_timeout(wait).ok()?;
        self.occupied.store(false, Ordering::Release);
        Some(frame)
    }
}

impl Drop for VideoInput {
    fn drop(&mut self) {
        crate::screen_capture::remove_screen_capture(self.id);
    }
}

const SCALE_WEIGHT: u32 = 1 << 14;

#[derive(Default)]
struct ScaleScratch {
    cached_w: (u32, u32),
    h_plan: HorizontalPlan,
    vrow: Vec<u16>, // One 8.7 row plus a zero RGBA pixel for fixed-width reads.
    vsum16: Vec<u16>,
    vsum32: Vec<u32>,
}

#[derive(Default)]
enum HorizontalPlan {
    #[default]
    Identity,
    TwoTap(Vec<HSpan2>),
    ThreeTap(Vec<HSpan3>),
    BoxSpan { w_full: u32, m_lo: u32, exact_ratio: bool, spans: Vec<HSpanBox> },
}

#[derive(Clone, Copy)]
struct HSpan2 { x0: u32, w0: u16, w1: u16 }
#[derive(Clone, Copy)]
#[repr(C)]
struct HSpan3 {
    x0: u32,
    w: [u16; 4], // Three weights summing to SCALE_WEIGHT and a zero SIMD lane.
}
#[derive(Clone, Copy)]
struct HSpanBox { x0: u32, inner_len: u32, wf: u32, wl: u32 }

// Coverage in units of 1/dst of a source pixel. The interior pixels each cover dst units.
struct BoxCoverage { first: u32, inner_len: u32, first_cov: u32, last_cov: u32 }

fn box_coverage(src: u32, dst: u32, pos: u32) -> BoxCoverage {
    let dst = u64::from(dst);
    let start = u64::from(pos) * u64::from(src);
    let end = start + u64::from(src);
    let first = start / dst;
    let last = (end - 1) / dst;
    BoxCoverage {
        first: first as u32,
        inner_len: (last - first - 1) as u32,
        first_cov: ((first + 1) * dst - start) as u32,
        last_cov: (end - last * dst) as u32,
    }
}

impl BoxCoverage {
    fn weights(&self, unit: u32, full: u32) -> (u32, u32) {
        let remaining = unit - self.inner_len * full;
        let edges = u64::from(self.first_cov) + u64::from(self.last_cov);
        let first = ((u64::from(remaining) * u64::from(self.first_cov) + edges / 2) / edges) as u32;
        (first, remaining - first)
    }
}

fn bilinear_span(src: u32, dst: u32, pos: u32) -> HSpan2 {
    // Map destination centers into source space, clamping at both ends.
    let edge = u64::from(pos) * u64::from(src);
    let dst = u64::from(dst);
    let index = edge / dst;
    let center = 2 * (edge % dst) + u64::from(src);
    let (first, fraction) = if center < dst {
        if index == 0 { (0, 0) } else { (index - 1, center + dst) }
    } else {
        (index, center - dst)
    };
    let right = if first + 1 == u64::from(src) { 0 } else {
        ((fraction * u64::from(SCALE_WEIGHT) + dst) / (2 * dst)) as u16
    };
    HSpan2 { x0: first as u32, w0: SCALE_WEIGHT as u16 - right, w1: right }
}

impl HorizontalPlan {
    fn new(src: u32, dst: u32) -> Self {
        if src == dst { return Self::Identity; }
        if src < dst {
            return Self::TwoTap((0..dst).map(|x| bilinear_span(src, dst, x)).collect());
        }
        if u64::from(src) == 2 * u64::from(dst) {
            return Self::TwoTap((0..dst).map(|x| HSpan2 { x0: 2 * x, w0: 8192, w1: 8192 }).collect());
        }
        if u64::from(src) < 2 * u64::from(dst) {
            let full = ((u64::from(dst) << 14) / u64::from(src)) as u32;
            return Self::ThreeTap((0..dst).map(|x| {
                let span = box_coverage(src, dst, x);
                let (first, last) = span.weights(SCALE_WEIGHT, full);
                let (w1, w2) = if span.inner_len == 0 { (last, 0) } else { (full, last) };
                debug_assert!(span.first <= src - 2 && span.inner_len <= 1);
                debug_assert_eq!(first + w1 + w2, SCALE_WEIGHT);
                HSpan3 { x0: span.first, w: [first as u16, w1 as u16, w2 as u16, 0] }
            }).collect());
        }
        let exact_ratio = u64::from(src) > 64 * u64::from(dst);
        let w_full = if exact_ratio { dst } else { ((u64::from(dst) << 16) / u64::from(src)) as u32 };
        let m_lo = (src - 1) / dst - 1;
        debug_assert!(m_lo >= 1);
        let spans = (0..dst).map(|x| {
            let span = box_coverage(src, dst, x);
            let (wf, wl) = if exact_ratio { (span.first_cov, span.last_cov) } else { span.weights(1 << 16, w_full) };
            debug_assert!((m_lo..=m_lo + 1).contains(&span.inner_len));
            debug_assert!(u64::from(span.first) + u64::from(span.inner_len) + 1 < u64::from(src));
            if !exact_ratio {
                debug_assert!((1024..=32767).contains(&w_full));
                debug_assert!((1..=63).contains(&span.inner_len));
                debug_assert!(wf + wl <= 64512);
                debug_assert_eq!(wf + span.inner_len * w_full + wl, 1 << 16);
            }
            HSpanBox { x0: span.first, inner_len: span.inner_len, wf, wl }
        }).collect();
        Self::BoxSpan { w_full, m_lo, exact_ratio, spans }
    }
}

fn v_blend_2(vrow: &mut [u16], r0: &[u8], r1: &[u8], w0: u16, w1: u16) {
    let (w0, w1) = (u32::from(w0), u32::from(w1));
    for ((v, &a), &b) in vrow.iter_mut().zip(r0).zip(r1) {
        *v = ((u32::from(a) * w0 + u32::from(b) * w1 + 64) >> 7) as u16;
    }
}

fn v_blend_3(vrow: &mut [u16], r0: &[u8], r1: &[u8], r2: &[u8], w0: u16, w1: u16, w2: u16) {
    let (w0, w1, w2) = (u32::from(w0), u32::from(w1), u32::from(w2));
    for (((v, &a), &b), &c) in vrow.iter_mut().zip(r0).zip(r1).zip(r2) {
        *v = ((u32::from(a) * w0 + u32::from(b) * w1 + u32::from(c) * w2 + 64) >> 7) as u16;
    }
}

fn v_box_4(vrow: &mut [u16], rows: [&[u8]; 4], wf: u32, full: u32, wl: u32) {
    let [r0, r1, r2, r3] = rows;
    for ((((v, &a), &b), &c), &d) in vrow.iter_mut().zip(r0).zip(r1).zip(r2).zip(r3) {
        let mid = u32::from(b) + u32::from(c);
        *v = ((u32::from(a) * wf + mid * full + u32::from(d) * wl + 32768) >> 16) as u16;
    }
}

fn v_box_5(vrow: &mut [u16], rows: [&[u8]; 5], wf: u32, full: u32, wl: u32) {
    let [r0, r1, r2, r3, r4] = rows;
    for (((((v, &a), &b), &c), &d), &e) in vrow.iter_mut().zip(r0).zip(r1).zip(r2).zip(r3).zip(r4) {
        let mid = u32::from(b) + u32::from(c) + u32::from(d);
        *v = ((u32::from(a) * wf + mid * full + u32::from(e) * wl + 32768) >> 16) as u16;
    }
}

impl ScaleScratch {
    fn vertical_row(&mut self, src: &[u8], stride: usize, sh: u32, dh: u32, dy: u32) {
        let row = |y: u32| &src[y as usize * stride..][..stride];
        let v = &mut self.vrow[..stride];
        if sh <= dh {
            let tap = if sh == dh { HSpan2 { x0: dy, w0: SCALE_WEIGHT as u16, w1: 0 } }
                else { bilinear_span(sh, dh, dy) };
            if tap.w1 == 0 {
                for (v, &p) in v.iter_mut().zip(row(tap.x0)) { *v = u16::from(p) << 7; }
            } else {
                v_blend_2(v, row(tap.x0), row(tap.x0 + 1), tap.w0, tap.w1);
            }
            return;
        }
        let span = box_coverage(sh, dh, dy);
        let y = span.first;
        let last = y + span.inner_len + 1;
        let (a, b) = (row(y), row(last));
        if span.inner_len <= 1 {
            let full = ((u64::from(dh) << 14) / u64::from(sh)) as u32;
            let (wf, wl) = span.weights(SCALE_WEIGHT, full);
            if span.inner_len == 0 {
                v_blend_2(v, a, b, wf as u16, wl as u16);
            } else {
                v_blend_3(v, a, row(y + 1), b, wf as u16, full as u16, wl as u16);
            }
        } else if span.inner_len <= 255 {
            let full = ((u64::from(dh) << 23) / u64::from(sh)) as u32;
            let (wf, wl) = span.weights(1 << 23, full);
            match span.inner_len {
                2 => v_box_4(v, [a, row(y + 1), row(y + 2), b], wf, full, wl),
                3 => v_box_5(v, [a, row(y + 1), row(y + 2), row(y + 3), b], wf, full, wl),
                _ => {
                    self.vsum16.resize(stride, 0);
                    for (s, &p) in self.vsum16.iter_mut().zip(row(y + 1)) { *s = u16::from(p); }
                    for sy in y + 2..last {
                        for (s, &p) in self.vsum16.iter_mut().zip(row(sy)) { *s += u16::from(p); }
                    }
                    for (((v, &a), &mid), &b) in v.iter_mut().zip(a).zip(&self.vsum16).zip(b) {
                        *v = ((u32::from(a) * wf + u32::from(mid) * full + u32::from(b) * wl + 32768) >> 16) as u16;
                    }
                }
            }
        } else {
            let denom = u64::from(sh);
            let combine = |a: u8, mid: u64, b: u8| {
                let sum = u64::from(a) * u64::from(span.first_cov) + mid * u64::from(dh) + u64::from(b) * u64::from(span.last_cov);
                (((sum << 7) + denom / 2) / denom) as u16
            };
            if span.inner_len > u32::MAX / 255 {
                // The unweighted sum itself can exceed u32 for tall input.
                for i in 0..stride {
                    let mid = (y + 1..last).map(|sy| u64::from(row(sy)[i])).sum();
                    v[i] = combine(a[i], mid, b[i]);
                }
                return;
            }
            self.vsum32.resize(stride, 0);
            for (s, &p) in self.vsum32.iter_mut().zip(row(y + 1)) { *s = u32::from(p); }
            for sy in y + 2..last {
                for (s, &p) in self.vsum32.iter_mut().zip(row(sy)) { *s += u32::from(p); }
            }
            for (((v, &a), &mid), &b) in v.iter_mut().zip(a).zip(&self.vsum32).zip(b) {
                *v = combine(a, u64::from(mid), b);
            }
        }
    }
}

fn h_three_tap_scalar(vrow: &[u16], taps: &[HSpan3], out: &mut [u8]) {
    for (px, tap) in out.chunks_exact_mut(4).zip(taps) {
        let s = &vrow[tap.x0 as usize * 4..][..12];
        let [w0, w1, w2, _] = tap.w.map(u32::from);
        for c in 0..4 {
            let sum = u32::from(s[c]) * w0 + u32::from(s[c+4]) * w1 + u32::from(s[c+8]) * w2;
            px[c] = ((sum + (1 << 20)) >> 21) as u8;
        }
    }
}

fn h_box_span_scalar(vrow: &[u16], spans: &[HSpanBox], w_full: u32, out: &mut [u8]) {
    for (px, tap) in out.chunks_exact_mut(4).zip(spans) {
        let n = (tap.inner_len as usize + 1) * 4;
        let s = &vrow[tap.x0 as usize * 4..][..n + 4];
        let mut mid = [0u32; 4];
        for p in s[4..n].chunks_exact(4) {
            for c in 0..4 { mid[c] += u32::from(p[c]); }
        }
        for c in 0..4 {
            let sum = u32::from(s[c]) * tap.wf + mid[c] * w_full + u32::from(s[n + c]) * tap.wl;
            px[c] = ((sum + (1 << 22)) >> 23) as u8;
        }
    }
}

fn h_box_span_exact(vrow: &[u16], spans: &[HSpanBox], w_full: u32, sw: u32, out: &mut [u8]) {
    let denom = u64::from(sw) << 7;
    for (px, tap) in out.chunks_exact_mut(4).zip(spans) {
        let n = (tap.inner_len as usize + 1) * 4;
        let s = &vrow[tap.x0 as usize * 4..][..n + 4];
        let mut mid = [0u64; 4];
        for p in s[4..n].chunks_exact(4) {
            for c in 0..4 { mid[c] += u64::from(p[c]); }
        }
        for c in 0..4 {
            let sum = u64::from(s[c]) * u64::from(tap.wf) + mid[c] * u64::from(w_full)
                + u64::from(s[n + c]) * u64::from(tap.wl);
            px[c] = ((sum + denom / 2) / denom) as u8;
        }
    }
}

#[cfg(all(target_arch = "x86_64", not(relax)))]
#[target_feature(enable = "avx2")]
unsafe fn h_three_tap_avx2(vrow: &[u16], taps: &[HSpan3], out: &mut [u8]) {
    use std::arch::x86_64::*;

    let count = taps.len().min(out.len() / 4);
    let pairs = count / 2;
    let round = _mm_set1_epi32(1 << 20);
    let src_ptr = vrow.as_ptr();

    for (pair, dst) in taps[..pairs * 2].chunks_exact(2).zip(out[..pairs * 8].chunks_exact_mut(8)) {
        let (ta, tb) = (&pair[0], &pair[1]);

        let pa = src_ptr.add(ta.x0 as usize * 4);
        let a0 = _mm_loadl_epi64(pa.cast());
        let a1 = _mm_loadl_epi64(pa.add(4).cast());
        let a2 = _mm_cvtepu16_epi32(_mm_loadl_epi64(pa.add(8).cast()));
        let wa01 = _mm_set1_epi32(std::ptr::read_unaligned(ta.w.as_ptr().cast::<i32>()));
        let wa2 = _mm_set1_epi32(std::ptr::read_unaligned(ta.w.as_ptr().add(2).cast::<i32>()));
        let sa = _mm_add_epi32(
            _mm_madd_epi16(_mm_unpacklo_epi16(a0, a1), wa01),
            _mm_madd_epi16(a2, wa2),
        );
        let ra = _mm_srli_epi32::<21>(_mm_add_epi32(sa, round));

        let pb = src_ptr.add(tb.x0 as usize * 4);
        let b0 = _mm_loadl_epi64(pb.cast());
        let b1 = _mm_loadl_epi64(pb.add(4).cast());
        let b2 = _mm_cvtepu16_epi32(_mm_loadl_epi64(pb.add(8).cast()));
        let wb01 = _mm_set1_epi32(std::ptr::read_unaligned(tb.w.as_ptr().cast::<i32>()));
        let wb2 = _mm_set1_epi32(std::ptr::read_unaligned(tb.w.as_ptr().add(2).cast::<i32>()));
        let sb = _mm_add_epi32(
            _mm_madd_epi16(_mm_unpacklo_epi16(b0, b1), wb01),
            _mm_madd_epi16(b2, wb2),
        );
        let rb = _mm_srli_epi32::<21>(_mm_add_epi32(sb, round));

        let u16s = _mm_packus_epi32(ra, rb);
        let u8s = _mm_packus_epi16(u16s, u16s);
        _mm_storel_epi64(dst.as_mut_ptr().cast(), u8s);
    }

    if count > pairs * 2 {
        h_three_tap_scalar(vrow, &taps[pairs * 2..count], &mut out[pairs * 8..count * 4]);
    }
}

#[cfg(all(target_arch = "aarch64", not(relax)))]
unsafe fn h_three_tap_neon(vrow: &[u16], taps: &[HSpan3], out: &mut [u8]) {
    use std::arch::aarch64::*;

    let count = taps.len().min(out.len() / 4);
    let pairs = count / 2;
    let src_ptr = vrow.as_ptr();

    for (pair, dst) in taps[..pairs * 2].chunks_exact(2).zip(out[..pairs * 8].chunks_exact_mut(8)) {
        let (ta, tb) = (&pair[0], &pair[1]);

        let pa = src_ptr.add(ta.x0 as usize * 4);
        let a01 = vld1q_u16(pa);
        let a2 = vld1_u16(pa.add(8));
        let wa = vld1_u16(ta.w.as_ptr());
        let acc_a = vmlal_lane_u16::<2>(
            vmlal_high_lane_u16::<1>(vmull_lane_u16::<0>(vget_low_u16(a01), wa), a01, wa),
            a2,
            wa,
        );
        let ra = vrshrq_n_u32::<21>(acc_a);

        let pb = src_ptr.add(tb.x0 as usize * 4);
        let b01 = vld1q_u16(pb);
        let b2 = vld1_u16(pb.add(8));
        let wb = vld1_u16(tb.w.as_ptr());
        let acc_b = vmlal_lane_u16::<2>(
            vmlal_high_lane_u16::<1>(vmull_lane_u16::<0>(vget_low_u16(b01), wb), b01, wb),
            b2,
            wb,
        );
        let rb = vrshrq_n_u32::<21>(acc_b);

        let u16s = vcombine_u16(vmovn_u32(ra), vmovn_u32(rb));
        vst1_u8(dst.as_mut_ptr(), vmovn_u16(u16s));
    }

    if count > pairs * 2 {
        h_three_tap_scalar(vrow, &taps[pairs * 2..count], &mut out[pairs * 8..count * 4]);
    }
}

#[cfg(all(target_arch = "x86_64", not(relax)))]
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn h_box_pixel_avx2(
    src_ptr: *const u16,
    tap: &HSpanBox,
    v_wfull: std::arch::x86_64::__m128i,
    round: std::arch::x86_64::__m128i,
    m_lo: usize,
) -> std::arch::x86_64::__m128i {
    use std::arch::x86_64::*;

    let base = src_ptr.add(tap.x0 as usize * 4);
    let p_first = _mm_cvtepu16_epi32(_mm_loadl_epi64(base.cast()));
    let mut mid = _mm_cvtepu16_epi32(_mm_loadl_epi64(base.add(4).cast()));
    for j in 1..m_lo {
        mid = _mm_add_epi32(mid, _mm_cvtepu16_epi32(_mm_loadl_epi64(base.add((j + 1) * 4).cast())));
    }
    let extra = _mm_cvtepu16_epi32(_mm_loadl_epi64(base.add((m_lo + 1) * 4).cast()));
    let mask = _mm_set1_epi32(-((tap.inner_len as usize > m_lo) as i32));
    mid = _mm_add_epi32(mid, _mm_and_si128(extra, mask));
    let p_last = _mm_cvtepu16_epi32(_mm_loadl_epi64(base.add((tap.inner_len as usize + 1) * 4).cast()));

    let t_f = _mm_mullo_epi32(p_first, _mm_set1_epi32(tap.wf as i32));
    let t_m = _mm_mullo_epi32(mid, v_wfull);
    let t_l = _mm_mullo_epi32(p_last, _mm_set1_epi32(tap.wl as i32));
    let sum = _mm_add_epi32(_mm_add_epi32(t_f, t_m), _mm_add_epi32(t_l, round));
    _mm_srli_epi32::<23>(sum)
}

#[cfg(all(target_arch = "x86_64", not(relax)))]
#[target_feature(enable = "avx2")]
unsafe fn h_box_span_avx2(vrow: &[u16], spans: &[HSpanBox], w_full: u32, m_lo: usize, out: &mut [u8]) {
    use std::arch::x86_64::*;

    let count = spans.len().min(out.len() / 4);
    let pairs = count / 2;
    let src_ptr = vrow.as_ptr();
    let v_wfull = _mm_set1_epi32(w_full as i32);
    let round = _mm_set1_epi32(1 << 22);

    for (pair, dst) in spans[..pairs * 2].chunks_exact(2).zip(out[..pairs * 8].chunks_exact_mut(8)) {
        let ra = h_box_pixel_avx2(src_ptr, &pair[0], v_wfull, round, m_lo);
        let rb = h_box_pixel_avx2(src_ptr, &pair[1], v_wfull, round, m_lo);
        let u16s = _mm_packus_epi32(ra, rb);
        let u8s = _mm_packus_epi16(u16s, u16s);
        _mm_storel_epi64(dst.as_mut_ptr().cast(), u8s);
    }

    if count > pairs * 2 {
        h_box_span_scalar(vrow, &spans[pairs * 2..count], w_full, &mut out[pairs * 8..count * 4]);
    }
}

#[cfg(all(target_arch = "aarch64", not(relax)))]
#[inline]
unsafe fn h_box_pixel_neon(
    src_ptr: *const u16,
    tap: &HSpanBox,
    v_wfull: std::arch::aarch64::uint32x4_t,
    m_lo: usize,
) -> std::arch::aarch64::uint32x4_t {
    use std::arch::aarch64::*;

    let base = src_ptr.add(tap.x0 as usize * 4);
    let p_first = vld1_u16(base);
    let mut mid = vmovl_u16(vld1_u16(base.add(4)));
    for j in 1..m_lo {
        mid = vaddw_u16(mid, vld1_u16(base.add((j + 1) * 4)));
    }
    let extra = vld1_u16(base.add((m_lo + 1) * 4));
    let mask = vdup_n_u16(if tap.inner_len as usize > m_lo { u16::MAX } else { 0 });
    mid = vaddw_u16(mid, vand_u16(extra, mask));
    let p_last = vld1_u16(base.add((tap.inner_len as usize + 1) * 4));

    let acc = vmlaq_u32(
        vmlal_n_u16(vmull_n_u16(p_first, tap.wf as u16), p_last, tap.wl as u16),
        mid,
        v_wfull,
    );
    vrshrq_n_u32::<23>(acc)
}

#[cfg(all(target_arch = "aarch64", not(relax)))]
unsafe fn h_box_span_neon(vrow: &[u16], spans: &[HSpanBox], w_full: u32, m_lo: usize, out: &mut [u8]) {
    use std::arch::aarch64::*;

    let count = spans.len().min(out.len() / 4);
    let pairs = count / 2;
    let src_ptr = vrow.as_ptr();
    let v_wfull = vdupq_n_u32(w_full);

    for (pair, dst) in spans[..pairs * 2].chunks_exact(2).zip(out[..pairs * 8].chunks_exact_mut(8)) {
        let ra = h_box_pixel_neon(src_ptr, &pair[0], v_wfull, m_lo);
        let rb = h_box_pixel_neon(src_ptr, &pair[1], v_wfull, m_lo);
        let u16s = vcombine_u16(vmovn_u32(ra), vmovn_u32(rb));
        vst1_u8(dst.as_mut_ptr(), vmovn_u16(u16s));
    }

    if count > pairs * 2 {
        h_box_span_scalar(vrow, &spans[pairs * 2..count], w_full, &mut out[pairs * 8..count * 4]);
    }
}

impl HorizontalPlan {
    fn write_row(&self, vrow: &[u16], sw: u32, out: &mut [u8], use_simd: bool) {
        match self {
            Self::Identity => {
                for (dst, &v) in out.iter_mut().zip(vrow) { *dst = ((v + 64) >> 7) as u8; }
            }
            Self::TwoTap(taps) => {
                for (px, tap) in out.chunks_exact_mut(4).zip(taps) {
                    let s = &vrow[tap.x0 as usize * 4..][..8];
                    for c in 0..4 {
                        let sum = u32::from(s[c]) * u32::from(tap.w0) + u32::from(s[c+4]) * u32::from(tap.w1);
                        px[c] = ((sum + (1 << 20)) >> 21) as u8;
                    }
                }
            }
            Self::ThreeTap(taps) => {
                // Each span starts at or before sw - 2; the zero sentinel covers its third load.
                #[cfg(all(target_arch = "x86_64", not(relax)))]
                if use_simd && vrow.len() >= (sw as usize + 1) * 4 && std::arch::is_x86_feature_detected!("avx2") {
                    unsafe { h_three_tap_avx2(vrow, taps, out) };
                    return;
                }
                #[cfg(all(target_arch = "aarch64", not(relax)))]
                if use_simd && vrow.len() >= (sw as usize + 1) * 4 {
                    unsafe { h_three_tap_neon(vrow, taps, out) };
                    return;
                }
                let _ = use_simd;
                h_three_tap_scalar(vrow, taps, out);
            }
            Self::BoxSpan { w_full, m_lo, exact_ratio, spans } => {
                if *exact_ratio {
                    h_box_span_exact(vrow, spans, *w_full, sw, out);
                    return;
                }
                // The optional interior load is within the span even when it reads the last pixel.
                #[cfg(all(target_arch = "x86_64", not(relax)))]
                if use_simd && *m_lo >= 1 && vrow.len() >= sw as usize * 4 && std::arch::is_x86_feature_detected!("avx2") {
                    unsafe { h_box_span_avx2(vrow, spans, *w_full, *m_lo as usize, out) };
                    return;
                }
                #[cfg(all(target_arch = "aarch64", not(relax)))]
                if use_simd && *m_lo >= 1 && vrow.len() >= sw as usize * 4 {
                    unsafe { h_box_span_neon(vrow, spans, *w_full, *m_lo as usize, out) };
                    return;
                }
                let _ = (use_simd, m_lo);
                h_box_span_scalar(vrow, spans, *w_full, out);
            }
        }
    }
}

fn scale_rgba_area_separable(src: &[u8], sw: u32, sh: u32, dst: &mut [u8], dw: u32, dh: u32, scratch: &mut ScaleScratch) {
    scale_rgba_area_separable_mode(src, sw, sh, dst, dw, dh, scratch, true);
}

fn scale_rgba_area_separable_mode(
    src: &[u8], sw: u32, sh: u32, dst: &mut [u8], dw: u32, dh: u32, scratch: &mut ScaleScratch, use_simd: bool,
) {
    let stride = sw as usize * 4;
    if scratch.cached_w != (sw, dw) {
        scratch.h_plan = HorizontalPlan::new(sw, dw);
        scratch.vrow.resize(stride + 4, 0);
        scratch.vrow[stride..].fill(0);
        scratch.cached_w = (sw, dw);
    }
    for (dy, out) in dst.chunks_exact_mut(dw as usize * 4).enumerate() {
        scratch.vertical_row(src, stride, sh, dh, dy as u32);
        scratch.h_plan.write_row(&scratch.vrow, sw, out, use_simd);
    }
}

fn scale_rgba_area(src: &[u8], sw: u32, sh: u32, dst: &mut [u8], dw: u32, dh: u32, scratch: &mut ScaleScratch) {
    if (sw, sh) == (dw, dh) {
        dst.copy_from_slice(src);
    } else if sw == dw.saturating_mul(2) && sh == dh.saturating_mul(2) {
        let stride = sw as usize * 4;
        for (pair, out) in src.chunks_exact(stride * 2).zip(dst.chunks_exact_mut(dw as usize * 4)) {
            let (a, b) = pair.split_at(stride);
            for ((a, b), px) in a.chunks_exact(8).zip(b.chunks_exact(8)).zip(out.chunks_exact_mut(4)) {
                for c in 0..4 {
                    let sum = u16::from(a[c]) + u16::from(a[c+4]) + u16::from(b[c]) + u16::from(b[c+4]);
                    px[c] = ((sum + 2) >> 2) as u8;
                }
            }
        }
    } else {
        scale_rgba_area_separable(src, sw, sh, dst, dw, dh, scratch);
    }
}

struct VideoCapture {
    codec: StreamVideoCodec,
    encoder: Option<VideoStreamEncoder>,
    scaled: Vec<u8>,
    scale_scratch: ScaleScratch,
    nv12: Vec<u8>,
    seq: u64,
    key_after: Option<u64>,
}

impl VideoCapture {
    fn control(&mut self, args: &mut FramesArgs, bytes: &[u8]) -> Result<(), String> {
        let msg = std::str::from_utf8(bytes).map_err(|_| "frames: control must be UTF-8")?.trim();
        if msg == "key" {
            self.key_after.get_or_insert_with(video_time_ns);
        } else if let Some(value) = msg.strip_prefix("kbps=") {
            args.kbps = bitrate(value)?;
            if let Some(enc) = &mut self.encoder { enc.set_bitrate_kbps(args.kbps); }
        } else if let Some(value) = msg.strip_prefix("fps=") {
            args.every = capture_interval(value.parse().map_err(|_| "frames: fps must be a number")?)?;
        } else {
            return Err("frames: expected key, kbps=N or fps=N".into());
        }
        Ok(())
    }

    fn encode(&mut self, args: &FramesArgs, frame: VideoFrame) -> Result<Vec<Vec<u8>>, String> {
        let dimension = |pixels| {
            let n = (f64::from(pixels) * args.scale).round();
            if n > f64::from(u32::MAX) { return Err("frames: scaled dimension exceeds u32"); }
            Ok(((n as u32) & !1).max(2))
        };
        let (w, h) = (dimension(frame.width)?, dimension(frame.height)?);
        let len = (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(4))
            .ok_or("frames: scaled picture is too large")?;
        if self.encoder.as_ref().is_none_or(|e| (e.options().width, e.options().height) != (w, h)) {
            let options = VideoStreamEncoderOptions {
                codec: self.codec, width: w, height: h,
                fps: (1.0 / args.every.as_secs_f64()).round().max(1.0) as u32,
                bitrate_kbps: args.kbps, keyint: 0, low_latency: true, full_chroma: args.full_chroma,
            };
            let mut encoder = VideoStreamEncoder::new(options).map_err(|e| e.to_string())?;
            if let Some(qp) = args.max_qp { encoder.set_max_qp(qp); }
            self.encoder = Some(encoder);
        }
        let rgba = if (frame.width, frame.height) == (w, h) {
            &frame.rgba
        } else {
            if len > self.scaled.len() {
                self.scaled.try_reserve_exact(len - self.scaled.len()).map_err(|e| e.to_string())?;
            }
            self.scaled.resize(len, 0);
            scale_rgba_area(&frame.rgba, frame.width, frame.height, &mut self.scaled, w, h, &mut self.scale_scratch);
            &self.scaled
        };
        let encoder = self.encoder.as_mut().unwrap();
        let chroma = if encoder.options().full_chroma {
            makepad_video::nv12::rgbx_to_nv24(rgba, w, h, 4, makepad_video::VideoColor::BT709, &mut self.nv12);
            "444"
        } else {
            makepad_video::nv12::rgbx_to_nv12(rgba, w, h, 4, makepad_video::VideoColor::BT709, &mut self.nv12);
            "420"
        };
        let packets = encoder.push_frame_nv12(&self.nv12, (frame.time_ns / 100) as i64)
            .map_err(|e| e.to_string())?;
        let codec = match self.codec { StreamVideoCodec::H264 => "h264", StreamVideoCodec::Hevc => "hevc" };
        let window = args.window.unwrap_or(0);
        let mut records = Vec::with_capacity(packets.len());
        for packet in packets {
            let capabilities = if self.seq == 0 {
                let caps = VideoStreamEncoder::caps();
                let codecs = if caps.hevc { r#"["h264","hevc"]"# } else { r#"["h264"]"# };
                format!(r#","codecs":{codecs},"full_chroma":{}"#, caps.full_chroma)
            } else { String::new() };
            let header = format!(
                r#"{{"seq":{},"w":{w},"h":{h},"window":{window},"t_ns":{},"codec":"{codec}","key":{},"pts":{},"chroma":"{chroma}"{capabilities}}}"#,
                self.seq, packet.pts_100ns * 100, packet.is_key, packet.pts_100ns,
            );
            records.push(makepad_remote::frames::encode_record(&header, &packet.data));
            self.seq += 1;
        }
        Ok(records)
    }
}

fn run_video_capture<S: GrabSubmit>(mut args: FramesArgs, codec: StreamVideoCodec, out: &FramesOut, submit: S) -> Result<(), String> {
    use makepad_remote::conn::SendError;
    use std::collections::VecDeque;

    let skipped = Arc::new(AtomicU64::new(0));
    // the forced keyframe grabs' worker and window, once
    start_encoder()?;
    let grab_window = resolve_grab_window(args.window)?;
    let mut input = Some(VideoInput::new(&args, skipped.clone()));
    let mut video = VideoCapture { codec, encoder: None, scaled: Vec::new(), scale_scratch: ScaleScratch::default(),
        nv12: Vec::new(), seq: 0, key_after: Some(video_time_ns()) };
    let mut pending = VecDeque::new();
    let mut log_at = Instant::now();
    let mut paused_at: Option<Instant> = None;
    let mut paused_slots = 0.0;
    let mut waiting_for_grab = false;

    loop {
        if let Some(last) = &mut paused_at {
            paused_slots += last.elapsed().as_secs_f64() / args.every.as_secs_f64();
            *last = Instant::now();
        }
        if log_at.elapsed() >= Duration::from_secs(10) {
            crate::log!("frames video: {} capture slots skipped under send backpressure, {} frames skipped before encode",
                paused_slots as u64, skipped.swap(0, Ordering::Relaxed));
            paused_slots = paused_slots.fract();
            log_at = Instant::now();
        }
        let old_interval = args.every;
        if !out.receive(|bytes| video.control(&mut args, bytes))? { return Ok(()); }
        if args.every != old_interval { input.take(); }

        if let Some(record) = pending.pop_front() {
            match (out.try_send)(record) {
                Ok(()) => {}
                Err(SendError::Closed) => return Ok(()),
                Err(SendError::Full(record)) => {
                    pending.push_front(record);
                    input.take();
                    paused_at.get_or_insert_with(Instant::now);
                    std::thread::sleep(args.every.min(Duration::from_millis(200)));
                }
            }
            continue;
        }
        paused_at = None;
        let source = input.get_or_insert_with(|| VideoInput::new(&args, skipped.clone()));
        let poll = Duration::from_millis(200);
        let key_wait = video.key_after.map(|at| {
            args.every.saturating_sub(Duration::from_nanos(video_time_ns().saturating_sub(at)))
        });
        let frame = match source.next(key_wait.unwrap_or(poll).min(poll)) {
            Some(frame) if video.key_after.is_some_and(|at| frame.time_ns < at) => continue,
            Some(frame) => frame,
            None if key_wait.is_some_and(|wait| wait <= poll) => {
                // No natural present in the capture interval. The one-shot forces one even
                // for a hidden or idle window. Remove the live sink during its readback.
                input.take();
                // a short wait, never extended: this loop serves its control messages and tries again
                let permit = match wait_for_grab_slot(Instant::now() + poll, || false, || false) {
                    Ok(permit) => permit,
                    // the grab window is full: serve control messages and natural presents,
                    // then try again
                    Err(AcquireError::TimedOut | AcquireError::Cancelled) => {
                        if !waiting_for_grab {
                            crate::log!("frames video: waiting for room for a keyframe grab");
                            waiting_for_grab = true;
                        }
                        video.key_after = Some(video_time_ns());
                        continue;
                    }
                };
                let mut grab = GrabSeries::new(0, grab_window, 1.0, true, submit.clone());
                grab.arm_chunk(vec![permit], Instant::now(), Duration::ZERO, 0)?;
                let controls = std::cell::RefCell::new((&mut video, &mut args));
                let error = std::cell::Cell::new(None);
                let gone = || match out.receive(|bytes| {
                    let mut controls = controls.borrow_mut();
                    let (video, args) = &mut *controls;
                    video.control(args, bytes)
                }) {
                    Ok(open) => !open,
                    Err(why) => { error.set(Some(why)); true }
                };
                let current = grab.receive_next(&gone);
                if let Some(why) = error.take() { return Err(why); }
                let current = current?;
                let frame = VideoFrame { width: current.width, height: current.height,
                    rgba: current.data, time_ns: video_time_ns() };
                input = Some(VideoInput::new(&args, skipped.clone()));
                frame
            }
            None => continue,
        };
        waiting_for_grab = false;
        if video.key_after.take().is_some() {
            if let Some(enc) = &mut video.encoder { enc.request_keyframe(); }
        }
        pending.extend(video.encode(&args, frame)?);
    }
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

/// The `frames` service of the endpoint: each OPEN becomes a producer thread ([`run_frames_with`]).
/// The app's own endpoint only: under the JIT the host serves it (remote.rs `host_frames_open`).
#[cfg(not(relax))]
pub fn frames_service() -> makepad_remote::endpoint::Factory {
    frames_service_with(UiQueue)
}

#[cfg(any(not(relax), all(test, target_os = "macos")))]
fn frames_service_with<S: GrabSubmit>(submit: S) -> makepad_remote::endpoint::Factory {
    use makepad_remote::conn::{SendError, CLOSE_REFUSED};
    Arc::new(move |_conn, open| {
        let args = parse_frames_args(&open.args).map_err(|why| (CLOSE_REFUSED, why))?;
        let submit = submit.clone();
        std::thread::Builder::new()
            .name("makepad-remote-frames".into())
            .spawn(move || {
                let ch = open.channel;
                let (tx, video_tx, tx2) = (ch.tx(), ch.tx(), ch.tx());
                run_frames_with(
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
                        try_send: Box::new(move |record| video_tx.try_send(record)),
                        close: Box::new(move |code, why| tx2.close(code, why)),
                        try_recv: Box::new(move || ch.try_recv()),
                    },
                    submit,
                );
            })
            .map(|_| ())
            .map_err(|e| (CLOSE_REFUSED, format!("cannot start the frames thread: {e}")))
    })
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WaitError {
    Disconnected,
    ClientGone,
    Stalled(String),
}

impl WaitError {
    fn message(self) -> String {
        match self {
            Self::Disconnected => "app reply channel disconnected".into(),
            Self::ClientGone => "remote client disconnected".into(),
            Self::Stalled(reason) => reason,
        }
    }
}

pub(crate) trait ProgressProbe {
    fn sample_thread(&mut self) -> ThreadExecSample;
    fn sample_jit(&mut self, quiescent: bool) -> JitWaitSample;
}

struct RealProgressProbe {
    thread: UiThreadProbe,
    #[cfg(relax)]
    inspect: Option<makepad_remote::Client>,
}

impl RealProgressProbe {
    fn new() -> Self {
        Self {
            thread: UiThreadProbe::global(),
            #[cfg(relax)]
            inspect: None,
        }
    }

    #[cfg(relax)]
    fn inspect(&mut self) -> Option<&makepad_remote::Client> {
        if self.inspect.is_none() {
            let client = makepad_remote::Client::connect(&std::process::id().to_string()).ok()?;
            client.set_timeout(Some(Duration::from_secs(2)));
            self.inspect = Some(client);
        }
        self.inspect.as_ref()
    }

    #[cfg(relax)]
    fn jit_busy(&mut self) -> bool {
        self.inspect().and_then(|client| makepad_remote::jit::busy(client).ok()).unwrap_or(false)
    }
}

impl ProgressProbe for RealProgressProbe {
    fn sample_thread(&mut self) -> ThreadExecSample {
        self.thread.sample()
    }

    #[cfg(relax)]
    fn sample_jit(&mut self, quiescent: bool) -> JitWaitSample {
        use crate::makepad_micro_serde::{DeJson, JsonValue};
        if !quiescent { return JitWaitSample::default(); }
        let Some(client) = self.inspect() else { return JitWaitSample::default() };
        if let Ok(lines) = client.cmd("panics") {
            for line in lines.map_while(Result::ok) {
                let Ok(event) = JsonValue::deserialize_json(&line) else { continue };
                let field = |key| event.get(key).and_then(JsonValue::as_str).unwrap_or("");
                if field("thread") == "main" {
                    let reason = format!("{} {} at {}: {}", field("id"), field("kind"), field("location"), field("message"));
                    return JitWaitSample { busy: false, parked: Some(reason) };
                }
            }
        }
        JitWaitSample { busy: self.jit_busy(), parked: None }
    }

    #[cfg(not(relax))]
    fn sample_jit(&mut self, _quiescent: bool) -> JitWaitSample {
        JitWaitSample::default()
    }
}

/// Only command and frames threads run this loop. OS and JIT probes start after the first miss.
pub(crate) fn wait_for_reply<T>(
    what: &'static str,
    kind: WaitKind,
    rx: &Receiver<T>,
    live: &CommandLive,
    progress: &UiProgressState,
    cfg: WatchConfig,
    probe: &mut dyn ProgressProbe,
    off_ui: Option<&AtomicBool>,
    gone: &dyn Fn() -> bool,
    rewake: impl Fn(),
) -> Result<T, WaitError> {
    use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
    let started = Instant::now();
    let elapsed_ms = || started.elapsed().as_millis() as u64;
    let encoding = || off_ui.is_some_and(|flag| flag.load(Ordering::Acquire));
    match rx.try_recv() {
        Ok(reply) => return Ok(reply),
        Err(TryRecvError::Disconnected) => return Err(WaitError::Disconnected),
        Err(TryRecvError::Empty) => {}
    }
    let mut last_ui = progress.snapshot();
    let mut last_stage = live.stage();
    if last_ui.state == UiState::Exited && !encoding() {
        return Err(WaitError::Stalled("app event loop exited".into()));
    }
    let interval = Duration::from_millis(cfg.probe_interval_ms);
    match rx.recv_timeout(interval) {
        Ok(reply) => return Ok(reply),
        Err(RecvTimeoutError::Disconnected) => return Err(WaitError::Disconnected),
        Err(RecvTimeoutError::Timeout) => {}
    }
    if gone() { return Err(WaitError::ClientGone); }
    let mut last_thread = if encoding() { ThreadExecSample::unknown() } else { probe.sample_thread() };
    let mut watch = ProgressWatch::new(what, kind, cfg, 0, WatchSample {
        ui: last_ui, stage: last_stage, thread: last_thread, jit: JitWaitSample::default(),
    });
    let reply = loop {
        let probe_start = Instant::now();
        let ui = progress.snapshot();
        let action = if encoding() {
            watch.waiting_for_encoder(elapsed_ms(), ui)
        } else {
            let stage = live.stage();
            let thread = probe.sample_thread();
            let advanced = |now: Option<u64>, prev| matches!((now, prev), (Some(n), Some(p)) if n > p);
            let idle_wait = kind != WaitKind::Turn && stage == CmdStage::Waiting
                && ui.state == UiState::InPump && ui.repaints == last_ui.repaints;
            let quiet = stage == last_stage && ui.turns == last_ui.turns && ui.repaints == last_ui.repaints
                && !advanced(thread.cpu_ns, last_thread.cpu_ns) && !advanced(thread.run_delay_ns, last_thread.run_delay_ns)
                && !matches!(thread.state, ThreadExecState::RunningOrRunnable | ThreadExecState::UninterruptibleIo);
            let quiescent = ui.holds == 0 && (idle_wait || quiet) && ui.state != UiState::Exited
                && !matches!(thread.state, ThreadExecState::Stopped | ThreadExecState::Exited);
            let jit = probe.sample_jit(quiescent);
            last_ui = ui;
            last_stage = stage;
            last_thread = thread;
            watch.step(elapsed_ms(), WatchSample { ui, stage, thread, jit })
        };
        match action {
            WatchAction::Continue => {}
            WatchAction::Rewake => rewake(),
            WatchAction::LogSlow(line) => crate::log!("{line}"),
            WatchAction::Stalled(reason) => match rx.try_recv() {
                Ok(reply) => break reply,
                Err(TryRecvError::Disconnected) => return Err(WaitError::Disconnected),
                Err(TryRecvError::Empty) if !encoding() => return Err(WaitError::Stalled(reason)),
                Err(TryRecvError::Empty) => {}
            },
        }
        let until_probe = interval.saturating_sub(probe_start.elapsed()).max(Duration::from_millis(50)).min(interval);
        match rx.recv_timeout(until_probe) {
            Ok(reply) => break reply,
            Err(RecvTimeoutError::Disconnected) => return Err(WaitError::Disconnected),
            Err(RecvTimeoutError::Timeout) if gone() => return Err(WaitError::ClientGone),
            Err(RecvTimeoutError::Timeout) => {}
        }
    };
    if watch.logged_slow() {
        crate::log!("{}", watch.summary_on_complete(elapsed_ms(), progress.snapshot()));
    }
    Ok(reply)
}

pub fn ask<F>(what: &'static str, kind: WaitKind, make: F) -> Reply
where F: FnOnce(Sender<Reply>) -> Cmd {
    ask_with(what, kind, &GLOBAL_UI_PROGRESS, WatchConfig::default(), make)
}

pub(crate) fn ask_with<F>(what: &'static str, kind: WaitKind, progress: &UiProgressState, cfg: WatchConfig, make: F) -> Reply
where F: FnOnce(Sender<Reply>) -> Cmd {
    let (tx, rx) = channel();
    let (live, _pending) = CommandLive::new();
    let request = QueuedCmd { cmd: make(tx), by: asked_by(), user_seq: REQUEST_USER_SEQ.get(), live: live.clone() };
    {
        let mut commands = queue().lock().unwrap();
        if progress.snapshot().state == UiState::Exited {
            return if what == "quit" { Reply::Ok } else { Reply::Err("app event loop exited".into()) };
        }
        commands.push(request);
    }
    wake();
    match wait_for_reply(what, kind, &rx, &live, progress, cfg, &mut RealProgressProbe::new(), None, &request_gone, wake) {
        Ok(reply) => reply,
        Err(error) => {
            let exited = error == WaitError::Disconnected
                || matches!(&error, WaitError::Stalled(s) if s == "app event loop exited");
            if what == "quit" && exited && (live.stage() != CmdStage::Queued || progress.snapshot().state == UiState::Exited) {
                Reply::Ok
            } else {
                Reply::Err(error.message())
            }
        }
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

    const GRAB_CHILD: &str = "MAKEPAD_REMOTE_GRAB_TESTS_CHILD";
    /// These tests stand in for the UI thread. Another test's loop exit can clear their grab sinks
    /// if they share a process.
    const IN_OWN_PROCESS: &[&str] = &[
        "a_grab_over_the_pixel_budget_keeps_its_requested_size",
        "a_grabs_pixels_do_not_depend_on_queue_halving",
        "concurrent_65th_grab_waits_for_slot_logs_once_and_completes",
        "encode_grab_png_accepts_frame_over_64_mib_and_matches_pixels",
        "encode_grab_raw_accepts_frame_over_64_mib_and_matches_pixels",
        "encode_grab_rejects_overflowing_scaled_dimensions",
        #[cfg(target_os = "macos")]
        "frames_video_capture_decodes_and_accepts_key_requests",
        "run_frames_paces_65_frame_series_through_sliding_window",
        "submit_encode_never_refuses_when_encoder_channel_receives_65_jobs",
    ];

    /// A failed test may leave grab slots held. Stop there so the next test cannot wait for them.
    static GRAB_TEST_FAILED: AtomicBool = AtomicBool::new(false);
    struct MarksAFailure;
    impl Drop for MarksAFailure {
        fn drop(&mut self) {
            if std::thread::panicking() {
                GRAB_TEST_FAILED.store(true, Ordering::SeqCst);
            }
        }
    }

    macro_rules! in_own_process {
        ($name:ident) => {
            if std::env::var_os(GRAB_CHILD).is_none() {
                assert!(IN_OWN_PROCESS.contains(&stringify!($name)),
                    "{} runs in the grab tests' process: list it in IN_OWN_PROCESS", stringify!($name));
                return;
            }
            if GRAB_TEST_FAILED.load(Ordering::SeqCst) {
                println!("{}: not run, a grab test before it failed", stringify!($name));
                return;
            }
            let _marks_a_failure = MarksAFailure;
        };
    }

    #[test]
    fn the_grab_tests_run_in_a_process_of_their_own() {
        if std::env::var_os(GRAB_CHILD).is_some() {
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "--test-threads=1"])
            .args(IN_OWN_PROCESS.iter().map(|name| format!("remote::bridge::tests::{name}")))
            .env(GRAB_CHILD, "1")
            .output()
            .unwrap();
        let all = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        let with = |verdict: &str| -> Vec<&str> {
            IN_OWN_PROCESS.iter().copied()
                .filter(|name| all.contains(&format!("test remote::bridge::tests::{name} ... {verdict}")))
                .collect()
        };
        if let Some(failed) = with("FAILED").first() {
            // libtest prints the panic message below its thread's panic line; the JIT reports it as JSON
            let lines: Vec<&str> = all.lines().collect();
            let mark = format!("thread 'remote::bridge::tests::{failed}' ");
            let panic = lines
                .windows(2)
                .find(|w| w[0].starts_with(&mark) && w[0].contains(" panicked at "))
                .map(|w| format!("{} ({})", w[1].trim(), w[0].trim()))
                .or_else(|| lines.iter().find(|l| l.contains("\"kind\":\"panic\"")).map(|l| l.trim().to_string()))
                .unwrap_or_else(|| "no panic line".to_string());
            panic!("{failed} failed in the grab tests' process: {panic}\n{all}");
        }
        let passed = with("ok");
        assert!(
            passed.len() == IN_OWN_PROCESS.len() && output.status.success(),
            "the grab tests' process ({}) passed {} of the {} listed; not run: {:?}\n{all}",
            output.status,
            passed.len(),
            IN_OWN_PROCESS.len(),
            IN_OWN_PROCESS.iter().filter(|name| !passed.contains(name)).collect::<Vec<_>>()
        );
        println!("the grab tests' process passed {} tests: {}", passed.len(), passed.join(", "));
    }

    // Inject only the execution sample. The real receiver, cancellation and watch stay in the loop.
    struct TestProbe<F>(F);
    impl<F: FnMut() -> ThreadExecSample> ProgressProbe for TestProbe<F> {
        fn sample_thread(&mut self) -> ThreadExecSample { (self.0)() }
        fn sample_jit(&mut self, _quiescent: bool) -> JitWaitSample { JitWaitSample::default() }
    }

    fn wait_config(interval: u64, confirm: u32) -> WatchConfig {
        WatchConfig { probe_interval_ms: interval, stall_confirm_probes: confirm, ..WatchConfig::default() }
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple", all(windows, target_pointer_width = "64")))]
    fn real_busy_ui_thread_outlives_confirm_window_and_applies_click() {
        let progress = UiProgressState::new();
        let _turn = progress.enter_turn();
        let (live, _pending) = CommandLive::new();
        let cfg = wait_config(20, 5);
        let samples = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let (probe_tx, probe_rx) = channel();
        let worker = {
            let (live, samples, stop) = (live.clone(), samples.clone(), stop.clone());
            std::thread::spawn(move || {
                probe_tx.send(UiThreadProbe::for_current_thread()).unwrap();
                // One initial baseline, followed by more than a full confirmation window.
                while samples.load(Ordering::Acquire) <= 1 + cfg.stall_confirm_probes as usize {
                    if stop.load(Ordering::Acquire) { return; }
                    std::hint::spin_loop();
                }
                assert!(live.is_alive(), "the busy UI can still apply the click");
                live.set_stage(CmdStage::Running);
                tx.send(Reply::Ok).unwrap();
            })
        };
        let mut os = probe_rx.recv().unwrap();
        let mut probe = TestProbe(|| {
            let sample = os.sample();
            samples.fetch_add(1, Ordering::Release);
            sample
        });
        let answer = wait_for_reply("click", WaitKind::Turn, &rx, &live, &progress, cfg, &mut probe, None, &|| false, || {});
        stop.store(true, Ordering::Release);
        worker.join().unwrap();
        assert!(matches!(answer, Ok(Reply::Ok)), "click reply after confirmed busy samples: {:?}", answer.err());
        assert!(samples.load(Ordering::Acquire) > 1 + cfg.stall_confirm_probes as usize);
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple", all(windows, target_pointer_width = "64")))]
    fn real_parked_ui_thread_fails_fast_with_blocked_reason() {
        let progress = UiProgressState::new();
        let _turn = progress.enter_turn();
        progress.swap_phase(crate::thread::UiPhase::FrameBuild as u32);
        let (live, pending) = CommandLive::new();
        let cfg = wait_config(20, 5);
        let (tx, rx) = channel::<Reply>();
        let (probe_tx, probe_rx) = channel();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let worker = {
            let gate = gate.clone();
            std::thread::spawn(move || {
                let mut released = gate.0.lock().unwrap();
                probe_tx.send(UiThreadProbe::for_current_thread()).unwrap();
                while !*released { released = gate.1.wait(released).unwrap(); }
                drop(tx);
            })
        };
        let mut os = probe_rx.recv().unwrap();
        let sleeping = (0..100_000).any(|_| {
            std::thread::yield_now();
            os.sample().state == ThreadExecState::Sleeping
        });
        let probes = std::cell::Cell::new(0u32);
        let mut probe = TestProbe(|| {
            probes.set(probes.get() + 1);
            os.sample()
        });
        // Bound the test by samples even if the OS never reports the expected evidence.
        let answer = if sleeping {
            wait_for_reply("click", WaitKind::Turn, &rx, &live, &progress, cfg, &mut probe, None,
                &|| probes.get() > cfg.stall_confirm_probes + 2, || {})
        } else { Err(WaitError::ClientGone) };
        drop(pending);
        let cancelled_before_release = !live.is_alive();
        *gate.0.lock().unwrap() = true;
        gate.1.notify_one();
        worker.join().unwrap();
        assert!(sleeping, "the target must be observed asleep before starting the wait");
        assert!(matches!(&answer, Err(WaitError::Stalled(s)) if s.contains("blocked or deadlocked")
            && s.contains("phase=frame-build") && s.contains("thread=sleeping")), "unexpected result: {:?}", answer.err());
        assert!(cancelled_before_release);
        assert_eq!(probes.get(), cfg.stall_confirm_probes + 1, "one baseline plus confirmation probes");
    }

    #[test]
    fn dropped_sender_client_gone_or_exited_loop_fails_at_once() {
        use std::cell::Cell;
        let cfg = wait_config(10, 5);
        let count = Cell::new(0);
        let mut probe = TestProbe(|| {
            count.set(count.get() + 1);
            ThreadExecSample { state: ThreadExecState::Sleeping, cpu_ns: Some(0), run_delay_ns: None }
        });
        let (live, pending) = CommandLive::new();
        let progress = UiProgressState::new();
        let (tx, rx) = channel::<()>();
        drop(tx);
        assert_eq!(wait_for_reply("click", WaitKind::Turn, &rx, &live, &progress, cfg, &mut probe, None, &|| false, || {}),
            Err(WaitError::Disconnected));
        assert_eq!(count.get(), 0);

        let (tx, rx) = channel();
        progress.mark_exited();
        assert_eq!(wait_for_reply("click", WaitKind::Turn, &rx, &live, &progress, cfg, &mut probe, None, &|| false, || {}),
            Err(WaitError::Stalled("app event loop exited".into())));
        assert_eq!(count.get(), 0);
        let progress = UiProgressState::new();
        assert_eq!(wait_for_reply("click", WaitKind::Turn, &rx, &live, &progress, cfg, &mut probe, None, &|| true, || {}),
            Err(WaitError::ClientGone));
        drop(pending);
        assert!(!live.is_alive());
        assert!(count.get() <= 1);

        let live = CommandLive::detached();
        live.set_stage(CmdStage::Waiting);
        progress.set_in_pump();
        let polls = Cell::new(0);
        let gone = || {
            polls.set(polls.get() + 1);
            if polls.get() > cfg.stall_confirm_probes { tx.send(()).unwrap(); }
            false
        };
        let before = count.get();
        let answer = wait_for_reply("grab", WaitKind::Grab { not_before_ms: None }, &rx, &live, &progress, cfg,
            &mut probe, Some(&AtomicBool::new(true)), &gone, || panic!("encoder needs no UI wake"));
        assert_eq!(answer, Ok(()));
        assert!(polls.get() > cfg.stall_confirm_probes);
        assert_eq!(count.get(), before, "encoding needs no UI thread probe");
    }

    #[test]
    fn cpu_starved_and_unknown_thread_wait_loop_never_fails() {
        for (state, cpu_ns, delay) in [
            (ThreadExecState::RunningOrRunnable, Some(10_000), true),
            (ThreadExecState::RunningOrRunnable, Some(10_000), false),
            (ThreadExecState::Unknown, Some(10_000), false),
            (ThreadExecState::Unknown, None, false),
        ] {
            let cfg = wait_config(5, 4);
            let progress = UiProgressState::new();
            let _turn = progress.enter_turn();
            let (live, _pending) = CommandLive::new();
            let (tx, rx) = channel();
            let mut samples = 0;
            let mut probe = TestProbe(|| {
                samples += 1;
                if samples == 11 { tx.send(Reply::Ok).unwrap(); }
                ThreadExecSample { state, cpu_ns, run_delay_ns: delay.then_some(samples) }
            });
            let answer = wait_for_reply("click", WaitKind::Turn, &rx, &live, &progress, cfg, &mut probe, None, &|| false, || {});
            assert!(matches!(answer, Ok(Reply::Ok)), "{state:?}, cpu={cpu_ns:?}, run delay={delay}: {:?}", answer.err());
            assert!(live.is_alive(), "a starved or unobservable UI can still apply the command");
            assert_eq!(samples, 11, "one baseline and ten slow probes");
        }
    }

    #[test]
    fn parse_frames_args_accepts_chroma_for_video_only() {
        assert!(parse_frames_args("cap=1 codec=hevc chroma=444").unwrap().full_chroma);
        assert!(!parse_frames_args("cap=1 codec=h264 chroma=420").unwrap().full_chroma);
        assert!(!parse_frames_args("cap=1 codec=hevc").unwrap().full_chroma);
        for args in ["codec=png chroma=444", "codec=png chroma=420", "cap=1 codec=hevc chroma=422"] {
            assert!(parse_frames_args(args).is_err(), "{args}");
        }
    }

    #[test]
    fn scale_rgba_area_preserves_thin_lines_and_flat_fields() {
        let mut src = vec![255; 500 * 500 * 4];
        // Point sampling at 5:3 skips source column/row 2 in each period.
        for (i, pixel) in src.chunks_exact_mut(4).enumerate() {
            if i % 500 % 5 == 2 || i / 500 % 5 == 2 {
                pixel[..3].fill(0);
            }
        }
        let mut dst = vec![0; 300 * 300 * 4];
        let mut scratch = ScaleScratch::default();
        scale_rgba_area(&src, 500, 500, &mut dst, 300, 300, &mut scratch);
        // Sample away from crossings. Each stroke covers 3/5 of its destination pixel.
        assert_eq!(dst[0], 255);
        for p in (1..300).step_by(3) {
            assert_eq!(dst[p * 4], 102, "column {p}");
            assert_eq!(dst[p * 300 * 4], 102, "row {p}");
        }
        let flat = [17, 42, 200, 255];
        for pixel in src.chunks_exact_mut(4) { pixel.copy_from_slice(&flat); }
        scale_rgba_area(&src, 500, 500, &mut dst, 300, 300, &mut scratch);
        assert!(dst.chunks_exact(4).all(|p| p == flat));
        scale_rgba_area(&dst[..3 * 2 * 4], 3, 2, &mut src[..7 * 5 * 4], 7, 5, &mut scratch);
        assert!(src[..7 * 5 * 4].chunks_exact(4).all(|p| p == flat));

        // Box-filter vertically and interpolate horizontally, including the two clamped ends.
        let mixed = [[0; 4], [80; 4], [40; 4], [120; 4]].concat();
        scale_rgba_area(&mixed, 2, 2, &mut dst[..16], 4, 1, &mut scratch);
        assert_eq!(&dst[..16], &[[20; 4], [40; 4], [80; 4], [100; 4]].concat());
    }

    fn exact_area_pixel(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32, dx: u32, dy: u32) -> [u8; 4] {
        let (sw, sh, dw, dh) = (u128::from(sw), u128::from(sh), u128::from(dw), u128::from(dh));
        let (x0, y0) = (u128::from(dx) * sw, u128::from(dy) * sh);
        let (x1, y1) = (x0 + sw, y0 + sh);
        let mut sum = [0u128; 4];
        for y in y0/dh..y1.div_ceil(dh) {
            let cy = ((y + 1) * dh).min(y1) - (y * dh).max(y0);
            for x in x0/dw..x1.div_ceil(dw) {
                let cx = ((x + 1) * dw).min(x1) - (x * dw).max(x0);
                let i = ((y * sw + x) * 4) as usize;
                for c in 0..4 { sum[c] += u128::from(src[i+c]) * cx * cy; }
            }
        }
        let area = sw * sh;
        sum.map(|v| ((v + area/2) / area) as u8)
    }

    #[test]
    fn scale_rgba_area_fractional_boxes_match_rational_coverage() {
        let mut src = vec![255; 320 * 200 * 4];
        for (i, p) in src.chunks_exact_mut(4).enumerate() {
            let (x, y) = (i % 320, i / 320);
            if x == 48 { p[0] = 0; }
            if y == 91 { p[1] = 0; }
            p[2] = (x ^ y) as u8;
            p[3] = (x * 37 + y * 73) as u8;
        }
        let mut dst = vec![0; 86 * 54 * 4];
        let mut scratch = ScaleScratch::default();
        scale_rgba_area(&src, 320, 200, &mut dst, 86, 54, &mut scratch);
        for (i, p) in dst.chunks_exact(4).enumerate() {
            let (x, y) = ((i % 86) as u32, (i / 86) as u32);
            let exact = exact_area_pixel(&src, 320, 200, 86, 54, x, y);
            for c in 0..4 {
                assert!(p[c].abs_diff(exact[c]) <= 1, "({x},{y}) channel {c}: {} vs {}", p[c], exact[c]);
            }
            if x * 320 < 49 * 86 && (x + 1) * 320 > 48 * 86 { assert!(p[0] < 255, "column {x}"); }
            if y * 200 < 92 * 54 && (y + 1) * 200 > 91 * 54 { assert!(p[1] < 255, "row {y}"); }
        }
        let flat = [17, 42, 200, 255];
        for p in src.chunks_exact_mut(4) { p.copy_from_slice(&flat); }
        scale_rgba_area(&src, 320, 200, &mut dst, 86, 54, &mut scratch);
        assert!(dst.chunks_exact(4).all(|p| p == flat));
    }

    #[test]
    fn scale_rgba_area_two_by_two_matches_separable() {
        let mut src: Vec<u8> = (0..8*6*4).map(|i| (i * 73 + i / 7) as u8).collect();
        let flat = [13, 23, 179, 251];
        let mut fast = [0; 4*3*4];
        let mut separable = [0; 4*3*4];
        let mut scratch = ScaleScratch::default();
        for flat_field in [false, true] {
            if flat_field {
                for p in src.chunks_exact_mut(4) { p.copy_from_slice(&flat); }
            }
            scale_rgba_area(&src, 8, 6, &mut fast, 4, 3, &mut scratch);
            scale_rgba_area_separable(&src, 8, 6, &mut separable, 4, 3, &mut scratch);
            assert_eq!(fast, separable);
            for (i, p) in fast.chunks_exact(4).enumerate() {
                assert_eq!(p, exact_area_pixel(&src, 8, 6, 4, 3, (i % 4) as u32, (i / 4) as u32));
                if flat_field { assert_eq!(p, flat); }
            }
        }
    }

    #[test]
    fn scale_rgba_area_extreme_ratios_and_visibility_boundaries() {
        let flat = [17, 42, 200, 255];
        let mut src = flat.repeat(512 * 512);
        let mut scratch = ScaleScratch::default();
        let mut dst = [0; 4];
        scale_rgba_area(&src, 512, 512, &mut dst, 1, 1, &mut scratch);
        assert_eq!(dst, flat);
        src.fill(255);
        // Two dark interior rows/columns are visible at 1/512. A single
        // black row rounds to white vertically from ratio 507 due to 8.7 rounding.
        for (i, p) in src.chunks_exact_mut(4).enumerate() {
            if matches!(i / 512, 93 | 318) { p[0] = 0; }
            if matches!(i % 512, 93 | 318) { p[1] = 0; }
        }
        scale_rgba_area(&src, 512, 512, &mut dst, 1, 1, &mut scratch);
        assert_eq!(dst, [254, 254, 255, 255]);
        let mut line = vec![255; 506 * 4];
        line[252*4..253*4].fill(0);
        for (sw, sh) in [(1, 506), (506, 1)] {
            scale_rgba_area(&line, sw, sh, &mut dst, 1, 1, &mut scratch);
            assert_eq!(dst, [254; 4], "interior stroke in {sw}x{sh}");
        }
        let mut split = [0; 8];
        scale_rgba_area(&line[..505*4], 1, 505, &mut split, 1, 2, &mut scratch);
        assert_eq!(split, [254; 8], "each output pixel contains half the stroke");
    }

    #[test]
    fn scale_rgba_area_reuses_scratch_across_sizes_and_identity_axes() {
        let mut scratch = ScaleScratch::default();
        for (sw, sh, dw, dh) in [(21, 8, 13, 7), (5, 3, 3, 2), (3, 3, 7, 3), (7, 5, 7, 2), (5, 7, 2, 7)] {
            let src: Vec<u8> = (0..sw*sh*4).map(|i| (i * 19 + 21) as u8).collect();
            let mut reused = vec![0; (dw*dh*4) as usize];
            let mut fresh = reused.clone();
            scale_rgba_area(&src, sw, sh, &mut reused, dw, dh, &mut scratch);
            scale_rgba_area(&src, sw, sh, &mut fresh, dw, dh, &mut ScaleScratch::default());
            assert_eq!(reused, fresh);
            assert_eq!(&scratch.vrow[sw as usize*4..], &[0; 4]);
            if dw <= sw && dh <= sh {
                for (i, p) in reused.chunks_exact(4).enumerate() {
                    let exact = exact_area_pixel(&src, sw, sh, dw, dh, i as u32 % dw, i as u32 / dw);
                    assert!(p.iter().zip(exact).all(|(v, e)| v.abs_diff(e) <= 1));
                }
            }
            let flat = [37, 71, 192, 249];
            scale_rgba_area(&flat.repeat((sw*sh) as usize), sw, sh, &mut reused, dw, dh, &mut scratch);
            assert!(reused.chunks_exact(4).all(|p| p == flat));
        }
        let ramp = [[0; 4], [80; 4]].concat();
        let exact = [[0; 4], [20; 4], [60; 4], [80; 4]].concat();
        for (sw, sh, dw, dh) in [(2, 1, 4, 1), (1, 2, 1, 4)] {
            let mut out = [0; 16];
            scale_rgba_area(&ramp, sw, sh, &mut out, dw, dh, &mut scratch);
            assert!(out.iter().zip(&exact).all(|(v, &e)| v.abs_diff(e) <= 1));
        }
    }

    #[test]
    fn scale_rgba_area_simd_matches_scalar() {
        let mut scalar_scratch = ScaleScratch::default();
        let mut simd_scratch = ScaleScratch::default();
        for (sw, sh, dw, dh) in [
            (320, 200, 192, 120), (129, 97, 77, 59),
            (320, 200, 86, 54), (321, 201, 85, 53),
            (30, 20, 15, 10), (30, 20, 70, 50),
            (43_691, 1, 21_845, 1), (130, 2, 2, 2),
        ] {
            let src: Vec<u8> = (0..sw * sh * 4).map(|i| ((i * 73) ^ (i >> 3) ^ (i >> 11)) as u8).collect();
            let mut scalar = vec![0; (dw * dh * 4) as usize];
            let mut simd = vec![0; scalar.len()];
            scale_rgba_area_separable_mode(&src, sw, sh, &mut scalar, dw, dh, &mut scalar_scratch, false);
            scale_rgba_area_separable_mode(&src, sw, sh, &mut simd, dw, dh, &mut simd_scratch, true);
            assert_eq!(simd, scalar, "{sw}x{sh} -> {dw}x{dh}");
            if sw == 43_691 {
                let HorizontalPlan::BoxSpan { spans, .. } = &simd_scratch.h_plan else { panic!("expected box spans") };
                assert!(spans.iter().any(|s| s.wf > i16::MAX as u32), "exercise unsigned edge weights");
            }
            if sw == dw * 2 && sh == dh * 2 {
                simd.fill(0);
                scale_rgba_area(&src, sw, sh, &mut simd, dw, dh, &mut simd_scratch);
                assert_eq!(simd, scalar, "2x2 fast path");
            }
        }
    }

    /// Feed the capture-delivery boundary and exercise the service, channel and real codecs.
    /// VideoToolbox can run this without a window server or a HAL test session.
    #[cfg(target_os = "macos")]
    #[test]
    fn frames_video_capture_decodes_and_accepts_key_requests() {
        in_own_process!(frames_video_capture_decodes_and_accepts_key_requests);
        use makepad_remote::{carrier::Carrier, frame::FrameReader, frames, Endpoint, Event};
        use std::os::unix::net::UnixStream;

        let _lock = grab_test_lock();
        fn carrier(stream: UnixStream) -> Carrier {
            Carrier {
                reader: FrameReader::new(Box::new(stream.try_clone().unwrap())),
                writer: Box::new(stream.try_clone().unwrap()),
                shutdown: Box::new(move || { let _ = stream.shutdown(std::net::Shutdown::Both); }),
            }
        }
        let ep = Endpoint::new("capture-test");
        let (ui, cmds) = LocalGrabQueue::channel();
        ep.register("frames", 1, frames_service_with(ui));
        let (a, b) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let car = carrier(a);
            ep.serve_peer(car.reader, car.writer, car.shutdown, "", 0).unwrap()
        });
        let client = Endpoint::new("capture-client").connect(carrier(b), "test").unwrap();
        let server = server.join().unwrap();
        let window = 8673;
        let request = frames::Request { window: Some(window), cap: true, codec: Some("h264"), scale: Some(0.5), ..Default::default() };
        let ch = client.open("frames", &request.video_args(2000, None)).unwrap();
        let rgba = vec![128; 323 * 243 * 4];
        // Stand in for the window's event loop. Nothing presents until the service asks
        // for the current drawable through the same grab command a real window receives.
        let serve_forced_present = || {
            let cmd = cmds.recv_timeout(Duration::from_secs(30)).expect("the service requests a forced present");
            let Cmd::Grab { window: requested_window, grabs, tx, .. } = cmd else {
                panic!("expected a grab for the forced present");
            };
            assert_eq!(requested_window, Some(window));
            let sink = grab_sinks().lock().unwrap().remove(&grabs[0].0).unwrap();
            assert!(sink.raw_output);
            assert!(tx.send(Reply::Ok).is_ok());
            assert!(sink.tx.send(Ok(Grabbed { window_id: window, width: 323, height: 243,
                data: rgba.clone(), capture_ms: 0., encode_ms: 0., backend_png: false,
                drawn: None, _bytes: None })).is_ok());
        };
        let next = || match ch.recv() {
            Some(Event::Data(msg)) => frames::decode_record(&msg).unwrap(),
            event => panic!("expected an encoded unit, got {event:?}"),
        };
        serve_forced_present();
        let first = next();
        assert_eq!(first.flag("key"), Some(true), "the stream must start independently decodable");
        assert_eq!(first.text("codec").as_deref(), Some("h264"));
        assert_eq!(first.flag("full_chroma"), Some(VideoStreamEncoder::caps().full_chroma));
        assert!(first.json.contains(r#""codecs":["h264""#));
        let mut decoder = makepad_video::VideoStreamDecoder::new(StreamVideoCodec::H264).unwrap();
        let mut decoded = decoder.push_packet(&first.bytes, first.num("pts").unwrap() as i64).unwrap();
        decoded.extend(decoder.flush().unwrap());
        assert_eq!(decoded.iter().map(|f| (f.width, f.height)).collect::<Vec<_>>(), [(162, 122)]);
        crate::screen_capture::deliver_capture_frame(Some(window), 323, 243, &rgba);
        let delta = next();
        assert_eq!(delta.flag("key"), Some(false), "the second unit should be predicted");
        ch.send(b"kbps=1000".to_vec()).unwrap();
        ch.send(b"fps=60".to_vec()).unwrap();
        ch.send(b"key".to_vec()).unwrap();
        serve_forced_present();
        let key = next();
        assert_eq!(key.flag("key"), Some(true), "an idle window answers key without an app update");
        assert!(key.num("pts").unwrap() > delta.num("pts").unwrap());
        ch.close(makepad_remote::conn::CLOSE_NORMAL, "");
        client.close("test finished");
        server.close("test finished");
    }

    #[test]
    fn dblclick_is_one_input_batch() {
        use MouseKind::{Down, Move, Up};

        let params = Params(vec![
            ("k".into(), "dblclick".into()),
            ("x".into(), "50".into()),
            ("y".into(), "60".into()),
        ]);
        let mut fingers = crate::event::CxFingers::default();
        let mut kinds = Vec::new();
        let mut taps = Vec::new();
        for input in mouse_inputs(&params, None).unwrap() {
            let Input::Mouse { kind, x, y, time, .. } = input else { panic!("expected mouse input") };
            assert_eq!((x, y, time), (50.0, 60.0, None), "all events use the point and the batch clock");
            kinds.push(kind);
            if kind == Down {
                taps.push(fingers.process_tap_count(crate::makepad_math::dvec2(x, y), 10.0));
            }
        }
        assert!(matches!(kinds.as_slice(), [Move, Down, Up, Down, Up]), "expected two clicks in one batch");
        assert_eq!(taps, [1, 2]);
    }

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
            matches!(parse_file_input(&Params(valid), FileInput::Drop), Ok((Some(2), Input::DropFile { path: p, x: 12.5, y: 20.0 }, true)) if p == path)
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
                parse_file_input(&Params(invalid), FileInput::Drop).is_err(),
                "{key} accepted invalid value"
            );
        }
        let mut duplicate = fields.clone();
        duplicate.push(("path".into(), path.clone()));
        assert!(parse_file_input(&Params(duplicate), FileInput::Drop).is_err());
        let mut aliases = fields.clone();
        aliases.extend([("w".into(), "1".into()), ("window".into(), "2".into())]);
        assert!(parse_file_input(&Params(aliases), FileInput::Drop).is_err());
        let mut long_path = fields.clone();
        long_path[0].1 = format!("{path}{}", "a".repeat(4096));
        assert!(parse_file_input(&Params(long_path), FileInput::Drop).is_err());
        // a drag takes what a drop takes; its end only the window and the wait
        assert!(
            matches!(parse_file_input(&Params(fields.clone()), FileInput::Drag), Ok((None, Input::DragFile { path: p, x: 12.5, y: 20.0 }, false)) if p == path)
        );
        assert!(parse_file_input(&Params(Vec::new()), FileInput::Drag).is_err_and(|e| e.contains("drag requires path=")));
        assert!(matches!(parse_file_input(&Params(vec![("w".into(), "1".into())]), FileInput::DragEnd), Ok((Some(1), Input::DragFileEnd, false))));
        assert!(parse_file_input(&Params(fields), FileInput::DragEnd).is_err_and(|e| e.contains("unknown drag_end parameter")));
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

    /// Every route of the HTTP bridge is a command of the endpoint, by its route name.
    #[test]
    fn every_former_route_resolves_to_a_command() {
        let routes = [
            ("status", 1),
            ("gamepads", 1),
            ("activity", 1),
            ("worker_violations", 1),
            ("mouse", 1),
            ("click", 1),
            ("move", 1),
            ("key", 1),
            ("text", 1),
            ("drop", 1),
            ("drag", 1),
            ("drag_end", 1),
            ("trace", 1),
            ("step", 1),
            ("settled", 1),
            ("uicounts", 1),
            ("cap pause", 2),
            ("cap resume", 2),
            ("cap start", 2),
            ("cap stop", 2),
            ("input record", 2),
            ("input replay", 2),
            ("input stop", 2),
            ("input status", 2),
            ("front", 1),
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
            ("gpu_inventory", 1),
            ("quit", 1),
            ("log", 1),
            ("cpu", 1),
            ("net", 1),
            ("loop", 1),
            ("midi", 1),
            ("cursor", 1),
        ];
        for (line, used) in routes {
            let r = resolve(&words(line)).unwrap_or_else(|e| panic!("{line}: {e}"));
            assert_eq!((r.key, r.used), (line, used), "{line}");
        }
        for &(op, _) in makepad_remote::params::DESIGN {
            let design = resolve(&words(&format!("design {op}"))).unwrap();
            assert_eq!((design.key, design.sub.as_deref(), design.used), ("design", Some(op), 2));
        }
        assert!(design_reads("tree", &[]) && design_reads("pick", &[]) && design_reads("props", &[]) && design_reads("locate", &[]));
        assert!(design_reads("verify", &[]), "verification stays available during user activity");
        assert!(!design_reads("apply", &[]) && !design_reads("pose", &[]));
        for op in ["drop", "preview_chunk"] {
            assert!(design_reads(op, &[]), "{op} must answer without waiting for a frame");
        }
        for op in ["preview", "revert"] {
            assert!(!design_reads(op, &[]), "{op} must use the guarded mutation route");
        }
        assert!(design_reads("theme", &[("get".into(), "1".into())]));
        assert!(!design_reads("theme", &[("set".into(), "1".into())]));
        assert!(resolve(&words("design")).is_err());
        for removed in ["snap", "dump", "focus"] {
            assert!(resolve(&words(removed)).unwrap_err().contains("unknown command"));
        }
        assert!(resolve(&words("nope")).unwrap_err().contains("unknown command"));
        let unknown = resolve(&words("m")).unwrap_err();
        assert!(unknown.starts_with("unknown command `m`: the commands are "), "{unknown}");
        assert!(unknown.contains("mouse, click, move"), "{unknown}");
        assert_eq!(unknown.matches("cap").count(), 1, "{unknown}");
        assert!(unknown.ends_with("(`help` describes each)"), "{unknown}");
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

    /// Tests that touch the process's grab state (`PENDING_GRABS`, `GRAB_BYTES`, the grab
    /// window) take this, so one test's slots or pixels never land in another's.
    static GRAB_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn grab_test_lock() -> std::sync::MutexGuard<'static, ()> {
        GRAB_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[derive(Clone)]
    struct LocalGrabQueue(Arc<Mutex<Sender<Cmd>>>);

    impl LocalGrabQueue {
        fn channel() -> (Self, Receiver<Cmd>) {
            let (tx, rx) = channel();
            (Self(Arc::new(Mutex::new(tx))), rx)
        }
    }

    impl GrabSubmit for LocalGrabQueue {
        fn ask_grab(&self, make: impl FnOnce(Sender<Reply>) -> Cmd) -> Reply {
            let (tx, rx) = channel();
            if self.0.lock().unwrap().send(make(tx)).is_err() {
                return Reply::Err("local grab queue closed".into());
            }
            rx.recv_timeout(Duration::from_secs(60))
                .unwrap_or_else(|e| Reply::Err(format!("local grab queue: {e}")))
        }

        fn receive_grab(&self, rx: &Receiver<Result<Grabbed, String>>, _: Instant,
            _: &AtomicBool, _: &dyn Fn() -> bool) -> Result<Grabbed, String>
        {
            rx.recv_timeout(Duration::from_secs(60)).map_err(|e| e.to_string())?
        }

        fn cancel_grabs(&self, ids: Vec<u64>) {
            let _ = self.0.lock().unwrap().send(Cmd::CancelGrabs(ids));
        }
    }

    /// Waits (up to 2 s) for every grab slot to be released: the encoder worker drops a job's
    /// sink, and its permit, just after it delivered the grab.
    fn wait_for_no_pending_grabs() -> bool {
        let until = Instant::now() + Duration::from_secs(2);
        while PENDING_GRABS.load(Ordering::SeqCst) != 0 {
            if Instant::now() >= until {
                return false;
            }
            std::thread::yield_now();
        }
        true
    }

    /// A slot past the window, for a burst that bypasses it (as a direct encoder submit does).
    fn burst_permit() -> GrabPermit {
        PENDING_GRABS.fetch_add(1, Ordering::SeqCst);
        GrabPermit(())
    }

    /// A 2x2 RGBA readback for `sink`.
    fn tiny_grab_job(sink: GrabSink) -> EncodeJob {
        EncodeJob {
            sink,
            width: 2,
            height: 2,
            pixels: crate::HostBuf::from_vec(vec![200; 16]),
            stride: 8,
            order: GrabOrder::Rgba,
            origin: GrabOrigin::TopLeft,
            capture_ms: 0.0,
            backend_png: false,
            bytes: None,
        }
    }

    fn grab_test_job(width: u32, height: u32, pixels: Vec<u8>) -> EncodeJob {
        let (tx, _) = sync_channel(1);
        EncodeJob {
            sink: GrabSink {
                encoding: Arc::new(AtomicBool::new(false)),
                id: GRAB_ID_BASE + NEXT_ID.fetch_add(1, Ordering::Relaxed),
                window: None,
                requested_at: Instant::now(),
                scale: 1.0,
                raw_output: false,
                cancelled: Arc::new(AtomicBool::new(false)),
                tx,
                // GrabSink releases its slot on drop, including for direct encoder calls.
                _permit: try_acquire_grab_slot().expect("a free grab slot"),
            },
            width,
            height,
            pixels: crate::HostBuf::from_vec(pixels),
            stride: width as usize * 4,
            order: GrabOrder::Rgba,
            origin: GrabOrigin::TopLeft,
            capture_ms: 0.0,
            backend_png: false,
            bytes: None,
        }
    }

    const GRAB_MARKERS: [(usize, usize, [u8; 4]); 3] = [
        (0, 0, [13, 29, 47, 91]),
        (2048, 2048, [53, 71, 97, 127]),
        (4095, 4096, [83, 107, 151, 211]),
    ];

    fn grab_one_row_over_64_mib() -> EncodeJob {
        let (width, height) = (4096, 4097);
        let template = [7, 41, 89, 255].repeat(width);
        let mut pixels = vec![0; template.len() * height];
        for row in pixels.chunks_exact_mut(template.len()) {
            row.copy_from_slice(&template);
        }
        for (x, y, marker) in GRAB_MARKERS {
            let i = (y * width + x) * 4;
            pixels[i..i + 4].copy_from_slice(&marker);
        }
        grab_test_job(width as u32, height as u32, pixels)
    }

    #[test]
    fn encode_grab_png_accepts_frame_over_64_mib_and_matches_pixels() {
        in_own_process!(encode_grab_png_accepts_frame_over_64_mib_and_matches_pixels);
        use makepad_zune_png::{makepad_zune_core::bytestream::ZCursor, PngDecoder};

        let _lock = grab_test_lock();
        let mut job = grab_one_row_over_64_mib();
        job.order = GrabOrder::Bgra;
        job.origin = GrabOrigin::BottomLeft;
        let (w, h, png) = encode_grab(&job).unwrap();
        drop(job);
        assert_eq!((w, h), (4096, 4097));
        let mut decoder = PngDecoder::new(ZCursor::new(&png));
        let rgba = decoder.decode_raw().unwrap();
        assert_eq!(decoder.dimensions(), Some((4096, 4097)));
        assert_eq!(decoder.colorspace(), Some(ColorSpace::RGBA));
        assert_eq!(rgba.len(), 4096 * 4097 * 4);
        for (x, y, [b, g, r, a]) in GRAB_MARKERS {
            let decoded_y = 4096 - y;
            let i = (decoded_y * 4096 + x) * 4;
            assert_eq!(&rgba[i..i + 4], &[r, g, b, a], "PNG pixel ({x}, {decoded_y})");
        }
    }

    #[test]
    fn encode_grab_raw_accepts_frame_over_64_mib_and_matches_pixels() {
        in_own_process!(encode_grab_raw_accepts_frame_over_64_mib_and_matches_pixels);
        let _lock = grab_test_lock();
        let mut job = grab_one_row_over_64_mib();
        job.sink.raw_output = true;
        let (w, h, raw) = encode_grab(&job).unwrap();
        assert_eq!((w, h), (4096, 4097));
        assert_eq!(raw.len(), 4096 * 4097 * 4);
        for (x, y, marker) in GRAB_MARKERS {
            let i = (y * 4096 + x) * 4;
            assert_eq!(&raw[i..i + 4], &marker, "raw pixel ({x}, {y})");
        }
    }

    #[test]
    fn row_halver_matches_iterated_halve_pixels_bit_for_bit() {
        let (width, height) = (67, 35);
        let stride = width * 4 + 12;
        let mut seed = 42u32;
        let pixels: Vec<u8> = (0..stride * height).map(|_| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 24) as u8
        }).collect();

        let mut expected = halve_pixels(&pixels, width, height, stride).unwrap();
        for halvings in 1..=3 {
            let w = width >> halvings;
            let h = height >> halvings;
            let mut halver = RowHalver::new(width, halvings);
            for y in 0..h {
                assert_eq!(
                    halver.row(&pixels, stride, y),
                    &expected[y * w * 4..(y + 1) * w * 4],
                    "memory row {y} after {halvings} halvings"
                );
            }
            if halvings < 3 {
                expected = halve_pixels(&expected, w, h, w * 4).unwrap();
            }
        }
    }

    #[test]
    fn encode_grab_rejects_overflowing_scaled_dimensions() {
        in_own_process!(encode_grab_rejects_overflowing_scaled_dimensions);
        let _lock = grab_test_lock();
        let mut job = grab_test_job(2, 2, vec![0; 16]);
        job.sink.scale = 1e12;
        for raw_output in [false, true] {
            job.sink.raw_output = raw_output;
            assert_eq!(
                encode_grab(&job).unwrap_err(),
                "scaled grab dimension exceeds PNG/u32 range",
                "raw_output={raw_output}"
            );
        }
    }

    /// A gseq of a 4K window outran the encoder and its frames came back halved again and again
    /// (1920x1080 down to 7x4): over the pixel budget a frame is halved only while its requested
    /// size allows, then it queues as it is.
    #[test]
    fn a_grab_over_the_pixel_budget_keeps_its_requested_size() {
        in_own_process!(a_grab_over_the_pixel_budget_keeps_its_requested_size);
        let _lock = grab_test_lock();
        start_encoder().unwrap();
        // other grabs' pixels fill the budget
        GRAB_BYTES.fetch_add(GRAB_QUEUE_BUDGET_BYTES, Ordering::Relaxed);
        let mut sizes = Vec::new();
        for (i, scale) in [0.5, 1.0].into_iter().enumerate() {
            let (tx, rx) = sync_channel(1);
            let sink = GrabSink {
                encoding: Arc::new(AtomicBool::new(false)),
                id: GRAB_ID_BASE + (1 << 40) + i as u64,
                window: None,
                requested_at: Instant::now(),
                scale,
                raw_output: true,
                cancelled: Arc::new(AtomicBool::new(false)),
                tx,
                _permit: try_acquire_grab_slot().expect("a free grab slot"),
            };
            let pixels = crate::HostBuf::from_vec(vec![128u8; 64 * 32 * 4]);
            submit_encode(
                EncodeJob {
                    sink,
                    width: 64,
                    height: 32,
                    pixels,
                    stride: 64 * 4,
                    order: GrabOrder::Rgba,
                    origin: GrabOrigin::TopLeft,
                    capture_ms: 0.0,
                    backend_png: false,
                    bytes: None,
                },
                true,
            );
            let grabbed = rx.recv_timeout(Duration::from_secs(10)).expect("the grab is answered");
            sizes.push(grabbed.map(|g| (g.width, g.height)));
        }
        GRAB_BYTES.fetch_sub(GRAB_QUEUE_BUDGET_BYTES, Ordering::Relaxed);
        assert_eq!(sizes, vec![Ok((32, 16)), Ok((64, 32))], "each grab arrives at its requested size, none refused");
    }

    /// A gseq at scale=0.5 point-sampled frames the queue had not halved, so unchanged text
    /// flipped between two looks (2026-11-17).
    #[test]
    fn a_grabs_pixels_do_not_depend_on_queue_halving() {
        in_own_process!(a_grabs_pixels_do_not_depend_on_queue_halving);
        let _lock = grab_test_lock();
        start_encoder().unwrap();
        let stride = 67 * 4 + 12;
        let mut pixels = vec![0u8; stride * 35];
        for y in 0..35 {
            for x in 0..67 {
                for c in 0..4 {
                    pixels[y * stride + x * 4 + c] = ((x * 37 + y * 101 + c * 59) ^ (x * y)) as u8;
                }
            }
        }
        let mut grabs = Vec::new();
        for i in 0..2 {
            let (tx, rx) = sync_channel(1);
            let sink = GrabSink {
                encoding: Arc::new(AtomicBool::new(false)),
                id: GRAB_ID_BASE + (1 << 40) + 2 + i as u64,
                window: None,
                requested_at: Instant::now(),
                scale: 0.25,
                raw_output: true,
                cancelled: Arc::new(AtomicBool::new(false)),
                tx,
                _permit: try_acquire_grab_slot().expect("a free grab slot"),
            };
            if i == 0 { GRAB_BYTES.fetch_add(GRAB_QUEUE_BUDGET_BYTES, Ordering::Relaxed); }
            submit_encode(
                EncodeJob {
                    sink,
                    width: 67,
                    height: 35,
                    pixels: crate::HostBuf::from_vec(pixels.clone()),
                    stride,
                    order: GrabOrder::Bgra,
                    origin: GrabOrigin::BottomLeft,
                    capture_ms: 0.0,
                    backend_png: false,
                    bytes: None,
                },
                true,
            );
            let grabbed = rx.recv_timeout(Duration::from_secs(10));
            if i == 0 { GRAB_BYTES.fetch_sub(GRAB_QUEUE_BUDGET_BYTES, Ordering::Relaxed); }
            grabs.push(grabbed.expect("the grab is answered").expect("the grab succeeds"));
        }
        let size = "a grab's size does not depend on whether the queue halved it";
        assert_eq!((grabs[0].width, grabs[0].height), (16, 8), "{size}");
        assert_eq!((grabs[1].width, grabs[1].height), (16, 8), "{size}");
        assert_eq!(grabs[0].data, grabs[1].data, "a grab's pixels do not depend on whether the queue halved it");
    }

    /// A series of any length, over any span, is paced through the grab window; only the gap
    /// between frames is bounded.
    #[test]
    fn parse_frames_args_accepts_series_over_64_and_over_60s_span() {
        for (args, n, every_ms) in [("n=65 fps=125", 65, 8), ("n=65 fps=1", 65, 1000), ("n=1000 fps=0.1", 1000, 10_000), ("once=1", 1, 0)] {
            let parsed = match parse_frames_args(args) {
                Ok(parsed) => parsed,
                Err(why) => panic!("{args}: {why}"),
            };
            assert_eq!((parsed.n, parsed.every), (Some(n), Duration::from_millis(every_ms)), "{args}");
        }
        for (args, names) in [("n=0", "positive"), ("n=2 fps=200", "8..=60000 ms"), ("n=2 fps=0.01", "8..=60000 ms")] {
            let Err(why) = parse_frames_args(args) else { panic!("{args} is refused") };
            assert!(why.contains(names), "{args}: {why}");
        }
    }

    /// A 65th grab in flight waits for a slot (logging once per episode) instead of being
    /// refused; it times out only at its deadline and gives up when its client is gone.
    #[test]
    fn concurrent_65th_grab_waits_for_slot_logs_once_and_completes() {
        in_own_process!(concurrent_65th_grab_waits_for_slot_logs_once_and_completes);
        let _lock = grab_test_lock();
        crate::Cx::init_log();
        assert!(wait_for_no_pending_grabs(), "another test left grab slots held");
        let mut held: Vec<GrabPermit> = (0..GRAB_IN_FLIGHT_LIMIT).map(|_| try_acquire_grab_slot().expect("a free grab slot")).collect();
        assert_eq!(PENDING_GRABS.load(Ordering::SeqCst), GRAB_IN_FLIGHT_LIMIT);
        assert!(try_acquire_grab_slot().is_none(), "the window is full");
        GRAB_SLOTS.saturated_logged.store(false, Ordering::SeqCst);
        let cursor = crate::log_ring::read_since(0, 0).newest;

        // the window's own wait, without the JIT's extension: in a JIT test process the other
        // tests' compiles would extend an expired wait again and again
        let waiter = || std::thread::spawn(|| wait_for_grab_slot(Instant::now() + Duration::from_secs(5), || false, || false));
        let (w65, w66) = (waiter(), waiter());
        let until = Instant::now() + Duration::from_secs(5);
        while GRAB_SLOTS.waiters.load(Ordering::SeqCst) < 2 {
            assert!(Instant::now() < until, "both waiters wait");
            std::thread::yield_now();
        }
        std::thread::sleep(Duration::from_millis(50));
        assert!(!w65.is_finished() && !w66.is_finished(), "both block while 64 are held");
        assert_eq!(wait_for_grab_slot(Instant::now() + Duration::from_millis(30), || false, || false).err(), Some(AcquireError::TimedOut));
        let extensions = std::cell::Cell::new(0);
        let extended = wait_for_grab_slot(Instant::now(), || extensions.get() == 1, || { extensions.set(extensions.get() + 1); true });
        assert_eq!((extended.err(), extensions.get()), (Some(AcquireError::Cancelled), 1), "an expired wait asks once to go on, and goes on");
        assert_eq!(wait_for_grab_slot(Instant::now() + Duration::from_secs(5), || true, || false).err(), Some(AcquireError::Cancelled));

        held.truncate(GRAB_IN_FLIGHT_LIMIT - 2);
        let p65 = w65.join().unwrap().expect("#65 gets a slot");
        let p66 = w66.join().unwrap().expect("#66 gets a slot");
        assert_eq!(PENDING_GRABS.load(Ordering::SeqCst), GRAB_IN_FLIGHT_LIMIT);
        drop((p65, p66));
        drop(held);
        assert_eq!(PENDING_GRABS.load(Ordering::SeqCst), 0);
        assert!(!GRAB_SLOTS.saturated_logged.load(Ordering::SeqCst), "the episode is over");

        // logging is asynchronous: wait for a line logged after the episode
        let sentinel = format!("grab window test sentinel {}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
        crate::log!("{sentinel}");
        let until = Instant::now() + Duration::from_secs(5);
        let lines = loop {
            let lines = crate::log_ring::read_since(cursor, crate::log_ring::CAP).lines;
            if lines.iter().any(|line| line.text.contains(&sentinel)) {
                break lines;
            }
            assert!(Instant::now() < until, "the sentinel reaches the log ring");
            std::thread::sleep(Duration::from_millis(1));
        };
        let full = lines.iter().filter(|line| line.text.contains("grab window full (64 in flight)")).count();
        assert_eq!(full, 1, "one log line per saturation episode");
    }

    /// `frames n=65` arms its series in chunks of at most the series window, each joining the
    /// same series on the UI side, with its cadence kept across chunks.
    #[test]
    fn run_frames_paces_65_frame_series_through_sliding_window() {
        in_own_process!(run_frames_paces_65_frame_series_through_sliding_window);
        use makepad_remote::conn::{SendError, CLOSE_NORMAL};
        use std::sync::mpsc::RecvTimeoutError;
        let _lock = grab_test_lock();
        assert!(wait_for_no_pending_grabs(), "another test left grab slots held");
        let window = 8765;
        let args = parse_frames_args(&format!("window={window} codec=raw n=65 fps=125")).unwrap();
        assert_eq!(args.every, Duration::from_millis(8));
        let records = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
        let closed = Arc::new(Mutex::new(None::<(u16, String)>));
        let out = FramesOut {
            send: {
                let records = records.clone();
                Box::new(move |record: Vec<u8>, _wait: bool| {
                    records.lock().unwrap().push(record);
                    true
                })
            },
            try_send: Box::new(|_: Vec<u8>| -> Result<(), SendError> { Ok(()) }),
            close: {
                let closed = closed.clone();
                Box::new(move |code: u16, why: &str| {
                    closed.lock().unwrap().get_or_insert((code, why.to_string()));
                })
            },
            try_recv: Box::new(|| -> Result<makepad_remote::Event, bool> { Err(true) }),
        };
        let (ui, cmds) = LocalGrabQueue::channel();
        let producer = std::thread::spawn(move || run_frames_with(args, out, ui));
        // stand in for the UI thread: take each chunk, answer it and read its grabs back
        let (mut chunks, mut dues, mut armed, mut peak_pending) = (Vec::new(), Vec::new(), 0usize, 0usize);
        loop {
            let (requested_window, series, grabs, tx) = match cmds.recv_timeout(Duration::from_secs(60)) {
                Ok(Cmd::Grab { window, series, grabs, tx, .. }) => (window, series, grabs, tx),
                Ok(Cmd::CancelGrabs(ids)) => panic!("unexpected cancellation of grabs {ids:?}"),
                Ok(_) => panic!("expected a grab for the frame series"),
                Err(RecvTimeoutError::Timeout) => panic!("timed out waiting for the frame series"),
                Err(RecvTimeoutError::Disconnected) => break,
            };
            assert_eq!(requested_window, Some(window));
            // the series asks while it waits for this answer: what it armed and has not sent
            armed += grabs.len();
            peak_pending = peak_pending.max(armed - records.lock().unwrap().len());
            chunks.push((series, grabs.len()));
            dues.extend(grabs.iter().map(|grab| grab.1));
            assert!(tx.send(Reply::Ok).is_ok());
            for (id, _) in grabs {
                let sink = grab_sinks().lock().unwrap().remove(&id).expect("an armed grab has a sink");
                submit_encode(tiny_grab_job(sink), true);
            }
        }
        producer.join().unwrap();
        assert_eq!(closed.lock().unwrap().clone(), Some((CLOSE_NORMAL, String::new())));
        let records = records.lock().unwrap();
        assert_eq!(records.len(), 65);
        for (index, record) in records.iter().enumerate() {
            let record = makepad_remote::frames::decode_record(record).unwrap();
            assert_eq!(record.num("seq"), Some(index as f64));
            assert_eq!((record.num("w"), record.num("h")), (Some(2.0), Some(2.0)));
        }
        assert!(chunks.len() >= 2, "{chunks:?}");
        let series = chunks[0].0;
        assert_ne!(series, 0, "a series has an id");
        assert!(chunks.iter().all(|&(s, len)| s == series && (1..=GRAB_SERIES_WINDOW).contains(&len)), "{chunks:?}");
        assert_eq!(dues.len(), 65);
        assert!(dues.windows(2).all(|pair| pair[1] - pair[0] == Duration::from_millis(8)), "the cadence holds across chunks");
        assert!((2..=GRAB_SERIES_WINDOW).contains(&peak_pending), "{peak_pending}");
        assert!(wait_for_no_pending_grabs(), "every slot is released");
    }

    /// The encoder queue is unbounded: a burst past the grab window is encoded, never refused.
    #[test]
    fn submit_encode_never_refuses_when_encoder_channel_receives_65_jobs() {
        in_own_process!(submit_encode_never_refuses_when_encoder_channel_receives_65_jobs);
        let _lock = grab_test_lock();
        assert!(wait_for_no_pending_grabs(), "another test left grab slots held");
        start_encoder().unwrap();
        let replies: Vec<_> = (0..GRAB_IN_FLIGHT_LIMIT + 1)
            .map(|_| {
                let (tx, rx) = sync_channel(1);
                let sink = GrabSink {
                    encoding: Arc::new(AtomicBool::new(false)),
                    id: GRAB_ID_BASE + NEXT_ID.fetch_add(1, Ordering::Relaxed),
                    window: None,
                    requested_at: Instant::now(),
                    scale: 1.0,
                    raw_output: true,
                    cancelled: Arc::new(AtomicBool::new(false)),
                    tx,
                    _permit: burst_permit(),
                };
                submit_encode(tiny_grab_job(sink), true);
                rx
            })
            .collect();
        for (index, rx) in replies.iter().enumerate() {
            match rx.recv_timeout(Duration::from_secs(10)).expect("the grab is answered") {
                Ok(grabbed) => assert_eq!((grabbed.width, grabbed.height), (2, 2), "grab {index}"),
                Err(why) => panic!("grab {index} refused: {why}"),
            }
        }
        assert!(wait_for_no_pending_grabs(), "every slot is released");
    }
}
