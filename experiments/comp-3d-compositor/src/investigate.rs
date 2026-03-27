use super::surface_scene_3d::surface_scene_state_for_rect;
use super::textured_plane_3d::{textured_plane_local_scale, textured_plane_model_matrix};
use makepad_widgets::*;

fn plane_unit_positions() -> [[f32; 3]; 4] {
    [
        [-0.5, 0.0, -0.5],
        [0.5, 0.0, -0.5],
        [-0.5, 0.0, 0.5],
        [0.5, 0.0, 0.5],
    ]
}

fn scene_state() -> super::surface_scene_3d::SurfaceSceneState3D {
    surface_scene_state_for_rect(
        Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(1400.0, 900.0),
        },
        dvec2(1400.0, 900.0),
        46.0,
        6.2,
        0.02,
        400.0,
        vec3(0.0, 0.85, 0.0),
        vec2(0.0, 1.0),
        0.0,
    )
}

fn plane_world_points(position: Vec3f, rotation: Vec3f, size: Vec2f) -> [Vec3f; 4] {
    let model = textured_plane_model_matrix(position, rotation, vec3(1.0, 1.0, 1.0));
    let scale = textured_plane_local_scale(size);
    plane_unit_positions().map(|p| {
        let local = vec4(p[0] * scale.x, p[1] * scale.y, p[2] * scale.z, 1.0);
        let world = model.transform_vec4(local);
        vec3(world.x / world.w, world.y / world.w, world.z / world.w)
    })
}

fn plane_ndc_points(position: Vec3f, rotation: Vec3f, size: Vec2f) -> [Vec3f; 4] {
    let scene = scene_state();
    let model = textured_plane_model_matrix(position, rotation, vec3(1.0, 1.0, 1.0));
    let scale = textured_plane_local_scale(size);
    plane_unit_positions().map(|p| {
        let local = vec4(p[0] * scale.x, p[1] * scale.y, p[2] * scale.z, 1.0);
        let world = model.transform_vec4(local);
        let view = scene.view.transform_vec4(world);
        let clip = scene.projection_viewport.transform_vec4(view);
        vec3(clip.x / clip.w, clip.y / clip.w, clip.z / clip.w)
    })
}

fn ndc_bounds(points: &[Vec3f; 4]) -> (f32, f32, f32, f32) {
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for p in points {
        min_x = min_x.min(p.x);
        max_x = max_x.max(p.x);
        min_y = min_y.min(p.y);
        max_y = max_y.max(p.y);
    }
    (min_x, max_x, min_y, max_y)
}

#[test]
fn zero_rotation_panel_faces_camera() {
    let world = plane_world_points(vec3(0.0, 0.9, 0.0), vec3(0.0, 0.0, 0.0), vec2(2.2, 1.8));
    let edge_a = world[2] - world[0];
    let edge_b = world[1] - world[0];
    let normal = Vec3f::cross(edge_a, edge_b).normalize();
    let center = (world[0] + world[1] + world[2] + world[3]) * 0.25;
    let to_camera = (scene_state().camera_pos - center).normalize();
    assert!(normal.dot(to_camera) > 0.9, "normal={normal:?} to_camera={to_camera:?}");
}

#[test]
fn three_row_panels_project_to_three_visible_columns() {
    let x_plane = plane_ndc_points(vec3(-2.7, 0.9, 0.0), vec3(0.55, 0.0, 0.0), vec2(2.2, 1.8));
    let y_plane = plane_ndc_points(vec3(0.0, 0.9, 0.0), vec3(0.0, 0.8, 0.0), vec2(2.2, 1.8));
    let z_plane = plane_ndc_points(vec3(2.7, 0.9, 0.0), vec3(0.0, 0.0, 0.45), vec2(2.2, 1.8));

    let xb = ndc_bounds(&x_plane);
    let yb = ndc_bounds(&y_plane);
    let zb = ndc_bounds(&z_plane);

    assert!(xb.1 < yb.0, "x={xb:?} y={yb:?}");
    assert!(yb.1 < zb.0, "y={yb:?} z={zb:?}");

    for bounds in [xb, yb, zb] {
        assert!(bounds.1 > -1.0 && bounds.0 < 1.0, "x bounds offscreen: {bounds:?}");
        assert!(bounds.3 > -1.0 && bounds.2 < 1.0, "y bounds offscreen: {bounds:?}");
        assert!(bounds.1 - bounds.0 > 0.12, "panel too narrow: {bounds:?}");
        assert!(bounds.3 - bounds.2 > 0.18, "panel too short: {bounds:?}");
    }
}
