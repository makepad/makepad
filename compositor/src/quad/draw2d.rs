use super::clip::{current_view_projection, set_clip_masks, set_clip_planes, PROJECTED_CORNER_IDS};
use super::shaders::{DrawMaskedProjectiveQuad, DrawProjectedCornerQuad, DrawProjectedQuad};
use super::types::MpCompositedQuad;
use crate::*;

pub(crate) fn draw_composited_quad(
    cx: &mut Cx2d,
    quad: &MpCompositedQuad,
    draw_quad: &mut DrawProjectedQuad,
    draw_corner_quad: &mut DrawProjectedCornerQuad,
    _draw_projective_quad: &mut DrawMaskedProjectiveQuad,
) {
    if !quad.backface_visible && is_backface_culled(cx, quad) {
        return;
    }

    if quad.transform.v[3].abs() > 1e-6
        || quad.transform.v[7].abs() > 1e-6
        || quad.transform.v[11].abs() > 1e-6
    {
        let corners = [
            project_clip_corner(&quad.transform, quad.local_rect.pos.x as f32, quad.local_rect.pos.y as f32),
            project_clip_corner(
                &quad.transform,
                (quad.local_rect.pos.x + quad.local_rect.size.x) as f32,
                quad.local_rect.pos.y as f32,
            ),
            project_clip_corner(
                &quad.transform,
                quad.local_rect.pos.x as f32,
                (quad.local_rect.pos.y + quad.local_rect.size.y) as f32,
            ),
            project_clip_corner(
                &quad.transform,
                (quad.local_rect.pos.x + quad.local_rect.size.x) as f32,
                (quad.local_rect.pos.y + quad.local_rect.size.y) as f32,
            ),
        ];

        draw_corner_quad.draw_super.rect_pos = dvec2(0.0, 0.0).into();
        draw_corner_quad.draw_super.rect_size = dvec2(1.0, 1.0).into();
        draw_corner_quad.uv_rect = rect_to_uv_vec4(quad.uv_rect);
        draw_corner_quad.opacity = quad.opacity.clamp(0.0, 1.0);
        draw_corner_quad.premultiplied = if quad.premultiplied { 1.0 } else { 0.0 };
        draw_corner_quad.draw_super.draw_vars.options.depth_write = quad.depth_write;
        draw_corner_quad
            .draw_super
            .draw_vars
            .set_texture(0, &quad.texture);
        set_clip_planes(
            cx,
            &mut draw_corner_quad.draw_super.draw_vars,
            &quad.clip_planes,
        );
        set_clip_masks(
            cx,
            &mut draw_corner_quad.draw_super.draw_vars,
            &quad.mask,
        );
        set_projected_corners(
            cx,
            &mut draw_corner_quad.draw_super.draw_vars,
            &corners,
        );
        draw_corner_quad.draw(cx);
        return;
    }

    draw_quad.draw_super.rect_pos = quad.local_rect.pos.into();
    draw_quad.draw_super.rect_size = quad.local_rect.size.into();
    draw_quad.transform = quad.transform;
    draw_quad.uv_rect = rect_to_uv_vec4(quad.uv_rect);
    draw_quad.opacity = quad.opacity.clamp(0.0, 1.0);
    draw_quad.premultiplied = if quad.premultiplied { 1.0 } else { 0.0 };
    draw_quad.draw_super.draw_vars.options.depth_write = quad.depth_write;
    draw_quad
        .draw_super
        .draw_vars
        .set_texture(0, &quad.texture);
    set_clip_planes(
        cx,
        &mut draw_quad.draw_super.draw_vars,
        &quad.clip_planes,
    );
    set_clip_masks(cx, &mut draw_quad.draw_super.draw_vars, &quad.mask);
    draw_quad.draw(cx);
}

fn set_projected_corners(cx: &Cx2d, draw_vars: &mut DrawVars, corners: &[Vec4f; 4]) {
    for (index, id) in PROJECTED_CORNER_IDS.iter().enumerate() {
        let corner = corners[index];
        draw_vars.set_uniform(cx.cx, *id, &[corner.x, corner.y, corner.z, corner.w]);
    }
}

fn rect_to_uv_vec4(rect: Rect) -> Vec4f {
    vec4(
        rect.pos.x as f32,
        rect.pos.y as f32,
        (rect.pos.x + rect.size.x) as f32,
        (rect.pos.y + rect.size.y) as f32,
    )
}

fn project_clip_corner(transform: &Mat4f, x: f32, y: f32) -> Vec4f {
    transform.transform_vec4(vec4f(x, y, 0.0, 1.0))
}

fn is_backface_culled(cx: &Cx2d, quad: &MpCompositedQuad) -> bool {
    let Some((pass_view_projection, draw_list_transform)) = current_view_projection(cx) else {
        return false;
    };

    let reference_mvp = Mat4f::mul(&pass_view_projection, &draw_list_transform);
    let transformed_mvp = Mat4f::mul(&reference_mvp, &quad.transform);

    let Some(reference_area) = projected_signed_area(&reference_mvp, quad.local_rect) else {
        return false;
    };
    let Some(quad_area) = projected_signed_area(&transformed_mvp, quad.local_rect) else {
        return false;
    };

    if reference_area.abs() <= 1e-6 || quad_area.abs() <= 1e-6 {
        return false;
    }

    reference_area.is_sign_positive() != quad_area.is_sign_positive()
}

fn projected_signed_area(mvp: &Mat4f, rect: Rect) -> Option<f32> {
    let p0 = project_point(mvp, rect.pos.x as f32, rect.pos.y as f32)?;
    let p1 = project_point(mvp, (rect.pos.x + rect.size.x) as f32, rect.pos.y as f32)?;
    let p2 = project_point(
        mvp,
        (rect.pos.x + rect.size.x) as f32,
        (rect.pos.y + rect.size.y) as f32,
    )?;
    Some((p1.x - p0.x) * (p2.y - p0.y) - (p1.y - p0.y) * (p2.x - p0.x))
}

fn project_point(mvp: &Mat4f, x: f32, y: f32) -> Option<Vec2f> {
    let clip = mvp.transform_vec4(vec4f(x, y, 0.0, 1.0));
    if clip.w.abs() <= 1e-6 {
        return None;
    }
    Some(vec2(clip.x / clip.w, clip.y / clip.w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winding_flips_for_half_turn_around_y() {
        let rect = Rect {
            pos: dvec2(-1.0, -1.0),
            size: dvec2(2.0, 2.0),
        };
        let reference_mvp = Mat4f::identity();
        let flipped_mvp = Mat4f::mul(
            &reference_mvp,
            &Mat4f::rotation(vec3(0.0, std::f32::consts::PI, 0.0)),
        );

        let reference_area = projected_signed_area(&reference_mvp, rect).unwrap();
        let flipped_area = projected_signed_area(&flipped_mvp, rect).unwrap();

        assert!(reference_area.abs() > 0.0);
        assert!(flipped_area.abs() > 0.0);
        assert_ne!(
            reference_area.is_sign_positive(),
            flipped_area.is_sign_positive()
        );
    }
}
