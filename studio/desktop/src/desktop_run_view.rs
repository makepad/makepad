use crate::makepad_widgets::image_cache::{
    load_image_from_cache, load_image_from_data_async, process_async_image_load, AsyncImageLoad,
    AsyncLoadResult,
};
use crate::makepad_widgets::makepad_micro_serde::SerBin;
use crate::makepad_widgets::makepad_platform::shared_framebuf::{
    shared_swapchain_from_host_swapchain, HostSwapchain,
};
use crate::makepad_widgets::*;
use makepad_studio_protocol::hub_protocol::{FrameCodec, QueryId, RunViewInputVizKind};
use makepad_studio_protocol::{
    MouseButton, PresentableDraw, RemoteKeyModifiers, RemoteMouseDown, RemoteMouseMove,
    RemoteMouseUp, RemoteScroll, RunViewFrameData, RunViewFrameRequest, StudioToApp,
    StudioToAppVec,
};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
use crate::makepad_widgets::makepad_platform::shared_framebuf::aux_chan;
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
use std::sync::Mutex;

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
                let counter = (self.rect_size * self.draw_pass.dpi_factor) / tp
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
struct RunTarget {
    build_id: QueryId,
    window_id: usize,
}

#[derive(Clone, Copy, Debug)]
struct InputVizEvent {
    kind: RunViewInputVizKind,
    pos: Vec2d,
    size: Option<Vec2d>,
}

#[derive(Clone, Debug)]
struct PendingRemoteDecode {
    path: PathBuf,
    frame_id: u64,
    width: u32,
    height: u32,
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

    fn clear_cached_remote_path(cx: &mut Cx, path: &PathBuf) {
        cx.global::<crate::makepad_widgets::image_cache::ImageCache>()
            .map
            .remove(path);
    }

    fn apply_remote_texture(
        &mut self,
        cx: &mut Cx,
        texture: &Texture,
        width: u32,
        height: u32,
        y_flip: f32,
    ) {
        self.draw_app.set_texture(0, texture);
        self.draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(tex_scale), &[1.0f32, 1.0f32]);
        self.draw_app.draw_vars.set_dyn_instance(
            cx,
            id!(tex_size),
            &[width.max(1) as f32, height.max(1) as f32],
        );
        self.draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(y_flip), &[y_flip]);
        self.draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(packed_header), &[0.0f32]);
        self.redraw_countdown = self.redraw_countdown.max(20);
        self.redraw(cx);
    }

    fn request_remote_frame_if_needed(&mut self, target: RunTarget) -> Option<StudioToApp> {
        if self.last_rect.size.x <= 0.0 || self.last_rect.size.y <= 0.0 {
            return None;
        }
        if self.remote_frame_request_in_flight || self.remote_pending_decode.is_some() {
            return None;
        }
        if !self.remote_mode && self.debug_present_ok_count > 0 {
            return None;
        }
        let frame_id = self.remote_next_frame_id.max(1);
        self.remote_next_frame_id = frame_id.wrapping_add(1).max(1);
        self.remote_frame_request_in_flight = true;
        self.remote_requested_frame_id = Some(frame_id);
        Some(StudioToApp::RunViewFrameRequest(RunViewFrameRequest {
            window_id: target.window_id,
            frame_id,
            width: (self.last_rect.size.x * self.last_dpi_factor)
                .ceil()
                .max(1.0) as u32,
            height: (self.last_rect.size.y * self.last_dpi_factor)
                .ceil()
                .max(1.0) as u32,
            dpi_factor: self.last_dpi_factor,
        }))
    }

    fn set_remote_frame(&mut self, cx: &mut Cx, build_id: QueryId, frame: RunViewFrameData) {
        let Some(target) = self.current_target else {
            return;
        };
        if target.build_id != build_id || target.window_id != frame.window_id {
            return;
        }
        if frame.frame_id < self.remote_current_frame_id {
            return;
        }
        let codec = frame.codec.clone().unwrap_or(FrameCodec::Png);
        self.remote_mode = true;
        self.remote_frame_request_in_flight = false;
        self.remote_requested_frame_id = None;

        let ext = match codec {
            FrameCodec::Png => "png",
            FrameCodec::Jpeg => "jpg",
            FrameCodec::ZstdRgba => return,
        };
        if let Some(prev_path) = self.remote_current_path.take() {
            Self::clear_cached_remote_path(cx, &prev_path);
        }
        if let Some(pending) = self.remote_pending_decode.take() {
            Self::clear_cached_remote_path(cx, &pending.path);
        }
        let path = PathBuf::from(format!(
            "studio_remote_runview://build-{}-window-{}-frame-{}.{}",
            build_id.0, frame.window_id, frame.frame_id, ext
        ));
        let bytes = Arc::new(frame.data);
        match load_image_from_data_async(cx, &path, bytes) {
            Ok(AsyncLoadResult::Loaded) => {
                if let Some(texture) = load_image_from_cache(cx, &path) {
                    let y_flip = if cfg!(all(target_os = "linux", not(target_env = "ohos"))) {
                        1.0
                    } else {
                        0.0
                    };
                    self.apply_remote_texture(cx, &texture, frame.width, frame.height, y_flip);
                    self.remote_current_frame_id = frame.frame_id;
                    self.remote_current_path = Some(path);
                    return;
                }
            }
            Ok(AsyncLoadResult::Loading(_, _)) => {}
            Err(_) => {
                crate::log!(
                    "runview remote frame decode start failed build={} frame={}",
                    build_id.0,
                    frame.frame_id,
                );
                Self::clear_cached_remote_path(cx, &path);
                return;
            }
        }
        self.remote_pending_decode = Some(PendingRemoteDecode {
            path,
            frame_id: frame.frame_id,
            width: frame.width,
            height: frame.height,
        });
        self.redraw(cx);
    }

    fn handle_remote_decode_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            let Some(AsyncImageLoad { image_path, result }) = action.downcast_ref() else {
                continue;
            };
            let Some((pending_path, pending_frame_id, pending_width, pending_height)) =
                self.remote_pending_decode.as_ref().map(|pending| {
                    (
                        pending.path.clone(),
                        pending.frame_id,
                        pending.width,
                        pending.height,
                    )
                })
            else {
                continue;
            };
            if image_path != &pending_path {
                continue;
            }
            if let Some(result) = result.borrow_mut().take() {
                process_async_image_load(cx, image_path, result);
            }
            if let Some(texture) = load_image_from_cache(cx, image_path) {
                let y_flip = if cfg!(all(target_os = "linux", not(target_env = "ohos"))) {
                    1.0
                } else {
                    0.0
                };
                self.apply_remote_texture(cx, &texture, pending_width, pending_height, y_flip);
                self.remote_current_frame_id = pending_frame_id;
                self.remote_current_path = Some(pending_path);
                self.remote_pending_decode = None;
            }
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
        draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(packed_header), &[1.0f32]);
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

    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    fn setup_aux_chan(&mut self, studio_addr: Option<&str>, build_id: QueryId) {
        // Only create the listener once per target
        if self.aux_chan_host_endpoint.is_some() {
            return;
        }
        let Some(studio_addr) = studio_addr else {
            return;
        };
        let listener = match aux_chan::ExternalEndpointListener::new_for_studio(
            studio_addr,
            &build_id.0.to_string(),
        ) {
            Ok(l) => l,
            Err(err) => {
                log!("aux_chan listener failed: {}", err);
                return;
            }
        };
        let slot = Arc::new(Mutex::new(None));
        self.aux_chan_host_endpoint = Some(slot.clone());
        // Accept in background — the child may take a long time to compile and start.
        std::thread::Builder::new()
            .name("aux-chan-accept".into())
            .spawn(move || match listener.accept_host_endpoint() {
                Ok(endpoint) => {
                    *slot.lock().unwrap() = Some(endpoint);
                }
                Err(err) => {
                    crate::log!("aux_chan accept failed: {}", err);
                }
            })
            .ok();
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

        let rect_changed = self.last_rect != rect || self.last_dpi_factor != dpi_factor;
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

        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        {
            if !self.app_ready_for_swapchain {
                return outbound;
            }
            let Some(endpoint_slot) = self.aux_chan_host_endpoint.as_ref() else {
                return outbound;
            };
            let endpoint_guard = endpoint_slot.lock().unwrap();
            let Some(host_endpoint) = endpoint_guard.as_ref() else {
                return outbound;
            };
            if let Some(swapchain) = self.swapchain.as_mut() {
                match shared_swapchain_from_host_swapchain(swapchain, cx, host_endpoint) {
                    Ok(shared) => outbound.push(StudioToApp::Swapchain(shared)),
                    Err(err) => log!("swapchain share failed: {:?}", err),
                }
            }
        }
        #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
        {
            // Keep websocket-only targets, such as Android app sockets, on the
            // remote frame path unless the app has explicitly signaled stdin-loop
            // style readiness via RunViewCreated.
            if !self.app_ready_for_swapchain {
                return outbound;
            }
            if let Some(swapchain) = self.swapchain.as_ref() {
                let shared_swapchain = shared_swapchain_from_host_swapchain(swapchain, cx);
                outbound.push(StudioToApp::Swapchain(shared_swapchain));
            }
        }

        outbound
    }

    pub fn set_presentable_draw(&mut self, cx: &mut Cx, presentable_draw: PresentableDraw) {
        if self.try_present_draw(cx, presentable_draw) {
            self.pending_draw = None;
            self.debug_present_ok_count += 1;
            self.bootstrap_pending = false;
            self.bootstrap_tick_count = 0;
            self.remote_mode = false;
            self.remote_frame_request_in_flight = false;
            self.remote_requested_frame_id = None;
        } else {
            self.pending_draw = Some(presentable_draw);
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

    pub fn show_input_viz(
        &mut self,
        cx: &mut Cx,
        kind: RunViewInputVizKind,
        x: Option<f64>,
        y: Option<f64>,
    ) {
        let has_target_size = self.last_rect.size.x > 0.0 && self.last_rect.size.y > 0.0;
        let event = match kind {
            RunViewInputVizKind::ClickDown | RunViewInputVizKind::ClickUp => {
                self.awaiting_focus_rect = true;
                self.input_focus_rect = None;
                let local_pos = match (x, y) {
                    (Some(x), Some(y)) => dvec2(x, y),
                    _ if has_target_size => {
                        dvec2(self.last_rect.size.x * 0.5, self.last_rect.size.y * 0.5)
                    }
                    _ => self.ai_viz_pos,
                };
                let local_pos = dvec2(
                    local_pos.x.clamp(0.0, self.last_rect.size.x.max(0.0)),
                    local_pos.y.clamp(0.0, self.last_rect.size.y.max(0.0)),
                );
                InputVizEvent {
                    kind,
                    pos: local_pos,
                    size: None,
                }
            }
            RunViewInputVizKind::TypeText | RunViewInputVizKind::Return => {
                if self.awaiting_focus_rect {
                    self.pending_focus_viz_queue.push_back(kind);
                    return;
                }
                let Some(focus_rect) = self.input_focus_rect else {
                    return;
                };
                InputVizEvent {
                    kind,
                    pos: focus_rect.pos,
                    size: Some(focus_rect.size),
                }
            }
        };
        self.enqueue_or_start_input_viz(event);
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
        self.ai_viz_size = event.size;
        self.ai_viz_frames_left = total_frames;
        self.ai_viz_total_frames = total_frames;
    }

    fn enqueue_or_start_input_viz(&mut self, event: InputVizEvent) {
        if self.ai_viz_kind.is_some() {
            self.ai_viz_queue.push_back(event);
        } else {
            self.start_input_viz(event);
        }
    }

    fn set_input_focus_rect(
        &mut self,
        cx: &mut Cx,
        x: Option<f64>,
        y: Option<f64>,
        width: Option<f64>,
        height: Option<f64>,
    ) {
        self.input_focus_rect = match (x, y, width, height) {
            (Some(x), Some(y), Some(width), Some(height)) if width > 0.0 && height > 0.0 => {
                Some(Rect {
                    pos: dvec2(x, y),
                    size: dvec2(width, height),
                })
            }
            _ => None,
        };
        self.awaiting_focus_rect = false;
        if let Some(focus_rect) = self.input_focus_rect {
            while let Some(kind) = self.pending_focus_viz_queue.pop_front() {
                self.enqueue_or_start_input_viz(InputVizEvent {
                    kind,
                    pos: focus_rect.pos,
                    size: Some(focus_rect.size),
                });
            }
        } else {
            self.pending_focus_viz_queue.clear();
        }
        self.redraw(cx);
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
                        RunViewInputVizKind::Return => {
                            ([0.36, 0.90, 0.50, 1.0], 0.0, 0.0, 0.0, 0.0)
                        }
                    };
                    (
                        color,
                        dot_radius,
                        dot_alpha,
                        ripple_radius,
                        ripple_alpha,
                        0.0f32,
                        6.0f32,
                        1.5f32,
                        Rect {
                            pos: dvec2(
                                rect.pos.x + self.ai_viz_pos.x - 28.0,
                                rect.pos.y + self.ai_viz_pos.y - 28.0,
                            ),
                            size: dvec2(56.0, 56.0),
                        },
                    )
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
                    .set_dyn_instance(cx, id!(shape_kind), &[shape_kind]);
                self.draw_ai_viz.draw_vars.set_dyn_instance(
                    cx,
                    id!(corner_radius),
                    &[corner_radius],
                );
                self.draw_ai_viz
                    .draw_vars
                    .set_dyn_instance(cx, id!(stroke_width), &[stroke_width]);
                self.draw_ai_viz
                    .draw_vars
                    .set_dyn_instance(cx, id!(color), &color);
                self.draw_ai_viz.draw_abs(cx, viz_rect);
                self.ai_viz_frames_left = self.ai_viz_frames_left.saturating_sub(1);
                if self.ai_viz_frames_left == 0 {
                    self.ai_viz_kind = None;
                    self.ai_viz_size = None;
                    if let Some(next) = self.ai_viz_queue.pop_front() {
                        self.start_input_viz(next);
                    }
                }
                self.redraw(cx);
            } else {
                self.ai_viz_kind = None;
                self.ai_viz_size = None;
            }
        }

        if waiting_for_framebuffer {
            self.no_fb_view
                .draw_walk_all(cx, scope, Walk::abs_rect(rect));
        }
        self.area = self.draw_app.area();
        if target.is_some() && cx.has_key_focus(self.area) {
            cx.show_text_ime(self.area, self.clamped_ime_pos(rect));
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let target = self.current_target;

        if let Event::Timer(timer_event) = event {
            if self.tick_timer.is_timer(timer_event).is_some() {
                if let Some(target) = target {
                    let mut msgs = Vec::new();
                    let should_bootstrap =
                        self.debug_present_ok_count == 0 || self.bootstrap_pending;
                    if should_bootstrap {
                        self.bootstrap_tick_count = self.bootstrap_tick_count.wrapping_add(1);
                        if self.bootstrap_tick_count == 1 || self.bootstrap_tick_count % 15 == 0 {
                            msgs.extend(self.build_bootstrap_msgs(cx, target));
                        }
                    }
                    if let Some(request) = self.request_remote_frame_if_needed(target) {
                        msgs.push(request);
                    }
                    msgs.push(StudioToApp::Tick);
                    self.emit_to_app(cx, target.build_id, msgs);
                }
            }
        }

        if let Event::Actions(actions) = event {
            self.handle_remote_decode_actions(cx, actions);
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
                    cx.set_key_focus(self.area);
                    self.ime_pos = Some(local);
                    self.redraw(cx);
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

    pub fn set_presentable_draw(&self, cx: &mut Cx, presentable_draw: PresentableDraw) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_presentable_draw(cx, presentable_draw);
        }
    }

    pub fn set_remote_frame(&self, cx: &mut Cx, build_id: QueryId, frame: RunViewFrameData) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_remote_frame(cx, build_id, frame);
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

    pub fn rebootstrap_after_app_ready(&self, cx: &mut Cx, build_id: QueryId, window_id: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.rebootstrap_after_app_ready(cx, build_id, window_id);
        }
    }
}
