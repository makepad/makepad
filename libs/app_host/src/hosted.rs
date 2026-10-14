//! One hosted makepad child: the protocol state a host keeps with it.
//!
//! Ported out of the window manager's tile widget (studio's
//! DesktopRunView before it). The shared-GPU swapchain, the bootstrap
//! messages, the presented frame, the `Tick`/`TickDone` pacing, the Linux
//! aux channel, input forwarding and the widget-snapshot round trip live
//! here; what a host draws or does with the frame does not. The owner
//! (the WM's `MpRunView`, Stage's app layer) feeds the events it gets from
//! the hub into the methods below, forwards `take_outgoing()` batches to
//! the child's socket after every call, and reads the frame through
//! `presented()` / `texture()`.

use std::collections::HashMap;

use crate::hub::ClientId;
use makepad_platform::makepad_micro_serde::SerBin;
use makepad_platform::shared_framebuf::HostSwapchain;
#[cfg(not(target_os = "linux"))]
use makepad_platform::shared_framebuf::shared_swapchain_from_host_swapchain;
#[cfg(target_os = "linux")]
use makepad_platform::shared_framebuf::{export_host_swapchain, ExportedHostSwapchain, LinuxSwapchainSender};
#[cfg(target_os = "linux")]
use makepad_platform::shared_framebuf::aux_chan::ExternalEndpointListener;
use makepad_platform::*;
use makepad_studio_protocol::{
    AppToStudio, MouseButton, PresentableDraw, RemoteKeyModifiers, RemoteMouseDown, RemoteMouseMove,
    PinchPhase, RemoteMouseUp, RemotePinch, RemoteScroll, ScrollPhase, StudioToApp, StudioToAppVec, WidgetSnapshot, WidgetSnapshotRequest,
    WidgetSnapshotResponse,
};
use makepad_error_log::log;

/// The host's two clocks. A host passes its own (the WM's `host::now` is
/// `Cx::monotonic_now`, which also works on the web, where `Instant` traps).
#[derive(Clone, Copy)]
pub struct Clock {
    /// Seconds since the program started, monotonic.
    pub now: fn() -> f64,
    /// Seconds since the Unix epoch.
    pub wall_now: fn() -> f64,
}

/// Drag-stall hunt: timestamped trace lines appended to the file named by
/// MAKEPAD_WM_TRACE. Free when unset (one static branch). Timestamps are
/// UNIX ms (mod 1e7) so host and child (MAKEPAD_STUDIO_TRACE) lines
/// correlate. `wall_now` is the host's wall clock in seconds.
pub fn trace_host(wall_now: f64, line: &str) {
    use std::io::Write;
    use std::sync::{Mutex, OnceLock};
    static FILE: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
    let file = FILE.get_or_init(|| {
        std::env::var("MAKEPAD_WM_TRACE").ok().and_then(|p| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok()
                .map(Mutex::new)
        })
    });
    let Some(file) = file else { return };
    let ms = wall_now * 1000.0;
    if let Ok(mut f) = file.lock() {
        let _ = writeln!(f, "{:.2} H {}", ms % 1.0e7, line);
    }
}

/// The child window this host presents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunTarget {
    pub client: ClientId,
    pub window_id: usize,
}

/// The newest presented frame of the child: the swapchain image's texture
/// and how a sampler finds the frame in it.
#[derive(Clone)]
pub struct Presented {
    pub texture: Texture,
    /// The frame's share of the (power-of-two or exact) image: width and
    /// height over the allocation.
    pub tex_scale: [f32; 2],
    /// The allocation, in texels.
    pub tex_size: [f32; 2],
    /// 1.0 when the frame carries the child's size in its first two pixels
    /// (the Linux software path), else 0.0.
    pub packed_header: f32,
}

/// Seconds between two Ticks at the least: the frame. A timer may fire a
/// little early, so a gap of nine tenths counts as open.
const TICK_GAP: f64 = 0.9;

/// Tick pacing: the next Tick is not due before a frame has passed since
/// the last one.
fn tick_gap_open(last_sent: Option<f64>, now: f64, period: f64) -> bool {
    last_sent.map_or(true, |at| now - at >= period * TICK_GAP)
}

/// Linux frames are the window's exact size, except during a live resize: then any frames
/// that hold the size are kept (the child draws into their top-left), and new ones are
/// made with room to grow, so a drag replaces them a few times instead of every step.
#[cfg(any(target_os = "linux", test))]
fn swapchain_size_holds(live: bool, need: [u32; 2], alloc: [u32; 2]) -> bool {
    if live {
        need[0] <= alloc[0] && need[1] <= alloc[1]
    } else {
        need == alloc
    }
}

/// The frame size made for `need` (see `swapchain_size_holds`): a quarter more, at least
/// 256 px, rounded up to 64, while resizing live.
#[cfg(any(target_os = "linux", test))]
fn swapchain_alloc(live: bool, need: [u32; 2]) -> [u32; 2] {
    let grow = |n: u32| if live { (n + (n / 4).max(256)).div_ceil(64) * 64 } else { n.max(1) };
    [grow(need[0]), grow(need[1])]
}

/// When [`HostedApp`]'s beat sends next: at the owner's next frame (a Tick), or at a time
/// (the transport poll, the fallback for a Tick never acknowledged).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Beat {
    Frame,
    At(f64),
}

/// How often the bootstrap messages go out again while no frame confirms them.
const BOOTSTRAP_RETRY: f64 = 0.062;

pub struct HostedApp {
    clock: Clock,
    current_target: Option<RunTarget>,
    /// The one-shot timer of the next send ([`HostedApp::ensure_beat`]);
    /// empty while nothing is due.
    tick_timer: Timer,
    /// When `tick_timer` fires.
    tick_timer_due: f64,
    /// The owner's next frame, asked for a Tick ([`Beat::Frame`]); `tick_frame_asked` while
    /// it is pending.
    tick_frame: NextFrame,
    tick_frame_asked: bool,
    /// When the bootstrap messages last went out.
    last_bootstrap_at: Option<f64>,
    /// The tick's period: the unit of the Tick pacing below.
    tick_period: f64,
    /// When the `Tick` the child has not acknowledged (`TickDone`) yet
    /// went out. The timer sends the next Tick only once this clears, so
    /// a slow child never has more than one frame's ticks and pointer
    /// moves queued behind it; a child that never acknowledges (an older
    /// binary, a GPU handoff pause) is pumped again after `tick_fallback`.
    tick_outstanding: Option<f64>,
    /// When the last Tick went out: no two Ticks are closer than one frame
    /// ([`tick_gap_open`]), whether the beat or an acknowledgement sends.
    last_tick_at: Option<f64>,
    /// The child has something to do: input is queued for it, it asked for
    /// a frame (`RequestAnimationFrame`) or its timer fell due (`WakeIn`).
    /// The beat ticks the child only while this holds; an idle child is not
    /// ticked, and the beat itself stops (`ensure_beat` starts it again).
    tick_wanted: bool,
    /// The owner holds the child at 0 fps ([`HostedApp::set_throttled`]).
    throttled: bool,
    /// This window paces the child: it sends the `Tick`s. A child with
    /// several hosted windows has one pacing window; the others only carry
    /// their own geometry, swapchain, frames and input
    /// ([`HostedApp::set_paces_ticks`]).
    paces_ticks: bool,
    /// The child shows this window's content some other way (a frame
    /// ring): no swapchain is shared for it and no frame is waited for;
    /// geometry and input are all it is sent.
    frames_external: bool,
    /// Where this window sits in the child's window space. A child's
    /// windows are hit-tested by position, so the hosted windows of one
    /// child are told apart by origin; input is sent at `origin + local`.
    window_origin: Vec2d,
    /// Fires when the child's earliest timer is due (`WakeIn`).
    wake_timer: Timer,
    /// The child announced it decodes `StudioToApp::MouseCancel`
    /// (`HostedPointerCaps`); until it does, a cancelled press is relayed the
    /// way an older child can take it.
    child_mouse_cancel: bool,
    /// The one log line for a child that cannot take a cancellation.
    logged_no_mouse_cancel: bool,
    /// Ticks sent to the child and `TickDone`s back. The child reads its
    /// messages in order and draws only inside a Tick, so once every Tick
    /// sent before some message is acknowledged, each later frame was drawn
    /// after that message (the phone's tile faces fence on this).
    ticks_sent: u64,
    ticks_done: u64,
    /// The pointer's latest position since the last Tick went out: the
    /// child sees at most one MouseMove per frame, flushed ahead of the
    /// Tick or of any Down/Up/Scroll so their order holds.
    pending_move: Option<RemoteMouseMove>,
    last_rect: Rect,
    last_dpi_factor: f64,
    bootstrap_pending: bool,
    bootstrap_tick_count: u32,
    swapchain: Option<HostSwapchain>,
    last_swapchain_with_completed_draws: Option<HostSwapchain>,
    pending_draw: Option<PresentableDraw>,
    present_ok_count: usize,
    app_ready_for_swapchain: bool,
    /// When the FIRST frame landed (the owner's arrival fade reads it).
    first_present_at: Option<f64>,
    presented: Option<Presented>,
    /// Bumped whenever `presented` changes (a new frame, or none).
    present_serial: u64,
    status: Option<crate::AppStatus>,
    /// While the rect is being ANIMATED, the layout's settled target size.
    /// The owner draws at the animated rect; the swapchain and the
    /// child's WindowGeomChange always use this, so a tween never causes
    /// swapchain churn or per-frame child relayouts.
    target_size: Option<Vec2d>,
    /// The owner is resizing interactively (a drag on an edge): the child gets each new
    /// size, at most one per frame, and keeps drawing into frames that hold it -- a
    /// swapchain per step would leave every step waiting for its images, and nothing
    /// new would show until the drag stopped. When it ends the frames go back to the
    /// window's exact size (`set_live_resize`).
    live_resize: bool,
    /// Batches for the child, serialized, oldest first.
    outgoing: Vec<(ClientId, Vec<u8>)>,
    next_snapshot_id: u64,
    snapshots_pending: Vec<u64>,
    snapshots_done: HashMap<u64, Vec<WidgetSnapshot>>,

    /// The child's GPU descriptor channel: one per child process, shared by the views of its
    /// windows (its first window's view makes it; a later window's view borrows it).
    #[cfg(target_os = "linux")]
    outbox: Option<SharedOutbox>,
    #[cfg(target_os = "linux")]
    aux_chan_listener: Option<ExternalEndpointListener>,
    /// A view of one of the child's later windows: the client whose channel it borrows.
    #[cfg(target_os = "linux")]
    borrows_outbox_of: Option<ClientId>,
    /// In-flight descriptor batch tag.
    #[cfg(target_os = "linux")]
    inflight: Option<u64>,
    /// `try_send` Full: the same batch, retried by ownership.
    #[cfg(target_os = "linux")]
    pending: Option<(u64, ExportedHostSwapchain)>,
    #[cfg(target_os = "linux")]
    transport_error: Option<String>,
    #[cfg(target_os = "linux")]
    outbox_full_logged: bool,
}

impl HostedApp {
    pub fn new(clock: Clock) -> HostedApp {
        HostedApp {
            clock,
            current_target: None,
            tick_timer: Timer::empty(),
            tick_timer_due: 0.0,
            tick_frame: NextFrame::default(),
            tick_frame_asked: false,
            last_bootstrap_at: None,
            tick_period: 0.008,
            tick_outstanding: None,
            last_tick_at: None,
            tick_wanted: true,
            throttled: false,
            paces_ticks: true,
            frames_external: false,
            window_origin: Vec2d::default(),
            wake_timer: Timer::empty(),
            child_mouse_cancel: false,
            logged_no_mouse_cancel: false,
            ticks_sent: 0,
            ticks_done: 0,
            pending_move: None,
            last_rect: Rect::default(),
            last_dpi_factor: 0.0,
            bootstrap_pending: false,
            bootstrap_tick_count: 0,
            swapchain: None,
            last_swapchain_with_completed_draws: None,
            pending_draw: None,
            present_ok_count: 0,
            app_ready_for_swapchain: false,
            first_present_at: None,
            presented: None,
            present_serial: 0,
            status: None,
            target_size: None,
            live_resize: false,
            outgoing: Vec::new(),
            next_snapshot_id: 1,
            snapshots_pending: Vec::new(),
            snapshots_done: HashMap::new(),
            #[cfg(target_os = "linux")]
            outbox: None,
            #[cfg(target_os = "linux")]
            aux_chan_listener: None,
            #[cfg(target_os = "linux")]
            borrows_outbox_of: None,
            #[cfg(target_os = "linux")]
            inflight: None,
            #[cfg(target_os = "linux")]
            pending: None,
            #[cfg(target_os = "linux")]
            transport_error: None,
            #[cfg(target_os = "linux")]
            outbox_full_logged: false,
        }
    }

    /// Start the tick beat. Called once, when the owner has a `Cx`.
    pub fn start(&mut self, cx: &mut Cx) {
        // 240 Hz direct sessions must not be throttled by the old 125 Hz poll.
        self.tick_period = if matches!(cx.os_type(), OsType::LinuxDirect) { 1.0 / 240.0 } else { 0.008 };
        self.tick_wanted = true;
        self.ensure_beat(cx);
    }

    fn now(&self) -> f64 {
        (self.clock.now)()
    }

    fn trace(&self, line: &str) {
        trace_host((self.clock.wall_now)(), line);
    }

    pub fn client(&self) -> Option<ClientId> {
        self.current_target.map(|t| t.client)
    }

    pub fn target(&self) -> Option<RunTarget> {
        self.current_target
    }

    // ---- what goes out to the child ----------------------------------

    /// The batches queued for the child since the last call, serialized
    /// as `StudioToAppVec` frames with the client they are for. The owner
    /// writes each to that client's socket after every call into this type.
    pub fn take_outgoing(&mut self) -> Vec<(ClientId, Vec<u8>)> {
        std::mem::take(&mut self.outgoing)
    }

    /// Queue messages for the child (input, anything else of the protocol).
    pub fn send(&mut self, msgs: Vec<StudioToApp>) {
        if let Some(target) = self.current_target {
            self.emit(target.client, msgs);
        }
    }

    fn emit(&mut self, client: ClientId, msgs: Vec<StudioToApp>) {
        if msgs.is_empty() {
            return;
        }
        self.tick_wanted = true;
        self.outgoing.push((client, StudioToAppVec(msgs).serialize_bin()));
    }

    /// Whether the beat has anything to drive: a Tick the child asked for
    /// or input waits for, one still unanswered, the startup handshake, or
    /// the swapchain transport.
    fn beat_needed(&self) -> bool {
        if self.at_rest() {
            return false;
        }
        if !self.paces_ticks {
            // The pacing window ticks the child; this one only owes it its
            // bootstrap and the pointer it holds.
            return self.pending_move.is_some()
                || (self.app_ready_for_swapchain && !self.presenting())
                || self.bootstrap_pending
                || self.transport_busy();
        }
        self.tick_wanted
            || self.tick_outstanding.is_some()
            || self.pending_move.is_some()
            || !self.presenting()
            || self.bootstrap_pending
            || self.transport_busy()
    }

    /// Whether this window sends the child's `Tick`s (true by default).
    /// The owner of a child with several hosted windows lets one of them
    /// pace and forwards the others' wishes to it ([`HostedApp::wants_tick`]).
    pub fn set_paces_ticks(&mut self, cx: &mut Cx, on: bool) {
        if self.paces_ticks == on {
            return;
        }
        self.paces_ticks = on;
        self.tick_outstanding = None;
        self.tick_wanted = true;
        self.ensure_beat(cx);
    }

    /// Input or a request is waiting for the child's next frame. A window
    /// that does not pace reports it here for the pacing one.
    pub fn wants_tick(&self) -> bool {
        self.tick_wanted || self.pending_move.is_some()
    }

    /// Place this window at `origin` in the child's window space (see
    /// `window_origin`). Takes effect with the next geometry message.
    pub fn set_window_origin(&mut self, origin: Vec2d) {
        if self.window_origin != origin {
            self.window_origin = origin;
            self.bootstrap_pending = true;
            self.bootstrap_tick_count = 0;
        }
    }

    /// `local` (this window's points) in the child's window space, where its pointer
    /// messages land: a later window of a multi-window child sits at its own origin.
    pub fn child_point(&self, local: Vec2d) -> Vec2d {
        self.at_origin(local)
    }

    fn at_origin(&self, local: Vec2d) -> Vec2d {
        local + self.window_origin
    }

    #[cfg(target_os = "linux")]
    fn transport_busy(&self) -> bool {
        self.aux_chan_listener.is_some()
            || self.inflight.is_some()
            || self.pending.is_some()
    }

    #[cfg(not(target_os = "linux"))]
    fn transport_busy(&self) -> bool {
        false
    }

    /// How often a busy transport is polled: every frame during a handoff,
    /// every other frame while only waiting for the child to connect.
    fn transport_poll(&self) -> f64 {
        #[cfg(target_os = "linux")]
        if self.inflight.is_none() && self.pending.is_none() && self.aux_chan_listener.is_some() {
            return self.tick_period * 2.0;
        }
        self.tick_period
    }

    /// Hold the child at 0 fps (true) or let it run (false). A throttled
    /// child is still driven until its first frame is up (so one launched
    /// early is warm) and then gets no Tick and no beat. Letting it run
    /// wakes it at once.
    pub fn set_throttled(&mut self, cx: &mut Cx, on: bool) {
        if self.throttled == on {
            return;
        }
        self.throttled = on;
        if !on {
            self.tick_wanted = true;
            self.ensure_beat(cx);
        }
    }

    pub fn throttled(&self) -> bool {
        self.throttled
    }

    /// Throttled with a frame up and the handshake done: nothing is due.
    fn at_rest(&self) -> bool {
        self.throttled && self.presenting() && !self.bootstrap_pending && self.tick_outstanding.is_none() && !self.transport_busy()
    }

    /// Start the beat when there is work for it and it is not running. The
    /// owner calls this after every call into this type that has a `Cx`.
    pub fn ensure_beat(&mut self, cx: &mut Cx) {
        let want = if self.current_target.is_some() && self.beat_needed() { self.next_beat() } else { None };
        match want {
            None => {
                // a frame already asked for lapses: its event finds nothing due
                if self.tick_timer.0 != 0 {
                    cx.stop_timer(self.tick_timer);
                    self.tick_timer = Timer::empty();
                }
            }
            Some(Beat::Frame) => {
                if self.tick_timer.0 != 0 {
                    cx.stop_timer(self.tick_timer);
                    self.tick_timer = Timer::empty();
                }
                if !self.tick_frame_asked {
                    self.tick_frame = cx.new_next_frame();
                    self.tick_frame_asked = true;
                }
            }
            Some(Beat::At(at)) => {
                // Armed for a sooner moment already: it fires, finds nothing
                // due and arms again for this one.
                if self.tick_timer.0 != 0 && self.tick_timer_due <= at + 0.0005 {
                    return;
                }
                if self.tick_timer.0 != 0 {
                    cx.stop_timer(self.tick_timer);
                }
                self.tick_timer_due = at;
                self.tick_timer = cx.start_timeout((at - self.now()).max(0.0));
            }
        }
    }

    /// When the host next has something to send the child. There is no fast
    /// poll: the child is ticked at the owner's frames while it asks, and not
    /// at all while it does not. A Tick goes out at the owner's next frame (a
    /// child frame per frame the owner shows, never more: a child ticked by
    /// its own clock drew 2-4 frames per shown one); a Tick in flight is
    /// waited for (the fallback is the only timer then).
    fn next_beat(&self) -> Option<Beat> {
        let now = self.now();
        if self.transport_busy() || !self.paces_ticks {
            // The descriptor handoff is polled, and a window that does not
            // pace owes only its bootstrap and the pointer it holds. A
            // listener nobody has connected to yet (the child is still
            // building) is polled at half the rate.
            return Some(Beat::At(now + self.transport_poll()));
        }
        if let Some(sent_at) = self.tick_outstanding {
            return Some(Beat::At(sent_at + self.tick_fallback()));
        }
        Some(Beat::Frame)
    }

    /// The child asked for a frame (`RequestAnimationFrame`).
    pub fn request_tick(&mut self, cx: &mut Cx) {
        self.tick_wanted = true;
        self.ensure_beat(cx);
    }

    /// The child is idle until `secs` from now (`WakeIn`).
    pub fn wake_in(&mut self, cx: &mut Cx, secs: f64) {
        if self.current_target.is_none() {
            return;
        }
        if self.wake_timer.0 != 0 {
            cx.stop_timer(self.wake_timer);
        }
        self.wake_timer = cx.start_timeout(secs.max(0.0));
    }

    /// The pointer samples owed to the child, oldest first.
    fn take_moves(&mut self, msgs: &mut Vec<StudioToApp>) {
        if let Some(mv) = self.pending_move.take() {
            msgs.push(StudioToApp::MouseMove(mv));
        }
    }

    /// A press, release or scroll: the pointer position it happened at
    /// goes out first, in the same batch, so the child sees the move
    /// before the edge exactly as the host did.
    fn emit_after_pending_move(&mut self, client: ClientId, msg: StudioToApp) {
        let mut msgs = Vec::with_capacity(2);
        self.take_moves(&mut msgs);
        msgs.push(msg);
        self.emit(client, msgs);
    }

    // ---- input, in the child's points --------------------------------

    pub fn mouse_down(&mut self, local: Vec2d, button: MouseButton, time: f64, modifiers: &KeyModifiers) {
        let Some(target) = self.current_target else { return };
        self.emit_after_pending_move(
            target.client,
            StudioToApp::MouseDown(RemoteMouseDown {
                button_raw_bits: button.bits(),
                x: local.x + self.window_origin.x,
                y: local.y + self.window_origin.y,
                time,
                modifiers: RemoteKeyModifiers::from_key_modifiers(modifiers),
            }),
        );
    }

    /// The pointer moved while pressed. Held until the next Tick (or the
    /// next edge): one position per frame is all a frame can show.
    pub fn mouse_move(&mut self, local: Vec2d, time: f64, modifiers: &KeyModifiers) {
        if self.current_target.is_none() {
            return;
        }
        self.trace("mm");
        self.tick_wanted = true;
        let local = self.at_origin(local);
        self.pending_move = Some(RemoteMouseMove {
            x: local.x,
            y: local.y,
            time,
            modifiers: RemoteKeyModifiers::from_key_modifiers(modifiers),
        });
    }

    /// The pointer hovers: remembered, sent with the next Tick.
    pub fn hover_move(&mut self, local: Vec2d, time: f64, modifiers: &KeyModifiers) {
        self.tick_wanted = true;
        let local = self.at_origin(local);
        self.pending_move = Some(RemoteMouseMove {
            x: local.x,
            y: local.y,
            time,
            modifiers: RemoteKeyModifiers::from_key_modifiers(modifiers),
        });
    }

    /// A release. `cancelled`: a press the host took away is relayed as a
    /// cancellation, so the app lets go of it without clicking or flinging.
    /// A child that never announced `MouseCancel` is sent nothing -- the
    /// host never cancelled its presses before, and any release could
    /// commit what the press started. A cancel is owed even when the
    /// pointer has no position in the child any more (`local` None); where
    /// it lands does not matter, it clicks nothing.
    pub fn mouse_up(&mut self, local: Option<Vec2d>, button: MouseButton, time: f64, modifiers: &KeyModifiers, cancelled: bool) {
        let Some(target) = self.current_target else { return };
        if cancelled && !self.child_mouse_cancel {
            if !std::mem::replace(&mut self.logged_no_mouse_cancel, true) {
                log!("app host: client {} does not take MouseCancel; its cancelled presses are not relayed", target.client);
            }
            return;
        }
        let local = local.or_else(|| cancelled.then(Default::default)).map(|local| self.at_origin(local));
        let release = if cancelled && self.child_mouse_cancel { StudioToApp::MouseCancel } else { StudioToApp::MouseUp };
        if let Some(local) = local {
            // A cancel travels in a batch of its own.
            if cancelled {
                if let Some(mv) = self.pending_move.take() {
                    self.emit(target.client, vec![StudioToApp::MouseMove(mv)]);
                }
            }
            self.emit_after_pending_move(
                target.client,
                release(RemoteMouseUp {
                    button_raw_bits: button.bits(),
                    x: local.x,
                    y: local.y,
                    time,
                    modifiers: RemoteKeyModifiers::from_key_modifiers(modifiers),
                }),
            );
        }
    }

    /// `phase` is the host's trackpad gesture phase (`ScrollPhase::None` for a
    /// wheel): it travels with the delta so the app's scrollers follow the
    /// host's momentum stream and rubber-band exactly like local ones.
    pub fn scroll(&mut self, local: Vec2d, delta: Vec2d, is_mouse: bool, phase: ScrollPhase, time: f64, modifiers: &KeyModifiers) {
        let Some(target) = self.current_target else { return };
        let local = self.at_origin(local);
        self.emit_after_pending_move(
            target.client,
            StudioToApp::Scroll(RemoteScroll {
                is_mouse,
                time,
                x: local.x,
                y: local.y,
                sx: delta.x,
                sy: delta.y,
                modifiers: RemoteKeyModifiers::from_key_modifiers(modifiers),
                phase,
            }),
        );
    }

    /// A trackpad pinch at `local`; `scale` is relative to the previous event.
    pub fn pinch(&mut self, local: Vec2d, scale: f64, phase: PinchPhase, time: f64, modifiers: &KeyModifiers) {
        let Some(target) = self.current_target else { return };
        let local = self.at_origin(local);
        self.emit_after_pending_move(
            target.client,
            StudioToApp::Pinch(RemotePinch {
                time,
                x: local.x,
                y: local.y,
                scale,
                phase,
                modifiers: RemoteKeyModifiers::from_key_modifiers(modifiers),
            }),
        );
    }

    pub fn key_down(&mut self, e: KeyEvent) {
        self.send(vec![StudioToApp::KeyDown(e)]);
    }

    pub fn key_up(&mut self, e: KeyEvent) {
        self.send(vec![StudioToApp::KeyUp(e)]);
    }

    pub fn text_input(&mut self, e: TextInputEvent) {
        self.send(vec![StudioToApp::TextInput(e)]);
    }

    /// Files dropped on the child's window at `local` (points in its window).
    pub fn drop_files(&mut self, local: Vec2d, paths: Vec<String>, modifiers: &KeyModifiers) {
        let local = self.at_origin(local);
        self.send(vec![StudioToApp::DropFiles(makepad_studio_protocol::RemoteDrop {
            x: local.x,
            y: local.y,
            paths,
            modifiers: RemoteKeyModifiers::from_key_modifiers(modifiers),
        })]);
    }

    pub fn text_copy(&mut self) {
        self.send(vec![StudioToApp::TextCopy]);
    }

    pub fn text_cut(&mut self) {
        self.send(vec![StudioToApp::TextCut]);
    }

    // ---- what the child said ------------------------------------------

    /// What the child's pointer input understands.
    pub fn set_pointer_caps(&mut self, caps: makepad_platform::ime::HostedPointerCaps) {
        self.child_mouse_cancel = caps.mouse_cancel;
    }

    /// Whether the child takes `MouseCancel` (its later windows' views share it).
    pub fn takes_mouse_cancel(&self) -> bool {
        self.child_mouse_cancel
    }

    /// Route one `AppToStudio` message of this child to the state it
    /// belongs to. True when it was one of the protocol's own: a presented
    /// frame, `TickDone`, `RequestAnimationFrame`, the pacing and pointer
    /// capability announcements, a widget snapshot. Everything else (window
    /// creation, cursor, clipboard, relays) is the owner's.
    pub fn on_app_message(&mut self, cx: &mut Cx, msg: &AppToStudio) -> bool {
        match msg {
            AppToStudio::DrawCompleteAndFlip(pd) => {
                self.set_presentable_draw(cx, *pd);
            }
            AppToStudio::TickDone => {
                self.tick_done();
                self.ensure_beat(cx);
            }
            AppToStudio::RequestAnimationFrame => self.request_tick(cx),
            AppToStudio::WakeIn(secs) => self.wake_in(cx, *secs),
            AppToStudio::WidgetSnapshot(response) => self.on_snapshot_response(response.clone()),
            AppToStudio::Custom(json) => {
                if let Some(caps) = makepad_platform::ime::HostedPointerCaps::parse(json) {
                    self.set_pointer_caps(caps);
                } else if let Some(status) = crate::AppStatus::parse(json) {
                    self.status = Some(status);
                } else {
                    return false;
                }
            }
            _ => return false,
        }
        true
    }

    pub fn status(&self) -> Option<&crate::AppStatus> {
        self.status.as_ref()
    }

    // ---- widget snapshots ---------------------------------------------

    /// Ask the child for its widget tree. The answer is collected by
    /// `on_app_message`; poll `take_snapshot` with the id returned here.
    pub fn snapshot(&mut self) -> u64 {
        let request_id = self.next_snapshot_id;
        self.next_snapshot_id += 1;
        self.snapshots_pending.push(request_id);
        self.send(vec![StudioToApp::WidgetSnapshot(WidgetSnapshotRequest { request_id })]);
        request_id
    }

    fn on_snapshot_response(&mut self, response: WidgetSnapshotResponse) {
        if let Some(at) = self.snapshots_pending.iter().position(|id| *id == response.request_id) {
            self.snapshots_pending.remove(at);
            self.snapshots_done.insert(response.request_id, response.widgets);
        }
    }

    /// The answer to `snapshot`, once it arrived (taken: asked for once).
    pub fn take_snapshot(&mut self, request_id: u64) -> Option<Vec<WidgetSnapshot>> {
        self.snapshots_done.remove(&request_id)
    }

    // ---- tick pacing ---------------------------------------------------

    /// How long a Tick may stay unacknowledged before the child is pumped
    /// anyway: four tick periods, never under 50 ms, so a child that does
    /// not speak `TickDone` still runs at 20 Hz.
    fn tick_fallback(&self) -> f64 {
        (self.tick_period * 4.0).max(0.050)
    }

    /// The owner painted the presented frame.
    pub fn painted(&mut self) {
        if self.present_ok_count > 0 {
            self.trace(&format!("paint c{}", self.client().unwrap_or(0)));
        }
    }

    fn append_tick(&mut self, target: RunTarget, msgs: &mut Vec<StudioToApp>) {
        self.trace(&format!("tick c{}", target.client));
        self.take_moves(msgs);
        msgs.push(StudioToApp::Tick);
        if makepad_error_log::trace_enabled("pace") {
            log!("pace wm tick_tx={:.2} c{}", (self.clock.wall_now)() * 1000.0 % 1.0e6, target.client);
        }
        self.ticks_sent += 1;
        let now = self.now();
        self.tick_outstanding = Some(now);
        self.last_tick_at = Some(now);
    }

    /// A Tick may go out: at least one frame since the last one.
    fn tick_gap_ok(&self) -> bool {
        tick_gap_open(self.last_tick_at, self.now(), self.tick_period)
    }

    /// (Ticks sent, TickDones received) -- see `ticks_sent`.
    pub fn tick_counts(&self) -> (u64, u64) {
        (self.ticks_sent, self.ticks_done)
    }

    /// Return the tick credit: the child drew and answered. The next Tick,
    /// if one is wanted, is sent by the beat a frame after the last.
    pub fn tick_done(&mut self) {
        // The child coalesces queued Ticks into one TickDone: it answers the
        // newest Tick sent.
        self.ticks_done = self.ticks_sent;
        if let Some(target) = self.current_target {
            self.trace(&format!("ack c{}", target.client));
        }
        self.tick_outstanding = None;
    }

    /// The owner's event hook: the tick beat (a timer). Sends the
    /// bootstrap messages and the Tick when one is due.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::GpuReset = event {
            self.gpu_reset(cx);
        }
        let target = self.current_target;
        let mut beat = false;
        if let Event::Timer(timer_event) = event {
            if self.wake_timer.0 != 0 && self.wake_timer.is_timer(timer_event).is_some() {
                self.wake_timer = Timer::empty();
                self.tick_wanted = true;
                self.ensure_beat(cx);
            }
            if self.tick_timer.0 != 0 && self.tick_timer.is_timer(timer_event).is_some() {
                // One-shot: spent. `ensure_beat` arms the next one when
                // there is something to send; a view without a client, or
                // an idle child, leaves it off until `set_target` or a
                // request finds some.
                self.tick_timer = Timer::empty();
                beat = target.is_some() && self.beat_needed();
            }
        }
        let mut framed = false;
        if self.tick_frame_asked && self.tick_frame.is_event(event).is_some() {
            self.tick_frame_asked = false;
            framed = target.is_some() && self.beat_needed();
        }
        if beat || framed {
            if let Some(target) = target {
                self.send_beat(cx, target, framed);
            }
        }
        self.ensure_beat(cx);
    }

    /// The beat is due: what the child is owed goes out, and a Tick when none
    /// is in flight -- at the owner's frame (`framed`), or from a timer (the
    /// transport poll) when a frame has passed since the last one.
    fn send_beat(&mut self, cx: &mut Cx, target: RunTarget, framed: bool) {
        let mut msgs = Vec::new();
        #[cfg(target_os = "linux")]
        {
            self.poll_aux_chan(cx);
            msgs.extend(self.take_outbox_msgs());
        }
        let should_bootstrap = !self.presenting() || self.bootstrap_pending;
        if should_bootstrap {
            self.bootstrap_tick_count = self.bootstrap_tick_count.wrapping_add(1);
            let now = self.now();
            if self.bootstrap_tick_count == 1 || self.last_bootstrap_at.map_or(true, |at| now - at >= BOOTSTRAP_RETRY) {
                self.last_bootstrap_at = Some(now);
                msgs.extend(self.build_bootstrap_msgs(cx, target));
                // No frame will confirm it: the geometry sent is the bootstrap.
                if self.frames_external && self.app_ready_for_swapchain && self.last_rect.size.x > 0.0 {
                    self.bootstrap_pending = false;
                }
            }
        }
        if !self.paces_ticks {
            // The pacing window's Tick draws this window too; the pointer
            // this one holds goes out now, ahead of it.
            self.take_moves(&mut msgs);
            self.emit(target.client, msgs);
            self.tick_wanted = false;
            return;
        }
        let now = self.now();
        let due = match self.tick_outstanding {
            None => framed || self.tick_gap_ok(),
            Some(sent_at) if now - sent_at >= self.tick_fallback() => {
                self.trace(&format!("tick-fallback c{} {:.1}ms", target.client, (now - sent_at) * 1000.0));
                true
            }
            Some(_) => false,
        };
        if due {
            self.append_tick(target, &mut msgs);
        }
        self.emit(target.client, msgs);
        // The Tick just sent (or deferred to the child's acknowledgement)
        // serves what was wanted so far; a request that arrives later wants
        // the next one. A beat that came too soon after the last Tick sent
        // nothing: the request stays for the next frame.
        if due || self.tick_outstanding.is_some() {
            self.tick_wanted = false;
        }
    }

    /// This process's GPU device was replaced (`Event::GpuReset`): the shared images the
    /// child renders into belonged to the old one. A swapchain with new ids goes out with the
    /// next bootstrap (the child installs it, importing the new images and retiring the old
    /// ones; the same ids would be taken for the swapchain it has); the child's next frame
    /// lands in it. Until then the tile shows what its snapshot holds.
    fn gpu_reset(&mut self, cx: &mut Cx) {
        let Some(old) = self.swapchain.take() else { return };
        self.swapchain = Some(HostSwapchain::new(old.window_id, old.alloc_width, old.alloc_height, cx));
        self.last_swapchain_with_completed_draws = None;
        self.pending_draw = None;
        self.bootstrap_pending = true;
        self.bootstrap_tick_count = 0;
        self.last_bootstrap_at = None;
        self.tick_wanted = true;
        self.ensure_beat(cx);
    }

    // ---- target and swapchain ------------------------------------------

    /// Point at a child window (or none). False when it already was.
    pub fn set_target(&mut self, cx: &mut Cx, target: Option<RunTarget>) -> bool {
        if self.current_target == target {
            return false;
        }
        self.ticks_sent = 0;
        self.ticks_done = 0;
        #[cfg(target_os = "linux")]
        {
            // Drop the outbox first: Drop shuts the aux endpoint before the
            // client socket is replaced.
            self.outbox = None;
            self.aux_chan_listener = None;
            self.inflight = None;
            self.pending = None;
            self.transport_error = None;
            self.outbox_full_logged = false;
        }
        if self.current_target.map(|target| target.client) != target.map(|target| target.client) {
            self.status = None;
        }
        self.current_target = target;
        self.tick_outstanding = None;
        self.tick_wanted = true;
        if self.wake_timer.0 != 0 {
            cx.stop_timer(self.wake_timer);
            self.wake_timer = Timer::empty();
        }
        self.ensure_beat(cx);
        self.pending_move = None;
        self.swapchain = None;
        self.last_swapchain_with_completed_draws = None;
        self.pending_draw = None;
        self.present_ok_count = 0;
        self.first_present_at = None;
        self.app_ready_for_swapchain = false;
        self.last_rect = Rect::default();
        self.last_dpi_factor = 0.0;
        self.bootstrap_pending = target.is_some();
        self.bootstrap_tick_count = 0;
        self.presented = None;
        self.present_serial += 1;
        true
    }

    /// Point at client `client`'s window and, on Linux, listen for its aux
    /// channel (the GPU descriptor transport). True when the target changed.
    pub fn set_run_target(&mut self, cx: &mut Cx, client: ClientId, window_id: usize, _hub_port: u16) -> bool {
        let changed = self.set_target(cx, Some(RunTarget { client, window_id }));
        #[cfg(target_os = "linux")]
        self.setup_aux_chan(cx, _hub_port, client);
        changed
    }

    /// Point at one of the child `owner`'s later windows (`window_id`), shown as the desk's
    /// client `client`: input and messages go out as `client`'s, the GPU descriptor channel
    /// is `owner`'s (made by its first window's view). True when the target changed.
    pub fn set_window_target(&mut self, cx: &mut Cx, client: ClientId, window_id: usize, _owner: ClientId) -> bool {
        let changed = self.set_target(cx, Some(RunTarget { client, window_id }));
        // The app's first window paces it; this one carries its own geometry, frames and input.
        self.set_paces_ticks(cx, false);
        #[cfg(target_os = "linux")]
        {
            self.borrows_outbox_of = Some(_owner);
        }
        changed
    }

    /// CreateWindow arrived: the child's stdin loop is live, share the
    /// swapchain from now on. True when the target changed with it.
    pub fn app_ready(&mut self, cx: &mut Cx, client: ClientId, window_id: usize) -> bool {
        let target = RunTarget { client, window_id };
        let changed = if self.current_target != Some(target) { self.set_target(cx, Some(target)) } else { false };
        self.app_ready_for_swapchain = true;
        self.tick_outstanding = None;
        self.present_ok_count = 0;
        self.first_present_at = None;
        self.bootstrap_pending = true;
        self.bootstrap_tick_count = 0;
        changed
    }

    pub fn clear_run_target(&mut self, cx: &mut Cx) -> bool {
        self.set_target(cx, None)
    }

    /// The child is told it is `rect` at `dpi_factor` (window geometry), and
    /// gets frames that big: allocates a swapchain for the size when the
    /// current one cannot hold it, and queues a bootstrap on a size change.
    /// A translation alone changes nothing -- child coordinates stay local.
    /// While `target_size` is set the settled size is used instead of `rect`'s.
    pub fn set_geometry(&mut self, cx: &mut Cx, rect: Rect, dpi_factor: f64) {
        let Some(target) = self.current_target else { return };
        let config_rect = Rect {
            pos: rect.pos,
            size: self.target_size.unwrap_or(rect.size),
        };
        self.ensure_swapchain_for_rect(cx, config_rect, dpi_factor, target);
    }

    /// The child is about to be told it is `size` (a phone tile opening
    /// full screen): give it frames that big now. Its frames are otherwise
    /// sized where the view is drawn -- for a tile opening, only once a
    /// full-size frame confirmed the face, which needs frames that big: the
    /// child used to break that loop by drawing its first full frame into
    /// the tile's small frames, clipped (the zoom's smeared frame).
    pub fn prepare_size(&mut self, cx: &mut Cx, size: Vec2d, dpi_factor: f64) {
        let Some(target) = self.current_target else { return };
        self.target_size = Some(size);
        let rect = Rect { pos: self.last_rect.pos, size };
        self.ensure_swapchain_for_rect(cx, rect, dpi_factor, target);
    }

    /// The size the child was last configured at, and the settled size asked for.
    pub fn trace_config(&self) -> (Vec2d, Option<Vec2d>) {
        (self.last_rect.size, self.target_size)
    }

    pub fn set_target_size(&mut self, size: Option<Vec2d>) {
        self.target_size = size;
    }

    /// See `live_resize`. Takes effect with the next `set_geometry`.
    pub fn set_live_resize(&mut self, on: bool) {
        self.live_resize = on;
    }

    fn ensure_swapchain_for_rect(&mut self, cx: &mut Cx, rect: Rect, dpi_factor: f64, target: RunTarget) {
        if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
            return;
        }
        if self.frames_external {
            if self.last_rect.size != rect.size || self.last_dpi_factor != dpi_factor {
                self.bootstrap_pending = true;
                self.bootstrap_tick_count = 0;
            }
            self.last_rect = rect;
            self.last_dpi_factor = dpi_factor;
            return;
        }
        let min_width = ((rect.size.x * dpi_factor).ceil() as u32).max(1);
        let min_height = ((rect.size.y * dpi_factor).ceil() as u32).max(1);
        #[cfg(target_os = "linux")]
        let live = self.live_resize;
        let needs_new_swapchain = self
            .swapchain
            .as_ref()
            .map(|swapchain| {
                #[cfg(target_os = "linux")]
                {
                    swapchain.window_id != target.window_id
                        || !swapchain_size_holds(live, [min_width, min_height], [swapchain.alloc_width, swapchain.alloc_height])
                }
                #[cfg(not(target_os = "linux"))]
                {
                    min_width > swapchain.alloc_width
                        || min_height > swapchain.alloc_height
                        || swapchain.window_id != target.window_id
                }
            })
            .unwrap_or(true);

        // Child coordinates stay local. A desktop translation does not resize
        // the app or replace its shared framebuffer.
        let rect_changed = self.last_rect.size != rect.size || self.last_dpi_factor != dpi_factor;
        if needs_new_swapchain {
            if self.last_swapchain_with_completed_draws.is_none() {
                self.last_swapchain_with_completed_draws = self.swapchain.take();
            } else {
                self.swapchain = None;
            }

            #[cfg(target_os = "linux")]
            let [alloc_width, alloc_height] = swapchain_alloc(live, [min_width, min_height]);
            #[cfg(not(target_os = "linux"))]
            let (alloc_width, alloc_height) = (
                min_width.max(64).next_power_of_two(),
                min_height.max(64).next_power_of_two(),
            );

            self.swapchain = Some(HostSwapchain::new(target.window_id, alloc_width, alloc_height, cx));
        }

        if rect_changed || needs_new_swapchain {
            self.bootstrap_pending = true;
            self.bootstrap_tick_count = 0;
            // An idle child's beat is asleep: the new geometry must still go out.
            self.ensure_beat(cx);
        }

        self.last_rect = rect;
        self.last_dpi_factor = dpi_factor;
    }

    fn build_bootstrap_msgs(&mut self, cx: &mut Cx, target: RunTarget) -> Vec<StudioToApp> {
        if self.last_rect.size.x <= 0.0 || self.last_rect.size.y <= 0.0 {
            return Vec::new();
        }
        let mut outbound = vec![StudioToApp::WindowGeomChange {
            window_id: target.window_id,
            dpi_factor: self.last_dpi_factor,
            left: self.window_origin.x,
            top: self.window_origin.y,
            width: self.last_rect.size.x,
            height: self.last_rect.size.y,
        }];

        if !self.app_ready_for_swapchain {
            return outbound;
        }

        #[cfg(target_os = "linux")]
        {
            self.poll_aux_chan(cx);
            outbound.extend(self.take_outbox_msgs());
            if self.outbox.is_some() && !self.export_busy() {
                let exported = self.swapchain.as_mut().map(|swapchain| export_host_swapchain(swapchain, cx));
                match exported {
                    Some(Ok(batch)) => {
                        if let Err(error) = self.queue_exported(batch) {
                            log!("app host swapchain export queue failed: {error}");
                        }
                    }
                    Some(Err(makepad_platform::shared_framebuf::SharedSwapchainCreateError::NotReady)) => {}
                    Some(Err(err)) => {
                        let error = format!("{err:?}");
                        log!("app host swapchain export failed: {error}");
                        self.transport_error = Some(error);
                    }
                    None => {}
                }
            }
            outbound.extend(self.take_outbox_msgs());
        }
        #[cfg(not(target_os = "linux"))]
        {
            if let Some(swapchain) = self.swapchain.as_ref() {
                if let Some(shared) = shared_swapchain_from_host_swapchain(swapchain, cx) {
                    outbound.push(StudioToApp::Swapchain(shared));
                }
            }
        }
        outbound
    }

    // ---- presented frames ------------------------------------------------

    pub fn has_frame(&self) -> bool {
        self.present_ok_count > 0
    }

    /// Frames are not awaited: they arrive, or the child shows the window
    /// another way (`set_frames_external`).
    pub fn presenting(&self) -> bool {
        self.frames_external || self.present_ok_count > 0
    }

    /// The child shows this window's content some other way (a frame ring
    /// of its own): no shared swapchain is made for it (a window-size
    /// surface less) and no frame is waited for. Set before the window is
    /// targeted.
    pub fn set_frames_external(&mut self, on: bool) {
        self.frames_external = on;
        if on {
            self.swapchain = None;
            self.last_swapchain_with_completed_draws = None;
        }
    }

    /// When the first frame of this target landed.
    pub fn first_present_at(&self) -> Option<f64> {
        self.first_present_at
    }


    /// The frame the child last presented, if any.
    pub fn presented(&self) -> Option<&Presented> {
        self.presented.as_ref()
    }

    /// The size, in pixels, of the frame the child last presented.
    pub fn presented_px(&self) -> Option<[u32; 2]> {
        self.presented.as_ref().map(|p| {
            [(p.tex_scale[0] * p.tex_size[0]).round() as u32, (p.tex_scale[1] * p.tex_size[1]).round() as u32]
        })
    }

    /// The texture of the frame the child last presented.
    pub fn texture(&self) -> Option<Texture> {
        self.presented.as_ref().map(|p| p.texture.clone())
    }

    /// Changes whenever `presented` does (a new frame, a new target).
    pub fn present_serial(&self) -> u64 {
        self.present_serial
    }

    fn presented_from_draw(cx: &mut Cx, presentable_draw: PresentableDraw, swapchain: &HostSwapchain) -> Option<Presented> {
        // Zero-sized frames from early startup races stall bootstrap if
        // treated as presented.
        if presentable_draw.width == 0 || presentable_draw.height == 0 {
            return None;
        }
        let drawn = swapchain.get_image(presentable_draw.target_id)?;
        // A frame bigger than the image it names was clipped by the child
        // (drawn before it had the bigger frames): never stretch it.
        if presentable_draw.width > swapchain.alloc_width || presentable_draw.height > swapchain.alloc_height {
            return None;
        }
        let texture = drawn.texture_for_draw(cx, &presentable_draw, swapchain.alloc_width, swapchain.alloc_height, &swapchain.snapshot)?;
        // The in-band size header exists only on the Linux software path;
        // reading it elsewhere blanks tiles whose top-left pixel is black
        // (see the studio RunView note).
        // macOS children always draw with sequence 0 and carry no header:
        // reading that as "in band" made every macOS frame a placeholder.
        #[cfg(not(target_os = "linux"))]
        let packed_header = 0.0f32;
        #[cfg(target_os = "linux")]
        let packed_header = if presentable_draw.sequence == 0 { 1.0f32 } else { 0.0f32 };
        Some(Presented {
            texture,
            tex_scale: [
                (presentable_draw.width as f32) / (swapchain.alloc_width as f32),
                (presentable_draw.height as f32) / (swapchain.alloc_height as f32),
            ],
            tex_size: [swapchain.alloc_width as f32, swapchain.alloc_height as f32],
            packed_header,
        })
    }

    fn try_present_draw(&mut self, cx: &mut Cx, presentable_draw: PresentableDraw) -> bool {
        if let Some(swapchain) = self.swapchain.as_ref() {
            if let Some(presented) = Self::presented_from_draw(cx, presentable_draw, swapchain) {
                self.last_swapchain_with_completed_draws = None;
                self.presented = Some(presented);
                self.present_serial += 1;
                return true;
            }
        }
        if let Some(swapchain) = self.last_swapchain_with_completed_draws.as_ref() {
            if let Some(presented) = Self::presented_from_draw(cx, presentable_draw, swapchain) {
                self.presented = Some(presented);
                self.present_serial += 1;
                return true;
            }
        }
        false
    }

    /// A frame the child finished (`DrawCompleteAndFlip`): makes its image
    /// the presented texture. True when it was applied; otherwise it is kept
    /// and retried by `retry_pending_draw`.
    pub fn set_presentable_draw(&mut self, cx: &mut Cx, presentable_draw: PresentableDraw) -> bool {
        if makepad_error_log::trace_enabled("pace") {
            log!("pace wm flip_rx={:.2} c{}", (self.clock.wall_now)() * 1000.0 % 1.0e6, self.client().unwrap_or(0));
        }
        if self.try_present_draw(cx, presentable_draw) {
            self.trace(&format!("pd c{}", self.client().unwrap_or(0)));
            self.pending_draw = None;
            self.present_ok_count += 1;
            if self.present_ok_count == 1 {
                // Whatever frames come in first, they fade in quickly
                // rather than popping over the "starting..." panel.
                self.first_present_at = Some(self.now());
            }
            // A present settles the bootstrap only once its messages went
            // out for the current rect (the first bootstrap beat sends
            // them). A frame already in flight when the size changed
            // must not cancel a geometry the child was never sent: it kept
            // drawing at the old size, and the view showed that frame
            // stretched to the new rect.
            if self.bootstrap_tick_count > 0 {
                self.bootstrap_pending = false;
                self.bootstrap_tick_count = 0;
            }
            true
        } else {
            // A frame still waiting for its image is replaced by a newer one.
            self.pending_draw = Some(presentable_draw);
            false
        }
    }

    /// Try the frame that could not be applied when it arrived (its
    /// swapchain image was not there yet). True when it was applied.
    pub fn retry_pending_draw(&mut self, cx: &mut Cx) -> bool {
        match self.pending_draw {
            Some(pd) => self.set_presentable_draw(cx, pd),
            None => false,
        }
    }

    // ---- Linux aux channel and GPU transitions ---------------------------

    #[cfg(target_os = "linux")]
    fn setup_aux_chan(&mut self, _cx: &mut Cx, hub_port: u16, client: ClientId) {
        if self.outbox.is_some() || self.aux_chan_listener.is_some() {
            return;
        }
        let studio_addr = format!("http://127.0.0.1:{}", hub_port);
        let listener = match ExternalEndpointListener::new_for_studio(&studio_addr, &client.to_string()) {
            Ok(listener) => listener,
            Err(err) => {
                log!("app host aux_chan listener failed: {}", err);
                return;
            }
        };
        // The listener is nonblocking. Poll it on the existing view timer;
        // a dormant child must not monopolize the shared heavy worker.
        // Keep listening for as long as the owner keeps the child alive.
        // The owner polls the process, reports its exit status and clears
        // the run target on exit; a slow startup is not a transport error.
        self.aux_chan_listener = Some(listener);
    }

    #[cfg(target_os = "linux")]
    fn poll_aux_chan(&mut self, cx: &mut Cx) {
        if self.outbox.is_none() {
            if let Some(owner) = self.borrows_outbox_of {
                self.outbox = SharedOutbox::of(owner);
                return;
            }
        }
        let Some(listener) = &self.aux_chan_listener else {
            return;
        };
        match listener.try_accept_host_endpoint() {
            Ok(Some(endpoint)) => {
                self.aux_chan_listener = None;
                match LinuxSwapchainSender::start(&cx.thread_spawner(), endpoint) {
                    Ok(sender) => self.outbox = self.current_target.map(|target| SharedOutbox::new(target.client, sender)),
                    Err(error) => {
                        log!("app host aux_chan outbox failed: {error}");
                        self.transport_error = Some(error);
                    }
                }
            }
            Err(error) => {
                log!("app host aux_chan accept failed: {error}");
                self.transport_error = Some(error.to_string());
                self.aux_chan_listener = None;
            }
            Ok(None) => {}
        }
    }

    #[cfg(target_os = "linux")]
    fn export_busy(&self) -> bool {
        self.pending.is_some() || self.inflight.is_some()
    }

    /// Queue one exported batch. One transaction at a time: the caller must
    /// not export while `export_busy`. Full retains the batch for retry.
    #[cfg(target_os = "linux")]
    fn queue_exported(&mut self, batch: ExportedHostSwapchain) -> Result<(), String> {
        let Some(outbox) = self.outbox.as_ref() else {
            return Err("GPU descriptor outbox is not ready".into());
        };
        let mut outbox = outbox.0.borrow_mut();
        let tag = outbox.next_tag;
        outbox.next_tag = outbox.next_tag.wrapping_add(1);
        self.inflight = Some(tag);
        match outbox.sender.try_send(tag, batch) {
            Ok(()) => Ok(()),
            Err(batch) => {
                if let Some(error) = outbox.sender.error().map(str::to_string) {
                    self.inflight = None;
                    self.transport_error = Some(error.clone());
                    Err(error)
                } else {
                    self.pending = Some((tag, batch));
                    if !self.outbox_full_logged {
                        self.outbox_full_logged = true;
                        log!("app host swapchain outbox full; retrying the same batch");
                    }
                    Ok(())
                }
            }
        }
    }

    /// Retry a Full batch and collect completed descriptor metadata. Always
    /// emit completed tags: the FDs are already in the child's ordered stream.
    #[cfg(target_os = "linux")]
    fn take_outbox_msgs(&mut self) -> Vec<StudioToApp> {
        let mut msgs = Vec::new();
        let Some(shared) = self.outbox.clone() else {
            return msgs;
        };
        let mut outbox = shared.0.borrow_mut();
        if let Some((tag, batch)) = self.pending.take() {
            match outbox.sender.try_send(tag, batch) {
                Ok(()) => {}
                Err(batch) => {
                    if let Some(error) = outbox.sender.error().map(str::to_string) {
                        self.inflight = None;
                        self.transport_error = Some(error);
                    } else {
                        self.pending = Some((tag, batch));
                    }
                }
            }
        }
        // The child reads the descriptors in the order they went out and pairs each
        // `Swapchain` message with the next ones: the messages must reach it in that same
        // order, whichever window they are for. So the view that polls sends every completed
        // batch now, in order (they all go to the same child), and each view clears its own
        // in-flight tag when it sees it done. A view taking only its own batch let the
        // windows' messages overtake each other: the child dropped the other window's
        // descriptors as stale and stalled on them.
        for sent in outbox.sender.poll() {
            outbox.done_tags.push(sent.tag);
            msgs.push(StudioToApp::Swapchain(sent.swapchain));
        }
        if let Some(tag) = self.inflight {
            if let Some(at) = outbox.done_tags.iter().position(|done| *done == tag) {
                outbox.done_tags.swap_remove(at);
                self.inflight = None;
            }
        }
        if let Some(error) = outbox.sender.error().map(str::to_string) {
            if self.transport_error.is_none() {
                self.transport_error = Some(error);
            }
        }
        msgs
    }

}

/// A child process's GPU descriptor channel, shared by the views of its windows (the
/// channel's descriptor stream is ordered: one sender, tags allocated here).
#[cfg(target_os = "linux")]
#[derive(Clone)]
struct SharedOutbox(std::rc::Rc<std::cell::RefCell<OutboxState>>);

#[cfg(target_os = "linux")]
struct OutboxState {
    sender: LinuxSwapchainSender,
    next_tag: u64,
    /// Completed batches (already sent to the child) whose view has not seen them yet.
    done_tags: Vec<u64>,
}

#[cfg(target_os = "linux")]
thread_local! {
    /// The channel of each child, by the client its first window's view shows: what the
    /// views of its later windows borrow.
    static OUTBOXES: std::cell::RefCell<HashMap<ClientId, std::rc::Weak<std::cell::RefCell<OutboxState>>>> = Default::default();
}

#[cfg(target_os = "linux")]
impl SharedOutbox {
    fn new(client: ClientId, sender: LinuxSwapchainSender) -> SharedOutbox {
        let shared = SharedOutbox(std::rc::Rc::new(std::cell::RefCell::new(OutboxState { sender, next_tag: 0, done_tags: Vec::new() })));
        OUTBOXES.with(|o| {
            let mut o = o.borrow_mut();
            o.retain(|_, weak| weak.strong_count() > 0);
            o.insert(client, std::rc::Rc::downgrade(&shared.0));
        });
        shared
    }
    fn of(client: ClientId) -> Option<SharedOutbox> {
        OUTBOXES.with(|o| o.borrow().get(&client).and_then(|weak| weak.upgrade())).map(SharedOutbox)
    }
}

#[cfg(test)]
mod pacing_tests {
    use super::*;
    use std::cell::Cell;

    thread_local!(static NOW: Cell<f64> = Cell::new(0.0));
    fn now() -> f64 {
        NOW.with(Cell::get)
    }
    fn at_ms(ms: f64) {
        NOW.with(|n| n.set(ms / 1000.0));
    }

    fn app() -> HostedApp {
        let mut app = HostedApp::new(Clock { now, wall_now: now });
        app.current_target = Some(RunTarget { client: 1, window_id: 0 });
        app
    }

    #[test]
    fn the_gap_opens_after_most_of_a_frame() {
        assert!(tick_gap_open(None, 0.0, 0.008));
        assert!(!tick_gap_open(Some(0.0), 0.001, 0.008));
        assert!(tick_gap_open(Some(0.0), 0.0073, 0.008));
    }

    fn settled_app() -> HostedApp {
        let mut app = app();
        app.present_ok_count = 1;
        app.bootstrap_pending = false;
        app.tick_wanted = false;
        app
    }

    /// One Tick in flight at a time: while it is out the only timer is the
    /// fallback, however often the child asks; after the ack the next Tick
    /// waits for the owner's next frame, never a clock of its own (a child
    /// ticked every 8 ms drew 2.4 frames per frame the WM showed).
    #[test]
    fn a_request_during_a_tick_waits_for_the_owners_next_frame() {
        at_ms(100.0);
        let mut app = settled_app();
        app.tick_wanted = true;
        assert_eq!(app.next_beat(), Some(Beat::Frame));
        let mut msgs = Vec::new();
        app.append_tick(app.current_target.unwrap(), &mut msgs);
        app.tick_wanted = false;
        assert_eq!(app.ticks_sent, 1);
        // the child asks again while it draws
        at_ms(101.0);
        app.tick_wanted = true;
        assert_eq!(app.next_beat(), Some(Beat::At(0.1 + app.tick_fallback())), "nothing before the ack");
        app.tick_done();
        assert!(app.tick_outstanding.is_none());
        assert_eq!(app.next_beat(), Some(Beat::Frame), "the owner's next frame, not the ack, not a clock");
        // a child slower than a frame: the same, its Tick goes out with the owner's next frame
        at_ms(130.0);
        assert_eq!(app.next_beat(), Some(Beat::Frame));
    }

    /// A live resize keeps frames that hold the new size and makes new ones with room to
    /// grow; at rest the frames are the window's exact size again.
    #[test]
    fn a_live_resize_keeps_frames_that_hold_the_size() {
        assert!(swapchain_size_holds(true, [800, 600], [1000, 800]));
        assert!(!swapchain_size_holds(true, [1001, 600], [1000, 800]));
        assert!(!swapchain_size_holds(false, [800, 600], [1000, 800]));
        assert!(swapchain_size_holds(false, [800, 600], [800, 600]));
        assert_eq!(swapchain_alloc(false, [801, 601]), [801, 601]);
        let [w, h] = swapchain_alloc(true, [801, 601]);
        assert!(w >= 801 + 256 && h >= 601 + 256 && w % 64 == 0 && h % 64 == 0);
        let [w, _] = swapchain_alloc(true, [3000, 100]);
        assert!(w >= 3750, "a quarter more for big windows: {w}");
        // A drag of 1 px per frame from 800 to 1200 wide makes new frames once.
        let mut alloc = swapchain_alloc(true, [800, 600]);
        let mut made = 0;
        for width in 800..=1200 {
            if !swapchain_size_holds(true, [width, 600], alloc) {
                alloc = swapchain_alloc(true, [width, 600]);
                made += 1;
            }
        }
        assert_eq!(made, 1);
    }

    #[test]
    fn no_request_no_tick() {
        at_ms(200.0);
        let mut app = settled_app();
        assert!(!app.beat_needed());
        let mut msgs = Vec::new();
        app.append_tick(app.current_target.unwrap(), &mut msgs);
        at_ms(205.0);
        app.tick_done();
        assert!(!app.beat_needed(), "nothing asked, nothing owed: no timer, no tick");
        app.tick_wanted = true;
        assert!(app.beat_needed());
    }
}
