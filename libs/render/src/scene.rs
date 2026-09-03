//! Per-view camera: scene-state construction and pass-uniform upload, moved
//! verbatim from gamemaker's game_view.rs. The orbit state is a per-view
//! input ([`CameraRig`]) — N views over one world each bring their own rig.

use makepad_draw::*;
use makepad_game_sim::{
    camera_boom_limit, camera_shake_offset, heading_to_forward, GameWorld,
};

/// The view-local half of the camera: mouse-orbit angles plus the "input is
/// pinned by a tape test" flag (shake reads as zero under tapes).
#[derive(Clone, Copy, Debug)]
pub struct CameraRig {
    pub yaw: f32,
    pub pitch: f32,
    pub in_test: bool,
}


/// The world's near plane: a derive-Default 0 means the stock 0.15, and
/// explicit values clamp to sane metres.
fn scene_near(cam_near: f32) -> f32 {
    if cam_near <= 0.0 { 0.15 } else { cam_near.clamp(0.02, 1.0) }
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
            // The filmed body is never its own obstruction.
            let boom = camera_boom_limit(world, pivot, forward * -1.0, world.cam_boom, world.cam_third);
            let mut camera_pos = pivot - forward * boom;
            camera_pos = camera_pos + camera_shake_offset(world, in_test);
            let view = Mat4f::look_at(camera_pos, pivot, vec3f(0.0, 1.0, 0.0));
            let aspect = (rect.size.x / rect.size.y).max(0.001) as f32;
            // Near plane 1.0 (Godot's CAM_NEAR): a creature overlapping the
            // lens clips open instead of filling the screen with one giant
            // polygon. FOV is script-tunable (racing games widen with speed).
            let projection = Mat4f::perspective(
                world.cam_fov.clamp(20.0, 120.0),
                aspect,
                scene_near(world.cam_near),
                500.0,
            );
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
    let projection = Mat4f::perspective(
        world.cam_fov.clamp(20.0, 120.0),
        aspect,
        scene_near(world.cam_near),
        500.0,
    );
    Some(SceneState3D {
        time,
        camera_pos,
        view,
        projection,
        viewport_rect: rect,
    })
}

/// Driver-eye camera for a vehicle whose PLAYER is the vehicle itself.
///
/// Direct-control racing worlds intentionally have no walking `PlayerRig`,
/// but first/third person is still a view-local choice.  The third-person
/// path continues through [`scene_state`]; this companion only builds the
/// in-car shot.  Position follows the chassis immediately (lag inside a car
/// reads as the driver's head floating loose), while roll and pitch from the
/// rigid body are deliberately excluded so a suspension impulse cannot tilt
/// the horizon or make the player motion-sick.  Free-look still comes from
/// the view-local yaw/pitch in [`CameraRig`].
pub fn vehicle_cockpit_scene_state(
    world: &GameWorld,
    rect: Rect,
    time: f64,
    rig: &CameraRig,
    vehicle: u64,
) -> Option<SceneState3D> {
    if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
        return None;
    }
    let entity = world.entity(vehicle)?;

    // A little forward of the chassis centre and just below its roof: this is
    // the driver's seat, not a bumper camera.  Deriving it from the collider
    // keeps generated compact cars and long race cars usable without another
    // per-model camera table.  The exterior for this one local car is hidden
    // by the caller, so near-plane clipping cannot expose its roof polygons.
    let body_forward = heading_to_forward(entity.visual_heading());
    let eye = entity.pos
        + vec3f(0.0, entity.half.y + 0.52, 0.0)
        + body_forward * (entity.half.z * 0.18);
    let yaw = rig.yaw;
    // A cockpit can glance up/down, but not pitch far enough that the road
    // fills the entire near plane (or the sky becomes the whole windshield).
    // Wide free-look remains available in the exterior chase camera.
    let pitch = rig.pitch.clamp(-0.40, 0.35);
    let forward = vec3f(
        yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    )
    .normalize();
    let view = Mat4f::look_at(eye, eye + forward * 20.0, vec3f(0.0, 1.0, 0.0));
    let aspect = (rect.size.x / rect.size.y).max(0.001) as f32;
    // The slightly wider floor sells speed and preserves peripheral track
    // markers in a narrow split pane.  Authored wider FOVs still win.
    let projection = Mat4f::perspective(world.cam_fov.clamp(64.0, 120.0), aspect, 0.08, 500.0);
    Some(SceneState3D {
        time,
        camera_pos: eye,
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
    // A direct camera write: the web backend uploads a pass block only when
    // its generation moved.
    cx.passes[pass.draw_pass_id()].mark_pass_uniforms_dirty();
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_game_sim::{Entity, PlayerId};

    fn cockpit_world(yaw: f32) -> GameWorld {
        let (s, c) = makepad_game_sim::math::sincos(yaw * 0.5);
        let mut world = GameWorld::default();
        world.cam_fov = 58.0;
        world.push_entity(Entity {
            id: 7,
            pos: vec3f(4.0, 1.1, -6.0),
            half: vec3f(0.95, 0.35, 1.85),
            yaw: -1.1, // stale spawn heading must not own a rigid cockpit.
            orient: Quat {
                x: 0.0,
                y: s,
                z: 0.0,
                w: c,
            },
            ..Default::default()
        });
        world.players.local_mut().id = PlayerId::LOCAL;
        world
    }

    #[test]
    fn cockpit_eye_tracks_rigid_heading_and_uses_a_driver_height() {
        let yaw = 0.6;
        let world = cockpit_world(yaw);
        let rig = CameraRig {
            yaw: makepad_game_sim::heading_to_camera_yaw(yaw),
            pitch: -0.08,
            in_test: false,
        };
        let rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(1280.0, 720.0),
        };
        let scene = vehicle_cockpit_scene_state(&world, rect, 3.0, &rig, 7).unwrap();
        let entity = world.entity(7).unwrap();
        let expected = entity.pos
            + vec3f(0.0, entity.half.y + 0.52, 0.0)
            + heading_to_forward(yaw) * (entity.half.z * 0.18);
        assert!((scene.camera_pos - expected).length() < 1.0e-5);
        assert!(scene.camera_pos.y > entity.pos.y + entity.half.y);
    }

    #[test]
    fn cockpit_is_view_local_and_missing_vehicle_fails_closed() {
        let world = cockpit_world(0.0);
        let rig = CameraRig {
            yaw: 0.4,
            pitch: 0.2,
            in_test: false,
        };
        let rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(640.0, 360.0),
        };
        assert!(vehicle_cockpit_scene_state(&world, rect, 0.0, &rig, 99).is_none());
        let scene = vehicle_cockpit_scene_state(&world, rect, 0.0, &rig, 7).unwrap();
        // Camera construction never mutates the shared entity or world view.
        assert_eq!(world.entity(7).unwrap().yaw, -1.1);
        assert_eq!(world.cam_fov, 58.0);
        assert_eq!(scene.viewport_rect, rect);
    }

    #[test]
    fn chase_camera_is_geometrically_behind_the_live_rigid_heading() {
        let yaw = -0.9;
        let mut world = cockpit_world(yaw);
        world.cam_third = 7;
        world.cam_height = 1.35;
        world.cam_boom = 7.8;
        world.cam_fov = 64.0;
        let rig = CameraRig {
            yaw: makepad_game_sim::heading_to_camera_yaw(yaw),
            pitch: -0.22,
            in_test: true,
        };
        let rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(1280.0, 720.0),
        };
        let scene = scene_state(&world, rect, 0.0, &rig).unwrap();
        let pivot = world.entity(7).unwrap().pos + vec3f(0.0, world.cam_height, 0.0);
        let eye_to_car = pivot - scene.camera_pos;
        let horizontal = vec3f(eye_to_car.x, 0.0, eye_to_car.z).normalize();
        assert!(
            horizontal.dot(heading_to_forward(yaw)) > 0.999,
            "the chase eye must look forward through the rear of the car, not across its side",
        );
    }
}
