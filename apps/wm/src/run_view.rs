//! A tile hosting one Makepad child process, ported from Studio's
//! DesktopRunView (studio/desktop/src/desktop_run_view.rs): shared-GPU
//! swapchain presentation plus input forwarding over the studio protocol.
//! Trimmed of studio-only concerns (remote PNG frames, AI input viz) and
//! given rounded corners -- the child texture is clipped by a rounded-rect
//! mask in the shader, Omarchy-style.
//!
//! The protocol state with the child (swapchain, bootstrap, tick pacing,
//! input forwarding, presented frame) is `makepad_app_host::HostedApp`; this
//! widget is what the tile adds: the quad that draws the frame with its
//! corner mask and fades, the "starting..." panel, key focus, the phone's
//! soft keyboard and the cursor.

use crate::own_chrome::ChromePresses;
use makepad_app_host::{ClientId, HostedApp};
use makepad_studio_protocol::MouseButton;
use makepad_widgets::*;

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
            packed_header: instance(1.0)
            // The size the app was last told, in pixels (0: unknown). A frame drawn at
            // another size (an older one, while a resize waits for the app) is shown at
            // its own size from the top-left, not stretched; past its edge the edge
            // pixels continue.
            expect_px: instance(vec2(0.0, 0.0))
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
                let uv = cpos
                if self.packed_header < 0.5 {
                    if self.expect_px.x <= 0.0 || self.expect_px.y <= 0.0 {
                        return self.tex.sample(uv * self.tex_scale) * self.fade
                    }
                    let drawn = self.tex_scale * self.tex_size
                    let px = min(uv * self.expect_px, max(drawn - vec2(0.5), vec2(0.5)))
                    return self.tex.sample(px / self.tex_size) * self.fade
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
    /// Since a live resize began, until a frame of the size the app was told arrives
    /// after it: frames of another size are shown unscaled from the top-left
    /// (`expect_px`). Otherwise a frame fills the quad as before (open and close
    /// animations scale it on purpose).
    #[rust]
    anchor_frames: bool,
    #[rust]
    live_resize: bool,
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
    /// The protocol state with the child.
    #[rust(HostedApp::new(crate::host::CLOCK))]
    pub hosted: HostedApp,
    /// `hosted.present_serial()` as the quad last took it.
    #[rust]
    quad_serial: u64,
    /// FOCUS RULE: false on a Quick-Look panel. Clicks still hit-test and
    /// still reach the child (hover, scroll, buttons all work), but the
    /// press does not pull the compositor's key focus off the requester --
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
    #[rust]
    redraw_countdown: usize,
    /// Counts every applied presented frame (never reset): the content's version.
    #[rust]
    present_serial: u64,
    #[rust]
    remote_cursor: MouseCursor,
    #[rust]
    is_hovered: bool,
    #[rust]
    ime_pos: Option<Vec2d>,
    /// On a phone: the caret of the child's focused text field, while it has
    /// one (`HostedImeState`); the soft keyboard shows only then.
    #[rust]
    child_ime: Option<Vec2d>,
    /// The app draws its own chrome (`own_chrome.rs`): a press asks it where
    /// it landed.
    #[rust]
    chrome_press: ChromePresses,
    /// Whether the window is fullscreen, and what the app was last told (`HostedFullscreen`).
    #[rust]
    fullscreen: bool,
    #[rust]
    told_fullscreen: Option<bool>,
}

impl ScriptHook for MpRunView {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            self.draw_app.set_texture(0, &cx.null_texture());
            self.hosted.start(cx);
            self.draw_app
                .draw_vars
                .set_dyn_instance(cx, id!(packed_header), &[1.0f32]);
        });
    }
}

impl MpRunView {
    /// Hand the batches the child is owed to the desk, which writes them to
    /// its socket. Called after every call into `hosted`.
    fn flush(&mut self, cx: &mut Cx) {
        self.hosted.ensure_beat(cx);
        for (client, msg_bin) in self.hosted.take_outgoing() {
            cx.widget_action(self.uid, MpRunViewAction::ForwardToApp { client, msg_bin });
        }
    }

    /// The child's text field took (`show`) or gave up the keyboard.
    pub fn set_child_ime(&mut self, cx: &mut Cx, ime: makepad_widgets::makepad_platform::ime::HostedImeState) {
        self.child_ime = ime.visible.then(|| dvec2(ime.x, ime.y + ime.height));
        if !ime.visible && crate::host::device_phone() && cx.has_key_focus(self.area) {
            cx.hide_text_ime();
        }
        self.redraw(cx);
    }


    /// The app's answer to the press' drag query: where the window drag
    /// starts when the press was on its caption. The press is then the
    /// window manager's, so the app is told it was cancelled.
    pub fn take_caption_press(&mut self, cx: &mut Cx, answer: &makepad_studio_protocol::HostedDragAnswer, time: f64) -> Option<crate::own_chrome::CaptionPress> {
        let press = self.chrome_press.answer(answer)?;
        if let crate::own_chrome::CaptionPress::Drag(_) = press {
            self.hosted.mouse_up(None, MouseButton::PRIMARY, time, &KeyModifiers::default(), true);
            self.flush(cx);
        }
        Some(press)
    }

    /// Send the child a compact-forms message (`HostedParked`, see
    /// `spacewarp/compact.rs`) on the existing custom channel.
    pub fn send_parked(&mut self, cx: &mut Cx, msg: &makepad_widgets::makepad_platform::ime::HostedParked) -> bool {
        if self.hosted.client().is_none() { return false; }
        self.hosted.send(vec![makepad_studio_protocol::StudioToApp::Custom(msg.to_json())]);
        self.flush(cx);
        true
    }

    /// Did the child present a frame of `size` (points, at `dpi`)? Within two
    /// pixels: the child's area is snapped to the device grid.
    pub fn frame_matches(&self, size: Vec2d, dpi: f64) -> bool {
        self.hosted.presented_px().is_some_and(|[w, h]| {
            (w as f64 - size.x * dpi).abs() <= 2.0 && (h as f64 - size.y * dpi).abs() <= 2.0
        })
    }

    /// For `frametrace`: (the size the child was last told, the settled
    /// size requested, the presented frame's pixels, the present serial,
    /// whether it has a frame).
    pub fn trace_sizes(&self) -> (Vec2d, Option<Vec2d>, Option<[u32; 2]>, u64, bool) {
        let (cfg, tsz) = self.hosted.trace_config();
        (cfg, tsz, self.hosted.presented_px(), self.hosted.present_serial(), self.hosted.has_frame())
    }

    /// Give the child frames of `size` now (a frozen window keeps its child
    /// configured without drawing it).
    pub fn configure(&mut self, cx: &mut Cx, size: Vec2d, dpi: f64) {
        self.hosted.prepare_size(cx, size, dpi);
        self.flush(cx);
    }

    /// The child asked for a frame.
    pub fn request_tick(&mut self, cx: &mut Cx) {
        self.hosted.request_tick(cx);
    }

    /// The child is idle for `secs` (its earliest timer).
    pub fn wake_in(&mut self, cx: &mut Cx, secs: f64) {
        self.hosted.wake_in(cx, secs);
    }

    /// Tell the child whether any of its window shows (`HostedVisible`).
    pub fn send_visible(&mut self, cx: &mut Cx, visible: bool) -> bool {
        if self.hosted.client().is_none() { return false; }
        let json = makepad_studio_protocol::HostedVisible { visible }.to_json();
        self.hosted.send(vec![makepad_studio_protocol::StudioToApp::Custom(json)]);
        self.flush(cx);
        true
    }

    pub fn tick_done(&mut self, cx: &mut Cx) {
        self.hosted.tick_done();
        self.flush(cx);
    }

    /// What a changed target resets in the tile (the protocol state reset
    /// itself in `HostedApp::set_target`).
    fn target_changed(&mut self, cx: &mut Cx, had_target: bool) {
        self.remote_cursor = MouseCursor::Default;
        self.is_hovered = false;
        self.ime_pos = None;
        if self.hosted.client().is_some() {
            // Keep redrawing during startup so bootstrap messages resend
            // until the child socket is ready.
            self.redraw_countdown = self.redraw_countdown.max(240);
        } else {
            if had_target {
                cx.hide_text_ime();
            }
            self.redraw_countdown = 0;
        }
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

    /// Put the hosted frame (or the empty one) on the quad when it changed.
    fn sync_quad(&mut self, cx: &mut Cx) {
        if self.quad_serial == self.hosted.present_serial() {
            return;
        }
        self.quad_serial = self.hosted.present_serial();
        match self.hosted.presented() {
            Some(presented) => {
                self.draw_app.set_texture(0, &presented.texture);
                self.draw_app.draw_vars.set_dyn_instance(cx, id!(tex_scale), &presented.tex_scale);
                self.draw_app.draw_vars.set_dyn_instance(cx, id!(tex_size), &presented.tex_size);
                self.draw_app.draw_vars.set_dyn_instance(cx, id!(packed_header), &[presented.packed_header]);
            }
            None => {
                self.draw_app.set_texture(0, &cx.null_texture());
                self.draw_app.draw_vars.set_dyn_instance(cx, id!(tex_scale), &[0.0f32, 0.0f32]);
                self.draw_app.draw_vars.set_dyn_instance(cx, id!(tex_size), &[1.0f32, 1.0f32]);
                self.draw_app.draw_vars.set_dyn_instance(cx, id!(packed_header), &[1.0f32]);
            }
        }
    }

    /// A presented frame was applied (or not): the quad redraws for it.
    fn presented_frame(&mut self, cx: &mut Cx, applied: bool) {
        if applied {
            self.present_serial += 1;
            self.redraw_countdown = self.redraw_countdown.max(20);
            self.redraw(cx);
        }
    }

    pub fn set_presentable_draw(&mut self, cx: &mut Cx, presentable_draw: makepad_studio_protocol::PresentableDraw) {
        let applied = self.hosted.set_presentable_draw(cx, presentable_draw);
        self.presented_frame(cx, applied);
    }

    pub fn set_run_target(&mut self, cx: &mut Cx, client: ClientId, window_id: usize, hub_port: u16) {
        let had_target = self.hosted.client().is_some();
        if self.hosted.set_run_target(cx, client, window_id, hub_port) {
            self.target_changed(cx, had_target);
        }
    }

    /// Show one of the app `owner`'s later windows (`window_id`) as the desk client
    /// `client` (`client_windows.rs`): its GPU channel is the app's.
    pub fn set_later_window_target(&mut self, cx: &mut Cx, client: ClientId, window_id: usize, owner: ClientId) {
        let had_target = self.hosted.client().is_some();
        if self.hosted.set_window_target(cx, client, window_id, owner) {
            self.target_changed(cx, had_target);
        }
    }

    /// Where this window starts in the app's coordinates (a later window's is far from
    /// the first's, so the app tells their pointer events apart).
    pub fn set_window_origin(&mut self, origin: Vec2d) {
        self.hosted.set_window_origin(origin);
    }

    /// CreateWindow arrived: the child's stdin loop is live, share the
    /// swapchain from now on.
    pub fn app_ready(&mut self, cx: &mut Cx, client: ClientId, window_id: usize) {
        let had_target = self.hosted.client().is_some();
        if self.hosted.app_ready(cx, client, window_id) {
            self.target_changed(cx, had_target);
        }
        self.redraw_countdown = self.redraw_countdown.max(240);
        self.redraw(cx);
    }

    pub fn clear_run_target(&mut self, cx: &mut Cx) {
        let had_target = self.hosted.client().is_some();
        if self.hosted.clear_run_target(cx) {
            self.target_changed(cx, had_target);
        }
    }

    pub fn client(&self) -> Option<ClientId> {
        self.hosted.client()
    }

    /// Focus the compositor keyboard on this tile.
    /// Claim the compositor's key focus for this tile. False while the
    /// tile has no live Area -- never drawn, or not drawn since its draw
    /// list was rebuilt (the AI pane hides its run view entirely between
    /// showings) -- because focusing a dead area is a no-op; the caller
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

    /// Give the keyboard back if this tile holds it -- the AI pane hiding
    /// must not leave its child as the invisible key target.
    pub fn release_keyboard(&mut self, cx: &mut Cx) {
        if self.area != Area::Empty && cx.has_key_focus(self.area) {
            cx.set_key_focus(Area::Empty);
        }
    }

    /// FOCUS RULE: mark this tile a Quick-Look panel -- mouse yes, keyboard
    /// never. Set once when the WM floats a preview viewer.
    pub fn set_takes_key_focus(&mut self, on: bool) {
        self.takes_key_focus = on;
    }

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
    /// tile's darkness continuous -- no bright flash between the wash
    /// vanishing and the content appearing.
    pub fn arrival_fade(&self) -> f32 {
        match self.hosted.first_present_at() {
            Some(t0) => {
                let t = ((crate::host::now() - t0) as f32 / ARRIVAL_FADE_SECS).min(1.0);
                t * t * (3.0 - 2.0 * t)
            }
            None => 1.0,
        }
    }

    /// The newest line the child (or the cargo wrapper building it) wrote.
    /// Shown under "starting..." until the first frame arrives.
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

    /// Keep the tail -- that is where the crate name is.
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
        self.hosted.has_frame()
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
        // Only the "starting..." state gets a backdrop; a presented frame is
        // composited straight over the wallpaper so translucent children
        // (Omarchy's 0.985/0.96 window opacity) show it through.
        if !self.hosted.has_frame() {
            self.draw_bg.draw_abs(cx, rect);
        }

        let target = self.hosted.target();
        // The app draws no caption while its window is fullscreen (`HostedFullscreen`).
        if target.is_some() && self.told_fullscreen != Some(self.fullscreen) {
            self.told_fullscreen = Some(self.fullscreen);
            self.hosted.send(vec![makepad_studio_protocol::StudioToApp::Custom(
                makepad_widgets::makepad_platform::ime::HostedFullscreen { window_id: target.map_or(0, |t| t.window_id), on: self.fullscreen }.to_json())]);
            self.flush(cx);
        }
        if target.is_some() {
            self.hosted.set_geometry(cx, rect, dpi_factor);
            let applied = self.hosted.retry_pending_draw(cx);
            self.presented_frame(cx, applied);
        }
        self.sync_quad(cx);

        // A cargo build has no protocol target yet. Its launch panel must
        // still be visible before the application connects.
        let waiting_for_framebuffer = !self.hosted.has_frame();
        if waiting_for_framebuffer {
            self.redraw(cx);
        } else if self.redraw_countdown > 0 {
            self.redraw_countdown -= 1;
            self.redraw(cx);
        }

        self.hosted.painted();
        self.draw_app
            .draw_vars
            .set_dyn_instance(cx, id!(host_dpi_factor), &[dpi_factor as f32]);
        let told = self.hosted.trace_config().0 * dpi_factor;
        let told = [told.x.ceil() as u32, told.y.ceil() as u32];
        if self.anchor_frames && !self.live_resize && self.hosted.presented_px() == Some(told) {
            self.anchor_frames = false;
        }
        let expect = if self.anchor_frames { [told[0] as f32, told[1] as f32] } else { [0.0, 0.0] };
        self.draw_app.draw_vars.set_dyn_instance(cx, id!(expect_px), &expect);
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
        let first_fade = match self.hosted.first_present_at() {
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
            let t = self.status_line.trim_start();
            let lower = t.to_ascii_lowercase();
            let headline = if lower.starts_with("compiling") {
                "Compiling…"
            } else if lower.starts_with("waiting for another build") {
                "Waiting to compile…"
            } else if lower.starts_with("build failed") {
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
        // On a phone ShowTextIME raises the soft keyboard: only while the
        // child has a focused text field, never for the tile's focus alone.
        let wants_ime = !crate::host::device_phone() || self.child_ime.is_some();
        if target.is_some() && wants_ime && cx.has_key_focus(self.area) {
            let ime = self
                .child_ime
                .or(self.ime_pos)
                .unwrap_or_else(|| dvec2(rect.size.x * 0.5, rect.size.y * 0.5));
            // This anchors the native candidate window for a remote process.
            // It is not a text-input request from the WM or a module client.
            cx.push_unique_platform_op(CxOsOp::ShowTextIME(
                self.area, Rect{pos:ime,size:Vec2d::default()}, TextInputConfig::default(),
            ));
        }
        // What the draw told the host (geometry, a new swapchain) is owed to
        // the child now, and wakes the beat if it was asleep.
        self.flush(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.hosted.handle_event(cx, event);
        self.flush(cx);
        // Input handed to the host wakes the beat if it was asleep (the
        // input path ends in `flush`).
        self.handle_input(cx, event, scope);
    }
}

impl MpRunView {
    fn handle_input(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {

        let Some(target) = self.hosted.target() else {
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
                    self.hosted.mouse_down(local, Self::default_mouse_button(&e.device), e.time, &e.modifiers);
                    if Self::default_mouse_button(&e.device) == MouseButton::PRIMARY {
                        let query = self.chrome_press.begin(e.abs, self.hosted.child_point(local));
                        self.hosted.send(vec![makepad_studio_protocol::StudioToApp::Custom(query.to_json())]);
                    }
                }
            }
            Hit::FingerMove(e) => {
                if let Some(local) = self.local_from_area(cx, e.abs) {
                    self.hosted.mouse_move(local, e.time, &e.modifiers);
                }
            }
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                self.is_hovered = true;
                cx.set_cursor(self.remote_cursor);
                if let Some(local) = self.local_from_area(cx, e.abs) {
                    self.hosted.hover_move(local, e.time, &e.modifiers);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.is_hovered = false;
                cx.set_cursor(MouseCursor::Default);
            }
            Hit::FingerUp(e) => {
                let local = self.local_from_area(cx, e.abs);
                self.chrome_press.end();
                self.hosted.mouse_up(local, Self::default_mouse_button(&e.device), e.time, &e.modifiers, e.cancelled);
            }
            Hit::FingerScroll(e) => {
                if let Some(local) = self.local_from_area(cx, e.abs) {
                    // `FingerScroll` reports every scroll as a mouse device; a
                    // trackpad gesture is the one that carries a phase.
                    let is_mouse = e.device.is_mouse() && e.phase == makepad_widgets::makepad_platform::event::ScrollPhase::None;
                    self.hosted.scroll(local, e.scroll, is_mouse, e.phase, e.time, &e.modifiers);
                }
            }
            Hit::FingerPinch(e) => {
                if let Some(local) = self.local_from_area(cx, e.abs) {
                    self.hosted.pinch(local, e.scale, e.phase, e.time, &e.modifiers);
                }
            }
            Hit::TextInput(e) => self.hosted.text_input(e),
            Hit::KeyDown(e) => self.hosted.key_down(e),
            Hit::KeyUp(e) => self.hosted.key_up(e),
            Hit::TextCopy(_) => self.hosted.text_copy(),
            Hit::TextCut(_) => self.hosted.text_cut(),
            _ => {}
        }
        self.flush(cx);
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

    /// The applied-frame count, with the first draws after a present
    /// (the post-present redraw run, `presented_frame` sets 20) kept
    /// distinct: those still repaint so the image's upload settles; later
    /// ones reuse the capture. None before the first frame.
    fn content_serial(&self) -> Option<u64> {
        let fresh = self.redraw_countdown.saturating_sub(16).min(4) as u64;
        self.has_frame().then_some(self.present_serial * 8 + fresh)
    }

    fn presented_image(&self) -> Option<(Texture, [u32; 2])> {
        let px = self.hosted.presented_px()?;
        Some((self.hosted.presented()?.texture.clone(), px))
    }

    fn drop_files(&mut self, cx: &mut Cx, abs: Vec2d, paths: Vec<String>) -> bool {
        let Some(local) = self.local_from_area(cx, abs) else { return false };
        self.hosted.drop_files(local, paths, &KeyModifiers::default());
        true
    }

    fn set_target_size(&mut self, size: Option<Vec2d>) {
        self.hosted.set_target_size(size)
    }

    fn set_fullscreen(&mut self, on: bool) {
        self.fullscreen = on;
    }

    fn set_live_resize(&mut self, on: bool) {
        self.live_resize = on;
        self.anchor_frames |= on;
        self.hosted.set_live_resize(on)
    }

    fn set_close_crop(&mut self, crop: Option<(Vec2d, Vec2d)>) {
        MpRunView::set_close_crop(self, crop)
    }

    fn set_fade(&mut self, fade: f32) {
        MpRunView::set_fade(self, fade)
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) -> bool {
        MpRunView::send_visible(self, cx, visible)
    }
}
