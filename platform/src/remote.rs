//! `--remote`: the process's makepad-remote endpoint, with the control commands of every makepad
//! app.
//!
//! Opened from `app_main!` when the process is launched with `--remote` (no value) or
//! `MAKEPAD_REMOTE=1` / `MAKEPAD_INSPECT=1`, or as an ordinary launch on a developer computer
//! (`MAKEPAD_REMOTE=local` or the per-user setting, `makepad_remote::ask`). It listens on the process's Unix socket (`makepad_remote::carrier::unix`),
//! prints one grep-able line, and then lets an external agent drive the app: read window
//! geometry, view windows as frames (the `frames` service), inject mouse / key / text events
//! through the *real* event path, dump the widget tree, and tail the log. The commands are the
//! `cmd` service (`makepad-remote app <pid> help`); `grab`, `gseq`, `gq` are client side.
//!
//! Design notes:
//! - Zero external crates. The wire is makepad-remote's (`libs/remote`).
//! - The command threads never touch `Cx`. They push commands onto a global queue and
//!   block on a reply channel; [`poll`] drains the queue from the event loop
//!   (via `Cx::poll_control_channel`, which every backend already calls) and
//!   answers. So replies may block a command thread, never the UI thread.
//! - Input is injected through `Cx::dispatch_studio_msg`, the same function the
//!   studio remote bridge uses, so `Hits`, capture and gestures behave exactly
//!   as they do for real events. File drops use the native drag/drop event path.
//! - Frames use readback tickets for render
//!   textures and the presenting command buffer for Metal drawables. Raw
//!   pixels are scaled and encoded on one bounded, long-lived worker, and travel to the client
//!   as `frames` records; an image never becomes a file on the app's side.
//! - Standalone macOS serializes a grab with remote commands: apply earlier
//!   commands, draw pending changes, then submit the capture before applying
//!   later commands (including input). No NextFrame/animation tick is inserted
//!   at this boundary. Concurrent requests are ordered by UI queue order,
//!   not by the time their clients connected. Each sequence deadline is
//!   a new boundary; it does not freeze the app for the entire sequence.

/// A JSON string literal for `s`, quotes included. Every target builds it: widgets core's
/// design data uses it on the web too, where the remote itself is not built.
pub fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

#[cfg(not(target_arch = "wasm32"))]
#[path = "remote_core.rs"]
mod bridge;

#[cfg(not(target_arch = "wasm32"))]
pub use makepad_remote::parse_startup_line;

#[path = "app_clock.rs"]
pub mod app_clock;
#[path = "synthetic_cursor.rs"]
pub mod synthetic_cursor;

/// Marks synchronous injected dispatch, including hardware-path mouse input and
/// nested events. Native input delivered on a later event-loop turn stays native.
/// The flag is the `Cx`'s (`RemoteActivity::remote_input`); the guard holds a
/// handle to it so the dispatch it brackets is free to borrow the `Cx`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct RemoteInputScope {
    origin: std::rc::Rc<std::cell::Cell<bool>>,
    previous: bool,
    sender: std::rc::Rc<std::cell::RefCell<String>>,
    previous_by: String,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn remote_input_scope(cx: &crate::cx::Cx) -> RemoteInputScope {
    remote_input_scope_by(cx, "")
}

/// [`remote_input_scope`] for a command `by` sent (`QueuedCmd::by`: the agent and process, empty
/// where not known): what the injected input reaches says who sent it ([`crate::cx::Cx::injected_by`]).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn remote_input_scope_by(cx: &crate::cx::Cx, by: &str) -> RemoteInputScope {
    let origin = cx.remote_activity.remote_input.clone();
    let previous = origin.replace(true);
    let sender = cx.remote_activity.remote_by.clone();
    let previous_by = sender.replace(by.to_string());
    RemoteInputScope { origin, previous, sender, previous_by }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for RemoteInputScope {
    fn drop(&mut self) {
        self.origin.set(self.previous);
        self.sender.replace(std::mem::take(&mut self.previous_by));
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[path = "remote_activity.rs"]
mod activity;

#[cfg(not(target_arch = "wasm32"))]
#[path = "remote_capture.rs"]
mod capture;

#[cfg(not(target_arch = "wasm32"))]
#[path = "remote_input_rec.rs"]
mod input_rec;

#[cfg(not(target_arch = "wasm32"))]
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
    use crate::texture::{ReadbackError, ReadbackRequest, ReadbackTicket, TextureReadback};
    use crate::texture::{ReadbackChannelOrder, ReadbackOrigin};
    use crate::ui_progress::{CmdStage, CommandLive, WaitKind};
    use crate::window::WindowId;
    use makepad_studio_protocol::{
        KeyEvent, RemoteMouseDown, RemoteMouseMove,
        RemoteMouseUp, RemotePinch, RemoteScroll, ScreenshotRequest, StudioToApp, TextInputEvent,
    };
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};
    use super::bridge::*;
    use makepad_remote::{Out, Req};
    use makepad_remote::program::{ServiceClose, ServiceData, ServiceOpen};
    pub use super::bridge::{
        is_active, note_window_close_requested, requested, tag_window_title,
        take_window_close_requested,
    };
    pub(crate) use super::bridge::stop_encoder;

    /// Commands built by the platform-side routes; they travel through the
    /// core queue as `Cmd::Platform`.
    enum PlatformCmd {
        /// A command the app registered (`register_app_query`), answered on the UI thread.
        AppQuery {
            f: AppQueryFn,
            words: Vec<String>,
            kv: Vec<(String, String)>,
            tx: Sender<Reply>,
        },
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

    /// Runs one resolved command. `p` holds the request's key=value arguments (words after the
    /// command name count as flags set to `1`).
    pub fn command(r: &Resolved, p: &Params) -> Answer {
        let schema = match r.key {
            "design" => makepad_remote::params::design(r.sub.as_deref().unwrap_or("")),
            "tweak" => makepad_remote::params::tweak("toggle"),
            "tweak op" => match p.get(&["op"]) {
                Some(op) => if let Some(op) = op.strip_prefix("design_") {
                    makepad_remote::params::design(op)
                } else { makepad_remote::params::tweak(op) },
                None => return err("need op="),
            },
            key if key.starts_with("tweak ") => makepad_remote::params::tweak(&key[6..]),
            key => COMMANDS.iter().find(|c| c.words == key).map(|c| c.params),
        };
        let Some(schema) = schema else { return err("unknown command operation"); };
        if let Err(e) = makepad_remote::params::validate(p.0.iter().filter(|(k, _)| !(r.key == "tweak op" && k == "op")).map(|(k, _)| k.as_str()), schema) {
            return err(&e);
        }
        REQUEST_USER_SEQ.set(match p.get(&["if_user_seq"]) {
            Some(value) => match value.parse::<u64>() {
                Ok(value) => Some(value),
                Err(_) => return err("if_user_seq must be an unsigned integer"),
            },
            // A script that does not track the sequence is gated by the quiet
            // period alone; one that does is refused once the person has
            // intervened since the sequence it named.
            None => None,
        });
        match r.key {
            "status" => route_status(p),
            "gamepads" => reply_to_answer(ask("gamepads", WaitKind::Turn, Cmd::Gamepads)),
            "worker_violations" => {
                let items = makepad_core_util::ui_thread::violations().into_iter()
                    .map(|(operation, count)| format!("{}:{count}", json_str(operation)))
                    .collect::<Vec<_>>();
                Answer::Json(format!("{{{}}}", items.join(",")))
            }
            "activity" => reply_to_answer(ask("activity", WaitKind::Turn, Cmd::Activity)),
            "mouse" => route_mouse(p, None),
            "click" => route_mouse(p, Some("click")),
            "move" => route_mouse(p, Some("move")),
            "key" => route_key(p),
            "text" => route_text(p),
            "drop" => route_file_input(p, FileInput::Drop),
            "drag" => route_file_input(p, FileInput::Drag),
            "drag_end" => route_file_input(p, FileInput::DragEnd),
            "trace" => route_trace(p),
            // read from the runtimes' ledgers, never through the UI thread: a stuck loop is what it shows
            "net" => Answer::Text(crate::makepad_network::ledger::report()),
            "loop" => Answer::Text(crate::os::hal::loop_watch::report()),
            "step" => route_step(p),
            "settled" => reply_to_answer(ask("settled", WaitKind::Turn, Cmd::Settled)),
            // counts so far: a read waits for the UI thread through a stall and misses nothing
            "uicounts" => {
                let waited = p.flag(&["waited"]);
                reply_to_answer(ask("uicounts", WaitKind::Turn, move |tx| Cmd::UiCounts(waited, tx)))
            }
            "cap pause" => reply_to_answer(ask("cap pause", WaitKind::Turn, |tx| Cmd::CapPause(true, tx))),
            "cap resume" => reply_to_answer(ask("cap resume", WaitKind::Turn, |tx| Cmd::CapPause(false, tx))),
            "front" => reply_to_answer(ask("front", WaitKind::Turn, Cmd::Front)),
            "close" => {
                let window = p.window();
                reply_to_answer(ask("close", WaitKind::Turn, move |tx| Cmd::Close { window, tx }))
            }
            // A responsive layout is checked by resizing, so the bridge can:
            // `resize w= h= [window=]` in layout points, answered after the
            // frame that shows it; a following `status` reports the size taken.
            "resize" => {
                // `w` is the width here, so the window goes by `window=`.
                let window = p.get(&["window"]).and_then(|v| v.parse::<usize>().ok());
                let (w, h) = (p.f64(&["w"], 0.0), p.f64(&["h"], 0.0));
                let sane = |v: f64| v.is_finite() && (1.0..=16384.0).contains(&v);
                if !sane(w) || !sane(h) {
                    return err("give w= and h= in layout points, 1 to 16384");
                }
                reply_to_answer(ask("resize", WaitKind::Frame, move |tx| Cmd::Resize { window, size: dvec2(w, h), tx }))
            }
            // `window k=maximize`: the window's own maximise, answered after the
            // next drawn frame with `wait` (the maximise/occlusion proof);
            // `k=fullscreen`: the whole screen at its own size (a window
            // manager's maximise keeps its bar and dock).
            "window" => match p.get(&["k", "kind"]) {
                Some("maximize") | Some("max") => send_input("window", p.window(), vec![Input::Maximize], p.flag(&["wait"])),
                Some("fullscreen") => send_input("window", p.window(), vec![Input::Fullscreen], p.flag(&["wait"])),
                Some("dragquery") => match (p.get(&["x"]).and_then(|v| v.parse::<f64>().ok()), p.get(&["y"]).and_then(|v| v.parse::<f64>().ok())) {
                    (Some(x), Some(y)) => send_input("window", p.window(), vec![Input::DragQuery(x, y)], p.flag(&["wait"])),
                    _ => err("dragquery needs x= and y= (window points)"),
                },
                Some("resize") => match (p.get(&["width"]).and_then(|v| v.parse::<f64>().ok()), p.get(&["height"]).and_then(|v| v.parse::<f64>().ok())) {
                    (Some(w), Some(h)) if w >= 1.0 && h >= 1.0 => send_input("window", p.window(), vec![Input::Resize(w, h)], p.flag(&["wait"])),
                    _ => err("resize needs width= and height= (points)"),
                },
                other => err(&format!("unknown window op {other:?}; k=maximize|fullscreen|resize|dragquery")),
            },
            // The AI chat overlay (apps/aichat, seated in every Window on F10):
            // `ai on=1|0` toggles, `ai say=TEXT` types a line, `ai transcript`
            // reads the conversation. Answered by `Cx::ai_callback`.
            "ai" => route_ai(
                if p.get(&["say"]).is_some() {
                    "say"
                } else if p.get(&["confirm"]).is_some() {
                    "confirm"
                } else {
                    "toggle"
                },
                p,
                true,
            ),
            "ai transcript" => route_ai("transcript", p, false),
            // The red hands-off frame, held up or let go by hand. It lights
            // by itself for a few seconds after any injected input, and
            // shows from the next frame the app draws.
            "handsoff" => {
                let on = p.get(&["on"]).map(|v| v.to_string()).unwrap_or_else(|| "1".to_string()) != "0";
                HANDS_OFF.store(on, Ordering::Relaxed);
                Answer::Json(format!("{{\"handsoff\":{}}}", on as u8))
            }
            // The tweaker overlay (design feedback). Thin: parse here, decide
            // in the widgets-side callback. `wait` answers after the next
            // drawn frame so a following grab sees the change.
            "tweak" => route_tweak("toggle", p, true),
            "tweak state" => route_tweak("state", p, false),
            "tweak apply" => route_tweak("apply", p, true),
            "tweak diff" => route_tweak("diff", p, false),
            "tweak clear" => route_tweak("clear", p, false),
            "tweak final" => route_tweak("final", p, false),
            // Escape hatch for tweaker ops that don't have (or need) a named
            // command yet: `tweak op op=NAME ...` -- the callback decides.
            "tweak op" => match p.get(&["op"]) {
                Some(op) => route_tweak(&op.to_string(), p, false),
                None => err("need op="),
            },
            // Widget data operations and structural edits share the widgets callback.
            "design" => route_design(r.sub.as_deref().unwrap_or(""), p),
            // The shader constant tables: annotated literals compiled into
            // a hot-patchable uniform buffer (see Cx::shader_const_patch).
            "shader consts" => {
                let shader = p
                    .get(&["shader", "s"])
                    .and_then(|v| v.parse::<usize>().ok());
                reply_to_answer(ask("shader consts", WaitKind::Turn, move |tx| Cmd::ShaderConsts { shader, tx }))
            }
            "shader const" => {
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
                reply_to_answer(ask("shader const", WaitKind::Turn,
                    move |tx| Cmd::ShaderConstPatch {
                        shader,
                        index,
                        value,
                        tx,
                    },
                ))
            }
            "gpu_inventory" => {
                let n = p.get(&["n"]).and_then(|v| v.parse::<usize>().ok()).unwrap_or(15);
                reply_to_answer(ask("gpu_inventory", WaitKind::Turn, move |tx| Cmd::GpuInventory { n, tx }))
            }
            "quit" => reply_to_answer(ask("quit", WaitKind::Turn, |tx| quit_command(p, tx))),
            // `cap start`, `cap stop`, `cursor`, `midi`, `log`: commands that
            // need platform state are answered by platform.
            other => platform_command(other, p).unwrap_or_else(|| err("no such command")),
        }
    }

    fn quit_command(p: &Params, tx: Sender<Reply>) -> Cmd {
        let force = p.flag(&["force"]);
        if p.flag(&["native"]) {
            Cmd::QuitNative { force, tx }
        } else if force {
            Cmd::QuitForced(tx)
        } else {
            Cmd::Quit(tx)
        }
    }

    /// The commands that need platform state.
    fn platform_command(name: &str, p: &Params) -> Option<Answer> {
        Some(match name {
            "log" => route_log(p),
            "cpu" => Answer::Text(crate::thread::cpu_summary()),
            "midi" => route_midi(p),
            "cap start" => route_capture_start(p),
            "cap stop" => route_capture_stop(p),
            "cursor" => route_cursor(p),
            "input record" => super::input_rec::record(p),
            "input replay" => super::input_rec::replay(p),
            "input stop" => super::input_rec::stop(),
            "input status" => super::input_rec::status(),
            _ => return None,
        })
    }

    // For `deliver_grab_pixels`, the raw pixel path Metal takes.
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
        /// 0: a standalone grab; else the `frames n=N` series whose refilled chunks join it
        series: u64,
        window: WindowId,
        /// `(grab id, due)` in cadence order
        ids: std::collections::VecDeque<(u64, Instant)>,
        last_request: Option<u64>,
        readback_wait_logged: bool,
        cancelled: Arc<AtomicBool>,
    }

    /// The grab request `id` still waits for its frame: a screenshot request not yet served, or
    /// a readback still pending.
    fn grab_request_pending(cx: &Cx, tickets: &HashMap<ReadbackTicket, u64>, id: u64) -> bool {
        cx.screenshot_requests.iter().any(|request| request.request_id == id)
            || tickets.iter().any(|(ticket, request)| {
                *request == id
                    && cx
                        .textures
                        .1
                        .readbacks
                        .slots
                        .iter()
                        .any(|slot| slot.result.ticket == *ticket && slot.pending)
            })
    }

    #[derive(Default)]
    struct Captures {
        batches: Vec<GrabBatch>,
        windows: HashMap<u64, usize>,
        tickets: HashMap<ReadbackTicket, u64>,
        ready: Vec<(u64, TextureReadback)>,
        failures: Vec<(u64, String)>,
    }

    struct FrameWaiter {
        live: CommandLive,
        window: Option<WindowId>,
        target_repaint: u64,
        min_live_edit_seq: Option<u64>,
        tx: Sender<Reply>,
        payload: Option<String>,
        post_frame_rect: Option<(u64, Option<usize>)>,
        local_user_seq: u64,
    }

    struct ReloadWaiter {
        file: Option<String>,
        op: String,
        args: Vec<(String, String)>,
        tx: Sender<Reply>,
        live: CommandLive,
        by: String,
    }

    thread_local! {
        // This state belongs only to the UI, unlike the HTTP reply sinks.
        static CAPTURES: std::cell::RefCell<Captures> = Default::default();
        static FRAME_WAITERS: std::cell::RefCell<Vec<FrameWaiter>> = Default::default();
        static RELOAD_WAITERS: std::cell::RefCell<Vec<ReloadWaiter>> = Default::default();
        /// The `/step` in progress, if any: UI-owned, like the captures.
        static STEP: std::cell::RefCell<Option<StepJob>> = const { std::cell::RefCell::new(None) };
        /// The `/cap/start` session as the UI sees it.
        static CAPTURE: std::cell::RefCell<Option<UiCapture>> = const { std::cell::RefCell::new(None) };
        /// The file a `drag` holds over a window, until `drag_end` or `drop`.
        static FILE_DRAG: std::cell::RefCell<Option<FileDrag>> = const { std::cell::RefCell::new(None) };
    }

    pub(crate) fn on_loop_exit() {
        FRAME_WAITERS.with_borrow_mut(Vec::clear);
        RELOAD_WAITERS.with_borrow_mut(Vec::clear);
        STEP.with_borrow_mut(Option::take);
        CAPTURES.with_borrow_mut(|captures| *captures = Captures::default());
        if ACTIVE.load(Ordering::Relaxed) {
            queue().lock().unwrap_or_else(|e| e.into_inner()).clear();
            grab_sinks().lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
    }

    /// A file held over a window by `drag`: its items (one list for the whole drag, as a native
    /// drag has) and where it was last.
    struct FileDrag {
        window_id: WindowId,
        path: String,
        items: Arc<Vec<crate::event::DragItem>>,
        at: Vec2d,
    }

    // ------------------------------------------------------------------
    // command queue
    // ------------------------------------------------------------------

    /// A `/step` in progress (UI thread only).
    struct StepJob {
        live: CommandLive,
        window: WindowId,
        fps: u32,
        remaining: u64,
        open: Option<OpenFrame>,
        /// Since when the step has been waiting on the capture encoder.
        wait_since: Option<Instant>,
        /// `wait_loads=1`: no frame opens while asynchronous loads (image
        /// decodes, glyph rasters, fetched resources) are in flight.
        wait_loads: bool,
        /// An empty snapshot starts a fresh wait before the next frame.
        last_loads: Vec<(&'static str, usize)>,
        last_glyph_answers: u64,
        last_load_progress: Instant,
        tx: Sender<Reply>,
    }

    /// The step frame being presented: advanced, dispatched, not yet on the
    /// GPU (a drawable or a shader pipeline may still be on its way).
    struct OpenFrame {
        submission: u64,
        /// The capture request id and the frame index it carries.
        capture: Option<(u64, u64)>,
        opened: Instant,
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
    /// How long one step frame may fail to reach the screen, or a step may
    /// wait on the capture encoder, before `/step` gives up with an error.
    const STEP_FRAME_LIMIT: Duration = Duration::from_secs(20);

    /// How long pending counts and received glyph answers may both stay unchanged before a step fails.
    const STEP_LOAD_STALL_LIMIT: Duration = Duration::from_secs(30);

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

    /// When this app instance (or Relax domain) started, seeded at the top of
    /// `start_if_requested` (from `Cx::new`) before the remote endpoint binds or
    /// registers `"app"`. Domain-local under Relax so an in-place JIT restart
    /// starts a fresh clock instead of inheriting the OS process's age.
    static STARTED: OnceLock<Instant> = OnceLock::new();

    /// Elapsed time since this app instance (or Relax domain) started.
    pub fn app_age() -> Duration {
        STARTED.get_or_init(Instant::now).elapsed()
    }

    /// Opens the process's makepad-remote endpoint (the one place it is opened, from `Cx::new`,
    /// so every process with a `Cx` can have it): a same-user Unix socket, nothing on the
    /// network, when the one rule asks for it ([`request`]): off by default; on for an agent
    /// (`--remote`, `MAKEPAD_REMOTE=1`), and as an ordinary launch on a developer computer
    /// (`MAKEPAD_REMOTE=local` or the per-user setting), so a local agent reaches the apps the
    /// person started there (a release Android or iOS app opens it only when asked). The
    /// `cmd` service answers the remote commands and the GPU ledger's, the `frames`
    /// service streams window images, and the `app` service lists the built-ins until an upper
    /// crate registers the process's AI tools with [`register_service`]. An instance launched for an agent (`--remote`,
    /// `MAKEPAD_REMOTE=1`, `MAKEPAD_INSPECT=1`) prints `[makepad-remote] pid=<pid> app=<app> socket=<path>`. A port or
    /// address (the old HTTP forms) is refused with a message and exit status 2. Under
    /// `makepad run` the host owns the endpoint and the commands and services are registered with it.
    pub fn start_if_requested() {
        let _ = STARTED.get_or_init(Instant::now);
        let driven = match request() {
            Request::Off => return,
            Request::Refused(why) => {
                eprintln!("[makepad-remote] {why}");
                makepad_core_util::process_end::end(2);
            }
            Request::Local => false,
            Request::On => true,
        };
        if ACTIVE.load(Ordering::Relaxed) {
            return;
        }
        let app = app_name();
        let pid = std::process::id();
        {
            let mut status = status_cell().lock().unwrap();
            status.app = app.clone();
            status.pid = pid;
        }
        #[cfg(relax)]
        {
            let _ = driven;
            register_with_host();
            for (name, ..) in APP_QUERIES.lock().unwrap().iter() {
                super::host_registration::register_with_host(name);
            }
            // held until ACTIVE is set, so a service registered meanwhile is not lost
            let mut pending = PENDING_SERVICES.lock().unwrap();
            super::host_registration::register_service_with_host("app", 1, no_app_open, no_app_close, None);
            for (name, version, open, close, data) in std::mem::take(&mut *pending) {
                super::host_registration::register_service_with_host(&name, version, open, close, data);
            }
            ACTIVE.store(true, Ordering::SeqCst);
            drop(pending);
            super::input_rec::replay_at_start();
        }
        #[cfg(not(relax))]
        {
            // the UI thread (start_if_requested runs in Cx::new): `prof threads=main`
            #[cfg(target_os = "macos")]
            crate::prof_sampler::note_main_thread();
            let endpoint = makepad_remote::Endpoint::new(&app);
            endpoint.register_cmd(dispatch);
            endpoint.set_end_fields(end_fields);
            endpoint.register("frames", 1, frames_service());
            // the profiler stream (Scope's Live, `makepad inspect <app> pf`)
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            endpoint.register("prof", 1, crate::prof_native::service(app.clone()));
            endpoint.register("gpuprof", 1, makepad_hal::gpu_prof::stream_service());
            // held until ACTIVE is set, so a service registered meanwhile is not lost
            let mut pending = PENDING_SERVICES.lock().unwrap();
            endpoint.register_program("app", 1, no_app_open, no_app_close, None);
            for (name, version, open, close, data) in std::mem::take(&mut *pending) {
                endpoint.register_program(&name, version, open, close, data);
            }
            // An iOS app's files, sockets included, live in its sandbox container, which no other
            // process can open: there is no local route to listen on (a paired network carrier
            // would be one). Said once, not as a failure to listen.
            #[cfg(target_os = "ios")]
            {
                let _ = (driven, pid, endpoint, pending);
                static SAID: std::sync::Once = std::sync::Once::new();
                SAID.call_once(|| eprintln!("[makepad-remote] iOS has no local remote-control route: no other process can reach the app's sandbox"));
                // a recorded input replay needs no route
                super::input_rec::replay_at_start();
            }
            #[cfg(not(target_os = "ios"))]
            match endpoint.listen_unix("") {
                Ok(socket) => {
                    ACTIVE.store(true, Ordering::SeqCst);
                    *ENDPOINT.lock().unwrap() = Some(endpoint);
                    if driven {
                        eprintln!("{}", makepad_remote::startup_line("makepad", pid, &app, &socket));
                    }
                    super::input_rec::replay_at_start();
                }
                Err(err) => eprintln!("[makepad-remote] cannot listen: {err}"),
            }
            let _ = std::io::stderr().flush();
        }
    }

    #[cfg(not(relax))]
    static ENDPOINT: Mutex<Option<makepad_remote::Endpoint>> = Mutex::new(None);

    /// The HAL inspector's command table (the words are the request).
    pub(crate) fn gpu_commands() -> Vec<crate::remote_schema::Command> {
        makepad_hal::inspect::COMMANDS
            .iter()
            .chain(makepad_hal::a11y::COMMANDS.iter())
            .chain(makepad_hal::framecap::COMMANDS.iter())
            .chain(makepad_hal::gpu_prof::COMMANDS.iter())
            .map(|(words, args, answers)| crate::remote_schema::Command {
                words,
                args,
                answers,
                service: if words.starts_with("ui") { "ui" } else { "inspect" },
                risk: if matches!(*words, "ui act" | "ui type") { crate::remote_schema::CmdRisk::Act } else { crate::remote_schema::CmdRisk::Read },
                params: if *words == "ui" || words.starts_with("ui ") {
                    makepad_remote::params::ui(words.strip_prefix("ui").unwrap().trim()).unwrap_or("")
                } else { makepad_remote::params::inspect(words).unwrap_or("") },
                required: &[],
                positional: if *words == "ui act" { &["action"] } else { &[] },
            })
            .collect()
    }

    /// The built-in commands as the `app` services `ui`, `edit`, `logs`, `inspect`: their
    /// `Instance` lines for an `app` list (remote_schema.rs).
    pub fn builtin_instances() -> Vec<String> {
        let gpu = gpu_commands();
        let all: Vec<&crate::remote_schema::Command> = COMMANDS.iter().chain(gpu.iter()).collect();
        crate::remote_schema::instance_lines(&all)
    }

    /// A call of a built-in service's tool, run as the `cmd` request it stands for: the `Result`
    /// line. `None` when `service` is not a built-in one.
    pub fn builtin_call(call_id: &str, service: &str, tool: &str, args: &str) -> Option<String> {
        use crate::remote_schema::{request_line, result_line, tool_name};
        if !crate::remote_schema::SERVICES.iter().any(|s| s.0 == service) {
            return None;
        }
        let gpu = gpu_commands();
        let Some(c) = COMMANDS.iter().chain(gpu.iter()).find(|c| c.service == service && tool_name(c.words) == tool) else {
            return Some(result_line(call_id, false, &format!("`{service}` has no tool `{tool}` (help lists them)")));
        };
        if CLIENT_SIDE.contains(&c.words) {
            return Some(result_line(call_id, false, &format!("`{service}.{tool}` reads the frames service: the caller runs it (makepad-remote app <pid> {})", c.words)));
        }
        let line = match request_line(c, args) {
            Ok(l) => l,
            Err(why) => return Some(result_line(call_id, false, &why)),
        };
        let reply = makepad_remote::cmd::answer(dispatch, &line);
        let text = String::from_utf8_lossy(&reply).trim_end().to_string();
        let failed = text.lines().any(|l| l.starts_with("{\"err\":"));
        Some(result_line(call_id, !failed, &text))
    }

    /// Compact age for remote status notes: `<N> s`, `<N> min`, or `<N> h`.
    fn format_age(age: Duration) -> String {
        let secs = age.as_secs();
        if secs < 60 {
            format!("{secs} s")
        } else if secs < 3600 {
            format!("{} min", secs / 60)
        } else {
            format!("{} h", secs / 3600)
        }
    }

    /// App-neutral status note when no app-owned AI services are open yet.
    /// Shared by the platform placeholder (`no_app_open`) and `app_service`
    /// (while zero services are connected in `ServiceRegistry::process`).
    pub fn no_app_tools_note_for(age: Duration) -> String {
        format!(
            "no AI tools of its own open yet (app started {} ago); an app may open them once it has started — try again, or see ui/logs",
            format_age(age)
        )
    }

    /// [`no_app_tools_note_for`] for this instance's current [`app_age`].
    pub fn no_app_tools_note() -> String {
        no_app_tools_note_for(app_age())
    }

    /// The `AppLine::Progress` JSON line emitted on `list` when no app-owned AI tools are open yet.
    pub fn no_app_tools_progress_line(age: Duration) -> String {
        format!(
            "{{\"Progress\":{{\"note\":{},\"permille\":0}}}}",
            json_str(&no_app_tools_note_for(age))
        )
    }

    fn placeholder_preflight(args: &str, age: Duration) -> Result<(), String> {
        let head = args.split_once('\n').map_or(args, |(h, _)| h);
        let mut words = head.split_whitespace();
        match words.next().unwrap_or("") {
            "list" | "context" => Ok(()),
            "call" => {
                let Some((service, _)) = words
                    .next()
                    .and_then(|t| t.split_once('.'))
                    .filter(|(s, t)| !s.is_empty() && !t.is_empty())
                else {
                    return Err("app: list, context or call <service>.<tool>".into());
                };
                if crate::remote_schema::SERVICES.iter().any(|s| s.0 == service) {
                    Ok(())
                } else {
                    Err(no_app_tools_note_for(age))
                }
            }
            _ => Err("app: list, context or call <service>.<tool>".into()),
        }
    }

    fn placeholder_answer(args: &str, age: Duration) -> Vec<String> {
        let mut lines = builtin_answer(args);
        let head = args.split_once('\n').map_or(args, |(h, _)| h);
        if head.split_whitespace().next() == Some("list") {
            lines.push(no_app_tools_progress_line(age));
        }
        lines
    }

    /// Serves the built-ins and reports the app's age on `list` and non-built-in `call`
    /// until an upper crate registers `"app"`.
    extern "C" fn no_app_open(args: *const u8, len: usize, sink: makepad_remote::program::ServiceSink, ctx: *mut std::ffi::c_void) -> usize {
        let age = app_age();
        let args = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(args, len) }).into_owned();
        if let Err(why) = placeholder_preflight(&args, age) {
            sink(ctx, 2, why.as_ptr(), why.len());
            return 0;
        }
        // answered on its own thread: a call waits for the event loop
        let ctx = ctx as usize;
        let spawned = std::thread::Builder::new().name("makepad-remote-app".into()).spawn(move || {
            let lines = placeholder_answer(&args, age);
            for l in lines {
                if !sink(ctx as *mut std::ffi::c_void, 0, l.as_ptr(), l.len()) {
                    break;
                }
            }
            sink(ctx as *mut std::ffi::c_void, 1, std::ptr::null(), 0);
        });
        if spawned.is_err() {
            let why = "cannot start the app thread";
            sink(ctx as *mut std::ffi::c_void, 2, why.as_ptr(), why.len());
            return 0;
        }
        1
    }

    /// The answer lines of an `app` request the built-in services take: `list` (their
    /// instances), `context` (none), or a call of one of their tools.
    pub fn builtin_answer(args: &str) -> Vec<String> {
        let (head, body) = args.split_once('\n').unwrap_or((args, ""));
        let mut words = head.split_whitespace();
        match words.next() {
            Some("list") => builtin_instances(),
            Some("context") => vec!["{\"Context\":{\"text\":\"\"}}".to_string()],
            Some("call") => words
                .next()
                .and_then(|t| t.split_once('.'))
                .and_then(|(service, tool)| builtin_call("app0", service, tool, if body.trim().is_empty() { "{}" } else { body }))
                .into_iter()
                .collect(),
            _ => Vec::new(),
        }
    }

    /// An app's own inspect command: `f(cx, words, kv)` runs on the UI thread and returns the
    /// answer's JSON lines (an `{"err":..}` line for an error); words[0] is the command.
    pub type AppQueryFn = fn(&mut Cx, &[String], &[(String, String)]) -> Vec<String>;

    /// JSON-schema properties for these command words, checked before queuing the callback.
    pub type AppQueryParams = fn(&[String]) -> &'static str;

    static APP_QUERIES: Mutex<Vec<(&'static str, &'static str, &'static str, AppQueryParams, AppQueryFn, crate::remote_schema::CmdRisk)>> = Mutex::new(Vec::new());

    /// Registers the app's inspect command `name` (`makepad inspect <app> game actors`): what a
    /// person or an agent asks the live app about its own state (a game's entities and actors).
    /// `args` and `answers` are its `help` line. Read-only by convention; a later registration of
    /// the same name replaces the earlier one. `params` selects its per-command JSON properties.
    pub fn register_app_query(name: &'static str, args: &'static str, answers: &'static str, params: AppQueryParams, f: AppQueryFn) {
        register_app(name, args, answers, params, f, crate::remote_schema::CmdRisk::Read);
    }

    /// Registers the app's own command `name` that changes what the app does (a hold on its
    /// work, a mode a test puts it in): as [`register_app_query`], listed in `help` as an act.
    pub fn register_app_command(name: &'static str, args: &'static str, answers: &'static str, params: AppQueryParams, f: AppQueryFn) {
        register_app(name, args, answers, params, f, crate::remote_schema::CmdRisk::Act);
    }

    fn register_app(name: &'static str, args: &'static str, answers: &'static str, params: AppQueryParams, f: AppQueryFn, risk: crate::remote_schema::CmdRisk) {
        let mut q = APP_QUERIES.lock().unwrap();
        q.retain(|x| x.0 != name);
        q.push((name, args, answers, params, f, risk));
        drop(q);
        #[cfg(relax)]
        if ACTIVE.load(Ordering::SeqCst) {
            super::host_registration::register_with_host(name);
        }
    }

    fn app_query(name: &str) -> Option<(AppQueryParams, AppQueryFn)> {
        APP_QUERIES.lock().unwrap().iter().find(|x| x.0 == name).map(|x| (x.3, x.4))
    }

    extern "C" fn no_app_close(_handle: usize) {}
    static PENDING_SERVICES: Mutex<Vec<(String, u32, ServiceOpen, ServiceClose, Option<ServiceData>)>> = Mutex::new(Vec::new());

    /// Serves `name` on this process's endpoint with a producer per channel
    /// ([`makepad_remote::program`]): the process's own endpoint natively, the Relax host's under
    /// `makepad run`. Registered before the endpoint is up, it is served once it is; with the
    /// endpoint off (the default, or `MAKEPAD_REMOTE=0`) it is never served.
    /// An optional DATA callback takes client messages in order on the service thread.
    pub fn register_service(
        name: &str,
        version: u32,
        open: ServiceOpen,
        close: ServiceClose,
        data: Option<ServiceData>,
    ) {
        let mut pending = PENDING_SERVICES.lock().unwrap();
        if !ACTIVE.load(Ordering::SeqCst) {
            pending.retain(|s| s.0 != name);
            pending.push((name.to_string(), version, open, close, data));
            return;
        }
        drop(pending);
        #[cfg(relax)]
        super::host_registration::register_service_with_host(name, version, open, close, data);
        #[cfg(not(relax))]
        if let Some(endpoint) = ENDPOINT.lock().unwrap().as_ref() {
            endpoint.register_program(name, version, open, close, data);
        }
    }

    /// The `cmd` service of this process: the remote commands (`help` lists them) and the GPU
    /// ledger's `gpu ...`.
    pub(crate) fn dispatch(req: &Req, out: &mut Out) {
        let _gone = RequestGoneScope::enter(out.gone());
        if matches!(req.cmd.as_str(), "help" | "ping") {
            if let Err(e) = req.validate_params("") { return out.err(&e); }
        }
        REQUEST_START_USER_SEQ.set(activity::user_seq_now());
        match req.cmd.as_str() {
            "ping" => out.line(&format!(
                "{{\"pid\":{},\"app\":{},\"ms\":{}}}",
                std::process::id(),
                json_str(&app_name()),
                makepad_remote::now_ms()
            )),
            "help" => {
                out.measured("commands", "static", &[]);
                for (name, args, answers, _, _, risk) in APP_QUERIES.lock().unwrap().iter() {
                    out.line(&format!(
                        "{{\"cmd\":{},\"args\":{},\"answers\":{},\"tool\":{},\"risk\":\"{:?}\"}}",
                        json_str(name),
                        json_str(args),
                        json_str(answers),
                        json_str(&format!("app.{}", name)),
                        risk
                    ));
                }
                for c in COMMANDS.iter().chain(gpu_commands().iter()) {
                    out.line(&format!(
                        "{{\"cmd\":{},\"args\":{},\"answers\":{},\"tool\":{},\"risk\":\"{:?}\"}}",
                        json_str(c.words),
                        json_str(c.args),
                        json_str(c.answers),
                        json_str(&format!("{}.{}", c.service, crate::remote_schema::tool_name(c.words))),
                        c.risk
                    ));
                }
            }
            "" => out.err("empty request"),
            c if app_query(c).is_some() => {
                let (params, f) = app_query(c).unwrap();
                let mut words = vec![req.cmd.clone()];
                words.extend(req.pos.iter().cloned());
                if let Err(e) = req.validate_params(params(&words)) { return out.err(&e); }
                let kv = req.kv.clone();
                match ask("app query", WaitKind::Turn, move |tx| Cmd::Platform(Box::new(PlatformCmd::AppQuery { f, words, kv, tx }))) {
                    Reply::Text(t) => {
                        for l in t.lines() {
                            out.line(l);
                        }
                    }
                    Reply::Err(e) | Reply::Conflict(e) => out.err(&e),
                    Reply::Ok => out.line("{\"ok\":1}"),
                }
            }
            _ => {
                // Relax dispatches GPU queries in its host; program code only serves UI commands.
                #[cfg(not(relax))]
                if makepad_hal::a11y::dispatch(req, out) || makepad_hal::inspect::dispatch(req, out) {
                    return;
                }
                // the sampling profiler (inspect's `prof` / `why-slow`, same buckets and lines)
                #[cfg(all(not(relax), target_os = "macos"))]
                match req.cmd.as_str() {
                    "prof" => return crate::prof_sampler::cmd_prof(req, out),
                    "why-slow" => return crate::prof_sampler::cmd_why_slow(req, out),
                    _ => {}
                }
                let mut words = vec![req.cmd.clone()];
                words.extend(req.pos.iter().cloned());
                let resolved = match resolve(&words) {
                    Ok(r) => r,
                    Err(why) => return out.err(&why),
                };
                // words after the command are flags (`snap all` = `snap all=1`)
                let mut args = req.kv.clone();
                args.extend(words[resolved.used..].iter().map(|w| (w.clone(), "1".to_string())));
                match command(&resolved, &Params(args)) {
                    Answer::Json(text) | Answer::Text(text) => out.line(&text),
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // under `makepad run`: the host owns the endpoint
    // ------------------------------------------------------------------

    /// A command for the Relax host: the request line in, the reply lines out through `sink`
    /// as they are produced (chunks, not lines).
    #[cfg(relax)]
    pub(crate) extern "C" fn host_query(line: *const u8, len: usize, sink: extern "C" fn(*const u8, usize, *mut std::ffi::c_void), ctx: *mut std::ffi::c_void) {
        struct SinkWriter(extern "C" fn(*const u8, usize, *mut std::ffi::c_void), *mut std::ffi::c_void);
        impl Write for SinkWriter {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                (self.0)(b.as_ptr(), b.len(), self.1);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let line = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(line, len)) };
        let mut w = std::io::BufWriter::with_capacity(64 << 10, SinkWriter(sink, ctx));
        makepad_remote::cmd::answer_to(dispatch, line, &mut w);
        let _ = w.flush();
    }

    /// The host's sink of a service channel: `kind` 0 sends `ptr..len` as one message (false: the
    /// client is gone), 1 ends the stream, 2 ends it with `ptr..len` as the reason.
    #[cfg(relax)]
    type HostSink = extern "C" fn(ctx: *mut std::ffi::c_void, kind: u32, ptr: *const u8, len: usize) -> bool;

    #[cfg(relax)]
    static HOST_FRAMES: Mutex<Vec<(usize, std::sync::mpsc::SyncSender<makepad_remote::Event>)>> = Mutex::new(Vec::new());

    /// `open` of the `frames` service for the host: starts a producer feeding `sink`.
    #[cfg(relax)]
    extern "C" fn host_frames_open(args: *const u8, len: usize, sink: HostSink, ctx: *mut std::ffi::c_void) -> usize {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        let args = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(args, len)) };
        let parsed = match parse_frames_args(args) {
            Ok(a) => a,
            Err(why) => {
                sink(ctx, 2, why.as_ptr(), why.len());
                return 0;
            }
        };
        if parsed.video.is_some() && crate::cx::relax_host_symbol(c"relax_register_remote_service_data").is_null() {
            let why = "frames: this host lacks bidirectional video capture";
            sink(ctx, 2, why.as_ptr(), why.len());
            return 0;
        }
        let handle = NEXT.fetch_add(1, Ordering::Relaxed);
        let (inbound, events) = std::sync::mpsc::sync_channel(1);
        HOST_FRAMES.lock().unwrap().push((handle, inbound));
        let ctx = ctx as usize;
        let spawned = std::thread::Builder::new().name("makepad-remote-frames".into()).spawn(move || {
            let ctx = ctx as *mut std::ffi::c_void;
            let ended = Arc::new(AtomicBool::new(false));
            let (ended_by_close, ended_after) = (ended.clone(), ended.clone());
            let ctx_close = ctx as usize;
            let ctx_send = ctx as usize;
            run_frames(
                parsed,
                FramesOut {
                    send: Box::new(move |record, _wait| sink(ctx_send as *mut std::ffi::c_void, 0, record.as_ptr(), record.len())),
                    try_send: Box::new(move |record| {
                        if sink(ctx_send as *mut std::ffi::c_void, 3, record.as_ptr(), record.len()) {
                            Ok(())
                        } else {
                            Err(makepad_remote::conn::SendError::Full(record))
                        }
                    }),
                    close: Box::new(move |code, why| {
                        if !ended_by_close.swap(true, Ordering::SeqCst) {
                            let kind = if code == 0 { 1 } else { 2 };
                            sink(ctx_close as *mut std::ffi::c_void, kind, why.as_ptr(), why.len());
                        }
                    }),
                    try_recv: Box::new(move || events.try_recv().map_err(|e| e == std::sync::mpsc::TryRecvError::Empty)),
                },
            );
            // a producer that stopped without ending the stream (the client left): the host
            // still gets its final call, which frees the channel's state
            if !ended_after.swap(true, Ordering::SeqCst) {
                sink(ctx, 1, std::ptr::null(), 0);
            }
            HOST_FRAMES.lock().unwrap().retain(|(h, _)| *h != handle);
        });
        if spawned.is_err() {
            HOST_FRAMES.lock().unwrap().retain(|(h, _)| *h != handle);
            let why = "cannot start the frames thread";
            sink(ctx as *mut std::ffi::c_void, 2, why.as_ptr(), why.len());
            return 0;
        }
        handle
    }

    /// False with a full inbox: the host keeps this message and retries before the next one.
    #[cfg(relax)]
    extern "C" fn host_frames_data(handle: usize, ptr: *const u8, len: usize) -> bool {
        let tx = HOST_FRAMES.lock().unwrap().iter().find(|(h, _)| *h == handle).map(|(_, tx)| tx.clone());
        let Some(tx) = tx else { return true };
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        !matches!(tx.try_send(makepad_remote::Event::Data(bytes.to_vec())), Err(std::sync::mpsc::TrySendError::Full(_)))
    }

    /// Disconnect the inbox. Queued DATA precedes the disconnect as it does on a native channel.
    #[cfg(relax)]
    extern "C" fn host_frames_close(handle: usize) {
        HOST_FRAMES.lock().unwrap().retain(|(h, _)| *h != handle);
    }

    /// Registers every remote command word and the `frames` service with the Relax host (its own
    /// commands win a clash): the one mechanism for what a program offers its host.
    #[cfg(relax)]
    fn register_with_host() {
        for c in COMMANDS {
            let first = c.words.split(' ').next().unwrap_or(c.words);
            super::host_registration::register_with_host(first);
        }
        super::host_registration::register_service_with_host("frames", 1, host_frames_open, host_frames_close, Some(host_frames_data));
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
    pub(crate) fn needs_ticks() -> bool {
        if !ACTIVE.load(Ordering::Relaxed) {
            return false;
        }
        !queue().try_lock().map(|q| q.is_empty()).unwrap_or(false)
            || FRAME_WAITERS.with_borrow(|waiters| !waiters.is_empty())
            || has_reload_waiters()
            || PENDING_GRABS.load(Ordering::Relaxed) != 0
            || STEP.with_borrow(|step| step.is_some())
            || CAPTURE.with_borrow(|capture| capture.as_ref().is_some_and(|c| !c.virtual_clock))
    }

    /// A remote request waits for the next painted frame (a resize, an input
    /// or a tweak with `wait`): under the virtual clock the app draws for it
    /// (`app_draws_now`).
    pub(crate) fn frame_awaited() -> bool {
        FRAME_WAITERS.with_borrow(|waiters| !waiters.is_empty())
    }

    fn has_reload_waiters() -> bool {
        RELOAD_WAITERS.with_borrow(|waiters| !waiters.is_empty())
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
        if requested() {
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
        if requested() {
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

    /// Every id is a remote grab or capture: answering them only hands the pixels to the
    /// encoder (no PNG on the way), short work.
    pub(crate) fn grab_ids_only(ids: &[u64]) -> bool {
        ids.iter().all(|id| is_grab_id(*id))
    }

    /// Whether a pending screenshot request waits on `window_id`'s pass, without
    /// taking it: a backend that paces presents (Wayland frame callbacks, which a
    /// compositor withholds from an occluded window) still draws that window.
    pub(crate) fn grab_waits_on_window(request_id: u64, window_id: usize) -> bool {
        if !is_grab_id(request_id) {
            return true;
        }
        if capture::is_capture_id(request_id) {
            return CAPTURE.with_borrow(|capture| capture.as_ref().map(|c| c.window)) == Some(window_id);
        }
        CAPTURES.with_borrow(|captures| captures.windows.get(&request_id).is_none_or(|want| *want == window_id))
    }

    /// The frame build took the snapshot of `ids` in frame `frame` (`Cx::repaint_id`), with
    /// `patch` live patches applied: the grabs' records carry both.
    pub(crate) fn note_grabs_drawn(ids: &[u64], frame: u64, patch: u64) {
        grabs_drawn(ids, Drawn { frame, patch });
    }

    /// Hand a finished PNG to whichever grab requests asked for it. Returns the
    /// ids that were *not* remote grabs, so the caller can route them onwards.
    /// Compatibility path for backends that still supply PNGs. The worker
    /// handles decoding/scaling; capture_kind makes their timing limitation
    /// explicit. Metal's raw path bypasses this full-size encoding entirely.
    pub(crate) fn fail_grabs(ids: &[u64], why: &str) {
        CAPTURES.with_borrow_mut(|captures| {
            for id in ids { captures.failures.push((*id, why.to_string())); }
        });
    }

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
                            pixels: crate::HostBuf::from_vec(png.to_vec()),
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
    pub(crate) fn deliver_grab_pixels(
        request_ids: Vec<u64>,
        width: u32,
        height: u32,
        pixels: crate::HostBuf,
        stride: usize,
        order: ReadbackChannelOrder,
        origin: ReadbackOrigin,
    ) -> Vec<u64> {
        if !ACTIVE.load(Ordering::Relaxed) {
            return request_ids;
        }
        let captured = Instant::now();
        // Every holder of this lock keeps it for a few map updates (a send under it never
        // waits), so the UI takes it too: the HAL's grabs come here from the UI thread.
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
                let Some(&(id, due)) = batch.ids.front() else {
                    // A series between chunks keeps its last request, so the next refilled
                    // frame still waits for it; a finished batch goes.
                    return batch.series != 0
                        && batch.last_request.is_some_and(|previous| grab_request_pending(cx, &captures.tickets, previous));
                };
                if now < due {
                    return true;
                }
                // Wait for the preceding frame to be ENCODED, not read
                // back. A skipped/minimized drawable must not accumulate
                // several sequence requests on one eventual presentation.
                if batch.last_request.is_some_and(|previous| grab_request_pending(cx, &captures.tickets, previous)) {
                    return true;
                }
                if !cx.windows.is_valid(batch.window) || !cx.windows[batch.window].is_created {
                    if let Ok(mut sinks) = grab_sinks().try_lock() {
                        for (id, _) in batch.ids.drain(..) {
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
                // The encoder consumes RGBA8; other attachments use the window grab.
                let texture = texture.filter(|texture| {
                    cx.in_makepad_studio
                        && matches!(
                            cx.textures[texture.texture_id()].format,
                            crate::TextureFormat::RenderBGRAu8 { .. }
                        )
                });
                let ticket = match texture {
                    Some(texture) => match texture.read_back(cx, ReadbackRequest { next_render: true }) {
                        Ok(ticket) => {
                            batch.readback_wait_logged = false;
                            Some(ticket)
                        }
                        Err(ReadbackError::Backpressure) => {
                            if !batch.readback_wait_logged {
                                crate::log!("remote grab: context readback request limit reached; grab {id} waits");
                                batch.readback_wait_logged = true;
                            }
                            // The next presentation polls this same batch again.
                            cx.request_remote_window_present(batch.window);
                            return true;
                        }
                        Err(_) => None,
                    },
                    None => None,
                };
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
                batch.readback_wait_logged = false;
                batch.ids.pop_front();
                batch.last_request = Some(id);
                // Preserve draw-buffer tweaks at rest; the native boundary
                // also records any redraw already pending from earlier input.
                cx.request_remote_window_present(batch.window);
                crate::trace!(
                    "remote.grab",
                    "arm id={} window={} repaint={} late_ms={:.3}",
                    id,
                    batch.window.id(),
                    cx.repaint_id,
                    now.saturating_duration_since(due).as_secs_f64() * 1000.0
                );
                // a series stays while this frame is pending, for its next chunk
                !batch.ids.is_empty() || batch.series != 0
            });
        });
    }

    // ------------------------------------------------------------------
    // event-loop side
    // ------------------------------------------------------------------

    /// Drain the command queue and publish window state. Called from
    /// `Cx::poll_control_channel`, i.e. from the event loop of every backend.
    /// Drain app-side control work. The backend's next frame completes grabs and frame waits.
    pub(crate) fn poll(cx: &mut Cx) {
        poll_frames(cx);
    }

    fn poll_frames(cx: &mut Cx) -> bool {
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
        pending |= drive_step(cx);
        for cmd in cmds {
            poll_captures(cx);
            pending |= drive_step(cx);
            let wait_window = match &cmd.cmd {
                Cmd::Input {
                    window, wait: true, ..
                } => Some(*window),
                Cmd::Tweak { wait: true, .. } | Cmd::Ai { wait: true, .. } => Some(None),
                _ => None,
            };
            // what this thread runs for a remote command is counted as served (a test's idle rule
            // leaves it out, wherever its asks fall); for one that only observes the app, as the
            // thread's answering too (a test's CPU budget for it leaves that out): driving the app
            // is its work, observing it is not
            let observes = cmd.cmd.observes();
            let start = makepad_hal::clock::thread_cpu_ns();
            apply(cx, cmd);
            let ran = makepad_hal::clock::thread_cpu_ns().saturating_sub(start);
            if observes { charge_answered(ran) } else { makepad_remote::charge_served(ran) }
            // Do not batch later input ahead of a grab in this same drain.
            poll_captures(cx);
            pending |= drive_step(cx);
            if let Some(window) = wait_window.and_then(|window| resolve_window(cx, window).ok()) {
                cx.request_remote_window_present(window);
                // The input has been applied above, whatever the present
                // does: a frame that cannot be submitted now (the window is
                // busy presenting) or a pass that stays dirty is painted on
                // a later beat, and the waiters resolve on that repaint. An
                // answer of "retry" here made drivers send the input again,
                // and a busy app took every key twice ("bbrowser").
                pending = true;
            }
            resolve_reload_waiters(cx);
            resolve_frame_waiters(cx);
        }

        poll_captures(cx);
        pending |= drive_step(cx);
        resolve_reload_waiters(cx);
        resolve_frame_waiters(cx);
        pending
    }

    /// Run the `/step` in progress as far as the GPU lets it: each frame
    /// advances the virtual clock, fires due timers, delivers NextFrame, and
    /// is drawn and presented (with a capture request while a capture runs)
    /// before the next one starts. Returns whether it is still running.
    fn drive_step(cx: &mut Cx) -> bool {
        let Some(mut job) = STEP.with_borrow_mut(Option::take) else {
            return false;
        };
        let pending = loop {
            if !job.live.is_alive() { return false; }
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
                        let glyph_answers = cx.hal_glyph_answers_received();
                        if loads != job.last_loads || glyph_answers != job.last_glyph_answers {
                            job.last_loads = loads;
                            job.last_glyph_answers = glyph_answers;
                            job.last_load_progress = Instant::now();
                        }
                        if job.last_load_progress.elapsed() < STEP_LOAD_STALL_LIMIT {
                            break true;
                        }
                        let _ = job.tx.send(Reply::Err(format!(
                            "asynchronous loads made no progress for {:?} before step frame {}: {} (glyph answers queued, not yet taken by a draw: {})",
                            STEP_LOAD_STALL_LIMIT,
                            app_clock::frame() + 1,
                            loads_text(&job.last_loads),
                            cx.hal_glyph_answers_queued()
                        )));
                        return false;
                    }
                    job.last_loads.clear();
                }
                job.open = Some(open_step_frame(cx, &job));
            }
            let open = job.open.as_mut().unwrap();
            let presented = cx.hal.frames.window_completion(crate::event::hal_intake::hal_window_id(job.window)) > open.submission
                && cx.pipelines_pending() == 0;
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
            submission: cx.hal.frames.window_submission(crate::event::hal_intake::hal_window_id(job.window)),
            capture,
            opened: Instant::now(),
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

    /// The process's major page faults so far: each one a wait for a page read back from disk
    /// (swap, or a mapped file's page). A test's idle rule tells an app that waits on memory from
    /// one that sits idle by them: neither runs CPU. One getrusage call; 0 where none counts them
    /// (Windows' fault count has the soft ones in it).
    /// What the UI thread has run answering remote peers ([`charge_answered`]).
    static UI_ANSWERED_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// The UI thread ran `ns` of its CPU answering a remote peer: a command it applied that only
    /// observes the app ([`Cmd::observes`]), or a frame it drew only to build the tree a request
    /// asked for. Counted as served, and as the
    /// UI thread's answering (`uicounts`' `answered_ns`), which a test's CPU budget for the app's
    /// main thread leaves out: what observing the app costs it is not the app's work.
    pub(crate) fn charge_answered(ns: u64) {
        makepad_remote::charge_served(ns);
        UI_ANSWERED_NS.fetch_add(ns, std::sync::atomic::Ordering::Relaxed);
    }

    fn major_faults() -> u64 {
        #[cfg(unix)]
        {
            #[repr(C)]
            struct Rusage {
                /// ru_utime and ru_stime (two timevals, two longs' room each)
                times: [std::ffi::c_long; 4],
                /// ru_maxrss .. ru_nivcsw; ru_majflt is the sixth
                counts: [std::ffi::c_long; 14],
            }
            extern "C" {
                fn getrusage(who: i32, usage: *mut Rusage) -> i32;
            }
            const RUSAGE_SELF: i32 = 0;
            let mut usage = Rusage { times: [0; 4], counts: [0; 14] };
            // SAFETY: `usage` is a valid struct rusage for the call to fill
            if unsafe { getrusage(RUSAGE_SELF, &mut usage) } == 0 { usage.counts[5].max(0) as u64 } else { 0 }
        }
        #[cfg(not(unix))]
        {
            0
        }
    }

    /// Who holds the process's CPU budget slots now, for `/settled` while pool jobs wait (a starved
    /// pool's verdict names them): one entry per class, client, label and lent, with how many hold
    /// it and the longest any has held its slot. A lent slot's thread sleeps and its token is back.
    fn budget_slots() -> Vec<String> {
        let mut slots: Vec<(u8, u8, String, bool, usize, u64)> = Vec::new();
        for h in makepad_cpu_budget::tokens().holders {
            match slots.iter_mut().find(|s| s.0 == h.class && s.1 == h.client && s.2 == h.label && s.3 == h.lent) {
                Some(s) => {
                    s.4 += 1;
                    s.5 = s.5.max(h.held_ns);
                }
                None => slots.push((h.class, h.client, h.label, h.lent, 1, h.held_ns)),
            }
        }
        slots.sort_by(|a, b| b.5.cmp(&a.5));
        let class = |c: u8| ["interactive", "visible", "background"].get(c as usize).copied().unwrap_or("?");
        let client = |c| match c { makepad_cpu_budget::APP => "app", makepad_cpu_budget::HAL => "hal", _ => "?" };
        slots
            .iter()
            .map(|(c, k, label, lent, n, ns)| {
                format!("{{\"class\":\"{}\",\"client\":\"{}\",\"label\":{},\"lent\":{lent},\"n\":{n},\"held_ms\":{}}}", class(*c), client(*k), json_str(label), ns / 1_000_000)
            })
            .collect()
    }

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
        if cx.pipelines_pending() > 0 {
            reasons.push("shaders");
        }
        // Jobs on the app's pool, queued (waiting for a worker or a CPU permit: a starved pool runs
        // nothing, and its app no CPU) or running: their results are still to come. A job that runs
        // for the app's whole life belongs on a long-lived worker, not the pool, or its app never
        // settles.
        let pool = cx.task_pool_backlog();
        if pool.as_ref().is_some_and(|p| p.running > 0 || p.queued.iter().any(|&n| n > 0)) {
            reasons.push("pool");
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
        if let Some(p) = &pool {
            out.push_str(&format!(",\"pool\":{{\"queued\":[{},{},{}],\"running\":{},\"taken\":{}}}", p.queued[0], p.queued[1], p.queued[2], p.running, p.taken));
        }
        if reasons.contains(&"pool") {
            out.push_str(&format!(",\"slots\":[{}]", budget_slots().join(",")));
        }
        if let Some(now) = app_clock::now() {
            out.push_str(&format!(",\"frame\":{},\"time\":{}", app_clock::frame(), num(now)));
        }
        out.push('}');
        out
    }

    /// Keep the capture overlay's cursor in step with injected pointer input.
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

    /// An unregistered file goes straight to the callback for its usual error.
    /// A verify without a file waits for all queued preview and revert work.
    fn verify_pending_reload(cx: &Cx, args: &[(String, String)]) -> Option<Option<String>> {
        let file = match args.iter().find(|(k, _)| k == "file") {
            Some((_, raw)) => Some(cx.preview_source_file(raw).ok()?),
            None => None,
        };
        cx.preview_change_queued(file.as_deref()).then_some(file)
    }

    pub(crate) fn resolve_reload_waiters(cx: &mut Cx) {
        // A callback can queue a rollback. Check readiness again for the next
        // caller, and release the queue borrow before entering widget code.
        loop {
            let next = RELOAD_WAITERS.with_borrow_mut(|waiters| {
                let idx = waiters.iter().position(|w| {
                    !w.live.is_alive() || !cx.preview_change_queued(w.file.as_deref())
                })?;
                Some(waiters.remove(idx))
            });
            let Some(waiter) = next else { break };
            if !waiter.live.is_alive() {
                continue;
            }
            waiter.live.set_stage(CmdStage::Running);
            let _origin = super::remote_input_scope_by(cx, &waiter.by);
            let start = makepad_hal::clock::thread_cpu_ns();
            let result = match cx.tweak_callback {
                Some(callback) => callback(cx, &waiter.op, &waiter.args),
                None => Err("no tweaker (this app has no widgets ui root)".to_string()),
            };
            makepad_remote::charge_served(makepad_hal::clock::thread_cpu_ns().saturating_sub(start));
            let reply = match result {
                Ok(json) => Reply::Text(json),
                Err(msg) => Reply::Err(msg),
            };
            let _ = waiter.tx.send(reply);
        }
    }

    fn trailing_uid(json: &str) -> Option<u64> {
        let body = json.strip_suffix('}')?;
        let (prefix, digits) = body.rsplit_once(r#","uid":"#)?;
        // The widget callback appends uid immediately after the new property map.
        if !prefix.ends_with('}') || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        digits.parse::<u64>().ok().filter(|&uid| uid != 0)
    }

    fn resolve_frame_waiters(cx: &mut Cx) {
        // Resolve anyone who asked to be answered after the next frame.
        let repaint_id = cx.repaint_id;
        let local_user_seq = cx.remote_activity.local_user_seq();
        let mut ready = Vec::new();
        FRAME_WAITERS.with_borrow_mut(|waiters| {
            let mut i = 0;
            while i < waiters.len() {
                if !waiters[i].live.is_alive() {
                    waiters.remove(i);
                    continue;
                }
                if waiters[i].local_user_seq != local_user_seq {
                    let w = waiters.remove(i);
                    let _ = w.tx.send(Reply::Conflict(activity::interrupted(cx)));
                    continue;
                }
                if waiters[i].window.is_some_and(|id| !cx.windows.is_valid(id) || !cx.windows[id].is_created) {
                    let w = waiters.remove(i);
                    let _ = w.tx.send(Reply::Text(format!(r#"{{"ok":1,"f":{repaint_id},"window_closed":true}}"#)));
                    continue;
                }
                if waiters[i].min_live_edit_seq.is_some_and(|seq| cx.live_edit_seq() < seq && cx.has_pending_live_edit_reload()) {
                    // A nested event pump can finish a frame painted before the reload.
                    waiters[i].target_repaint = waiters[i].target_repaint.max(repaint_id + 1);
                    i += 1;
                    continue;
                }
                if repaint_id >= waiters[i].target_repaint {
                    ready.push(waiters.remove(i));
                } else {
                    i += 1;
                }
            }
        });
        for w in ready {
            let mut text = w.payload.unwrap_or_else(|| format!("{{\"ok\":1,\"f\":{repaint_id}}}"));
            if let (Some(callback), Some((uid, window))) = (cx.tweak_callback, w.post_frame_rect) {
                let mut rect_args = Vec::with_capacity(2);
                rect_args.push(("uid".into(), uid.to_string()));
                if let Some(window) = window {
                    rect_args.push(("w".into(), window.to_string()));
                }
                if let Ok(extra) = callback(cx, "design_rect", &rect_args) {
                    if let (Some(head), Some(tail)) = (text.strip_suffix('}'), extra.strip_prefix('{')) {
                        if !head.trim_end().ends_with('{') && tail.ends_with('}') && tail.len() > 1 {
                            text = format!("{head},{tail}");
                        }
                    }
                }
            }
            let _ = w.tx.send(Reply::Text(text));
        }
    }

    /// The window table the command threads read. Every app's endpoint is open, so this runs on
    /// every poll of every app: it compares in place and allocates only when a window changed.
    fn publish_windows(cx: &Cx) {
        let Ok(mut status) = status_cell().try_lock() else { return };
        let created = || cx.windows.id_iter().filter(|id| cx.windows[*id].is_created);
        // A window the app shows counts even when the instance keeps it off screen
        // (MAKEPAD_HIDE_WINDOWS): grabs and captures read it all the same. The headless backend
        // has no pixels to capture.
        let drawn = !matches!(cx.gpu_backend(), crate::cx::GpuBackend::Headless);
        let visible = |id| {
            let w: &crate::window::CxWindow = &cx.windows[id];
            drawn && w.shown && !w.is_popup && w.window_geom.inner_size.x > 0. && w.window_geom.inner_size.y > 0.
        };
        let same = created().count() == status.windows.len()
            && created().zip(status.windows.iter()).all(|(window_id, info)| {
                let window = &cx.windows[window_id];
                let geom = &window.window_geom;
                info.id == window_id.id()
                    && info.visible == visible(window_id)
                    && info.title == window.create_title
                    && info.w == geom.inner_size.x
                    && info.h == geom.inner_size.y
                    && info.dpi == geom.dpi_factor
                    && info.pass_dpi == main_pass_dpi(cx, window_id)
                    && info.x == geom.position.x
                    && info.y == geom.position.y
                    && info.key == window.key
            });
        if same {
            return;
        }
        let windows: Vec<_> = created()
            .map(|window_id| {
                let window = &cx.windows[window_id];
                let geom = &window.window_geom;
                WinInfo {
                    id: window_id.id(),
                    title: window.create_title.clone(),
                    visible: visible(window_id),
                    w: geom.inner_size.x,
                    h: geom.inner_size.y,
                    dpi: geom.dpi_factor,
                    pass_dpi: main_pass_dpi(cx, window_id),
                    x: geom.position.x,
                    y: geom.position.y,
                    key: window.key,
                }
            })
            .collect();
        fn advertised(windows: &[WinInfo]) -> impl Iterator<Item = (usize, &str)> {
            windows.iter().filter(|w| w.visible).map(|w| (w.id, application_title(&w.title)))
        }
        fn main_title(windows: &[WinInfo]) -> Option<&str> {
            windows.first().map(|w| application_title(&w.title)).filter(|s| !s.is_empty())
        }
        let changed = main_title(&windows) != main_title(&status.windows)
            || !advertised(&windows).eq(advertised(&status.windows));
        status.windows = windows;
        if changed {
            let title = main_title(&status.windows).map(str::to_owned);
            let listing = advertised(&status.windows).map(|(id, name)| (id, name.to_owned())).collect::<Vec<_>>();
            drop(status);
            // Hide, rename and close snapshots must reach the registry in event order.
            use crate::thread::{Class, Strand, Ticket};
            thread_local! { static WRITER: std::cell::OnceCell<Strand> = const { std::cell::OnceCell::new() }; }
            WRITER.with(|writer| writer.get_or_init(|| Strand::new(&cx.task_pool(), "window discovery"))
                .submit(Class::Background, Ticket::none(), move |_| {
                    if let Err(e) = makepad_remote::discovery::publish_windows(title.as_deref(), &listing) {
                        crate::log!("Could not publish window titles: {e}");
                    }
                }).detach());
        }
    }

    /// The scale the window's main pass last rasterised at; 0 before it has one.
    fn main_pass_dpi(cx: &Cx, window_id: WindowId) -> f64 {
        cx.windows[window_id].main_pass_id.and_then(|pass| cx.passes[pass].dpi_factor).unwrap_or(0.0)
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
        }
        if !request.live.is_alive() {
            if let Some(tx) = request.cmd.mutation_reply() {
                let _ = tx.send(Reply::Err("cancelled command was not applied".into()));
            }
            return;
        }
        request.live.set_stage(CmdStage::Running);
        #[cfg(target_os = "macos")]
        let global_user_seq = cx.remote_activity.user_seq();
        let local_user_seq = cx.remote_activity.local_user_seq();
        let _origin = super::remote_input_scope_by(cx, &request.by);
        match request.cmd {
            Cmd::Gamepads(tx) => {
                use crate::game_input::CxGameInputApi;
                use crate::makepad_micro_serde::SerJson;
                let infos = cx.game_input_infos();
                let states = cx.game_input_states();
                let devices: Vec<_> = states.iter().enumerate().map(|(index, state)| {
                    let info = infos.get(index);
                    let remote: makepad_studio_protocol::RemoteGameInput = state.into();
                    format!("{{\"id\":{},\"name\":{},\"state\":{}}}",
                        info.map(|i| i.id.0).unwrap_or(index as u64),
                        json_str(info.map(|i| i.name.as_str()).unwrap_or("Controller")), remote.serialize_json())
                }).collect();
                let _ = tx.send(Reply::Text(format!("{{\"gamepads\":[{}]}}", devices.join(","))));
            }
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
                for input in coalesce_moves(inputs) {
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
                                        phase: Default::default(),
                                    }),
                                    window_id,
                                    dvec2(0.0, 0.0),
                                );
                            }
                        }
                        continue;
                    }
                    let msg = match input {
                        Input::NativeClick { x, y } => {
                            #[cfg(target_os = "macos")]
                            {
                                let sid = cx.hal.frames.w.session();
                                let id = crate::event::hal_intake::hal_window_id(window_id);
                                let native = cx.windows[window_id].layout_vec2d_to_native_points(dvec2(x, y));
                                let live = request.live;
                                let input_live = live.clone();
                                let native_by = request.by.clone();
                                makepad_hal::native_click(sid, id, native.x, native.y,
                                    move || input_live.is_alive() && activity::user_seq_now() == global_user_seq,
                                    move |result| match result {
                                        Ok(true) => { let _ = tx.send(Reply::Text("{\"ok\":1,\"native\":1}".into())); }
                                        Ok(false) => {
                                            if !live.is_alive() { return; }
                                            live.set_stage(CmdStage::Queued);
                                            let inputs = [MouseKind::Move, MouseKind::Down, MouseKind::Up].into_iter().map(|kind| Input::Mouse {
                                                kind, x, y, button: 0, dx: 0.0, dy: 0.0, mods: Default::default(), hw: false, time: None,
                                            }).collect();
                                            queue().lock().unwrap().push(QueuedCmd {
                                                cmd: Cmd::Input { window: Some(window_id.id()), inputs, wait, tx },
                                                user_seq: Some(global_user_seq), live, by: native_by,
                                            });
                                            wake();
                                        }
                                        Err(message) => { let _ = tx.send(Reply::Err(message.into())); }
                                    });
                            }
                            #[cfg(not(target_os = "macos"))]
                            { let _ = (x, y); let _ = tx.send(Reply::Err("native=1 requires AppKit".into())); }
                            return;
                        }
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
                                    phase: Default::default(),
                                }),
                            }
                        }
                        Input::PadScroll {
                            x,
                            y,
                            dx,
                            dy,
                            phase,
                            mods,
                            time: at,
                        } => {
                            let native = cx.windows[window_id]
                                .layout_vec2d_to_native_points(dvec2(x, y));
                            StudioToApp::Scroll(RemoteScroll {
                                time: at.unwrap_or(time),
                                x: native.x,
                                y: native.y,
                                sx: dx,
                                sy: dy,
                                is_mouse: false,
                                modifiers: mods,
                                phase,
                            })
                        }
                        Input::Touch {
                            x,
                            y,
                            uid,
                            state,
                            mods,
                            time: at,
                        } => {
                            // Through native points and back, like the mouse routes above.
                            let native = cx.windows[window_id]
                                .layout_vec2d_to_native_points(dvec2(x, y));
                            let abs = cx.stdin_pointer_abs(native, dvec2(0.0, 0.0), window_id);
                            cx.dispatch_remote_touch(
                                window_id,
                                abs,
                                uid,
                                state,
                                mods.into_key_modifiers(),
                                at.unwrap_or(time),
                            );
                            continue;
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
                        Input::Fullscreen => {
                            cx.push_unique_platform_op(crate::cx_api::CxOsOp::FullscreenWindow(window_id));
                            input_result = Some("{\"ok\":1,\"fullscreen\":1}".to_string());
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
                        Input::DropFile { x, y, .. } | Input::DragFile { x, y, .. } if {
                            let size = cx.windows[window_id].window_geom.inner_size;
                            x >= size.x || y >= size.y
                        } => {
                            let _ = tx.send(Reply::Err("file coordinates are outside the window".into()));
                            return;
                        }
                        input @ (Input::DropFile { .. } | Input::DragFile { .. } | Input::DragFileEnd) => {
                            match file_input(cx, window_id, input) {
                                Ok(answer) => input_result = Some(answer),
                                Err(e) => {
                                    let _ = tx.send(Reply::Err(e));
                                    return;
                                }
                            }
                            cx.redraw_all();
                            continue;
                        }
                    };
                    // A copy / cut chord, as a physical key press gives it, before the key itself:
                    // a HAL app asks its focused widget and sets the clipboard (hal_intake); a
                    // hosted app gets the TextCopy / TextCut its host sends for a physical chord,
                    // and its answer goes to the host's clipboard.
                    if let StudioToApp::KeyDown(e) = &msg {
                        let copy = match e.key_code {
                            crate::event::KeyCode::KeyC => Some(true),
                            crate::event::KeyCode::KeyX => Some(false),
                            _ => None,
                        };
                        if let Some(copy) = copy.filter(|_| e.modifiers.control || e.modifiers.logo) {
                            if cx.hal.hosted.is_none() {
                                cx.hal_clipboard_chord(copy);
                            } else {
                                let chord = if copy { StudioToApp::TextCopy } else { StudioToApp::TextCut };
                                cx.dispatch_studio_msg(chord, window_id, dvec2(0.0, 0.0));
                            }
                        }
                    }
                    cx.dispatch_studio_msg(msg, window_id, dvec2(0.0, 0.0));
                }
                if wait {
                    request.live.set_stage(CmdStage::Waiting);
                    FRAME_WAITERS.with_borrow_mut(|waiters| {
                        waiters.push(FrameWaiter {
                            window: Some(window_id),
                            live: request.live,
                            target_repaint: cx.repaint_id + 1,
                            min_live_edit_seq: None,
                            tx,
                            payload: input_result,
                            post_frame_rect: None,
                            local_user_seq,
                        });
                    });
                } else {
                    let _ = tx.send(input_result.map_or(Reply::Ok, Reply::Text));
                }
            }
            Cmd::Grab {
                window,
                series,
                grabs,
                cancelled,
                tx,
            } => {
                // its frame shows every live patch installed before it was asked for, the ones
                // whose wake the loop has not served yet too (a look right after a patch got the
                // frame from before it, 11-04)
                #[cfg(relax)]
                cx.take_in_live_patches(true);
                let window = match resolve_window(cx, window) {
                    Ok(window) => window,
                    Err(err) => {
                        let _ = tx.send(Reply::Err(err));
                        return;
                    }
                };
                CAPTURES.with_borrow_mut(|captures| {
                    // a series' refilled chunk joins its batch, which keeps its last request
                    if series != 0 {
                        if let Some(batch) = captures
                            .batches
                            .iter_mut()
                            .find(|batch| batch.series == series && !batch.cancelled.load(Ordering::Relaxed))
                        {
                            batch.ids.extend(grabs);
                            return;
                        }
                    }
                    captures.batches.push(GrabBatch {
                        series,
                        window,
                        ids: grabs.into(),
                        last_request: None,
                        readback_wait_logged: false,
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
                    live: request.live,
                    window,
                    fps,
                    remaining: frames,
                    open: None,
                    wait_since: None,
                    wait_loads,
                    last_loads: Vec::new(),
                    last_glyph_answers: 0,
                    last_load_progress: Instant::now(),
                    tx,
                }));
            }
            Cmd::Settled(tx) => {
                let _ = tx.send(Reply::Text(settled_json(cx)));
            }
            Cmd::UiCounts(waited, tx) => {
                let (ins, cyc) = crate::thread_counters::read().unwrap_or((0, 0));
                let (cpu, process, served, runtime) = (makepad_hal::clock::thread_cpu_ns(), makepad_hal::clock::process_cpu_ns(), makepad_remote::served_cpu_ns(), makepad_remote::runtime_cpu_ns());
                let answered = UI_ANSWERED_NS.load(std::sync::atomic::Ordering::Relaxed);
                let faults = major_faults();
                // what the app's other threads wanted and a loaded box did not give them: not this
                // thread's (its CPU says its work; its waits are mostly to answer asks such as this
                // one), nor the serving threads', nor the runtime's (a busy JIT is none of the app's)
                // and its threads waiting on the disk now (not the serving ones)
                let waited = match waited {
                    true => format!(
                        ",\"waited_ns\":{},\"io_threads\":{}",
                        makepad_hal::clock::threads_waited_ns(makepad_remote::own_thread).saturating_sub(makepad_remote::runtime_waited_ns()),
                        makepad_hal::clock::threads_in_io_wait(makepad_remote::own_thread)
                    ),
                    false => String::new(),
                };
                let _ = tx.send(Reply::Text(format!("{{\"ins\":{ins},\"cyc\":{cyc},\"cpu_ns\":{cpu},\"answered_ns\":{answered},\"process_cpu_ns\":{process},\"served_cpu_ns\":{served},\"runtime_cpu_ns\":{runtime},\"faults\":{faults}{waited}}}")));
            }
            Cmd::Platform(payload) => match *payload
                .downcast::<PlatformCmd>()
                .expect("remote: a platform command from another platform")
            {
                PlatformCmd::AppQuery { f, words, kv, tx } => {
                    let lines = f(cx, &words, &kv);
                    let _ = tx.send(Reply::Text(lines.join("\n")));
                }
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
            Cmd::ShaderConsts { shader, tx } => {
                let _ = tx.send(Reply::Text(shader_consts_json(cx, shader)));
            }
            Cmd::GpuInventory { n, tx } => {
                let _ = tx.send(Reply::Text(cx.gpu_inventory().report_json_lines(n)));
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
            Cmd::Front(tx) => {
                cx.show_to_person();
                let _ = tx.send(Reply::Text("{\"front\":1}".into()));
            }
            Cmd::Close { window, tx } => {
                let window_id = match resolve_window(cx, window) {
                    Ok(window_id) => window_id,
                    Err(err) => {
                        let _ = tx.send(Reply::Err(err));
                        return;
                    }
                };
                // The window's own close button's path (HalEvent::CloseRequested), never a hard
                // close: the app may keep the window by clearing accept_close, and its unsaved-work
                // handling applies.
                let accept_close = std::rc::Rc::new(std::cell::Cell::new(true));
                cx.call_event_handler(&crate::event::Event::WindowCloseRequested(crate::event::WindowCloseRequestedEvent { window_id, accept_close: accept_close.clone() }));
                let by = cx.injected_by().unwrap_or_default();
                let line = if accept_close.get() {
                    format!("[makepad-remote] remote closed window {}, asked by {by}", window_id.id())
                } else {
                    format!("[makepad-remote] remote close of window {} declined by the app, asked by {by}", window_id.id())
                };
                println!("{line}");
                let _ = std::io::stdout().flush();
                push_log_line(line);
                let _ = tx.send(Reply::Ok);
                if accept_close.get() {
                    cx.push_unique_platform_op(crate::cx_api::CxOsOp::CloseWindow(window_id));
                }
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
                request.live.set_stage(CmdStage::Waiting);
                FRAME_WAITERS.with_borrow_mut(|waiters| waiters.push(FrameWaiter {
                    window: Some(window_id),
                    live: request.live,
                    target_repaint: cx.repaint_id + 1,
                    min_live_edit_seq: None,
                    tx,
                    payload: Some(format!("{{\"resize\":[{},{}]}}", size.x, size.y)),
                    post_frame_rect: None,
                    local_user_seq,
                }));
                cx.redraw_all();
            }
            Cmd::Tweak { op, args, wait, tx } => {
                if op == "design_verify" && !wait {
                    if let Some(file) = verify_pending_reload(cx, &args) {
                        request.live.set_stage(CmdStage::Waiting);
                        RELOAD_WAITERS.with_borrow_mut(|waiters| waiters.push(ReloadWaiter {
                            file, op, args, tx, live: request.live, by: request.by,
                        }));
                        if let Ok(window) = resolve_window(cx, None) {
                            cx.request_remote_window_present(window);
                        }
                        wake();
                        return;
                    }
                }
                let result = match cx.tweak_callback {
                    Some(callback) => callback(cx, &op, &args),
                    None => Err("no tweaker (this app has no widgets ui root)".to_string()),
                };
                match result {
                    Ok(json) => {
                        if wait {
                            let post_frame_rect = matches!(op.as_str(), "design_typed" | "design_apply")
                                .then(|| trailing_uid(&json)).flatten().map(|uid| {
                                    let window = args.iter().find(|(k, _)| k == "w")
                                        .or_else(|| args.iter().find(|(k, _)| k == "window"))
                                        .and_then(|(_, v)| v.parse::<usize>().ok());
                                    (uid, window)
                                });
                            request.live.set_stage(CmdStage::Waiting);
                            FRAME_WAITERS.with_borrow_mut(|waiters| {
                                waiters.push(FrameWaiter {
                                    window: None,
                                    live: request.live,
                                    target_repaint: cx.repaint_id + 1,
                                    min_live_edit_seq: cx.has_pending_live_edit_reload().then(|| cx.live_edit_seq() + 1),
                                    tx,
                                    payload: Some(json),
                                    post_frame_rect,
                                    local_user_seq,
                                });
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
                            request.live.set_stage(CmdStage::Waiting);
                            FRAME_WAITERS.with_borrow_mut(|waiters| {
                                waiters.push(FrameWaiter {
                                    window: None,
                                    live: request.live,
                                    target_repaint: cx.repaint_id + 1,
                                    min_live_edit_seq: None,
                                    tx,
                                    payload: Some(json),
                                    post_frame_rect: None,
                                    local_user_seq,
                                });
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
            Cmd::Quit(tx) | Cmd::QuitForced(tx) => {
                let line = "[makepad-remote] remote quit".to_string();
                println!("{line}");
                let _ = std::io::stdout().flush();
                push_log_line(line);
                // Answer before the loop tears down, so the caller always sees
                // the ack rather than a dropped connection.
                let _ = tx.send(Reply::Ok);
                cx.request_quit(crate::event::QuitReason::App);
            }
            Cmd::QuitNative { tx, .. } => {
                push_log_line("[makepad-remote] remote quit through AppKit (terminate:, where a logout's quit arrives)".to_string());
                // the HAL's op: under the JIT, AppKit's app and its run state are the loader's
                let _ = tx.send(if makepad_hal::ask_native_quit() {
                    Reply::Ok
                } else if cfg!(target_os = "macos") {
                    Reply::Err("native=1 needs AppKit's app, which an offscreen run (MAKEPAD_HIDE_WINDOWS on the default backend) does not start: MAKEPAD_HAL_BACKEND=metal keeps hidden AppKit windows".into())
                } else {
                    Reply::Err("native=1 requires AppKit".into())
                });
            }
        }
    }

    // ------------------------------------------------------------------
    // HTTP
    // ------------------------------------------------------------------

    /// Called only while applying UI commands. These event markers never
    /// cross the HTTP command channel; only the path and coordinates do.
    /// A `drop`, `drag` or `drag_end` (coordinates already inside the window), as a native drag
    /// gives it: a drop with no drag open is Drag, Drop and DragEnd at once; a `drag` holds the
    /// file there (a Drag event per call, the same items throughout), `drag_end` leaves (DragEnd)
    /// and a `drop` of the held file drops it (a Drag first where the point moved).
    fn file_input(cx: &mut Cx, window_id: WindowId, input: Input) -> Result<String, String> {
        let open = FILE_DRAG.with(|d| d.borrow_mut().take());
        if let Some(drag) = open.as_ref().filter(|d| d.window_id != window_id) {
            let message = format!("a drag is open over window {}: drag_end it first", drag.window_id.0);
            FILE_DRAG.with(|d| *d.borrow_mut() = open);
            return Err(message);
        }
        let response_word = |r: crate::event::DragResponse| match r {
            crate::event::DragResponse::None => "none",
            crate::event::DragResponse::Copy => "copy",
            crate::event::DragResponse::Link => "link",
            crate::event::DragResponse::Move => "move",
        };
        match (input, open) {
            (Input::DropFile { path, x, y }, None) => {
                let (handled, response) = inject_file_drop(cx, window_id, path, x, y);
                Ok(format!("{{\"ok\":1,\"drop_handled\":{handled},\"drag_response\":\"{}\"}}", response_word(response)))
            }
            (Input::DragFileEnd, None) => Err("no drag is open: drag path= x= y= opens one".into()),
            (Input::DragFile { path, .. } | Input::DropFile { path, .. }, Some(drag)) if path != drag.path => {
                let message = format!("a drag of {} is open: drop or drag_end it first", drag.path);
                FILE_DRAG.with(|d| *d.borrow_mut() = Some(drag));
                Err(message)
            }
            (Input::DragFile { path, x, y }, open) => {
                let entered = open.is_none();
                let items = open.map_or_else(|| Arc::new(vec![crate::event::DragItem::FilePath { path: path.clone(), internal_id: None }]), |d| d.items);
                let response = file_drag_at(cx, window_id, &items, x, y);
                FILE_DRAG.with(|d| *d.borrow_mut() = Some(FileDrag { window_id, path, items, at: dvec2(x, y) }));
                Ok(format!("{{\"ok\":1,\"drag\":\"{}\",\"drag_response\":\"{}\"}}", if entered { "in" } else { "over" }, response_word(response)))
            }
            (Input::DropFile { x, y, .. }, Some(drag)) => {
                let response = (drag.at != dvec2(x, y)).then(|| file_drag_at(cx, window_id, &drag.items, x, y));
                let handled = file_drop_at(cx, window_id, drag.items, x, y);
                let response = response.map_or(String::new(), |r| format!(",\"drag_response\":\"{}\"", response_word(r)));
                Ok(format!("{{\"ok\":1,\"drop_handled\":{handled}{response}}}"))
            }
            (Input::DragFileEnd, Some(_)) => {
                cx.call_event_handler(&crate::event::Event::DragEnd);
                cx.drag_drop.cycle_drag();
                Ok("{\"ok\":1,\"drag_end\":1}".into())
            }
            (_, open) => {
                FILE_DRAG.with(|d| *d.borrow_mut() = open);
                Err("not a file input".into())
            }
        }
    }

    /// One Drag event of `items` at a point; what the app answered.
    fn file_drag_at(cx: &mut Cx, window_id: WindowId, items: &Arc<Vec<crate::event::DragItem>>, x: f64, y: f64) -> crate::event::DragResponse {
        use crate::event::{DragEvent, DragResponse, Event};
        let response = Arc::new(Mutex::new(DragResponse::None));
        cx.call_event_handler(&Event::Drag(DragEvent {
            window_id,
            modifiers: Default::default(),
            handled: Arc::new(Mutex::new(false)),
            abs: dvec2(x, y),
            items: items.clone(),
            response: response.clone(),
        }));
        cx.drag_drop.cycle_drag();
        let answered = *response.lock().unwrap_or_else(|e| e.into_inner());
        answered
    }

    /// Drop and DragEnd of `items` at a point; whether a target took the drop.
    fn file_drop_at(cx: &mut Cx, window_id: WindowId, items: Arc<Vec<crate::event::DragItem>>, x: f64, y: f64) -> bool {
        use crate::event::{DropEvent, Event};
        let handled = Arc::new(Mutex::new(false));
        cx.call_event_handler(&Event::Drop(DropEvent {
            window_id,
            modifiers: Default::default(),
            handled: handled.clone(),
            abs: dvec2(x, y),
            items,
        }));
        cx.drag_drop.cycle_drag();
        cx.call_event_handler(&Event::DragEnd);
        cx.drag_drop.cycle_drag();
        let taken = *handled.lock().unwrap_or_else(|e| e.into_inner());
        taken
    }

    fn inject_file_drop(
        cx: &mut Cx,
        window_id: WindowId,
        path: String,
        x: f64,
        y: f64,
    ) -> (bool, crate::event::DragResponse) {
        let items = Arc::new(vec![crate::event::DragItem::FilePath {
            path,
            internal_id: None,
        }]);
        let response = file_drag_at(cx, window_id, &items, x, y);
        (file_drop_at(cx, window_id, items, x, y), response)
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
                let loc = cx.with_vm(|vm| vm.ip_to_loc(tc.ip));
                let (file, line, col) = match loc {
                    Some(loc) => (loc.file().to_owned(), loc.line(), loc.col()),
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
    fn route_capture_start(p: &Params) -> Answer {
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
        match ask("cap start", WaitKind::Turn,
            move |tx| {
                Cmd::Platform(Box::new(PlatformCmd::CapStart {
                    window,
                    config,
                    session,
                    tx,
                }))
            },
        ) {
            Reply::Text(text) => {
                if !virtual_clock {
                    capture::install_screen_sink(window, fps);
                }
                Answer::Json(text)
            }
            other => {
                capture::close_session();
                reply_to_answer(other)
            }
        }
    }

    /// `/cap/stop[?hashes=1]`: answered once the file is finalized.
    fn route_capture_stop(p: &Params) -> Answer {
        capture::remove_screen_sink();
        match ask("cap stop", WaitKind::Turn, Cmd::CapStop) {
            Reply::Text(_) => {}
            other => return reply_to_answer(other),
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
                Answer::Json(out)
            }
            Err(msg) => err(&msg),
        }
    }

    /// `/cursor?show=1|0[&style=arrow|hand|text|crosshair|app][&x=&y=][&w=]`.
    fn route_cursor(p: &Params) -> Answer {
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
        reply_to_answer(ask("cursor", WaitKind::Turn,
            move |tx| {
                Cmd::Platform(Box::new(PlatformCmd::Cursor {
                    style,
                    window,
                    pos,
                    tx,
                }))
            },
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
    fn route_midi(p: &Params) -> Answer {
        use crate::midi::{MidiData, MidiPortId};
        use crate::makepad_live_id::LiveId;
        match p.get(&["k", "kind"]).unwrap_or("in") {
            "reset" => {
                crate::midi_inject::reset();
                Answer::Json("{\"ok\":1}".to_string())
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
                Answer::Json(format!("{{\"n\":{},\"ports\":[{}]}}", rows.len(), rows.join(",")))
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
                Answer::Json(format!("{{\"n\":{},\"m\":[{}]}}", rows.len(), rows.join(",")))
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
                Answer::Json(format!(
                        "{{\"ok\":{},\"dropped\":{}}}",
                        queued as u8,
                        crate::midi_inject::dropped()
                    ),
                )
            }
            other => err(&format!("bad kind {other}")),
        }
    }

    fn route_log(p: &Params) -> Answer {
        crate::log::flush_sink();
        Answer::Json(log_json(p))
    }

    fn log_json(p: &Params) -> String {
        let since = p.get(&["since"]).and_then(|v| v.parse::<u64>().ok());
        let count = p
            .get(&["n", "count", "tail"])
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(50);
        // The ring already tails and already carries the newest sequence,
        // so the count is applied by the read rather than by trimming a
        // vector afterwards.
        // `d`: lines after the cursor (0 for a tail) that the ring no longer keeps. A reader
        // that needs every line since its cursor has a gap, and the full log is on stderr.
        let crate::log_ring::LogRead { newest, dropped, lines } = match since {
            Some(since) => crate::log_ring::read_since(since, usize::MAX),
            None => crate::log_ring::read_since(0, count),
        };
        let mut out = format!("{{\"n\":{newest},\"d\":{dropped},\"l\":[");
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&json_str(&line.text));
        }
        out.push_str("]}");
        out
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

        mod fixture {
            use crate::makepad_script::*;
            script_mod! {
                let seed = #(1.0)
                seed + 9
            }
        }

        #[test]
        fn window_close_and_loop_exit_resolve_waiters_immediately() {
            use std::sync::mpsc::{channel, TryRecvError};
            let mut cx = Cx::new(Box::new(|_, _| {}));
            let handle = crate::window::WindowHandle::new(&mut cx);
            let window = handle.window_id();
            let frame = |cx: &mut Cx, live: CommandLive| {
                let (tx, rx) = channel();
                apply(cx, QueuedCmd {
                    cmd: Cmd::Resize { window: Some(window.id()), size: dvec2(240.0, 160.0), tx },
                    live, user_seq: None, by: String::new(),
                });
                rx
            };

            cx.windows[window].is_created = true;
            let live = CommandLive::detached();
            let closed = frame(&mut cx, live.clone());
            assert_eq!(live.stage(), CmdStage::Waiting);
            cx.windows[window].is_created = false;
            resolve_frame_waiters(&mut cx);
            assert!(matches!(closed.try_recv(), Ok(Reply::Text(line)) if line.contains(r#""window_closed":true"#)));

            cx.windows[window].is_created = true;
            let (live, pending) = CommandLive::new();
            let cancelled = frame(&mut cx, live);
            drop(pending);
            resolve_frame_waiters(&mut cx);
            assert!(matches!(cancelled.try_recv(), Err(TryRecvError::Disconnected)));
            assert!(!frame_awaited());

            let waiting_frame = frame(&mut cx, CommandLive::detached());
            let (reload_tx, waiting_reload) = channel();
            RELOAD_WAITERS.with_borrow_mut(|ws| ws.push(ReloadWaiter {
                live: CommandLive::detached(), file: None, op: "design_verify".into(),
                args: Vec::new(), tx: reload_tx, by: String::new(),
            }));
            let (step_tx, waiting_step) = channel();
            STEP.set(Some(StepJob {
                live: CommandLive::detached(), window, fps: 60, remaining: 1, open: None,
                wait_since: None, wait_loads: false, last_loads: Vec::new(), last_glyph_answers: 0,
                last_load_progress: Instant::now(), tx: step_tx,
            }));
            on_loop_exit();
            for rx in [waiting_frame, waiting_reload, waiting_step] {
                assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)), "loop exit releases the reply sender");
            }
            assert!(!frame_awaited() && !has_reload_waiters() && STEP.with_borrow(Option::is_none));
        }

        #[test]
        fn design_typed_attaches_post_frame_rect_after_repaint() {
            use std::sync::mpsc::{channel, TryRecvError};
            const EDIT: &str = r#"{"old":{"width":"Size.Fit"},"new":{"width":"Size.Fixed(96)"},"uid":42}"#;
            const LEGACY: &str = r#"{"old":{"text":"a,\"uid\":99"},"new":{"text":"b,\"uid\":99"}}"#;
            #[derive(Default)]
            struct Fixture {
                edit: &'static str,
                window: Option<usize>,
                drawn: bool,
                rect_calls: usize,
            }
            let mut cx = Cx::new(Box::new(|_, _| {}));
            cx.tweak_callback = Some(|cx, op, args| {
                let fixture = cx.global::<Fixture>();
                match op {
                    "design_typed" => Ok(fixture.edit.into()),
                    "design_rect" => {
                        assert!(!frame_awaited(), "the ready waiter is drained before the callback");
                        let mut expected = vec![("uid".into(), "42".into())];
                        if let Some(w) = fixture.window {
                            expected.push(("w".into(), w.to_string()));
                        }
                        assert_eq!(args, expected);
                        fixture.rect_calls += 1;
                        if !fixture.drawn { return Err("widget has no drawn rect".into()) }
                        Ok(format!(r#"{{"window":{},"rect":[324,0,96,28]}}"#, fixture.window.unwrap_or(0)))
                    }
                    _ => panic!("unexpected callback {op}"),
                }
            });
            for (edit, window, drawn, calls) in [
                (EDIT, None, true, 1), (EDIT, Some(1), true, 1),
                (EDIT, None, false, 1), (LEGACY, None, true, 0),
            ] {
                *cx.global::<Fixture>() = Fixture { edit, window, drawn, rect_calls: 0 };
                let mut args = vec![("path".into(), "/btn".into()), ("prop".into(), "width".into()), ("value".into(), "96".into())];
                if let Some(w) = window { args.push(("window".into(), w.to_string())); }
                let (tx, rx) = channel();
                apply(&mut cx, QueuedCmd {
                    cmd: Cmd::Tweak { op: "design_typed".into(), args, wait: true, tx },
                    user_seq: None, live: CommandLive::detached(), by: String::new(),
                });
                resolve_frame_waiters(&mut cx);
                assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)), "the edit waits for repaint");
                assert_eq!(cx.global::<Fixture>().rect_calls, 0, "no geometry is read before repaint");
                cx.repaint_id += 1;
                resolve_frame_waiters(&mut cx);
                let expected = if drawn && calls == 1 {
                    format!(r#"{},"window":{},"rect":[324,0,96,28]}}"#, edit.strip_suffix('}').unwrap(), window.unwrap_or(0))
                } else { edit.to_owned() };
                let Ok(Reply::Text(reply)) = rx.try_recv() else { panic!("expected the edit reply after repaint") };
                assert_eq!(reply, expected);
                assert_eq!(cx.global::<Fixture>().rect_calls, calls);
            }
            assert!(!frame_awaited());
        }

        #[test]
        fn design_verify_waits_for_live_edit_completion_without_blocking() {
            use std::sync::mpsc::{channel, TryRecvError};

            let mut cx = Cx::new(Box::new(|_, _| {}));
            cx.with_vm(|vm| fixture::script_mod(vm));
            let file = cx.preview_source_file(file!()).expect("the fixture is registered");
            let disk = std::fs::read_to_string(&file).unwrap();
            let edited = disk.replacen("seed + 9", "seed + 8", 1);
            cx.tweak_callback = Some(|cx, op, args| {
                let file = args.iter().find(|(k, _)| k == "file")
                    .map(|(_, value)| cx.preview_source_file(value)).transpose()?;
                if op == "design_preview" {
                    let file = file.expect("preview has a file");
                    let text = std::fs::read_to_string(&file).unwrap();
                    cx.queue_preview_file_change(&file, text);
                    return Ok("preview".into());
                }
                let errors = cx.take_live_edit_errors();
                if !errors.is_empty() {
                    if let Some(file) = file {
                        cx.finish_preview_change(&file, true);
                    }
                }
                Ok(format!("{}:{}", cx.live_edit_seq(), errors.join("\n")))
            });

            let dispatch = |cx: &mut Cx, op: &str, file: Option<&str>, live: CommandLive| {
                let (tx, rx) = channel();
                let args = file.into_iter().map(|f| ("file".into(), f.to_owned())).collect();
                apply(cx, QueuedCmd {
                    cmd: Cmd::Tweak { op: op.into(), args, wait: op == "design_preview", tx },
                    user_seq: None,
                    live,
                    by: "designer".into(),
                });
                rx
            };
            let seq = cx.live_edit_seq();
            cx.queue_preview_file_change(&file, edited.clone());
            let rx = dispatch(&mut cx, "design_verify", Some(&file), CommandLive::detached());
            assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)), "verify parks without blocking dispatch");
            assert!(has_reload_waiters() && cx.has_pending_live_edit_reload(), "the parked verify and reload both need a beat");
            resolve_reload_waiters(&mut cx);
            assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)), "verify waits through a poll before the reload");
            cx.run_live_edit_if_needed("test");
            assert_eq!(cx.live_edit_seq(), seq + 1, "the successful reload advances the counter");
            assert!(matches!(rx.try_recv(), Ok(Reply::Text(s)) if s == format!("{}:", seq + 1)), "completion immediately delivers the verified sequence");
            assert!(!has_reload_waiters(), "the completed verify leaves the queue");

            cx.begin_preview_change(&file);
            cx.queue_preview_file_change(&file, String::new());
            let rejected = dispatch(&mut cx, "design_verify", Some(&file), CommandLive::detached());
            let after_rollback = dispatch(&mut cx, "design_verify", None, CommandLive::detached());
            assert!(matches!(rejected.try_recv(), Err(TryRecvError::Empty)), "a rejected batch must still finish before verify answers");
            cx.run_live_edit_if_needed("test");
            assert_eq!(cx.live_edit_seq(), seq + 2, "a rejected batch advances the counter");
            assert!(matches!(rejected.try_recv(), Ok(Reply::Text(s)) if s.starts_with(&format!("{}:", seq + 2)) && s.contains("runtime has 1, file has 0")), "verify reports the rejection from that pass");
            assert!(matches!(after_rollback.try_recv(), Err(TryRecvError::Empty)), "a second verify stays parked through the first caller's rollback");
            cx.run_live_edit_if_needed("test");
            assert!(matches!(after_rollback.try_recv(), Ok(Reply::Text(s)) if s == format!("{}:", seq + 3)), "fileless verify wakes when the rollback finishes");

            let seq = cx.live_edit_seq();
            cx.queue_preview_file_change(&file, edited.clone());
            let cancelled = dispatch(&mut cx, "design_verify", Some(&file), CommandLive::detached());
            assert!(matches!(cancelled.try_recv(), Err(TryRecvError::Empty)), "verify is parked before cancellation");
            cx.clear_preview_text(&file);
            resolve_reload_waiters(&mut cx);
            assert!(matches!(cancelled.try_recv(), Ok(Reply::Text(s)) if s == format!("{seq}:")), "cancelling a queued preview releases verify without a reload");

            cx.queue_preview_file_change(&file, edited);
            cx.live_edit_errors.push("saved error".into());
            let (live, pending) = CommandLive::new();
            let expired = dispatch(&mut cx, "design_verify", Some(&file), live);
            drop(pending);
            resolve_reload_waiters(&mut cx);
            assert!(matches!(expired.try_recv(), Err(TryRecvError::Disconnected)), "a cancelled waiter drops its reply channel");
            assert_eq!(cx.take_live_edit_errors(), ["saved error"], "a cancelled verify never calls the callback");
            let missing = dispatch(&mut cx, "design_verify", Some("missing.rs"), CommandLive::detached());
            assert!(matches!(missing.try_recv(), Ok(Reply::Err(e)) if e.contains("no registered source file")), "an unknown file is refused immediately even during another preview");
            assert!(!has_reload_waiters(), "an unknown file never joins the waiter queue");
            cx.clear_preview_text(&file);

            let preview = dispatch(&mut cx, "design_preview", Some(&file), CommandLive::detached());
            cx.repaint_id += 1;
            resolve_frame_waiters(&mut cx);
            assert!(matches!(preview.try_recv(), Err(TryRecvError::Empty)), "a pre-reload frame cannot complete preview");
            cx.run_live_edit_if_needed("test");
            resolve_frame_waiters(&mut cx);
            assert!(matches!(preview.try_recv(), Err(TryRecvError::Empty)), "reload completion alone does not paint the preview");
            cx.repaint_id += 1;
            resolve_frame_waiters(&mut cx);
            assert!(matches!(preview.try_recv(), Ok(Reply::Text(s)) if s == "preview"), "the post-reload frame completes preview");
            assert!(!has_reload_waiters() && !frame_awaited(), "both waiter queues are empty after completion");
        }

        #[test]
        fn app_age_formats_seconds_minutes_and_hours() {
            for (millis, expected) in [
                (0, "0 s"), (999, "0 s"), (4_000, "4 s"), (59_000, "59 s"),
                (60_000, "1 min"), (119_000, "1 min"), (3_599_000, "59 min"),
                (3_600_000, "1 h"), (7_200_000, "2 h"),
            ] {
                assert_eq!(format_age(Duration::from_millis(millis)), expected, "{millis} ms");
            }
        }

        #[test]
        fn placeholder_list_and_call_report_app_age_while_allowing_builtins() {
            let age = Duration::from_secs(4);
            let note = "no AI tools of its own open yet (app started 4 s ago); an app may open them once it has started — try again, or see ui/logs";
            let builtins = builtin_instances();
            assert_eq!(builtins.len(), 4);
            assert!(builtins.iter().all(|line| line.starts_with(r#"{"Instance":"#)));
            assert_eq!(builtin_answer("list"), builtins);

            assert_eq!(placeholder_preflight("list", age), Ok(()));
            let lines = placeholder_answer("list", age);
            assert_eq!(lines.len(), 5);
            assert_eq!(&lines[..4], builtins.as_slice());
            assert_eq!(lines[4], format!(r#"{{"Progress":{{"note":"{note}","permille":0}}}}"#));

            assert_eq!(placeholder_preflight("call town.join\n{}", age), Err(note.to_owned()));
            let why = placeholder_preflight("call town.join\n{}", Duration::from_secs(7200)).unwrap_err();
            assert!(why.contains("app started 2 h ago"), "{why}");
            for request in ["call ui.status\n{}", "call logs.log\n{}", "context"] {
                assert_eq!(placeholder_preflight(request, age), Ok(()), "{request}");
            }
            assert_eq!(placeholder_answer("context", age), [r#"{"Context":{"text":""}}"#]);
        }

        #[test]
        fn placeholder_malformed_call_returns_usage() {
            for request in ["call bogus", "call .join", "call town.", "call\n{}", "drop"] {
                assert_eq!(
                    placeholder_preflight(request, Duration::from_secs(4)),
                    Err("app: list, context or call <service>.<tool>".into()),
                    "{request}"
                );
            }
        }

        #[test]
        fn cx_new_seeds_app_age_clock() {
            let _cx = Cx::new(Box::new(|_, _| {}));
            assert!(STARTED.get().is_some());
            assert!(app_age() < Duration::from_secs(600));
        }

        /// The reasons of a `settled` answer, as written.
        fn settled_reasons(json: &str) -> &str {
            let start = json.find("\"reasons\":[").expect("a reasons list") + "\"reasons\":[".len();
            &json[start..start + json[start..].find(']').expect("the list's end")]
        }

        /// `settled` waits for the app pool's jobs while they run, not only while they wait for a
        /// worker: a job on a worker is a result still to come (a plate built for 30 s on the pool
        /// read as settled, 11-16). Once it has ended, the pool is no reason.
        /// Its Cx has a pool of its own: on the process's pool, another test's job still queued
        /// there read as this one's after it had ended (11-24).
        #[test]
        fn settled_waits_for_a_running_pool_job() {
            let mut cx = Cx::new(Box::new(|_, _| {}));
            cx.use_own_task_pool();
            let (started_tx, started) = std::sync::mpsc::channel::<()>();
            let (release, gate) = std::sync::mpsc::channel::<()>();
            let mut job = cx
                .task_pool()
                .submit(crate::thread::Class::Background, move || {
                    started_tx.send(()).unwrap();
                    let _ = gate.recv();
                })
                .unwrap();
            started.recv().unwrap();
            let busy = settled_json(&cx);
            assert!(settled_reasons(&busy).contains("\"pool\""), "a running pool job is no reason: {busy}");
            assert!(busy.contains("\"settled\":false") && busy.contains("\"running\":1"), "a running pool job read as settled: {busy}");
            release.send(()).unwrap();
            // The UI thread never joins: it takes the result once it is there, and the worker
            // gives its slot back just after.
            let ended = std::time::Instant::now();
            let mut result = None;
            while result.is_none() || cx.task_pool_backlog().is_some_and(|p| p.running > 0) {
                assert!(ended.elapsed() < std::time::Duration::from_secs(5), "the ended job still runs");
                if result.is_none() {
                    result = job.try_take();
                }
                std::thread::yield_now();
            }
            assert!(matches!(result, Some(Ok(()))), "the job's result: {result:?}");
            let idle = settled_json(&cx);
            assert!(!settled_reasons(&idle).contains("\"pool\""), "an ended job is still a reason: {idle}");
        }

        /// A file input reaches a drop target as a native drag does: a drop is Drag, Drop and
        /// DragEnd, twice with no stale drag state; a held drag enters once, moves, then leaves
        /// or drops where it is (a move first when the drop's point is another).
        #[test]
        fn file_drop_dispatches_native_sequence_and_clears_drag_area() {
            use crate::area::{Area, RectArea};
            use crate::draw_list::{CxRectArea, DrawList};
            use crate::event::{DragHit, DragItem, DragResponse, Event};
            use crate::makepad_math::Rect;
            use std::{
                cell::{Cell, RefCell},
                rc::Rc,
            };

            const PATH: &str = "/not-read-by-platform/reference.png";
            let area = Rc::new(Cell::new(Area::Empty));
            let target = area.clone();
            let sequence = Rc::new(RefCell::new(Vec::<String>::new()));
            let seen = sequence.clone();
            let mut cx = Cx::new(Box::new(move |cx, event| {
                let hit = event.drag_hits(cx, target.get());
                seen.borrow_mut().push(match hit {
                    DragHit::Drag(hit) => {
                        assert!(
                            matches!(hit.items.as_slice(), [DragItem::FilePath { path, internal_id: None }] if path == PATH)
                        );
                        *hit.response.lock().unwrap_or_else(|e| e.into_inner()) = DragResponse::Copy;
                        format!("{:?}", hit.state)
                    }
                    DragHit::Drop(hit) => format!("Drop {},{}", hit.abs.x, hit.abs.y),
                    DragHit::DragEnd | DragHit::NoHit => {
                        assert!(matches!(event, Event::DragEnd));
                        "DragEnd".into()
                    }
                });
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
            let window = WindowId(0, 0);
            let _origin = crate::remote::remote_input_scope(&cx);
            let run = |cx: &mut Cx, inputs: Vec<Input>| -> Vec<String> {
                sequence.borrow_mut().clear();
                for input in inputs {
                    file_input(cx, window, input).unwrap();
                }
                sequence.borrow().clone()
            };
            let drop = |x, y| Input::DropFile { path: PATH.into(), x, y };
            let drag = |x, y| Input::DragFile { path: PATH.into(), x, y };
            assert_eq!(run(&mut cx, vec![drop(20.0, 30.0), drop(20.0, 30.0)]), ["In", "Drop 20,30", "DragEnd", "In", "Drop 20,30", "DragEnd"], "a drop is a whole drag, its state cleared after");
            assert_eq!(run(&mut cx, vec![drag(20.0, 30.0), drag(40.0, 50.0), Input::DragFileEnd]), ["In", "Over", "DragEnd"], "a held drag enters, moves and leaves");
            assert_eq!(run(&mut cx, vec![drag(20.0, 30.0), drag(40.0, 50.0), drop(40.0, 50.0)]), ["In", "Over", "Drop 40,50", "DragEnd"], "a held drag drops where it is");
            assert_eq!(run(&mut cx, vec![drag(20.0, 30.0), drop(60.0, 70.0)]), ["In", "Over", "Drop 60,70", "DragEnd"], "a drop elsewhere moves there first");
            assert!(file_input(&mut cx, window, Input::DragFileEnd).is_err(), "nothing to leave once dropped");
            file_input(&mut cx, window, drag(20.0, 30.0)).unwrap();
            let other = file_input(&mut cx, window, Input::DropFile { path: "/not-read-by-platform/other.png".into(), x: 20.0, y: 30.0 });
            assert!(other.is_err_and(|e| e.contains("drag_end it first")), "another file waits for the open drag");
            assert!(file_input(&mut cx, WindowId(1, 0), Input::DragFileEnd).is_err(), "the drag is the window's it was opened over");
            assert_eq!(run(&mut cx, vec![Input::DragFileEnd]), ["DragEnd"], "the open drag survives the refusals");
        }

        fn params(pairs: &[(&str, &str)]) -> Params {
            Params(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
        }

        /// What a command answers from its arguments alone, without asking the UI thread.
        fn answered(line: &str, pairs: &[(&str, &str)]) -> String {
            let words: Vec<String> = line.split_whitespace().map(str::to_string).collect();
            match command(&resolve(&words).unwrap(), &params(pairs)) {
                Answer::Json(j) => j,
                Answer::Text(t) => t,
            }
        }

        #[test]
        fn platform_and_app_reject_unknown_parameters() {
            for line in ["tools typo=1", "call service.tool typo=1"] {
                let error = makepad_remote::app::app_request_of(line).unwrap().unwrap_err();
                assert!(error.contains("unknown parameter") && error.contains("known:"), "{line}: {error}");
            }
            for command in ["status", "resize", "cap start", "tweak apply", "design insert", "input replay"] {
                let answer = answered(command, &[("typo", "1")]);
                assert!(answer.contains("unknown parameter") && answer.contains("typo") && answer.contains("known:"), "{command}: {answer}");
            }
            for line in ["status typo=1", r#"{"cmd":"resize","typo":1}"#, "settled typo=1"] {
                let bytes = makepad_remote::cmd::answer(dispatch, line);
                let answer = String::from_utf8(bytes).unwrap();
                assert!(answer.contains("unknown parameter") && answer.contains("known:"), "{line}: {answer}");
            }
            assert!(makepad_remote::params::validate(["path", "splash", "if_user_seq"], makepad_remote::params::tweak("apply").unwrap()).is_ok());
            for op in ["tree", "pick", "props", "apply", "locate", "theme", "states", "pose"] {
                let schema = makepad_remote::params::design(op).unwrap();
                assert!(makepad_remote::params::validate(["w"], schema).is_ok(), "design {op} must accept the linked window");
            }
            let cross = answered("design state", &[("type", "View")]);
            assert!(cross.contains("unknown parameter"), "{cross}");
            let cross = answered("tweak op", &[("op", "state"), ("splash", "{}")]);
            assert!(cross.contains("unknown parameter"), "{cross}");
            let alias = answered("tweak op", &[("op", "design_state"), ("type", "View")]);
            assert!(alias.contains("unknown parameter"), "{alias}");
            let unknown = answered("tweak op", &[("op", "design_bogus")]);
            assert!(unknown.contains("unknown command operation"), "{unknown}");
            register_app_query("parameter_test", "[limit=]", "parameter validation", |_| r#""limit":{"type":"integer"}"#, |_, _, _| panic!("invalid request reached the UI"));
            let bytes = makepad_remote::cmd::answer(dispatch, "parameter_test typo=1");
            let answer = String::from_utf8(bytes).unwrap();
            APP_QUERIES.lock().unwrap().retain(|q| q.0 != "parameter_test");
            assert!(answer.contains("unknown parameter") && answer.contains("known: limit"), "{answer}");
        }

        #[test]
        fn arguments_are_checked_before_the_ui_thread_is_asked() {
            assert!(answered("drop", &[]).contains("drop requires path="));
            assert!(answered("resize", &[("w", "0"), ("h", "10")]).contains("layout points"));
            assert!(answered("key", &[]).contains("need t="));
            assert!(answered("text", &[]).contains("need t="));
            assert!(answered("mouse", &[("k", "bogus")]).contains("bad kind"));
            assert!(answered("mouse", &[("k", "pinch"), ("phase", "sideways")]).contains("bad pinch phase"));
            assert!(answered("shader const", &[]).contains("need shader="));
            assert!(answered("shader const", &[("shader", "1"), ("i", "0")]).contains("need v="));
            assert!(answered("tweak op", &[]).contains("need op="));
            assert!(answered("window", &[("k", "nope")]).contains("k=maximize|fullscreen|resize|dragquery"));
            assert!(answered("cap start", &[]).contains("needs path="));
            assert!(answered("cursor", &[("style", "x")]).contains("cursor style"));
            assert!(answered("midi", &[("k", "bogus")]).contains("bad kind"));
            assert!(answered("status", &[("if_user_seq", "x")]).contains("if_user_seq"));
            assert!(answered("handsoff", &[("on", "0")]).contains("\"handsoff\":0"));
            assert!(answered("trace", &[("off", "1")]).contains("topics"));
        }

        #[test]
        fn quit_respects_held_unless_forced_including_native_flag() {
            use crate::event::{Event, MouseButton, MouseDownEvent};
            let mut cx = Cx::new(Box::new(|_, e| {
                if let Event::QuitRequested(quit) = e { quit.handled.set(true); }
            }));
            let now = Instant::now();
            let down = Event::MouseDown(MouseDownEvent {
                window_id: WindowId(0, 0),
                abs: Default::default(),
                button: MouseButton::PRIMARY,
                modifiers: Default::default(),
                handled: Default::default(),
                time: 0.0,
            });
            let run = |cx: &mut Cx, cmd| apply(cx, QueuedCmd {
                cmd,
                user_seq: None,
                live: CommandLive::detached(),
                by: String::new(),
            });
            cx.remote_activity.note_native_at(&down, false, now - Duration::from_secs(733));
            let (tx, rx) = std::sync::mpsc::channel();
            run(&mut cx, Cmd::Activity(tx.clone()));
            assert!(matches!(rx.try_recv(), Ok(Reply::Text(json)) if json.contains(r#""held":false"#)));
            run(&mut cx, Cmd::Quit(tx));
            assert!(matches!(rx.try_recv(), Ok(Reply::Ok)), "ordinary quit after a stale activity query");

            cx.remote_activity.note_native_at(&down, false, now);
            for native in [false, true] {
                for force in [None, Some("0"), Some("1")] {
                    let mut p = Params(Vec::new());
                    if native {
                        p.0.push(("native".into(), "1".into()));
                    }
                    if let Some(value) = force {
                        p.0.push(("force".into(), value.into()));
                    }
                    let (tx, rx) = std::sync::mpsc::channel();
                    let cmd = quit_command(&p, tx);
                    let forced = force == Some("1");
                    assert_eq!(matches!(cmd, Cmd::QuitNative { .. }), native);
                    assert_eq!(cmd.mutation_reply().is_none(), forced);
                    if native && forced {
                        // Applying this command would ask AppKit to terminate the test process.
                        assert!(matches!(cmd, Cmd::QuitNative { force: true, .. }));
                        continue;
                    }
                    run(&mut cx, cmd);
                    if forced {
                        assert!(matches!(rx.try_recv(), Ok(Reply::Ok)));
                    } else {
                        assert!(matches!(rx.try_recv(), Ok(Reply::Conflict(message))
                            if message.contains(r#""err":"user_interacting""#)));
                    }
                }
            }
        }

        /// `window k=...` reaches the platform as that window's own op, for the window it names.
        #[test]
        fn window_ops_reach_the_platform_for_the_window() {
            use crate::cx_api::CxOsOp;
            let mut cx = Cx::new(Box::new(|_, _| {}));
            let window_id = crate::window::WindowHandle::new(&mut cx).window_id();
            cx.windows[window_id].is_created = true;
            for (input, op, answer) in [
                (Input::Maximize, CxOsOp::MaximizeWindow(window_id), "\"maximize\":1"),
                (Input::Fullscreen, CxOsOp::FullscreenWindow(window_id), "\"fullscreen\":1"),
                (Input::Resize(640.0, 360.0), CxOsOp::ResizeWindow(window_id, dvec2(640.0, 360.0)), "\"resize\":[640,360]"),
            ] {
                let (tx, rx) = std::sync::mpsc::channel();
                cx.platform_ops.clear();
                apply(&mut cx, QueuedCmd {
                    cmd: Cmd::Input { window: Some(window_id.id()), inputs: vec![input], wait: false, tx },
                    user_seq: None,
                    live: CommandLive::detached(),
                    by: String::new(),
                });
                assert!(cx.platform_ops.contains(&op), "{answer}: the op reaches the platform");
                match rx.try_recv() {
                    Ok(Reply::Text(text)) => assert!(text.contains(answer), "{answer}: answered {text}"),
                    _ => panic!("{answer}: no answer"),
                }
            }
        }

        /// A series' refilled chunk joins its batch and keeps its last request: the next frame
        /// waits until the one before it was served, across the chunk boundary.
        #[test]
        fn grab_batch_refill_preserves_last_request_ordering() {
            let mut cx = Cx::new(Box::new(|_, _| {}));
            let window_id = crate::window::WindowHandle::new(&mut cx).window_id();
            cx.windows[window_id].is_created = true;
            let cancelled = Arc::new(AtomicBool::new(false));
            let (id_0, id_1) = (GRAB_ID_BASE + (1 << 41), GRAB_ID_BASE + (1 << 41) + 1);
            let chunk = |cx: &mut Cx, id: u64| {
                let (tx, rx) = std::sync::mpsc::channel();
                apply(cx, QueuedCmd {
                    cmd: Cmd::Grab { window: Some(window_id.id()), series: 7, grabs: vec![(id, Instant::now())], cancelled: cancelled.clone(), tx },
                    user_seq: None,
                    live: CommandLive::detached(),
                    by: String::new(),
                });
                assert!(matches!(rx.try_recv(), Ok(Reply::Ok)), "the chunk is taken");
            };
            let requested = |cx: &Cx, id: u64| cx.screenshot_requests.iter().any(|request| request.request_id == id);
            chunk(&mut cx, id_0);
            poll_captures(&mut cx);
            assert!(requested(&cx, id_0), "the first frame is asked for");
            chunk(&mut cx, id_1);
            poll_captures(&mut cx);
            assert!(!requested(&cx, id_1), "the next chunk's frame waits while the last one is pending");
            cx.screenshot_requests.retain(|request| request.request_id != id_0);
            poll_captures(&mut cx);
            assert!(requested(&cx, id_1), "then it is asked for");
            CAPTURES.with_borrow_mut(|captures| *captures = Captures::default());
        }

        /// `dispatch` is the `cmd` service: it answers help with every command, resolves words
        /// the way a typed line and a JSON request both give them, and refuses what it does not
        /// know.
        #[test]
        fn dispatch_answers_help_and_refuses_unknown_commands() {
            let run = |line: &str| {
                let reply = makepad_remote::cmd::answer(dispatch, line);
                String::from_utf8(reply).unwrap()
            };
            let help = run("help");
            for c in COMMANDS {
                assert!(help.contains(&format!("{{\"cmd\":\"{}\"", c.words)), "help lists {}", c.words);
            }
            assert!(help.contains("\"tool\":\"ui.cap_start\""), "help names each command's tool");
            assert!(help.contains("\"cmd\":\"gpu top\""), "help lists the GPU ledger's commands");
            let unknown = run("frobnicate");
            assert!(unknown.contains("unknown command `frobnicate`"), "{unknown}");
            assert!(unknown.contains(", move, "), "{unknown}");
            assert!(run("grab").contains("client-side"));
            assert!(run("ping").contains("\"pid\""));
            // the words of a command arrive as positional words or inside a JSON `cmd`
            assert!(run("handsoff on=0").contains("\"handsoff\":0"));
            assert!(run("{\"cmd\":\"handsoff\",\"on\":\"1\"}").contains("\"handsoff\":1"));
            assert!(run("design").contains("design <op>"));
            assert!(run("{\"cmd\":\"shader const\",\"shader\":\"2\"}").contains("need i="));
        }

        #[test]
        fn path_queries_match_whole_trailing_segments() {
            use makepad_a11y::{query::Query, Node};
            let node = Node { path: "body.main.new_note".into(), ..Default::default() };
            for q in ["path=main.new_note", "path=new_note", "path=body.main.new_note"] {
                assert!(Query::parse(q).unwrap().matches(&node));
            }
            for q in ["path=note", "path=main", "path=other.new_note"] {
                assert!(!Query::parse(q).unwrap().matches(&node));
            }
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

        /// The remote `log` route drains the native async log sink before reading the ring, so
        /// any `log!()` or `defer_log()` submitted before the query is visible immediately.
        #[test]
        fn remote_log_route_sees_records_logged_before_the_query() {
            crate::Cx::init_log();
            let tag = format!("remote-log-barrier-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
            let query = |line: &str| {
                String::from_utf8(makepad_remote::cmd::answer(dispatch, line)).unwrap()
            };
            let parse_seq = |reply: &str| -> u64 {
                reply
                    .strip_prefix("{\"n\":")
                    .and_then(|s| s.split_once(','))
                    .and_then(|(n, _)| n.parse::<u64>().ok())
                    .unwrap_or_else(|| panic!("unexpected log reply: {reply}"))
            };

            let mut seq = parse_seq(&query("log n=0"));
            for i in 0..32 {
                let needle = format!("{tag}-{i}");
                crate::log!("{needle}");
                let reply = query(&format!("log since={seq}"));
                assert!(
                    reply.contains(&needle),
                    "round {i}: `log since={seq}` missed `{needle}` logged immediately before it: {reply}"
                );
                seq = parse_seq(&reply);
            }

            let deferred = format!("{tag}-deferred");
            let deferred_msg = deferred.clone();
            crate::log::defer_log(file!(), line!(), column!(), move || deferred_msg);
            let reply = query(&format!("log since={seq}"));
            assert!(
                reply.contains(&deferred),
                "`log since={seq}` missed deferred record `{deferred}` logged immediately before it: {reply}"
            );
        }

    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use crate::cx::Cx;

    pub type AppQueryFn = fn(&mut Cx, &[String], &[(String, String)]) -> Vec<String>;
    pub type AppQueryParams = fn(&[String]) -> &'static str;
    /// No remote endpoint on the web: nothing asks.
    pub fn register_app_query(_name: &'static str, _args: &'static str, _answers: &'static str, _params: AppQueryParams, _f: AppQueryFn) {}
    pub fn register_app_command(_name: &'static str, _args: &'static str, _answers: &'static str, _params: AppQueryParams, _f: AppQueryFn) {}

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
    pub(crate) fn on_loop_exit() {}
    pub(crate) fn resolve_reload_waiters(_cx: &mut Cx) {}
    pub(crate) fn finish_capture_on_shutdown(_cx: &mut Cx) {}
    pub(crate) fn grab_targets_window(_request_id: u64, _window_id: Option<usize>) -> bool {
        true
    }
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn grab_ids_only(_ids: &[u64]) -> bool {
        false
    }
    /// There is no remote bridge, so every grab id is non-remote and is returned to the caller.
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

#[cfg(all(relax, any(unix, windows)))]
pub(crate) mod host_registration {
    fn host_symbol(name: *const std::ffi::c_char) -> *mut std::ffi::c_void {
        crate::cx::relax_host_symbol(unsafe { std::ffi::CStr::from_ptr(name) })
    }

    /// Registers the command `name` with the Relax host, answered by the process's dispatch
    /// (`remote::dispatch`).
    #[cfg(relax)]
    pub(crate) fn register_with_host(name: &str) {
        type Query = extern "C" fn(
            *const u8,
            usize,
            extern "C" fn(*const u8, usize, *mut std::ffi::c_void),
            *mut std::ffi::c_void,
        );
        let register = host_symbol(c"relax_register_inspect".as_ptr());
        if register.is_null() {
            return;
        }
        let register: extern "C" fn(*const u8, usize, Query) =
            unsafe { std::mem::transmute(register) };
        register(name.as_ptr(), name.len(), crate::remote::host_query);
    }

    /// Registers the service `name` with the Relax host: `open(args, len, sink, ctx)` starts a
    /// producer for one channel and returns its handle (0: refused, after a sink call with the
    /// reason), `close(handle)` stops it when the client closes the channel. The producer calls
    /// `sink(ctx, kind, ptr, len)`: kind 0 sends a message (false: the client is gone), 1 ends the
    /// stream, 2 ends it with a reason; the end call is always the last and frees `ctx`.
    #[cfg(relax)]
    pub(crate) fn register_service_with_host(
        name: &str,
        version: u32,
        open: makepad_remote::program::ServiceOpen,
        close: makepad_remote::program::ServiceClose,
        data: Option<makepad_remote::program::ServiceData>,
    ) {
        use makepad_remote::program::{ServiceClose, ServiceData, ServiceOpen};
        if let Some(data) = data {
            let register = host_symbol(c"relax_register_remote_service_data".as_ptr());
            if !register.is_null() {
                let register: extern "C" fn(*const u8, usize, u32, ServiceOpen, ServiceClose, ServiceData) =
                    unsafe { std::mem::transmute(register) };
                register(name.as_ptr(), name.len(), version, open, close, data);
                return;
            }
        }
        let register = host_symbol(c"relax_register_remote_service".as_ptr());
        if register.is_null() {
            return;
        }
        let register: extern "C" fn(*const u8, usize, u32, ServiceOpen, ServiceClose) =
            unsafe { std::mem::transmute(register) };
        register(name.as_ptr(), name.len(), version, open, close);
    }
}
