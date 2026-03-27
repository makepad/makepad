use crate::scene::{MpEvaluatedMask, MpMaskExec, MpMaskKind};
use crate::*;

pub const MP_MAX_CLIP_PLANES: usize = 8;
pub const MP_MAX_CLIP_MASKS: usize = 4;

const CLIP_PLANE_IDS: [LiveId; MP_MAX_CLIP_PLANES] = [
    live_id!(clip_plane_0),
    live_id!(clip_plane_1),
    live_id!(clip_plane_2),
    live_id!(clip_plane_3),
    live_id!(clip_plane_4),
    live_id!(clip_plane_5),
    live_id!(clip_plane_6),
    live_id!(clip_plane_7),
];
const CLIP_MASK_TYPE_IDS: [LiveId; MP_MAX_CLIP_MASKS] = [
    live_id!(clip_mask_type_0),
    live_id!(clip_mask_type_1),
    live_id!(clip_mask_type_2),
    live_id!(clip_mask_type_3),
];
const CLIP_MASK_RECT_IDS: [LiveId; MP_MAX_CLIP_MASKS] = [
    live_id!(clip_mask_rect_0),
    live_id!(clip_mask_rect_1),
    live_id!(clip_mask_rect_2),
    live_id!(clip_mask_rect_3),
];
const CLIP_MASK_RADIUS_IDS: [LiveId; MP_MAX_CLIP_MASKS] = [
    live_id!(clip_mask_radius_0),
    live_id!(clip_mask_radius_1),
    live_id!(clip_mask_radius_2),
    live_id!(clip_mask_radius_3),
];
const CLIP_MASK_MATRIX_IDS: [LiveId; MP_MAX_CLIP_MASKS] = [
    live_id!(clip_mask_matrix_0),
    live_id!(clip_mask_matrix_1),
    live_id!(clip_mask_matrix_2),
    live_id!(clip_mask_matrix_3),
];
pub(super) const PROJECTED_CORNER_IDS: [LiveId; 4] = [
    live_id!(corner_0),
    live_id!(corner_1),
    live_id!(corner_2),
    live_id!(corner_3),
];

/// Explicit draw-time basis for browser-scene clip evaluation.
///
/// **Spatial vocabulary:**
/// - `scene_from_origin`: retained transform from primitive/picture origin
///   space to the browser-scene host's local coordinate system.
/// - `world_from_scene`: the active outer Makepad draw-list `view_transform`.
/// - `clip_from_world`: the current pass view-projection.
///
/// Derived:
/// - `clip_from_origin = clip_from_world * world_from_scene * scene_from_origin`
/// - `origin_from_clip = inverse(clip_from_origin)`
#[derive(Clone, Copy, Debug)]
pub struct MpClipBasis {
    #[allow(dead_code)]
    pub clip_from_origin: Mat4f,
    pub origin_from_clip: Mat4f,
}

impl MpClipBasis {
    /// Build the explicit basis from the three transform stages.
    pub fn new(scene_from_origin: Mat4f, world_from_scene: Mat4f, clip_from_world: Mat4f) -> Self {
        let clip_from_origin = Mat4f::mul(&Mat4f::mul(&clip_from_world, &world_from_scene), &scene_from_origin);
        let origin_from_clip = clip_from_origin.invert();
        Self {
            clip_from_origin,
            origin_from_clip,
        }
    }

    /// Build from the active Cx2d state plus an explicit `scene_from_origin`.
    pub fn from_cx(cx: &Cx2d, scene_from_origin: Mat4f) -> Self {
        let (clip_from_world, world_from_scene) =
            current_clip_and_view(cx).unwrap_or((Mat4f::identity(), Mat4f::identity()));
        Self::new(scene_from_origin, world_from_scene, clip_from_world)
    }
}

/// Evaluated clip state ready for uniform submission. Produced by the
/// shared clip evaluator from origin-space retained data + explicit basis.
#[derive(Clone, Debug, Default)]
pub struct MpEvaluatedClipState {
    /// Planes in clip space, ready for `dot(clip_space, plane) >= 0` test.
    pub clip_planes: Vec<Vec4f>,
    /// Mask entries with `mask_local_from_clip` matrices.
    pub masks: MpMaskExec,
}

/// Shared clip evaluator. Converts origin-space retained clip data into
/// execution-ready clip-space values using the explicit draw-time basis.
///
/// This is the only place that turns retained clip data into shader uniforms.
///
/// The `include_origin_clip_rect` parameter controls whether `origin_clip_rect`
/// is converted to clip-space planes. For direct primitives this is `false`
/// because the primitive shader has a dedicated `local_clip_rect` uniform.
/// For composited picture quads this is `true` because those shaders lack
/// the local clip rect path.
pub fn evaluate_clip_chain(
    clip_chain: &crate::browser_primitives::MpPrimitiveClipChain,
    basis: &MpClipBasis,
) -> MpEvaluatedClipState {
    evaluate_clip_chain_impl(clip_chain, basis, true)
}

/// Like `evaluate_clip_chain` but skips `origin_clip_rect` → plane conversion.
/// Used for direct primitives which have a dedicated `local_clip_rect` shader
/// uniform for the axis-aligned fast path.
pub fn evaluate_clip_chain_no_rect(
    clip_chain: &crate::browser_primitives::MpPrimitiveClipChain,
    basis: &MpClipBasis,
) -> MpEvaluatedClipState {
    evaluate_clip_chain_impl(clip_chain, basis, false)
}

fn evaluate_clip_chain_impl(
    clip_chain: &crate::browser_primitives::MpPrimitiveClipChain,
    basis: &MpClipBasis,
    include_origin_clip_rect: bool,
) -> MpEvaluatedClipState {
    let (mut origin_planes, origin_masks) = crate::browser_primitives::lower_clip_chain_exec(clip_chain);

    if include_origin_clip_rect {
        if let Some(rect) = clip_chain.origin_clip_rect {
            origin_planes.extend_from_slice(&origin_rect_as_planes(rect));
        }
    }

    let plane_transform = basis.origin_from_clip.transpose();
    let clip_planes: Vec<Vec4f> = origin_planes
        .iter()
        .map(|plane| plane_transform.transform_vec4(*plane))
        .collect();

    let masks = MpMaskExec {
        masks: origin_masks
            .masks
            .iter()
            .map(|mask| MpEvaluatedMask {
                kind: mask.kind.clone(),
                clip_to_local: Mat4f::mul(&mask.clip_to_local, &basis.origin_from_clip),
            })
            .collect(),
    };

    MpEvaluatedClipState { clip_planes, masks }
}

fn origin_rect_as_planes(rect: Rect) -> [Vec4f; 4] {
    [
        vec4(1.0, 0.0, 0.0, -(rect.pos.x as f32)),
        vec4(-1.0, 0.0, 0.0, (rect.pos.x + rect.size.x) as f32),
        vec4(0.0, 1.0, 0.0, -(rect.pos.y as f32)),
        vec4(0.0, -1.0, 0.0, (rect.pos.y + rect.size.y) as f32),
    ]
}

/// Write pre-evaluated clip-space planes to shader uniforms. Pure writer —
/// no ambient basis derivation.
pub(crate) fn write_clip_planes(cx: &Cx2d, draw_vars: &mut DrawVars, clip_planes: &[Vec4f]) {
    let count = clip_planes.len().min(MP_MAX_CLIP_PLANES);
    draw_vars.set_uniform(cx.cx, live_id!(clip_plane_count), &[count as f32]);
    for (index, id) in CLIP_PLANE_IDS.iter().enumerate() {
        let plane = clip_planes
            .get(index)
            .copied()
            .unwrap_or_else(|| vec4(0.0, 0.0, 0.0, 0.0));
        draw_vars.set_uniform(cx.cx, *id, &[plane.x, plane.y, plane.z, plane.w]);
    }
}

/// Write pre-evaluated mask data to shader uniforms. Pure writer —
/// no ambient basis derivation.
pub(crate) fn write_clip_masks(cx: &Cx2d, draw_vars: &mut DrawVars, mask: &MpMaskExec) {
    let count = mask.masks.len().min(MP_MAX_CLIP_MASKS);
    draw_vars.set_uniform(cx.cx, live_id!(clip_mask_count), &[count as f32]);
    for index in 0..MP_MAX_CLIP_MASKS {
        let (mask_type, rect, radius, matrix) = match mask.masks.get(index) {
            Some(mask) => {
                let (mask_type, rect, radius) = match &mask.kind {
                    MpMaskKind::RoundedRect { rect, radius } => (1.0, *rect, *radius),
                    MpMaskKind::ImageMask { rect } => (2.0, *rect, vec4(0.0, 0.0, 0.0, 0.0)),
                };
                (mask_type, rect_to_mask_rect(rect), radius, mask.clip_to_local)
            }
            None => (
                0.0,
                vec4(0.0, 0.0, 0.0, 0.0),
                vec4(0.0, 0.0, 0.0, 0.0),
                Mat4f::identity(),
            ),
        };
        draw_vars.set_uniform(cx.cx, CLIP_MASK_TYPE_IDS[index], &[mask_type]);
        draw_vars.set_uniform(cx.cx, CLIP_MASK_RECT_IDS[index], &[rect.x, rect.y, rect.z, rect.w]);
        draw_vars.set_uniform(
            cx.cx,
            CLIP_MASK_RADIUS_IDS[index],
            &[radius.x, radius.y, radius.z, radius.w],
        );
        draw_vars.set_uniform(cx.cx, CLIP_MASK_MATRIX_IDS[index], &matrix.v);
    }
}

/// Evaluate origin-space clip chain through an explicit basis and write to
/// shader uniforms. Convenience wrapper combining evaluate + write.
pub(crate) fn set_clip_planes_evaluated(
    cx: &Cx2d,
    draw_vars: &mut DrawVars,
    evaluated: &MpEvaluatedClipState,
) {
    let planes = if evaluated.clip_planes.len() > MP_MAX_CLIP_PLANES {
        &evaluated.clip_planes[..MP_MAX_CLIP_PLANES]
    } else {
        &evaluated.clip_planes
    };
    write_clip_planes(cx, draw_vars, planes);
}

pub(crate) fn set_clip_masks_evaluated(
    cx: &Cx2d,
    draw_vars: &mut DrawVars,
    evaluated: &MpEvaluatedClipState,
) {
    write_clip_masks(cx, draw_vars, &evaluated.masks);
}

/// Legacy: set clip planes from pre-evaluated planes (used by non-browser
/// compositor scene path which already stores clip-space planes).
pub(crate) fn set_clip_planes(cx: &Cx2d, draw_vars: &mut DrawVars, clip_planes: &[Vec4f]) {
    let count = clip_planes.len().min(MP_MAX_CLIP_PLANES);
    draw_vars.set_uniform(cx.cx, live_id!(clip_plane_count), &[count as f32]);
    let world_to_clip = current_world_to_clip(cx).unwrap_or_else(Mat4f::identity);
    let clip_plane_transform = world_to_clip.invert().transpose();
    for (index, id) in CLIP_PLANE_IDS.iter().enumerate() {
        let plane = clip_planes
            .get(index)
            .copied()
            .map(|plane| clip_plane_transform.transform_vec4(plane))
            .unwrap_or_else(|| vec4(0.0, 0.0, 0.0, 0.0));
        draw_vars.set_uniform(cx.cx, *id, &[plane.x, plane.y, plane.z, plane.w]);
    }
}

pub(crate) fn set_clip_masks(cx: &Cx2d, draw_vars: &mut DrawVars, mask: &MpMaskExec) {
    let count = mask.masks.len().min(MP_MAX_CLIP_MASKS);
    draw_vars.set_uniform(cx.cx, live_id!(clip_mask_count), &[count as f32]);
    let world_to_clip = current_world_to_clip(cx).unwrap_or_else(Mat4f::identity);
    let clip_to_world = world_to_clip.invert();
    for index in 0..MP_MAX_CLIP_MASKS {
        let (mask_type, rect, radius, matrix) = match mask.masks.get(index) {
            Some(mask) => {
                let (mask_type, rect, radius) = match &mask.kind {
                    MpMaskKind::RoundedRect { rect, radius } => (1.0, *rect, *radius),
                    MpMaskKind::ImageMask { rect } => (2.0, *rect, vec4(0.0, 0.0, 0.0, 0.0)),
                };
                (
                    mask_type,
                    rect_to_mask_rect(rect),
                    radius,
                    Mat4f::mul(&mask.clip_to_local, &clip_to_world),
                )
            }
            None => (
                0.0,
                vec4(0.0, 0.0, 0.0, 0.0),
                vec4(0.0, 0.0, 0.0, 0.0),
                Mat4f::identity(),
            ),
        };
        draw_vars.set_uniform(cx.cx, CLIP_MASK_TYPE_IDS[index], &[mask_type]);
        draw_vars.set_uniform(cx.cx, CLIP_MASK_RECT_IDS[index], &[rect.x, rect.y, rect.z, rect.w]);
        draw_vars.set_uniform(
            cx.cx,
            CLIP_MASK_RADIUS_IDS[index],
            &[radius.x, radius.y, radius.z, radius.w],
        );
        draw_vars.set_uniform(cx.cx, CLIP_MASK_MATRIX_IDS[index], &matrix.v);
    }
}

fn rect_to_mask_rect(rect: Rect) -> Vec4f {
    vec4(
        rect.pos.x as f32,
        rect.pos.y as f32,
        rect.size.x as f32,
        rect.size.y as f32,
    )
}

pub(super) fn current_view_projection(cx: &Cx2d) -> Option<(Mat4f, Mat4f)> {
    let draw_list_id = *cx.draw_list_stack.last()?;
    let draw_list = &cx.draw_lists[draw_list_id];
    let pass_id = draw_list.draw_pass_id?;
    let pass = &cx.passes[pass_id];
    let pass_view_projection =
        Mat4f::mul(&pass.pass_uniforms.camera_projection, &pass.pass_uniforms.camera_view);
    Some((pass_view_projection, draw_list.draw_list_uniforms.view_transform))
}

/// Returns (clip_from_world, world_from_scene) from the active Cx2d state.
fn current_clip_and_view(cx: &Cx2d) -> Option<(Mat4f, Mat4f)> {
    let (pass_view_projection, view_transform) = current_view_projection(cx)?;
    Some((pass_view_projection, view_transform))
}

fn current_world_to_clip(cx: &Cx2d) -> Option<Mat4f> {
    let (pass_view_projection, draw_list_transform) = current_view_projection(cx)?;
    Some(Mat4f::mul(&pass_view_projection, &draw_list_transform))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_primitives::{MpPrimitiveClipChain, MpPrimitiveClipEntry};

    fn point_passes_planes(planes: &[Vec4f], basis: &MpClipBasis, origin_x: f32, origin_y: f32) -> bool {
        let clip_pos = basis.clip_from_origin.transform_vec4(vec4f(origin_x, origin_y, 0.0, 1.0));
        planes.iter().all(|plane| {
            plane.x * clip_pos.x + plane.y * clip_pos.y + plane.z * clip_pos.z + plane.w * clip_pos.w >= -1e-4
        })
    }

    #[test]
    fn evaluate_clip_chain_turns_origin_clip_rect_into_planes() {
        let clip_chain = MpPrimitiveClipChain {
            origin_clip_rect: Some(Rect {
                pos: dvec2(4.0, 6.0),
                size: dvec2(20.0, 30.0),
            }),
            entries: Vec::new(),
        };
        let basis = MpClipBasis::new(Mat4f::identity(), Mat4f::identity(), Mat4f::identity());
        let evaluated = evaluate_clip_chain(&clip_chain, &basis);

        assert_eq!(evaluated.clip_planes.len(), 4);
        assert!(evaluated.masks.masks.is_empty());
    }

    #[test]
    fn translated_outer_draw_list_clip_moves_with_geometry() {
        let clip_chain = MpPrimitiveClipChain {
            origin_clip_rect: Some(Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(100.0, 100.0),
            }),
            entries: Vec::new(),
        };

        let scene_from_origin = Mat4f::identity();
        let world_from_scene = Mat4f::translation(vec3(0.0, 50.0, 0.0));
        let clip_from_world = Mat4f::identity();
        let basis = MpClipBasis::new(scene_from_origin, world_from_scene, clip_from_world);
        let evaluated = evaluate_clip_chain(&clip_chain, &basis);

        assert!(point_passes_planes(&evaluated.clip_planes, &basis, 50.0, 50.0));
        assert!(!point_passes_planes(&evaluated.clip_planes, &basis, 50.0, 150.0));
    }

    #[test]
    fn rotated_outer_basis_clips_consistently() {
        let clip_chain = MpPrimitiveClipChain {
            origin_clip_rect: Some(Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(100.0, 100.0),
            }),
            entries: Vec::new(),
        };

        let angle = std::f32::consts::FRAC_PI_4;
        let scene_from_origin = Mat4f::rotation(vec3(0.0, 0.0, angle));
        let basis = MpClipBasis::new(scene_from_origin, Mat4f::identity(), Mat4f::identity());
        let evaluated = evaluate_clip_chain(&clip_chain, &basis);

        assert!(point_passes_planes(&evaluated.clip_planes, &basis, 50.0, 50.0));
        assert!(!point_passes_planes(&evaluated.clip_planes, &basis, 200.0, 200.0));
    }

    #[test]
    fn primitive_clip_rect_and_mask_entries_use_same_basis() {
        let clip_chain = MpPrimitiveClipChain {
            origin_clip_rect: Some(Rect {
                pos: dvec2(10.0, 10.0),
                size: dvec2(80.0, 80.0),
            }),
            entries: vec![MpPrimitiveClipEntry::RoundedRect {
                rect: Rect {
                    pos: dvec2(20.0, 20.0),
                    size: dvec2(60.0, 60.0),
                },
                radius: vec4(5.0, 5.0, 5.0, 5.0),
                mask_local_from_origin: Mat4f::identity(),
            }],
        };

        let scene_from_origin = Mat4f::translation(vec3(100.0, 200.0, 0.0));
        let basis = MpClipBasis::new(scene_from_origin, Mat4f::identity(), Mat4f::identity());
        let evaluated = evaluate_clip_chain(&clip_chain, &basis);

        assert_eq!(evaluated.clip_planes.len(), 4);
        assert_eq!(evaluated.masks.masks.len(), 1);
        let mask_matrix = evaluated.masks.masks[0].clip_to_local;
        assert_ne!(mask_matrix, Mat4f::identity());
    }

    #[test]
    fn picture_clip_uses_same_evaluator_as_primitive() {
        let clip_chain = MpPrimitiveClipChain {
            origin_clip_rect: Some(Rect {
                pos: dvec2(5.0, 5.0),
                size: dvec2(90.0, 90.0),
            }),
            entries: vec![MpPrimitiveClipEntry::PlaneSet {
                planes: vec![vec4(1.0, 0.0, 0.0, -10.0)],
            }],
        };

        let scene_from_origin = Mat4f::translation(vec3(50.0, 30.0, 0.0));
        let basis = MpClipBasis::new(scene_from_origin, Mat4f::identity(), Mat4f::identity());
        let picture_eval = evaluate_clip_chain(&clip_chain, &basis);
        let prim_eval = evaluate_clip_chain_no_rect(&clip_chain, &basis);

        assert_eq!(prim_eval.clip_planes.len(), 1);
        assert_eq!(picture_eval.clip_planes.len(), 5);
        assert!(picture_eval.clip_planes.contains(&prim_eval.clip_planes[0]));
    }

    #[test]
    fn same_clip_chain_two_placements_clips_correctly() {
        let clip_chain = MpPrimitiveClipChain {
            origin_clip_rect: Some(Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(100.0, 100.0),
            }),
            entries: Vec::new(),
        };

        let scene_from_origin = Mat4f::identity();
        let basis_a = MpClipBasis::new(
            scene_from_origin,
            Mat4f::identity(),
            Mat4f::identity(),
        );
        let eval_a = evaluate_clip_chain(&clip_chain, &basis_a);
        let basis_b = MpClipBasis::new(
            scene_from_origin,
            Mat4f::translation(vec3(200.0, 300.0, 0.0)),
            Mat4f::identity(),
        );
        let eval_b = evaluate_clip_chain(&clip_chain, &basis_b);

        assert!(point_passes_planes(&eval_a.clip_planes, &basis_a, 50.0, 50.0));
        assert!(point_passes_planes(&eval_b.clip_planes, &basis_b, 50.0, 50.0));
        assert!(!point_passes_planes(&eval_a.clip_planes, &basis_a, 150.0, 50.0));
        assert!(!point_passes_planes(&eval_b.clip_planes, &basis_b, 150.0, 50.0));
        assert_ne!(eval_a.clip_planes, eval_b.clip_planes);
    }

    #[test]
    fn offscreen_task_basis_does_not_double_apply_placement() {
        let clip_chain = MpPrimitiveClipChain {
            origin_clip_rect: Some(Rect {
                pos: dvec2(30.0, 40.0),
                size: dvec2(100.0, 80.0),
            }),
            entries: Vec::new(),
        };

        let scene_from_origin = Mat4f::identity();
        let world_from_scene = Mat4f::translation(vec3(-30.0, -40.0, 0.0));
        let clip_from_world = Mat4f::identity();
        let basis = MpClipBasis::new(scene_from_origin, world_from_scene, clip_from_world);
        let evaluated = evaluate_clip_chain(&clip_chain, &basis);

        assert!(point_passes_planes(&evaluated.clip_planes, &basis, 80.0, 80.0));
        assert!(!point_passes_planes(&evaluated.clip_planes, &basis, 0.0, 0.0));
    }
}
