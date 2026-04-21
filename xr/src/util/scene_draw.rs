use makepad_widgets::makepad_draw::*;

pub use makepad_widgets::makepad_draw::SceneState3D;

pub fn scene_state_from_cx(cx: &mut Cx3d) -> Option<SceneState3D> {
    cx.scene_state_3d()
}

pub fn scene_node_world_transform_from_cx(cx: &mut Cx3d) -> Mat4f {
    cx.scene_world_transform_3d()
}

pub fn compose_scene_node_transform(position: Vec3f, rotation: Vec3f, scale: Vec3f) -> Mat4f {
    Mat4f::mul(
        &Mat4f::translation(position),
        &Mat4f::mul(
            &Mat4f::rotation(rotation),
            &Mat4f::nonuniform_scaled_translation(scale, vec3(0.0, 0.0, 0.0)),
        ),
    )
}

pub fn ray_from_scene_viewport(scene: &SceneState3D, abs: DVec2) -> Option<(Vec3f, Vec3f)> {
    let rect = scene.viewport_rect;
    if rect.size.x <= 1.0 || rect.size.y <= 1.0 || !rect.contains(abs) {
        return None;
    }

    let sx = ((abs.x - rect.pos.x) / rect.size.x).clamp(0.0, 1.0) as f32;
    let sy = ((abs.y - rect.pos.y) / rect.size.y).clamp(0.0, 1.0) as f32;
    let ndc_x = sx * 2.0 - 1.0;
    let ndc_y = 1.0 - sy * 2.0;

    let inv_projection = scene.projection.invert();
    let inv_view = scene.view.invert();

    let near_view = inv_projection.transform_vec4(vec4(ndc_x, ndc_y, -1.0, 1.0));
    let far_view = inv_projection.transform_vec4(vec4(ndc_x, ndc_y, 1.0, 1.0));
    if near_view.w.abs() <= 1.0e-6 || far_view.w.abs() <= 1.0e-6 {
        return None;
    }

    let near_view = vec4(
        near_view.x / near_view.w,
        near_view.y / near_view.w,
        near_view.z / near_view.w,
        1.0,
    );
    let far_view = vec4(
        far_view.x / far_view.w,
        far_view.y / far_view.w,
        far_view.z / far_view.w,
        1.0,
    );

    let near_world = inv_view.transform_vec4(near_view);
    let far_world = inv_view.transform_vec4(far_view);
    if near_world.w.abs() <= 1.0e-6 || far_world.w.abs() <= 1.0e-6 {
        return None;
    }

    let near_world = vec3f(
        near_world.x / near_world.w,
        near_world.y / near_world.w,
        near_world.z / near_world.w,
    );
    let far_world = vec3f(
        far_world.x / far_world.w,
        far_world.y / far_world.w,
        far_world.z / far_world.w,
    );
    let dir = far_world - near_world;
    if dir.length() <= 1.0e-6 {
        return None;
    }

    Some((scene.camera_pos, dir.normalize()))
}

#[allow(dead_code)]
pub fn register_draw_call_anchor(cx: &mut Cx3d, area: Area, world_pos: Vec3f) {
    cx.register_scene_draw_call_anchor_3d(area, world_pos);
}

#[allow(dead_code)]
pub fn register_last_draw_call_anchor(cx: &mut Cx3d, world_pos: Vec3f) {
    let Some(draw_list_id) = cx.get_current_draw_list_id() else {
        return;
    };
    let draw_item_id = {
        let draw_list = &cx.draw_lists[draw_list_id];
        let len = draw_list.draw_items.len();
        if len == 0 {
            return;
        }
        len - 1
    };
    cx.register_last_scene_draw_call_anchor_3d(draw_list_id, draw_item_id, world_pos);
}

pub fn apply_scene_to_draw_cube(_draw: &mut DrawCube, cx: &mut Cx3d) -> Option<SceneState3D> {
    let scene = cx.scene_state_3d()?;
    Some(scene)
}

pub fn apply_scene_to_draw_pbr(draw: &mut DrawPbr, cx: &mut Cx3d) -> Option<SceneState3D> {
    let scene = cx.scene_state_3d()?;
    if draw.has_env_texture < 0.5 {
        let env_texture = draw.default_env_texture(cx);
        draw.set_env_texture(Some(env_texture));
    }
    draw.reset_matrix();
    Some(scene)
}
