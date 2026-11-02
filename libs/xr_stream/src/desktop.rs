//! The desktop keeps its last picture while an XR interaction owns the frame budget.
//! One UI-thread state per server; no worker, lock, or per-frame allocation.

use makepad_widgets::*;

#[derive(Default)]
pub struct XrDesktopFreeze {
    window: Option<WindowId>,
    active: bool,
    last_active: f64,
    timer: Timer,
}

impl XrDesktopFreeze {
    pub const HOLD_SECONDS: f64 = 0.25;

    pub fn active(cx: &Cx) -> bool {
        cx.get_global_ref::<Self>().is_some_and(|s| s.active)
    }

    /// Keep only a desktop pass's already painted texture. Discard queued recordings so
    /// the frame-retention machinery cannot replay its old producers on a mirror paint.
    /// The owner must record this pass and its producers again after the hold.
    pub fn retain_pass(cx: &mut Cx, pass: &DrawPass) {
        let pass = &mut cx.passes[pass.draw_pass_id()];
        pass.main_draw_list_id = None;
        pass.paint_dirty = false;
        pass.repaint_requested = false;
        pass.repaint_next = false;
    }

    /// Bind the app window using the XR widget's desktop area, never an eye-pass area.
    pub fn bind(cx: &mut Cx, area: Area) {
        let Some(window) = cx.get_window_id_of(&area) else { return };
        let s = cx.global::<Self>();
        if s.window == Some(window) { return; }
        let old = s.window.replace(window);
        let active = s.active;
        if let Some(old) = old { cx.set_window_paused(old, false); }
        cx.set_window_paused(window, active);
    }

    /// Call every XR update with the app's held grabs, sliders and navigation input.
    /// Native headset rendering does not pause a desktop window.
    pub fn set_interaction(cx: &mut Cx, active: bool) {
        if !cx.xr_offscreen() {
            Self::clear(cx);
            return;
        }
        if !active { return; }
        let now = cx.seconds_since_app_start();
        let s = cx.global::<Self>();
        s.last_active = now;
        if s.active { return; }
        s.active = true;
        let window = s.window;
        // A timeout checks the release even if XR updates stop with a grab held. It is
        // rearmed only when it fires, never on each pose; it requests no redraw itself.
        let timer = cx.start_timeout(Self::HOLD_SECONDS);
        cx.global::<Self>().timer = timer;
        if let Some(window) = window { cx.set_window_paused(window, true); }
        Self::publish(cx, true);
    }

    /// Call before the app's event handling. Release owes exactly one full redraw after
    /// the hold, including when the last pose was a release or the peer disconnected.
    pub fn handle_event(cx: &mut Cx, event: &Event) {
        if !Self::active(cx) { return; }
        if matches!(event, Event::Shutdown | Event::GpuReset) || !cx.xr_offscreen() {
            Self::clear(cx);
            return;
        }
        if cx.global::<Self>().timer.is_event(event).is_none() { return; }
        let remaining = Self::HOLD_SECONDS - (cx.seconds_since_app_start() - cx.global::<Self>().last_active);
        if remaining > 0.0 {
            let timer = cx.start_timeout(remaining);
            cx.global::<Self>().timer = timer;
        } else {
            Self::clear(cx);
        }
    }

    pub(crate) fn clear(cx: &mut Cx) {
        if !Self::active(cx) { return; }
        let s = cx.global::<Self>();
        s.active = false;
        let timer = std::mem::replace(&mut s.timer, Timer::empty());
        let window = s.window;
        cx.stop_timer(timer);
        if let Some(window) = window { cx.set_window_paused(window, false); }
        Self::publish(cx, false);
    }

    pub(crate) fn permit_mirror(cx: &mut Cx) {
        if let Some(window) = cx.get_global_ref::<Self>().filter(|s| s.active).and_then(|s| s.window) {
            cx.repaint_window_once(window);
        }
    }

    fn publish(cx: &mut Cx, active: bool) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(server) = crate::XrServer::get(cx) {
            server.shared.interaction.store(active, std::sync::atomic::Ordering::Release);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (cx, active);
    }
}

/// Retains a desktop view that normally draws directly into its window. While frozen,
/// `begin` composites its last texture and returns false: no map traversal or child passes.
/// Apps with an existing composite can reuse that instead. Only allocated while serving XR.
pub struct XrDesktopCache {
    pass: DrawPass,
    list: DrawList2d,
    color: Texture,
    _depth: Texture,
    blit: DrawCachedQuad,
}

impl XrDesktopCache {
    pub fn new(cx: &mut Cx) -> Self {
        let pass = DrawPass::new_with_name(cx, "xr desktop cache");
        let color = Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true });
        color.set_retained_render_target(cx, true);
        let depth = Texture::new_with_format(cx, TextureFormat::DepthD32 { size: TextureSize::Auto, initial: true });
        pass.set_color_texture(cx, &color, DrawPassClearColor::ClearWith(vec4f(0.0, 0.0, 0.0, 0.0)));
        pass.set_depth_texture(cx, &depth, DrawPassClearDepth::ClearWith(1.0));
        Self { pass, list: DrawList2d::new(cx), color, _depth: depth, blit: cx.with_vm(DrawCachedQuad::script_new_with_default) }
    }

    pub fn begin(&mut self, cx: &mut Cx2d, rect: Rect) -> bool {
        if XrDesktopFreeze::active(cx) && cx.passes[self.pass.draw_pass_id()].painted_serial > 0 {
            // Do not attach the cache or any of its producers on a mirror-only paint.
            XrDesktopFreeze::retain_pass(cx, &self.pass);
            self.draw(cx, rect);
            return false;
        }
        // A first interaction can precede the first desktop cache. Give its initial
        // recording one paint; subsequent mirror ticks only composite that picture.
        XrDesktopFreeze::permit_mirror(cx);
        let dpi = cx.current_dpi_factor();
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, Some(dpi));
        self.pass.set_size(cx, rect.size);
        cx.set_pass_shift_scale(&self.pass, rect.pos, dvec2(1.0, 1.0));
        self.list.begin_always(cx);
        cx.begin_unclipped_root_turtle(rect.size, Layout::flow_overlay());
        true
    }

    pub fn end(&mut self, cx: &mut Cx2d, rect: Rect) {
        cx.end_pass_sized_turtle_no_clip();
        self.list.end(cx);
        cx.end_pass(&self.pass);
        self.draw(cx, rect);
    }

    fn draw(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.blit.draw_vars.set_texture(0, &self.color);
        self.blit.draw_abs(cx, rect);
    }
}
