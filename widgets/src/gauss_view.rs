//! GaussRoundedView — the refracting surface the glass family is built on.
//!
//! A glass surface has no colour of its own. It samples a snapshot of what is
//! behind it — the window's own content, rendered to a texture and blurred into
//! a mip pyramid — and bends that through its rounded edge. Building the pyramid
//! costs most of an idle window's GPU budget, so a window only builds one on the
//! frames a glass surface actually asked for it. This module owns that per-window
//! bookkeeping and the surface shaders; `widgets/src/window.rs` does the rendering.
//!
//! # The frame a glass surface first appears on
//!
//! The window has to decide whether to capture BEFORE any widget draws, and the
//! only evidence it has is which surfaces asked on the *previous* frame. On the
//! frame a glass surface first appears — app start, a page swap, a live reload —
//! nothing had asked yet, so no capture ran, every surface painted its flat
//! `fallback_color` face, and the real one arrived a frame or more later. That
//! is what `arm_gauss_capture` is for: a surface announces itself from its apply
//! hook, which runs before any drawing, and the next frame of every window
//! captures whether or not anything asked last frame.
//!
//! Announcing beats the alternative — holding the glass face back until a
//! snapshot exists — because a surface that skips a frame is still a change on
//! screen, only from nothing to something instead of from grey to something,
//! and whatever the surface sits over would show through the hole meanwhile.
//! Arming makes the first painted frame the right one instead of the second.
//!
//! A UI with no glass in it never arms and never captures, which is the whole
//! point of the accounting; `MAKEPAD_NO_GAUSS=1` switches capture off for good
//! and leaves every surface on its fallback colour.
//!
//! What this deliberately does not do: it does not drop the last snapshot when a
//! frame skips the capture, so an overlay-only repaint (a hover, a press) goes on
//! refracting the last scene instead of blinking to the fallback; and no shader
//! here reads `draw_pass.time`, which would pin the window at display rate for as
//! long as the glass is on screen — see the note on `ripple_age`.
use crate::{
    gauss_stack::{gauss_render_texture_y_flip_for_os, GaussStack},
    makepad_derive_widget::*,
    makepad_draw::*,
    view::View,
    widget::*,
    window::{DrawGaussDownsample, DrawGaussScene, DrawGaussUpsample},
};
use std::collections::HashMap;

pub const GAUSS_VIEW_LEVELS: usize = 6;

#[derive(Clone)]
pub struct GaussBlurSnapshot {
    pub scene_texture: Texture,
    pub mip_textures: Vec<Texture>,
    pub source_size: Vec2d,
    pub source_y_flip: f32,
    pub dpi_factor: f64,
}

/// MAKEPAD_NO_GAUSS=1: skip the scene capture + blur pyramid entirely (glass falls
/// back to fallback_color). A/B switch for frame-budget hunts — the pyramid is most
/// of an idle UI's per-frame GPU cost. Read once and cached: this is queried for
/// every window on every frame, and `var_os` allocates on each call.
fn gauss_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("MAKEPAD_NO_GAUSS").is_some())
}

#[derive(Default)]
struct GaussWindowEntry {
    generation: u64,
    requested_last_frame: bool,
    requested_this_frame: bool,
    capture_active: bool,
    /// The arm generation this window has already captured for. Zero is what a fresh
    /// entry and an un-armed global both hold, so a UI that never builds a glass
    /// surface never sees an arm.
    armed_seen: u64,
    snapshot: Option<GaussBlurSnapshot>,
}

/// What the end of a window's frame asks of it.
#[derive(Clone, Copy, Debug)]
struct GaussFrameEnd {
    /// The set of surfaces asking for a capture changed, so the window's pass has to be
    /// painted again — with the blur pyramid now attached to it, or without it.
    repaint: bool,
    /// Glass painted this frame with no capture behind it, so what it shows is not this
    /// frame's scene. A repaint cannot mend that: it re-issues the draw calls already
    /// recorded, and whether to capture is only re-decided when the widgets draw again.
    redraw: bool,
}

#[derive(Default)]
struct GaussWindowGlobal {
    windows: Vec<Option<GaussWindowEntry>>,
    /// Bumped by `arm_gauss_capture` every time a glass surface is built or re-applied.
    armed: u64,
    /// One entry per CAPTURE (a host recording an app into a texture of its
    /// own), keyed by the capture pass; `scope` is the nest of captures being
    /// recorded right now, innermost last.
    captures: HashMap<DrawPassId, GaussWindowEntry>,
    /// Each entry with the draw it was pushed in: a draw that unwound out
    /// of a capture (a panic caught by the platform) never popped, and a
    /// stale entry would put every later frame "inside a capture" — so an
    /// entry from an earlier draw is dead, and dropped on the next touch.
    scope: Vec<(DrawPassId, u64)>,
}

impl GaussWindowGlobal {
    /// The capture scope of draw `redraw_id`, entries of earlier draws gone.
    fn scope_in(&mut self, redraw_id: u64) -> &mut Vec<(DrawPassId, u64)> {
        self.scope.retain(|(_, id)| *id == redraw_id);
        &mut self.scope
    }

    fn entry_mut(&mut self, window_id: WindowId) -> &mut GaussWindowEntry {
        let index = window_id.id();
        if self.windows.len() <= index {
            self.windows.resize_with(index + 1, || None);
        }
        let entry = self.windows[index].get_or_insert_with(GaussWindowEntry::default);
        if entry.generation != window_id.1 {
            *entry = GaussWindowEntry {
                generation: window_id.1,
                ..Default::default()
            };
        }
        entry
    }

    fn arm(&mut self) {
        self.armed += 1;
    }

    /// True once per arm per window: this window has not yet run a capture for the
    /// current arm. Each window takes the arm separately, so a second window shows its
    /// glass correctly on its own first frame rather than on whichever frame the first
    /// window happened to spend the arm.
    fn take_arm(&mut self, window_id: WindowId) -> bool {
        let armed = self.armed;
        let entry = self.entry_mut(window_id);
        if entry.armed_seen == armed {
            return false;
        }
        entry.armed_seen = armed;
        true
    }

    fn wants_capture(&mut self, window_id: WindowId, capture_possible: bool) -> bool {
        // The arm is taken either way, so it cannot pile up and fire later.
        let armed = self.take_arm(window_id);
        capture_possible && (armed || self.entry_mut(window_id).requested_last_frame)
    }

    fn begin_frame(
        &mut self,
        window_id: WindowId,
        capture_active: bool,
        snapshot: Option<GaussBlurSnapshot>,
    ) {
        let entry = self.entry_mut(window_id);
        entry.capture_active = capture_active;
        entry.requested_this_frame = false;
        // Only replace the snapshot when we actually re-captured the scene this frame. On frames
        // that skip the capture (e.g. a hover-only overlay repaint), keep the last good snapshot so
        // the glass keeps refracting it instead of blinking to its flat fallback colour.
        if capture_active {
            entry.snapshot = snapshot;
        }
    }

    /// Record that a glass surface drew in this window, and hand it the last captured
    /// scene. `None` only before anything has ever been captured.
    fn request(&mut self, window_id: WindowId) -> Option<GaussBlurSnapshot> {
        let entry = self.entry_mut(window_id);
        entry.requested_this_frame = true;
        // Return the last captured snapshot regardless of whether THIS frame ran a capture pass.
        // This keeps the lensing stable across overlay-only repaints (the source of the hover
        // flicker). `requested_this_frame` still drives a fresh capture on the next full frame.
        entry.snapshot.clone()
    }

    fn finish_frame(&mut self, window_id: WindowId, capture_possible: bool) -> GaussFrameEnd {
        let entry = self.entry_mut(window_id);
        let end = GaussFrameEnd {
            repaint: entry.requested_last_frame != entry.requested_this_frame,
            // With capture switched off there is no better frame to wait for, and asking
            // for one every frame would spin the window forever.
            redraw: capture_possible && entry.requested_this_frame && !entry.capture_active,
        };
        entry.requested_last_frame = entry.requested_this_frame;
        entry.requested_this_frame = false;
        entry.capture_active = false;
        // Intentionally do NOT drop `entry.snapshot` here: a glass overlay can repaint on its own
        // (hover/press) without the window running a full capture pass. Keeping the last snapshot
        // means those repaints still refract the previously captured scene (≤1 capture stale, which
        // is invisible) rather than flickering. It is refreshed whenever a full frame captures again.
        end
    }
}

/// Tell every window to capture the scene on its next frame, because a glass surface
/// now exists that has not drawn yet. Call it from a glass widget's apply hook, which
/// runs before any drawing — the window commits to capturing or not before the widget
/// tree draws, so a surface that waits until its own `draw_walk` to ask has already
/// missed the frame it is being painted in and shows an uncorrected face until the next
/// one. Cheap to call often: a page of thirty glass surfaces still costs one capture.
pub fn arm_gauss_capture(cx: &mut Cx) {
    cx.global::<GaussWindowGlobal>().arm();
}

pub(crate) fn window_wants_gauss_capture(cx: &mut Cx, window_id: WindowId) -> bool {
    let capture_possible = !gauss_disabled();
    cx.global::<GaussWindowGlobal>()
        .wants_capture(window_id, capture_possible)
}

pub(crate) fn begin_window_gauss_frame(
    cx: &mut Cx,
    window_id: WindowId,
    capture_active: bool,
    snapshot: Option<GaussBlurSnapshot>,
) {
    cx.global::<GaussWindowGlobal>()
        .begin_frame(window_id, capture_active, snapshot);
}

/// Returns whether the window's pass has to be painted again. May also ask the whole
/// UI to redraw, which is the only way a window that painted glass without a capture
/// can get one: `use_gauss_capture` is decided in `Window::begin`, so nothing short of
/// drawing again re-decides it. Reached only when a surface appears without having
/// armed (it was already built and merely became visible), so it costs one extra draw
/// in that case and nothing in the ordinary one.
pub(crate) fn finish_window_gauss_frame(cx: &mut Cx, window_id: WindowId) -> bool {
    let capture_possible = !gauss_disabled();
    let end = cx
        .global::<GaussWindowGlobal>()
        .finish_frame(window_id, capture_possible);
    if end.redraw {
        cx.redraw_all();
    }
    end.repaint
}

pub fn request_window_gauss(cx: &mut Cx2d) -> Option<GaussBlurSnapshot> {
    if !cx.is_drawing_overlay() {
        return None;
    }
    // Inside a capture the "window" is the app's own texture: the pyramid
    // is the capture's ([`CaptureGauss`]), never the enclosing window's.
    let redraw_id = cx.redraw_id;
    let global = cx.global::<GaussWindowGlobal>();
    if let Some((pass, _)) = global.scope_in(redraw_id).last().copied() {
        let entry = global.captures.entry(pass).or_default();
        entry.requested_this_frame = true;
        return entry.snapshot.clone();
    }
    let window_id = cx.get_current_window_id()?;
    cx.global::<GaussWindowGlobal>().request(window_id)
}

/// Bind a snapshot's scene and pyramid to a shader that samples the window
/// behind it, or clear the slots so it paints its fallback face. For shaders
/// outside the glass family that sample the window themselves, such as a
/// frosted ring menu, which cannot borrow a `GaussRoundedView` for it.
///
/// The body is `GaussRoundedView::bind_snapshot`'s, slot for slot: slot 0 is
/// the scene, slots 1..=`GAUSS_VIEW_LEVELS` the mips, then `source_size`,
/// `source_y_flip` and `has_gauss`. The two private copies (that one and the
/// glass panel's) are left where they are, so this addition keeps the glass
/// family out of the diff; a shader using this declares the same names.
pub fn bind_gauss_snapshot(vars: &mut DrawVars, cx: &mut Cx2d, snapshot: Option<GaussBlurSnapshot>) {
    if let Some(snapshot) = snapshot {
        vars.set_texture(0, &snapshot.scene_texture);
        for slot in 1..=GAUSS_VIEW_LEVELS {
            if let Some(texture) = snapshot.mip_textures.get(slot - 1) {
                vars.set_texture(slot, texture);
            } else {
                vars.empty_texture(slot);
            }
        }
        vars.set_uniform(
            cx,
            live_id!(source_size),
            &[snapshot.source_size.x as f32, snapshot.source_size.y as f32],
        );
        vars.set_uniform(cx, live_id!(source_y_flip), &[snapshot.source_y_flip]);
        vars.set_uniform(cx, live_id!(has_gauss), &[1.0]);
    } else {
        for slot in 0..=GAUSS_VIEW_LEVELS {
            vars.empty_texture(slot);
        }
        vars.set_uniform(cx, live_id!(source_size), &[1.0, 1.0]);
        vars.set_uniform(cx, live_id!(source_y_flip), &[0.0]);
        vars.set_uniform(cx, live_id!(has_gauss), &[0.0]);
    }
}

/// A capture's own gauss pyramid — the blur an app hosted in a texture of
/// its own gets when it asks for the window's ([`request_window_gauss`]):
/// built from the APP'S frame, never the window's, and recorded into the
/// app's pass tree, so the result is baked into the app's texture and the
/// host only composites that.
///
/// Same shape as the `Window`'s gauss frame: when something asked for it
/// last frame, the app's body renders into a scene pass (sized and shifted
/// like the capture), the pyramid is built from that at `end`, and the
/// scene is drawn back into the capture under the app's overlays. Frames
/// nobody asks for cost nothing: the body records straight into the
/// capture. A request that appears or disappears repaints the capture once
/// (`end` returns `true`), the way the window repaints itself.
pub struct CaptureGauss {
    stack: GaussStack,
    downsample: DrawGaussDownsample,
    upsample: DrawGaussUpsample,
    scene: DrawGaussScene,
    active: bool,
}

impl CaptureGauss {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            stack: GaussStack::new(cx),
            downsample: cx.with_vm(|vm| DrawGaussDownsample::script_new_with_default(vm)),
            upsample: cx.with_vm(|vm| DrawGaussUpsample::script_new_with_default(vm)),
            scene: cx.with_vm(|vm| DrawGaussScene::script_new_with_default(vm)),
            active: false,
        }
    }

    /// Right after the capture pass, its root list and its root turtle are
    /// begun: `shift` and `size` are the capture pass's, `root_size` the
    /// root turtle's (the enclosing window's).
    pub fn begin(&mut self, cx: &mut Cx2d, pass: DrawPassId, shift: Vec2d, size: Vec2d, root_size: Vec2d) {
        let source_y_flip = gauss_render_texture_y_flip_for_os(cx.os_type());
        let dpi_factor = cx.current_dpi_factor();
        let wants = std::env::var_os("MAKEPAD_NO_GAUSS").is_none() && {
            let global = cx.global::<GaussWindowGlobal>();
            global.captures.entry(pass).or_default().requested_last_frame
        };
        let snapshot = wants.then(|| self.stack.snapshot(size, source_y_flip, dpi_factor));
        let redraw_id = cx.redraw_id;
        let global = cx.global::<GaussWindowGlobal>();
        let entry = global.captures.entry(pass).or_default();
        entry.capture_active = wants;
        entry.requested_this_frame = false;
        if wants {
            entry.snapshot = snapshot;
        }
        global.scope_in(redraw_id).push((pass, redraw_id));
        self.active = wants;
        if wants {
            self.stack.begin_scene_shifted(cx, size, shift, root_size);
        }
    }

    /// Before the capture's overlay is composited: the scene pass closes,
    /// the pyramid is built, the scene is drawn back into the capture at
    /// `rect` (its window rect). True when the request state changed.
    pub fn end(&mut self, cx: &mut Cx2d, pass: DrawPassId, rect: Rect) -> bool {
        if self.active {
            self.active = false;
            self.stack.end_scene(cx);
            if rect.size.x >= 0.5 && rect.size.y >= 0.5 {
                self.stack.draw_mip_chain(cx, &mut self.downsample, rect.size);
                self.stack.draw_high_blur_chain(cx, &mut self.upsample, rect.size);
                self.stack.draw_scene_at(cx, &mut self.scene, rect);
            }
        }
        let redraw_id = cx.redraw_id;
        let global = cx.global::<GaussWindowGlobal>();
        global.scope_in(redraw_id).pop();
        let entry = global.captures.entry(pass).or_default();
        let changed = entry.requested_last_frame != entry.requested_this_frame;
        entry.requested_last_frame = entry.requested_this_frame;
        entry.requested_this_frame = false;
        entry.capture_active = false;
        changed
    }

    /// The capture is gone: its request state must not greet the next
    /// capture that reuses the pass slot.
    pub fn forget(cx: &mut Cx, pass: DrawPassId) {
        cx.global::<GaussWindowGlobal>().captures.remove(&pass);
    }

    /// A capture is being recorded right now: whatever draws is already
    /// going into an app's own texture (a host that would otherwise open
    /// one of its own draws straight in).
    pub fn inside_capture(cx: &mut Cx) -> bool {
        let redraw_id = cx.redraw_id;
        !cx.global::<GaussWindowGlobal>().scope_in(redraw_id).is_empty()
    }

    /// How many captures are being recorded right now: what a host notes
    /// before drawing a guest under `catch_unwind`, to hand back to
    /// [`CaptureGauss::unwind_scope_to`] when the guest did not return.
    pub fn scope_depth(cx: &mut Cx) -> usize {
        let redraw_id = cx.redraw_id;
        cx.global::<GaussWindowGlobal>().scope_in(redraw_id).len()
    }

    /// Close every capture begun after `depth` captures were open: the
    /// ones a guest's unwound draw left recording.
    pub fn unwind_scope_to(cx: &mut Cx, depth: usize) {
        let redraw_id = cx.redraw_id;
        cx.global::<GaussWindowGlobal>().scope_in(redraw_id).truncate(depth);
    }
}

// DRAW-ORDER RULE FOR GLASS SURFACES
// -----------------------------------
// Gauss/lens surfaces render their refraction overlay in a LATER pass that
// composites above anything the *parent* widget drew after this child in the
// main pass. Consequence for widget authors: any chrome that must appear on
// top of a glass surface (badges, resize grips, selection outlines) must be a
// CHILD of the glass view - quads drawn by the parent after the child will be
// covered by the lens overlay even though they were drawn "later".

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.View

    mod.widgets.GaussRoundedViewBase = #(GaussRoundedView::register_widget(vm))

    // The glass material (local/agent_state/wm-all/design/
    // clock-weather-design.md §3): the scene behind the slab is TRANSMITTED,
    // not frosted — a low blur inside, sharper still at the rim — and the
    // rim is a LENS: in a band just inside the edge the sample point is
    // pushed outward along the rounded-rect normal, up to `lensing_strength`
    // points at the boundary and exactly zero `lensing_width` points in, so
    // the wallpaper visibly bends through the glass. A thin rim lit from
    // above (`rim_alpha`/`rim_width`), a faint inner shadow along the lower
    // edges, a near-zero tint, a 0.5 pt seal stroke and a soft drop shadow
    // give it thickness. There is no broad additive highlight and no
    // chromatic split by default (`diffraction_strength` is the optional
    // R/B offset, capped at 0.25 pt). Defaults are the phone dock's "clear"
    // profile; `GlassProfile` (Rust) carries the light/dark family.
    mod.widgets.GaussRoundedView = set_type_default() do mod.widgets.GaussRoundedViewBase{
        width: Fill
        height: Fit
        clip_x: false
        clip_y: false
        show_bg: true
        draw_bg +: {
            scene_texture: texture_2d(float)
            mip0_texture: texture_2d(float)
            mip1_texture: texture_2d(float)
            mip2_texture: texture_2d(float)
            mip3_texture: texture_2d(float)
            mip4_texture: texture_2d(float)
            mip5_texture: texture_2d(float)

            has_gauss: uniform(0.0)
            source_size: uniform(vec2(1.0, 1.0))
            source_y_flip: uniform(0.0)
            // Pyramid levels, not points: 0 = scene, 1 = mip0, 2 = mip1 … 6 = mip5.
            blur_level: uniform(1.0)
            edge_blur_level: uniform(0.25)
            gradient_blur_edge: uniform(0.0)
            gradient_blur_edge_width: uniform(0.16)
            gradient_blur_power: uniform(1.25)
            // The edge lens: amplitude, peak displacement in points at the
            // boundary, and the band (inside distance) over which it fades to 0.
            lensing_effect: uniform(1.0)
            lensing_strength: uniform(10.0)
            lensing_width: uniform(12.0)
            // Present/dismiss: the host drives 0 → 1 with its own geometry.
            lens_amplitude: uniform(1.0)
            // Optional R/B split at the lens, in points; 0 = one colour sample.
            diffraction_strength: uniform(0.0)
            // Press: 0..1 flattens the lens to 85% and lifts the rim by 0.06;
            // driven by the widget's own clock, never `draw_pass.time`.
            press_flatten: uniform(0.0)
            // PERF LAW: the click ripple's clock is fed by the widget, NOT by
            // `draw_pass.time`. Any shader that reads `draw_pass.time` is flagged
            // `uses_time` (platform/src/draw_shader.rs) and arms `demo_time_repaint`
            // every frame it draws, which pins the whole window at display rate for
            // as long as the glass is on screen. `ripple_age` is elapsed seconds since
            // the press, pushed from Rust on a NextFrame chain that only runs while a
            // ripple is live (~0.32s). Default 1000.0 = no ripple.
            ripple_age: uniform(1000.0)
            ripple_strength: uniform(0.0)
            // Reduce Transparency: an opaque surface, no lens, hairline only.
            opaque_surface: uniform(0.0)
            // Sdf2d.box renders a VISIBLE radius of 2 * corner_radius.
            corner_radius: instance(15.0)
            tint_color: instance(#ffffff)
            tint_alpha: uniform(0.015)
            surface_alpha: uniform(1.0)
            // Compositor opacity is independent of the material transmission.
            layer_opacity: instance(1.0)
            // The 0.5 pt seal that closes the edges the rim does not light.
            border_color: instance(#fff)
            border_alpha: instance(0.05)
            border_width: instance(0.5)
            // The lit top rim: alpha and width in points.
            rim_alpha: instance(0.55)
            rim_width: uniform(1.25)
            // The inner shadow along the lower edges: alpha and band in points.
            inner_shadow_alpha: instance(0.10)
            inner_shadow_band: uniform(2.5)
            // Optional upper-side glint (≤ 0.04), fading out by 55% of the height.
            specular_strength: instance(0.0)
            noise_strength: instance(0.002)
            /** The face a surface shows when there is nothing to refract:
             * before the first capture, and under MAKEPAD_NO_GAUSS=1.
             *
             * Mixed toward the TEXT colour, not toward color_fg_app. The two
             * app colours are one step apart on the same black-to-white ramp
             * (0.30 vs 0.36 in the dark theme, 0.15 vs 0.175 in the light
             * one, #D vs #E in the skeleton one), so mixing between them
             * lands within a percent of the page and the surface disappears
             * into it. The text colour is the one token guaranteed to
             * contrast with the background in every theme, because that is
             * what it is for, and it points the opposite way in a light
             * theme from a dark one - which is the direction a fixed colour
             * can never get right. Only .rgb is read, so the token's own
             * alpha does not come into it. */
            fallback_color: instance(mix(theme.color_bg_app, theme.color_text, 0.30))
            shadow_color: instance(#0000002a)
            /** how much of the shadow to paint 0..1 step 0.05
             *
             * A surface with no capture behind it is a flat colour, and a
             * full-strength halo round a flat colour reads as a mistake; a
             * page that wants a softer sheet has no other way to ask for one,
             * because shadow_color is a vec4 and the value wanted is a
             * scalar. 1.0 is identity: nothing in the library overrides it,
             * so every surface paints exactly as it did before this existed. */
            shadow_alpha: instance(1.0)
            // `shadow_radius` is the geometry padding (support); `shadow_sigma`
            // the Gaussian's sigma, 0 = half the padding (the old convention).
            shadow_radius: uniform(18.0)
            shadow_sigma: uniform(6.0)
            shadow_offset: uniform(vec2(0.0, 5.0))

            rect_size2: varying(vec2(0.0))
            rect_size3: varying(vec2(0.0))
            rect_pos2: varying(vec2(0.0))
            rect_shift: varying(vec2(0.0))
            sdf_rect_pos: varying(vec2(0.0))
            sdf_rect_size: varying(vec2(0.0))

            vertex: fn() {
                let min_offset = min(self.shadow_offset, vec2(0.0, 0.0))
                self.rect_size2 = self.rect_size + 2.0 * vec2(self.shadow_radius)
                self.rect_size3 = self.rect_size2 + abs(self.shadow_offset)
                self.rect_pos2 = self.rect_pos - vec2(self.shadow_radius) + min_offset
                self.sdf_rect_size = self.rect_size2 - vec2(self.shadow_radius * 2.0 + self.border_width * 2.0)
                self.sdf_rect_pos = -min_offset + vec2(self.border_width + self.shadow_radius)
                self.rect_shift = -min_offset
                return self.clip_and_transform_vertex(self.rect_pos2, self.rect_size3)
            }

            // Bicubic B-spline reconstruction, 4 bilinear taps. Bilinear alone is C0 — its
            // derivative kinks at every texel boundary read as a visible lattice when a low-res
            // mip is stretched over the window. The B-spline is C2-smooth so the texel grid
            // disappears entirely. h packs the two tap coordinates (h0.xy, h1.zw); g0 holds the
            // per-axis weight of the h0 tap pair (the h1 pair weight is 1 - g0).
            bicubic_h: fn(uv: vec2, size: vec2) -> vec4 {
                let tc = uv * size - 0.5
                let f = fract(tc)
                let tc0 = floor(tc)
                let f2 = f * f
                let f3 = f2 * f
                let omf = 1.0 - f
                let w1 = (f3 * 3.0 - f2 * 6.0 + 4.0) / 6.0
                let g0 = omf * omf * omf / 6.0 + w1
                let h0 = clamp((tc0 - 0.5 + w1 / g0) / size, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let h1 = clamp((tc0 + 1.5 + (f3 / 6.0) / (1.0 - g0)) / size, vec2(0.0, 0.0), vec2(1.0, 1.0))
                return vec4(h0.x, h0.y, h1.x, h1.y)
            }

            bicubic_g0: fn(uv: vec2, size: vec2) -> vec2 {
                let f = fract(uv * size - 0.5)
                let f2 = f * f
                let omf = 1.0 - f
                return omf * omf * omf / 6.0 + (f2 * f * 3.0 - f2 * 6.0 + 4.0) / 6.0
            }

            sample_level: fn(level: float, uv: vec2) -> vec4 {
                let source_uv = vec2(uv.x, mix(uv.y, 1.0 - uv.y, self.source_y_flip))
                let safe_uv = clamp(source_uv, vec2(0.0, 0.0), vec2(1.0, 1.0))
                if level < 0.5 {
                    return self.scene_texture.sample_as_bgra(safe_uv)
                }
                if level < 1.5 {
                    let size = max(self.mip0_texture.size(), vec2(1.0, 1.0))
                    let h = self.bicubic_h(safe_uv, size)
                    let g0 = self.bicubic_g0(safe_uv, size)
                    let g1 = 1.0 - g0
                    return self.mip0_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                        + self.mip0_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                        + self.mip0_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                        + self.mip0_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
                }
                if level < 2.5 {
                    let size = max(self.mip1_texture.size(), vec2(1.0, 1.0))
                    let h = self.bicubic_h(safe_uv, size)
                    let g0 = self.bicubic_g0(safe_uv, size)
                    let g1 = 1.0 - g0
                    return self.mip1_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                        + self.mip1_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                        + self.mip1_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                        + self.mip1_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
                }
                if level < 3.5 {
                    let size = max(self.mip2_texture.size(), vec2(1.0, 1.0))
                    let h = self.bicubic_h(safe_uv, size)
                    let g0 = self.bicubic_g0(safe_uv, size)
                    let g1 = 1.0 - g0
                    return self.mip2_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                        + self.mip2_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                        + self.mip2_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                        + self.mip2_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
                }
                if level < 4.5 {
                    let size = max(self.mip3_texture.size(), vec2(1.0, 1.0))
                    let h = self.bicubic_h(safe_uv, size)
                    let g0 = self.bicubic_g0(safe_uv, size)
                    let g1 = 1.0 - g0
                    return self.mip3_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                        + self.mip3_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                        + self.mip3_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                        + self.mip3_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
                }
                if level < 5.5 {
                    let size = max(self.mip4_texture.size(), vec2(1.0, 1.0))
                    let h = self.bicubic_h(safe_uv, size)
                    let g0 = self.bicubic_g0(safe_uv, size)
                    let g1 = 1.0 - g0
                    return self.mip4_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                        + self.mip4_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                        + self.mip4_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                        + self.mip4_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
                }
                let size = max(self.mip5_texture.size(), vec2(1.0, 1.0))
                let h = self.bicubic_h(safe_uv, size)
                let g0 = self.bicubic_g0(safe_uv, size)
                let g1 = 1.0 - g0
                return self.mip5_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                    + self.mip5_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                    + self.mip5_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                    + self.mip5_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
            }

            sample_blur: fn(level: float, uv: vec2) -> vec4 {
                let safe_level = clamp(level, 0.0, 6.0)
                if safe_level >= 5.999 {
                    return self.sample_level(6.0, uv)
                }
                let base_level = floor(safe_level)
                let t = safe_level - base_level

                let l1 = base_level
                let l2 = min(base_level + 1.0, 6.0)
                let blend = t * t * (3.0 - 2.0 * t)
                let c1 = self.sample_level(l1, uv)
                let c2 = self.sample_level(l2, uv)

                return c1.mix(c2, blend)
            }

            sample_gauss: fn(uv: vec2) -> vec4 {
                return self.sample_blur(self.blur_level, uv)
            }

            // The outward normal of the rounded rect, from the SDF's gradient
            // (flat in the interior, where it does not matter: the lens is 0 there).
            rounded_edge_normal: fn(shape: float) -> vec2 {
                let gradient = vec2(dFdx(shape), dFdy(shape))
                if length(gradient) > 0.00001 {
                    return normalize(gradient)
                }
                return vec2(0.0, 1.0)
            }

            // The lens band, capped at 22% of the short side so a small chip
            // does not become all edge; the displacement scales down with it.
            lens_band: fn() -> float {
                let cap = max(min(self.sdf_rect_size.x, self.sdf_rect_size.y) * 0.22, 1.0)
                return min(max(self.lensing_width, 1.0), cap)
            }

            // 1 at the boundary, 0 at `lens_band` inside, smooth between.
            lens_edge: fn(depth: float) -> float {
                let band = self.lens_band()
                let edge = 1.0 - smoothstep(0.0, max(band, 1.0), depth)
                let live = clamp(self.lensing_effect, 0.0, 1.0) * clamp(self.lens_amplitude, 0.0, 1.0)
                return edge * live * (1.0 - self.opaque_surface)
            }

            // Peak displacement in points, reduced with a capped band and by a press.
            lens_strength: fn() -> float {
                let press = clamp(self.press_flatten, 0.0, 1.0)
                return self.lensing_strength * (self.lens_band() / max(self.lensing_width, 1.0)) * (1.0 - 0.15 * press)
            }

            // A press ripple: one soft ring, at most 0.8 pt of displacement,
            // gone within 0.32 s. Radial from the centre of the slab.
            ripple_offset: fn(depth: float) -> vec2 {
                let age = max(self.ripple_age, 0.0)
                let life = 1.0 - smoothstep(0.0, 0.32, age)
                let lens_pos = self.pos * 2.0 - 1.0
                let dist = length(lens_pos)
                let center = smoothstep(0.0, 0.32, age) * 1.2
                let delta = dist - center
                let wave = exp(-(delta * delta) / 0.02) * life * clamp(self.ripple_strength, 0.0, 1.0)
                let dir = lens_pos / max(dist, 0.001)
                return dir * (wave * 0.8)
            }

            // The sample point behind `uv` for a pixel `depth` points inside
            // the edge (in source uv). Kept for the gradient variant.
            lensed_uv: fn(uv: vec2, shape: float) -> vec2 {
                let depth = max(-shape, 0.0)
                let normal = self.rounded_edge_normal(shape)
                let offset = normal * (self.lens_edge(depth) * self.lens_strength()) / max(self.source_size, vec2(1.0, 1.0))
                return clamp(uv + offset, vec2(0.0, 0.0), vec2(1.0, 1.0))
            }

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size3)
                sdf.box(
                    self.sdf_rect_pos.x
                    self.sdf_rect_pos.y
                    self.sdf_rect_size.x
                    self.sdf_rect_size.y
                    max(1.0, self.corner_radius)
                )
                if sdf.shape > -1.0 {
                    let m = self.shadow_radius
                    let o = self.shadow_offset + self.rect_shift
                    let sigma = mix(self.shadow_radius * 0.5, self.shadow_sigma, step(0.001, self.shadow_sigma))
                    let v = GaussShadow.rounded_box_shadow(
                        vec2(m) + o
                        self.rect_size2 + o
                        self.pos * (self.rect_size3 + vec2(m))
                        max(sigma, 0.5)
                        self.corner_radius * 2.0
                    )
                    sdf.clear(self.shadow_color * (v * self.shadow_alpha))
                }

                // Inside distance from the boundary, in points.
                let depth = max(-sdf.shape, 0.0)
                let normal = self.rounded_edge_normal(sdf.shape)
                let edge = self.lens_edge(depth)
                let strength = self.lens_strength()
                let press = clamp(self.press_flatten, 0.0, 1.0)

                // The lens: read the scene a little OUTSIDE the slab near its
                // rim, sharper there than in the interior.
                let src = max(self.source_size, vec2(1.0, 1.0))
                let screen_pos = self.rect_pos2 + self.pos * self.rect_size3
                let displaced = screen_pos + normal * (edge * strength) + self.ripple_offset(depth)
                let q = clamp(displaced / src, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let level = mix(self.blur_level, self.edge_blur_level, edge)
                var sampled = self.sample_blur(level, q)
                if self.diffraction_strength > 0.001 {
                    let chroma = normal * (min(self.diffraction_strength, 0.25) * edge) / src
                    let sr = self.sample_blur(level, clamp(q + chroma, vec2(0.0, 0.0), vec2(1.0, 1.0)))
                    let sb = self.sample_blur(level, clamp(q - chroma, vec2(0.0, 0.0), vec2(1.0, 1.0)))
                    sampled = vec4(sr.r, sampled.g, sb.b, sampled.a)
                }
                let fallback = vec4(self.fallback_color.rgb, 1.0)
                let transmitted = fallback.mix(sampled, self.has_gauss * (1.0 - self.opaque_surface))
                let material = transmitted.rgb.mix(self.tint_color.rgb, self.tint_alpha)

                // Thickness: the inner shadow hugs the lower edges, the rim
                // catches the light from above.
                let inner = (1.0 - smoothstep(0.0, max(self.inner_shadow_band, 0.01), depth))
                    * (0.25 + 0.75 * max(normal.y, 0.0))
                    * self.inner_shadow_alpha * (1.0 - self.opaque_surface)
                let light = normalize(vec2(-0.18, -1.0))
                let facing = pow(max(dot(normal, light), 0.0), 0.8)
                let rim = (1.0 - smoothstep(0.0, max(self.rim_width, 0.01), depth))
                    * facing * (self.rim_alpha + 0.06 * press) * (1.0 - self.opaque_surface)
                let local = clamp((self.pos * self.rect_size3 - self.sdf_rect_pos) / max(self.sdf_rect_size, vec2(1.0, 1.0)), vec2(0.0, 0.0), vec2(1.0, 1.0))
                let glint = self.specular_strength * (1.0 - abs(normal.y)) * (1.0 - smoothstep(0.0, 3.0, depth)) * (1.0 - smoothstep(0.0, 0.55, local.y))
                let shaded = (material * (1.0 - inner)).mix(vec3(1.0, 1.0, 1.0), clamp(rim + glint, 0.0, 1.0))
                // Static banding dither, hashed from screen position only. It must NOT
                // depend on `draw_pass.time`: that flags the shader `uses_time` and pins
                // the window at display rate forever (see the ripple_age note above).
                let noise = (Math.random_2d(screen_pos) - 0.5) * self.noise_strength
                // A captured surface replaces what is behind it at alpha 1: a
                // lower alpha would mix an undisplaced copy back in (ghosting).
                let fill_alpha = mix(self.surface_alpha, 1.0, max(self.has_gauss, self.opaque_surface))
                sdf.fill_keep(vec4(shaded + noise, fill_alpha))
                if self.border_width > 0.0 {
                    let seal = mix(self.border_alpha, max(self.border_alpha, 0.14), self.opaque_surface)
                    sdf.stroke(vec4(self.border_color.rgb, seal), self.border_width)
                }
                return sdf.result * self.layer_opacity
            }
        }
    }

    // The same material with the panel profile: a floating shell (the
    // switcher's cards, a popover), a little more frost and a softer lens.
    mod.widgets.LensedRoundedView = mod.widgets.GaussRoundedView{
        draw_bg +: {
            corner_radius: 14.0
            blur_level: 2.2
            edge_blur_level: 0.5
            lensing_strength: 7.0
            lensing_width: 12.0
            tint_color: #ffffff
            tint_alpha: 0.04
            rim_alpha: 0.35
            rim_width: 1.0
            inner_shadow_alpha: 0.07
            shadow_color: #0000002a
            shadow_radius: 18.0
            shadow_sigma: 6.0
            shadow_offset: vec2(0.0, 4.0)
        }
    }

    // The old name for the block above. It named a vendor rather than the
    // thing, which this repository does not do, and two examples outside
    // the library still write it.
    mod.widgets.AppleGlassRoundedView = mod.widgets.LensedRoundedView{}
    mod.widgets.GaussGradientRoundedView = mod.widgets.GaussRoundedView{
        draw_bg +: {
            blur_level: 4.35
            gradient_blur_edge: 1.45
            gradient_blur_edge_width: 0.20
            gradient_blur_power: 0.75
            tint_alpha: 0.045
            border_alpha: 0.42
            specular_strength: 0.08
            lensing_effect: 0.0

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size3)
                sdf.box(
                    self.sdf_rect_pos.x
                    self.sdf_rect_pos.y
                    self.sdf_rect_size.x
                    self.sdf_rect_size.y
                    max(1.0, self.corner_radius)
                )
                if sdf.shape > -1.0 {
                    let m = self.shadow_radius
                    let o = self.shadow_offset + self.rect_shift
                    let v = GaussShadow.rounded_box_shadow(
                        vec2(m) + o
                        self.rect_size2 + o
                        self.pos * (self.rect_size3 + vec2(m))
                        self.shadow_radius * 0.5
                        self.corner_radius * 2.0
                    )
                    sdf.clear(self.shadow_color * (v * self.shadow_alpha))
                }

                let screen_pos = self.rect_pos2 + self.pos * self.rect_size3
                let uv = screen_pos / max(self.source_size, vec2(1.0, 1.0))
                let fill_pos = clamp(
                    (self.pos * self.rect_size3 - self.sdf_rect_pos) / max(self.sdf_rect_size, vec2(1.0, 1.0)),
                    vec2(0.0, 0.0),
                    vec2(1.0, 1.0)
                )
                let edge_distance = min(
                    min(fill_pos.x, 1.0 - fill_pos.x),
                    min(fill_pos.y, 1.0 - fill_pos.y)
                )
                let edge_fill = smoothstep(
                    0.0,
                    max(self.gradient_blur_edge_width, 0.01),
                    edge_distance
                )
                let center = pow(edge_fill, max(self.gradient_blur_power, 0.01))
                let blur_level = mix(self.gradient_blur_edge, self.blur_level, center)
                let blurred = self.sample_blur(blur_level, self.lensed_uv(uv, sdf.shape))
                let fallback = vec4(self.fallback_color.rgb, 1.0)
                let base = fallback.mix(blurred, self.has_gauss)

                let material = base.rgb.mix(self.tint_color.rgb, self.tint_alpha)
                let edge_uv = abs(self.pos * 2.0 - 1.0)
                let edge_gradient = clamp((edge_uv.x + edge_uv.y) * 0.5, 0.0, 1.0)
                let highlight = self.specular_strength * (0.45 * edge_gradient + 0.55 * center + 0.22 * (1.0 - self.pos.y))
                let fill = vec4(material + highlight, self.surface_alpha)

                sdf.fill_keep(fill)
                if self.border_width > 0.0 {
                    sdf.stroke(
                        vec4(self.border_color.rgb, self.border_alpha),
                        self.border_width
                    )
                }
                return sdf.result * self.layer_opacity
            }
        }
    }
}

#[derive(Script, Widget)]
pub struct GaussRoundedView {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    // Used to self-manage an inline overlay so the glass refracts the scene even when this
    // view is placed in the normal (background) flow rather than inside a `glass.Layer`.
    #[rust]
    draw_list: Option<DrawList2d>,
}

impl ScriptHook for GaussRoundedView {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        // Built (or rebuilt by a live reload) and not drawn yet: ask for the scene to be
        // captured on the frame this first paints in, rather than the one after it.
        vm.with_cx_mut(|cx| arm_gauss_capture(cx));
    }
}

impl GaussRoundedView {
    /// Draw only this material against an explicit compositor checkpoint. No
    /// overlay capture or widget walk: a compositor can stamp it for each layer.
    pub fn draw_surface_with_backdrop(&mut self, cx: &mut Cx2d, rect: Rect, snapshot: Option<GaussBlurSnapshot>, opacity: f32) {
        self.view.draw_bg.draw_vars.set_dyn_instance(cx, live_id!(layer_opacity), &[opacity.clamp(0.0, 1.0)]);
        self.bind_snapshot(cx, snapshot);
        self.view.draw_bg.draw_abs(cx, rect);
    }

    fn bind_snapshot(&mut self, cx: &mut Cx2d, snapshot: Option<GaussBlurSnapshot>) {
        let draw_bg = &mut self.view.draw_bg.draw_vars;
        if let Some(snapshot) = snapshot {
            draw_bg.set_texture(0, &snapshot.scene_texture);
            for slot in 1..=GAUSS_VIEW_LEVELS {
                if let Some(texture) = snapshot.mip_textures.get(slot - 1) {
                    draw_bg.set_texture(slot, texture);
                } else {
                    draw_bg.empty_texture(slot);
                }
            }
            draw_bg.set_uniform(
                cx,
                live_id!(source_size),
                &[snapshot.source_size.x as f32, snapshot.source_size.y as f32],
            );
            draw_bg.set_uniform(cx, live_id!(source_y_flip), &[snapshot.source_y_flip]);
            draw_bg.set_uniform(cx, live_id!(has_gauss), &[1.0]);
        } else {
            for slot in 0..=GAUSS_VIEW_LEVELS {
                draw_bg.empty_texture(slot);
            }
            draw_bg.set_uniform(cx, live_id!(source_size), &[1.0, 1.0]);
            draw_bg.set_uniform(cx, live_id!(source_y_flip), &[0.0]);
            draw_bg.set_uniform(cx, live_id!(has_gauss), &[0.0]);
        }
    }

    pub fn set_opacity(&mut self, cx: &mut Cx, opacity: f32) {
        let surface_alpha = opacity.clamp(0.0, 1.0);
        let tint_alpha = (surface_alpha * 0.30).clamp(0.0, 0.36);
        self.view
            .draw_bg
            .draw_vars
            .set_uniform(cx, live_id!(surface_alpha), &[surface_alpha]);
        self.view
            .draw_bg
            .draw_vars
            .set_uniform(cx, live_id!(tint_alpha), &[tint_alpha]);
        self.view.draw_bg.draw_vars.set_uniform_on_area(
            cx,
            live_id!(surface_alpha),
            &[surface_alpha],
        );
        self.view
            .draw_bg
            .draw_vars
            .set_uniform_on_area(cx, live_id!(tint_alpha), &[tint_alpha]);
        self.redraw(cx);
    }

    pub fn set_blurriness(&mut self, cx: &mut Cx, blurriness: f32) {
        self.set_shader_uniform(cx, live_id!(blur_level), blurriness.clamp(0.0, 6.0));
    }

    pub fn set_lensing_effect(&mut self, cx: &mut Cx, lensing_effect: f32) {
        self.set_shader_uniform(cx, live_id!(lensing_effect), lensing_effect.clamp(0.0, 1.0));
    }

    /// Present/dismiss: the lens follows the host's geometry, 0 (flat) → 1
    /// (full), instead of popping in with the surface.
    pub fn set_lens_amplitude(&mut self, cx: &mut Cx, amplitude: f32) {
        self.set_shader_uniform(cx, live_id!(lens_amplitude), amplitude.clamp(0.0, 1.0));
    }

    /// One profile of the material family, in full: every per-surface and
    /// per-appearance value the shader reads. Set before the surface draws;
    /// instance values go on the next draw, uniforms on this and the
    /// retained draw call alike.
    pub fn apply_profile(&mut self, cx: &mut Cx, profile: &GlassProfile) {
        let vars = &mut self.view.draw_bg.draw_vars;
        let rgb = |c: Vec4| [c.x, c.y, c.z, c.w];
        vars.set_dyn_instance(cx, live_id!(corner_radius), &[profile.corner_radius]);
        vars.set_dyn_instance(cx, live_id!(tint_color), &rgb(profile.tint));
        vars.set_dyn_instance(cx, live_id!(rim_alpha), &[profile.rim_alpha]);
        vars.set_dyn_instance(cx, live_id!(inner_shadow_alpha), &[profile.inner_shadow_alpha]);
        vars.set_dyn_instance(cx, live_id!(border_alpha), &[profile.seal_alpha]);
        vars.set_dyn_instance(cx, live_id!(border_width), &[profile.seal_width]);
        vars.set_dyn_instance(cx, live_id!(shadow_color), &rgb(profile.shadow));
        for (id, value) in [
            (live_id!(blur_level), profile.interior_level),
            (live_id!(edge_blur_level), profile.edge_level),
            (live_id!(lensing_strength), profile.displacement),
            (live_id!(lensing_width), profile.band),
            (live_id!(tint_alpha), profile.tint_alpha),
            (live_id!(rim_width), profile.rim_width),
            (live_id!(shadow_radius), profile.shadow_support),
            (live_id!(shadow_sigma), profile.shadow_sigma),
            (live_id!(opaque_surface), if profile.opaque { 1.0 } else { 0.0 }),
        ] {
            vars.set_uniform(cx, id, &[value]);
            vars.set_uniform_on_area(cx, id, &[value]);
        }
        vars.set_uniform(cx, live_id!(shadow_offset), &[0.0, profile.shadow_y]);
        vars.set_uniform_on_area(cx, live_id!(shadow_offset), &[0.0, profile.shadow_y]);
    }

    /// Drive the press/ripple response. `ripple_age` is elapsed seconds since the
    /// press started (1000.0 = no ripple). The caller owns the clock and must only
    /// keep ticking (NextFrame) while the ripple is live - the shader deliberately
    /// does not read `draw_pass.time`, because that would pin the window at display
    /// rate for as long as the glass is visible.
    pub fn set_press_response(
        &mut self,
        cx: &mut Cx,
        flatten: f32,
        ripple_age: f32,
        ripple_strength: f32,
    ) {
        self.view.draw_bg.draw_vars.set_uniform(
            cx,
            live_id!(press_flatten),
            &[flatten.clamp(-1.0, 1.0)],
        );
        self.view
            .draw_bg
            .draw_vars
            .set_uniform(cx, live_id!(ripple_age), &[ripple_age]);
        self.view.draw_bg.draw_vars.set_uniform(
            cx,
            live_id!(ripple_strength),
            &[ripple_strength.clamp(0.0, 1.0)],
        );
        self.view.draw_bg.draw_vars.set_uniform_on_area(
            cx,
            live_id!(press_flatten),
            &[flatten.clamp(-1.0, 1.0)],
        );
        self.view
            .draw_bg
            .draw_vars
            .set_uniform_on_area(cx, live_id!(ripple_age), &[ripple_age]);
        self.view.draw_bg.draw_vars.set_uniform_on_area(
            cx,
            live_id!(ripple_strength),
            &[ripple_strength.clamp(0.0, 1.0)],
        );
        self.redraw(cx);
    }

    fn set_shader_uniform(&mut self, cx: &mut Cx, id: LiveId, value: f32) {
        self.view.draw_bg.draw_vars.set_uniform(cx, id, &[value]);
        self.view
            .draw_bg
            .draw_vars
            .set_uniform_on_area(cx, id, &[value]);
        self.redraw(cx);
    }
}

/// One member of the glass material family (design §3): the numbers a
/// surface draws with, per appearance. `dock`, `panel`, `pill` and `capsule`
/// are the design's rows; `opaque` is the Reduce Transparency profile of any
/// of them (an opaque surface, no lens, a hairline seal).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassProfile {
    /// The shader argument: the VISIBLE radius is twice this.
    pub corner_radius: f32,
    /// The lens band (points inside the edge) and the peak displacement (points).
    pub band: f32,
    pub displacement: f32,
    /// Pyramid levels: 0 = scene, 1 = mip0, 2 = mip1 …
    pub interior_level: f32,
    pub edge_level: f32,
    pub tint: Vec4,
    pub tint_alpha: f32,
    pub rim_alpha: f32,
    pub rim_width: f32,
    pub inner_shadow_alpha: f32,
    pub seal_alpha: f32,
    pub seal_width: f32,
    /// The drop shadow: colour with alpha, Gaussian sigma, geometry support, y offset.
    pub shadow: Vec4,
    pub shadow_sigma: f32,
    pub shadow_support: f32,
    pub shadow_y: f32,
    pub opaque: bool,
}

impl GlassProfile {
    fn white(alpha: f32) -> Vec4 {
        vec4(1.0, 1.0, 1.0, alpha)
    }
    fn black(alpha: f32) -> Vec4 {
        vec4(0.0, 0.0, 0.0, alpha)
    }

    /// The phone dock (378×82): the clear profile, the strongest lens.
    pub fn dock(dark: bool) -> Self {
        GlassProfile {
            corner_radius: 15.0,
            band: 12.0,
            // The design's peak is 10 with a tuning range of 8–14; measured on
            // the phone skin, the perceived bend of a soft wallpaper feature is
            // ~0.6 of the sampled displacement, so the dock sits at the top of
            // that range.
            displacement: 10.0,
            interior_level: 1.0,
            edge_level: 0.25,
            tint: if dark { vec4(0.043, 0.071, 0.125, 1.0) } else { Self::white(1.0) },
            tint_alpha: if dark { 0.055 } else { 0.015 },
            rim_alpha: if dark { 0.70 } else { 0.55 },
            rim_width: 1.25,
            inner_shadow_alpha: if dark { 0.16 } else { 0.10 },
            seal_alpha: if dark { 0.07 } else { 0.05 },
            seal_width: 0.5,
            shadow: Self::black(if dark { 0.26 } else { 0.16 }),
            shadow_sigma: 6.0,
            shadow_support: 18.0,
            shadow_y: 5.0,
            opaque: false,
        }
    }

    /// A floating shell (the switcher's cards, a popover): more frost, a
    /// softer lens.
    pub fn panel(dark: bool) -> Self {
        GlassProfile {
            corner_radius: 14.0,
            band: 12.0,
            displacement: 7.0,
            interior_level: 2.2,
            edge_level: 0.5,
            tint: if dark { Self::black(1.0) } else { Self::white(1.0) },
            tint_alpha: if dark { 0.10 } else { 0.04 },
            rim_alpha: if dark { 0.50 } else { 0.35 },
            rim_width: 1.0,
            inner_shadow_alpha: if dark { 0.11 } else { 0.07 },
            seal_alpha: if dark { 0.07 } else { 0.05 },
            seal_width: 0.5,
            shadow: Self::black(if dark { 0.26 } else { 0.16 }),
            shadow_sigma: 6.0,
            shadow_support: 18.0,
            shadow_y: 4.0,
            opaque: false,
        }
    }

    /// The App Library search pill (362×48) and any toolbar-sized control.
    pub fn pill(dark: bool) -> Self {
        GlassProfile {
            corner_radius: 12.0,
            band: 7.0,
            displacement: 3.0,
            interior_level: 2.2,
            edge_level: 0.5,
            tint: if dark { Self::black(1.0) } else { Self::white(1.0) },
            tint_alpha: if dark { 0.10 } else { 0.06 },
            rim_alpha: if dark { 0.50 } else { 0.35 },
            rim_width: 1.0,
            inner_shadow_alpha: if dark { 0.05 } else { 0.03 },
            seal_alpha: if dark { 0.07 } else { 0.05 },
            seal_width: 0.5,
            shadow: Self::black(if dark { 0.20 } else { 0.12 }),
            shadow_sigma: 3.0,
            shadow_support: 9.0,
            shadow_y: 2.0,
            opaque: false,
        }
    }

    /// A caption capsule (22 pt high): the faintest member.
    pub fn capsule(dark: bool) -> Self {
        GlassProfile {
            corner_radius: 5.5,
            band: 3.0,
            displacement: 0.8,
            interior_level: 1.8,
            edge_level: 0.5,
            tint: if dark { Self::black(1.0) } else { Self::white(1.0) },
            tint_alpha: if dark { 0.08 } else { 0.04 },
            rim_alpha: if dark { 0.24 } else { 0.18 },
            rim_width: 0.5,
            inner_shadow_alpha: if dark { 0.02 } else { 0.01 },
            seal_alpha: if dark { 0.05 } else { 0.03 },
            seal_width: 0.5,
            shadow: Self::black(if dark { 0.14 } else { 0.08 }),
            shadow_sigma: 2.0,
            shadow_support: 6.0,
            shadow_y: 1.0,
            opaque: false,
        }
    }

    /// Reduce Transparency: the same geometry as `self`, drawn as an opaque
    /// surface with no lens, no rim, and a hairline seal.
    pub fn opaque(mut self, dark: bool) -> Self {
        self.opaque = true;
        self.tint = if dark { vec4(0.11, 0.11, 0.12, 1.0) } else { vec4(0.95, 0.95, 0.97, 1.0) };
        self.tint_alpha = 1.0;
        self.rim_alpha = 0.0;
        self.inner_shadow_alpha = 0.0;
        self.seal_alpha = if dark { 0.22 } else { 0.14 };
        self.seal_width = 1.0;
        self
    }

    /// The pyramid level a compositor must render for this profile: the
    /// deeper of the two the shader interpolates between, plus its neighbour.
    pub fn requested_level(&self) -> f64 {
        (self.interior_level.max(self.edge_level) as f64 + 1.0).min(6.0)
    }

    /// How far outside its rect the surface samples: the peak displacement
    /// plus the chroma reach, for the compositor's reuse footprint.
    pub fn sample_reach(&self) -> f64 {
        self.displacement as f64 + 0.25
    }
}

impl GaussRoundedViewRef {
    pub fn set_opacity(&self, cx: &mut Cx, opacity: f32) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_opacity(cx, opacity);
        }
    }

    pub fn set_blurriness(&self, cx: &mut Cx, blurriness: f32) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_blurriness(cx, blurriness);
        }
    }

    pub fn set_lensing_effect(&self, cx: &mut Cx, lensing_effect: f32) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_lensing_effect(cx, lensing_effect);
        }
    }

    pub fn set_lens_amplitude(&self, cx: &mut Cx, amplitude: f32) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_lens_amplitude(cx, amplitude);
        }
    }

    pub fn apply_profile(&self, cx: &mut Cx, profile: &GlassProfile) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.apply_profile(cx, profile);
        }
    }

    pub fn set_press_response(
        &self,
        cx: &mut Cx,
        flatten: f32,
        ripple_age: f32,
        ripple_strength: f32,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_press_response(cx, flatten, ripple_age, ripple_strength);
        }
    }
}

impl Widget for GaussRoundedView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if cx.is_drawing_overlay() {
            // Already inside an overlay (e.g. a glass.Layer): draw inline.
            let snapshot = request_window_gauss(cx);
            self.bind_snapshot(cx, snapshot);
            self.view.draw_walk(cx, scope, walk)
        } else {
            // In normal flow: open our own overlay so the glass can sample the blurred scene
            // and refract the background beneath it (the layout space is still reserved in the
            // current turtle, so it composes like any other widget).
            if self.draw_list.is_none() {
                self.draw_list = Some(DrawList2d::new(cx));
            }
            self.draw_list.as_mut().unwrap().begin_overlay_reuse(cx);
            let snapshot = request_window_gauss(cx);
            self.bind_snapshot(cx, snapshot);
            let step = self.view.draw_walk(cx, scope, walk);
            self.draw_list.as_mut().unwrap().end(cx);
            step
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: usize) -> WindowId {
        WindowId(id, 0)
    }

    /// One window frame: `glass_draws` says whether a glass surface painted in it.
    /// Mirrors what `Window::begin`/`end` do around the widget tree.
    fn frame(g: &mut GaussWindowGlobal, w: WindowId, glass_draws: bool) -> (bool, GaussFrameEnd) {
        let captured = g.wants_capture(w, true);
        g.begin_frame(w, captured, None);
        if glass_draws {
            g.request(w);
        }
        (captured, g.finish_frame(w, true))
    }

    #[test]
    fn a_ui_with_no_glass_never_captures() {
        let mut g = GaussWindowGlobal::default();
        let w = window(0);
        for _ in 0..4 {
            let (captured, end) = frame(&mut g, w, false);
            assert!(!captured, "nothing asked, so nothing was rendered for it");
            assert!(!end.redraw);
            assert!(!end.repaint);
        }
    }

    #[test]
    fn the_frame_a_glass_surface_first_draws_in_captures() {
        let mut g = GaussWindowGlobal::default();
        let w = window(0);
        // The surface is built before anything draws, and says so.
        g.arm();
        let (captured, end) = frame(&mut g, w, true);
        assert!(captured, "the first painted frame had a scene to refract");
        assert!(!end.redraw, "so it does not have to be painted over");
    }

    #[test]
    fn each_window_takes_the_arm_for_itself() {
        let mut g = GaussWindowGlobal::default();
        g.arm();
        assert!(g.wants_capture(window(0), true));
        assert!(
            g.wants_capture(window(1), true),
            "the second window did not lose the arm to the first"
        );
    }

    #[test]
    fn the_arm_is_spent_once() {
        let mut g = GaussWindowGlobal::default();
        let w = window(0);
        g.arm();
        let (captured, _) = frame(&mut g, w, false);
        assert!(captured);
        let (captured, _) = frame(&mut g, w, false);
        assert!(
            !captured,
            "an unused surface does not keep the pyramid alive"
        );
    }

    #[test]
    fn capture_holds_while_the_glass_stays_and_stops_when_it_goes() {
        let mut g = GaussWindowGlobal::default();
        let w = window(0);
        g.arm();
        frame(&mut g, w, true);
        let (captured, end) = frame(&mut g, w, true);
        assert!(captured, "still on screen, still captured");
        assert!(
            !end.repaint,
            "and nothing about the window's passes changed"
        );

        let (_, end) = frame(&mut g, w, false);
        assert!(
            end.repaint,
            "the glass left: the pass is painted without the pyramid"
        );
        let (captured, _) = frame(&mut g, w, false);
        assert!(!captured);
    }

    #[test]
    fn glass_that_appears_without_arming_asks_to_be_drawn_again() {
        let mut g = GaussWindowGlobal::default();
        let w = window(0);
        // A surface that was built long ago and merely became visible: nobody armed.
        let (captured, end) = frame(&mut g, w, true);
        assert!(!captured, "the window had no way to know");
        assert!(end.redraw, "so it asks for the frame to be drawn again");
        // And the next frame is the corrected one, which asks for nothing further.
        let (captured, end) = frame(&mut g, w, true);
        assert!(captured);
        assert!(!end.redraw);
    }

    #[test]
    fn switched_off_it_neither_captures_nor_spins() {
        let mut g = GaussWindowGlobal::default();
        let w = window(0);
        g.arm();
        for _ in 0..3 {
            assert!(!g.wants_capture(w, false));
            g.begin_frame(w, false, None);
            g.request(w);
            let end = g.finish_frame(w, false);
            assert!(
                !end.redraw,
                "there is no better frame to wait for, so it must not ask for one"
            );
        }
    }
}

#[cfg(test)]
mod material_tests {
    use super::*;

    #[test]
    fn the_family_shares_one_model_and_the_dock_is_the_clear_profile() {
        let light = GlassProfile::dock(false);
        let dark = GlassProfile::dock(true);
        // The design's dock row: visible radius 30, band 12, 10 pt peak, levels 1 / 0.25.
        assert_eq!((light.corner_radius, light.band, light.displacement), (15.0, 12.0, 10.0));
        assert_eq!((light.interior_level, light.edge_level), (1.0, 0.25));
        assert!(light.tint_alpha < 0.02 && dark.tint_alpha < 0.06, "the fill stays clear");
        assert!(dark.rim_alpha > light.rim_alpha, "a dark backdrop takes a brighter rim");
        // The compositor renders mip0 and its neighbour for a level-1 interior.
        assert_eq!(light.requested_level(), 2.0);
        assert_eq!(light.sample_reach(), 10.25);
        // Smaller members lens less, never more than the dock.
        for p in [GlassProfile::panel(false), GlassProfile::pill(false), GlassProfile::capsule(false)] {
            assert!(p.displacement < light.displacement && p.band <= light.band);
            assert!(p.rim_alpha <= light.rim_alpha);
        }
        // Reduce Transparency keeps the geometry and drops the optics.
        let flat = GlassProfile::dock(false).opaque(false);
        assert!(flat.opaque && flat.rim_alpha == 0.0 && flat.tint_alpha == 1.0);
        assert_eq!(flat.corner_radius, light.corner_radius);
        assert!(flat.seal_alpha >= 0.14);
    }
}
