use crate::quad::{
    evaluate_clip_chain_no_rect, set_clip_masks_evaluated, set_clip_planes_evaluated, MpClipBasis,
};
use crate::scene::{MpEvaluatedMask, MpMaskExec, MpMaskKind};
use crate::*;

pub type MpPrimitiveTransformId = usize;
pub type MpPrimitiveClipChainId = usize;
pub type MpPrimitiveBatchId = usize;

/// Transform from primitive/picture origin space to the draw-list-local
/// coordinate system of the browser-scene host.
///
/// **Spatial vocabulary:**
/// - *origin space*: the coordinate system of each retained primitive or
///   picture after browser-scene lowering.
/// - *draw-list world space*: after applying the outer Makepad draw-list
///   `view_transform` (set at draw time, not stored here).
/// - *clip space*: after the current pass view-projection.
///
/// This struct stores only the `scene_from_origin` transform. The full
/// draw-time basis `clip_from_origin = clip_from_world * world_from_scene *
/// scene_from_origin` is derived at compositor draw time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MpPrimitiveTransform {
    /// Transform from origin space to the browser-scene host's local
    /// coordinate system. Formerly named `projected_transform`.
    pub scene_from_origin: Mat4f,
    pub backface_visibility: MpBackfaceVisibility,
}

/// A single declarative clip entry stored in origin space.
///
/// All data here is placement-independent. The compositor evaluates these
/// entries at draw time using the explicit full basis.
///
/// **Forbidden retained data:** clip-space planes, world-space planes that
/// assume active draw-list placement, matrices that already encode pass or
/// draw-list state.
#[derive(Clone, Debug, PartialEq)]
pub enum MpPrimitiveClipEntry {
    /// Rounded-rect mask in mask-local coordinates. The
    /// `mask_local_from_origin` matrix maps from origin space to the
    /// coordinate system in which `rect` and `radius` are defined.
    RoundedRect {
        rect: Rect,
        radius: Vec4f,
        mask_local_from_origin: Mat4f,
    },
    /// Image mask in mask-local coordinates.
    ImageMask {
        rect: Rect,
        mask_local_from_origin: Mat4f,
    },
    /// Origin-space plane set. Each plane `(a, b, c, d)` defines a half-space
    /// `a*x + b*y + c*z + d >= 0` in origin space.
    PlaneSet {
        planes: Vec<Vec4f>,
    },
}

/// Declarative clip chain stored in origin space. Self-contained by the time
/// it reaches the compositor — no back-references to browser-scene graph
/// structure.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MpPrimitiveClipChain {
    /// Axis-aligned clip rect in origin space (fast path).
    pub origin_clip_rect: Option<Rect>,
    /// Additional clip entries in origin space.
    pub entries: Vec<MpPrimitiveClipEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MpBrowserGradientStop {
    pub offset: f32,
    pub color: Vec4f,
}

#[derive(Clone, Debug)]
pub enum MpBrowserPrimitiveKind {
    SolidRect { color: Vec4f },
    RoundedRect { color: Vec4f, radius: Vec4f },
    Border { color: Vec4f, width: f32, radius: Vec4f },
    Image { texture: Texture },
    RepeatingImage { texture: Texture, tile_size: Vec2f },
    LinearGradient {
        start: Vec2f,
        end: Vec2f,
        repeating: bool,
        stops: Vec<MpBrowserGradientStop>,
    },
    RadialGradient {
        center: Vec2f,
        radius: Vec2f,
        repeating: bool,
        stops: Vec<MpBrowserGradientStop>,
    },
    ConicGradient {
        center: Vec2f,
        start_angle_rad: f32,
        repeating: bool,
        stops: Vec<MpBrowserGradientStop>,
    },
    BoxShadow {
        color: Vec4f,
        box_offset: Vec2f,
        box_size: Vec2f,
        sigma: f32,
        corner_radius_px: f32,
        inset: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MpBrowserPrimitiveClass {
    SolidRect,
    RoundedRect,
    Border,
    Image,
    RepeatingImage,
    LinearGradient,
    RadialGradient,
    ConicGradient,
    BoxShadow,
}

#[derive(Clone, Debug)]
pub struct MpBrowserPrimitive {
    pub local_rect: Rect,
    pub transform_id: MpPrimitiveTransformId,
    pub clip_chain_id: MpPrimitiveClipChainId,
    pub kind: MpBrowserPrimitiveKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MpPrimitiveBatch {
    pub primitive_class: MpBrowserPrimitiveClass,
    pub transform_id: MpPrimitiveTransformId,
    pub clip_chain_id: MpPrimitiveClipChainId,
    pub primitive_range: std::ops::Range<usize>,
}

#[derive(Clone, Debug)]
pub struct MpBrowserPrimitiveScene {
    pub host_rect: Rect,
    pub transforms: Vec<MpPrimitiveTransform>,
    pub clip_chains: Vec<MpPrimitiveClipChain>,
    pub primitives: Vec<MpBrowserPrimitive>,
    pub batches: Vec<MpPrimitiveBatch>,
    pub draw_order: Vec<MpPrimitiveBatchId>,
    pub(crate) batch_merge_barrier: bool,
}

impl MpBrowserPrimitiveKind {
    pub fn primitive_class(&self) -> MpBrowserPrimitiveClass {
        match self {
            Self::SolidRect { .. } => MpBrowserPrimitiveClass::SolidRect,
            Self::RoundedRect { .. } => MpBrowserPrimitiveClass::RoundedRect,
            Self::Border { .. } => MpBrowserPrimitiveClass::Border,
            Self::Image { .. } => MpBrowserPrimitiveClass::Image,
            Self::RepeatingImage { .. } => MpBrowserPrimitiveClass::RepeatingImage,
            Self::LinearGradient { .. } => MpBrowserPrimitiveClass::LinearGradient,
            Self::RadialGradient { .. } => MpBrowserPrimitiveClass::RadialGradient,
            Self::ConicGradient { .. } => MpBrowserPrimitiveClass::ConicGradient,
            Self::BoxShadow { .. } => MpBrowserPrimitiveClass::BoxShadow,
        }
    }
}

impl MpBrowserPrimitiveScene {
    pub fn new(host_rect: Rect) -> Self {
        Self {
            host_rect,
            transforms: vec![MpPrimitiveTransform {
                scene_from_origin: Mat4f::translation(vec3(
                    host_rect.pos.x as f32,
                    host_rect.pos.y as f32,
                    0.0,
                )),
                backface_visibility: MpBackfaceVisibility::Visible,
            }],
            clip_chains: vec![MpPrimitiveClipChain::default()],
            primitives: Vec::new(),
            batches: Vec::new(),
            draw_order: Vec::new(),
            batch_merge_barrier: false,
        }
    }

    pub fn push_transform(&mut self, transform: MpPrimitiveTransform) -> MpPrimitiveTransformId {
        let id = self.transforms.len();
        self.transforms.push(transform);
        id
    }

    pub fn push_clip_chain(&mut self, clip_chain: MpPrimitiveClipChain) -> MpPrimitiveClipChainId {
        let id = self.clip_chains.len();
        self.clip_chains.push(clip_chain);
        id
    }

    pub fn push_primitive(&mut self, primitive: MpBrowserPrimitive) -> usize {
        let primitive_id = self.primitives.len();
        let primitive_class = primitive.kind.primitive_class();
        let transform_id = primitive.transform_id;
        let clip_chain_id = primitive.clip_chain_id;
        self.primitives.push(primitive);
        match self.batches.last_mut() {
            Some(batch)
                if !self.batch_merge_barrier
                    && batch.primitive_class == primitive_class
                    && batch.transform_id == transform_id
                    && batch.clip_chain_id == clip_chain_id =>
            {
                batch.primitive_range.end += 1;
            }
            _ => {
                let batch_id = self.batches.len();
                self.batches.push(MpPrimitiveBatch {
                    primitive_class,
                    transform_id,
                    clip_chain_id,
                    primitive_range: primitive_id..primitive_id + 1,
                });
                self.draw_order.push(batch_id);
            }
        }
        self.batch_merge_barrier = false;
        primitive_id
    }

    pub fn break_batch(&mut self) {
        self.batch_merge_barrier = true;
    }

    pub fn transform(&self, id: MpPrimitiveTransformId) -> Option<&MpPrimitiveTransform> {
        self.transforms.get(id)
    }

    pub fn clip_chain(&self, id: MpPrimitiveClipChainId) -> Option<&MpPrimitiveClipChain> {
        self.clip_chains.get(id)
    }

    pub fn is_empty(&self) -> bool {
        self.primitives.is_empty()
    }
}

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.draw.DrawBrowserPrimitive = mod.std.set_type_default() do #(DrawBrowserPrimitive::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_texture: texture_2d(float)
        primitive_kind: uniform(float(0.0))
        image_repeating: uniform(float(0.0))
        repeat_tile_size: uniform(vec2(1.0, 1.0))
        color: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        border_width: uniform(float(1.0))
        border_radius: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        local_clip_enabled: uniform(float(0.0))
        local_clip_rect: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_count: uniform(float(0.0))
        clip_plane_0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_count: uniform(float(0.0))
        clip_mask_type_0: uniform(float(0.0))
        clip_mask_type_1: uniform(float(0.0))
        clip_mask_type_2: uniform(float(0.0))
        clip_mask_type_3: uniform(float(0.0))
        clip_mask_rect_0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_rect_1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_rect_2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_rect_3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_radius_0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_radius_1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_radius_2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_radius_3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_mask_matrix_0: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        clip_mask_matrix_1: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        clip_mask_matrix_2: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        clip_mask_matrix_3: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        grad_param0: uniform(float(0.0))
        grad_param1: uniform(float(0.0))
        grad_param2: uniform(float(1.0))
        grad_param3: uniform(float(1.0))
        grad_repeating: uniform(float(0.0))
        grad_stop_count: uniform(float(2.0))
        grad_stop0_color: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        grad_stop0_pos: uniform(float(0.0))
        grad_stop1_color: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        grad_stop1_pos: uniform(float(1.0))
        grad_stop2_color: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        grad_stop2_pos: uniform(float(0.0))
        grad_stop3_color: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        grad_stop3_pos: uniform(float(0.0))
        grad_stop4_color: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        grad_stop4_pos: uniform(float(0.0))
        grad_stop5_color: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        grad_stop5_pos: uniform(float(0.0))
        grad_stop6_color: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        grad_stop6_pos: uniform(float(0.0))
        grad_stop7_color: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        grad_stop7_pos: uniform(float(0.0))
        box_offset: uniform(vec2(0.0, 0.0))
        box_size: uniform(vec2(0.0, 0.0))
        shadow_sigma: uniform(float(0.001))
        shadow_corner: uniform(float(0.0))
        shadow_inset: uniform(float(0.0))

        local_uv: varying(vec2f)
        local_pos: varying(vec2f)
        clip_space: varying(vec4f)

        clip_projected: fn() -> float {
            if self.clip_plane_count > 0.5 && dot(self.clip_space, self.clip_plane_0) < 0.0 { return 0.0 }
            if self.clip_plane_count > 1.5 && dot(self.clip_space, self.clip_plane_1) < 0.0 { return 0.0 }
            if self.clip_plane_count > 2.5 && dot(self.clip_space, self.clip_plane_2) < 0.0 { return 0.0 }
            if self.clip_plane_count > 3.5 && dot(self.clip_space, self.clip_plane_3) < 0.0 { return 0.0 }
            if self.clip_plane_count > 4.5 && dot(self.clip_space, self.clip_plane_4) < 0.0 { return 0.0 }
            if self.clip_plane_count > 5.5 && dot(self.clip_space, self.clip_plane_5) < 0.0 { return 0.0 }
            if self.clip_plane_count > 6.5 && dot(self.clip_space, self.clip_plane_6) < 0.0 { return 0.0 }
            if self.clip_plane_count > 7.5 && dot(self.clip_space, self.clip_plane_7) < 0.0 { return 0.0 }
            return 1.0
        }

        rect_mask_alpha: fn(rect: vec4, local: vec2f) -> float {
            if local.x < rect.x || local.y < rect.y || local.x > rect.x + rect.z || local.y > rect.y + rect.w {
                return 0.0
            }
            return 1.0
        }

        rounded_mask_alpha: fn(rect: vec4, radius: vec4, local: vec2f) -> float {
            if self.rect_mask_alpha(rect, local) < 0.5 {
                return 0.0
            }
            let min_x = rect.x
            let min_y = rect.y
            let max_x = rect.x + rect.z
            let max_y = rect.y + rect.w
            let tl = max(radius.x, 0.0)
            if tl > 0.0 && local.x < min_x + tl && local.y < min_y + tl {
                let d = local - vec2(min_x + tl, min_y + tl)
                if length(d) <= tl { return 1.0 }
                return 0.0
            }
            let tr = max(radius.y, 0.0)
            if tr > 0.0 && local.x > max_x - tr && local.y < min_y + tr {
                let d = local - vec2(max_x - tr, min_y + tr)
                if length(d) <= tr { return 1.0 }
                return 0.0
            }
            let br = max(radius.z, 0.0)
            if br > 0.0 && local.x > max_x - br && local.y > max_y - br {
                let d = local - vec2(max_x - br, max_y - br)
                if length(d) <= br { return 1.0 }
                return 0.0
            }
            let bl = max(radius.w, 0.0)
            if bl > 0.0 && local.x < min_x + bl && local.y > max_y - bl {
                let d = local - vec2(min_x + bl, max_y - bl)
                if length(d) <= bl { return 1.0 }
                return 0.0
            }
            return 1.0
        }

        clip_mask_alpha: fn(mask_type: float, rect: vec4, radius: vec4, mask_matrix: mat4x4f) -> float {
            let local_h = mask_matrix * self.clip_space
            if abs(local_h.w) <= 0.000001 {
                return 0.0
            }
            let local = local_h.xy / local_h.w
            if mask_type < 0.5 {
                return 1.0
            }
            if mask_type < 1.5 {
                return self.rounded_mask_alpha(rect, radius, local)
            }
            return self.rect_mask_alpha(rect, local)
        }

        accumulated_mask_alpha: fn() -> float {
            var mask = 1.0
            if self.clip_mask_count > 0.5 {
                mask *= self.clip_mask_alpha(self.clip_mask_type_0, self.clip_mask_rect_0, self.clip_mask_radius_0, self.clip_mask_matrix_0)
            }
            if self.clip_mask_count > 1.5 {
                mask *= self.clip_mask_alpha(self.clip_mask_type_1, self.clip_mask_rect_1, self.clip_mask_radius_1, self.clip_mask_matrix_1)
            }
            if self.clip_mask_count > 2.5 {
                mask *= self.clip_mask_alpha(self.clip_mask_type_2, self.clip_mask_rect_2, self.clip_mask_radius_2, self.clip_mask_matrix_2)
            }
            if self.clip_mask_count > 3.5 {
                mask *= self.clip_mask_alpha(self.clip_mask_type_3, self.clip_mask_rect_3, self.clip_mask_radius_3, self.clip_mask_matrix_3)
            }
            return mask
        }

        local_clip_alpha: fn() -> float {
            if self.local_clip_enabled < 0.5 {
                return 1.0
            }
            let rect = self.local_clip_rect
            if self.local_pos.x < rect.x || self.local_pos.y < rect.y || self.local_pos.x > rect.x + rect.z || self.local_pos.y > rect.y + rect.w {
                return 0.0
            }
            return 1.0
        }

        gradient_color: fn(t_in: float) -> vec4 {
            var t = t_in
            if self.grad_repeating > 0.5 {
                let first = self.grad_stop0_pos
                var last = self.grad_stop1_pos
                if self.grad_stop_count >= 3.0 { last = self.grad_stop2_pos }
                if self.grad_stop_count >= 4.0 { last = self.grad_stop3_pos }
                if self.grad_stop_count >= 5.0 { last = self.grad_stop4_pos }
                if self.grad_stop_count >= 6.0 { last = self.grad_stop5_pos }
                if self.grad_stop_count >= 7.0 { last = self.grad_stop6_pos }
                if self.grad_stop_count >= 8.0 { last = self.grad_stop7_pos }
                let range = last - first
                if range > 0.00001 {
                    t = first + fract((t - first) / range) * range
                }
            } else {
                t = clamp(t, 0.0, 1.0)
            }
            var color = self.grad_stop0_color
            if self.grad_stop_count >= 2.0 {
                var c0 = self.grad_stop0_color
                var p0 = self.grad_stop0_pos
                var c1 = self.grad_stop1_color
                var p1 = self.grad_stop1_pos
                if self.grad_stop_count >= 3.0 && t > self.grad_stop1_pos {
                    c0 = self.grad_stop1_color; p0 = self.grad_stop1_pos
                    c1 = self.grad_stop2_color; p1 = self.grad_stop2_pos
                }
                if self.grad_stop_count >= 4.0 && t > self.grad_stop2_pos {
                    c0 = self.grad_stop2_color; p0 = self.grad_stop2_pos
                    c1 = self.grad_stop3_color; p1 = self.grad_stop3_pos
                }
                if self.grad_stop_count >= 5.0 && t > self.grad_stop3_pos {
                    c0 = self.grad_stop3_color; p0 = self.grad_stop3_pos
                    c1 = self.grad_stop4_color; p1 = self.grad_stop4_pos
                }
                if self.grad_stop_count >= 6.0 && t > self.grad_stop4_pos {
                    c0 = self.grad_stop4_color; p0 = self.grad_stop4_pos
                    c1 = self.grad_stop5_color; p1 = self.grad_stop5_pos
                }
                if self.grad_stop_count >= 7.0 && t > self.grad_stop5_pos {
                    c0 = self.grad_stop5_color; p0 = self.grad_stop5_pos
                    c1 = self.grad_stop6_color; p1 = self.grad_stop6_pos
                }
                if self.grad_stop_count >= 8.0 && t > self.grad_stop6_pos {
                    c0 = self.grad_stop6_color; p0 = self.grad_stop6_pos
                    c1 = self.grad_stop7_color; p1 = self.grad_stop7_pos
                }
                let range = p1 - p0
                var frac = 0.0
                if range > 0.00001 {
                    frac = clamp((t - p0) / range, 0.0, 1.0)
                }
                color = mix(c0, c1, frac)
            }
            return vec4(color.rgb * color.a, color.a)
        }

        rounded_rect_alpha: fn(point: vec2f, size: vec2f, radius: vec4) -> float {
            let min_x = 0.0
            let min_y = 0.0
            let max_x = size.x
            let max_y = size.y
            if point.x < min_x || point.y < min_y || point.x > max_x || point.y > max_y {
                return 0.0
            }
            let tl = max(radius.x, 0.0)
            if tl > 0.0 && point.x < min_x + tl && point.y < min_y + tl {
                let d = point - vec2(min_x + tl, min_y + tl)
                if length(d) <= tl { return 1.0 }
                return 0.0
            }
            let tr = max(radius.y, 0.0)
            if tr > 0.0 && point.x > max_x - tr && point.y < min_y + tr {
                let d = point - vec2(max_x - tr, min_y + tr)
                if length(d) <= tr { return 1.0 }
                return 0.0
            }
            let br = max(radius.z, 0.0)
            if br > 0.0 && point.x > max_x - br && point.y > max_y - br {
                let d = point - vec2(max_x - br, max_y - br)
                if length(d) <= br { return 1.0 }
                return 0.0
            }
            let bl = max(radius.w, 0.0)
            if bl > 0.0 && point.x < min_x + bl && point.y > max_y - bl {
                let d = point - vec2(min_x + bl, max_y - bl)
                if length(d) <= bl { return 1.0 }
                return 0.0
            }
            return 1.0
        }

        rounded_rect_color: fn() -> vec4 {
            let alpha = self.rounded_rect_alpha(self.local_uv * self.rect_size, self.rect_size, self.border_radius)
            return vec4(self.color.rgb * self.color.a * alpha, self.color.a * alpha)
        }

        border_color: fn() -> vec4 {
            let outer = self.rounded_rect_alpha(self.local_uv * self.rect_size, self.rect_size, self.border_radius)
            let inner_size = max(self.rect_size - vec2(self.border_width * 2.0, self.border_width * 2.0), vec2(0.0, 0.0))
            let inner_radius = max(self.border_radius - vec4(self.border_width, self.border_width, self.border_width, self.border_width), vec4(0.0, 0.0, 0.0, 0.0))
            let inner_point = self.local_uv * self.rect_size - vec2(self.border_width, self.border_width)
            let inner = self.rounded_rect_alpha(inner_point, inner_size, inner_radius)
            let alpha = clamp(outer - inner, 0.0, 1.0)
            return vec4(self.color.rgb * self.color.a * alpha, self.color.a * alpha)
        }

        image_pixel: fn() -> vec4 {
            let uv = if self.image_repeating > 0.5 {
                vec2(
                    fract((self.local_uv.x * self.rect_size.x) / max(self.repeat_tile_size.x, 0.0001)),
                    fract((self.local_uv.y * self.rect_size.y) / max(self.repeat_tile_size.y, 0.0001))
                )
            } else {
                self.local_uv
            }
            return self.color_texture.sample_as_bgra(uv)
        }

        gradient_pixel: fn() -> vec4 {
            let uv = self.local_uv
            var t = 0.0
            if self.primitive_kind < 5.5 {
                let p0 = vec2(self.grad_param0, self.grad_param1)
                let p1 = vec2(self.grad_param2, self.grad_param3)
                let d = p1 - p0
                let len2 = dot(d, d)
                if len2 > 0.000001 {
                    t = dot(uv - p0, d) / len2
                }
            } else if self.primitive_kind < 6.5 {
                let dx = uv.x - self.grad_param0
                let dy = uv.y - self.grad_param1
                if self.grad_param2 > 0.0001 && self.grad_param3 > 0.0001 {
                    t = length(vec2(dx / self.grad_param2, dy / self.grad_param3))
                }
            } else {
                let dx = uv.x - self.grad_param0
                let dy = uv.y - self.grad_param1
                var angle = atan2(dx, -dy) - self.grad_param2
                let two_pi = 6.283185307
                angle = angle - floor(angle / two_pi) * two_pi
                t = angle / two_pi
            }
            return self.gradient_color(t)
        }

        box_shadow_pixel: fn() -> vec4 {
            let pos = self.local_uv * self.rect_size
            let half_size = self.box_size * 0.5
            let center = self.box_offset + half_size
            let radius = max(self.shadow_corner, 0.0)
            let q = abs(pos - center) - (half_size - vec2(radius, radius))
            let outside = length(max(q, vec2(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - radius
            let sigma = max(self.shadow_sigma, 0.001)
            let shadow = exp(-(outside * outside) / (2.0 * sigma * sigma))
            let value = if self.shadow_inset > 0.5 {
                1.0 - clamp(shadow, 0.0, 1.0)
            } else {
                clamp(shadow, 0.0, 1.0)
            }
            return vec4(self.color.xyz * self.color.w * value, self.color.w * value)
        }

        vertex: fn() {
            let local = self.geom.pos * self.rect_size + self.rect_pos
            self.local_uv = self.geom.pos
            self.local_pos = local
            let world = self.draw_list.view_transform * self.transform * vec4(
                local.x,
                local.y,
                self.draw_depth + self.draw_call.zbias,
                1.0
            )
            let view_pos = self.draw_pass.camera_view * world
            self.clip_space = self.draw_pass.camera_projection * view_pos
            self.vertex_pos = self.clip_space
        }

        pixel: fn() {
            if self.clip_projected() < 0.5 {
                discard()
            }
            let clip_alpha = self.local_clip_alpha() * self.accumulated_mask_alpha()
            if clip_alpha < 0.0001 {
                discard()
            }
            let color = if self.primitive_kind < 0.5 {
                vec4(self.color.rgb * self.color.a, self.color.a)
            } else if self.primitive_kind < 1.5 {
                self.rounded_rect_color()
            } else if self.primitive_kind < 2.5 {
                self.border_color()
            } else if self.primitive_kind < 4.5 {
                self.image_pixel()
            } else if self.primitive_kind < 7.5 {
                self.gradient_pixel()
            } else {
                self.box_shadow_pixel()
            }
            return color * clip_alpha
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawBrowserPrimitive {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    transform: Mat4f,
}

impl DrawBrowserPrimitive {
    fn draw(&mut self, cx: &mut Cx2d) {
        self.draw_super.draw(cx);
    }
}

pub(crate) struct MpBrowserPrimitiveRenderer {
    draw_primitive: DrawBrowserPrimitive,
}

impl MpBrowserPrimitiveRenderer {
    pub(crate) fn new(vm: &mut ScriptVm) -> Self {
        Self {
            draw_primitive: DrawBrowserPrimitive::script_new_with_default(vm),
        }
    }

    pub(crate) fn draw_scene(&mut self, cx: &mut Cx2d, scene: &MpBrowserPrimitiveScene) {
        for &batch_id in &scene.draw_order {
            self.draw_batch(cx, scene, batch_id);
        }
    }

    pub(crate) fn draw_batch(
        &mut self,
        cx: &mut Cx2d,
        scene: &MpBrowserPrimitiveScene,
        batch_id: MpPrimitiveBatchId,
    ) {
        let Some(batch) = scene.batches.get(batch_id) else {
            return;
        };
        let Some(transform) = scene.transforms.get(batch.transform_id).copied() else {
            return;
        };
        let Some(clip_chain) = scene.clip_chains.get(batch.clip_chain_id) else {
            return;
        };
        // Batch-level: compute clip state once (shared by all primitives in batch)
        self.configure_batch_clip(cx, transform, clip_chain);

        for primitive in &scene.primitives[batch.primitive_range.clone()] {
            if matches!(transform.backface_visibility, MpBackfaceVisibility::Hidden)
                && primitive_backface_culled(transform.scene_from_origin, primitive.local_rect)
            {
                continue;
            }
            self.configure_primitive(cx, primitive, transform);
            self.configure_kind(cx, &primitive.kind);
            self.draw_primitive.draw(cx);
        }
    }

    fn configure_batch_clip(
        &mut self,
        cx: &mut Cx2d,
        transform: MpPrimitiveTransform,
        clip_chain: &MpPrimitiveClipChain,
    ) {
        let draw_vars = &mut self.draw_primitive.draw_super.draw_vars;
        if let Some(rect) = clip_chain.origin_clip_rect {
            draw_vars.set_uniform(cx.cx, live_id!(local_clip_enabled), &[1.0]);
            draw_vars.set_uniform(
                cx.cx,
                live_id!(local_clip_rect),
                &[rect.pos.x as f32, rect.pos.y as f32, rect.size.x as f32, rect.size.y as f32],
            );
        } else {
            draw_vars.set_uniform(cx.cx, live_id!(local_clip_enabled), &[0.0]);
            draw_vars.set_uniform(cx.cx, live_id!(local_clip_rect), &[0.0, 0.0, 0.0, 0.0]);
        }
        let basis = MpClipBasis::from_cx(cx, transform.scene_from_origin);
        let evaluated = evaluate_clip_chain_no_rect(clip_chain, &basis);
        set_clip_planes_evaluated(cx, draw_vars, &evaluated);
        set_clip_masks_evaluated(cx, draw_vars, &evaluated);
    }

    fn configure_primitive(
        &mut self,
        _cx: &mut Cx2d,
        primitive: &MpBrowserPrimitive,
        transform: MpPrimitiveTransform,
    ) {
        self.draw_primitive.draw_super.rect_pos = primitive.local_rect.pos.into();
        self.draw_primitive.draw_super.rect_size = primitive.local_rect.size.into();
        self.draw_primitive.draw_super.draw_clip = vec4(-100000.0, -100000.0, 100000.0, 100000.0);
        self.draw_primitive.transform = transform.scene_from_origin;
        let draw_vars = &mut self.draw_primitive.draw_super.draw_vars;
        draw_vars.texture_slots[0] = None;
        draw_vars.options.depth_write = false;
        // Clip uniforms already set at batch level by configure_batch_clip()
    }

    fn configure_kind(&mut self, cx: &mut Cx2d, kind: &MpBrowserPrimitiveKind) {
        let draw_vars = &mut self.draw_primitive.draw_super.draw_vars;
        reset_gradient_uniforms(cx, draw_vars);
        reset_shape_uniforms(cx, draw_vars);
        match kind {
            MpBrowserPrimitiveKind::SolidRect { color } => {
                draw_vars.set_uniform(cx.cx, live_id!(primitive_kind), &[0.0]);
                draw_vars.set_uniform(cx.cx, live_id!(color), &[color.x, color.y, color.z, color.w]);
            }
            MpBrowserPrimitiveKind::RoundedRect { color, radius } => {
                draw_vars.set_uniform(cx.cx, live_id!(primitive_kind), &[1.0]);
                draw_vars.set_uniform(cx.cx, live_id!(color), &[color.x, color.y, color.z, color.w]);
                draw_vars.set_uniform(cx.cx, live_id!(border_radius), &[radius.x, radius.y, radius.z, radius.w]);
            }
            MpBrowserPrimitiveKind::Border { color, width, radius } => {
                draw_vars.set_uniform(cx.cx, live_id!(primitive_kind), &[2.0]);
                draw_vars.set_uniform(cx.cx, live_id!(color), &[color.x, color.y, color.z, color.w]);
                draw_vars.set_uniform(cx.cx, live_id!(border_width), &[*width]);
                draw_vars.set_uniform(cx.cx, live_id!(border_radius), &[radius.x, radius.y, radius.z, radius.w]);
            }
            MpBrowserPrimitiveKind::Image { texture } => {
                draw_vars.set_uniform(cx.cx, live_id!(primitive_kind), &[3.0]);
                draw_vars.set_uniform(cx.cx, live_id!(image_repeating), &[0.0]);
                draw_vars.set_uniform(cx.cx, live_id!(repeat_tile_size), &[1.0, 1.0]);
                draw_vars.set_texture(0, texture);
            }
            MpBrowserPrimitiveKind::RepeatingImage { texture, tile_size } => {
                draw_vars.set_uniform(cx.cx, live_id!(primitive_kind), &[4.0]);
                draw_vars.set_uniform(cx.cx, live_id!(image_repeating), &[1.0]);
                draw_vars.set_uniform(cx.cx, live_id!(repeat_tile_size), &[tile_size.x, tile_size.y]);
                draw_vars.set_texture(0, texture);
            }
            MpBrowserPrimitiveKind::LinearGradient {
                start,
                end,
                repeating,
                stops,
            } => {
                draw_vars.set_uniform(cx.cx, live_id!(primitive_kind), &[5.0]);
                draw_vars.set_uniform(cx.cx, live_id!(grad_param0), &[start.x]);
                draw_vars.set_uniform(cx.cx, live_id!(grad_param1), &[start.y]);
                draw_vars.set_uniform(cx.cx, live_id!(grad_param2), &[end.x]);
                draw_vars.set_uniform(cx.cx, live_id!(grad_param3), &[end.y]);
                draw_vars.set_uniform(cx.cx, live_id!(grad_repeating), &[*repeating as u8 as f32]);
                set_gradient_stops(cx, draw_vars, stops);
            }
            MpBrowserPrimitiveKind::RadialGradient {
                center,
                radius,
                repeating,
                stops,
            } => {
                draw_vars.set_uniform(cx.cx, live_id!(primitive_kind), &[6.0]);
                draw_vars.set_uniform(cx.cx, live_id!(grad_param0), &[center.x]);
                draw_vars.set_uniform(cx.cx, live_id!(grad_param1), &[center.y]);
                draw_vars.set_uniform(cx.cx, live_id!(grad_param2), &[radius.x]);
                draw_vars.set_uniform(cx.cx, live_id!(grad_param3), &[radius.y]);
                draw_vars.set_uniform(cx.cx, live_id!(grad_repeating), &[*repeating as u8 as f32]);
                set_gradient_stops(cx, draw_vars, stops);
            }
            MpBrowserPrimitiveKind::ConicGradient {
                center,
                start_angle_rad,
                repeating,
                stops,
            } => {
                draw_vars.set_uniform(cx.cx, live_id!(primitive_kind), &[7.0]);
                draw_vars.set_uniform(cx.cx, live_id!(grad_param0), &[center.x]);
                draw_vars.set_uniform(cx.cx, live_id!(grad_param1), &[center.y]);
                draw_vars.set_uniform(cx.cx, live_id!(grad_param2), &[*start_angle_rad]);
                draw_vars.set_uniform(cx.cx, live_id!(grad_param3), &[0.0]);
                draw_vars.set_uniform(cx.cx, live_id!(grad_repeating), &[*repeating as u8 as f32]);
                set_gradient_stops(cx, draw_vars, stops);
            }
            MpBrowserPrimitiveKind::BoxShadow {
                color,
                box_offset,
                box_size,
                sigma,
                corner_radius_px,
                inset,
            } => {
                draw_vars.set_uniform(cx.cx, live_id!(primitive_kind), &[8.0]);
                draw_vars.set_uniform(cx.cx, live_id!(color), &[color.x, color.y, color.z, color.w]);
                draw_vars.set_uniform(cx.cx, live_id!(box_offset), &[box_offset.x, box_offset.y]);
                draw_vars.set_uniform(cx.cx, live_id!(box_size), &[box_size.x, box_size.y]);
                draw_vars.set_uniform(cx.cx, live_id!(shadow_sigma), &[*sigma]);
                draw_vars.set_uniform(cx.cx, live_id!(shadow_corner), &[*corner_radius_px]);
                draw_vars.set_uniform(cx.cx, live_id!(shadow_inset), &[*inset as u8 as f32]);
            }
        }
    }
}

/// Extract origin-space planes and mask entries from a declarative clip chain.
///
/// The returned planes and mask matrices are still in origin space. The
/// compositor evaluator converts them to clip space at draw time.
pub(crate) fn lower_clip_chain_exec(clip_chain: &MpPrimitiveClipChain) -> (Vec<Vec4f>, MpMaskExec) {
    let mut clip_planes = Vec::new();
    let mut masks = Vec::new();
    for entry in &clip_chain.entries {
        match entry {
            MpPrimitiveClipEntry::RoundedRect {
                rect,
                radius,
                mask_local_from_origin,
            } => masks.push(MpEvaluatedMask {
                kind: MpMaskKind::RoundedRect {
                    rect: *rect,
                    radius: *radius,
                },
                clip_to_local: *mask_local_from_origin,
            }),
            MpPrimitiveClipEntry::ImageMask { rect, mask_local_from_origin } => masks.push(MpEvaluatedMask {
                kind: MpMaskKind::ImageMask { rect: *rect },
                clip_to_local: *mask_local_from_origin,
            }),
            MpPrimitiveClipEntry::PlaneSet { planes } => clip_planes.extend(planes.iter().copied()),
        }
    }
    (clip_planes, MpMaskExec { masks })
}

fn primitive_backface_culled(projected_transform: Mat4f, rect: Rect) -> bool {
    let Some(p0) = projected_point(&projected_transform, rect.pos.x as f32, rect.pos.y as f32) else {
        return false;
    };
    let Some(p1) = projected_point(
        &projected_transform,
        (rect.pos.x + rect.size.x) as f32,
        rect.pos.y as f32,
    ) else {
        return false;
    };
    let Some(p2) = projected_point(
        &projected_transform,
        rect.pos.x as f32,
        (rect.pos.y + rect.size.y) as f32,
    ) else {
        return false;
    };
    let area = (p1.x - p0.x) * (p2.y - p0.y) - (p1.y - p0.y) * (p2.x - p0.x);
    area < 0.0
}

fn projected_point(transform: &Mat4f, x: f32, y: f32) -> Option<Vec2f> {
    let clip = transform.transform_vec4(vec4f(x, y, 0.0, 1.0));
    if clip.w.abs() <= 1e-6 {
        return None;
    }
    Some(vec2(clip.x / clip.w, clip.y / clip.w))
}

fn reset_shape_uniforms(cx: &mut Cx2d, draw_vars: &mut DrawVars) {
    draw_vars.set_uniform(cx.cx, live_id!(color), &[0.0, 0.0, 0.0, 1.0]);
    draw_vars.set_uniform(cx.cx, live_id!(image_repeating), &[0.0]);
    draw_vars.set_uniform(cx.cx, live_id!(repeat_tile_size), &[1.0, 1.0]);
    draw_vars.set_uniform(cx.cx, live_id!(border_width), &[1.0]);
    draw_vars.set_uniform(cx.cx, live_id!(border_radius), &[0.0, 0.0, 0.0, 0.0]);
    draw_vars.set_uniform(cx.cx, live_id!(box_offset), &[0.0, 0.0]);
    draw_vars.set_uniform(cx.cx, live_id!(box_size), &[0.0, 0.0]);
    draw_vars.set_uniform(cx.cx, live_id!(shadow_sigma), &[0.001]);
    draw_vars.set_uniform(cx.cx, live_id!(shadow_corner), &[0.0]);
    draw_vars.set_uniform(cx.cx, live_id!(shadow_inset), &[0.0]);
}

fn reset_gradient_uniforms(cx: &mut Cx2d, draw_vars: &mut DrawVars) {
    draw_vars.set_uniform(cx.cx, live_id!(grad_param0), &[0.0]);
    draw_vars.set_uniform(cx.cx, live_id!(grad_param1), &[0.0]);
    draw_vars.set_uniform(cx.cx, live_id!(grad_param2), &[1.0]);
    draw_vars.set_uniform(cx.cx, live_id!(grad_param3), &[1.0]);
    draw_vars.set_uniform(cx.cx, live_id!(grad_repeating), &[0.0]);
    for (color_id, pos_id) in [
        (live_id!(grad_stop0_color), live_id!(grad_stop0_pos)),
        (live_id!(grad_stop1_color), live_id!(grad_stop1_pos)),
        (live_id!(grad_stop2_color), live_id!(grad_stop2_pos)),
        (live_id!(grad_stop3_color), live_id!(grad_stop3_pos)),
        (live_id!(grad_stop4_color), live_id!(grad_stop4_pos)),
        (live_id!(grad_stop5_color), live_id!(grad_stop5_pos)),
        (live_id!(grad_stop6_color), live_id!(grad_stop6_pos)),
        (live_id!(grad_stop7_color), live_id!(grad_stop7_pos)),
    ] {
        draw_vars.set_uniform(cx.cx, color_id, &[0.0, 0.0, 0.0, 0.0]);
        draw_vars.set_uniform(cx.cx, pos_id, &[0.0]);
    }
    draw_vars.set_uniform(cx.cx, live_id!(grad_stop0_color), &[0.0, 0.0, 0.0, 1.0]);
    draw_vars.set_uniform(cx.cx, live_id!(grad_stop1_color), &[1.0, 1.0, 1.0, 1.0]);
    draw_vars.set_uniform(cx.cx, live_id!(grad_stop0_pos), &[0.0]);
    draw_vars.set_uniform(cx.cx, live_id!(grad_stop1_pos), &[1.0]);
    draw_vars.set_uniform(cx.cx, live_id!(grad_stop_count), &[2.0]);
}

fn set_gradient_stops(cx: &mut Cx2d, draw_vars: &mut DrawVars, stops: &[MpBrowserGradientStop]) {
    let mut resolved = stops.to_vec();
    if resolved.len() > 8 {
        resolved.truncate(8);
    }
    draw_vars.set_uniform(cx.cx, live_id!(grad_stop_count), &[resolved.len() as f32]);
    for (index, stop) in resolved.iter().enumerate() {
        let color_id = match index {
            0 => live_id!(grad_stop0_color),
            1 => live_id!(grad_stop1_color),
            2 => live_id!(grad_stop2_color),
            3 => live_id!(grad_stop3_color),
            4 => live_id!(grad_stop4_color),
            5 => live_id!(grad_stop5_color),
            6 => live_id!(grad_stop6_color),
            _ => live_id!(grad_stop7_color),
        };
        let pos_id = match index {
            0 => live_id!(grad_stop0_pos),
            1 => live_id!(grad_stop1_pos),
            2 => live_id!(grad_stop2_pos),
            3 => live_id!(grad_stop3_pos),
            4 => live_id!(grad_stop4_pos),
            5 => live_id!(grad_stop5_pos),
            6 => live_id!(grad_stop6_pos),
            _ => live_id!(grad_stop7_pos),
        };
        draw_vars.set_uniform(
            cx.cx,
            color_id,
            &[stop.color.x, stop.color.y, stop.color.z, stop.color.w],
        );
        draw_vars.set_uniform(cx.cx, pos_id, &[stop.offset]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_scene_batches_consecutive_matching_primitives() {
        let mut scene = MpBrowserPrimitiveScene::new(Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(100.0, 100.0),
        });
        let clip_chain = scene.push_clip_chain(MpPrimitiveClipChain::default());
        for offset in [0.0, 12.0] {
            scene.push_primitive(MpBrowserPrimitive {
                local_rect: Rect {
                    pos: dvec2(offset, 0.0),
                    size: dvec2(10.0, 10.0),
                },
                transform_id: 0,
                clip_chain_id: clip_chain,
                kind: MpBrowserPrimitiveKind::SolidRect {
                    color: vec4(1.0, 0.0, 0.0, 1.0),
                },
            });
        }

        assert_eq!(scene.batches.len(), 1);
        assert_eq!(scene.draw_order, vec![0]);
        assert_eq!(scene.batches[0].primitive_range, 0..2);
    }

    #[test]
    fn transform_palette_lookup_returns_inserted_entry() {
        let mut scene = MpBrowserPrimitiveScene::new(Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(100.0, 100.0),
        });
        let transform = MpPrimitiveTransform {
            scene_from_origin: Mat4f::translation(vec3(12.0, 18.0, 0.0)),
            backface_visibility: MpBackfaceVisibility::Hidden,
        };
        let id = scene.push_transform(transform);

        assert_eq!(scene.transform(id), Some(&transform));
    }

    #[test]
    fn clip_chain_lookup_returns_inserted_entry() {
        let mut scene = MpBrowserPrimitiveScene::new(Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(100.0, 100.0),
        });
        let clip_chain = MpPrimitiveClipChain {
            origin_clip_rect: Some(Rect {
                pos: dvec2(4.0, 5.0),
                size: dvec2(30.0, 40.0),
            }),
            entries: vec![MpPrimitiveClipEntry::PlaneSet {
                planes: vec![vec4(1.0, 0.0, 0.0, -4.0)],
            }],
        };
        let id = scene.push_clip_chain(clip_chain.clone());

        assert_eq!(scene.clip_chain(id), Some(&clip_chain));
    }

    #[test]
    fn clip_chain_exec_collects_masks_and_planes() {
        let clip_chain = MpPrimitiveClipChain {
            origin_clip_rect: None,
            entries: vec![
                MpPrimitiveClipEntry::RoundedRect {
                    rect: Rect {
                        pos: dvec2(0.0, 0.0),
                        size: dvec2(20.0, 20.0),
                    },
                    radius: vec4(4.0, 4.0, 4.0, 4.0),
                    mask_local_from_origin: Mat4f::identity(),
                },
                MpPrimitiveClipEntry::PlaneSet {
                    planes: vec![vec4(1.0, 0.0, 0.0, -5.0)],
                },
            ],
        };

        let (planes, masks) = lower_clip_chain_exec(&clip_chain);

        assert_eq!(planes, vec![vec4(1.0, 0.0, 0.0, -5.0)]);
        assert_eq!(masks.masks.len(), 1);
    }
}
