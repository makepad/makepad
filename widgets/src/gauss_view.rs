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
    gauss_stack::GaussStack,
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

// A capture's own gauss pyramid — the blur an app hosted in a texture of
// its own gets when it asks for the window's ([`request_window_gauss`]):
// built from the APP'S frame, never the window's, and recorded into the
// app's pass tree, so the result is baked into the app's texture and the
// host only composites that.
//
// Same shape as the `Window`'s gauss frame: when something asked for it
// last frame, the app's body renders into a scene pass (sized and shifted
// like the capture), the pyramid is built from that at `end`, and the
// scene is drawn back into the capture under the app's overlays. Frames
// nobody asks for cost nothing: the body records straight into the
// capture. A request that appears or disappears repaints the capture once
// (`end` returns `true`), the way the window repaints itself.
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
        let source_y_flip = 0.0;
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

// A capture's own gauss pyramid — the blur an app hosted in a texture of
// its own gets when it asks for the window's ([`request_window_gauss`]):
// built from the APP'S frame, never the window's, and recorded into the
// app's pass tree, so the result is baked into the app's texture and the
// host only composites that.
//
// Same shape as the `Window`'s gauss frame: when something asked for it
// last frame, the app's body renders into a scene pass (sized and shifted
// like the capture), the pyramid is built from that at `end`, and the
// scene is drawn back into the capture under the app's overlays. Frames
// nobody asks for cost nothing: the body records straight into the
// capture. A request that appears or disappears repaints the capture once
// (`end` returns `true`), the way the window repaints itself.

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
            // RippleLensRoundedView reads it differently, and reads a
            // negative value too: see there.
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

    // The water lens: a surface meant to be pressed. It is the family's
    // earlier, livelier material, kept as a preset of its own because the
    // restrained glass above is right for a dock or a panel and wrong for a
    // lens whose whole point is the press. Against that glass it has:
    //
    // - a milky sheen, brightest toward the rim, the ends and the top
    //   (`specular_strength`), and a bright 1 pt seal;
    // - a rim lens that falls off as `pow(1 - depth / width, 1.45)` over a
    //   band capped at 35% of the short side, so what is behind it bends
    //   and crowds into the edge instead of scribbling along it;
    // - a real red/blue split at the rim, `diffraction_strength` points
    //   deep, with no cap on it;
    // - one blur level everywhere (`blur_level`), and a shadow whose sigma
    //   is half its radius unless `shadow_sigma` says otherwise. The shadow
    //   keeps to the room its container leaves it (`shadow_room`): where a
    //   clip would cut it, it thins out before the cut instead;
    // - the press: `ripple_age` drives a ring that crosses the lens in
    //   0.88 s and fades over 1.05 s, pushes the scene up to
    //   `ripple_reach()` points along its radius, splits its colour and
    //   lights its crest. `press_flatten` 0..1 lays the lens flat behind
    //   the ring, down to a tenth of its bend; a negative flatten is a
    //   rebound from that much flat, which lifts the lens past rest just
    //   behind a second ring before it settles.
    //
    // The clock is the host's (`set_press_response`, `LensPress`), never
    // `draw_pass.time`. Reduce Transparency (`opaque_surface`) drops the
    // lens, the ring and the sheen and keeps a hairline seal, as the base
    // does.
    //
    // Three hooks let a control lie on the lens without a second copy of
    // this shader: `face` recolours the material (a filled button's tint),
    // `glow` adds to the sheen (a hover), and `ripple_reach` sizes the
    // ring's push to the control. `GlassButton` overrides all three.
    mod.widgets.RippleLensRoundedView = mod.widgets.GaussRoundedView{
        draw_bg +: {
            blur_level: 5.0
            corner_radius: 14.0
            tint_color: #b8b8b8
            tint_alpha: 0.10
            surface_alpha: 0.74
            border_width: 1.0
            border_alpha: 0.62
            specular_strength: 0.16
            noise_strength: 0.012
            lensing_effect: 0.75
            lensing_strength: 14.0
            lensing_width: 22.0
            diffraction_strength: 2.4
            shadow_color: #0007
            shadow_radius: 14.0
            shadow_sigma: 0.0
            shadow_offset: vec2(0.0, 5.0)

            // How far the ring pushes the scene at its steepest, in points.
            ripple_reach: fn() -> float {
                return 22.0
            }

            // The material as a control lying on the lens wants it; as it is here.
            face: fn(material: vec3) -> vec3 {
                return material
            }

            // Light a control adds to the sheen; none here.
            glow: fn() -> float {
                return 0.0
            }

            // How much of its shadow the pixel at `p` paints. The shadow
            // reaches past the surface, and a container that clips at the
            // surface's own edge, a row only as tall as a button, would cut
            // it into a dark slab with hard edges. So the shadow thins out
            // with the room left under the surface, to nothing where there is
            // none, and fades out over half its radius before any clip edge
            // instead of stopping at it. Unclipped, it is all there.
            shadow_room: fn(p: vec2) -> float {
                let reach = max(self.shadow_radius + max(self.shadow_offset.y, 0.0), 1.0)
                let below = clamp((self.draw_clip.w - self.rect_pos.y - self.rect_size.y) / reach, 0.0, 1.0)
                let inside = min(min(p.x - self.draw_clip.x, self.draw_clip.z - p.x), min(p.y - self.draw_clip.y, self.draw_clip.w - p.y))
                return below * below * (3.0 - 2.0 * below) * smoothstep(0.0, max(self.shadow_radius * 0.5, 1.0), inside)
            }

            // The rim band, never wider than 35% of the short side: past
            // that a small surface becomes all edge.
            water_band: fn() -> float {
                let cap = max(min(self.sdf_rect_size.x, self.sdf_rect_size.y) * 0.35, 1.0)
                return min(max(self.lensing_width, 1.0), cap)
            }

            // A capped band bends proportionally less.
            water_scale: fn() -> float {
                return self.water_band() / max(self.lensing_width, 1.0)
            }

            // 1 at the boundary, 0 a band inside it (and a band outside it,
            // which only the antialiased edge ever reads).
            water_edge: fn(shape: float) -> float {
                let edge = clamp(1.0 - abs(shape) / self.water_band(), 0.0, 1.0)
                let live = clamp(self.lensing_effect, 0.0, 1.0) * clamp(self.lens_amplitude, 0.0, 1.0)
                return pow(edge, 1.45) * live * (1.0 - self.opaque_surface)
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
                    let room = self.shadow_room(self.rect_pos2 + self.pos * self.rect_size3)
                    sdf.clear(self.shadow_color * (v * self.shadow_alpha * room))
                }

                let clear = 1.0 - self.opaque_surface
                let src = max(self.source_size, vec2(1.0, 1.0))
                let screen_pos = self.rect_pos2 + self.pos * self.rect_size3
                let uv = screen_pos / src

                // The ring. `lens_pos` spans the whole quad, shadow padding
                // included, so when the ring meets the rim depends on the
                // shadow as well as on the size.
                let ripple_age = max(self.ripple_age, 0.0)
                let ripple_life = clamp(1.0 - ripple_age / 1.05, 0.0, 1.0)
                let lens_pos = self.pos * 2.0 - 1.0
                let ripple_dist = length(lens_pos)
                let wave_t = clamp(ripple_age / 0.88, 0.0, 1.0)
                let wave_center = mix(0.0, 1.25, wave_t * wave_t * (3.0 - 2.0 * wave_t))
                let wave_width = 0.24
                let wave_delta = ripple_dist - wave_center
                let wave = exp(-(wave_delta * wave_delta) / (wave_width * wave_width))
                let wave_mask = smoothstep(0.0, 0.10, ripple_age) * (1.0 - smoothstep(1.16, 1.44, ripple_dist))
                let ripple_wave = wave * ripple_life * ripple_life * clamp(self.ripple_strength, 0.0, 1.0) * wave_mask * clear
                let ripple_slope = (-wave_delta / wave_width) * ripple_wave
                let ripple_dir = lens_pos / max(ripple_dist, 0.001)

                // The flatten sweeps out behind the ring; a rebound undoes
                // it the same way and overshoots just behind the front.
                let press = clamp(self.press_flatten, 0.0, 1.0)
                let restore = clamp(-self.press_flatten, 0.0, 1.0)
                let wave_flatten = smoothstep(ripple_dist - 0.14, ripple_dist + 0.26, wave_center)
                let flatten = clamp(press * wave_flatten + restore * (1.0 - wave_flatten), 0.0, 1.0)
                let lift = restore * wave_flatten * (1.0 - wave_t) * 0.45
                let ripple_surface = ripple_slope * 0.85 + ripple_wave * 0.20
                let lens_depth = clamp(1.0 - flatten * 0.90 + lift * 0.55 + ripple_wave * 0.18, 0.0, 1.55)
                let diffraction_depth = clamp(1.0 - flatten * 0.76 + lift * 0.70 + (abs(ripple_surface) + ripple_wave) * 1.15, 0.0, 2.10)

                let edge = self.water_edge(sdf.shape)
                let lens = edge * lens_depth
                let normal = self.rounded_edge_normal(sdf.shape)
                let water_offset = ripple_dir * (ripple_surface * self.ripple_reach()) / src
                let base_offset = normal * (lens * self.lensing_strength * self.water_scale()) / src + water_offset
                let color_offset = normal * (lens * self.diffraction_strength * diffraction_depth) / src
                    + ripple_dir * ((ripple_surface + ripple_wave * 0.65) * self.diffraction_strength * 4.5) / src
                let uv_g = clamp(uv + base_offset, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let uv_r = clamp(uv_g + color_offset, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let uv_b = clamp(uv_g - color_offset, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let sample_r = self.sample_gauss(uv_r)
                let sample_g = self.sample_gauss(uv_g)
                let sample_b = self.sample_gauss(uv_b)
                let refracted = vec4(sample_r.r, sample_g.g, sample_b.b, (sample_r.a + sample_g.a + sample_b.a) * 0.3333333)
                let fallback = vec4(self.fallback_color.rgb, 1.0)
                let base = fallback.mix(refracted, self.has_gauss * clear)
                let material = self.face(base.rgb.mix(self.tint_color.rgb, self.tint_alpha))

                // The sheen: brighter toward the ends, at the rim and at the
                // top, dimmed a little while the lens lies flat.
                let edge_uv = abs(self.pos * 2.0 - 1.0)
                let edge_gradient = clamp((edge_uv.x + edge_uv.y) * 0.5, 0.0, 1.0)
                let sparkle = edge * self.diffraction_strength * 0.004 * (1.0 - flatten * 0.45)
                let sheen = self.specular_strength * (0.45 * edge_gradient + 0.55 * edge + 0.30 * (1.0 - self.pos.y)) * (1.0 - flatten * 0.28)
                let highlight = (sheen + ripple_wave * 0.11 + self.glow()) * clear
                // Static banding dither - see the note in GaussRoundedView's pixel().
                let noise = (Math.random_2d(screen_pos) - 0.5) * self.noise_strength
                let fill_alpha = mix(self.surface_alpha, 1.0, max(self.has_gauss, self.opaque_surface))
                sdf.fill_keep(vec4(material + highlight + sparkle + noise, fill_alpha))
                if self.border_width > 0.0 {
                    let seal = mix(self.border_alpha, max(self.border_alpha, 0.14), self.opaque_surface)
                    sdf.stroke(vec4(self.border_color.rgb, seal), self.border_width)
                }
                return sdf.result * self.layer_opacity
            }
        }
    }

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
    /// press started (`RIPPLE_AT_REST` = no ripple). The caller owns the clock and must only
    /// keep ticking (NextFrame) while the ripple is live - the shader deliberately
    /// does not read `draw_pass.time`, because that would pin the window at display
    /// rate for as long as the glass is visible.
    ///
    /// What the numbers do depends on the preset. On `RippleLensRoundedView`
    /// the ring crosses the lens in 0.88 s and lives about 1.05 s, a flatten of
    /// 0..1 lays the lens flat behind it, and a negative flatten is a rebound
    /// from that much flat that overshoots before it settles; `LensPress` is a
    /// clock-free host for exactly that. The restrained presets draw a 0.8 pt
    /// ring for about a third of a second, flatten the lens by at most 15%,
    /// and ignore a negative flatten.
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

/// The ripple clock's value for "no ripple": every surface of the family
/// treats an age this far past a ring's life as none.
pub const RIPPLE_AT_REST: f32 = 1000.0;

/// One frame of a press, in the three numbers `set_press_response` takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PressResponse {
    /// 0..1 lays the lens flat. Below zero is a rebound from that much flat,
    /// which only `RippleLensRoundedView` draws; the restrained presets
    /// clamp it to 0.
    pub flatten: f32,
    /// Seconds since the press or the release began, or `RIPPLE_AT_REST`.
    pub ripple_age: f32,
    /// 0..1, how strong the ring is on this frame.
    pub ripple_strength: f32,
}

impl PressResponse {
    /// Nothing pressed, no ring.
    pub const REST: Self = Self { flatten: 0.0, ripple_age: RIPPLE_AT_REST, ripple_strength: 0.0 };
    /// Held down after the ring has gone: flat and still.
    pub const HELD: Self = Self { flatten: 1.0, ripple_age: RIPPLE_AT_REST, ripple_strength: 0.0 };
}

impl Default for PressResponse {
    fn default() -> Self {
        Self::REST
    }
}

/// How a water lens answers a press, in seconds. The defaults are the lens
/// button's: flat in 0.78 s, a ring that fades over 1.05 s, and a release
/// ring at 62% of the press ring. How fast the ring travels is the shader's
/// (0.88 s across), and so is how long it is drawn at all (1.05 s); this
/// says how flat the lens is and how strong the ring is on each frame, and
/// the clock runs until the shader has nothing left to move.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LensPressCurve {
    /// How long the lens takes to lie flat under a press.
    pub press_secs: f64,
    /// How long a ring takes to fade to nothing.
    pub ripple_secs: f64,
    /// The release ring's strength, as a share of the press ring's.
    pub release_ripple: f32,
}

impl Default for LensPressCurve {
    fn default() -> Self {
        Self { press_secs: 0.78, ripple_secs: 1.05, release_ripple: 0.62 }
    }
}

impl LensPressCurve {
    /// How long the clock runs on after a ring has faded, so the last frame
    /// it draws has no ring left in it.
    pub const TAIL_SECS: f64 = 0.03;
    /// How long the shader's ring takes to cross the lens. The flatten, and
    /// the rebound, sweep the lens behind its front, so neither is done
    /// before this, however soon the ring fades.
    pub const RING_TRAVEL_SECS: f64 = 0.88;
    /// How long the shader draws a ring at all. Past this age it draws none,
    /// however strong the curve still says the ring is.
    pub const RING_LIFE_SECS: f64 = 1.05;

    /// When the ring has nothing left to show: it has crossed the lens, and
    /// it has faded or the shader has stopped drawing it.
    fn ring_done(&self) -> f64 {
        Self::RING_TRAVEL_SECS.max(self.ripple_secs.max(0.0).min(Self::RING_LIFE_SECS))
    }

    /// When the clock stops on a press, in seconds after it: the lens is
    /// flat and the ring is done.
    pub fn press_stops_at(&self) -> f64 {
        self.press_secs.max(0.0).max(self.ring_done()) + Self::TAIL_SECS
    }

    /// When the clock stops on a release, in seconds after it: the rebound
    /// has swept the lens and the ring is done.
    pub fn release_stops_at(&self) -> f64 {
        self.ring_done() + Self::TAIL_SECS
    }

    /// `age` seconds into a press: the frame, and whether the clock has to
    /// tick again. The lens eases flat over `press_secs` while the ring fades
    /// over `ripple_secs`, a tenth weaker the flatter the lens is. Once the
    /// clock stops the lens is held flat with no ring.
    pub fn pressed(&self, age: f64) -> (PressResponse, bool) {
        let age = age.max(0.0);
        if age >= self.press_stops_at() {
            return (PressResponse::HELD, false);
        }
        let t = if self.press_secs > 0.0 { (age / self.press_secs).min(1.0) as f32 } else { 1.0 };
        let flatten = t * t * (3.0 - 2.0 * t);
        let response = PressResponse {
            flatten,
            ripple_age: age as f32,
            ripple_strength: self.fade(age) * (1.0 - 0.10 * flatten),
        };
        (response, true)
    }

    /// `age` seconds into a release from `restore` flat: the lens rebounds
    /// from there under a ring `release_ripple` as strong as a press's, and
    /// is at rest once the clock stops.
    pub fn released(&self, age: f64, restore: f32) -> (PressResponse, bool) {
        let age = age.max(0.0);
        if age >= self.release_stops_at() {
            return (PressResponse::REST, false);
        }
        let response = PressResponse {
            flatten: -restore.clamp(0.0, 1.0),
            ripple_age: age as f32,
            ripple_strength: self.fade(age) * self.release_ripple.clamp(0.0, 1.0),
        };
        (response, true)
    }

    fn fade(&self, age: f64) -> f32 {
        if self.ripple_secs > 0.0 {
            (1.0 - age / self.ripple_secs).clamp(0.0, 1.0) as f32
        } else {
            0.0
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum LensPressPhase {
    #[default]
    Rest,
    Pressing,
    Held,
    Releasing,
}

/// What a host does with one step of a press: push `response` through
/// `set_press_response` when there is one, and ask for a frame when `tick`
/// says so.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LensPressStep {
    pub response: Option<PressResponse>,
    pub tick: bool,
}

/// A press on a water lens, with the clock left to its host. The host
/// forwards the finger going down and coming up, and the time of each
/// `NextFrame` it asked for; it gets back what to push and whether to ask
/// for another. The clock runs only while the lens is flattening or
/// rebounding, so a long hold and an idle lens cost no frames.
///
/// A press that lands during a rebound starts over from a lens at rest.
/// With `reduced_motion` the lens goes flat on the press and back on the
/// release, with no ring and no clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct LensPress {
    phase: LensPressPhase,
    /// The first tick's time in this phase, once it has come.
    started: Option<f64>,
    /// How flat the lens is, 0..1: what a release rebounds from.
    flatten: f32,
}

impl LensPress {
    /// The finger went down on the lens.
    pub fn down(&mut self, reduced_motion: bool) -> LensPressStep {
        self.started = None;
        if reduced_motion {
            self.phase = LensPressPhase::Held;
            self.flatten = 1.0;
            return LensPressStep { response: Some(PressResponse::HELD), tick: false };
        }
        self.phase = LensPressPhase::Pressing;
        self.flatten = 0.0;
        LensPressStep { response: Some(PressResponse::REST), tick: true }
    }

    /// The finger came up. Nothing happens unless the lens was pressed, so
    /// a host that hears one release twice does not restart the rebound.
    pub fn up(&mut self, reduced_motion: bool) -> LensPressStep {
        if !matches!(self.phase, LensPressPhase::Pressing | LensPressPhase::Held) {
            return LensPressStep { response: None, tick: false };
        }
        self.started = None;
        if reduced_motion {
            self.phase = LensPressPhase::Rest;
            self.flatten = 0.0;
            return LensPressStep { response: Some(PressResponse::REST), tick: false };
        }
        self.phase = LensPressPhase::Releasing;
        LensPressStep { response: None, tick: true }
    }

    /// A frame the host asked for came, at `time` seconds. `None` when
    /// nothing is moving, so a stale frame asks for nothing.
    pub fn tick(&mut self, time: f64, curve: &LensPressCurve) -> Option<LensPressStep> {
        let pressing = match self.phase {
            LensPressPhase::Pressing => true,
            LensPressPhase::Releasing => false,
            _ => return None,
        };
        let age = time - *self.started.get_or_insert(time);
        let (response, live) = if pressing {
            let (response, live) = curve.pressed(age);
            self.flatten = response.flatten;
            if !live {
                self.phase = LensPressPhase::Held;
            }
            (response, live)
        } else {
            let step = curve.released(age, self.flatten);
            if !step.1 {
                self.phase = LensPressPhase::Rest;
                self.flatten = 0.0;
            }
            step
        };
        Some(LensPressStep { response: Some(response), tick: live })
    }

    /// Forget the press: the lens is at rest, and the response says so.
    pub fn rest(&mut self) -> PressResponse {
        *self = Self::default();
        PressResponse::REST
    }

    /// No press, no rebound.
    pub fn is_at_rest(&self) -> bool {
        self.phase == LensPressPhase::Rest
    }

    /// The finger is down: the lens is flattening, or flat and still.
    pub fn is_held(&self) -> bool {
        matches!(self.phase, LensPressPhase::Pressing | LensPressPhase::Held)
    }

    /// The lens is flattening or rebounding, so the clock is running.
    pub fn is_animating(&self) -> bool {
        matches!(self.phase, LensPressPhase::Pressing | LensPressPhase::Releasing)
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

#[cfg(test)]
mod press_tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    /// Frames at 60 Hz from `start` until the lens stops asking for one,
    /// returning every step and the time of the last tick.
    fn run(press: &mut LensPress, curve: &LensPressCurve, start: f64) -> (Vec<LensPressStep>, f64) {
        let mut steps = Vec::new();
        let mut time = start;
        loop {
            let step = press.tick(time, curve).expect("a frame was asked for, so the lens is moving");
            steps.push(step);
            if !step.tick {
                return (steps, time);
            }
            time += 1.0 / 60.0;
            assert!(time < start + 10.0, "the clock never stopped");
        }
    }

    #[test]
    fn a_press_eases_the_lens_flat_over_the_settle_time() {
        let curve = LensPressCurve::default();
        let (at_zero, live) = curve.pressed(0.0);
        assert!(live);
        assert_eq!(at_zero.flatten, 0.0);
        assert_eq!(at_zero.ripple_strength, 1.0, "the ring starts at full strength");

        let (half, live) = curve.pressed(0.39);
        assert!(live);
        assert!(close(half.flatten, 0.5), "half the settle time is half flat on a smoothstep: {}", half.flatten);

        let (settled, live) = curve.pressed(0.78);
        assert!(close(settled.flatten, 1.0), "flat by {} s: {}", curve.press_secs, settled.flatten);
        assert!(live, "the ring outlives the settle");
        assert!(close(curve.pressed(0.95).0.flatten, 1.0), "and it stays flat");

        let (held, live) = curve.pressed(1.1);
        assert!(!live, "the clock is still running after the ring has gone");
        assert_eq!(held, PressResponse::HELD);
    }

    #[test]
    fn the_ring_fades_over_its_life() {
        let curve = LensPressCurve::default();
        let mut last = f32::MAX;
        for i in 0..=21 {
            let age = i as f64 * 0.05;
            let (response, live) = curve.pressed(age);
            assert!(live, "{age} s is inside the ring's life and its tail");
            assert!(response.ripple_strength <= last, "the ring grew back at {age} s");
            assert_eq!(response.ripple_age, age as f32, "the shader's clock is the press's age");
            last = response.ripple_strength;
        }
        let (gone, live) = curve.pressed(curve.ripple_secs);
        assert_eq!(gone.ripple_strength, 0.0, "nothing left at {} s", curve.ripple_secs);
        assert!(live, "one more frame draws the lens with no ring in it");
        assert!(!curve.pressed(curve.press_stops_at()).1);
        assert!(close(curve.press_stops_at() as f32, 1.08));
        assert!(close(curve.release_stops_at() as f32, 1.08));
    }

    /// The curve's knobs have wide ranges, but the shader has fixed times of
    /// its own: the ring takes 0.88 s to cross the lens and is drawn for
    /// 1.05 s at most. The clock follows whichever ends last, so a slow press
    /// does not snap flat, a short ring does not cut the rebound off half
    /// way across, and a long one does not tick on with nothing to draw.
    #[test]
    fn the_clock_runs_until_the_shader_has_nothing_left_to_move() {
        let slow = LensPressCurve { press_secs: 2.0, ..LensPressCurve::default() };
        assert!(close(slow.press_stops_at() as f32, 2.03));
        let (last, live) = slow.pressed(2.02);
        assert!(live);
        assert!(last.flatten > 0.999, "the last frame of a slow press is not flat: {}", last.flatten);
        assert_eq!(slow.release_stops_at(), LensPressCurve::default().release_stops_at(), "a release does not wait on the press");

        let short = LensPressCurve { ripple_secs: 0.3, ..LensPressCurve::default() };
        assert!(short.released(0.8, 1.0).1, "a short ring stopped the rebound before it crossed the lens");
        assert!(short.pressed(0.8).1, "a short ring stopped the flatten before it crossed the lens");
        assert!(!short.released(LensPressCurve::RING_TRAVEL_SECS + LensPressCurve::TAIL_SECS, 1.0).1);
        assert_eq!(short.pressed(0.5).0.ripple_strength, 0.0, "the ring itself is gone at 0.3 s");

        let long = LensPressCurve { ripple_secs: 3.0, ..LensPressCurve::default() };
        assert!(close(long.press_stops_at() as f32, 1.08), "ticked on past the shader's ring: {}", long.press_stops_at());
        assert!(close(long.release_stops_at() as f32, 1.08));
    }

    #[test]
    fn a_release_rebounds_under_a_weaker_ring() {
        let curve = LensPressCurve::default();
        let (release, live) = curve.released(0.0, 0.7);
        assert!(live);
        assert!(close(release.flatten, -0.7), "the rebound starts from as flat as the lens was: {}", release.flatten);
        for age in [0.0, 0.3, 0.6, 0.9] {
            let pressed = curve.pressed(age).0.ripple_strength;
            let released = curve.released(age, 1.0).0.ripple_strength;
            assert!(released < pressed, "the release ring is not weaker at {age} s");
        }
        assert!(close(curve.released(0.0, 1.0).0.ripple_strength, 0.62));

        let (rest, live) = curve.released(1.1, 0.7);
        assert!(!live, "the rebound stops the clock too");
        assert_eq!(rest, PressResponse::REST);
        assert_eq!(curve.released(0.0, 3.0).0.flatten, -1.0, "a restore past flat is flat");
    }

    #[test]
    fn the_clock_stops_after_the_ring_is_gone_held_or_released() {
        let curve = LensPressCurve::default();
        let mut press = LensPress::default();
        assert!(press.is_at_rest());
        assert_eq!(press.tick(0.0, &curve), None, "an idle lens asks for no frame");

        let down = press.down(false);
        assert_eq!(down, LensPressStep { response: Some(PressResponse::REST), tick: true });
        assert!(press.is_held());
        let (steps, stopped) = run(&mut press, &curve, 5.0);
        assert!(stopped - 5.0 >= curve.press_stops_at() && stopped - 5.0 < curve.press_stops_at() + 1.0 / 60.0);
        assert_eq!(steps.last().unwrap().response, Some(PressResponse::HELD));
        assert!(!press.is_animating() && !press.is_at_rest(), "held, and still");
        assert!(press.is_held());
        assert_eq!(press.tick(9.0, &curve), None, "a long hold costs no frames");

        let up = press.up(false);
        assert_eq!(up, LensPressStep { response: None, tick: true });
        assert!(!press.is_held() && press.is_animating());
        let (steps, stopped) = run(&mut press, &curve, 20.0);
        let first = steps[0].response.unwrap();
        assert_eq!(first.flatten, -1.0, "released from flat, it rebounds from flat");
        assert!(stopped - 20.0 < curve.release_stops_at() + 1.0 / 60.0);
        assert_eq!(steps.last().unwrap().response, Some(PressResponse::REST));
        assert!(press.is_at_rest());
        assert_eq!(press.tick(30.0, &curve), None);
        assert_eq!(press.up(false), LensPressStep { response: None, tick: false }, "a second release does nothing");
    }

    #[test]
    fn a_quick_click_rebounds_from_as_flat_as_it_got() {
        let curve = LensPressCurve::default();
        let mut press = LensPress::default();
        press.down(false);
        press.tick(1.0, &curve);
        let partway = press.tick(1.2, &curve).unwrap().response.unwrap();
        assert!(partway.flatten > 0.0 && partway.flatten < 1.0);
        press.up(false);
        let rebound = press.tick(1.25, &curve).unwrap().response.unwrap();
        assert!(close(rebound.flatten, -partway.flatten));
        assert_eq!(rebound.ripple_age, 0.0, "the release ring has its own clock");

        // Pressed again mid-rebound, it starts over from rest.
        assert_eq!(press.down(false).response, Some(PressResponse::REST));
        assert_eq!(press.tick(1.5, &curve).unwrap().response.unwrap().flatten, 0.0);
    }

    #[test]
    fn reduced_motion_skips_the_ring_and_the_clock() {
        let curve = LensPressCurve::default();
        let mut press = LensPress::default();
        assert_eq!(press.down(true), LensPressStep { response: Some(PressResponse::HELD), tick: false });
        assert_eq!(press.tick(1.0, &curve), None);
        assert_eq!(press.up(true), LensPressStep { response: Some(PressResponse::REST), tick: false });
        assert_eq!(press.tick(2.0, &curve), None);
        assert!(press.is_at_rest());
    }

    /// The water lens is DSL, which the Rust compiler never reads, and a
    /// shader that fails to compile is not an error anywhere: the draw is
    /// skipped and the surface paints nothing. Building one of each preset
    /// and reading the shader-error slot back turns that into a failure.
    #[test]
    fn the_water_lens_preset_builds_and_its_shader_compiles() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let built = cx.with_vm(|vm| {
            crate::script_mod(vm);
            let _ = crate::makepad_draw::makepad_platform::shader_error::take();
            let widgets = vm.module(id!(widgets));
            ["RippleLensRoundedView", "LensedRoundedView", "GaussRoundedView"]
                .iter()
                .map(|name| {
                    let value = vm.bx.heap.value(widgets, LiveId::from_str(name).into(), NoTrap);
                    assert!(value.as_object().is_some(), "no template {name}");
                    WidgetRef::script_from_value(vm, value)
                })
                .collect::<Vec<_>>()
        });
        assert_eq!(
            crate::makepad_draw::makepad_platform::shader_error::take(),
            None,
            "a draw shader failed to compile"
        );
        for view in &built {
            assert!(view.borrow::<GaussRoundedView>().is_some(), "every preset is the one Rust widget");
        }
        // And the water lens compiled a draw of its own: the press inputs, the
        // colour split and the base's Reduce Transparency switch are all in it.
        let lens = built[0].borrow::<GaussRoundedView>().unwrap();
        for id in [live_id!(ripple_age), live_id!(press_flatten), live_id!(diffraction_strength), live_id!(opaque_surface)] {
            assert!(
                lens.view.draw_bg.draw_vars.uniform_range(&cx, id).is_some(),
                "the water lens has no {id} input"
            );
        }
    }
}
