use crate::makepad_widgets::makepad_micro_serde::SerBin;
use crate::makepad_widgets::makepad_platform::shared_framebuf::{
    shared_swapchain_from_host_swapchain, HostSwapchain,
};
use crate::makepad_widgets::*;
use makepad_studio_protocol::hub_protocol::{QueryId, RunViewInputVizKind};
use makepad_studio_protocol::{
    MouseButton, PresentableDraw, RemoteKeyModifiers, RemoteMouseDown, RemoteMouseMove,
    RemoteMouseUp, RemoteScroll, StudioToApp, StudioToAppVec,
};
use std::collections::VecDeque;

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
            y_flip: instance(0.0)
            pixel: fn() {
                let tp1 = self.tex.sample(vec2(0.5 / self.tex_size.x, 0.5 / self.tex_size.y))
                let tp2 = self.tex.sample(vec2(1.5 / self.tex_size.x, 0.5 / self.tex_size.y))
                let tp = vec2(tp1.r * 65280.0 + tp1.b * 255.0, tp2.r * 65280.0 + tp2.b * 255.0)
                if tp.x <= 0.0 || tp.y <= 0.0 {
                    return #0000
                }
                let counter = (self.rect_size * self.draw_pass.dpi_factor) / tp
                let tex_scale = tp / self.tex_size
                let uv = vec2(self.pos.x, self.pos.y + self.y_flip - 2.0 * self.y_flip * self.pos.y)
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
            color: instance(vec4(0.0, 0.831, 1.0, 1.0))
            pixel: fn() {
                if self.dot_alpha <= 0.001 && self.ripple_alpha <= 0.001 {
                    return vec4(0.0, 0.0, 0.0, 0.0)
                }
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let c = self.rect_size * 0.5
                let dot_r = self.dot_radius.min(self.rect_size.x * 0.5).min(self.rect_size.y * 0.5)
                if self.dot_alpha > 0.001 {
                    sdf.circle(c.x, c.y, dot_r)
                    sdf.fill(vec4(self.color.xyz, self.dot_alpha))
                }
                if self.ripple_alpha > 0.001 {
                    let ripple_r = self.ripple_radius.min(self.rect_size.x * 0.5).min(self.rect_size.y * 0.5)
                    sdf.circle(c.x, c.y, ripple_r)
                    sdf.stroke(vec4(self.color.xyz, self.ripple_alpha), 1.5)
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
struct RunTarget {
    build_id: QueryId,
    window_id: usize,
}

#[derive(Clone, Copy, Debug)]
struct InputVizEvent {
    kind: RunViewInputVizKind,
    pos: Vec2d,
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
    redraw_countdown: usize,
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
    remote_cursor: MouseCursor,
    #[rust]
    is_hovered: bool,
    #[rust]
    ai_viz_kind: Option<RunViewInputVizKind>,
    #[rust]
    ai_viz_pos: Vec2d,
    #[rust]
    ai_viz_frames_left: u8,
    #[rust]
    ai_viz_total_frames: u8,
    #[rust]
    ai_viz_queue: VecDeque<InputVizEvent>,
}

impl ScriptHook for DesktopRunView {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            self.draw_app.set_texture(0, &cx.null_texture());
            self.tick_timer = cx.start_interval(0.008);
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
        self.current_target = target;
        self.remote_cursor = MouseCursor::Default;
        self.is_hovered = false;
        self.swapchain = None;
        self.last_swapchain_with_completed_draws = None;
        self.pending_draw = None;
        self.debug_present_ok_count = 0;
        self.ai_viz_kind = None;
        self.ai_viz_frames_left = 0;
        self.ai_viz_total_frames = 0;
        self.ai_viz_queue.clear();
        self.last_rect = Rect::default();
        if target.is_some() {
            // Keep redrawing during startup so bootstrap messages can be resent
            // until the child app socket is ready.
            self.redraw_countdown = self.redraw_countdown.max(240);
        } else {
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

    fn apply_presentable_draw_to_quad(
        cx: &mut Cx,
        draw_app: &mut DrawQuad,
        redraw_countdown: &mut usize,
        presentable_draw: PresentableDraw,
        swapchain: &HostSwapchain,
    ) -> bool {
        // Ignore zero-sized frames from early startup races (before geom is applied).
        // Treating these as "presented" can stall bootstrap until a manual resize.
        if presentable_draw.width == 0 || presentable_draw.height == 0 {
            return false;
        }

        let Some(drawn) = swapchain.get_image(presentable_draw.target_id) else {
            return false;
        };

        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        if let Some(buffer) = drawn.software_buffer.as_ref() {
            cx.upload_presentable_image_software_buffer(
                &drawn.texture,
                swapchain.alloc_width,
                swapchain.alloc_height,
                buffer.as_bytes(),
            );
        }

        draw_app.set_texture(0, &drawn.texture);
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
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(y_flip), &[1.0f32]);
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

    fn ensure_swapchain_for_rect(
        &mut self,
        cx: &mut Cx,
        rect: Rect,
        dpi_factor: f64,
        target: RunTarget,
    ) {
        if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
            return;
        }

        let force_bootstrap = self.debug_present_ok_count == 0;
        let mut outbound = Vec::new();
        if self.last_rect != rect || force_bootstrap {
            outbound.push(StudioToApp::WindowGeomChange {
                window_id: target.window_id,
                dpi_factor,
                left: 0.0,
                top: 0.0,
                width: rect.size.x,
                height: rect.size.y,
            });
        }

        let min_width = ((rect.size.x * dpi_factor).ceil() as u32).max(1);
        let min_height = ((rect.size.y * dpi_factor).ceil() as u32).max(1);
        let needs_new_swapchain = self
            .swapchain
            .as_ref()
            .map(|swapchain| {
                #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                {
                    min_width != swapchain.alloc_width || min_height != swapchain.alloc_height
                }
                #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
                {
                    min_width > swapchain.alloc_width || min_height > swapchain.alloc_height
                }
            })
            .unwrap_or(true);

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

        #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
        if (needs_new_swapchain || force_bootstrap) && self.swapchain.is_some() {
            if let Some(swapchain) = self.swapchain.as_ref() {
                let shared_swapchain = shared_swapchain_from_host_swapchain(swapchain, cx);
                outbound.push(StudioToApp::Swapchain(shared_swapchain));
            }
        }

        self.last_rect = rect;
        if !outbound.is_empty() {
            self.emit_to_app(cx, target.build_id, outbound);
        }
    }

    pub fn set_presentable_draw(&mut self, cx: &mut Cx, presentable_draw: PresentableDraw) {
        if self.try_present_draw(cx, presentable_draw) {
            self.pending_draw = None;
            self.debug_present_ok_count += 1;
        } else {
            self.pending_draw = Some(presentable_draw);
        }
    }

    pub fn set_run_target(&mut self, cx: &mut Cx, build_id: QueryId, window_id: Option<usize>) {
        self.set_target(
            cx,
            Some(RunTarget {
                build_id,
                // Bootstrap stdin-loop apps before they emit CreateWindow.
                // Main window id is 0 in the platform protocol.
                window_id: window_id.unwrap_or(0),
            }),
        );
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
        self.last_rect = Rect::default();
        self.debug_present_ok_count = 0;
        self.redraw_countdown = self.redraw_countdown.max(240);
        self.redraw(cx);
    }

    pub fn clear_run_target(&mut self, cx: &mut Cx) {
        self.set_target(cx, None);
    }

    pub fn show_input_viz(
        &mut self,
        cx: &mut Cx,
        kind: RunViewInputVizKind,
        x: Option<f64>,
        y: Option<f64>,
    ) {
        let has_target_size = self.last_rect.size.x > 0.0 && self.last_rect.size.y > 0.0;
        let local_pos = match (x, y) {
            (Some(x), Some(y)) => dvec2(x, y),
            _ if has_target_size => dvec2(self.last_rect.size.x * 0.5, self.last_rect.size.y * 0.5),
            _ => self.ai_viz_pos,
        };
        let local_pos = dvec2(
            local_pos.x.clamp(0.0, self.last_rect.size.x.max(0.0)),
            local_pos.y.clamp(0.0, self.last_rect.size.y.max(0.0)),
        );
        let event = InputVizEvent {
            kind,
            pos: local_pos,
        };
        if self.ai_viz_kind.is_some() {
            self.ai_viz_queue.push_back(event);
        } else {
            self.start_input_viz(event);
        }
        self.redraw(cx);
    }

    fn start_input_viz(&mut self, event: InputVizEvent) {
        let total_frames = match event.kind {
            // Old studio model: quick down pulse, then longer up ripple.
            RunViewInputVizKind::ClickDown => 4,
            RunViewInputVizKind::ClickUp => 30,
            RunViewInputVizKind::TypeText => 16,
            RunViewInputVizKind::Return => 20,
        };
        self.ai_viz_kind = Some(event.kind);
        self.ai_viz_pos = event.pos;
        self.ai_viz_frames_left = total_frames;
        self.ai_viz_total_frames = total_frames;
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
}

impl Widget for DesktopRunView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let dpi_factor = cx.current_dpi_factor();
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

        let waiting_for_framebuffer = target.is_some() && self.debug_present_ok_count == 0;
        if waiting_for_framebuffer {
            self.redraw(cx);
        } else if self.redraw_countdown > 0 {
            self.redraw_countdown -= 1;
            self.redraw(cx);
        }

        self.draw_app.draw_abs(cx, rect);

        if let Some(kind) = self.ai_viz_kind {
            if self.ai_viz_frames_left > 0 {
                let total = self.ai_viz_total_frames.max(1) as f32;
                let frames_left = self.ai_viz_frames_left as f32;
                let t = 1.0f32 - (frames_left / total);
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
                    RunViewInputVizKind::TypeText => (
                        [1.00, 0.78, 0.24, 1.0],
                        4.0f32,
                        0.70f32 * (1.0f32 - t),
                        8.0f32 + 16.0f32 * t,
                        0.55f32 * (1.0f32 - t),
                    ),
                    RunViewInputVizKind::Return => (
                        [0.36, 0.90, 0.50, 1.0],
                        4.0f32,
                        0.80f32 * (1.0f32 - t),
                        8.0f32 + 18.0f32 * t,
                        0.58f32 * (1.0f32 - t),
                    ),
                };
                self.draw_ai_viz
                    .draw_vars
                    .set_dyn_instance(cx, id!(dot_radius), &[dot_radius]);
                self.draw_ai_viz.draw_vars.set_dyn_instance(
                    cx,
                    id!(dot_alpha),
                    &[dot_alpha.max(0.0)],
                );
                self.draw_ai_viz.draw_vars.set_dyn_instance(
                    cx,
                    id!(ripple_radius),
                    &[ripple_radius],
                );
                self.draw_ai_viz.draw_vars.set_dyn_instance(
                    cx,
                    id!(ripple_alpha),
                    &[ripple_alpha.max(0.0)],
                );
                self.draw_ai_viz
                    .draw_vars
                    .set_dyn_instance(cx, id!(color), &color);
                let click_rect = Rect {
                    pos: dvec2(
                        rect.pos.x + self.ai_viz_pos.x - 28.0,
                        rect.pos.y + self.ai_viz_pos.y - 28.0,
                    ),
                    size: dvec2(56.0, 56.0),
                };
                self.draw_ai_viz.draw_abs(cx, click_rect);
                self.ai_viz_frames_left = self.ai_viz_frames_left.saturating_sub(1);
                if self.ai_viz_frames_left == 0 {
                    self.ai_viz_kind = None;
                    if let Some(next) = self.ai_viz_queue.pop_front() {
                        self.start_input_viz(next);
                    }
                }
                self.redraw(cx);
            } else {
                self.ai_viz_kind = None;
            }
        }

        if waiting_for_framebuffer {
            self.no_fb_view
                .draw_walk_all(cx, scope, Walk::abs_rect(rect));
        }
        self.area = self.draw_app.area();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let target = self.current_target;

        if let Event::Timer(timer_event) = event {
            if self.tick_timer.is_timer(timer_event).is_some() {
                if let Some(target) = target {
                    self.emit_to_app(cx, target.build_id, vec![StudioToApp::Tick]);
                }
            }
        }

        let Some(target) = target else {
            return;
        };

        match event.hits(cx, self.area) {
            Hit::FingerDown(e) => {
                if let Some(local) = self.local_from_area(cx, e.abs) {
                    cx.set_key_focus(self.area);
                    self.emit_to_app(
                        cx,
                        target.build_id,
                        vec![StudioToApp::MouseDown(RemoteMouseDown {
                            button_raw_bits: Self::default_mouse_button(&e.device).bits(),
                            x: local.x,
                            y: local.y,
                            time: e.time,
                            modifiers: RemoteKeyModifiers::from_key_modifiers(&e.modifiers),
                        })],
                    );
                }
            }
            Hit::FingerMove(e) => {
                if let Some(local) = self.local_from_area(cx, e.abs) {
                    self.emit_to_app(
                        cx,
                        target.build_id,
                        vec![StudioToApp::MouseMove(RemoteMouseMove {
                            x: local.x,
                            y: local.y,
                            time: e.time,
                            modifiers: RemoteKeyModifiers::from_key_modifiers(&e.modifiers),
                        })],
                    );
                }
            }
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                self.is_hovered = true;
                cx.set_cursor(self.remote_cursor);
                if let Some(local) = self.local_from_area(cx, e.abs) {
                    self.emit_to_app(
                        cx,
                        target.build_id,
                        vec![StudioToApp::MouseMove(RemoteMouseMove {
                            x: local.x,
                            y: local.y,
                            time: e.time,
                            modifiers: RemoteKeyModifiers::from_key_modifiers(&e.modifiers),
                        })],
                    );
                }
            }
            Hit::FingerHoverOut(_) => {
                self.is_hovered = false;
                cx.set_cursor(MouseCursor::Default);
            }
            Hit::FingerUp(e) => {
                if let Some(local) = self.local_from_area(cx, e.abs) {
                    self.emit_to_app(
                        cx,
                        target.build_id,
                        vec![StudioToApp::MouseUp(RemoteMouseUp {
                            button_raw_bits: Self::default_mouse_button(&e.device).bits(),
                            x: local.x,
                            y: local.y,
                            time: e.time,
                            modifiers: RemoteKeyModifiers::from_key_modifiers(&e.modifiers),
                        })],
                    );
                }
            }
            Hit::FingerScroll(e) => {
                if let Some(local) = self.local_from_area(cx, e.abs) {
                    self.emit_to_app(
                        cx,
                        target.build_id,
                        vec![StudioToApp::Scroll(RemoteScroll {
                            is_mouse: e.device.is_mouse(),
                            time: e.time,
                            x: local.x,
                            y: local.y,
                            sx: e.scroll.x,
                            sy: e.scroll.y,
                            modifiers: RemoteKeyModifiers::from_key_modifiers(&e.modifiers),
                        })],
                    );
                }
            }
            Hit::TextInput(e) => {
                self.emit_to_app(cx, target.build_id, vec![StudioToApp::TextInput(e)]);
            }
            Hit::KeyDown(e) => {
                self.emit_to_app(cx, target.build_id, vec![StudioToApp::KeyDown(e)]);
            }
            Hit::KeyUp(e) => {
                self.emit_to_app(cx, target.build_id, vec![StudioToApp::KeyUp(e)]);
            }
            Hit::TextCopy(_) => {
                self.emit_to_app(cx, target.build_id, vec![StudioToApp::TextCopy]);
            }
            Hit::TextCut(_) => {
                self.emit_to_app(cx, target.build_id, vec![StudioToApp::TextCut]);
            }
            _ => {}
        }
    }
}

impl DesktopRunViewRef {
    pub fn set_run_target(&self, cx: &mut Cx, build_id: QueryId, window_id: Option<usize>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_run_target(cx, build_id, window_id);
        }
    }

    pub fn set_presentable_draw(&self, cx: &mut Cx, presentable_draw: PresentableDraw) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_presentable_draw(cx, presentable_draw);
        }
    }

    pub fn clear_run_target(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear_run_target(cx);
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

    pub fn rebootstrap_after_app_ready(&self, cx: &mut Cx, build_id: QueryId, window_id: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.rebootstrap_after_app_ready(cx, build_id, window_id);
        }
    }
}
