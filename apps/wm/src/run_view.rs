//! A tile hosting one Makepad child process, ported from Studio's
//! DesktopRunView (studio/desktop/src/desktop_run_view.rs): shared-GPU
//! swapchain presentation plus input forwarding over the studio protocol.
//! Trimmed of studio-only concerns (remote PNG frames, AI input viz) and
//! given rounded corners — the child texture is clipped by a rounded-rect
//! mask in the shader, Omarchy-style.

use crate::hub::ClientId;
use makepad_studio_protocol::{
    MouseButton, PresentableDraw, RemoteKeyModifiers, RemoteMouseDown, RemoteMouseMove,
    RemoteMouseUp, RemoteScroll, StudioToApp, StudioToAppVec,
};
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
use makepad_studio_protocol::HostToAppGpu;
use makepad_widgets::makepad_micro_serde::SerBin;
use makepad_widgets::makepad_platform::shared_framebuf::HostSwapchain;
#[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
use makepad_widgets::makepad_platform::shared_framebuf::shared_swapchain_from_host_swapchain;
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
use makepad_widgets::makepad_platform::shared_framebuf::{
    export_host_swapchain, ExportedHostSwapchain, LinuxSwapchainSender,
};
use makepad_widgets::*;

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
use makepad_widgets::makepad_platform::shared_framebuf::aux_chan::ExternalEndpointListener;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.MpRunViewBase = #(MpRunView::register_widget(vm))

    mod.widgets.MpRunView = set_type_default() do mod.widgets.MpRunViewBase {
        width: Fill
        height: Fill
        draw_bg +: {
            color: uniform(#0000)
            radius: uniform(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, self.radius)
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_app +: {
            tex: texture_2d(float)
            tex_scale: instance(vec2(0.0, 0.0))
            tex_size: instance(vec2(1.0, 1.0))
            host_dpi_factor: instance(1.0)
            y_flip: instance(0.0)
            packed_header: instance(1.0)
            // The close-crop: while a tile closes, its quad shrinks but the
            // frozen app image must STAY PUT — the quad becomes a moving
            // window over the unmoving texture. crop_origin/crop_span map
            // this quad into the ORIGINAL tile rect (identity when open).
            crop_origin: instance(vec2(0.0, 0.0))
            crop_span: instance(vec2(1.0, 1.0))
            // The popin fade: Hyprland fades the WHOLE snapshot while it
            // pops (146ms almostLinear); without this the opaque content
            // shrinking reads as a diagonal slide instead of a soft pop.
            // Premultiplied output, so one multiply fades everything.
            fade: instance(1.0)
            pixel: fn() {
                let cpos = self.crop_origin + self.pos * self.crop_span
                let uv = vec2(cpos.x, cpos.y + self.y_flip - 2.0 * self.y_flip * cpos.y)
                if self.packed_header < 0.5 {
                    return self.tex.sample(uv * self.tex_scale) * self.fade
                }
                let tp1 = self.tex.sample(vec2(0.5 / self.tex_size.x, 0.5 / self.tex_size.y))
                let tp2 = self.tex.sample(vec2(1.5 / self.tex_size.x, 0.5 / self.tex_size.y))
                let tp = vec2(tp1.r * 65280.0 + tp1.b * 255.0, tp2.r * 65280.0 + tp2.b * 255.0)
                if tp.x <= 0.0 || tp.y <= 0.0 {
                    return #0000
                }
                // The mapping uses the ORIGINAL rect size (quad / span),
                // so texels remain screen-fixed while the quad shrinks.
                let counter = ((self.rect_size / self.crop_span) * self.host_dpi_factor) / tp
                let tex_scale = tp / self.tex_size
                // Row zero carries the child's size tracking pixels. Never
                // blend that transport metadata (or unused swapchain space)
                // into the visible window edge.
                let sample_uv = clamp(uv * tex_scale * counter,
                    vec2(0.5, 1.5) / self.tex_size,
                    max(tp - vec2(0.5), vec2(0.5, 1.5)) / self.tex_size)
                let fb = self.tex.sample(sample_uv)
                if fb.r == 1.0 && fb.g == 0.0 && fb.b == 1.0 {
                    return #2 * self.fade
                }
                return fb * self.fade
            }
        }
        no_fb_view: Splash {
            width: Fill
            height: Fill
        }
    }
}

/// The arrival crossfade length: the wash fades out while the first
/// frames fade in, smoothstepped, long enough to read as a resolve
/// rather than a zap.
const ARRIVAL_FADE_SECS: f32 = 0.28;

/// Drag-stall hunt: timestamped trace lines appended to the file named by
/// MAKEPAD_WM_TRACE. Free when unset (one static branch). Timestamps are UNIX ms
/// (mod 1e7) so host and child (MAKEPAD_STUDIO_TRACE) lines correlate.
pub fn trace_host(line: &str) {
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
    let ms = crate::host::wall_now() * 1000.0;
    if let Ok(mut f) = file.lock() {
        let _ = writeln!(f, "{:.2} H {}", ms % 1.0e7, line);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RunTarget {
    client: ClientId,
    window_id: usize,
}

/// One GPU renderer-replacement transaction for this tile. Native Linux only.
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
struct GpuTransition {
    id: u64,
    epoch: u64,
    candidate: Option<HostSwapchain>,
    queued: bool,
}

#[derive(Clone, Debug, Default)]
pub enum MpRunViewAction {
    ForwardToApp {
        client: ClientId,
        msg_bin: Vec<u8>,
    },
    /// The user clicked this tile (the WM moves focus to it).
    Clicked {
        client: ClientId,
    },
    /// A module instance's root panicked under this tile (in an event or
    /// a draw). The tile has let go of the root and shows "crashed"; the
    /// WM tears the instance down in the host's order.
    Crashed {
        client: ClientId,
        message: String,
    },
    #[default]
    None,
}

#[derive(Script, Widget)]
pub struct MpRunView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[redraw]
    #[live]
    draw_app: DrawQuad,
    #[find]
    #[redraw]
    #[live]
    no_fb_view: WidgetRef,
    #[rust]
    area: Area,
    /// FOCUS RULE: false on a Quick-Look panel. Clicks still hit-test and
    /// still reach the child (hover, scroll, buttons all work), but the
    /// press does not pull the compositor's key focus off the requester —
    /// files keeps the keyboard so its arrows go on dialing.
    #[rust(true)]
    takes_key_focus: bool,
    /// Newest stdout/stderr line from the child, shown while it starts.
    #[rust]
    status_line: String,
    #[rust]
    startup_initialized: bool,
    #[rust] startup_glass: bool,
    #[rust] startup_glass_applied: Option<bool>,
    #[rust]
    startup_app: String,
    /// While closing: this quad's place inside the ORIGINAL tile rect
    /// (normalized origin + span), so the frozen frame stays screen-fixed
    /// and the shrinking quad merely crops it.
    #[rust]
    close_crop: Option<(Vec2d, Vec2d)>,
    /// The popin fade (1.0 = solid); the desk drives it during open/close.
    #[rust(1.0f32)]
    fade: f32,
    /// When the FIRST frame landed: the content fades in quickly from the
    /// "starting…" panel instead of popping on abruptly.
    #[rust]
    first_present_at: Option<f64>,
    #[rust]
    tick_timer: Timer,
    /// The tile tick's period: the unit of the Tick pacing below.
    #[rust]
    tick_period: f64,
    /// When the `Tick` the child has not acknowledged (`TickDone`) yet
    /// went out. The timer sends the next Tick only once this clears, so
    /// a slow child never has more than one frame's ticks and pointer
    /// moves queued behind it; a child that never acknowledges (an older
    /// binary, a GPU handoff pause) is pumped again after `tick_fallback`.
    #[rust]
    tick_outstanding: Option<f64>,
    /// A timer beat arrived while the child was busy. Retain one request so
    /// its acknowledgement can start the next frame without another beat.
    #[rust]
    tick_deferred: bool,
    /// The pointer's latest position since the last Tick went out: the
    /// child sees at most one MouseMove per frame, flushed ahead of the
    /// Tick or of any Down/Up/Scroll so their order holds.
    #[rust]
    pending_move: Option<RemoteMouseMove>,
    #[rust]
    last_rect: Rect,
    #[rust]
    last_dpi_factor: f64,
    #[rust]
    redraw_countdown: usize,
    #[rust]
    bootstrap_pending: bool,
    #[rust]
    bootstrap_tick_count: u32,
    #[rust]
    current_target: Option<RunTarget>,
    #[rust]
    swapchain: Option<HostSwapchain>,
    #[rust]
    last_swapchain_with_completed_draws: Option<HostSwapchain>,
    #[rust]
    pending_draw: Option<PresentableDraw>,
    #[rust]
    present_ok_count: usize,
    #[rust]
    app_ready_for_swapchain: bool,
    #[rust]
    remote_cursor: MouseCursor,
    #[rust]
    is_hovered: bool,
    #[rust]
    ime_pos: Option<Vec2d>,
    /// While the tile rect is being ANIMATED, the layout's settled target
    /// size. The quad draws at the animated rect; the swapchain and the
    /// child's WindowGeomChange always use this, so a tween never causes
    /// swapchain churn or per-frame child relayouts (see
    /// local/agent_state/wm/resize-sync-design.md).
    #[rust]
    target_size: Option<Vec2d>,

    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    outbox: Option<LinuxSwapchainSender>,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    aux_chan_listener: Option<ExternalEndpointListener>,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    aux_chan_deadline: f64,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    next_export_tag: u64,
    /// In-flight descriptor batch: tag and optional GPU (transition, epoch).
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    inflight: Option<(u64, Option<(u64, u64)>)>,
    /// `try_send` Full: the same batch, retried by ownership.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    pending: Option<(u64, ExportedHostSwapchain)>,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    gpu_transition: Option<GpuTransition>,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    frozen: Option<u64>,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    retired_swapchain: Option<HostSwapchain>,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    transport_error: Option<String>,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    outbox_full_logged: bool,
}

impl ScriptHook for MpRunView {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            self.draw_app.set_texture(0, &cx.null_texture());
            // 240 Hz direct sessions must not be throttled by the old 125 Hz poll.
            self.tick_period = if matches!(cx.os_type(), OsType::LinuxDirect) { 1.0 / 240.0 } else { 0.008 };
            self.tick_timer = cx.start_interval(self.tick_period);
            self.draw_app
                .draw_vars
                .set_dyn_instance(cx, id!(packed_header), &[1.0f32]);
        });
    }
}

impl MpRunView {
    fn emit_to_app(&self, cx: &mut Cx, client: ClientId, msgs: Vec<StudioToApp>) {
        if msgs.is_empty() {
            return;
        }
        let msg_bin = StudioToAppVec(msgs).serialize_bin();
        cx.widget_action(self.uid, MpRunViewAction::ForwardToApp { client, msg_bin });
    }

    /// A press, release or scroll: the pointer position it happened at
    /// goes out first, in the same batch, so the child sees the move
    /// before the edge exactly as the host did.
    fn emit_after_pending_move(&mut self, cx: &mut Cx, client: ClientId, msg: StudioToApp) {
        let mut msgs = Vec::with_capacity(2);
        if let Some(mv) = self.pending_move.take() {
            msgs.push(StudioToApp::MouseMove(mv));
        }
        msgs.push(msg);
        self.emit_to_app(cx, client, msgs);
    }

    /// How long a Tick may stay unacknowledged before the child is pumped
    /// anyway: four tick periods, never under 50 ms, so a child that does
    /// not speak `TickDone` still runs at 20 Hz.
    fn tick_fallback(&self) -> f64 {
        (self.tick_period * 4.0).max(0.050)
    }

    fn append_tick(&mut self, target: RunTarget, msgs: &mut Vec<StudioToApp>) {
        trace_host(&format!("tick c{}", target.client));
        if let Some(mv) = self.pending_move.take() {
            msgs.push(StudioToApp::MouseMove(mv));
        }
        msgs.push(StudioToApp::Tick);
        self.tick_outstanding = Some(crate::host::now());
        self.tick_deferred = false;
    }

    /// Return one tick credit. If a timer beat was missed while the child
    /// rendered, service that retained request now instead of quantizing a
    /// slightly late child to half the compositor's frame rate.
    pub fn tick_done(&mut self, cx: &mut Cx) {
        if let Some(target) = self.current_target {
            trace_host(&format!("ack c{}", target.client));
        }
        if self.tick_outstanding.take().is_none() {
            return;
        }
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        if self.frozen.is_some() {
            self.tick_deferred = false;
            return;
        }
        if self.tick_deferred {
            if let Some(target) = self.current_target {
                let mut msgs = Vec::new();
                self.append_tick(target, &mut msgs);
                self.emit_to_app(cx, target.client, msgs);
            }
        }
    }

    fn set_target(&mut self, cx: &mut Cx, target: Option<RunTarget>) {
        if self.current_target == target {
            return;
        }
        let had_target = self.current_target.is_some();
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        {
            // Drop the outbox first: Drop shuts the aux endpoint before the
            // client socket is replaced.
            self.outbox = None;
            self.aux_chan_listener = None;
            self.inflight = None;
            self.pending = None;
            self.gpu_transition = None;
            self.frozen = None;
            self.retired_swapchain = None;
            self.transport_error = None;
            self.outbox_full_logged = false;
        }
        self.current_target = target;
        self.tick_outstanding = None;
        self.tick_deferred = false;
        self.pending_move = None;
        self.remote_cursor = MouseCursor::Default;
        self.is_hovered = false;
        self.swapchain = None;
        self.last_swapchain_with_completed_draws = None;
        self.pending_draw = None;
        self.present_ok_count = 0;
        self.first_present_at = None;
        self.app_ready_for_swapchain = false;
        self.ime_pos = None;
        self.last_rect = Rect::default();
        self.last_dpi_factor = 0.0;
        self.bootstrap_pending = target.is_some();
        self.bootstrap_tick_count = 0;
        if target.is_some() {
            // Keep redrawing during startup so bootstrap messages resend
            // until the child socket is ready.
            self.redraw_countdown = self.redraw_countdown.max(240);
        } else {
            if had_target {
                cx.hide_text_ime();
            }
            self.redraw_countdown = 0;
        }
        self.draw_app.set_texture(0, &cx.null_texture());
        self.draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(tex_scale), &[0.0f32, 0.0f32]);
        self.draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(tex_size), &[1.0f32, 1.0f32]);
        self.draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(y_flip), &[0.0f32]);
        self.draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(packed_header), &[1.0f32]);
        self.redraw(cx);
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
        self.draw_app.redraw(cx);
        self.no_fb_view.redraw(cx);
    }

    pub fn set_remote_cursor(&mut self, cx: &mut Cx, cursor: MouseCursor) {
        self.remote_cursor = cursor;
        if self.is_hovered {
            cx.set_cursor(self.remote_cursor);
        }
    }

    fn apply_presentable_draw_to_quad(
        cx: &mut Cx,
        draw_app: &mut DrawQuad,
        redraw_countdown: &mut usize,
        presentable_draw: PresentableDraw,
        swapchain: &HostSwapchain,
    ) -> bool {
        // Zero-sized frames from early startup races stall bootstrap if
        // treated as presented.
        if presentable_draw.width == 0 || presentable_draw.height == 0 {
            return false;
        }
        let Some(drawn) = swapchain.get_image(presentable_draw.target_id) else {
            return false;
        };

        let Some(texture) = drawn.texture_for_draw(cx, &presentable_draw, swapchain.alloc_width, swapchain.alloc_height) else {
            return false;
        };
        draw_app.set_texture(0, &texture);
        draw_app.draw_vars.set_dyn_instance(
            cx,
            id!(tex_scale),
            &[
                (presentable_draw.width as f32) / (swapchain.alloc_width as f32),
                (presentable_draw.height as f32) / (swapchain.alloc_height as f32),
            ],
        );
        draw_app.draw_vars.set_dyn_instance(
            cx,
            id!(tex_size),
            &[
                (swapchain.alloc_width as f32),
                (swapchain.alloc_height as f32),
            ],
        );
        // The in-band size header exists only on the Linux software path;
        // reading it elsewhere blanks tiles whose top-left pixel is black
        // (see the studio RunView note).
        #[cfg(target_os = "windows")]
        draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(packed_header), &[0.0f32]);
        #[cfg(not(target_os = "windows"))]
        draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(packed_header), &[if presentable_draw.sequence == 0 { 1.0f32 } else { 0.0f32 }]);
        // Linux's software fallback is copied row-for-row from a top-left
        // framebuffer and needs the historical shader flip. A GPU-shared
        // DMA-BUF texture already has the orientation expected by the GL
        // sampler; flipping that path turns every hosted app upside down.
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        draw_app.draw_vars.set_dyn_instance(
            cx,
            id!(y_flip),
            &[if drawn.software_buffer.is_some() {
                1.0f32
            } else {
                0.0f32
            }],
        );
        #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
        draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(y_flip), &[0.0f32]);

        *redraw_countdown = (*redraw_countdown).max(20);
        true
    }

    fn try_present_draw(&mut self, cx: &mut Cx, presentable_draw: PresentableDraw) -> bool {
        if let Some(swapchain) = self.swapchain.as_ref() {
            if Self::apply_presentable_draw_to_quad(
                cx,
                &mut self.draw_app,
                &mut self.redraw_countdown,
                presentable_draw,
                swapchain,
            ) {
                self.last_swapchain_with_completed_draws = None;
                self.redraw(cx);
                return true;
            }
        }
        if let Some(swapchain) = self.last_swapchain_with_completed_draws.as_ref() {
            if Self::apply_presentable_draw_to_quad(
                cx,
                &mut self.draw_app,
                &mut self.redraw_countdown,
                presentable_draw,
                swapchain,
            ) {
                self.redraw(cx);
                return true;
            }
        }
        false
    }

    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    fn setup_aux_chan(&mut self, _cx: &mut Cx, hub_port: u16, client: ClientId) {
        if self.outbox.is_some() || self.aux_chan_listener.is_some() {
            return;
        }
        let studio_addr = format!("http://127.0.0.1:{}", hub_port);
        let listener = match ExternalEndpointListener::new_for_studio(
            &studio_addr,
            &client.to_string(),
        ) {
            Ok(listener) => listener,
            Err(err) => {
                log!("wm aux_chan listener failed: {}", err);
                return;
            }
        };
        // The listener is nonblocking. Poll it on the existing view timer;
        // a dormant child must not monopolize the shared heavy worker.
        self.aux_chan_listener = Some(listener);
        self.aux_chan_deadline = crate::host::now() + 120.0;
    }

    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    fn poll_aux_chan(&mut self, cx: &mut Cx) {
        let Some(listener) = &self.aux_chan_listener else {
            return;
        };
        match listener.try_accept_host_endpoint() {
            Ok(Some(endpoint)) => {
                self.aux_chan_listener = None;
                match LinuxSwapchainSender::start(&cx.thread_spawner(), endpoint) {
                    Ok(sender) => self.outbox = Some(sender),
                    Err(error) => {
                        log!("wm aux_chan outbox failed: {error}");
                        self.transport_error = Some(error);
                    }
                }
            }
            Err(error) => {
                log!("wm aux_chan accept failed: {error}");
                self.transport_error = Some(error.to_string());
                self.aux_chan_listener = None;
            }
            Ok(None) if crate::host::now() >= self.aux_chan_deadline => {
                let error = "timeout while waiting for child aux-channel connection";
                log!("wm aux_chan accept failed: {error}");
                self.transport_error = Some(error.into());
                self.aux_chan_listener = None;
            }
            Ok(None) => {}
        }
    }

    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    fn export_busy(&self) -> bool {
        self.pending.is_some() || self.inflight.is_some()
    }

    /// Queue one exported batch. One transaction at a time: the caller must
    /// not export while `export_busy`. Full retains the batch for retry.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    fn queue_exported(
        &mut self,
        batch: ExportedHostSwapchain,
        kind: Option<(u64, u64)>,
    ) -> Result<(), String> {
        let tag = self.next_export_tag;
        self.next_export_tag = self.next_export_tag.wrapping_add(1);
        self.inflight = Some((tag, kind));
        let Some(outbox) = self.outbox.as_mut() else {
            self.inflight = None;
            return Err("GPU descriptor outbox is not ready".into());
        };
        match outbox.try_send(tag, batch) {
            Ok(()) => Ok(()),
            Err(batch) => {
                if let Some(error) = outbox.error().map(str::to_string) {
                    self.inflight = None;
                    self.transport_error = Some(error.clone());
                    Err(error)
                } else {
                    self.pending = Some((tag, batch));
                    if !self.outbox_full_logged {
                        self.outbox_full_logged = true;
                        log!("wm swapchain outbox full; retrying the same batch");
                    }
                    Ok(())
                }
            }
        }
    }

    /// Retry a Full batch and collect completed descriptor metadata. Always
    /// emit completed tags: the FDs are already in the child's ordered stream.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    fn take_outbox_msgs(&mut self) -> Vec<StudioToApp> {
        let mut msgs = Vec::new();
        let Some(outbox) = self.outbox.as_mut() else {
            return msgs;
        };
        if let Some((tag, batch)) = self.pending.take() {
            match outbox.try_send(tag, batch) {
                Ok(()) => {}
                Err(batch) => {
                    if let Some(error) = outbox.error().map(str::to_string) {
                        self.inflight = None;
                        self.transport_error = Some(error);
                    } else {
                        self.pending = Some((tag, batch));
                    }
                }
            }
        }
        let completed = outbox.poll();
        let sender_error = outbox.error().map(str::to_string);
        for sent in completed {
            let kind = match self.inflight {
                Some((tag, kind)) if tag == sent.tag => {
                    self.inflight = None;
                    kind
                }
                _ => {
                    self.transport_error = Some("GPU descriptor completion tag mismatch".into());
                    // Unknown metadata cannot safely describe the ordered
                    // FD stream. Close this transport, rather than rebinding
                    // descriptors to an ordinary swapchain by accident.
                    self.outbox = None;
                    self.pending = None;
                    self.inflight = None;
                    return msgs;
                }
            };
            msgs.push(match kind {
                Some((transition, transport_epoch)) => StudioToApp::Gpu(HostToAppGpu::Swapchain {
                    transition,
                    transport_epoch,
                    swapchain: sent.swapchain,
                }),
                None => StudioToApp::Swapchain(sent.swapchain),
            });
        }
        if let Some(error) = sender_error {
            if self.transport_error.is_none() {
                self.transport_error = Some(error);
            }
        }
        msgs
    }

    fn ensure_swapchain_for_rect(
        &mut self,
        cx: &mut Cx,
        rect: Rect,
        dpi_factor: f64,
        target: RunTarget,
    ) {
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        if self.frozen.is_some() {
            return;
        }
        if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
            return;
        }
        let min_width = ((rect.size.x * dpi_factor).ceil() as u32).max(1);
        let min_height = ((rect.size.y * dpi_factor).ceil() as u32).max(1);
        let needs_new_swapchain = self
            .swapchain
            .as_ref()
            .map(|swapchain| {
                #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                {
                    min_width != swapchain.alloc_width
                        || min_height != swapchain.alloc_height
                        || swapchain.window_id != target.window_id
                }
                #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
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

            #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
            let (alloc_width, alloc_height) = (min_width.max(1), min_height.max(1));
            #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
            let (alloc_width, alloc_height) = (
                min_width.max(64).next_power_of_two(),
                min_height.max(64).next_power_of_two(),
            );

            self.swapchain = Some(HostSwapchain::new(
                target.window_id,
                alloc_width,
                alloc_height,
                cx,
            ));
        }

        if rect_changed || needs_new_swapchain {
            self.bootstrap_pending = true;
            self.bootstrap_tick_count = 0;
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
            left: 0.0,
            top: 0.0,
            width: self.last_rect.size.x,
            height: self.last_rect.size.y,
        }];

        if !self.app_ready_for_swapchain {
            return outbound;
        }

        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        {
            self.poll_aux_chan(cx);
            outbound.extend(self.take_outbox_msgs());
            if self.outbox.is_some() && !self.export_busy() {
                let exported = self.swapchain.as_mut().map(|swapchain| export_host_swapchain(swapchain, cx));
                match exported {
                    Some(Ok(batch)) => {
                        if let Err(error) = self.queue_exported(batch, None) {
                            log!("wm swapchain export queue failed: {error}");
                        }
                    }
                    Some(Err(err)) => {
                        let error = format!("{err:?}");
                        log!("wm swapchain export failed: {error}");
                        self.transport_error = Some(error);
                    }
                    None => {}
                }
            }
            outbound.extend(self.take_outbox_msgs());
        }
        #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
        {
            if let Some(swapchain) = self.swapchain.as_ref() {
                let shared = shared_swapchain_from_host_swapchain(swapchain, cx);
                outbound.push(StudioToApp::Swapchain(shared));
            }
        }
        outbound
    }

    pub fn set_presentable_draw(&mut self, cx: &mut Cx, presentable_draw: PresentableDraw) {
        if self.try_present_draw(cx, presentable_draw) {
            trace_host(&format!(
                "pd c{}",
                self.current_target.map(|t| t.client).unwrap_or(0)
            ));
            self.pending_draw = None;
            self.present_ok_count += 1;
            if self.present_ok_count == 1 {
                // Whatever frames come in first, they fade in quickly
                // rather than popping over the "starting…" panel.
                self.first_present_at = Some(crate::host::now());
            }
            self.bootstrap_pending = false;
            self.bootstrap_tick_count = 0;
        } else {
            self.pending_draw = Some(presentable_draw);
        }
    }

    pub fn set_run_target(
        &mut self,
        cx: &mut Cx,
        client: ClientId,
        window_id: usize,
        _hub_port: u16,
    ) {
        self.set_target(cx, Some(RunTarget { client, window_id }));
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        self.setup_aux_chan(cx, _hub_port, client);
    }

    /// CreateWindow arrived: the child's stdin loop is live, share the
    /// swapchain from now on.
    pub fn app_ready(&mut self, cx: &mut Cx, client: ClientId, window_id: usize) {
        let target = RunTarget { client, window_id };
        if self.current_target != Some(target) {
            self.set_target(cx, Some(target));
        }
        self.app_ready_for_swapchain = true;
        self.tick_outstanding = None;
        self.tick_deferred = false;
        self.present_ok_count = 0;
        self.first_present_at = None;
        self.bootstrap_pending = true;
        self.bootstrap_tick_count = 0;
        self.redraw_countdown = self.redraw_countdown.max(240);
        self.redraw(cx);
    }

    pub fn clear_run_target(&mut self, cx: &mut Cx) {
        self.set_target(cx, None);
    }

    pub fn client(&self) -> Option<ClientId> {
        self.current_target.map(|t| t.client)
    }

    /// A hosted window may freeze only after bootstrap has produced a frame.
    /// Freezing earlier would suppress the geometry/bootstrap it still needs.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    pub fn gpu_window_id(&self) -> Option<usize> {
        (self.app_ready_for_swapchain
            && self.present_ok_count > 0
            && self.outbox.is_some()
            && self.gpu_error().is_none())
            .then(|| self.current_target.map(|t| t.window_id))
            .flatten()
    }

    /// Freeze ordinary Tick/bootstrap and swapchain resize. Last rect, DPI,
    /// and displayed texture stay. In-flight descriptor metadata still
    /// drains through the outbox.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    pub fn gpu_freeze(&mut self, id: u64) {
        match self.frozen {
            Some(frozen) if frozen == id => {}
            Some(_) => {
                self.transport_error = Some("GPU freeze id mismatch".into());
            }
            None => {
                self.frozen = Some(id);
            }
        }
    }

    /// Export a candidate swapchain from the prepared renderer. `Ok(false)`
    /// means accept or a previous export is not ready yet; retry. Errors do
    /// not drop the active swapchain.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    pub fn gpu_prepare(&mut self, cx: &mut Cx, id: u64, epoch: u64) -> Result<bool, String> {
        if self.frozen != Some(id) {
            return Err("GPU prepare without matching freeze".into());
        }
        if let Some(transition) = &self.gpu_transition {
            if transition.id != id || transition.epoch != epoch {
                return Err("GPU prepare transport epoch mismatch".into());
            }
            if transition.queued { return Ok(true); }
        }
        self.poll_aux_chan(cx);
        if self.outbox.is_none() || self.export_busy() {
            return Ok(false);
        }
        if !self.app_ready_for_swapchain {
            return Ok(false);
        }
        if self.last_rect.size.x <= 0.0
            || self.last_rect.size.y <= 0.0
            || !(self.last_dpi_factor.is_finite() && self.last_dpi_factor > 0.0)
        {
            return Ok(false);
        }
        let Some(target) = self.current_target else {
            return Err("GPU prepare has no client window".into());
        };
        let alloc_width = ((self.last_rect.size.x * self.last_dpi_factor).ceil() as u32).max(1);
        let alloc_height = ((self.last_rect.size.y * self.last_dpi_factor).ceil() as u32).max(1);
        let mut candidate = HostSwapchain::new(target.window_id, alloc_width, alloc_height, cx);
        match export_host_swapchain(&mut candidate, cx) {
            Ok(batch) => {
                self.queue_exported(batch, Some((id, epoch)))?;
                self.gpu_transition = Some(GpuTransition {
                    id,
                    epoch,
                    candidate: Some(candidate),
                    queued: true,
                });
                Ok(true)
            }
            Err(err) => {
                let error = format!("{err:?}");
                self.transport_error = Some(error.clone());
                Err(error)
            }
        }
    }

    /// Install the queued candidate as the active swapchain. Stay frozen
    /// until `gpu_retire`. The last displayed texture is left on `draw_app`.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    pub fn gpu_commit(&mut self, cx: &mut Cx, id: u64) -> Result<(), String> {
        let Some(transition) = self.gpu_transition.as_mut() else {
            return Err("GPU commit without prepared candidate".into());
        };
        if transition.id != id || !transition.queued {
            return Err("GPU commit requires a queued candidate".into());
        }
        if self.frozen != Some(id) {
            return Err("GPU commit without matching freeze".into());
        }
        let candidate = transition
            .candidate
            .take()
            .ok_or_else(|| "GPU commit missing candidate swapchain".to_string())?;
        self.retired_swapchain = self.swapchain.take();
        self.swapchain = Some(candidate);
        self.last_swapchain_with_completed_draws = None;
        self.pending_draw = None;
        // Preserve the displayed old frame until the first new-generation
        // present, without showing startup chrome or replaying its fade.
        self.bootstrap_pending = false;
        self.bootstrap_tick_count = 0;
        self.redraw(cx);
        Ok(())
    }

    /// Drop the retired generation and unfreeze. Ordinary Tick resumes.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    pub fn gpu_retire(&mut self, cx: &mut Cx, id: u64) {
        let matches = self.frozen == Some(id)
            || self.gpu_transition.as_ref().is_some_and(|t| t.id == id);
        if !matches {
            self.transport_error = Some("GPU retire id mismatch".into());
            return;
        }
        self.retired_swapchain = None;
        self.gpu_transition = None;
        self.frozen = None;
        self.redraw(cx);
    }

    /// Drop the candidate, unfreeze the old active swapchain, and keep
    /// draining any in-flight descriptor metadata. Terminal transport
    /// errors are not retried as a reconnect.
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    pub fn gpu_cancel(&mut self, cx: &mut Cx, id: u64) {
        let matches = self.frozen == Some(id)
            || self.gpu_transition.as_ref().is_some_and(|t| t.id == id);
        if !matches {
            self.transport_error = Some("GPU cancel id mismatch".into());
            return;
        }
        self.gpu_transition = None;
        self.frozen = None;
        self.redraw(cx);
    }

    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    pub fn gpu_error(&self) -> Option<&str> {
        self.transport_error
            .as_deref()
            .or_else(|| self.outbox.as_ref().and_then(|sender| sender.error()))
    }

    pub fn set_target_size(&mut self, size: Option<Vec2d>) {
        self.target_size = size;
    }

    /// Focus the compositor keyboard on this tile.
    /// Claim the compositor's key focus for this tile. False while the
    /// tile has no live Area — never drawn, or not drawn since its draw
    /// list was rebuilt (the AI pane hides its run view entirely between
    /// showings) — because focusing a dead area is a no-op; the caller
    /// keeps such a focus PENDING and retries after the next draw.
    pub fn focus_keyboard(&mut self, cx: &mut Cx) -> bool {
        // A Quick-Look panel declines the keyboard outright. True, not
        // false: there is nothing to retry later, the tile simply never
        // wants it (the FOCUS RULE).
        if !self.takes_key_focus {
            return true;
        }
        if !self.area.is_valid(cx) {
            return false;
        }
        cx.set_key_focus(self.area);
        true
    }

    /// Give the keyboard back if this tile holds it — the AI pane hiding
    /// must not leave its child as the invisible key target.
    pub fn release_keyboard(&mut self, cx: &mut Cx) {
        if self.area != Area::Empty && cx.has_key_focus(self.area) {
            cx.set_key_focus(Area::Empty);
        }
    }

    /// FOCUS RULE: mark this tile a Quick-Look panel — mouse yes, keyboard
    /// never. Set once when the WM floats a preview viewer.
    pub fn set_takes_key_focus(&mut self, on: bool) {
        self.takes_key_focus = on;
    }

    /// The newest line the child (or the cargo wrapper building it) wrote.
    /// Shown under "starting…" until the first frame arrives.
    /// The desk sets this every frame while the tile closes: `origin` and
    /// `span` place the shrinking quad inside the tile's original rect
    /// (both normalized), pinning the frozen frame in screen space.
    pub fn set_close_crop(&mut self, crop: Option<(Vec2d, Vec2d)>) {
        self.close_crop = crop;
    }

    /// The desk's popin fade for this frame (content fades WITH the ring).
    pub fn set_fade(&mut self, fade: f32) {
        self.fade = fade;
    }

    /// How far the arrival fade-in has come (0 = first frame just landed,
    /// 1 = fully shown; also 1 before any frame). The desk uses the
    /// complement on its dark starting wash so the crossfade keeps the
    /// tile's darkness continuous — no bright flash between the wash
    /// vanishing and the content appearing.
    pub fn arrival_fade(&self) -> f32 {
        match self.first_present_at {
            Some(t0) => {
                let t = ((crate::host::now() - t0) as f32 / ARRIVAL_FADE_SECS).min(1.0);
                t * t * (3.0 - 2.0 * t)
            }
            None => 1.0,
        }
    }

    pub fn set_status_line(&mut self, cx: &mut Cx, line: &str) {
        if self.status_line == line {
            return;
        }
        self.status_line = line.to_string();
        self.redraw(cx);
    }

    pub fn set_startup_style(&mut self, cx: &mut Cx, sheet: &desktop_style::StyleSheet) {
        self.startup_glass = sheet.name.starts_with("macos");
        self.startup_glass_applied = None;
        if let Some(mut splash) = self.no_fb_view.borrow_mut::<Splash>() {
            splash.set_stylesheet(cx, sheet.clone());
        }
    }

    pub fn set_startup_app(&mut self, app: &str) {
        if self.startup_app != app {
            self.startup_app.clear();
            self.startup_app.push_str(app);
        }
    }

    /// Keep the tail — that is where the crate name is.
    fn trimmed_status(&self, width: f64) -> String {
        // The code font at 9pt is about 7 logical px per character.
        let max = ((width - 24.0) / 7.0).floor().max(8.0) as usize;
        let chars: Vec<char> = self.status_line.chars().collect();
        if chars.len() <= max {
            return self.status_line.clone();
        }
        let tail: String = chars[chars.len() - (max - 1)..].iter().collect();
        format!("\u{2026}{}", tail)
    }

    pub fn has_frame(&self) -> bool {
        self.present_ok_count > 0
    }

    fn local_from_area(&self, cx: &Cx, abs: Vec2d) -> Option<Vec2d> {
        if !self.area.is_valid(cx) {
            return None;
        }
        let rect = self.area.rect(cx);
        Some(dvec2(abs.x - rect.pos.x, abs.y - rect.pos.y))
    }

    fn default_mouse_button(device: &DigitDevice) -> MouseButton {
        device.mouse_button().unwrap_or(MouseButton::PRIMARY)
    }

    fn host_dpi_factor(cx: &Cx2d) -> f64 {
        cx.get_current_window_id()
            .map(|window_id| cx.windows[window_id].window_geom.dpi_factor)
            .filter(|dpi_factor| dpi_factor.is_finite() && *dpi_factor > 0.0)
            .unwrap_or_else(|| cx.current_dpi_factor())
    }
}

impl Widget for MpRunView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let dpi_factor = Self::host_dpi_factor(cx);
        let rect = cx.walk_turtle(walk).dpi_snap(1.0 / dpi_factor);
        // Only the "starting…" state gets a backdrop; a presented frame is
        // composited straight over the wallpaper so translucent children
        // (Omarchy's 0.985/0.96 window opacity) show it through.
        if self.present_ok_count == 0 {
            self.draw_bg.draw_abs(cx, rect);
        }

        let target = self.current_target;
        if let Some(target) = target {
            let config_rect = Rect {
                pos: rect.pos,
                size: self.target_size.unwrap_or(rect.size),
            };
            self.ensure_swapchain_for_rect(cx, config_rect, dpi_factor, target);
            if let Some(presentable_draw) = self.pending_draw {
                self.set_presentable_draw(cx, presentable_draw);
            }
        }

        // A cargo build has no protocol target yet. Its launch panel must
        // still be visible before the application connects.
        let waiting_for_framebuffer = self.present_ok_count == 0;
        if waiting_for_framebuffer {
            self.redraw(cx);
        } else if self.redraw_countdown > 0 {
            self.redraw_countdown -= 1;
            self.redraw(cx);
        }

        if self.present_ok_count > 0 {
            trace_host(&format!(
                "paint c{}",
                target.map(|t| t.client).unwrap_or(0)
            ));
        }
        self.draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(host_dpi_factor), &[dpi_factor as f32]);
        let (crop_origin, crop_span) = self
            .close_crop
            .unwrap_or((dvec2(0.0, 0.0), dvec2(1.0, 1.0)));
        self.draw_app.draw_vars.set_dyn_instance(
            cx,
            id!(crop_origin),
            &[crop_origin.x as f32, crop_origin.y as f32],
        );
        self.draw_app.draw_vars.set_dyn_instance(
            cx,
            id!(crop_span),
            &[crop_span.x as f32, crop_span.y as f32],
        );
        // The arrival fade: ~130ms from the first presented frame, over
        // whatever frames come in, multiplied with the desk's popin fade.
        const FIRST_FADE: f32 = ARRIVAL_FADE_SECS;
        let first_fade = match self.first_present_at {
            Some(t0) => {
                let t = ((crate::host::now() - t0) as f32 / FIRST_FADE).min(1.0);
                if t < 1.0 {
                    self.redraw(cx);
                }
                // almostLinear-ish ease-out.
                t * t * (3.0 - 2.0 * t)
            }
            None => 1.0,
        };
        self.draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(fade), &[self.fade * first_fade]);
        if waiting_for_framebuffer || first_fade < 1.0 {
            if !self.startup_initialized {
                self.startup_initialized = true;
                self.no_fb_view.set_text(cx, include_str!("../resources/startup.splash"));
            }
            let headline = if self.status_line.starts_with("compiling ") {
                "Compiling…"
            } else if self.status_line.starts_with("waiting for another build") {
                "Waiting to compile…"
            } else if self.status_line.starts_with("build failed") {
                "Could not build application"
            } else { "Starting…" };
            self.no_fb_view.label(cx, ids!(placeholder)).set_text(cx, headline);
            let status = self.trimmed_status(rect.size.x);
            self.no_fb_view
                .label(cx, ids!(status_line))
                .set_text(cx, &status);
            if let Some(mut icon) = self.no_fb_view.widget(cx, ids!(startup_icon)).borrow_mut::<app_icon::AppIcon>() {
                icon.set_name(cx, if self.startup_app.is_empty() {"app"} else {&self.startup_app});
            }
            self.no_fb_view.draw_walk_all(cx, scope, Walk::abs_rect(rect));
            if self.startup_glass_applied != Some(self.startup_glass) {
                let surface = self.no_fb_view.widget(cx, ids!(startup_surface));
                if let Some(mut surface) = surface.borrow_mut::<View>() {
                    let opacity = if self.startup_glass {0.20f32} else {1.0};
                    // This view belongs to the Splash isolate. Change its draw
                    // instance directly; never apply main-heap script values.
                    surface.draw_bg.draw_vars.set_dyn_instance(cx, id!(opacity), &[opacity]);
                    surface.redraw(cx);
                    drop(surface);
                    self.startup_glass_applied = Some(self.startup_glass);
                    self.redraw(cx);
                };
            }
        }
        self.draw_app.draw_abs(cx, rect);
        self.area = self.draw_app.area();
        if target.is_some() && cx.has_key_focus(self.area) {
            let ime = self
                .ime_pos
                .unwrap_or_else(|| dvec2(rect.size.x * 0.5, rect.size.y * 0.5));
            // This anchors the native candidate window for a remote process.
            // It is not a text-input request from the WM or a module client.
            cx.push_unique_platform_op(CxOsOp::ShowTextIME(
                self.area, Rect{pos:ime,size:Vec2d::default()}, TextInputConfig::default(),
            ));
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let target = self.current_target;

        if let Event::Timer(timer_event) = event {
            if self.tick_timer.is_timer(timer_event).is_some() {
                if let Some(target) = target {
                    let mut msgs = Vec::new();
                    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                    {
                        self.poll_aux_chan(cx);
                        msgs.extend(self.take_outbox_msgs());
                    }
                    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                    let frozen = self.frozen.is_some();
                    #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
                    let frozen = false;
                    let should_bootstrap = !frozen && (self.present_ok_count == 0 || self.bootstrap_pending);
                    if should_bootstrap {
                        self.bootstrap_tick_count = self.bootstrap_tick_count.wrapping_add(1);
                        if self.bootstrap_tick_count == 1 || self.bootstrap_tick_count % 15 == 0 {
                            msgs.extend(self.build_bootstrap_msgs(cx, target));
                        }
                    }
                    if !frozen {
                        self.tick_deferred = true;
                        let now = crate::host::now();
                        let due = match self.tick_outstanding {
                            None => true,
                            Some(sent_at) if now - sent_at >= self.tick_fallback() => {
                                trace_host(&format!(
                                    "tick-fallback c{} {:.1}ms",
                                    target.client,
                                    (now - sent_at) * 1000.0
                                ));
                                true
                            }
                            Some(_) => false,
                        };
                        if due {
                            self.append_tick(target, &mut msgs);
                        }
                    }
                    self.emit_to_app(cx, target.client, msgs);
                }
            }
        }

        let Some(target) = target else {
            return;
        };

        match event.hits(cx, self.area) {
            Hit::KeyFocus(_) => {
                self.redraw(cx);
            }
            Hit::KeyFocusLost(_) => {
                cx.hide_text_ime();
                self.redraw(cx);
            }
            Hit::FingerDown(e) => {
                if let Some(local) = self.local_from_area(cx, e.abs) {
                    // FOCUS RULE: a Quick-Look panel is hit-tested and gets
                    // the press, but must not take the keyboard from the
                    // requester that is dialing through files.
                    if self.takes_key_focus {
                        cx.set_key_focus(self.area);
                        self.ime_pos = Some(local);
                    }
                    cx.widget_action(
                        self.uid,
                        MpRunViewAction::Clicked {
                            client: target.client,
                        },
                    );
                    self.redraw(cx);
                    self.emit_after_pending_move(
                        cx,
                        target.client,
                        StudioToApp::MouseDown(RemoteMouseDown {
                            button_raw_bits: Self::default_mouse_button(&e.device).bits(),
                            x: local.x,
                            y: local.y,
                            time: e.time,
                            modifiers: RemoteKeyModifiers::from_key_modifiers(&e.modifiers),
                        }),
                    );
                }
            }
            Hit::FingerMove(e) => {
                if let Some(local) = self.local_from_area(cx, e.abs) {
                    trace_host("mm");
                    // Held until the next Tick (or the next edge): one
                    // position per frame is all a frame can show.
                    self.pending_move = Some(RemoteMouseMove {
                        x: local.x,
                        y: local.y,
                        time: e.time,
                        modifiers: RemoteKeyModifiers::from_key_modifiers(&e.modifiers),
                    });
                }
            }
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                self.is_hovered = true;
                cx.set_cursor(self.remote_cursor);
                if let Some(local) = self.local_from_area(cx, e.abs) {
                    self.pending_move = Some(RemoteMouseMove {
                        x: local.x,
                        y: local.y,
                        time: e.time,
                        modifiers: RemoteKeyModifiers::from_key_modifiers(&e.modifiers),
                    });
                }
            }
            Hit::FingerHoverOut(_) => {
                self.is_hovered = false;
                cx.set_cursor(MouseCursor::Default);
            }
            Hit::FingerUp(e) => {
                if let Some(local) = self.local_from_area(cx, e.abs) {
                    self.emit_after_pending_move(
                        cx,
                        target.client,
                        StudioToApp::MouseUp(RemoteMouseUp {
                            button_raw_bits: Self::default_mouse_button(&e.device).bits(),
                            x: local.x,
                            y: local.y,
                            time: e.time,
                            modifiers: RemoteKeyModifiers::from_key_modifiers(&e.modifiers),
                        }),
                    );
                }
            }
            Hit::FingerScroll(e) => {
                if let Some(local) = self.local_from_area(cx, e.abs) {
                    self.emit_after_pending_move(
                        cx,
                        target.client,
                        StudioToApp::Scroll(RemoteScroll {
                            is_mouse: e.device.is_mouse(),
                            time: e.time,
                            x: local.x,
                            y: local.y,
                            sx: e.scroll.x,
                            sy: e.scroll.y,
                            modifiers: RemoteKeyModifiers::from_key_modifiers(&e.modifiers),
                        }),
                    );
                }
            }
            Hit::TextInput(e) => {
                self.emit_to_app(cx, target.client, vec![StudioToApp::TextInput(e)]);
            }
            Hit::KeyDown(e) => {
                self.emit_to_app(cx, target.client, vec![StudioToApp::KeyDown(e)]);
            }
            Hit::KeyUp(e) => {
                self.emit_to_app(cx, target.client, vec![StudioToApp::KeyUp(e)]);
            }
            Hit::TextCopy(_) => {
                self.emit_to_app(cx, target.client, vec![StudioToApp::TextCopy]);
            }
            Hit::TextCut(_) => {
                self.emit_to_app(cx, target.client, vec![StudioToApp::TextCut]);
            }
            _ => {}
        }
    }
}

impl crate::tile::TileHost for MpRunView {
    fn client(&self) -> Option<ClientId> {
        MpRunView::client(self)
    }

    fn set_status_line(&mut self, cx: &mut Cx, line: &str) {
        MpRunView::set_status_line(self, cx, line)
    }

    fn focus_keyboard(&mut self, cx: &mut Cx) -> bool {
        MpRunView::focus_keyboard(self, cx)
    }

    fn release_keyboard(&mut self, cx: &mut Cx) {
        MpRunView::release_keyboard(self, cx)
    }

    fn set_takes_key_focus(&mut self, on: bool) {
        MpRunView::set_takes_key_focus(self, on)
    }

    fn set_remote_cursor(&mut self, cx: &mut Cx, cursor: MouseCursor) {
        MpRunView::set_remote_cursor(self, cx, cursor)
    }

    fn has_frame(&self) -> bool {
        MpRunView::has_frame(self)
    }

    fn arrival_fade(&self) -> f32 {
        MpRunView::arrival_fade(self)
    }

    fn set_target_size(&mut self, size: Option<Vec2d>) {
        MpRunView::set_target_size(self, size)
    }

    fn set_close_crop(&mut self, crop: Option<(Vec2d, Vec2d)>) {
        MpRunView::set_close_crop(self, crop)
    }

    fn set_fade(&mut self, fade: f32) {
        MpRunView::set_fade(self, fade)
    }
}
