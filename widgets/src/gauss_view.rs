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
use crate::{makepad_derive_widget::*, makepad_draw::*, view::View, widget::*};

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
}

impl GaussWindowGlobal {
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
    let window_id = cx.get_current_window_id()?;
    cx.global::<GaussWindowGlobal>().request(window_id)
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
            blur_level: uniform(5.0)
            gradient_blur_edge: uniform(0.0)
            gradient_blur_edge_width: uniform(0.16)
            gradient_blur_power: uniform(1.25)
            lensing_effect: uniform(0.0)
            lensing_strength: uniform(12.0)
            lensing_width: uniform(22.0)
            press_flatten: uniform(0.0)
            // PERF LAW: the click ripple's clock is fed by the widget, NOT by
            // `draw_pass.time`. Any shader that reads `draw_pass.time` is flagged
            // `uses_time` (platform/src/draw_shader.rs) and arms `demo_time_repaint`
            // every frame it draws, which pins the whole window at display rate for
            // as long as the glass is on screen. `ripple_age` is elapsed seconds since
            // the press, pushed from Rust on a NextFrame chain that only runs while a
            // ripple is live (~1.1s). Default 1000.0 = no ripple.
            ripple_age: uniform(1000.0)
            ripple_strength: uniform(0.0)
            corner_radius: instance(14.0)
            tint_color: instance(#b8b8b8)
            tint_alpha: uniform(0.08)
            surface_alpha: uniform(0.88)
            border_color: instance(#fff)
            border_alpha: instance(0.36)
            border_width: instance(1.0)
            specular_strength: instance(0.10)
            noise_strength: instance(0.012)
            fallback_color: instance(#8c8c8c)
            shadow_color: instance(#0007)
            shadow_radius: uniform(14.0)
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

            rounded_edge_normal: fn(shape: float) -> vec2 {
                let gradient = vec2(dFdx(shape), dFdy(shape))
                if length(gradient) > 0.00001 {
                    return normalize(gradient)
                }
                return vec2(0.0, 1.0)
            }

            // Effective refraction band width: never wider than ~35% of the
            // surface's smaller side. Past that the whole surface becomes edge
            // distortion, which renders small discs (< ~32px) as smeared blobs.
            eff_lensing_width: fn() -> float {
                let cap = max(min(self.sdf_rect_size.x, self.sdf_rect_size.y) * 0.35, 1.0)
                return min(max(self.lensing_width, 1.0), cap)
            }

            // Scale factor for lensing strength when the band was capped, so
            // small surfaces also refract proportionally less.
            eff_lensing_scale: fn() -> float {
                return self.eff_lensing_width() / max(self.lensing_width, 1.0)
            }

            rounded_edge_lens: fn(shape: float) -> float {
                let edge = clamp(1.0 - abs(shape) / self.eff_lensing_width(), 0.0, 1.0)
                return pow(edge, 1.45) * clamp(self.lensing_effect, 0.0, 1.0)
            }

            lensed_uv: fn(uv: vec2, shape: float) -> vec2 {
                let normal = self.rounded_edge_normal(shape)
                let lens = self.rounded_edge_lens(shape)
                let offset = normal * (lens * self.lensing_strength * self.eff_lensing_scale()) / max(self.source_size, vec2(1.0, 1.0))
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
                    let v = GaussShadow.rounded_box_shadow(
                        vec2(m) + o
                        self.rect_size2 + o
                        self.pos * (self.rect_size3 + vec2(m))
                        self.shadow_radius * 0.5
                        self.corner_radius * 2.0
                    )
                    sdf.clear(self.shadow_color * v)
                }

                let screen_pos = self.rect_pos2 + self.pos * self.rect_size3
                let uv = screen_pos / max(self.source_size, vec2(1.0, 1.0))
                let blurred = self.sample_gauss(self.lensed_uv(uv, sdf.shape))
                let fallback = vec4(self.fallback_color.rgb, 1.0)
                let base = fallback.mix(blurred, self.has_gauss)

                let material = base.rgb.mix(self.tint_color.rgb, self.tint_alpha)
                let edge_uv = abs(self.pos * 2.0 - 1.0)
                let edge_gradient = clamp((edge_uv.x + edge_uv.y) * 0.5, 0.0, 1.0)
                let highlight = self.specular_strength * (0.55 * edge_gradient + 0.45 * (1.0 - self.pos.y))
                // Static banding dither, hashed from screen position only. It must NOT
                // depend on `draw_pass.time`: that flags the shader `uses_time` and pins
                // the window at display rate forever (see the ripple_age note above).
                // The grain is a de-banding device, not an animation - frozen looks the same.
                let noise = (Math.random_2d(screen_pos) - 0.5) * self.noise_strength
                let fill = vec4(material + highlight + noise, self.surface_alpha)

                sdf.fill_keep(fill)
                if self.border_width > 0.0 {
                    sdf.stroke(
                        vec4(self.border_color.rgb, self.border_alpha),
                        self.border_width
                    )
                }
                return sdf.result
            }
        }
    }

    mod.widgets.AppleGlassRoundedView = mod.widgets.GaussRoundedView{
        draw_bg +: {
            tint_alpha: 0.10
            surface_alpha: 0.74
            border_alpha: 0.62
            specular_strength: 0.16
            lensing_effect: 0.75
            lensing_strength: 14.0
            lensing_width: 22.0
            diffraction_strength: uniform(2.4)

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
                    sdf.clear(self.shadow_color * v)
                }

                let screen_pos = self.rect_pos2 + self.pos * self.rect_size3
                let uv = screen_pos / max(self.source_size, vec2(1.0, 1.0))
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
                let ripple_wave = wave * ripple_life * ripple_life * self.ripple_strength * wave_mask
                let ripple_slope = (-wave_delta / wave_width) * ripple_wave
                let ripple_dir = lens_pos / max(ripple_dist, 0.001)
                let press = clamp(self.press_flatten, 0.0, 1.0)
                let restore = clamp(-self.press_flatten, 0.0, 1.0)
                let wave_flatten = smoothstep(ripple_dist - 0.14, ripple_dist + 0.26, wave_center)
                let flatten = clamp(press * wave_flatten + restore * (1.0 - wave_flatten), 0.0, 1.0)
                let lift = restore * wave_flatten * (1.0 - wave_t) * 0.45
                let ripple_surface = ripple_slope * 0.85 + ripple_wave * 0.20
                let lens_depth = clamp(1.0 - flatten * 0.90 + lift * 0.55 + ripple_wave * 0.18, 0.0, 1.55)
                let diffraction_depth = clamp(1.0 - flatten * 0.76 + lift * 0.70 + (abs(ripple_surface) + ripple_wave) * 1.15, 0.0, 2.10)
                let lens = self.rounded_edge_lens(sdf.shape) * lens_depth
                let normal = self.rounded_edge_normal(sdf.shape)
                let water_offset = ripple_dir * (ripple_surface * 22.0) / max(self.source_size, vec2(1.0, 1.0))
                let base_offset = normal * (lens * self.lensing_strength * self.eff_lensing_scale()) / max(self.source_size, vec2(1.0, 1.0)) + water_offset
                let color_offset = normal * (lens * self.diffraction_strength * diffraction_depth) / max(self.source_size, vec2(1.0, 1.0))
                    + ripple_dir * ((ripple_surface + ripple_wave * 0.65) * self.diffraction_strength * 4.5) / max(self.source_size, vec2(1.0, 1.0))
                let uv_g = clamp(uv + base_offset, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let uv_r = clamp(uv_g + color_offset, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let uv_b = clamp(uv_g - color_offset, vec2(0.0, 0.0), vec2(1.0, 1.0))
                let sample_r = self.sample_gauss(uv_r)
                let sample_g = self.sample_gauss(uv_g)
                let sample_b = self.sample_gauss(uv_b)
                let refracted = vec4(sample_r.r, sample_g.g, sample_b.b, (sample_r.a + sample_g.a + sample_b.a) * 0.3333333)
                let fallback = vec4(self.fallback_color.rgb, 1.0)
                let base = fallback.mix(refracted, self.has_gauss)

                let edge = self.rounded_edge_lens(sdf.shape)
                let material = base.rgb.mix(self.tint_color.rgb, self.tint_alpha)
                let edge_uv = abs(self.pos * 2.0 - 1.0)
                let edge_gradient = clamp((edge_uv.x + edge_uv.y) * 0.5, 0.0, 1.0)
                let ripple_highlight = ripple_wave * 0.11
                let sparkle = edge * self.diffraction_strength * 0.004 * (1.0 - flatten * 0.45)
                let highlight = self.specular_strength * (0.45 * edge_gradient + 0.55 * edge + 0.30 * (1.0 - self.pos.y)) * (1.0 - flatten * 0.28) + ripple_highlight
                // Static banding dither - see the note in GaussRoundedView's pixel().
                let noise = (Math.random_2d(screen_pos) - 0.5) * self.noise_strength
                let fill_alpha = mix(self.surface_alpha, 1.0, self.has_gauss)
                let fill = vec4(material + highlight + sparkle + noise, fill_alpha)

                sdf.fill_keep(fill)
                if self.border_width > 0.0 {
                    sdf.stroke(
                        vec4(self.border_color.rgb, self.border_alpha),
                        self.border_width
                    )
                }
                return sdf.result
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
                    sdf.clear(self.shadow_color * v)
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
                return sdf.result
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
