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

#[path = "app_clock.rs"]
pub mod app_clock;
#[path = "synthetic_cursor.rs"]
pub mod synthetic_cursor;

/// Marks synchronous injected dispatch, including hardware-path mouse input and
/// nested events. Native input delivered on a later event-loop turn stays native.
/// The flag is the `Cx`'s (`RemoteActivity::remote_input`); the guard holds a
/// handle to it so the dispatch it brackets is free to borrow the `Cx`.
#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
pub(crate) struct RemoteInputScope {
    origin: std::rc::Rc<std::cell::Cell<bool>>,
    previous: bool,
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
pub(crate) fn remote_input_scope(cx: &crate::cx::Cx) -> RemoteInputScope {
    let origin = cx.remote_activity.remote_input.clone();
    let previous = origin.replace(true);
    RemoteInputScope { origin, previous }
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
impl Drop for RemoteInputScope {
    fn drop(&mut self) {
        self.origin.set(self.previous);
    }
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
#[path = "remote_activity.rs"]
mod activity;

#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
#[path = "remote_capture.rs"]
mod capture;

#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
mod imp {
    use super::activity;
    use super::app_clock;
    use super::capture;
    use super::synthetic_cursor;
    pub(crate) use activity::note_user_event;
    pub(crate) use activity::RemoteActivity;
    use crate::cx::Cx;
    use crate::cx_api::CxOsApi;
    use crate::makepad_math::{dvec2, Vec2d};
    use crate::texture::{
        ReadbackChannelOrder, ReadbackOrigin, ReadbackRequest, ReadbackTicket, TextureReadback,
    };
    use crate::window::WindowId;
    use makepad_studio_protocol::{
        KeyEvent, RemoteMouseDown, RemoteMouseMove,
        RemoteMouseUp, RemotePinch, RemoteScroll, ScreenshotRequest, StudioToApp, TextInputEvent,
        WidgetSnapshot,
    };
    use std::collections::HashMap;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};
    use makepad_remote_core::*;
    pub use makepad_remote_core::{
        is_active, note_window_close_requested, requested, tag_window_title,
        take_window_close_requested,
    };

    /// Commands built by the platform-side routes; they travel through the
    /// core queue as `Cmd::Platform`.
    enum PlatformCmd {
        CapStart {
            window: usize,
            config: capture::CaptureConfig,
            session: capture::NewSession,
            tx: Sender<Reply>,
        },
        /// Show (`Some(Some(style))`), hide (`Some(None)`) or just place
        /// (`None`) the synthetic cursor.
        Cursor {
            style: Option<Option<synthetic_cursor::SyntheticCursorStyle>>,
            window: Option<usize>,
            pos: Option<Vec2d>,
            tx: Sender<Reply>,
        },
    }

    /// The routes that need platform state (see `makepad_remote_core::route`).
    fn platform_route(path: &str, p: &Params) -> Option<Out> {
        Some(match path {
            "/log" => route_log(p),
            "/midi" => route_midi(p),
            "/cap/start" => route_capture_start(p),
            "/cap/stop" => route_capture_stop(p),
            "/cursor" => route_cursor(p),
            _ => return None,
        })
    }

    fn raw_grab_order(order: ReadbackChannelOrder) -> GrabOrder {
        match order {
            ReadbackChannelOrder::Rgba => GrabOrder::Rgba,
            ReadbackChannelOrder::Bgra => GrabOrder::Bgra,
        }
    }

    fn raw_grab_origin(origin: ReadbackOrigin) -> GrabOrigin {
        match origin {
            ReadbackOrigin::TopLeft => GrabOrigin::TopLeft,
            ReadbackOrigin::BottomLeft => GrabOrigin::BottomLeft,
        }
    }




    /// Is the bridge driving right now, as far as a person watching the
    /// window should be told?
    ///
    /// Read by app chrome on every frame: a scripted run injects a burst of
    /// events and then thinks, so this stays true for a few seconds after
    /// the last one rather than flickering off between them. Because it goes
    /// quiet on its own, a caller that draws something from it must keep
    /// asking for frames until it does.
    pub fn hands_off_active() -> bool {
        // The marker is for a person watching the window. A hidden window has
        // no watcher, and there the frame only lands in the grabs, where it
        // changes what a pixel test or a vision check sees from one capture
        // to the next (present within three seconds of a click, absent after).
        static HIDDEN: OnceLock<bool> = OnceLock::new();
        if *HIDDEN.get_or_init(|| std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some()) {
            return false;
        }
        // Nor does it belong in an in-process capture of the window.
        if capture::capturing() {
            return false;
        }
        if HANDS_OFF.load(Ordering::Relaxed) {
            return true;
        }
        let at = INJECTED_AT_MS.load(Ordering::Relaxed);
        at != 0 && uptime_ms().saturating_sub(at) < HANDS_OFF_LINGER_MS
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
        static FRAME_WAITERS: std::cell::RefCell<Vec<(u64, Sender<Reply>, Option<String>, u64)>> = Default::default();
        /// The `/step` in progress, if any: UI-owned, like the captures.
        static STEP: std::cell::RefCell<Option<StepJob>> = const { std::cell::RefCell::new(None) };
        /// The `/cap/start` session as the UI sees it.
        static CAPTURE: std::cell::RefCell<Option<UiCapture>> = const { std::cell::RefCell::new(None) };
    }






    // ------------------------------------------------------------------
    // command queue
    // ------------------------------------------------------------------







    /// A `/step` in progress (UI thread only).
    struct StepJob {
        window: WindowId,
        fps: u32,
        remaining: u64,
        open: Option<OpenFrame>,
        /// Since when the step has been waiting on the capture encoder.
        wait_since: Option<Instant>,
        /// `wait_loads=1`: no frame opens while asynchronous loads (image
        /// decodes, glyph rasters, fetched resources) are in flight.
        wait_loads: bool,
        loads_since: Option<Instant>,
        tx: Sender<Reply>,
    }

    /// The step frame being presented: advanced, dispatched, not yet on the
    /// GPU (a drawable or a shader pipeline may still be on its way).
    struct OpenFrame {
        repaint_id: u64,
        /// The capture request id and the frame index it carries.
        capture: Option<(u64, u64)>,
        opened: Instant,
        prepared: bool,
    }

    /// The capture session as the UI sees it.
    struct UiCapture {
        session: u64,
        window: usize,
        virtual_clock: bool,
        fps: u32,
        /// Step frames given a capture request.
        armed: u64,
        /// Armed frames given up on (never presented, or never read back).
        abandoned: u64,
        /// The worker's failure has been reported to a `/step`.
        failure_reported: bool,
        tx: crate::makepad_network::mpsc::Sender<capture::CaptureMsg>,
        counters: Arc<capture::CaptureCounters>,
        tap: Option<u64>,
    }

    /// How long a step frame waits for shader pipelines still compiling
    /// before it is presented without them (logged).
    const STEP_SHADER_WAIT: Duration = Duration::from_secs(10);
    /// How long one step frame may fail to reach the screen, or a step may
    /// wait on the capture encoder, before `/step` gives up with an error.
    const STEP_FRAME_LIMIT: Duration = Duration::from_secs(20);

    /// How long `/step?wait_loads=1` waits for asynchronous loads before a
    /// frame, before it fails naming them.
    const STEP_LOADS_LIMIT: Duration = Duration::from_secs(30);

    fn loads_text(loads: &[(&'static str, usize)]) -> String {
        loads
            .iter()
            .map(|(kind, count)| format!("{kind}={count}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// What a `/step` must wait for from a running capture.
    enum CaptureGate {
        Ready,
        Wait,
        Failed(String),
    }

    /// `last`: every step frame is presented; the captured ones must also
    /// have left the GPU. Otherwise: may another frame be armed, or is the
    /// encoder too far behind?
    fn capture_gate(last: bool) -> CaptureGate {
        CAPTURE.with_borrow_mut(|capture| {
            let Some(c) = capture.as_mut().filter(|c| c.virtual_clock) else {
                return CaptureGate::Ready;
            };
            if let Some(error) = c.counters.failure() {
                if c.failure_reported {
                    // the capture is dead; steps go on uncaptured
                    return CaptureGate::Ready;
                }
                c.failure_reported = true;
                return CaptureGate::Failed(error);
            }
            let lagging = if last {
                c.counters.delivered.load(Ordering::Acquire) + c.abandoned < c.armed
            } else {
                c.armed.saturating_sub(c.counters.consumed.load(Ordering::Acquire))
                    >= capture::MAX_FRAMES_AHEAD
            };
            if lagging {
                CaptureGate::Wait
            } else {
                CaptureGate::Ready
            }
        })
    }

    /// Which half of a remote present the event loop is asked for.
    #[derive(Clone, Copy, PartialEq, Debug)]
    pub(crate) enum PresentStage {
        /// Draw what is pending and say whether the GPU can render all of it
        /// now: `Some(true)` yes, `Some(false)` not yet (pipelines compiling).
        Prepare,
        /// Submit the window's frame: `Some(true)` submitted, `Some(false)`
        /// failed, `None` waiting on a drawable (poll again next beat).
        Present,
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





    /// Bind the control port and start the accept loop. Prints
    /// `[makepad-remote] listening on HOST:PORT grabs=DIR` and flushes
    /// (HOST is `127.0.0.1` unless the bind named another interface).
    pub fn start_if_requested(cx: &mut Cx) {
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
            status.user_seq = cx.remote_activity.seq_handle();
        }
        ACTIVE.store(true, Ordering::SeqCst);
        // One line, everything an agent needs to drive and clean up this
        // instance: port, pid, app, and where grabs land.
        println!(
            "[makepad-remote] listening on {host}:{bound} pid={pid} app={app} grabs={}",
            dir.display()
        );
        let _ = std::io::stdout().flush();

        set_platform_hooks(PlatformHooks {
            wake: wake_commands,
            route: platform_route,
        });
        serve(listener);
    }


    /// True while a remote request (input, a grab or a grab sequence, a
    /// step) is in flight: an app that paces its own frames when idle runs
    /// at full rate for it, so a capture sees a live, ticking world.
    pub fn request_in_flight() -> bool {
        needs_ticks()
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
            || STEP.with_borrow(|step| step.is_some())
            || CAPTURE.with_borrow(|capture| capture.as_ref().is_some_and(|c| !c.virtual_clock))
    }

    // ------------------------------------------------------------------
    // user-initiated window closes
    // ------------------------------------------------------------------




    /// The human clicked a window's close button (or hit Cmd-W). Say so on
    /// stdout — with or without `--remote` — so anyone tailing the log can tell
    /// "the user dismissed this" apart from "the app crashed", and remember it
    /// so requests aimed at that window get the real reason.
    pub fn note_user_closed_window(window_id: usize, title: &str) {
        // Only chatter when the bridge is actually up: this line is for the
        // agent driving the app, and a shipped app should not print
        // `[makepad-remote] ...` to stdout every time a window closes.
        let line = format!("[makepad-remote] user closed window {window_id} ({title:?})");
        if is_active() {
            println!("{line}");
            let _ = std::io::stdout().flush();
        }
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
        if is_active() {
            println!("{line}");
            let _ = std::io::stdout().flush();
        }
        push_log_line(line);
    }


    // ------------------------------------------------------------------
    // log ring (filled from log.rs)
    // ------------------------------------------------------------------

    /// The remote surface's own notes — a window the human closed, an HTTP
    /// request — go in the same ring as everything the app logs.
    pub fn push_log_line(line: String) {
        crate::log_ring::push(crate::log::LogLevel::Log, line);
    }

    // ------------------------------------------------------------------
    // grab plumbing, called from the screenshot pipeline
    // ------------------------------------------------------------------


    /// True when a pending screenshot request may be answered by the pass that
    /// belongs to `window_id`. Non-remote (studio / file-sink) ids always match,
    /// so this is transparent to the existing pipeline.
    pub(crate) fn grab_targets_window(request_id: u64, window_id: Option<usize>) -> bool {
        if !is_grab_id(request_id) {
            return true;
        }
        if capture::is_capture_id(request_id) {
            let target = CAPTURE.with_borrow(|capture| capture.as_ref().map(|c| c.window));
            return target.is_some() && target == window_id;
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
            if capture::is_capture_id(id) {
                capture::deliver(id, width, height, capture::Pixels::Png(png.to_vec()));
                continue;
            }
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
                            order: GrabOrder::Rgba,
                            origin: GrabOrigin::TopLeft,
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
            let raw = || capture::Pixels::Raw {
                data: pixels.clone(),
                stride,
                order,
                origin,
            };
            if capture::is_capture_id(id) {
                capture::deliver(id, width, height, raw());
                continue;
            }
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
                        order: raw_grab_order(order),
                        origin: raw_grab_origin(origin),
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
                                    stride: result.width * 4,
                                    order: GrabOrder::Rgba,
                                    origin: GrabOrigin::TopLeft,
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
        poll_with_present(cx, false, |_, _, _| None);
    }

    /// Returns whether a present is still waiting on its window's drawable
    /// (the caller schedules the next beat to poll it again).
    #[cfg(all(target_os = "macos", not(gpusim)))]
    pub(crate) fn poll_macos(
        cx: &mut Cx,
        present: impl FnMut(&mut Cx, WindowId, PresentStage) -> Option<bool>,
    ) -> bool {
        poll_with_present(cx, true, present)
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

    /// `presents`: the caller's closure submits frames itself (standalone
    /// macOS). Otherwise the backend's own render loop presents what the
    /// bridge marked, and a step frame counts as presented once `repaint_id`
    /// moves past it.
    fn poll_with_present(
        cx: &mut Cx,
        presents: bool,
        mut present: impl FnMut(&mut Cx, WindowId, PresentStage) -> Option<bool>,
    ) -> bool {
        if !ACTIVE.load(Ordering::Relaxed) {
            return false;
        }
        publish_windows(cx);
        let mut pending = false;
        // A wall-clock capture records what the window presents, so a still
        // window is kept presenting (a pass repaint, no widget redraw).
        if let Some(window) = CAPTURE.with_borrow(|capture| {
            capture.as_ref().filter(|c| !c.virtual_clock).map(|c| c.window)
        }) {
            if let Ok(window) = resolve_window(cx, Some(window)) {
                cx.request_remote_window_present(window);
            }
        }

        let cmds: Vec<QueuedCmd> = {
            match queue().try_lock() {
                Ok(mut q) => std::mem::take(&mut *q),
                Err(_) => Vec::new(),
            }
        };
        // A due sequence frame precedes commands dispatched on this wake.
        poll_captures(cx);
        pending |= present_captures(cx, presents, &mut present);
        for cmd in cmds {
            poll_captures(cx);
            pending |= present_captures(cx, presents, &mut present);
            let wait_window = match &cmd.cmd {
                Cmd::Input {
                    window, wait: true, ..
                } => Some(*window),
                Cmd::Tweak { wait: true, .. } | Cmd::Ai { wait: true, .. } => Some(None),
                _ => None,
            };
            apply(cx, cmd);
            // Do not batch later input ahead of a grab in this same drain.
            poll_captures(cx);
            pending |= present_captures(cx, presents, &mut present);
            if let Some(window) = wait_window.and_then(|window| resolve_window(cx, window).ok()) {
                cx.request_remote_window_present(window);
                // The input has been applied above, whatever the present
                // does: a frame that cannot be submitted now (the window is
                // busy presenting) or a pass that stays dirty is painted on
                // a later beat, and the waiters resolve on that repaint. An
                // answer of "retry" here made drivers send the input again,
                // and a busy app took every key twice ("bbrowser").
                match present(cx, window, PresentStage::Present) {
                    Some(true) => {}
                    Some(false) | None => {
                        cx.request_remote_window_present(window);
                        pending = true;
                    }
                }
            }
            resolve_frame_waiters(cx);
        }

        poll_captures(cx);
        pending |= present_captures(cx, presents, &mut present);
        resolve_frame_waiters(cx);
        pending
    }

    /// Returns whether a capture's present is waiting on its drawable. A
    /// `/step` in progress runs here too, so its frames are sealed at the
    /// same boundaries as grabs: before any later command is applied.
    fn present_captures(
        cx: &mut Cx,
        presents: bool,
        present: &mut impl FnMut(&mut Cx, WindowId, PresentStage) -> Option<bool>,
    ) -> bool {
        let mut pending = drive_step(cx, presents, present);
        let windows = CAPTURES.with_borrow(|captures| {
            let mut windows: Vec<usize> = captures.windows.values().copied().collect();
            windows.sort_unstable();
            windows.dedup();
            windows
        });
        for window in windows {
            let result = match resolve_window(cx, Some(window)) {
                Ok(window) => present(cx, window, PresentStage::Present),
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

    /// Run the `/step` in progress as far as the GPU lets it: each frame
    /// advances the virtual clock, fires due timers, delivers NextFrame, and
    /// is drawn and presented (with a capture request while a capture runs)
    /// before the next one starts. Returns whether it is still running.
    fn drive_step(
        cx: &mut Cx,
        presents: bool,
        present: &mut impl FnMut(&mut Cx, WindowId, PresentStage) -> Option<bool>,
    ) -> bool {
        let Some(mut job) = STEP.with_borrow_mut(Option::take) else {
            return false;
        };
        let pending = loop {
            if !cx.windows.is_valid(job.window) || !cx.windows[job.window].is_created {
                let _ = job.tx.send(Reply::Err("the stepped window closed".into()));
                return false;
            }
            if job.open.is_none() {
                // Before another frame (or the answer): the capture encoder
                // must have kept up, within a bound; its failure is reported.
                let last = job.remaining == 0;
                match capture_gate(last) {
                    CaptureGate::Ready => job.wait_since = None,
                    CaptureGate::Failed(error) => {
                        let _ = job.tx.send(Reply::Err(format!(
                            "capture failed: {error} (steps continue uncaptured; /cap/stop to close it)"
                        )));
                        return false;
                    }
                    CaptureGate::Wait => {
                        let since = *job.wait_since.get_or_insert_with(Instant::now);
                        if since.elapsed() < STEP_FRAME_LIMIT {
                            break true;
                        }
                        let lost = CAPTURE.with_borrow_mut(|capture| {
                            let c = capture.as_mut()?;
                            let delivered = c.counters.delivered.load(Ordering::Acquire);
                            let lost = c.armed.saturating_sub(delivered + c.abandoned);
                            if last {
                                // count them lost, so the next step does not wait
                                c.abandoned += lost;
                            }
                            Some(lost)
                        });
                        let _ = job.tx.send(Reply::Err(if last {
                            format!(
                                "{} captured frame(s) never came back from the GPU within {:?}; they are missing from the file",
                                lost.unwrap_or(0),
                                STEP_FRAME_LIMIT
                            )
                        } else {
                            format!("the capture encoder fell behind for {:?}", STEP_FRAME_LIMIT)
                        }));
                        return false;
                    }
                }
                if last {
                    let captured = CAPTURE.with_borrow(|capture| {
                        capture.as_ref().filter(|c| c.virtual_clock).map(|c| c.armed)
                    });
                    let mut answer = format!(
                        "{{\"frame\":{},\"time\":{}",
                        app_clock::frame(),
                        num(app_clock::now().unwrap_or(0.0))
                    );
                    if let Some(captured) = captured {
                        answer.push_str(&format!(",\"captured\":{captured}"));
                    }
                    answer.push('}');
                    let _ = job.tx.send(Reply::Text(answer));
                    return false;
                }
                // Loads earlier frames started land before the next frame,
                // however slowly their workers run.
                if job.wait_loads {
                    let loads = cx.pending_async_loads();
                    if !loads.is_empty() {
                        let since = *job.loads_since.get_or_insert_with(Instant::now);
                        if since.elapsed() < STEP_LOADS_LIMIT {
                            break true;
                        }
                        let _ = job.tx.send(Reply::Err(format!(
                            "asynchronous loads still in flight after {:?} before step frame {}: {}",
                            STEP_LOADS_LIMIT,
                            app_clock::frame() + 1,
                            loads_text(&loads)
                        )));
                        return false;
                    }
                    job.loads_since = None;
                }
                job.open = Some(open_step_frame(cx, &job));
            }
            let open = job.open.as_mut().unwrap();
            let presented = if presents {
                if !open.prepared {
                    if present(cx, job.window, PresentStage::Prepare) == Some(false) {
                        if open.opened.elapsed() < STEP_SHADER_WAIT {
                            break true;
                        }
                        crate::log!(
                            "[makepad-remote] step frame {}: shader pipelines still compiling after {:?}, presenting without them",
                            app_clock::frame(),
                            STEP_SHADER_WAIT
                        );
                    }
                    open.prepared = true;
                }
                let presented = present(cx, job.window, PresentStage::Present);
                presented == Some(true)
            } else {
                cx.repaint_id > open.repaint_id
            };
            let captured = open.capture.is_none_or(|(id, _)| {
                !cx.screenshot_requests.iter().any(|request| request.request_id == id)
            });
            if presented && captured {
                job.open = None;
                job.remaining -= 1;
                continue;
            }
            if open.opened.elapsed() >= STEP_FRAME_LIMIT {
                if let Some((id, index)) = open.capture {
                    let armed = cx.screenshot_requests.iter().any(|request| request.request_id == id);
                    cx.screenshot_requests.retain(|request| request.request_id != id);
                    if armed {
                        // Never read back: the file gets a gap there, and
                        // neither this step nor the encoder waits for it.
                        CAPTURE.with_borrow_mut(|capture| {
                            if let Some(c) = capture.as_mut() {
                                c.abandoned += 1;
                                let _ = c.tx.send(capture::CaptureMsg::Skip { index });
                            }
                        });
                    }
                }
                let _ = job.tx.send(Reply::Err(format!(
                    "step frame {} was not presented within {:?}",
                    app_clock::frame(),
                    STEP_FRAME_LIMIT
                )));
                return false;
            }
            // The frame is still to be presented (or presented without its
            // capture): mark the window again and retry on the next beat.
            cx.request_remote_window_present(job.window);
            break true;
        };
        STEP.set(Some(job));
        pending
    }

    /// Advance the virtual clock by one frame and deliver what that frame
    /// owes the app, in the order a real frame would: timers due by now,
    /// then NextFrame. The draw happens when the frame is presented.
    fn open_step_frame(cx: &mut Cx, job: &StepJob) -> OpenFrame {
        // What the previous frame or the commands since left queued happens
        // at the time it was queued, never after the advance by chance.
        cx.settle_deferred_events();
        let (_, time) = app_clock::advance(job.fps);
        // The batch due now; timers its handlers start fire next frame.
        let due = cx.app_clock.take_due(time);
        for timer_id in due {
            let event = crate::event::TimerEvent {
                time: Some(time),
                timer_id,
            };
            cx.handle_script_timer(&event);
            cx.call_event_handler(&crate::event::Event::Timer(event));
        }
        if !cx.new_next_frames.is_empty() {
            cx.app_clock.dispatching_frame = true;
            cx.call_next_frame_event(time);
            cx.app_clock.dispatching_frame = false;
        }
        let capture = CAPTURE.with_borrow_mut(|capture| match capture {
            Some(c)
                if c.virtual_clock
                    && c.window == job.window.id()
                    && !c.counters.failed.load(Ordering::Acquire)
                    && !c.counters.paused.load(Ordering::Acquire) =>
            {
                let index = c.armed;
                c.armed += 1;
                Some((capture::capture_id(c.session, index), index))
            }
            _ => None,
        });
        if let Some((id, _)) = capture {
            cx.screenshot_requests.push(ScreenshotRequest {
                request_id: id,
                kind_id: 0,
            });
        }
        cx.request_remote_window_present(job.window);
        OpenFrame {
            repaint_id: cx.repaint_id,
            capture,
            opened: Instant::now(),
            prepared: false,
        }
    }

    /// Detach the UI from the capture and tell the worker to finish: the
    /// audio tap comes off, and a step frame armed but not yet on the GPU is
    /// dropped (the file ends one frame short of it). Returns the frame
    /// count the file is finished at, `None` when no capture runs.
    fn stop_ui_capture(cx: &mut Cx) -> Option<u64> {
        let mut capture = CAPTURE.with_borrow_mut(Option::take)?;
        if let Some(tap) = capture.tap.take() {
            crate::audio_output_tap::remove_audio_output_tap(tap);
        }
        let dropped = STEP.with_borrow_mut(|step| {
            let open = step.as_mut().and_then(|job| job.open.as_mut())?;
            let (id, _) = open.capture.take()?;
            let pending = cx
                .screenshot_requests
                .iter()
                .any(|request| request.request_id == id);
            cx.screenshot_requests.retain(|request| request.request_id != id);
            pending.then_some(())
        });
        let frames = capture.armed - dropped.map_or(0, |_| 1);
        let _ = capture.tx.send(capture::CaptureMsg::Stop {
            frames: capture.virtual_clock.then_some(frames),
        });
        Some(frames)
    }

    /// The app is shutting down (`/quit`, `/gq`, its last window closed, a
    /// signal) with a capture open: finish the file so it is playable (its
    /// index is written by `finish`). This waits on the encoder, bounded, on
    /// the UI thread; the process is exiting and has nothing else to do.
    pub(crate) fn finish_capture_on_shutdown(cx: &mut Cx) {
        if stop_ui_capture(cx).is_none() {
            return;
        }
        capture::remove_screen_sink();
        let line = match capture::wait_for_result(CAPTURE_SHUTDOWN_WAIT) {
            Ok(result) => format!(
                "[makepad-remote] capture finished at shutdown: {} ({} frames)",
                result.path, result.frames
            ),
            Err(err) => format!("[makepad-remote] capture at shutdown: {err}"),
        };
        println!("{line}");
        let _ = std::io::stdout().flush();
        push_log_line(line);
    }

    /// Longer than the worker's wait for a missing frame, so a lost last
    /// readback still ends in a finished file.
    const CAPTURE_SHUTDOWN_WAIT: Duration = Duration::from_secs(20);

    /// `/settled`: why the app would still change if time moved on.
    fn settled_json(cx: &Cx) -> String {
        let mut reasons = Vec::new();
        if !cx.new_next_frames.is_empty() {
            reasons.push("next_frame");
        }
        if cx.need_redrawing() {
            reasons.push("redraw");
        }
        if cx.any_passes_dirty() {
            reasons.push("repaint");
        }
        if let Some(now) = app_clock::now() {
            if cx.app_clock.due_within(now + 1.0 / app_clock::fps().max(1) as f64) {
                reasons.push("timer");
            }
        }
        if STEP.with_borrow(|step| step.is_some()) {
            reasons.push("step");
        }
        // Work on workers or the network whose result will change a frame.
        let loads = cx.pending_async_loads();
        if !loads.is_empty() {
            reasons.push("loads");
        }
        #[cfg(all(
            not(gpusim),
            any(target_os = "macos", target_os = "ios", target_os = "tvos")
        ))]
        if cx.metal_pipelines_pending() > 0 {
            reasons.push("shaders");
        }
        let list: Vec<String> = reasons.iter().map(|reason| format!("\"{reason}\"")).collect();
        let mut out = format!(
            "{{\"settled\":{},\"reasons\":[{}]",
            reasons.is_empty(),
            list.join(",")
        );
        if !loads.is_empty() {
            let entries: Vec<String> = loads
                .iter()
                .map(|(kind, count)| format!("{}:{count}", json_str(kind)))
                .collect();
            out.push_str(&format!(",\"loads\":{{{}}}", entries.join(",")));
        }
        if let Some(now) = app_clock::now() {
            out.push_str(&format!(",\"frame\":{},\"time\":{}", app_clock::frame(), num(now)));
        }
        out.push('}');
        out
    }

    /// Does an id path answer a `path:` query? Exactly, or as its trailing
    /// whole segments: `new_note` and `main.new_note` both find
    /// `main.new_note`; `note` does not.
    fn path_matches(path: &str, query: &str) -> bool {
        let query = query.trim_matches('.');
        !query.is_empty()
            && (path == query
                || path
                    .strip_suffix(query)
                    .is_some_and(|head| head.ends_with('.')))
    }

    /// The widget rows `/snap` reports, each with its id path when the
    /// widgets layer registered one (`Cx::widget_paths_callback`).
    fn snapshot_rows(cx: &Cx) -> Vec<(WidgetSnapshot, Option<String>)> {
        if let Some(callback) = cx.widget_paths_callback {
            return callback(cx)
                .into_iter()
                .map(|(widget, path)| (widget, Some(path)))
                .collect();
        }
        match cx.widget_snapshot_callback {
            Some(callback) => callback(cx).into_iter().map(|widget| (widget, None)).collect(),
            None => Vec::new(),
        }
    }

    /// `/d?paths=1`: one visible widget per line, `path type x y w h` in
    /// window-local points (an unnamed widget shows its id).
    fn path_dump(cx: &Cx) -> String {
        if cx.widget_paths_callback.is_none() {
            return "no widget paths (this app's widgets layer registers none)\n".into();
        }
        let rows = snapshot_rows(cx);
        let mut out = String::new();
        for (widget, path) in rows {
            if !widget.visible || widget.width <= 0 || widget.height <= 0 {
                continue;
            }
            let origin = cx
                .windows
                .id_iter()
                .find(|id| id.id() == widget.window_index && cx.windows[*id].is_created)
                .map(|id| cx.windows[id].window_geom.position)
                .unwrap_or_default();
            let path = path.filter(|p| !p.is_empty()).unwrap_or_else(|| widget.id.clone());
            out.push_str(&format!(
                "{} {} {} {} {} {}\n",
                path,
                widget.widget_type,
                num(widget.x as f64 - origin.x),
                num(widget.y as f64 - origin.y),
                widget.width,
                widget.height
            ));
        }
        out
    }

    /// Injected pointer input moves the synthetic cursor (when shown).
    fn note_cursor(window_id: WindowId, kind: MouseKind, pos: Vec2d, time: f64) {
        let down = match kind {
            MouseKind::Down => Some(true),
            MouseKind::Up => Some(false),
            MouseKind::Move | MouseKind::Scroll => None,
        };
        synthetic_cursor::note_synthetic_pointer(window_id.id(), pos, down, time);
    }

    fn cursor_json() -> String {
        match synthetic_cursor::synthetic_cursor() {
            Some(cursor) => format!(
                "{{\"cursor\":1,\"style\":\"{}\",\"pos\":[{},{}],\"w\":{}}}",
                cursor.style.name(),
                num(cursor.pos.x),
                num(cursor.pos.y),
                cursor.window_id.map_or("null".to_string(), |id| id.to_string()),
            ),
            None => "{\"cursor\":0}".to_string(),
        }
    }

    fn resolve_frame_waiters(cx: &Cx) {
        // Resolve anyone who asked to be answered after the next frame.
        let repaint_id = cx.repaint_id;
        FRAME_WAITERS.with_borrow_mut(|waiters| {
            waiters.retain(|(target, tx, payload, user_seq)| {
                if *user_seq != cx.remote_activity.user_seq() {
                    let _ = tx.send(Reply::Conflict(activity::interrupted(cx)));
                    return false;
                }
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

    fn apply(cx: &mut Cx, request: QueuedCmd) {
        if let Some(tx) = request.cmd.mutation_reply() {
            if let Some(conflict) = activity::conflict(cx, request.user_seq) {
                let _ = tx.send(Reply::Conflict(conflict));
                return;
            }
            if Instant::now() >= request.deadline {
                let _ = tx.send(Reply::Err("expired command was not applied".into()));
                return;
            }
        }
        let user_seq = cx.remote_activity.user_seq();
        let _origin = super::remote_input_scope(cx);
        match request.cmd {
            Cmd::Activity(tx) => {
                let _ = tx.send(Reply::Text(activity::json(cx)));
            }
            Cmd::Input {
                window,
                inputs,
                wait,
                tx,
            } => {
                note_injected_input();
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
                        time: at,
                    } = input
                    {
                        let time = at.unwrap_or(time);
                        let raw = dvec2(x, y);
                        note_cursor(window_id, kind, raw, time);
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
                            time: at,
                        } => {
                            let time = at.unwrap_or(time);
                            note_cursor(window_id, kind, dvec2(x, y), time);
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
                        Input::Pinch {
                            x,
                            y,
                            scale,
                            phase,
                            mods,
                            time: at,
                        } => {
                            let native = cx.windows[window_id]
                                .layout_vec2d_to_native_points(dvec2(x, y));
                            StudioToApp::Pinch(RemotePinch {
                                time: at.unwrap_or(time),
                                x: native.x,
                                y: native.y,
                                scale,
                                phase,
                                modifiers: mods,
                            })
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
                        Input::DragQuery(x, y) => {
                            let response = std::rc::Rc::new(std::cell::Cell::new(crate::event::WindowDragQueryResponse::NoAnswer));
                            cx.call_event_handler(&crate::event::Event::WindowDragQuery(crate::event::WindowDragQueryEvent {
                                window_id,
                                abs: crate::makepad_math::dvec2(x, y),
                                response: response.clone(),
                            }));
                            input_result = Some(format!("{{\"ok\":1,\"drag\":\"{:?}\"}}", response.get()));
                            continue;
                        }
                        Input::Resize(w, h) => {
                            cx.push_unique_platform_op(crate::cx_api::CxOsOp::ResizeWindow(window_id, crate::makepad_math::dvec2(w, h)));
                            input_result = Some(format!("{{\"ok\":1,\"resize\":[{w},{h}]}}"));
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
                        waiters.push((cx.repaint_id + 1, tx, input_result, user_seq))
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
            Cmd::Dump { paths, tx } => {
                let dump = if paths {
                    path_dump(cx)
                } else {
                    match cx.widget_tree_dump_callback {
                        Some(callback) => callback(cx),
                        None => String::new(),
                    }
                };
                let _ = tx.send(Reply::Text(dump));
            }
            Cmd::Step {
                window,
                frames,
                fps,
                wait_loads,
                tx,
            } => {
                if !app_clock::enabled() {
                    let _ = tx.send(Reply::Err(
                        "/step needs the virtual clock (launch with --virtual-clock or MAKEPAD_VIRTUAL_CLOCK=1)".into(),
                    ));
                    return;
                }
                if STEP.with_borrow(|step| step.is_some()) {
                    let _ = tx.send(Reply::Err("a /step is already running".into()));
                    return;
                }
                // A capture's window is the one stepped, unless one is named.
                let window = window.or_else(|| CAPTURE.with_borrow(|c| c.as_ref().map(|c| c.window)));
                let window = match resolve_window(cx, window) {
                    Ok(window) => window,
                    Err(err) => {
                        let _ = tx.send(Reply::Err(err));
                        return;
                    }
                };
                // The file's timestamps are n / capture fps: a step at another
                // rate would play back faster or slower than it ran.
                let capture_fps = CAPTURE.with_borrow(|c| {
                    c.as_ref().filter(|c| c.virtual_clock && !c.failure_reported).map(|c| c.fps)
                });
                if let Some(capture_fps) = capture_fps.filter(|capture_fps| *capture_fps != fps) {
                    let _ = tx.send(Reply::Err(format!(
                        "a capture at {capture_fps} fps is running; step with fps={capture_fps}"
                    )));
                    return;
                }
                STEP.set(Some(StepJob {
                    window,
                    fps,
                    remaining: frames,
                    open: None,
                    wait_since: None,
                    wait_loads,
                    loads_since: None,
                    tx,
                }));
            }
            Cmd::Settled(tx) => {
                let _ = tx.send(Reply::Text(settled_json(cx)));
            }
            Cmd::Platform(payload) => match *payload
                .downcast::<PlatformCmd>()
                .expect("remote: a platform command from another platform")
            {
                PlatformCmd::CapStart {
                    window,
                    config,
                    session,
                    tx,
                } => {
                    let window_id = match resolve_window(cx, Some(window)) {
                        Ok(window_id) => window_id,
                        Err(err) => {
                            let _ = tx.send(Reply::Err(err));
                            return;
                        }
                    };
                    let capture::NewSession {
                        session,
                        tx: capture_tx,
                        rx,
                        counters,
                        done_tx,
                    } = session;
                    // Applied after its request gave up (and closed the session).
                    if !capture::session_open(session) {
                        let _ = tx.send(Reply::Err("capture session was closed before it started".into()));
                        return;
                    }
                    let virtual_clock = config.virtual_clock;
                    let audio = config
                        .audio
                        .then(|| Arc::new(Mutex::new(capture::AudioQueue::new(virtual_clock))));
                    let tap = audio.clone().and_then(|queue| {
                        let counters = counters.clone();
                        crate::audio_output_tap::add_audio_output_tap(move |info, buffer| {
                            capture::tap_audio(&queue, &counters, info.sample_rate, buffer)
                        })
                    });
                    let answer = format!(
                        "{{\"ok\":1,\"path\":{},\"fps\":{},\"audio\":{},\"virtual_clock\":{},\"w\":{},\"frame\":{}}}",
                        json_str(&config.path),
                        config.fps,
                        config.audio as u8,
                        config.virtual_clock as u8,
                        window,
                        app_clock::frame(),
                    );
                    let fps = config.fps;
                    let worker_counters = counters.clone();
                    let spawned = cx.thread_spawner().spawn_worker(
                        crate::thread::ThreadOptions {
                            name: Some("makepad-remote-capture".into()),
                            ..Default::default()
                        },
                        move || {
                            let result = capture::run_worker(config, rx, worker_counters, audio);
                            let _ = done_tx.send(result);
                        },
                    );
                    match spawned {
                        Ok(handle) => handle.detach(),
                        Err(err) => {
                            if let Some(tap) = tap {
                                crate::audio_output_tap::remove_audio_output_tap(tap);
                            }
                            let _ = tx.send(Reply::Err(format!("capture worker: {err:?}")));
                            return;
                        }
                    }
                    CAPTURE.set(Some(UiCapture {
                        session,
                        window: window_id.id(),
                        virtual_clock,
                        fps,
                        armed: 0,
                        abandoned: 0,
                        failure_reported: false,
                        tx: capture_tx,
                        counters,
                        tap,
                    }));
                    let _ = tx.send(Reply::Text(answer));
                }
                PlatformCmd::Cursor {
                    style,
                    window,
                    pos,
                    tx,
                } => {
                    if let Some(style) = style {
                        synthetic_cursor::set_synthetic_cursor(style);
                    }
                    if let Some(pos) = pos {
                        let window_id = match resolve_window(cx, window) {
                            Ok(window_id) => window_id,
                            Err(err) => {
                                let _ = tx.send(Reply::Err(err));
                                return;
                            }
                        };
                        let time = cx.seconds_since_app_start();
                        synthetic_cursor::note_synthetic_pointer(window_id.id(), pos, None, time);
                    }
                    // The cursor is drawn by the window; a hide or a new style
                    // is a change nothing else would redraw.
                    cx.redraw_all();
                    let _ = tx.send(Reply::Text(cursor_json()));
                }
            },
            Cmd::CapPause(pause, tx) => {
                // Under the virtual clock a paused capture arms no step frame,
                // so the next written frame takes the next index: pts, the
                // frame count and the audio (fed per written frame) go on
                // without a gap. Wall mode takes the paused time off its clock.
                let answer = CAPTURE.with_borrow(|capture| {
                    let c = capture.as_ref()?;
                    c.counters.paused.store(pause, Ordering::Release);
                    let frames = if c.virtual_clock {
                        c.armed.saturating_sub(c.abandoned)
                    } else {
                        c.counters.consumed.load(Ordering::Acquire)
                    };
                    Some(format!(
                        "{{\"ok\":1,\"{}\":1,\"frames\":{frames}}}",
                        if pause { "paused" } else { "resumed" }
                    ))
                });
                let _ = tx.send(match answer {
                    Some(answer) => Reply::Text(answer),
                    None => Reply::Err("no capture is running".into()),
                });
            }
            Cmd::CapStop(tx) => match stop_ui_capture(cx) {
                Some(frames) => {
                    let _ = tx.send(Reply::Text(format!("{{\"armed\":{frames}}}")));
                }
                None => {
                    let _ = tx.send(Reply::Err("no capture is running".into()));
                }
            },
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
                path,
                all,
                tx,
            } => {
                let widgets = snapshot_rows(cx);
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
                for (widget, widget_path) in &widgets {
                    if !all && (!widget.visible || widget.width <= 0 || widget.height <= 0) {
                        continue;
                    }
                    if let Some(query) = &path {
                        if !widget_path.as_deref().is_some_and(|p| path_matches(p, query)) {
                            continue;
                        }
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
                    if let Some(widget_path) = widget_path.as_ref().filter(|p| !p.is_empty()) {
                        out.push_str(&format!(",\"p\":{}", json_str(widget_path)));
                    }
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
            Cmd::Resize { window, size, tx } => {
                let window_id = match resolve_window(cx, window) {
                    Ok(window_id) => window_id,
                    Err(err) => {
                        let _ = tx.send(Reply::Err(err));
                        return;
                    }
                };
                cx.push_unique_platform_op(crate::cx_api::CxOsOp::ResizeWindow(window_id, size));
                // The window's own size event paints once; the reply goes
                // out with that frame and carries what was asked, the
                // following `/s` what is.
                FRAME_WAITERS.with_borrow_mut(|waiters| waiters.push((
                    cx.repaint_id + 1,
                    tx,
                    Some(format!("{{\"resize\":[{},{}]}}", size.x, size.y)),
                    user_seq,
                )));
                cx.redraw_all();
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
                            waiters.push((cx.repaint_id + 1, tx, Some(json), user_seq))
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
                        Err("no AI overlay (this app does not link makepad-app-aichat)".to_string())
                    }
                };
                match result {
                    Ok(json) => {
                        if wait {
                            FRAME_WAITERS.with_borrow_mut(|waiters| {
                            waiters.push((cx.repaint_id + 1, tx, Some(json), user_seq))
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



    /// `/cap/start?path=ABS.mp4[&fps=60][&audio=1][&w=]`.
    fn route_capture_start(p: &Params) -> Out {
        let Some(path) = p.get(&["path"]) else {
            return err("cap/start needs path= (an absolute .mp4 path)");
        };
        if let Err(msg) = capture::check_output_path(path, p.flag(&["overwrite"])) {
            return err(&msg);
        }
        let fps = match p.get(&["fps"]).unwrap_or("60").parse::<u32>() {
            Ok(fps @ 1..=240) => fps,
            _ => return err("cap/start fps must be an integer in 1..=240"),
        };
        let window = match p
            .window()
            .or_else(|| status_cell().lock().unwrap().windows.first().map(|w| w.id))
        {
            Some(window) => window,
            None => return err("no windows"),
        };
        let session = match capture::open_session() {
            Ok(session) => session,
            Err(msg) => return err(&msg),
        };
        let config = capture::CaptureConfig {
            path: path.to_string(),
            fps,
            audio: p.flag(&["audio"]),
            virtual_clock: app_clock::enabled(),
        };
        let virtual_clock = config.virtual_clock;
        match ask(
            move |tx| {
                Cmd::Platform(Box::new(PlatformCmd::CapStart {
                    window,
                    config,
                    session,
                    tx,
                }))
            },
            10,
        ) {
            Reply::Text(text) => {
                if !virtual_clock {
                    capture::install_screen_sink(window, fps);
                }
                Out::Json(200, text)
            }
            other => {
                capture::close_session();
                reply_to_out(other)
            }
        }
    }

    /// `/cap/stop[?hashes=1]`: answered once the file is finalized.
    fn route_capture_stop(p: &Params) -> Out {
        capture::remove_screen_sink();
        match ask(Cmd::CapStop, 10) {
            Reply::Text(_) => {}
            other => return reply_to_out(other),
        }
        match capture::wait_for_result(Duration::from_secs(600)) {
            Ok(result) => {
                let mut out = format!(
                    "{{\"ok\":1,\"path\":{},\"frames\":{},\"sz\":[{},{}],\"missing\":{},\"hash\":\"{:016x}\"",
                    json_str(&result.path),
                    result.frames,
                    result.width,
                    result.height,
                    result.missing,
                    result.hash,
                );
                if let Some(rate) = result.audio_rate {
                    out.push_str(&format!(",\"audio_rate\":{rate}"));
                }
                if p.flag(&["hashes"]) {
                    let hashes: Vec<String> = result
                        .frame_hashes
                        .iter()
                        .map(|hash| format!("\"{hash:016x}\""))
                        .collect();
                    out.push_str(&format!(",\"frame_hashes\":[{}]", hashes.join(",")));
                }
                out.push('}');
                Out::Json(200, out)
            }
            Err(msg) => err(&msg),
        }
    }

    /// `/cursor?show=1|0[&style=arrow|hand|text|crosshair|app][&x=&y=][&w=]`.
    fn route_cursor(p: &Params) -> Out {
        let style = match p.get(&["style"]) {
            Some(name) => match synthetic_cursor::SyntheticCursorStyle::parse(name) {
                Some(style) => Some(style),
                None => return err("cursor style must be arrow, hand, text, crosshair or app"),
            },
            None => None,
        };
        let style = match p.get(&["show", "on"]) {
            Some("0" | "false" | "off" | "no") => Some(None),
            Some(_) => Some(Some(style.unwrap_or(synthetic_cursor::SyntheticCursorStyle::Fixed(
                crate::cursor::MouseCursor::Arrow,
            )))),
            None => style.map(Some),
        };
        let pos = match (p.get(&["x"]), p.get(&["y"])) {
            (Some(_), Some(_)) => Some(dvec2(p.f64(&["x"], 0.0), p.f64(&["y"], 0.0))),
            (None, None) => None,
            _ => return err("cursor position needs both x= and y="),
        };
        let window = p.window();
        reply_to_out(ask(
            move |tx| {
                Cmd::Platform(Box::new(PlatformCmd::Cursor {
                    style,
                    window,
                    pos,
                    tx,
                }))
            },
            4,
        ))
    }








    /// The port names a `/midi?k=ports` list asks for, each with the ends it
    /// carries.
    ///
    /// A name may be suffixed with the direction it has: `Surface:i`,
    /// `Surface:o`, `Surface:io`. Only a suffix made entirely of `i` and `o`
    /// counts as one, because a real port name is full of colons -- an ALSA
    /// port is called things like `Through:Through Port-0 14:0` -- and a name
    /// that happens to contain one must survive being written down here.
    fn port_names(list: &str) -> Vec<(String, bool, bool)> {
        let mut names = Vec::new();
        for entry in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let (name, dirs) = match entry.rsplit_once(':') {
                Some((name, dirs))
                    if !name.is_empty()
                        && !dirs.is_empty()
                        && dirs.chars().all(|c| c == 'i' || c == 'o') =>
                {
                    (name, dirs)
                }
                _ => (entry, "io"),
            };
            names.push((name.to_string(), dirs.contains('i'), dirs.contains('o')));
        }
        names
    }

    /// Drive the MIDI input path with no hardware, and read back what the
    /// app sent.
    ///
    /// `in` queues a message as though a device had sent it: three bytes,
    /// on a port named by id. `out` takes everything the app has sent since
    /// the last read. `reset` forgets both, so one test cannot read the
    /// tail of another's traffic.
    ///
    /// This proves everything the app does WITH a message. It proves nothing
    /// about enumeration, the OS handles or real hot-plug, which all sit
    /// below the seam it enters at.
    fn route_midi(p: &Params) -> Out {
        use crate::midi::{MidiData, MidiPortId};
        use crate::makepad_live_id::LiveId;
        match p.get(&["k", "kind"]).unwrap_or("in") {
            "reset" => {
                crate::midi_inject::reset();
                Out::Json(200, "{\"ok\":1}".to_string())
            }
            "ports" => {
                // Names, comma separated, each optionally suffixed with the
                // direction it carries: "Surface:io", ":i", ":o". Both ends
                // by default, because a controller is normally both.
                let names = port_names(p.get(&["n", "names"]).unwrap_or(""));
                if names.is_empty() {
                    return err("need n=NAME[,NAME] (suffix :i or :o for one direction)");
                }
                let descs = crate::midi_inject::declare_ports(&names);
                let rows: Vec<String> = descs
                    .iter()
                    .map(|d| {
                        format!(
                            "{{\"name\":{},\"id\":{},\"dir\":\"{}\"}}",
                            json_str(&d.name),
                            d.port_id.0 .0,
                            if d.port_type.is_input() { "in" } else { "out" }
                        )
                    })
                    .collect();
                Out::Json(200, format!("{{\"n\":{},\"ports\":[{}]}}", rows.len(), rows.join(",")))
            }
            "out" => {
                let sent = crate::midi_inject::drain_outgoing();
                let rows: Vec<String> = sent
                    .iter()
                    .map(|(port, data)| {
                        format!(
                            "{{\"port\":{},\"d\":[{},{},{}]}}",
                            port.0 .0, data.data[0], data.data[1], data.data[2]
                        )
                    })
                    .collect();
                Out::Json(200, format!("{{\"n\":{},\"m\":[{}]}}", rows.len(), rows.join(",")))
            }
            "in" => {
                let byte = |keys: &[&str]| p.f64(keys, -1.0);
                let (a, b, c) = (byte(&["a", "status"]), byte(&["b", "d1"]), byte(&["c", "d2"]));
                if a < 0.0 || b < 0.0 || c < 0.0 {
                    return err("need a= b= c= (three MIDI bytes)");
                }
                if a > 255.0 || b > 127.0 || c > 127.0 {
                    return err("a<=255, b<=127, c<=127");
                }
                // A port may be given by the NAME it was declared under or
                // by its raw id. Absent, it is 0, which is what the app sees
                // for a message it never attributed to a port.
                let port = match p.get(&["p", "port"]) {
                    None => MidiPortId(LiveId(0)),
                    Some(value) => match value.parse::<u64>() {
                        Ok(raw) => MidiPortId(LiveId(raw)),
                        Err(_) => crate::midi_inject::port_id_for(value),
                    },
                };
                let data = MidiData { data: [a as u8, b as u8, c as u8] };
                let queued = crate::midi_inject::push_incoming(port, data);
                Out::Json(
                    200,
                    format!(
                        "{{\"ok\":{},\"dropped\":{}}}",
                        queued as u8,
                        crate::midi_inject::dropped()
                    ),
                )
            }
            other => err(&format!("bad kind {other}")),
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
    // socket or starting an app. The ring itself is always on; the flag is
    // what the rest of the bridge reads.
    #[cfg(gpusim)]
    pub fn gpusim_start_log_capture() {
        ACTIVE.store(true, Ordering::Relaxed);
    }

    #[cfg(gpusim)]
    pub fn gpusim_log_snapshot() -> String {
        log_json(&Params(vec![("since".into(), "0".into())]), "")
    }

    fn log_json(p: &Params, pool: &str) -> String {
        let since = p.get(&["since"]).and_then(|v| v.parse::<u64>().ok());
        let count = p
            .get(&["n", "count", "tail"])
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(50);
        // The ring already tails and already carries the newest sequence,
        // so the count is applied by the read rather than by trimming a
        // vector afterwards.
        let (newest, lines) = match since {
            Some(since) => crate::log_ring::read_since(since, usize::MAX),
            None => crate::log_ring::read_since(0, count),
        };
        let mut out = format!("{{\"n\":{newest},\"pool\":{},\"l\":[", json_str(pool));
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&json_str(&line.text));
        }
        out.push_str("]}");
        out
    }















    fn wake_commands() {
        // SignalToUI coalesces wakes until timer 0 clears its flag. A remote
        // capture must also wake between those ticks (including 200 ms idle).
        #[cfg(all(target_os = "macos", not(gpusim)))]
        crate::os::apple::macos::macos_app::wake_event_loop();
        #[cfg(not(all(target_os = "macos", not(gpusim))))]
        crate::thread::SignalToUI::set_internal_signal();
    }




    // ------------------------------------------------------------------
    // parameters
    // ------------------------------------------------------------------






    // ------------------------------------------------------------------
    // formatting helpers
    // ------------------------------------------------------------------




    #[cfg(test)]
    mod tests {
        use super::*;

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
        fn path_queries_match_whole_trailing_segments() {
            assert!(path_matches("main.new_note", "main.new_note"));
            assert!(path_matches("main.new_note", "new_note"));
            assert!(path_matches("body.main.new_note", "main.new_note"));
            assert!(path_matches("main.new_note", ".new_note"));
            assert!(!path_matches("main.new_note", "note"));
            assert!(!path_matches("main.new_note", "main"));
            assert!(!path_matches("main.new_note", ""));
            assert!(!path_matches("", "new_note"));
        }

        /// A port name is the caller's own text and a direction suffix is
        /// optional, so the split has to be conservative: only a suffix made
        /// of `i` and `o` is a direction, and everything else -- including
        /// the colons a real ALSA port name is full of -- is part of the name.
        #[test]
        fn a_direction_suffix_is_told_from_a_colon_in_the_name() {
            let both = |name: &str| (name.to_string(), true, true);
            assert_eq!(port_names("Wide Surface"), vec![both("Wide Surface")]);
            assert_eq!(
                port_names(" A , B "),
                vec![both("A"), both("B")],
                "spaces around a name are not part of it"
            );
            assert_eq!(
                port_names("Deck:i,Lamps:o"),
                vec![("Deck".to_string(), true, false), ("Lamps".to_string(), false, true)]
            );
            assert_eq!(port_names("Both:io"), vec![both("Both")]);
            // The colons a real port name carries.
            assert_eq!(
                port_names("Through:Through Port-0 14:0"),
                vec![both("Through:Through Port-0 14:0")]
            );
            assert_eq!(port_names("Port: 2"), vec![both("Port: 2")]);
            // A bare suffix names nothing, so it is a name, not a direction.
            assert_eq!(port_names(":i"), vec![both(":i")]);
            assert!(port_names("").is_empty());
            assert!(port_names(" , ").is_empty(), "empty entries are not ports");
        }

    }
}

#[cfg(any(target_arch = "wasm32", target_os = "android", target_env = "ohos"))]
mod imp {
    use crate::cx::Cx;

    pub fn start_if_requested(_cx: &mut Cx) {}
    pub(crate) fn note_user_event(_cx: &mut Cx, _event: &crate::event::Event) {}
    /// There is no remote bridge on these targets, so nothing ever asked for one.
    pub fn requested() -> bool {
        false
    }
    pub fn is_active() -> bool {
        false
    }
    /// There is no bridge on these targets, so nothing is ever driving.
    pub fn hands_off_active() -> bool {
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
    pub(crate) fn finish_capture_on_shutdown(_cx: &mut Cx) {}
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
