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
use makepad_widgets::makepad_micro_serde::SerBin;
use makepad_widgets::makepad_platform::shared_framebuf::{
    shared_swapchain_from_host_swapchain, HostSwapchain,
};
use makepad_widgets::*;

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
use makepad_widgets::makepad_platform::shared_framebuf::aux_chan;
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
use std::sync::{Arc, Mutex};

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
                let fb = self.tex.sample(uv * tex_scale * counter)
                if fb.r == 1.0 && fb.g == 0.0 && fb.b == 1.0 {
                    return #2 * self.fade
                }
                return fb * self.fade
            }
        }
        no_fb_view: RectView {
            width: Fill
            height: Fill
            draw_bg +: {
                color: #0000
            }
            View {
                width: Fill
                height: Fill
                flow: Down
                spacing: 6
                align: Align {x: 0.5 y: 0.5}
                placeholder := Label {
                    text: "starting…"
                    draw_text.color: #x565f89
                    draw_text.text_style.font_size: 11.0
                }
                // The child's newest stdout/stderr line — cargo's
                // "Compiling …" while it builds. One line, a step dimmer.
                status_line := Label {
                    text: ""
                    draw_text.color: #x3b4261
                    draw_text.text_style: theme.font_code
                    draw_text.text_style.font_size: 9.0
                }
            }
        }
    }
}

/// The arrival crossfade length: the wash fades out while the first
/// frames fade in, smoothstepped, long enough to read as a resolve
/// rather than a zap.
const ARRIVAL_FADE_SECS: f32 = 0.28;

/// Drag-stall hunt: timestamped trace lines appended to the file named by
/// MPWM_TRACE. Free when unset (one static branch). Timestamps are UNIX ms
/// (mod 1e7) so host and child (MAKEPAD_STUDIO_TRACE) lines correlate.
pub fn trace_host(line: &str) {
    use std::io::Write;
    use std::sync::{Mutex, OnceLock};
    static FILE: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
    let file = FILE.get_or_init(|| {
        std::env::var("MPWM_TRACE").ok().and_then(|p| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok()
                .map(Mutex::new)
        })
    });
    let Some(file) = file else { return };
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    if let Ok(mut f) = file.lock() {
        let _ = writeln!(f, "{:.2} H {}", ms % 1.0e7, line);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RunTarget {
    client: ClientId,
    window_id: usize,
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
    /// Newest stdout/stderr line from the child, shown while it starts.
    #[rust]
    status_line: String,
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
    first_present_at: Option<std::time::Instant>,
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
    /// While the tile rect is being ANIMATED, the layout's settled target
    /// size. The quad draws at the animated rect; the swapchain and the
    /// child's WindowGeomChange always use this, so a tween never causes
    /// swapchain churn or per-frame child relayouts (see
    /// local/agent_state/mpwm/resize-sync-design.md).
    #[rust]
    target_size: Option<Vec2d>,

    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    #[rust]
    aux_chan_host_endpoint: Option<Arc<Mutex<Option<aux_chan::HostEndpoint>>>>,
}

impl ScriptHook for MpRunView {
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

impl MpRunView {
    fn emit_to_app(&self, cx: &mut Cx, client: ClientId, msgs: Vec<StudioToApp>) {
        if msgs.is_empty() {
            return;
        }
        let msg_bin = StudioToAppVec(msgs).serialize_bin();
        cx.widget_action(self.uid, MpRunViewAction::ForwardToApp { client, msg_bin });
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
        self.present_ok_count = 0;
        self.first_present_at = None;
        self.app_ready_for_swapchain = false;
        self.ime_pos = None;
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        {
            self.aux_chan_host_endpoint = None;
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
    fn setup_aux_chan(&mut self, hub_port: u16, client: ClientId) {
        if self.aux_chan_host_endpoint.is_some() {
            return;
        }
        let studio_addr = format!("http://127.0.0.1:{}", hub_port);
        let listener = match aux_chan::ExternalEndpointListener::new_for_studio(
            &studio_addr,
            &client.to_string(),
        ) {
            Ok(listener) => listener,
            Err(err) => {
                log!("mpwm aux_chan listener failed: {}", err);
                return;
            }
        };
        let slot = Arc::new(Mutex::new(None));
        self.aux_chan_host_endpoint = Some(slot.clone());
        std::thread::Builder::new()
            .name("mpwm-aux-chan-accept".into())
            .spawn(move || match listener.accept_host_endpoint() {
                Ok(endpoint) => {
                    *slot.lock().unwrap() = Some(endpoint);
                }
                Err(err) => {
                    log!("mpwm aux_chan accept failed: {}", err);
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

        if !self.app_ready_for_swapchain {
            return outbound;
        }

        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        {
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
                    Err(err) => log!("mpwm swapchain share failed: {:?}", err),
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
                self.first_present_at = Some(std::time::Instant::now());
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
        self.setup_aux_chan(_hub_port, client);
    }

    /// CreateWindow arrived: the child's stdin loop is live, share the
    /// swapchain from now on.
    pub fn app_ready(&mut self, cx: &mut Cx, client: ClientId, window_id: usize) {
        let target = RunTarget { client, window_id };
        if self.current_target != Some(target) {
            self.set_target(cx, Some(target));
        }
        self.app_ready_for_swapchain = true;
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

    pub fn set_target_size(&mut self, size: Option<Vec2d>) {
        self.target_size = size;
    }

    /// Focus the compositor keyboard on this tile.
    /// Claim the compositor's key focus for this tile. False while the
    /// tile has never drawn (its Area is still empty — focusing it would
    /// be a no-op); the WM keeps such a focus PENDING and retries when the
    /// child's first frame arrives.
    pub fn focus_keyboard(&mut self, cx: &mut Cx) -> bool {
        if self.area == Area::Empty {
            return false;
        }
        cx.set_key_focus(self.area);
        true
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
                let t = (t0.elapsed().as_secs_f32() / ARRIVAL_FADE_SECS).min(1.0);
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
        let rect = cx.walk_turtle(walk).dpi_snap(dpi_factor);
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
                if self.try_present_draw(cx, presentable_draw) {
                    self.pending_draw = None;
                }
            }
        }

        let waiting_for_framebuffer = target.is_some() && self.present_ok_count == 0;
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
                let t = (t0.elapsed().as_secs_f32() / FIRST_FADE).min(1.0);
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
        self.draw_app.draw_abs(cx, rect);

        if waiting_for_framebuffer {
            let status = self.trimmed_status(rect.size.x);
            self.no_fb_view
                .label(cx, ids!(status_line))
                .set_text(cx, &status);
            self.no_fb_view.draw_walk_all(cx, scope, Walk::abs_rect(rect));
        }
        self.area = self.draw_app.area();
        if target.is_some() && cx.has_key_focus(self.area) {
            let ime = self
                .ime_pos
                .unwrap_or_else(|| dvec2(rect.size.x * 0.5, rect.size.y * 0.5));
            cx.show_text_ime(self.area, ime);
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let target = self.current_target;

        if let Event::Timer(timer_event) = event {
            if self.tick_timer.is_timer(timer_event).is_some() {
                if let Some(target) = target {
                    trace_host(&format!("tick c{}", target.client));
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
                    cx.widget_action(
                        self.uid,
                        MpRunViewAction::Clicked {
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
                    trace_host("mm");
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
