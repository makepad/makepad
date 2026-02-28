use crate::makepad_widgets::*;
use crate::makepad_widgets::makepad_micro_serde::SerBin;
use crate::makepad_widgets::makepad_platform::shared_framebuf::{HostSwapchain, SharedSwapchain};
use makepad_studio_backend::QueryId;
use makepad_studio_protocol::{
    MouseButton, PresentableDraw, RemoteKeyModifiers, RemoteMouseDown, RemoteMouseMove,
    RemoteMouseUp, RemoteScroll, StudioToApp, StudioToAppVec,
};

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
    debug_present_miss_count: usize,
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
        self.swapchain = None;
        self.last_swapchain_with_completed_draws = None;
        self.pending_draw = None;
        self.debug_present_ok_count = 0;
        self.debug_present_miss_count = 0;
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
        self.no_fb_view.redraw(cx);
    }

    fn apply_presentable_draw_to_quad(
        cx: &mut Cx,
        draw_app: &mut DrawQuad,
        redraw_countdown: &mut usize,
        presentable_draw: PresentableDraw,
        swapchain: &HostSwapchain,
    ) -> bool {
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
            &[(swapchain.alloc_width as f32), (swapchain.alloc_height as f32)],
        );
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        draw_app.draw_vars.set_dyn_instance(cx, id!(y_flip), &[1.0f32]);
        #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
        draw_app.draw_vars.set_dyn_instance(cx, id!(y_flip), &[0.0f32]);

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

            self.swapchain = Some(
                HostSwapchain::new(target.window_id, alloc_width, alloc_height, cx),
            );
        }

        #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
        if (needs_new_swapchain || force_bootstrap) && self.swapchain.is_some() {
            if let Some(swapchain) = self.swapchain.as_ref() {
                let shared_swapchain = SharedSwapchain::from_host_swapchain(swapchain, cx);
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
            self.debug_present_miss_count += 1;
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

    fn default_mouse_button(device: &DigitDevice) -> MouseButton {
        device
            .mouse_button()
            .unwrap_or(MouseButton::PRIMARY)
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
}
