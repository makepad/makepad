//! Per-view camera: scene-state construction and pass-uniform upload, moved
//! verbatim from gamemaker's game_view.rs. The orbit state is a per-view
//! input ([`CameraRig`]) — N views over one world each bring their own rig.

use makepad_draw::*;
use makepad_game_sim::{camera_boom_limit, camera_shake_offset, GameWorld};

/// The view-local half of the camera: mouse-orbit angles plus the "input is
/// pinned by a tape test" flag (shake reads as zero under tapes).
#[derive(Clone, Copy, Debug)]
pub struct CameraRig {
    pub yaw: f32,
    pub pitch: f32,
    pub in_test: bool,
}

pub fn scene_state(
    world: &GameWorld,
    rect: Rect,
    time: f64,
    rig: &CameraRig,
) -> Option<SceneState3D> {
    if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
        return None;
    }
    // Tape runs pin the camera completely — captures must not depend on
    // where the kid happened to leave the mouse.
    let in_test = rig.in_test;

    // Third-person rig: pivot above the entity, drag orbits around it,
    // boom slides in when geometry blocks the view (the Godot player cam).
    if world.cam_third != 0 {
        if let Some(e) = world.entity(world.cam_third) {
            let pivot = e.pos + vec3f(0.0, world.cam_height, 0.0);
            // No per-frame test pin: test start pins the orbit once and
            // the mouse is inert during tests, so script camera writes
            // render (and stay deterministic).
            let (yaw, pitch) = (rig.yaw, rig.pitch.clamp(-1.2, 0.25));
            let forward = vec3f(
                yaw.sin() * pitch.cos(),
                pitch.sin(),
                -yaw.cos() * pitch.cos(),
            )
            .normalize();
            let boom = camera_boom_limit(world, pivot, forward * -1.0, world.cam_boom);
            let mut camera_pos = pivot - forward * boom;
            camera_pos = camera_pos + camera_shake_offset(world, in_test);
            let view = Mat4f::look_at(camera_pos, pivot, vec3f(0.0, 1.0, 0.0));
            let aspect = (rect.size.x / rect.size.y).max(0.001) as f32;
            // Near plane 1.0 (Godot's CAM_NEAR): a creature overlapping the
            // lens clips open instead of filling the screen with one giant
            // polygon. FOV is script-tunable (racing games widen with speed).
            let projection =
                Mat4f::perspective(world.cam_fov.clamp(20.0, 120.0), aspect, 1.0, 500.0);
            return Some(SceneState3D {
                time,
                camera_pos,
                view,
                projection,
                viewport_rect: rect,
            });
        }
    }

    let mut target = world.cam_target;
    if world.cam_follow != 0 {
        if let Some(e) = world.entity(world.cam_follow) {
            target = e.pos;
        }
    }
    let distance = world.cam_distance.max(0.5);
    let (yaw, pitch) = if world.cam_side {
        // Side-on 2D style camera: look down -z.
        (0.0f32, -0.08f32)
    } else {
        // Tests pinned at test START (orbit reset once, mouse inert).
        (rig.yaw, rig.pitch.clamp(-1.45, 1.45))
    };
    let forward = vec3f(
        yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    )
    .normalize();
    // Camera sits behind the target looking along `forward` at it.
    let mut camera_pos = target - forward * distance;
    camera_pos = camera_pos + camera_shake_offset(world, in_test);
    let view = Mat4f::look_at(camera_pos, target, vec3f(0.0, 1.0, 0.0));
    let aspect = (rect.size.x / rect.size.y).max(0.001) as f32;
    let projection = Mat4f::perspective(world.cam_fov.clamp(20.0, 120.0), aspect, 1.0, 500.0);
    Some(SceneState3D {
        time,
        camera_pos,
        view,
        projection,
        viewport_rect: rect,
    })
}

pub fn set_pass_camera(cx: &mut Cx, pass: &DrawPass, scene: &SceneState3D) {
    let camera_inv = scene.view.invert();
    let pass_uniforms = &mut cx.passes[pass.draw_pass_id()].pass_uniforms;
    pass_uniforms.camera_projection = scene.projection;
    pass_uniforms.camera_projection_r = scene.projection;
    pass_uniforms.camera_view = scene.view;
    pass_uniforms.camera_view_r = scene.view;
    pass_uniforms.depth_projection = scene.projection;
    pass_uniforms.depth_projection_r = scene.projection;
    pass_uniforms.depth_view = scene.view;
    pass_uniforms.depth_view_r = scene.view;
    pass_uniforms.camera_inv = camera_inv;
    pass_uniforms.camera_inv_r = camera_inv;
}
