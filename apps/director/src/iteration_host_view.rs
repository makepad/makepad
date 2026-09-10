// Shared-GPU child-process presentation, adapted from the current WM run view.
// Process lifetime belongs to the iteration worker; textures and input belong to the UI.
use crate::iteration_host::ClientId;
use makepad_widgets::makepad_platform::studio::{
    MouseButton, PresentableDraw, RemoteKeyModifiers, RemoteMouseDown, RemoteMouseMove,
    RemoteMouseUp, RemoteScroll, StudioToApp, StudioToAppVec,
};
use makepad_widgets::makepad_micro_serde::SerBin;
use makepad_widgets::makepad_platform::shared_framebuf::{
    shared_swapchain_from_host_swapchain, HostSwapchain,
};
use makepad_widgets::*;

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
use makepad_widgets::makepad_platform::shared_framebuf::aux_chan;
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
use makepad_widgets::makepad_platform::thread::{Lane, TaskHandle};
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
use std::sync::mpsc::Receiver;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.IterationRunViewBase = #(IterationRunView::register_widget(vm))

    mod.widgets.IterationRunView = set_type_default() do mod.widgets.IterationRunViewBase {
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
                let tex_scale = tp / self.tex_size
                // Row zero carries the child's size tracking pixels. Never
                // blend that transport metadata (or unused swapchain space)
                // into the visible window edge.
                let sample_uv = clamp(uv * tex_scale,
                    vec2(0.5, 1.5) / self.tex_size,
                    max(tp - vec2(0.5), vec2(0.5, 1.5)) / self.tex_size)
                let fb = self.tex.sample(sample_uv)
                if fb.r == 1.0 && fb.g == 0.0 && fb.b == 1.0 {
                    return #2
                }
                return fb
            }
        }
        no_fb_view: Label {
            text: "Starting application…"
            width: Fill
            height: Fill
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RunTarget {
    client: ClientId,
    window_id: usize,
}

#[derive(Clone, Debug, Default)]
pub enum IterationRunViewAction {
    ForwardToApp {
        client: ClientId,
        msg_bin: Vec<u8>,
    },
    Clicked {
        client: ClientId,
    },
    #[default]
    None,
}

#[derive(Script, Widget)]
pub struct IterationRunView {
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
    #[rust(true)]
    takes_key_focus: bool,
    #[rust]
    read_only: bool,
    #[rust]
    status_line: String,
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
    #[rust]
    pub canvas_ime_anchor: Option<(Area, PopupAnchorTransform)>,
    #[rust]
    target_size: Option<Vec2d>,

    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    aux_chan_host_endpoint: Option<aux_chan::HostEndpoint>,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    aux_chan_rx: Option<Receiver<Result<aux_chan::HostEndpoint, String>>>,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    aux_chan_task: Option<TaskHandle<()>>,
}

impl ScriptHook for IterationRunView {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            self.draw_app.set_texture(0, &cx.null_texture());
            self.draw_app
                .draw_vars
                .set_dyn_instance(cx, id!(packed_header), &[1.0f32]);
        });
    }
}

impl IterationRunView {
    fn emit_to_app(&self, cx: &mut Cx, client: ClientId, msgs: Vec<StudioToApp>) {
        if msgs.is_empty() {
            return;
        }
        let msg_bin = StudioToAppVec(msgs).serialize_bin();
        cx.widget_action(self.uid, IterationRunViewAction::ForwardToApp { client, msg_bin });
    }

    fn set_target(&mut self, cx: &mut Cx, target: Option<RunTarget>) {
        if self.current_target == target {
            return;
        }
        let had_target = self.current_target.is_some();
        self.current_target = target;
        cx.stop_timer(self.tick_timer);
        if target.is_some() { self.tick_timer = cx.start_interval(1.0 / 60.0); }
        self.remote_cursor = MouseCursor::Default;
        self.is_hovered = false;
        self.swapchain = None;
        self.last_swapchain_with_completed_draws = None;
        self.pending_draw = None;
        self.present_ok_count = 0;
        self.app_ready_for_swapchain = false;
        self.ime_pos = None;
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        {
            self.aux_chan_host_endpoint = None;
            self.aux_chan_rx = None;
            if let Some(task) = self.aux_chan_task.take() {
                task.cancel();
            }
        }
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

    pub fn set_read_only(&mut self, cx: &mut Cx, read_only: bool) {
        self.read_only = read_only;
        if read_only {
            self.release_keyboard(cx);
            self.is_hovered = false;
            cx.set_cursor(MouseCursor::Default);
        }
    }

    pub fn clear_pointer_hover(&mut self) { self.is_hovered = false; }

    pub fn set_remote_cursor(&mut self, cx: &mut Cx, cursor: MouseCursor) {
        self.remote_cursor = cursor;
        if self.is_hovered && !self.read_only {
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
            .set_dyn_instance(cx, id!(packed_header), &[1.0f32]);
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
    fn setup_aux_chan(&mut self, cx: &mut Cx, hub_port: u16, client: ClientId) {
        if self.aux_chan_host_endpoint.is_some() || self.aux_chan_rx.is_some() {
            return;
        }
        let studio_addr = format!("http://127.0.0.1:{}", hub_port);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        match cx.task_pool().submit(Lane::Heavy, move || {
            let result = aux_chan::ExternalEndpointListener::new_for_studio(&studio_addr, &client.to_string())
                .and_then(|listener| listener.accept_host_endpoint())
                .map_err(|err| err.to_string());
            let _ = tx.try_send(result);
        }) {
            Ok(task) => {
                self.aux_chan_rx = Some(rx);
                self.aux_chan_task = Some(task);
            }
            Err(error) => log!("wm aux_chan accept could not be queued: {error}"),
        }
    }

    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    fn poll_aux_chan(&mut self) {
        let Some(rx) = &self.aux_chan_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(endpoint)) => {
                self.aux_chan_host_endpoint = Some(endpoint);
                self.aux_chan_rx = None;
            }
            Ok(Err(error)) => {
                log!("wm aux_chan accept failed: {error}");
                self.aux_chan_rx = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.aux_chan_rx = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
        }
        if let Some(mut task) = self.aux_chan_task.take() {
            let _ = task.try_take();
        }
    }

    fn ensure_swapchain_for_rect(
        &mut self,
        cx: &mut Cx,
        rect: Rect,
        dpi_factor: f64,
        target: RunTarget,
    ) {
        if !rect.size.x.is_finite() || !rect.size.y.is_finite() || !dpi_factor.is_finite()
            || dpi_factor <= 0.0 || rect.size.x <= 0.0 || rect.size.y <= 0.0 {
            return;
        }
        let min_width = ((rect.size.x * dpi_factor).ceil() as u32).max(1);
        let min_height = ((rect.size.y * dpi_factor).ceil() as u32).max(1);
        if min_width > 8192 || min_height > 8192 || u64::from(min_width) * u64::from(min_height) > 16_777_216 {
            self.set_status_line(cx, "Hosted app surface exceeds the 16-megapixel limit");
            return;
        }
        let needs_new_swapchain = self
            .swapchain
            .as_ref()
            .map(|swapchain| {
                min_width != swapchain.alloc_width
                    || min_height != swapchain.alloc_height
                    || swapchain.window_id != target.window_id
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

            // StdinMain captures the shared texture. Keep its allocation
            // equal to the app drawable so recorded frames have no padding.
            // Canvas zoom changes presentation size, not target_size.
            self.swapchain = Some(HostSwapchain::new(
                target.window_id,
                min_width,
                min_height,
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
            self.poll_aux_chan();
            let Some(host_endpoint) = self.aux_chan_host_endpoint.as_ref() else {
                return outbound;
            };
            if let Some(swapchain) = self.swapchain.as_mut() {
                match shared_swapchain_from_host_swapchain(swapchain, cx, host_endpoint) {
                    Ok(shared) => outbound.push(StudioToApp::Swapchain(shared)),
                    Err(err) => log!("wm swapchain share failed: {:?}", err),
                }
            }
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
        if self.current_target.is_none_or(|target| target.window_id != presentable_draw.window_id) { return; }
        if self.try_present_draw(cx, presentable_draw) {

            self.pending_draw = None;
            self.present_ok_count += 1;
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

    pub fn app_ready(&mut self, cx: &mut Cx, client: ClientId, window_id: usize) {
        let target = RunTarget { client, window_id };
        if self.current_target != Some(target) {
            self.set_target(cx, Some(target));
        }
        self.app_ready_for_swapchain = true;
        self.present_ok_count = 0;
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

    pub fn set_target_size(&mut self, size: Option<Vec2d>) {
        self.target_size = size;
    }

    pub fn focus_keyboard(&mut self, cx: &mut Cx) -> bool {
        if !self.takes_key_focus || self.read_only {
            return true;
        }
        if !self.area.is_valid(cx) {
            return false;
        }
        cx.set_key_focus(self.area);
        true
    }

    pub fn release_keyboard(&mut self, cx: &mut Cx) {
        if self.area != Area::Empty && cx.has_key_focus(self.area) {
            cx.set_key_focus(Area::Empty);
        }
    }

    pub fn set_takes_key_focus(&mut self, on: bool) {
        self.takes_key_focus = on;
    }

    pub fn set_status_line(&mut self, cx: &mut Cx, line: &str) {
        if self.status_line == line {
            return;
        }
        self.status_line = line.to_string();
        self.redraw(cx);
    }

    pub fn has_frame(&self) -> bool {
        self.present_ok_count > 0
    }

    fn local_from_area(&self, cx: &Cx, abs: Vec2d) -> Option<Vec2d> {
        if !self.area.is_valid(cx) {
            return None;
        }
        let rect = self.area.rect(cx);
        if rect.size.x <= 0.0 || rect.size.y <= 0.0 { return None; }
        Some(dvec2(
            (abs.x - rect.pos.x) * self.last_rect.size.x / rect.size.x,
            (abs.y - rect.pos.y) * self.last_rect.size.y / rect.size.y,
        ))
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

impl Widget for IterationRunView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let dpi_factor = Self::host_dpi_factor(cx);
        let rect = cx.walk_turtle(walk).dpi_snap(1.0 / dpi_factor);
        // Composite actual child pixels after the first completed GPU frame.
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
                if self.try_present_draw(cx, presentable_draw) {
                    self.pending_draw = None;
                    self.present_ok_count += 1;
                    self.bootstrap_pending = false;
                }
            }
        }

        // The retained executable may still be starting its first window.
        let waiting_for_framebuffer = self.present_ok_count == 0;
        if waiting_for_framebuffer {
            self.redraw(cx);
        } else if self.redraw_countdown > 0 {
            self.redraw_countdown -= 1;
            self.redraw(cx);
        }

        self.draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(host_dpi_factor), &[dpi_factor as f32]);
        if waiting_for_framebuffer {
            self.no_fb_view.set_text(cx, if self.status_line.is_empty() { "Starting application…" } else { &self.status_line });
            self.no_fb_view.draw_walk_all(cx, scope, Walk::abs_rect(rect));
        }
        self.draw_app.draw_abs(cx, rect);
        self.area = self.draw_app.area();
        if target.is_some() && cx.has_key_focus(self.area) {
            let ime = self
                .ime_pos
                .unwrap_or_else(|| dvec2(rect.size.x * 0.5, rect.size.y * 0.5));
            // This anchors the native candidate window for a remote process.
            // It is not a text-input request from the WM or a module client.
            let ime = dvec2(
                ime.x * rect.size.x / self.last_rect.size.x.max(1.0),
                ime.y * rect.size.y / self.last_rect.size.y.max(1.0),
            );
            let (anchor, ime) = if let Some((anchor, transform)) = self.canvas_ime_anchor {
                let screen = (rect.pos + ime) * transform.scale + transform.translation;
                (anchor, screen - anchor.rect(cx).pos)
            } else { (self.area, ime) };
            cx.push_unique_platform_op(CxOsOp::ShowTextIME(
                anchor, Rect{pos:ime,size:Vec2d::default()}, TextInputConfig::default(),
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
                    let should_bootstrap = self.present_ok_count == 0 || self.bootstrap_pending;
                    if should_bootstrap {
                        self.bootstrap_tick_count = self.bootstrap_tick_count.wrapping_add(1);
                        if self.bootstrap_tick_count == 1 || self.bootstrap_tick_count % 15 == 0 {
                            msgs.extend(self.build_bootstrap_msgs(cx, target));
                        }
                    }
                    msgs.push(StudioToApp::Tick);
                    self.emit_to_app(cx, target.client, msgs);
                }
            }
        }

        let Some(target) = target else {
            return;
        };

        if self.read_only { return; }

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
                    if self.takes_key_focus {
                        cx.set_key_focus(self.area);
                        self.ime_pos = Some(local);
                    }
                    cx.widget_action(
                        self.uid,
                        IterationRunViewAction::Clicked {
                            client: target.client,
                        },
                    );
                    self.redraw(cx);
                    self.emit_to_app(
                        cx,
                        target.client,
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
                        target.client,
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
                        target.client,
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
                        target.client,
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
                        target.client,
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
