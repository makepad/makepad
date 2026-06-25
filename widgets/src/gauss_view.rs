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

#[derive(Default)]
struct GaussWindowEntry {
    generation: u64,
    requested_last_frame: bool,
    requested_this_frame: bool,
    capture_active: bool,
    snapshot: Option<GaussBlurSnapshot>,
}

#[derive(Default)]
struct GaussWindowGlobal {
    windows: Vec<Option<GaussWindowEntry>>,
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
}

pub(crate) fn window_wants_gauss_capture(cx: &mut Cx, window_id: WindowId) -> bool {
    cx.global::<GaussWindowGlobal>()
        .entry_mut(window_id)
        .requested_last_frame
}

pub(crate) fn begin_window_gauss_frame(
    cx: &mut Cx,
    window_id: WindowId,
    capture_active: bool,
    snapshot: Option<GaussBlurSnapshot>,
) {
    let entry = cx.global::<GaussWindowGlobal>().entry_mut(window_id);
    entry.capture_active = capture_active;
    entry.requested_this_frame = false;
    // Only replace the snapshot when we actually re-captured the scene this frame. On frames
    // that skip the capture (e.g. a hover-only overlay repaint), keep the last good snapshot so
    // the glass keeps refracting it instead of blinking to its flat fallback colour.
    if capture_active {
        entry.snapshot = snapshot;
    }
}

pub(crate) fn finish_window_gauss_frame(cx: &mut Cx, window_id: WindowId) -> bool {
    let entry = cx.global::<GaussWindowGlobal>().entry_mut(window_id);
    let capture_changed = entry.requested_last_frame != entry.requested_this_frame;
    entry.requested_last_frame = entry.requested_this_frame;
    entry.requested_this_frame = false;
    entry.capture_active = false;
    // Intentionally do NOT drop `entry.snapshot` here: a glass overlay can repaint on its own
    // (hover/press) without the window running a full capture pass. Keeping the last snapshot
    // means those repaints still refract the previously captured scene (≤1 capture stale, which
    // is invisible) rather than flickering. It is refreshed whenever a full frame captures again.
    capture_changed
}

pub fn request_window_gauss(cx: &mut Cx2d) -> Option<GaussBlurSnapshot> {
    if !cx.is_drawing_overlay() {
        return None;
    }
    let window_id = cx.get_current_window_id()?;
    let entry = cx.global::<GaussWindowGlobal>().entry_mut(window_id);
    entry.requested_this_frame = true;
    // Return the last captured snapshot regardless of whether THIS frame ran a capture pass.
    // This keeps the lensing stable across overlay-only repaints (the source of the hover
    // flicker). `requested_this_frame` still drives a fresh capture on the next full frame.
    entry.snapshot.clone()
}

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
            ripple_start: uniform(-1000.0)
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
            shadow_radius: uniform(28.0)
            shadow_offset: uniform(vec2(0.0, 10.0))

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

            sample_level: fn(level: float, uv: vec2) -> vec4 {
                let source_uv = vec2(uv.x, mix(uv.y, 1.0 - uv.y, self.source_y_flip))
                let safe_uv = clamp(source_uv, vec2(0.0, 0.0), vec2(1.0, 1.0))
                if level < 0.5 {
                    return self.scene_texture.sample_as_bgra(safe_uv)
                }
                if level < 1.5 {
                    let size = self.mip0_texture.size()
                    let texel = vec2(1.0 / max(size.x, 1.0), 1.0 / max(size.y, 1.0))
                    return self.mip0_texture.sample_as_bgra(safe_uv) * 0.20
                        + self.mip0_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip0_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip0_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip0_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip0_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip0_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip0_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip0_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip0_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(2.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                        + self.mip0_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-2.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                        + self.mip0_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, 2.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                        + self.mip0_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, -2.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                }
                if level < 2.5 {
                    let size = self.mip1_texture.size()
                    let texel = vec2(1.0 / max(size.x, 1.0), 1.0 / max(size.y, 1.0))
                    return self.mip1_texture.sample_as_bgra(safe_uv) * 0.20
                        + self.mip1_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip1_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip1_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip1_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip1_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip1_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip1_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip1_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip1_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(2.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                        + self.mip1_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-2.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                        + self.mip1_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, 2.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                        + self.mip1_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, -2.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                }
                if level < 3.5 {
                    let size = self.mip2_texture.size()
                    let texel = vec2(1.0 / max(size.x, 1.0), 1.0 / max(size.y, 1.0))
                    return self.mip2_texture.sample_as_bgra(safe_uv) * 0.20
                        + self.mip2_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip2_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip2_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip2_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip2_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip2_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip2_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip2_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip2_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(2.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                        + self.mip2_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-2.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                        + self.mip2_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, 2.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                        + self.mip2_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, -2.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                }
                if level < 4.5 {
                    let size = self.mip3_texture.size()
                    let texel = vec2(1.0 / max(size.x, 1.0), 1.0 / max(size.y, 1.0))
                    return self.mip3_texture.sample_as_bgra(safe_uv) * 0.20
                        + self.mip3_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip3_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip3_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip3_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip3_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip3_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip3_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip3_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip3_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(2.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                        + self.mip3_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-2.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                        + self.mip3_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, 2.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                        + self.mip3_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, -2.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                }
                if level < 5.5 {
                    let size = self.mip4_texture.size()
                    let texel = vec2(1.0 / max(size.x, 1.0), 1.0 / max(size.y, 1.0))
                    return self.mip4_texture.sample_as_bgra(safe_uv) * 0.20
                        + self.mip4_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip4_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip4_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip4_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                        + self.mip4_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip4_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip4_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip4_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                        + self.mip4_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(2.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                        + self.mip4_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-2.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                        + self.mip4_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, 2.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                        + self.mip4_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, -2.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                }
                let size = self.mip5_texture.size()
                let texel = vec2(1.0 / max(size.x, 1.0), 1.0 / max(size.y, 1.0))
                return self.mip5_texture.sample_as_bgra(safe_uv) * 0.20
                    + self.mip5_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                    + self.mip5_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                    + self.mip5_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                    + self.mip5_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.12
                    + self.mip5_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                    + self.mip5_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                    + self.mip5_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(1.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                    + self.mip5_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-1.0, -1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.06
                    + self.mip5_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(2.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                    + self.mip5_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(-2.0, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                    + self.mip5_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, 2.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
                    + self.mip5_texture.sample_as_bgra(clamp(safe_uv + texel * vec2(0.0, -2.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.02
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

            rounded_edge_lens: fn(shape: float) -> float {
                let edge = clamp(1.0 - abs(shape) / max(self.lensing_width, 1.0), 0.0, 1.0)
                return pow(edge, 1.45) * clamp(self.lensing_effect, 0.0, 1.0)
            }

            lensed_uv: fn(uv: vec2, shape: float) -> vec2 {
                let normal = self.rounded_edge_normal(shape)
                let lens = self.rounded_edge_lens(shape)
                let offset = normal * (lens * self.lensing_strength) / max(self.source_size, vec2(1.0, 1.0))
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
                let noise = (
                    Math.random_2d(
                        screen_pos + vec2(self.draw_pass.time * 31.0, self.draw_pass.time * 17.0)
                    ) - 0.5
                ) * self.noise_strength
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
                let ripple_age = max(self.draw_pass.time - self.ripple_start, 0.0)
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
                let base_offset = normal * (lens * self.lensing_strength) / max(self.source_size, vec2(1.0, 1.0)) + water_offset
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
                let noise = (
                    Math.random_2d(
                        screen_pos + vec2(self.draw_pass.time * 31.0, self.draw_pass.time * 17.0)
                    ) - 0.5
                ) * self.noise_strength
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

#[derive(Script, ScriptHook, Widget)]
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

    pub fn set_press_response(
        &mut self,
        cx: &mut Cx,
        flatten: f32,
        ripple_start: f32,
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
            .set_uniform(cx, live_id!(ripple_start), &[ripple_start]);
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
        self.view.draw_bg.draw_vars.set_uniform_on_area(
            cx,
            live_id!(ripple_start),
            &[ripple_start],
        );
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
        ripple_start: f32,
        ripple_strength: f32,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_press_response(cx, flatten, ripple_start, ripple_strength);
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
