use crate::makepad_widgets::makepad_micro_serde::SerBin;
use crate::makepad_widgets::makepad_platform::shared_framebuf::HostSwapchain;
use crate::makepad_widgets::*;
use makepad_studio_protocol::hub_protocol::{QueryId, RunViewInputVizKind};
use makepad_studio_protocol::{
    MouseButton, PresentableDraw, RemoteKeyModifiers, RemoteMouseDown, RemoteMouseMove,
    RemoteMouseUp, RemoteScroll, RunViewFrameData, StudioToApp, StudioToAppVec,
};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
use crate::makepad_widgets::makepad_platform::shared_framebuf::aux_chan;
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
use std::sync::Mutex;

#[path = "desktop_run_view/swapchain.rs"]
pub mod swapchain;

#[path = "desktop_run_view/remote_decode.rs"]
pub mod remote_decode;

#[path = "desktop_run_view/input_viz.rs"]
pub mod input_viz;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DesktopRunViewBase = #(DesktopRunView::register_widget(vm))

    mod.widgets.DesktopRunView = set_type_default() do mod.widgets.DesktopRunViewBase {
        width: Fill
        height: Fill
        draw_bg +: {
            color: uniform(theme.color_bg_container)
            pixel: fn() {
                return self.color
            }
        }
        draw_app +: {
            tex: texture_2d(float)
            tex_scale: instance(vec2(0.0, 0.0))
            tex_size: instance(vec2(1.0, 1.0))
            host_dpi_factor: instance(1.0)
            y_flip: instance(0.0)
            packed_header: instance(1.0)
            pixel: fn() {
                let uv = vec2(self.pos.x, self.pos.y + self.y_flip - 2.0 * self.y_flip * self.pos.y)
                if self.packed_header < 0.5 {
                    return self.tex.sample(uv * self.tex_scale)
                }
                let tp1 = self.tex.sample(vec2(0.5 / self.tex_size.x, 0.5 / self.tex_size.y))
                let tp2 = self.tex.sample(vec2(1.5 / self.tex_size.x, 0.5 / self.tex_size.y))
                let tp = vec2(tp1.r * 65280.0 + tp1.b * 255.0, tp2.r * 65280.0 + tp2.b * 255.0)
                if tp.x <= 0.0 || tp.y <= 0.0 {
                    return #0000
                }
                let counter = (self.rect_size * self.host_dpi_factor) / tp
                let tex_scale = tp / self.tex_size
                let fb = self.tex.sample(uv * tex_scale * counter)
                if fb.r == 1.0 && fb.g == 0.0 && fb.b == 1.0 {
                    return #2
                }
                return fb
            }
        }
        draw_ai_viz +: {
            dot_radius: instance(5.0)
            dot_alpha: instance(0.0)
            ripple_radius: instance(5.0)
            ripple_alpha: instance(0.0)
            shape_kind: instance(0.0)
            corner_radius: instance(6.0)
            stroke_width: instance(1.5)
            color: instance(vec4(0.0, 0.831, 1.0, 1.0))
            pixel: fn() {
                if self.dot_alpha <= 0.001 && self.ripple_alpha <= 0.001 {
                    return vec4(0.0, 0.0, 0.0, 0.0)
                }
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                if self.shape_kind < 0.5 {
                    let c = self.rect_size * 0.5
                    let dot_r = self.dot_radius.min(self.rect_size.x * 0.5).min(self.rect_size.y * 0.5)
                    if self.dot_alpha > 0.001 {
                        sdf.circle(c.x, c.y, dot_r)
                        sdf.fill(vec4(self.color.xyz, self.dot_alpha))
                    }
                    if self.ripple_alpha > 0.001 {
                        let ripple_r = self.ripple_radius.min(self.rect_size.x * 0.5).min(self.rect_size.y * 0.5)
                        sdf.circle(c.x, c.y, ripple_r)
                        sdf.stroke(vec4(self.color.xyz, self.ripple_alpha), self.stroke_width)
                    }
                }
                else {
                    let inset = self.stroke_width.max(0.5)
                    let box_w = (self.rect_size.x - inset * 2.0).max(0.0)
                    let box_h = (self.rect_size.y - inset * 2.0).max(0.0)
                    let radius = self.corner_radius.min(box_w * 0.5).min(box_h * 0.5)
                    sdf.box(inset, inset, box_w, box_h, radius)
                    if self.dot_alpha > 0.001 {
                        sdf.fill(vec4(self.color.xyz, self.dot_alpha))
                    }
                    if self.ripple_alpha > 0.001 {
                        sdf.stroke(vec4(self.color.xyz, self.ripple_alpha), self.stroke_width)
                    }
                }
                return sdf.result
            }
        }
        no_fb_view: RectView {
            width: Fill
            height: Fill
            draw_bg +: {
                color: theme.color_bg_container
            }
            View {
                width: Fill
                height: Fill
                align: Align {x: 0.5 y: 0.5}
                placeholder := Label {
                    text: "no framebuffer"
                    draw_text.color: #xC3CCD8
                    draw_text.text_style.font_size: 13.0
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RunTarget {
    pub(crate) build_id: QueryId,
    pub(crate) window_id: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct InputVizEvent {
    pub(crate) kind: RunViewInputVizKind,
    pub(crate) pos: Vec2d,
    pub(crate) size: Option<Vec2d>,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingRemoteDecode {
    pub(crate) path: PathBuf,
    pub(crate) frame_id: u64,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Clone, Debug, Default)]
pub enum DesktopRunViewAction {
    ForwardToApp {
        build_id: QueryId,
        msg_bin: Vec<u8>,
    },
    #[default]
    None,
}

#[derive(Script, Widget)]
pub struct DesktopRunView {
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
    #[redraw]
    #[live]
    draw_ai_viz: DrawQuad,
    #[find]
    #[redraw]
    #[live]
    no_fb_view: WidgetRef,
    #[rust]
    area: Area,
    #[rust]
    tick_timer: Timer,
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
    last_trace_bootstrap: Option<(u64, usize, u64, u64, u64)>,
    #[rust]
    current_target: Option<RunTarget>,
    #[rust]
    swapchain: Option<HostSwapchain>,
    #[rust]
    last_swapchain_with_completed_draws: Option<HostSwapchain>,
    #[rust]
    pending_draw: Option<PresentableDraw>,
    #[rust]
    debug_present_ok_count: usize,
    #[rust]
    app_ready_for_swapchain: bool,
    #[rust]
    remote_cursor: MouseCursor,
    #[rust]
    is_hovered: bool,
    #[rust]
    ai_viz_kind: Option<RunViewInputVizKind>,
    #[rust]
    ai_viz_pos: Vec2d,
    #[rust]
    ai_viz_size: Option<Vec2d>,
    #[rust]
    ai_viz_frames_left: u8,
    #[rust]
    ai_viz_total_frames: u8,
    #[rust]
    ai_viz_queue: VecDeque<InputVizEvent>,
    #[rust]
    pending_focus_viz_queue: VecDeque<RunViewInputVizKind>,
    #[rust]
    awaiting_focus_rect: bool,
    #[rust]
    input_focus_rect: Option<Rect>,
    #[rust]
    ime_pos: Option<Vec2d>,
    #[rust]
    remote_mode: bool,
    #[rust]
    remote_frame_request_in_flight: bool,
    #[rust]
    remote_requested_frame_id: Option<u64>,
    #[rust]
    remote_next_frame_id: u64,
    #[rust]
    remote_current_frame_id: u64,
    #[rust]
    remote_current_path: Option<PathBuf>,
    #[rust]
    remote_pending_decode: Option<PendingRemoteDecode>,

    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    aux_chan_host_endpoint: Option<Arc<Mutex<Option<aux_chan::HostEndpoint>>>>,
}

impl ScriptHook for DesktopRunView {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            self.draw_app.set_texture(0, &cx.null_texture());
            self.tick_timer = cx.start_interval(0.008);
            self.draw_app
                .draw_vars
                .set_dyn_instance(cx, id!(packed_header), &[1.0f32]);
        });
    }
}

impl DesktopRunView {
    fn emit_to_app(&self, cx: &mut Cx, build_id: QueryId, msgs: Vec<StudioToApp>) {
        if msgs.is_empty() {
            return;
        }
        let msg_bin = StudioToAppVec(msgs).serialize_bin();
        cx.widget_action(
            self.uid,
            DesktopRunViewAction::ForwardToApp { build_id, msg_bin },
        );
    }

    fn set_target(&mut self, cx: &mut Cx, target: Option<RunTarget>) {
        if self.current_target == target {
            return;
        }
        let had_target = self.current_target.is_some();
        self.current_target = target;
        self.remote_cursor = MouseCursor::Default;
        self.is_hovered = false;
        self.swapchain = None;
        self.last_swapchain_with_completed_draws = None;
        self.pending_draw = None;
        self.debug_present_ok_count = 0;
        self.app_ready_for_swapchain = false;
        self.ai_viz_kind = None;
        self.ai_viz_frames_left = 0;
        self.ai_viz_total_frames = 0;
        self.ai_viz_queue.clear();
        self.ai_viz_size = None;
        self.pending_focus_viz_queue.clear();
        self.awaiting_focus_rect = false;
        self.input_focus_rect = None;
        self.ime_pos = None;
        self.remote_mode = false;
        self.remote_frame_request_in_flight = false;
        self.remote_requested_frame_id = None;
        self.remote_next_frame_id = 1;
        self.remote_current_frame_id = 0;
        self.remote_current_path = None;
        self.remote_pending_decode = None;
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        {
            self.aux_chan_host_endpoint = None;
        }
        self.last_rect = Rect::default();
        self.last_dpi_factor = 0.0;
        self.last_trace_bootstrap = None;
        self.bootstrap_pending = target.is_some();
        self.bootstrap_tick_count = 0;
        if target.is_some() {
            // Keep redrawing during startup so bootstrap messages can be resent
            // until the child app socket is ready.
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
        self.draw_ai_viz.redraw(cx);
        self.no_fb_view.redraw(cx);
    }

    fn set_remote_cursor(&mut self, cx: &mut Cx, cursor: MouseCursor) {
        self.remote_cursor = cursor;
        if self.is_hovered {
            cx.set_cursor(self.remote_cursor);
        }
    }

    pub fn set_run_target(
        &mut self,
        cx: &mut Cx,
        build_id: QueryId,
        window_id: Option<usize>,
        _studio_addr: Option<&str>,
    ) {
        // set_target must run before setup_aux_chan: it clears
        // aux_chan_host_endpoint when the target changes, so calling
        // setup_aux_chan first would create an endpoint that set_target
        // immediately destroys.
        self.set_target(
            cx,
            Some(RunTarget {
                build_id,
                // Bootstrap stdin-loop apps before they emit CreateWindow.
                // Main window id is 0 in the platform protocol.
                window_id: window_id.unwrap_or(0),
            }),
        );

        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        self.setup_aux_chan(_studio_addr, build_id);
    }

    pub fn rebootstrap_after_app_ready(
        &mut self,
        cx: &mut Cx,
        build_id: QueryId,
        window_id: usize,
    ) {
        let target = RunTarget {
            build_id,
            window_id,
        };
        if self.current_target != Some(target) {
            self.set_target(cx, Some(target));
            return;
        }
        // Re-send bootstrap against the current swapchain instead of reallocating.
        // This keeps shared-memory resources stable while still re-triggering
        // WindowGeomChange/Swapchain after app-side readiness.
        self.app_ready_for_swapchain = true;
        self.debug_present_ok_count = 0;
        self.bootstrap_pending = true;
        self.bootstrap_tick_count = 0;
        self.redraw_countdown = self.redraw_countdown.max(240);
        self.redraw(cx);
    }

    pub fn clear_run_target(&mut self, cx: &mut Cx) {
        self.set_target(cx, None);
    }

    fn local_from_area(&self, cx: &Cx, abs: Vec2d) -> Option<Vec2d> {
        if !self.area.is_valid(cx) {
            return None;
        }
        let rect = self.area.rect(cx);
        Some(dvec2(abs.x - rect.pos.x, abs.y - rect.pos.y))
    }

    fn default_ime_pos(rect: Rect) -> Vec2d {
        dvec2((rect.size.x * 0.5).max(0.0), (rect.size.y * 0.5).max(0.0))
    }

    fn clamped_ime_pos(&self, rect: Rect) -> Vec2d {
        let pos = self.ime_pos.unwrap_or_else(|| Self::default_ime_pos(rect));
        dvec2(
            pos.x.clamp(0.0, rect.size.x.max(0.0)),
            pos.y.clamp(0.0, rect.size.y.max(0.0)),
        )
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

impl Widget for DesktopRunView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let dpi_factor = Self::host_dpi_factor(cx);
        let rect = cx.walk_turtle(walk).dpi_snap(dpi_factor);
        self.draw_bg.draw_abs(cx, rect);

        let target = self.current_target;
        self.set_target(cx, target);

        if let Some(target) = target {
            self.ensure_swapchain_for_rect(cx, rect, dpi_factor, target);
            if let Some(presentable_draw) = self.pending_draw {
                if self.try_present_draw(cx, presentable_draw) {
                    self.pending_draw = None;
                }
            }
        }

        let waiting_for_framebuffer = target.is_some()
            && self.debug_present_ok_count == 0
            && self.remote_current_frame_id == 0
            && self.remote_pending_decode.is_none();
        if waiting_for_framebuffer {
            self.redraw(cx);
        } else if self.redraw_countdown > 0 {
            self.redraw_countdown -= 1;
            self.redraw(cx);
        }

        self.draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(host_dpi_factor), &[dpi_factor as f32]);
        self.draw_app.draw_abs(cx, rect);

        if let Some(kind) = self.ai_viz_kind {
            if self.ai_viz_frames_left > 0 {
                let total = self.ai_viz_total_frames.max(1) as f32;
                let frames_left = self.ai_viz_frames_left as f32;
                let t = 1.0f32 - (frames_left / total);
                let (
                    color,
                    dot_radius,
                    dot_alpha,
                    ripple_radius,
                    ripple_alpha,
                    shape_kind,
                    corner_radius,
                    stroke_width,
                    viz_rect,
                ) = if let Some(size) = self.ai_viz_size {
                    let pad = 2.0 + 4.0 * t as f64;
                    let fill_alpha = match kind {
                        RunViewInputVizKind::TypeText => 0.10f32 * (1.0f32 - t),
                        RunViewInputVizKind::Return => 0.12f32 * (1.0f32 - t),
                        _ => 0.0,
                    };
                    let outline_alpha = match kind {
                        RunViewInputVizKind::TypeText => 0.70f32 * (1.0f32 - t),
                        RunViewInputVizKind::Return => 0.80f32 * (1.0f32 - t),
                        _ => 0.0,
                    };
                    let color = match kind {
                        RunViewInputVizKind::TypeText => [1.00, 0.78, 0.24, 1.0],
                        RunViewInputVizKind::Return => [0.36, 0.90, 0.50, 1.0],
                        _ => [0.00, 0.83, 1.00, 1.0],
                    };
                    (
                        color,
                        0.0f32,
                        fill_alpha,
                        0.0f32,
                        outline_alpha,
                        1.0f32,
                        6.0f32,
                        2.0f32,
                        Rect {
                            pos: dvec2(
                                rect.pos.x + self.ai_viz_pos.x - pad,
                                rect.pos.y + self.ai_viz_pos.y - pad,
                            ),
                            size: dvec2(size.x + pad * 2.0, size.y + pad * 2.0),
                        },
                    )
                } else {
                    let (color, dot_radius, dot_alpha, ripple_radius, ripple_alpha) = match kind {
                        RunViewInputVizKind::ClickDown => {
                            ([0.00, 0.83, 1.00, 1.0], 5.0f32, 0.95f32, 5.0f32, 0.45f32)
                        }
                        RunViewInputVizKind::ClickUp => (
                            [0.00, 0.83, 1.00, 1.0],
                            5.0f32,
                            0.95f32 * (1.0f32 - t),
                            5.0f32 + 17.0f32 * t,
                            0.45f32 * (1.0f32 - t),
                        ),
                        RunViewInputVizKind::TypeText => {
                            ([1.00, 0.78, 0.24, 1.0], 0.0, 0.0, 0.0, 0.0)
                        }
                        RunViewInputVizKind::Return => ([0.36, 0.90, 0.50, 1.0], 0.0, 0.0, 0.0, 0.0),
                    };
                    (
                        color,
                        dot_radius,
                        dot_alpha,
                        ripple_radius,
                        ripple_alpha,
                        0.0f32,
                        0.0f32,
                        1.5f32,
                        Rect {
                            pos: rect.pos + self.ai_viz_pos - dvec2(28.0, 28.0),
                            size: dvec2(56.0, 56.0),
                        },
                    )
                };
                self.draw_ai_viz
                    .draw_vars
                    .set_dyn_instance(cx, id!(dot_radius), &[dot_radius]);
                self.draw_ai_viz
                    .draw_vars
                    .set_dyn_instance(cx, id!(dot_alpha), &[dot_alpha]);
                self.draw_ai_viz
                    .draw_vars
                    .set_dyn_instance(cx, id!(ripple_radius), &[ripple_radius]);
                self.draw_ai_viz
                    .draw_vars
                    .set_dyn_instance(cx, id!(ripple_alpha), &[ripple_alpha]);
                self.draw_ai_viz
                    .draw_vars
                    .set_dyn_instance(cx, id!(shape_kind), &[shape_kind]);
                self.draw_ai_viz
                    .draw_vars
                    .set_dyn_instance(cx, id!(corner_radius), &[corner_radius]);
                self.draw_ai_viz
                    .draw_vars
                    .set_dyn_instance(cx, id!(stroke_width), &[stroke_width]);
                self.draw_ai_viz
                    .draw_vars
                    .set_dyn_instance(cx, id!(color), &color);
                self.draw_ai_viz.draw_abs(cx, viz_rect);
            }
        }

        if waiting_for_framebuffer {
            self.no_fb_view.draw_all(cx, scope);
        }

        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::Timer(te) = event {
            if self.tick_timer.is_timer(te).is_some() {
                let mut outbound = Vec::new();
                if self.bootstrap_pending {
                    self.bootstrap_tick_count = self.bootstrap_tick_count.wrapping_add(1);
                    if self.bootstrap_tick_count % 30 == 1 {
                        if let Some(target) = self.current_target {
                            outbound.extend(self.build_bootstrap_msgs(cx, target));
                            if self.bootstrap_tick_count % 120 == 1 {
                                if let Some(msg) = self.request_remote_frame_if_needed(target) {
                                    outbound.push(msg);
                                }
                            }
                        }
                    }
                }

                let mut advance_queue = false;
                if self.ai_viz_kind.is_some() {
                    if self.ai_viz_frames_left > 0 {
                        self.ai_viz_frames_left -= 1;
                        self.redraw(cx);
                    } else {
                        advance_queue = true;
                    }
                }
                if advance_queue {
                    if let Some(event) = self.ai_viz_queue.pop_front() {
                        self.start_input_viz(event);
                        self.redraw(cx);
                    } else {
                        self.ai_viz_kind = None;
                        self.ai_viz_frames_left = 0;
                        self.ai_viz_total_frames = 0;
                    }
                }

                if let Some(target) = self.current_target {
                    self.emit_to_app(cx, target.build_id, outbound);
                }
            }
        }

        if let Event::Actions(actions) = event {
            self.handle_remote_decode_actions(cx, actions);
        }

        let Some(target) = self.current_target else {
            return;
        };

        let mut outbound = Vec::new();
        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) => {
                cx.set_key_focus(self.area);
                cx.show_text_ime(self.area, self.clamped_ime_pos(fe.rect));
                self.is_hovered = true;
                cx.set_cursor(self.remote_cursor);
                if let Some(local_pos) = self.local_from_area(cx, fe.abs) {
                    outbound.push(StudioToApp::MouseDown(RemoteMouseDown {
                        button_raw_bits: Self::default_mouse_button(&fe.device).bits(),
                        x: local_pos.x,
                        y: local_pos.y,
                        time: fe.time,
                        modifiers: RemoteKeyModifiers::from_key_modifiers(&fe.modifiers),
                    }));
                }
            }
            Hit::FingerMove(fe) => {
                self.is_hovered = true;
                cx.set_cursor(self.remote_cursor);
                if let Some(local_pos) = self.local_from_area(cx, fe.abs) {
                    outbound.push(StudioToApp::MouseMove(RemoteMouseMove {
                        x: local_pos.x,
                        y: local_pos.y,
                        time: fe.time,
                        modifiers: RemoteKeyModifiers::from_key_modifiers(&fe.modifiers),
                    }));
                }
            }
            Hit::FingerUp(fe) => {
                if let Some(local_pos) = self.local_from_area(cx, fe.abs) {
                    outbound.push(StudioToApp::MouseUp(RemoteMouseUp {
                        button_raw_bits: Self::default_mouse_button(&fe.device).bits(),
                        x: local_pos.x,
                        y: local_pos.y,
                        time: fe.time,
                        modifiers: RemoteKeyModifiers::from_key_modifiers(&fe.modifiers),
                    }));
                }
            }
            Hit::FingerScroll(fe) => {
                if let Some(local_pos) = self.local_from_area(cx, fe.abs) {
                    outbound.push(StudioToApp::Scroll(RemoteScroll {
                        x: local_pos.x,
                        y: local_pos.y,
                        sx: fe.scroll.x,
                        sy: fe.scroll.y,
                        is_mouse: fe.device.is_mouse(),
                        time: fe.time,
                        modifiers: RemoteKeyModifiers::from_key_modifiers(&fe.modifiers),
                    }));
                }
            }
            Hit::FingerHoverIn(_) => {
                self.is_hovered = true;
                cx.set_cursor(self.remote_cursor);
            }
            Hit::FingerHoverOut(_) => {
                self.is_hovered = false;
                cx.set_cursor(MouseCursor::Default);
            }
            Hit::KeyDown(ke) => {
                outbound.push(StudioToApp::KeyDown(ke.clone()));
            }
            Hit::KeyUp(ke) => {
                outbound.push(StudioToApp::KeyUp(ke.clone()));
            }
            Hit::TextInput(te) => {
                outbound.push(StudioToApp::TextInput(te.clone()));
            }
            Hit::TextCopy(_) => {
                outbound.push(StudioToApp::TextCopy);
            }
            Hit::TextCut(_) => {
                outbound.push(StudioToApp::TextCut);
            }
            Hit::KeyFocusLost(_) => {
                cx.hide_text_ime();
                outbound.push(StudioToApp::Kill);
            }
            _ => (),
        }

        if !outbound.is_empty() {
            self.emit_to_app(cx, target.build_id, outbound);
        }
    }
}

impl DesktopRunViewRef {
    pub fn set_run_target(
        &self,
        cx: &mut Cx,
        build_id: QueryId,
        window_id: Option<usize>,
        studio_addr: Option<&str>,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_run_target(cx, build_id, window_id, studio_addr);
        }
    }

    pub fn clear_run_target(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear_run_target(cx);
        }
    }

    pub fn rebootstrap_after_app_ready(
        &self,
        cx: &mut Cx,
        build_id: QueryId,
        window_id: usize,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.rebootstrap_after_app_ready(cx, build_id, window_id);
        }
    }

    pub fn set_presentable_draw(&self, cx: &mut Cx, presentable_draw: PresentableDraw) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_presentable_draw(cx, presentable_draw);
        }
    }

    pub fn set_remote_frame(
        &self,
        cx: &mut Cx,
        build_id: QueryId,
        frame: RunViewFrameData,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_remote_frame(cx, build_id, frame);
        }
    }

    pub fn set_remote_cursor(&self, cx: &mut Cx, cursor: MouseCursor) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_remote_cursor(cx, cursor);
        }
    }

    pub fn show_input_viz(
        &self,
        cx: &mut Cx,
        kind: RunViewInputVizKind,
        x: Option<f64>,
        y: Option<f64>,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show_input_viz(cx, kind, x, y);
        }
    }

    pub fn set_input_focus_rect(
        &self,
        cx: &mut Cx,
        x: Option<f64>,
        y: Option<f64>,
        width: Option<f64>,
        height: Option<f64>,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_input_focus_rect(cx, x, y, width, height);
        }
    }
}

