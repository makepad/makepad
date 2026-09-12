//! `--remote`: a tiny localhost HTTP control surface baked into every makepad app.
//!
//! Started from `app_main!` when the process is launched with `--remote`
//! (optionally `--remote=PORT`) or `MAKEPAD_REMOTE=1`. It binds an ephemeral
//! 127.0.0.1 port, prints one grep-able line, and then lets an external agent
//! drive the app: read window geometry, grab PNGs of any window, inject mouse /
//! key / text events through the *real* event path, dump the widget tree, and
//! tail the log.
//!
//! Design notes:
//! - Zero external crates. Hand-rolled HTTP/1.1 (localhost, connection-per-request)
//!   and hand-rolled JSON in/out.
//! - The HTTP threads never touch `Cx`. They push commands onto a global queue and
//!   block on a reply channel; [`poll`] drains the queue from the event loop
//!   (via `Cx::poll_control_channel`, which every backend already calls) and
//!   answers. So responses may block an HTTP thread, never the UI thread.
//! - Input is injected through `Cx::dispatch_studio_msg`, the same function the
//!   studio remote bridge uses, so `Hits`, capture and gestures behave exactly
//!   as they do for real events. File drops use the native drag/drop event path.
//! - Grabs use readback tickets for render
//!   textures and the presenting command buffer for Metal drawables. Raw
//!   pixels are scaled and encoded on one bounded, long-lived worker.
//! - Standalone macOS serializes a grab with remote commands: apply earlier
//!   commands, draw pending changes, then submit the capture before applying
//!   later commands (including input). No NextFrame/animation tick is inserted
//!   at this boundary. Concurrent HTTP requests are ordered by UI queue order,
//!   not by the time their clients opened sockets. Each sequence deadline is
//!   a new boundary; it does not freeze the app for the entire sequence.

#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
mod imp {
    use crate::cx::Cx;
    use crate::cx_api::CxOsApi;
    use crate::makepad_math::dvec2;
    use crate::texture::{
        ReadbackChannelOrder, ReadbackOrigin, ReadbackRequest, ReadbackTicket, TextureReadback,
    };
    use crate::window::WindowId;
    use makepad_studio_protocol::{
        KeyCode, KeyEvent, RemoteKeyModifiers, RemoteMouseDown, RemoteMouseMove, RemoteMouseUp,
        RemoteScroll, ScreenshotRequest, StudioToApp, TextInputEvent,
    };
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    // ------------------------------------------------------------------
    // global state (the HTTP threads' only view of the app)
    // ------------------------------------------------------------------

    static ACTIVE: AtomicBool = AtomicBool::new(false);

    /// True when this process was started with `--remote` (any form). Pure
    /// argv scan, usable before the bridge itself is up — the platform's
    /// focus policy reads it while the first window is being created.
    pub fn requested() -> bool {
        std::env::args().any(|a| a == "--remote" || a.starts_with("--remote="))
    }
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    static LIVE_CONNS: AtomicUsize = AtomicUsize::new(0);
    /// Grab request ids live in the top half of the id space, like the file
    /// sinks in `cx_shared.rs`, so they can never collide with studio ids.
    const GRAB_ID_BASE: u64 = 1 << 62;
    const MAX_LIVE_CONNS: usize = 24;
    const MAX_HEAD_BYTES: usize = 32 * 1024;
    const MAX_BODY_BYTES: usize = 1 << 20;
    const LOG_RING_CAP: usize = 4000;
    const GRABS_KEPT_PER_WINDOW: usize = 64;
    const MAX_PENDING_GRABS: usize = 64;
    const MAX_GRAB_BYTES: usize = 64 * 1024 * 1024;
    static PENDING_GRABS: AtomicUsize = AtomicUsize::new(0);
    static GRAB_BYTES: AtomicUsize = AtomicUsize::new(0);

    fn queue() -> &'static Mutex<Vec<Cmd>> {
        static Q: OnceLock<Mutex<Vec<Cmd>>> = OnceLock::new();
        Q.get_or_init(|| Mutex::new(Vec::new()))
    }

    fn status_cell() -> &'static Mutex<Status> {
        static S: OnceLock<Mutex<Status>> = OnceLock::new();
        S.get_or_init(|| Mutex::new(Status::default()))
    }

    fn grab_sinks() -> &'static Mutex<HashMap<u64, GrabSink>> {
        static G: OnceLock<Mutex<HashMap<u64, GrabSink>>> = OnceLock::new();
        G.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn log_ring() -> &'static Mutex<LogRing> {
        static L: OnceLock<Mutex<LogRing>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(LogRing::default()))
    }

    fn grab_dir() -> &'static Mutex<PathBuf> {
        static D: OnceLock<Mutex<PathBuf>> = OnceLock::new();
        D.get_or_init(|| Mutex::new(PathBuf::new()))
    }

    /// Windows the *human* dismissed, by id → title. Kept so a later request for
    /// that window can say why it is gone instead of "no window N", which reads
    /// like a crash.
    fn closed_windows() -> &'static Mutex<Vec<(usize, String)>> {
        static C: OnceLock<Mutex<Vec<(usize, String)>>> = OnceLock::new();
        C.get_or_init(|| Mutex::new(Vec::new()))
    }

    #[derive(Default)]
    struct Status {
        app: String,
        pid: u32,
        windows: Vec<WinInfo>,
    }

    #[derive(Clone, PartialEq)]
    struct WinInfo {
        id: usize,
        title: String,
        w: f64,
        h: f64,
        dpi: f64,
        x: f64,
        y: f64,
    }

    #[derive(Default)]
    struct LogRing {
        next_seq: u64,
        lines: std::collections::VecDeque<(u64, String)>,
    }

    struct GrabSink {
        window: Option<usize>,
        requested_at: Instant,
        scale: f64,
        cancelled: Arc<AtomicBool>,
        tx: SyncSender<Result<Grabbed, String>>,
    }

    impl Drop for GrabSink {
        fn drop(&mut self) {
            PENDING_GRABS.fetch_sub(1, Ordering::Relaxed);
        }
    }

    struct GrabBatch {
        window: WindowId,
        ids: std::collections::VecDeque<u64>,
        due: Instant,
        every: Duration,
        last_request: Option<u64>,
        cancelled: Arc<AtomicBool>,
    }

    #[derive(Default)]
    struct Captures {
        batches: Vec<GrabBatch>,
        windows: HashMap<u64, usize>,
        tickets: HashMap<ReadbackTicket, u64>,
        ready: Vec<(u64, TextureReadback)>,
        failures: Vec<(u64, String)>,
    }

    thread_local! {
        // This state belongs only to the UI, unlike the HTTP reply sinks.
        static CAPTURES: std::cell::RefCell<Captures> = Default::default();
        static FRAME_WAITERS: std::cell::RefCell<Vec<(u64, Sender<Reply>, Option<String>)>> = Default::default();
    }

    struct EncodeJob {
        sink: GrabSink,
        width: u32,
        height: u32,
        pixels: Arc<[u8]>,
        stride: usize,
        order: ReadbackChannelOrder,
        origin: ReadbackOrigin,
        capture_ms: f64,
        backend_png: bool,
        bytes: Option<GrabBytes>,
    }

    struct GrabBytes(usize);
    impl Drop for GrabBytes {
        fn drop(&mut self) {
            GRAB_BYTES.fetch_sub(self.0, Ordering::Relaxed);
        }
    }

    fn encode_queue() -> &'static Result<SyncSender<EncodeJob>, String> {
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

    fn submit_encode(mut job: EncodeJob, raw: bool) {
        if job.sink.cancelled.load(Ordering::Relaxed) {
            return;
        }
        let len = job.pixels.len();
        if GRAB_BYTES
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(len)
                    .filter(|total| *total <= MAX_GRAB_BYTES)
            })
            .is_err()
        {
            let _ = job.sink.tx.try_send(Err(
                "grab pixel budget full; retry at a slower cadence".into()
            ));
            return;
        }
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

    // ------------------------------------------------------------------
    // command queue
    // ------------------------------------------------------------------

    enum Cmd {
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
        Dump(Sender<Reply>),
        /// The task pool's one-line summary (workers, jobs, queue waits).
        PoolSummary(Sender<Reply>),
        Snap {
            window: Option<usize>,
            needle: String,
            all: bool,
            tx: Sender<Reply>,
        },
        Close {
            window: Option<usize>,
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

    enum Input {
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
    }

    #[derive(Clone, Copy, PartialEq)]
    enum MouseKind {
        Move,
        Down,
        Up,
        Scroll,
    }

    /// What `poll` hands back to a waiting HTTP thread.
    enum Reply {
        Ok,
        /// Deferred until the next rendered frame; the sender is re-armed by
        /// `poll` in a later tick.
        Text(String),
        Err(String),
    }

    /// `(target repaint_id, responder, payload)` — resolved once the app has
    /// drawn a frame that includes whatever the request did. `payload` is the
    /// JSON to answer with (a tweak op's result); `None` answers with the
    /// generic `{"ok":1,"f":N}` frame ack.
    /// Last hw-injected pointer position: hw moves carry deltas computed
    /// against it, like hardware events carry NSEvent deltas.
    fn hw_last() -> &'static Mutex<Option<crate::makepad_math::DVec2>> {
        static W: OnceLock<Mutex<Option<crate::makepad_math::DVec2>>> = OnceLock::new();
        W.get_or_init(|| Mutex::new(None))
    }

    // ------------------------------------------------------------------
    // startup
    // ------------------------------------------------------------------

    /// `--remote`, `--remote=PORT`, `--remote PORT`, `--remote=HOST:PORT`,
    /// `--remote HOST:PORT`, or `MAKEPAD_REMOTE=1|PORT|HOST:PORT`.
    ///
    /// The default host is loopback. Naming a host (`0.0.0.0:8399`,
    /// `10.0.0.5:8399`) binds that interface instead so another machine can
    /// drive this app — the fleet-box case, where the controlling agent sits
    /// on a different computer. Only do that on a trusted network: this
    /// surface injects real mouse/keyboard input and serves screen grabs.
    /// IPv4 or hostname only.
    fn requested_bind() -> Option<(String, u16)> {
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
    fn title_tag() -> Option<String> {
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

    fn app_name() -> String {
        std::env::args_os()
            .next()
            .as_ref()
            .and_then(|a| std::path::Path::new(a).file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("makepad")
            .to_string()
    }

    /// Bind the control port and start the accept loop. Prints
    /// `[makepad-remote] listening on HOST:PORT grabs=DIR` and flushes
    /// (HOST is `127.0.0.1` unless the bind named another interface).
    pub fn start_if_requested() {
        let Some((host, port)) = requested_bind() else {
            return;
        };
        if ACTIVE.load(Ordering::Relaxed) {
            return;
        }
        let listener = match TcpListener::bind((host.as_str(), port)) {
            Ok(l) => l,
            Err(err) => {
                println!("[makepad-remote] bind {host}:{port} failed: {err}");
                let _ = std::io::stdout().flush();
                return;
            }
        };
        let bound = match listener.local_addr() {
            Ok(addr) => addr.port(),
            Err(_) => return,
        };
        let app = app_name();
        let pid = std::process::id();
        let dir = std::env::temp_dir()
            .join("makepad-remote")
            .join(format!("{app}-{pid}"));
        let _ = std::fs::create_dir_all(&dir);
        *grab_dir().lock().unwrap() = dir.clone();
        {
            let mut status = status_cell().lock().unwrap();
            status.app = app.clone();
            status.pid = pid;
        }
        ACTIVE.store(true, Ordering::SeqCst);
        // One line, everything an agent needs to drive and clean up this
        // instance: port, pid, app, and where grabs land.
        println!(
            "[makepad-remote] listening on {host}:{bound} pid={pid} app={app} grabs={}",
            dir.display()
        );
        let _ = std::io::stdout().flush();

        std::thread::Builder::new()
            .name("makepad-remote".to_string())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    if LIVE_CONNS.load(Ordering::Relaxed) >= MAX_LIVE_CONNS {
                        let mut stream = stream;
                        let _ =
                            respond(&mut stream, 503, "application/json", b"{\"err\":\"busy\"}");
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

    pub fn is_active() -> bool {
        ACTIVE.load(Ordering::Relaxed)
    }

    /// True while a request is in flight, so the event loop knows to keep its
    /// paint clock at full rate instead of downshifting to the idle poll.
    /// Only macOS downshifts, so this is unused on the other backends — they
    /// poll the control channel at a fixed rate anyway.
    #[allow(dead_code)]
    #[allow(dead_code)]// only the macos paint clock asks
    pub(crate) fn needs_ticks() -> bool {
        if !ACTIVE.load(Ordering::Relaxed) {
            return false;
        }
        !queue().try_lock().map(|q| q.is_empty()).unwrap_or(false)
            || FRAME_WAITERS.with_borrow(|waiters| !waiters.is_empty())
            || PENDING_GRABS.load(Ordering::Relaxed) != 0
    }

    // ------------------------------------------------------------------
    // user-initiated window closes
    // ------------------------------------------------------------------

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

    fn close_requested() -> &'static Mutex<Vec<usize>> {
        static R: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();
        R.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// The human clicked a window's close button (or hit Cmd-W). Say so on
    /// stdout — with or without `--remote` — so anyone tailing the log can tell
    /// "the user dismissed this" apart from "the app crashed", and remember it
    /// so requests aimed at that window get the real reason.
    pub fn note_user_closed_window(window_id: usize, title: &str) {
        let line = format!("[makepad-remote] user closed window {window_id} ({title:?})");
        println!("{line}");
        let _ = std::io::stdout().flush();
        push_log_line(line);
        if let Ok(mut closed) = closed_windows().lock() {
            if !closed.iter().any(|(id, _)| *id == window_id) {
                closed.push((window_id, title.to_string()));
            }
        }
    }

    /// The window the human just closed was the last one, so the app is going
    /// away. Not a crash.
    pub fn note_user_closed_last_window() {
        let line = "[makepad-remote] app exit: user closed the last window".to_string();
        println!("{line}");
        let _ = std::io::stdout().flush();
        push_log_line(line);
    }

    fn window_gone_reason(window_id: usize) -> String {
        let closed = closed_windows().lock().ok();
        let hit = closed
            .as_ref()
            .and_then(|c| c.iter().find(|(id, _)| *id == window_id));
        match hit {
            Some(_) => format!("window {window_id} closed by user"),
            None => format!("no window {window_id}"),
        }
    }

    // ------------------------------------------------------------------
    // log ring (filled from log.rs)
    // ------------------------------------------------------------------

    pub fn push_log_line(line: String) {
        if !ACTIVE.load(Ordering::Relaxed) {
            return;
        }
        let Ok(mut ring) = log_ring().lock() else {
            return;
        };
        let seq = ring.next_seq + 1;
        ring.next_seq = seq;
        ring.lines.push_back((seq, line));
        while ring.lines.len() > LOG_RING_CAP {
            ring.lines.pop_front();
        }
    }

    // ------------------------------------------------------------------
    // grab plumbing, called from the screenshot pipeline
    // ------------------------------------------------------------------

    fn is_grab_id(id: u64) -> bool {
        // File sinks occupy 1<<63 and probes use 1<<40; preserve both.
        id >= GRAB_ID_BASE && id < (1 << 63)
    }

    /// True when a pending screenshot request may be answered by the pass that
    /// belongs to `window_id`. Non-remote (studio / file-sink) ids always match,
    /// so this is transparent to the existing pipeline.
    pub(crate) fn grab_targets_window(request_id: u64, window_id: Option<usize>) -> bool {
        if !is_grab_id(request_id) {
            return true;
        }
        // Called only while the renderer consumes screenshot_requests. Keep
        // targeting UI-owned so HTTP/GPU locks cannot defer the requested frame.
        CAPTURES.with_borrow_mut(|captures| match captures.windows.get(&request_id) {
            Some(want) if Some(*want) != window_id => false,
            _ => {
                captures.windows.remove(&request_id);
                true
            }
        })
    }

    /// Hand a finished PNG to whichever grab requests asked for it. Returns the
    /// ids that were *not* remote grabs, so the caller can route them onwards.
    /// Compatibility path for backends that still supply PNGs. The worker
    /// handles decoding/scaling; capture_kind makes their timing limitation
    /// explicit. Metal's raw path bypasses this full-size encoding entirely.
    pub(crate) fn deliver_grabs(
        request_ids: Vec<u64>,
        width: u32,
        height: u32,
        png: &[u8],
    ) -> Vec<u64> {
        if !ACTIVE.load(Ordering::Relaxed) {
            return request_ids;
        }
        let Ok(mut sinks) = grab_sinks().lock() else {
            return request_ids;
        };
        let mut rest = Vec::new();
        for id in request_ids {
            match sinks.remove(&id) {
                Some(sink) => {
                    let capture_ms = sink.requested_at.elapsed().as_secs_f64() * 1000.0;
                    submit_encode(
                        EncodeJob {
                            sink,
                            width,
                            height,
                            pixels: Arc::from(png),
                            stride: 0,
                            order: ReadbackChannelOrder::Rgba,
                            origin: ReadbackOrigin::TopLeft,
                            capture_ms,
                            backend_png: false,
                            bytes: None,
                        },
                        false,
                    );
                }
                None if !is_grab_id(id) => rest.push(id),
                None => {}
            }
        }
        rest
    }

    /// Metal calls this as soon as the presenting buffer's pixels are ready,
    /// before any PNG work. Return non-remote ids for probes/Studio/recording.
    #[cfg(all(
        not(gpusim),
        any(target_os = "macos", target_os = "ios", target_os = "tvos")
    ))]
    pub(crate) fn deliver_grab_pixels(
        request_ids: Vec<u64>,
        width: u32,
        height: u32,
        pixels: Arc<[u8]>,
        stride: usize,
        order: ReadbackChannelOrder,
        origin: ReadbackOrigin,
    ) -> Vec<u64> {
        if !ACTIVE.load(Ordering::Relaxed) {
            return request_ids;
        }
        let captured = Instant::now();
        // Only the GPU callback takes this blocking lock. The UI uses try_lock.
        let Ok(mut sinks) = grab_sinks().lock() else {
            return request_ids;
        };
        let mut rest = Vec::new();
        for id in request_ids {
            if let Some(sink) = sinks.remove(&id) {
                let capture_ms = captured.duration_since(sink.requested_at).as_secs_f64() * 1000.0;
                crate::trace!(
                    "remote.grab",
                    "pixels id={} window={:?} sz={}x{} capture_ms={:.3}",
                    id,
                    sink.window,
                    width,
                    height,
                    capture_ms
                );
                submit_encode(
                    EncodeJob {
                        sink,
                        width,
                        height,
                        pixels: pixels.clone(),
                        stride,
                        order,
                        origin,
                        capture_ms,
                        backend_png: false,
                        bytes: None,
                    },
                    true,
                );
            } else if !is_grab_id(id) {
                rest.push(id);
            }
        }
        rest
    }

    fn poll_captures(cx: &mut Cx) {
        CAPTURES.with_borrow_mut(|captures| {
            // On native platforms the legacy result lane is unused by apps
            // (its only other consumer is WebGL, where --remote is disabled).
            // Isolate our tickets there so an app's public try_take call cannot
            // consume them, and never drain the app's non-legacy tickets.
            if !captures.tickets.is_empty() {
                for (_, result) in cx.take_texture_readback_results(true) {
                    if let Some(id) = captures.tickets.remove(&result.ticket) {
                        captures.ready.push((id, result));
                    }
                }
            }
            if let Ok(mut sinks) = grab_sinks().try_lock() {
                for (id, error) in captures.failures.drain(..) {
                    if let Some(sink) = sinks.remove(&id) {
                        let _ = sink.tx.try_send(Err(error));
                    }
                }
                for (id, result) in captures.ready.drain(..) {
                    if let Some(sink) = sinks.remove(&id) {
                        let capture_ms = sink.requested_at.elapsed().as_secs_f64() * 1000.0;
                        match result.data {
                            Ok(pixels) => submit_encode(
                                EncodeJob {
                                    sink,
                                    width: result.width as u32,
                                    height: result.height as u32,
                                    pixels,
                                    stride: result.stride,
                                    order: result.channel_order,
                                    origin: result.origin,
                                    capture_ms,
                                    backend_png: false,
                                    bytes: None,
                                },
                                true,
                            ),
                            Err(err) => {
                                let _ = sink.tx.try_send(Err(err.to_string()));
                            }
                        }
                    }
                }
            }
            let now = Instant::now();
            captures.batches.retain_mut(|batch| {
                if batch.cancelled.load(Ordering::Relaxed) {
                    return false;
                }
                if now < batch.due {
                    return true;
                }
                if let Some(previous) = batch.last_request {
                    // Wait for the preceding frame to be ENCODED, not read
                    // back. A skipped/minimized drawable must not accumulate
                    // several sequence requests on one eventual presentation.
                    if cx
                        .screenshot_requests
                        .iter()
                        .any(|request| request.request_id == previous)
                        || captures.tickets.iter().any(|(ticket, id)| {
                            *id == previous
                                && cx
                                    .textures
                                    .1
                                    .readbacks
                                    .slots
                                    .iter()
                                    .any(|slot| slot.result.ticket == *ticket && slot.pending)
                        })
                    {
                        return true;
                    }
                }
                let Some(id) = batch.ids.front().copied() else {
                    return false;
                };
                if !cx.windows.is_valid(batch.window) || !cx.windows[batch.window].is_created {
                    if let Ok(mut sinks) = grab_sinks().try_lock() {
                        for id in batch.ids.drain(..) {
                            if let Some(sink) = sinks.remove(&id) {
                                let _ = sink.tx.try_send(Err("grab window closed".into()));
                            }
                        }
                        return false;
                    }
                    return true;
                }
                let texture = cx.windows[batch.window]
                    .main_pass_id
                    .and_then(|pass| cx.passes[pass].color_textures.first())
                    .map(|color| color.texture.clone());
                #[cfg(all(
                    not(gpusim),
                    any(target_os = "macos", target_os = "ios", target_os = "tvos")
                ))]
                let texture = if cx.in_makepad_studio { texture } else { None };
                let ticket = texture.and_then(|texture| {
                    texture
                        .read_back(cx, ReadbackRequest { next_render: true })
                        .ok()
                });
                if let Some(ticket) = ticket {
                    cx.textures
                        .1
                        .readbacks
                        .slots
                        .iter_mut()
                        .find(|slot| slot.result.ticket == ticket)
                        .unwrap()
                        .legacy = true;
                    captures.tickets.insert(ticket, id);
                } else {
                    // Native Metal drawables are not pooled Texture handles.
                    // The existing window path blits on the presenting buffer.
                    captures.windows.insert(id, batch.window.id());
                    cx.screenshot_requests.push(ScreenshotRequest {
                        request_id: id,
                        kind_id: 0,
                    });
                }
                batch.ids.pop_front();
                batch.last_request = Some(id);
                batch.due += batch.every;
                // Preserve draw-buffer tweaks at rest; the native boundary
                // also records any redraw already pending from earlier input.
                cx.request_remote_window_present(batch.window);
                crate::trace!(
                    "remote.grab",
                    "arm id={} window={} repaint={} late_ms={:.3}",
                    id,
                    batch.window.id(),
                    cx.repaint_id,
                    now.saturating_duration_since(batch.due - batch.every)
                        .as_secs_f64()
                        * 1000.0
                );
                !batch.ids.is_empty()
            });
        });
    }

    // ------------------------------------------------------------------
    // event-loop side
    // ------------------------------------------------------------------

    /// Drain the command queue and publish window state. Called from
    /// `Cx::poll_control_channel`, i.e. from the event loop of every backend.
    pub(crate) fn poll(cx: &mut Cx) {
        // The native Mac event callback supplies its renderer so it can seal
        // a grab BEFORE the next command. Hosted/gpusim backends retain
        // their ordinary next-render readback path.
        #[cfg(all(target_os = "macos", not(gpusim)))]
        if !cx.in_makepad_studio {
            return;
        }
        poll_with_present(cx, |_, _| None);
    }

    /// Returns whether a present is still waiting on its window's drawable
    /// (the caller schedules the next beat to poll it again).
    #[cfg(all(target_os = "macos", not(gpusim)))]
    pub(crate) fn poll_macos(cx: &mut Cx, present: impl FnMut(&mut Cx, WindowId) -> Option<bool>) -> bool {
        poll_with_present(cx, present)
    }

    #[cfg(all(target_os = "macos", not(gpusim)))]
    pub(crate) fn next_grab_deadline() -> Option<Instant> {
        CAPTURES.with_borrow(|captures| {
            captures
                .batches
                .iter()
                .filter(|batch| !batch.cancelled.load(Ordering::Relaxed))
                .map(|batch| batch.due)
                .min()
        })
    }

    fn poll_with_present(cx: &mut Cx, mut present: impl FnMut(&mut Cx, WindowId) -> Option<bool>) -> bool {
        if !ACTIVE.load(Ordering::Relaxed) {
            return false;
        }
        publish_windows(cx);
        let mut pending = false;

        let cmds: Vec<Cmd> = {
            match queue().try_lock() {
                Ok(mut q) => std::mem::take(&mut *q),
                Err(_) => Vec::new(),
            }
        };
        // A due sequence frame precedes commands dispatched on this wake.
        poll_captures(cx);
        pending |= present_captures(cx, &mut present);
        for cmd in cmds {
            poll_captures(cx);
            pending |= present_captures(cx, &mut present);
            let wait_window = match &cmd {
                Cmd::Input {
                    window, wait: true, ..
                } => Some(*window),
                Cmd::Tweak { wait: true, .. } | Cmd::Ai { wait: true, .. } => Some(None),
                _ => None,
            };
            apply(cx, cmd);
            // Do not batch later input ahead of a grab in this same drain.
            poll_captures(cx);
            pending |= present_captures(cx, &mut present);
            if let Some(window) = wait_window.and_then(|window| resolve_window(cx, window).ok()) {
                cx.request_remote_window_present(window);
                match present(cx, window) {
                    Some(false) => {
                        FRAME_WAITERS.with_borrow_mut(|waiters| {
                            for (_, tx, _) in waiters.drain(..) {
                                let _ = tx.send(Reply::Err(
                                    "requested input frame could not be submitted; retry".into(),
                                ));
                            }
                        });
                    }
                    // the pass stays dirty; the next beat paints it and the
                    // waiters resolve on its repaint
                    None => pending = true,
                    Some(true) => {}
                }
            }
            resolve_frame_waiters(cx);
        }

        poll_captures(cx);
        pending |= present_captures(cx, &mut present);
        resolve_frame_waiters(cx);
        pending
    }

    /// Returns whether a capture's present is waiting on its drawable.
    fn present_captures(cx: &mut Cx, present: &mut impl FnMut(&mut Cx, WindowId) -> Option<bool>) -> bool {
        let mut pending = false;
        let windows = CAPTURES.with_borrow(|captures| {
            let mut windows: Vec<usize> = captures.windows.values().copied().collect();
            windows.sort_unstable();
            windows.dedup();
            windows
        });
        for window in windows {
            let result = match resolve_window(cx, Some(window)) {
                Ok(window) => present(cx, window),
                Err(_) => Some(false),
            };
            if result.is_none() {
                pending = true;
                continue;
            }
            // A failed drawable must not silently move this capture across
            // later input. Fail closed; never wait on the UI for the GPU.
            CAPTURES.with_borrow_mut(|captures| {
                captures.windows.retain(|id, target| {
                    if *target != window {
                        return true;
                    }
                    cx.screenshot_requests
                        .retain(|request| request.request_id != *id);
                    captures.failures.push((
                        *id,
                        "grab frame could not be submitted at arming; retry".into(),
                    ));
                    false
                });
            });
        }
        pending
    }

    fn resolve_frame_waiters(cx: &Cx) {
        // Resolve anyone who asked to be answered after the next frame.
        let repaint_id = cx.repaint_id;
        FRAME_WAITERS.with_borrow_mut(|waiters| {
            waiters.retain(|(target, tx, payload)| {
                if repaint_id >= *target {
                    let text = match payload {
                        Some(payload) => payload.clone(),
                        None => format!("{{\"ok\":1,\"f\":{repaint_id}}}"),
                    };
                    let _ = tx.send(Reply::Text(text));
                    false
                } else {
                    true
                }
            })
        });
    }

    fn publish_windows(cx: &Cx) {
        let mut windows = Vec::new();
        for window_id in cx.windows.id_iter() {
            let window = &cx.windows[window_id];
            if !window.is_created {
                continue;
            }
            let geom = &window.window_geom;
            windows.push(WinInfo {
                id: window_id.id(),
                title: window.create_title.clone(),
                w: geom.inner_size.x,
                h: geom.inner_size.y,
                dpi: geom.dpi_factor,
                x: geom.position.x,
                y: geom.position.y,
            });
        }
        if let Ok(mut status) = status_cell().try_lock() {
            if status.windows != windows {
                status.windows = windows;
            }
        }
    }

    fn resolve_window(cx: &Cx, want: Option<usize>) -> Result<WindowId, String> {
        let mut first = None;
        for window_id in cx.windows.id_iter() {
            if !cx.windows[window_id].is_created {
                continue;
            }
            if first.is_none() {
                first = Some(window_id);
            }
            if let Some(want) = want {
                if window_id.id() == want {
                    return Ok(window_id);
                }
            }
        }
        match (want, first) {
            (Some(want), _) => Err(window_gone_reason(want)),
            (None, Some(first)) => Ok(first),
            (None, None) => Err("no windows".to_string()),
        }
    }

    fn apply(cx: &mut Cx, cmd: Cmd) {
        match cmd {
            Cmd::Input {
                window,
                inputs,
                wait,
                tx,
            } => {
                let window_id = match resolve_window(cx, window) {
                    Ok(window_id) => window_id,
                    Err(err) => {
                        let _ = tx.send(Reply::Err(err));
                        return;
                    }
                };
                let time = cx.seconds_since_app_start();
                crate::trace!(
                    "remote.grab",
                    "input window={} wait={} repaint={}",
                    window_id.id(),
                    wait,
                    cx.repaint_id
                );
                let mut input_result = None;
                for input in inputs {
                    // Hardware-faithful injection: the same platform path
                    // (and pointer-pin transform) physical events take.
                    if let Input::Mouse {
                        kind,
                        x,
                        y,
                        button,
                        dx,
                        dy,
                        mods,
                        hw: true,
                    } = input
                    {
                        let raw = dvec2(x, y);
                        match kind {
                            MouseKind::Move => {
                                let (seed, delta) = {
                                    let mut last = hw_last().lock().unwrap();
                                    let seed = last.unwrap_or(raw);
                                    let delta = dvec2(raw.x - seed.x, raw.y - seed.y);
                                    *last = Some(raw);
                                    (seed, delta)
                                };
                                cx.dispatch_hw_mouse_move(
                                    window_id,
                                    raw,
                                    delta,
                                    seed,
                                    mods.into_key_modifiers(),
                                    time,
                                );
                            }
                            MouseKind::Down => {
                                *hw_last().lock().unwrap() = Some(raw);
                                cx.dispatch_studio_msg(
                                    StudioToApp::MouseDown(RemoteMouseDown {
                                        time,
                                        x,
                                        y,
                                        button_raw_bits: 1 << button,
                                        modifiers: mods,
                                    }),
                                    window_id,
                                    dvec2(0.0, 0.0),
                                );
                            }
                            MouseKind::Up => {
                                cx.dispatch_hw_pin_release();
                                cx.dispatch_studio_msg(
                                    StudioToApp::MouseUp(RemoteMouseUp {
                                        time,
                                        x,
                                        y,
                                        button_raw_bits: 1 << button,
                                        modifiers: mods,
                                    }),
                                    window_id,
                                    dvec2(0.0, 0.0),
                                );
                            }
                            MouseKind::Scroll => {
                                cx.dispatch_studio_msg(
                                    StudioToApp::Scroll(RemoteScroll {
                                        time,
                                        x,
                                        y,
                                        sx: dx,
                                        sy: dy,
                                        is_mouse: true,
                                        modifiers: mods,
                                    }),
                                    window_id,
                                    dvec2(0.0, 0.0),
                                );
                            }
                        }
                        continue;
                    }
                    let msg = match input {
                        Input::Mouse {
                            kind,
                            x,
                            y,
                            button,
                            dx,
                            dy,
                            mods,
                            hw: _,
                        } => {
                            // Remote /click and /m are window-local layout points.
                            // dispatch_studio_msg calls stdin_pointer_abs ->
                            // dpi_override_scale and remaps native OS points into
                            // layout. Convert first so that remap restores the
                            // requested layout coordinate (saved scale 2.0 over
                            // native 1.3 would otherwise send x1193 to x775.45).
                            let native = cx.windows[window_id]
                                .layout_vec2d_to_native_points(dvec2(x, y));
                            match kind {
                                MouseKind::Move => StudioToApp::MouseMove(RemoteMouseMove {
                                    time,
                                    x: native.x,
                                    y: native.y,
                                    modifiers: mods,
                                }),
                                MouseKind::Down => StudioToApp::MouseDown(RemoteMouseDown {
                                    time,
                                    x: native.x,
                                    y: native.y,
                                    button_raw_bits: 1 << button,
                                    modifiers: mods,
                                }),
                                MouseKind::Up => StudioToApp::MouseUp(RemoteMouseUp {
                                    time,
                                    x: native.x,
                                    y: native.y,
                                    button_raw_bits: 1 << button,
                                    modifiers: mods,
                                }),
                                MouseKind::Scroll => StudioToApp::Scroll(RemoteScroll {
                                    time,
                                    x: native.x,
                                    y: native.y,
                                    sx: dx,
                                    sy: dy,
                                    is_mouse: true,
                                    modifiers: mods,
                                }),
                            }
                        }
                        Input::Key { down, code, mods } => {
                            let event = KeyEvent {
                                key_code: code,
                                is_repeat: false,
                                modifiers: mods.into_key_modifiers(),
                                time,
                            };
                            if down {
                                StudioToApp::KeyDown(event)
                            } else {
                                StudioToApp::KeyUp(event)
                            }
                        }
                        Input::Text(text) => StudioToApp::TextInput(TextInputEvent {
                            input: text,
                            replace_last: false,
                            was_paste: false,
                            ..Default::default()
                        }),
                        Input::Maximize => {
                            cx.push_unique_platform_op(crate::cx_api::CxOsOp::MaximizeWindow(window_id));
                            input_result = Some("{\"ok\":1,\"maximize\":1}".to_string());
                            cx.redraw_all();
                            continue;
                        }
                        Input::DropFile { path, x, y } => {
                            let size = cx.windows[window_id].window_geom.inner_size;
                            if x >= size.x || y >= size.y {
                                let _ = tx.send(Reply::Err(
                                    "drop coordinates are outside the window".into(),
                                ));
                                return;
                            }
                            let (handled, response) = inject_file_drop(cx, path, x, y);
                            let response = match response {
                                crate::event::DragResponse::None => "none",
                                crate::event::DragResponse::Copy => "copy",
                                crate::event::DragResponse::Link => "link",
                                crate::event::DragResponse::Move => "move",
                            };
                            input_result = Some(format!(
                                "{{\"ok\":1,\"drop_handled\":{handled},\"drag_response\":\"{response}\"}}"
                            ));
                            cx.redraw_all();
                            continue;
                        }
                    };
                    cx.dispatch_studio_msg(msg, window_id, dvec2(0.0, 0.0));
                }
                if wait {
                    FRAME_WAITERS.with_borrow_mut(|waiters| {
                        waiters.push((cx.repaint_id + 1, tx, input_result))
                    });
                } else {
                    let _ = tx.send(input_result.map_or(Reply::Ok, Reply::Text));
                }
            }
            Cmd::Grab {
                window,
                ids,
                started,
                every,
                cancelled,
                tx,
            } => {
                let window = match resolve_window(cx, window) {
                    Ok(window) => window,
                    Err(err) => {
                        let _ = tx.send(Reply::Err(err));
                        return;
                    }
                };
                CAPTURES.with_borrow_mut(|captures| {
                    captures.batches.push(GrabBatch {
                        window,
                        ids: ids.into(),
                        due: started,
                        every,
                        last_request: None,
                        cancelled,
                    })
                });
                let _ = tx.send(Reply::Ok);
            }
            Cmd::CancelGrabs(ids) => {
                cx.screenshot_requests
                    .retain(|request| !ids.contains(&request.request_id));
                CAPTURES.with_borrow_mut(|captures| {
                    captures
                        .batches
                        .retain(|batch| !batch.cancelled.load(Ordering::Relaxed));
                    for id in &ids {
                        captures.windows.remove(id);
                    }
                    for (ticket, id) in &captures.tickets {
                        if ids.contains(id) {
                            cx.cancel_texture_readback(*ticket);
                        }
                    }
                });
            }
            Cmd::Dump(tx) => {
                let dump = match cx.widget_tree_dump_callback {
                    Some(callback) => callback(cx),
                    None => String::new(),
                };
                let _ = tx.send(Reply::Text(dump));
            }
            Cmd::PoolSummary(tx) => {
                let _ = tx.send(Reply::Text(cx.task_pool_summary()));
            }
            Cmd::ShaderConsts { shader, tx } => {
                let _ = tx.send(Reply::Text(shader_consts_json(cx, shader)));
            }
            Cmd::ShaderConstPatch {
                shader,
                index,
                value,
                tx,
            } => {
                let id = crate::draw_shader::DrawShaderId { index: shader };
                let ok = match value {
                    Some(v) => cx.shader_const_patch(id, index, v),
                    None => cx.shader_const_reset(id, index),
                };
                if ok {
                    let _ = tx.send(Reply::Text(shader_consts_json(cx, Some(shader))));
                } else {
                    let _ = tx.send(Reply::Err(format!(
                        "no table constant {} on shader {}",
                        index, shader
                    )));
                }
            }
            Cmd::Snap {
                window,
                needle,
                all,
                tx,
            } => {
                let widgets = match cx.widget_snapshot_callback {
                    Some(callback) => callback(cx),
                    None => Vec::new(),
                };
                // Widget rects arrive in desktop coordinates (window position
                // already folded in). Remote input is window-local, so subtract
                // it back out — an agent must be able to feed a rect straight
                // into /click without doing arithmetic.
                let origins: Vec<(usize, f64, f64)> = cx
                    .windows
                    .id_iter()
                    .filter(|id| cx.windows[*id].is_created)
                    .map(|id| {
                        let pos = cx.windows[id].window_geom.position;
                        (id.id(), pos.x, pos.y)
                    })
                    .collect();
                let needle = needle.to_lowercase();
                let mut out = String::from("{\"s\":[");
                let mut first = true;
                for widget in &widgets {
                    if !all && (!widget.visible || widget.width <= 0 || widget.height <= 0) {
                        continue;
                    }
                    if let Some(want) = window {
                        if widget.window_index != want {
                            continue;
                        }
                    }
                    if !needle.is_empty() {
                        let hay = format!(
                            "{} {} {}",
                            widget.id,
                            widget.widget_type,
                            widget.text.clone().unwrap_or_default()
                        )
                        .to_lowercase();
                        if !hay.contains(&needle) {
                            continue;
                        }
                    }
                    let (ox, oy) = origins
                        .iter()
                        .find(|(id, _, _)| *id == widget.window_index)
                        .map(|(_, x, y)| (*x, *y))
                        .unwrap_or((0.0, 0.0));
                    if !first {
                        out.push(',');
                    }
                    first = false;
                    out.push_str(&format!(
                        "{{\"i\":{},\"ty\":{},\"r\":[{},{},{},{}],\"w\":{}",
                        json_str(&widget.id),
                        json_str(&widget.widget_type),
                        num(widget.x as f64 - ox),
                        num(widget.y as f64 - oy),
                        widget.width,
                        widget.height,
                        widget.window_index,
                    ));
                    out.push_str(&format!(
                        ",\"window_id\":{},\"enabled\":{}",
                        json_str(&widget.window_id),
                        widget.enabled,
                    ));
                    if let Some(text) = &widget.text {
                        if !text.is_empty() {
                            out.push_str(&format!(",\"t\":{}", json_str(text)));
                        }
                    }
                    if let Some(value) = &widget.value {
                        out.push_str(&format!(",\"val\":{}", json_str(value)));
                    }
                    if let Some(checked) = widget.checked {
                        out.push_str(&format!(",\"c\":{}", if checked { 1 } else { 0 }));
                    }
                    if let Some(selected) = &widget.selected {
                        out.push_str(&format!(",\"selected\":{}", json_str(selected)));
                    }
                    if !widget.visible {
                        out.push_str(",\"v\":0");
                    }
                    out.push('}');
                }
                out.push_str("]}");
                let _ = tx.send(Reply::Text(out));
            }
            Cmd::Close { window, tx } => {
                let window_id = match resolve_window(cx, window) {
                    Ok(window_id) => window_id,
                    Err(err) => {
                        let _ = tx.send(Reply::Err(err));
                        return;
                    }
                };
                let line = format!("[makepad-remote] remote closed window {}", window_id.id());
                println!("{line}");
                let _ = std::io::stdout().flush();
                push_log_line(line);
                let _ = tx.send(Reply::Ok);
                cx.push_unique_platform_op(crate::cx_api::CxOsOp::CloseWindow(window_id));
            }
            Cmd::Tweak { op, args, wait, tx } => {
                let result = match cx.tweak_callback {
                    Some(callback) => callback(cx, &op, &args),
                    None => Err("no tweaker (this app has no widgets ui root)".to_string()),
                };
                match result {
                    Ok(json) => {
                        if wait {
                            FRAME_WAITERS.with_borrow_mut(|waiters| {
                                waiters.push((cx.repaint_id + 1, tx, Some(json)))
                            });
                        } else {
                            let _ = tx.send(Reply::Text(json));
                        }
                    }
                    Err(msg) => {
                        let _ = tx.send(Reply::Err(msg));
                    }
                }
            }
            Cmd::Ai { op, args, wait, tx } => {
                let result = match cx.ai_callback {
                    Some(callback) => callback(cx, &op, &args),
                    None => {
                        Err("no AI overlay (this app does not link makepad-aichat)".to_string())
                    }
                };
                match result {
                    Ok(json) => {
                        if wait {
                            FRAME_WAITERS.with_borrow_mut(|waiters| {
                                waiters.push((cx.repaint_id + 1, tx, Some(json)))
                            });
                        } else {
                            let _ = tx.send(Reply::Text(json));
                        }
                    }
                    Err(msg) => {
                        let _ = tx.send(Reply::Err(msg));
                    }
                }
            }
            Cmd::Quit(tx) => {
                let line = "[makepad-remote] remote quit".to_string();
                println!("{line}");
                let _ = std::io::stdout().flush();
                push_log_line(line);
                // Answer before the loop tears down, so the caller always sees
                // the ack rather than a dropped connection.
                let _ = tx.send(Reply::Ok);
                cx.request_quit(crate::event::QuitReason::App);
            }
        }
    }

    // ------------------------------------------------------------------
    // HTTP
    // ------------------------------------------------------------------

    fn handle_conn(mut stream: TcpStream) {
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
        for line in lines {
            if let Some((name, value)) = line.split_once(':') {
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

    /// Called only while applying UI commands. These event markers never
    /// cross the HTTP command channel; only the path and coordinates do.
    fn inject_file_drop(
        cx: &mut Cx,
        path: String,
        x: f64,
        y: f64,
    ) -> (bool, crate::event::DragResponse) {
        use crate::event::{DragEvent, DragItem, DragResponse, DropEvent, Event};
        use crate::thread::lock_from_ui;
        use std::sync::Arc;

        let items = Arc::new(vec![DragItem::FilePath {
            path,
            internal_id: None,
        }]);
        let response = Arc::new(Mutex::new(DragResponse::None));
        cx.call_event_handler(&Event::Drag(DragEvent {
            modifiers: Default::default(),
            handled: Arc::new(Mutex::new(false)),
            abs: dvec2(x, y),
            items: items.clone(),
            response: response.clone(),
        }));
        cx.drag_drop.cycle_drag();
        let handled = Arc::new(Mutex::new(false));
        cx.call_event_handler(&Event::Drop(DropEvent {
            modifiers: Default::default(),
            handled: handled.clone(),
            abs: dvec2(x, y),
            items,
        }));
        cx.drag_drop.cycle_drag();
        cx.call_event_handler(&Event::DragEnd);
        cx.drag_drop.cycle_drag();
        let result = (*lock_from_ui(&handled), *lock_from_ui(&response));
        result
    }

    fn find_head_end(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|w| w == b"\r\n\r\n")
    }

    fn respond(
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
            431 => "Request Header Fields Too Large",
            503 => "Service Unavailable",
            _ => "Error",
        };
        let mut out = Vec::with_capacity(body.len() + 256);
        out.extend_from_slice(
            format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        );
        out.extend_from_slice(body);
        stream.write_all(&out)?;
        stream.flush()
    }

    enum Out {
        Json(u16, String),
        Text(u16, String),
        Png(Vec<u8>),
    }

    fn err(msg: &str) -> Out {
        Out::Json(404, format!("{{\"err\":{}}}", json_str(msg)))
    }

    fn route(method: &str, path: &str, p: &Params) -> Out {
        if method != "GET" && method != "POST" && method != "HEAD" {
            return Out::Json(400, "{\"err\":\"method\"}".to_string());
        }
        match path {
            "/" | "/help" => Out::Text(200, cheat_sheet()),
            "/s" | "/status" => route_status(p),
            "/g" | "/grab" => route_grab(p),
            "/gseq" => route_grab_sequence(p),
            "/gq" => route_grab_quit(p),
            "/m" | "/mouse" => route_mouse(p, None),
            "/click" => route_mouse(p, Some("click")),
            "/k" | "/key" => route_key(p),
            "/t" | "/text" => route_text(p),
            "/drop" => route_drop(p),
            "/log" => route_log(p),
            "/trace" => route_trace(p),
            "/d" | "/dump" => match ask(|tx| Cmd::Dump(tx), 4) {
                Reply::Text(text) => Out::Text(200, text),
                other => reply_to_out(other),
            },
            "/snap" => {
                let window = p.window();
                let needle = p.get(&["q", "query"]).unwrap_or_default().to_string();
                let all = p.flag(&["all"]);
                reply_to_out(ask(
                    move |tx| Cmd::Snap {
                        window,
                        needle,
                        all,
                        tx,
                    },
                    4,
                ))
            }
            "/close" => {
                let window = p.window();
                reply_to_out(ask(move |tx| Cmd::Close { window, tx }, 4))
            }
            // `/w?k=maximize`: the window's own maximise, answered after the
            // next drawn frame with `wait` (the maximise/occlusion proof).
            "/w" | "/window" => match p.get(&["k", "kind"]) {
                Some("maximize") | Some("max") => send_input(p.window(), vec![Input::Maximize], p.flag(&["wait"])),
                other => err(&format!("unknown window op {other:?}; k=maximize")),
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
            _ => err("no route"),
        }
    }

    /// `{"shaders":[{"id":N,"consts":[{"i":0,"name":..,"doc":..,"value":..,"initial":..,
    /// "min":..,"max":..,"step":..,"file":..,"line":..,"col":..}]}]}` — only shaders
    /// that carry table constants, or the one asked for.
    fn shader_consts_json(cx: &mut Cx, only: Option<usize>) -> String {
        fn opt_num(v: Option<f64>) -> String {
            match v {
                Some(v) => format!("{}", v),
                None => "null".to_string(),
            }
        }
        let mut entries: Vec<(usize, Vec<crate::draw_shader::DrawShaderTableConst>)> = Vec::new();
        for (id, sh) in cx.draw_shaders.shaders.iter().enumerate() {
            if only.is_some_and(|o| o != id) || sh.mapping.table_consts.is_empty() {
                continue;
            }
            entries.push((id, sh.mapping.table_consts.clone()));
        }
        let mut out = String::from("{\"shaders\":[");
        for (n, (id, consts)) in entries.iter().enumerate() {
            if n > 0 {
                out.push(',');
            }
            out.push_str(&format!("{{\"id\":{},\"consts\":[", id));
            for (i, tc) in consts.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let loc = cx.with_vm(|vm| vm.bx.code.ip_to_loc(tc.ip));
                let (file, line, col) = match loc {
                    Some(loc) => (loc.file, loc.line, loc.col),
                    None => (String::new(), 0, 0),
                };
                out.push_str(&format!(
                    "{{\"i\":{},\"name\":{},\"doc\":{},\"value\":{},\"initial\":{},\"min\":{},\"max\":{},\"step\":{},\"file\":{},\"line\":{},\"col\":{}}}",
                    i,
                    json_str(&tc.name),
                    json_str(&tc.doc),
                    tc.value,
                    tc.initial,
                    opt_num(tc.min),
                    opt_num(tc.max),
                    opt_num(tc.step),
                    json_str(&file),
                    line,
                    col
                ));
            }
            out.push_str("]}");
        }
        out.push_str("]}");
        out
    }

    fn cheat_sheet() -> String {
        let status = status_cell().lock().unwrap();
        let dir = grab_dir().lock().unwrap().display().to_string();
        format!(
            "makepad-remote  app={} pid={}  windows={}  grabs={}\n\
             all routes are GET; every answer is one line of JSON; x/y are layout points, window-local, y down\n\
             /                 this sheet\n\
             /s[?w=ID]         {{\"app\":..,\"pid\":..,\"w\":[{{\"i\":id,\"t\":title,\"sz\":[w,h],\"px\":[w,h],\"dpi\":f,\"pos\":[x,y]}}]}}\n\
             \x20                 a window the HUMAN closed is reported as {{\"err\":\"window N closed by user\"}} — not a crash, do not relaunch\n\
             /g?w=&scale=&raw= grab window w (default: first). returns {{\"png\":path,\"w\":id,\"sz\":[w,h],\"capture_ms\":ms,\"encode_ms\":ms}}; raw=1 sends image/png bytes\n\
             \x20                 standalone macOS: pending Draw + immediate present at UI arming, before later input; no animation tick. Other backends: next render\n\
             /gseq?n=8&every_ms=50&scale=1  a separate present per deadline; n=1..64, every_ms>=8, span<=60s; {{\"png\":[paths],\"frames\":[per-frame timings]}}\n\
             \x20                 scheduled_ms and pixels_ms share the request origin; capture_ms = pixels_ms - scheduled_ms; cadence never waits for PNG encoding\n\
             \x20                 commands follow UI queue order (concurrent sockets have no client-time order); macOS wait=1 input replies after submitting its applied frame\n\
             /m?k=&x=&y=&w=    mouse. k=move|down|up|click|scroll  b=0 left,1 right,2 middle  scroll: dx=,dy=\n\
                               add hw=1 to take the hardware pointer path (pointer-lock/pin transform included)\n\
             /click?x=&y=      alias for /m?k=click\n\
             /k?t=TEXT         type text. or /k?k=down|up&c=KeyA (Escape ReturnKey Tab Backspace ArrowLeft F1 Key1 ..)\n\
             /t?t=TEXT         same as /k?t=\n\
             /drop?path=&x=&y= drop one absolute file path through Drag/Drop/DragEnd; optional w= and wait=1; app validates/loads it\n\
             /log?n=50         {{\"n\":lastseq,\"l\":[lines]}}; /log?since=N for everything after seq N\n\
             /trace             get topics; ?topics=gpu.pass,wm sets them; ?off=1 clears them\n\
             /snap?q=&w=&all=  widget rects, ready to click: {{\"s\":[{{\"i\":id,\"ty\":type,\"r\":[x,y,w,h],\"w\":win,\"t\":text}}]}}\n\
             \x20                 q= filters id/type/text (substring); default lists only visible, sized widgets\n\
             /d                whole widget tree as indented text (id, type, x y w h)\n\
             /tweak?on=1|0     the TWEAKER design-feedback overlay (also Shift+F10 in-app). hover outlines widgets; click pins; buttons never fire\n\
             /tweak/state      selection + its editable properties + diff log + annotations, one JSON\n\
             /tweak/apply      POST {{\"path\":\"a.b.c\",\"splash\":\"{{padding: 20}}\"}} or {{\"path\":..,\"prop\":\"padding\",\"value\":\"20\"}} — live-apply + relayout\n\
             /tweak/diff       the raw edit log; POST /tweak/clear resets it\n\
             /tweak/final      coalesced end state per widget (original -> final); adds \"png\" when the user drew\n\
             /tweak/grab       window grab with the overlay composited (same as /g while tweaking)\n\
             /shader/consts    the compiled shaders' hot-patchable constants (annotated literals): {{\"shaders\":[{{\"id\",\"consts\":[{{\"i\",\"name\",\"value\",\"min\",\"max\",\"step\",\"file\",\"line\"}}]}}]}}; shader=ID for one\n\
             /shader/const     ?shader=ID&i=N&v=VALUE patches one constant on the GPU (no recompile, source untouched); reset=1 puts the literal back\n\
             /close?w=ID       close one window the normal way\n\
             /gq[?scale=&w=]   FINISH HERE: grab every window, then quit. {{\"png\":[paths],\"quit\":1}}\n\
             /quit             shut the app down gracefully (no final grab)\n\
             if you launched this app, you MUST end with /gq (or /quit) — never leave test windows on the user's screen, never pkill\n\
             add &wait=1 to any input route to answer only after the next frame is drawn (so a following /g sees it)\n\
             add &w=ID to target a window; omit for the first one. errors are {{\"err\":\"...\"}} with status 404\n\
             POST the same routes with a flat JSON body ({{\"x\":10,\"y\":20}}) when quoting query strings is painful\n",
            status.app,
            status.pid,
            status.windows.len(),
            dir,
        )
    }

    fn route_trace(p: &Params) -> Out {
        if p.flag(&["off"]) {
            crate::makepad_error_log::set_trace_topics("");
        } else if let Some(topics) = p.get(&["topics"]) {
            crate::makepad_error_log::set_trace_topics(topics);
        }
        Out::Json(
            200,
            format!(
                "{{\"topics\":{}}}",
                json_str(&crate::makepad_error_log::trace_topics())
            ),
        )
    }

    fn route_ai(op: &str, p: &Params, wait: bool) -> Out {
        let op = op.to_string();
        let args = p.0.clone();
        let timeout = if wait { 6 } else { 4 };
        reply_to_out(ask(move |tx| Cmd::Ai { op, args, wait, tx }, timeout))
    }

    fn route_tweak(op: &str, p: &Params, wait: bool) -> Out {
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
    fn route_tweak_final(p: &Params) -> Out {
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

    fn route_status(p: &Params) -> Out {
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
        out.push('}');
        Out::Json(200, out)
    }

    fn route_mouse(p: &Params, force_kind: Option<&str>) -> Out {
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
        let mouse = |kind| Input::Mouse {
            kind,
            x,
            y,
            button,
            dx,
            dy,
            mods,
            hw,
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
            other => return err(&format!("bad kind {other}")),
        };
        send_input(window, inputs, p.flag(&["wait"]))
    }

    fn route_key(p: &Params) -> Out {
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

    fn route_text(p: &Params) -> Out {
        let Some(text) = p.get(&["t", "text"]) else {
            return err("need t=");
        };
        send_input(
            p.window(),
            vec![Input::Text(text.to_string())],
            p.flag(&["wait"]),
        )
    }

    fn send_input(window: Option<usize>, inputs: Vec<Input>, wait: bool) -> Out {
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

    fn parse_drop(p: &Params) -> Result<(Option<usize>, Input, bool), &'static str> {
        // Unlike permissive mouse aliases, file input must not silently
        // ignore a typo or choose between duplicate parameters.
        let mut seen = Vec::new();
        for (key, _) in &p.0 {
            let key = match key.as_str() {
                "path" | "x" | "y" | "wait" => key.as_str(),
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

    fn route_drop(p: &Params) -> Out {
        match parse_drop(p) {
            Ok((window, input, wait)) => send_input(window, vec![input], wait),
            Err(message) => Out::Json(400, format!("{{\"err\":{}}}", json_str(message))),
        }
    }

    fn route_log(p: &Params) -> Out {
        // Asked before the ring is locked: the answer comes from the UI thread.
        let pool = match ask(Cmd::PoolSummary, 2) {
            Reply::Text(text) => text,
            _ => String::new(),
        };
        Out::Json(200, log_json(p, &pool))
    }

    // The recorder exercises the same /log serialization without binding a
    // socket or starting an app. Only this gpusim process's ring is enabled.
    #[cfg(gpusim)]
    pub fn gpusim_start_log_capture() {
        ACTIVE.store(true, Ordering::Relaxed);
    }

    #[cfg(gpusim)]
    pub fn gpusim_log_snapshot() -> String {
        log_json(&Params(vec![("since".into(), "0".into())]), "")
    }

    fn log_json(p: &Params, pool: &str) -> String {
        let ring = log_ring().lock().unwrap();
        let since = p.get(&["since"]).and_then(|v| v.parse::<u64>().ok());
        let count = p
            .get(&["n", "count", "tail"])
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(50);
        let mut selected: Vec<&(u64, String)> = match since {
            Some(since) => ring.lines.iter().filter(|(seq, _)| *seq > since).collect(),
            None => ring.lines.iter().collect(),
        };
        if since.is_none() && selected.len() > count {
            selected = selected.split_off(selected.len() - count);
        }
        let mut out = format!(
            "{{\"n\":{},\"pool\":{},\"l\":[",
            ring.next_seq,
            json_str(pool)
        );
        for (index, (_, line)) in selected.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&json_str(line));
        }
        out.push_str("]}");
        out
    }

    struct Grabbed {
        window_id: usize,
        width: u32,
        height: u32,
        png: Vec<u8>,
        capture_ms: f64,
        encode_ms: f64,
        backend_png: bool,
        _bytes: Option<GrabBytes>,
    }

    struct PendingGrabs {
        ids: Vec<u64>,
        replies: Vec<Receiver<Result<Grabbed, String>>>,
        cancelled: Arc<AtomicBool>,
        deadline: Instant,
        paths: Vec<PathBuf>,
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
                .push(Cmd::CancelGrabs(self.ids.clone()));
            wake_commands();
        }
    }

    fn arm_grabs(
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
            Reply::Text(_) => Err("unexpected grab reply".into()),
        }
    }

    fn receive_grab(pending: &PendingGrabs, index: usize) -> Result<Grabbed, String> {
        pending.replies[index]
            .recv_timeout(pending.deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| "grab timeout (is this backend rendering?)".to_string())?
    }

    fn grab_one(window: Option<usize>, scale: f64) -> Result<Grabbed, String> {
        let pending = arm_grabs(window, scale, 1, Duration::ZERO)?;
        receive_grab(&pending, 0)
    }

    fn grab_json(grabbed: &Grabbed, path: &std::path::Path) -> String {
        format!(
            "{{\"png\":{},\"w\":{},\"sz\":[{},{}],\"capture_ms\":{:.3},\"encode_ms\":{:.3},\"capture_kind\":{}}}",
            json_str(&path.display().to_string()), grabbed.window_id, grabbed.width, grabbed.height,
            grabbed.capture_ms, grabbed.encode_ms,
            json_str(if grabbed.backend_png { "backend_png" } else { "pixels" }),
        )
    }

    fn route_grab(p: &Params) -> Out {
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

    fn route_grab_sequence(p: &Params) -> Out {
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
    fn route_grab_quit(p: &Params) -> Out {
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
        // Quit regardless: a failed grab must never leave the app on screen.
        let quit = matches!(ask(|tx| Cmd::Quit(tx), 4), Reply::Ok);
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
    fn pinned_grabs() -> &'static Mutex<std::collections::HashSet<PathBuf>> {
        static PINS: OnceLock<Mutex<std::collections::HashSet<PathBuf>>> = OnceLock::new();
        PINS.get_or_init(Default::default)
    }

    fn write_grab(window_id: usize, png: &[u8]) -> Result<PathBuf, String> {
        write_grab_file(window_id, png, false)
    }

    fn write_grab_file(window_id: usize, png: &[u8], pin: bool) -> Result<PathBuf, String> {
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

    fn encode_grab(job: &EncodeJob) -> Result<(u32, u32, Vec<u8>), String> {
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
                ReadbackChannelOrder::Rgba,
                ReadbackOrigin::TopLeft,
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
            let src_y = if origin == ReadbackOrigin::BottomLeft {
                height - 1 - src_y
            } else {
                src_y
            };
            for x in 0..target_w {
                let src = src_y * stride + (x * width / target_w) * 4;
                let dst = (y * target_w + x) * 4;
                out[dst..dst + 4].copy_from_slice(&pixels[src..src + 4]);
                if order == ReadbackChannelOrder::Bgra {
                    out.swap(dst, dst + 2);
                }
            }
        }
        let bytes = Cx::encode_rgba_as_png(target_w as u32, target_h as u32, &out)?;
        Ok((target_w as u32, target_h as u32, bytes))
    }

    fn wake_commands() {
        // SignalToUI coalesces wakes until timer 0 clears its flag. A remote
        // capture must also wake between those ticks (including 200 ms idle).
        #[cfg(all(target_os = "macos", not(gpusim)))]
        crate::os::apple::macos::macos_app::wake_event_loop();
        #[cfg(not(all(target_os = "macos", not(gpusim))))]
        crate::thread::SignalToUI::set_ui_signal();
    }

    /// Queue a command and block this HTTP thread until the event loop answers.
    fn ask<F>(make: F, timeout_secs: u64) -> Reply
    where
        F: FnOnce(Sender<Reply>) -> Cmd,
    {
        let (tx, rx) = channel();
        queue().lock().unwrap().push(make(tx));
        wake_commands();
        match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
            Ok(reply) => reply,
            Err(_) => Reply::Err("timeout (app busy or not running its event loop)".to_string()),
        }
    }

    fn reply_to_out(reply: Reply) -> Out {
        match reply {
            Reply::Ok => Out::Json(200, "{\"ok\":1}".to_string()),
            Reply::Text(text) => Out::Json(200, text),
            Reply::Err(msg) => err(&msg),
        }
    }

    // ------------------------------------------------------------------
    // parameters
    // ------------------------------------------------------------------

    struct Params(Vec<(String, String)>);

    impl Params {
        fn get(&self, keys: &[&str]) -> Option<&str> {
            for key in keys {
                if let Some((_, value)) = self.0.iter().find(|(k, _)| k == key) {
                    return Some(value.as_str());
                }
            }
            None
        }
        fn f64(&self, keys: &[&str], default: f64) -> f64 {
            self.get(keys)
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(default)
        }
        fn flag(&self, keys: &[&str]) -> bool {
            match self.get(keys) {
                None => false,
                Some(v) => !matches!(v, "0" | "false" | "no" | "off"),
            }
        }
        fn window(&self) -> Option<usize> {
            self.get(&["w", "window"])
                .and_then(|v| v.parse::<usize>().ok())
        }
        fn mods(&self) -> RemoteKeyModifiers {
            RemoteKeyModifiers {
                shift: self.flag(&["shift"]),
                control: self.flag(&["ctrl", "control"]),
                alt: self.flag(&["alt", "option"]),
                logo: self.flag(&["cmd", "logo", "meta", "super"]),
            }
        }
    }

    fn parse_query(query: &str) -> Vec<(String, String)> {
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

    fn percent_decode(input: &str) -> String {
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
    fn parse_flat_json(body: &str) -> Vec<(String, String)> {
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

    // ------------------------------------------------------------------
    // formatting helpers
    // ------------------------------------------------------------------

    fn json_str(input: &str) -> String {
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
    fn num(value: f64) -> String {
        if value.fract() == 0.0 && value.abs() < 1.0e15 {
            format!("{}", value as i64)
        } else {
            let rounded = (value * 1000.0).round() / 1000.0;
            format!("{rounded}")
        }
    }

    fn parse_key_code(name: &str) -> Option<KeyCode> {
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
        fn file_drop_dispatches_native_sequence_and_clears_drag_area() {
            use crate::area::{Area, RectArea};
            use crate::draw_list::{CxRectArea, DrawList};
            use crate::event::{DragHit, DragItem, DragResponse, DragState, Event};
            use crate::makepad_math::Rect;
            use crate::thread::lock_from_ui;
            use std::{
                cell::{Cell, RefCell},
                rc::Rc,
            };

            let area = Rc::new(Cell::new(Area::Empty));
            let target = area.clone();
            let sequence = Rc::new(RefCell::new(Vec::new()));
            let seen = sequence.clone();
            let mut cx = Cx::new(Box::new(move |cx, event| {
                seen.borrow_mut().push(event.name());
                match event.drag_hits(cx, target.get()) {
                    DragHit::Drag(hit) => {
                        assert_eq!(
                            hit.state,
                            DragState::In,
                            "previous drop left stale drag state"
                        );
                        assert!(
                            matches!(hit.items.as_slice(), [DragItem::FilePath { path, internal_id: None }] if path == "/not-read-by-platform/reference.png")
                        );
                        *lock_from_ui(&hit.response) = DragResponse::Copy;
                    }
                    DragHit::Drop(hit) => assert_eq!(hit.abs, dvec2(20.0, 30.0)),
                    DragHit::DragEnd | DragHit::NoHit => assert!(matches!(event, Event::DragEnd)),
                }
            }));
            let draw_list = DrawList::new(&mut cx);
            let list = &mut cx.draw_lists[draw_list.id()];
            list.rect_areas.push(CxRectArea {
                rect: Rect {
                    pos: dvec2(10.0, 10.0),
                    size: dvec2(100.0, 100.0),
                },
                draw_clip: (dvec2(0.0, 0.0), dvec2(200.0, 200.0)),
            });
            area.set(Area::Rect(RectArea {
                draw_list_id: draw_list.id(),
                rect_id: 0,
                redraw_id: list.redraw_id,
            }));
            for _ in 0..2 {
                assert_eq!(
                    inject_file_drop(
                        &mut cx,
                        "/not-read-by-platform/reference.png".into(),
                        20.0,
                        30.0
                    ),
                    (true, DragResponse::Copy)
                );
            }
            assert_eq!(
                sequence.borrow().as_slice(),
                ["Drag", "Drop", "DragEnd", "Drag", "Drop", "DragEnd"]
            );
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
}

#[cfg(any(target_arch = "wasm32", target_os = "android", target_env = "ohos"))]
mod imp {
    use crate::cx::Cx;

    pub fn start_if_requested() {}
    pub fn is_active() -> bool {
        false
    }
    #[allow(dead_code)] // only the macos paint clock asks
    pub(crate) fn needs_ticks() -> bool {
        false
    }
    pub fn push_log_line(_line: String) {}
    pub fn note_user_closed_window(_window_id: usize, _title: &str) {}
    pub fn note_user_closed_last_window() {}
    pub fn note_window_close_requested(_window_id: usize) {}
    pub fn take_window_close_requested(_window_id: usize) -> bool {
        false
    }
    pub fn tag_window_title(title: String) -> String {
        title
    }
    pub(crate) fn poll(_cx: &mut Cx) {}
    pub(crate) fn grab_targets_window(_request_id: u64, _window_id: Option<usize>) -> bool {
        true
    }
    pub(crate) fn deliver_grabs(
        request_ids: Vec<u64>,
        _width: u32,
        _height: u32,
        _png: &[u8],
    ) -> Vec<u64> {
        request_ids
    }
}

pub use imp::*;
