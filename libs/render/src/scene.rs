//! Per-view camera: scene-state construction and pass-uniform upload, moved
//! verbatim from gamemaker's game_view.rs. The orbit state is a per-view
//! input ([`CameraRig`]) — N views over one world each bring their own rig.

use makepad_draw::*;
use makepad_scene::{
    heading_to_forward, CameraEffects, World,
};

/// The view-local half of the camera: mouse-orbit angles plus the "input is
/// pinned by a tape test" flag (shake reads as zero under tapes).
#[derive(Clone, Copy, Debug)]
pub struct CameraRig {
    pub yaw: f32,
    pub pitch: f32,
    pub in_test: bool,
    pub effects: CameraEffects,
}


/// The world's near plane: a derive-Default 0 means the stock 0.15, and
/// explicit values clamp to sane metres.
fn scene_near(cam_near: f32) -> f32 {
    if cam_near <= 0.0 { 0.15 } else { cam_near.clamp(0.02, 1.0) }
}

/// The camera's far plane: 0 = the stock 500 m, else clamped to 50..40000.
pub fn scene_far(cam_far: f32) -> f32 {
    if cam_far <= 0.0 { 500.0 } else { cam_far.clamp(50.0, 40_000.0) }
}

/// The far plane a world renders with. An explicit `camera.far` wins. The
/// stock one (0) is 500 m, stretched so (1) the fog has hidden the world
/// before geometry ends — τ = 3.5, ~97 % fog at ground level; with the
/// default sky that is ~2.3 km, so a distant overview fades into haze
/// instead of stopping at a hard terrain edge — and (2) an orbit camera
/// pulled far back still sees past its own target.
pub fn world_far(world: &World, orbit_distance: f32) -> f32 {
    if world.camera.far > 0.0 {
        return scene_far(world.camera.far);
    }
    let fog = world.sky.as_ref().map_or(0.0, |sky| sky.fog);
    let fogged = if fog > 1.0e-6 { (3.5 / fog).clamp(500.0, 4000.0) } else { 500.0 };
    fogged.max(orbit_distance * 2.5).min(40_000.0)
}

/// Near plane for a far plane: long views raise the near plane so the depth
/// ratio stays within what a D32 float buffer resolves (about 1:50000).
fn scene_near_for(cam_near: f32, far: f32) -> f32 {
    scene_near(cam_near).max(far * 2.0e-5)
}

pub fn scene_state(
    world: &World,
    rect: Rect,
    time: f64,
    rig: &CameraRig,
) -> Option<SceneState3D> {
    if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
        return None;
    }
    // Third-person rig: pivot above the entity, drag orbits around it,
    // boom slides in when geometry blocks the view (the Godot player cam).
    if world.camera.third != 0 {
        if let Some(e) = world.entity(world.camera.third) {
            let pivot = e.pos + vec3f(0.0, world.camera.height, 0.0);
            // No per-frame test pin: test start pins the orbit once and
            // the mouse is inert during tests, so script camera writes
            // render (and stay deterministic).
            // A non-finite angle or boom (a NaN written by a script or a
            // degenerate follow) would poison every matrix and draw the
            // whole scene black; hold a sane shot instead.
            let yaw = if rig.yaw.is_finite() { rig.yaw } else { 0.0 };
            let pitch = if rig.pitch.is_finite() { rig.pitch.clamp(-1.2, 0.25) } else { -0.3 };
            let forward = vec3f(
                yaw.sin() * pitch.cos(),
                pitch.sin(),
                -yaw.cos() * pitch.cos(),
            )
            .normalize();
            // The filmed body is never its own obstruction.
            let boom = rig.effects.boom_limit.unwrap_or(world.camera.boom);
            let boom = if boom.is_finite() { boom.max(0.0) } else { 4.0 };
            let mut camera_pos = pivot - forward * boom;
            camera_pos = camera_pos + rig.effects.shake_offset;
            let view = Mat4f::look_at(camera_pos, pivot, vec3f(0.0, 1.0, 0.0));
            let aspect = (rect.size.x / rect.size.y).max(0.001) as f32;
            // Near plane 1.0 (Godot's CAM_NEAR): a creature overlapping the
            // lens clips open instead of filling the screen with one giant
            // polygon. FOV is script-tunable (racing games widen with speed).
            let far = world_far(world, boom);
            let projection = Mat4f::perspective(
                world.camera.fov.clamp(20.0, 120.0),
                aspect,
                scene_near_for(world.camera.near, far),
                far,
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

    let mut target = world.camera.target;
    if world.camera.follow != 0 {
        if let Some(e) = world.entity(world.camera.follow) {
            target = e.pos;
        }
    }
    let distance = world.camera.distance.max(0.5);
    let (yaw, pitch) = if world.camera.side {
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
    camera_pos = camera_pos + rig.effects.shake_offset;
    let view = Mat4f::look_at(camera_pos, target, vec3f(0.0, 1.0, 0.0));
    let aspect = (rect.size.x / rect.size.y).max(0.001) as f32;
    let far = world_far(world, distance);
    let projection = Mat4f::perspective(
        world.camera.fov.clamp(20.0, 120.0),
        aspect,
        scene_near_for(world.camera.near, far),
        far,
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
    world: &World,
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
    Some(cockpit_scene_state_at(world, rect, time, rig, eye))
}

/// The in-car shot from an exact eye point: the vehicle model's own
/// `cockpit` socket (in front of the driver's visor), when it authors one.
/// The exterior then stays drawn and the modelled interior (dash, wheel and
/// hands, pillars, mirrors) frames the view. Same look limits and lens as
/// [`vehicle_cockpit_scene_state`].
pub fn cockpit_scene_state_at(
    world: &World,
    rect: Rect,
    time: f64,
    rig: &CameraRig,
    eye: Vec3f,
) -> SceneState3D {
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
    let far = world_far(world, 0.0);
    let projection = Mat4f::perspective(world.camera.fov.clamp(64.0, 120.0), aspect, 0.05_f32.max(far * 2.0e-5), far);
    SceneState3D {
        time,
        camera_pos: eye,
        view,
        projection,
        viewport_rect: rect,
    }
}

/// Camera-relative rendering: the render origin for a camera. Zero inside
/// 2 km of the world origin (every scene there renders exactly as it always
/// did); beyond, the camera position snapped to 1 km. The scene draw list
/// shifts world geometry by -origin (an exact f32 subtraction: both are
/// multiples of the coordinates' own ulp) and the pass view is rebuilt
/// around the origin in f64, so the GPU never multiplies a 10 km world
/// position by a 10 km view translation — the per-vertex wobble that cost
/// ~1 px at 10 km is gone. Lighting still reads TRUE world positions
/// (`transform * pos`), so no uniform family moves.
pub fn render_origin(camera: Vec3f) -> Vec3f {
    const NEAR: f32 = 2048.0;
    const SNAP: f32 = 1024.0;
    if !(camera.x.is_finite() && camera.y.is_finite() && camera.z.is_finite()) { return Vec3f::default(); }
    if camera.x.abs().max(camera.y.abs()).max(camera.z.abs()) < NEAR { return Vec3f::default(); }
    let s = |v: f32| (v / SNAP).round() * SNAP;
    vec3f(s(camera.x), s(camera.y), s(camera.z))
}

/// `view * translate(origin)`. When `eye` is the view's own eye (a rigid
/// look-at view, the normal case) the translation is rebuilt from
/// `eye - origin` — small, so exact — rather than from the view's f32
/// translation, which already rounded `-R * eye` to the ulp of kilometres
/// and would make the whole world swim by that much frame to frame.
pub fn relative_view(view: &Mat4f, eye: Vec3f, origin: Vec3f) -> Mat4f {
    let v = &view.v;
    let r = |i: usize, j: usize| v[j * 4 + i] as f64;
    let mut out = *view;
    // The eye this view implies: -R^T t.
    let t = [v[12] as f64, v[13] as f64, v[14] as f64];
    let implied = [0, 1, 2].map(|j| -(r(0, j) * t[0] + r(1, j) * t[1] + r(2, j) * t[2]));
    let e = [eye.x as f64, eye.y as f64, eye.z as f64];
    let o = [origin.x as f64, origin.y as f64, origin.z as f64];
    let consistent = (0..3).all(|k| (implied[k] - e[k]).abs() < 0.5);
    for i in 0..3 {
        out.v[12 + i] = if consistent {
            let d = [e[0] - o[0], e[1] - o[1], e[2] - o[2]];
            (-(r(i, 0) * d[0] + r(i, 1) * d[1] + r(i, 2) * d[2])) as f32
        } else {
            (t[i] + r(i, 0) * o[0] + r(i, 1) * o[1] + r(i, 2) * o[2]) as f32
        };
    }
    out
}

/// [`set_pass_camera`] for a camera-relative host: the view is rebuilt
/// around `origin` (see [`render_origin`]); the scene draw must shift world
/// geometry by the same origin (`Renderer::set_camera_relative`).
pub fn set_pass_camera_origin(cx: &mut Cx, pass: &DrawPass, scene: &SceneState3D, origin: Vec3f) {
    let mut relative = *scene;
    relative.view = relative_view(&scene.view, scene.camera_pos, origin);
    set_pass_camera(cx, pass, &relative);
}

/// A scene's camera on `pass` ([`DrawPass::set_camera`]).
pub fn set_pass_camera(cx: &mut Cx, pass: &DrawPass, scene: &SceneState3D) {
    pass.set_camera(cx, scene.view, scene.projection);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wobble test: a point 2 m in front of a camera 10 km out, moved
    /// through 50 camera positions a millimetre apart. Absolute f32 (the
    /// old path) jitters the projected point by a pixel-scale amount;
    /// camera-relative stays within float noise of the f64 truth.
    /// The stock far plane reaches past where the fog has hidden the world
    /// (overviews fade into haze, not to a hard edge) and past a far-pulled
    /// orbit camera's target; an authored far still wins.
    #[test]
    fn the_stock_far_plane_outruns_the_fog_and_the_orbit() {
        let mut world = World::default();
        assert_eq!(world_far(&world, 10.0), 500.0, "no sky, no fog: the stock 500 m");
        world.sky = Some(makepad_scene::SkyConfig::default());
        let fog = world.sky.unwrap().fog;
        let far = world_far(&world, 10.0);
        assert!((-(fog * far)).exp() < 0.05, "fog {fog} leaves {:.2} of the world visible at far {far}", (-(fog * far)).exp());
        assert!(world_far(&world, 2000.0) >= 5000.0);
        world.camera.far = 300.0;
        assert_eq!(world_far(&world, 2000.0), 300.0);
    }

    #[test]
    fn camera_relative_view_removes_the_far_from_origin_wobble() {
        let proj = Mat4f::perspective(60.0, 16.0 / 9.0, 0.2, 5000.0);
        let px = |ndc: f32| ndc as f64 * 960.0;
        let mut worst_abs: f64 = 0.0;
        let mut worst_rel: f64 = 0.0;
        for k in 0..50 {
            let eye = vec3f(10_000.37 + k as f32 * 0.001, 1.7, -9_500.11);
            let target = eye + vec3f(0.3, 0.0, -1.0);
            let p = eye + vec3f(0.6, 0.1, -2.0);
            let view = Mat4f::look_at(eye, target, vec3f(0.0, 1.0, 0.0));
            // f64 truth: the point relative to the eye, through the rotation only.
            let rel = [(p.x as f64 - eye.x as f64), (p.y as f64 - eye.y as f64), (p.z as f64 - eye.z as f64)];
            let vt = |m: &Mat4f, x: [f64; 3]| -> [f64; 3] { let v = &m.v; [0, 1, 2].map(|i| v[i] as f64 * x[0] + v[4 + i] as f64 * x[1] + v[8 + i] as f64 * x[2]) };
            let truth = vt(&view, rel);
            let tclip = proj.transform_vec4(vec4(truth[0] as f32, truth[1] as f32, truth[2] as f32, 1.0));
            let absolute = proj.transform_vec4(view.transform_vec4(vec4(p.x, p.y, p.z, 1.0)));
            let origin = render_origin(eye);
            assert!(origin.x != 0.0);
            let shifted = vec4(p.x - origin.x, p.y - origin.y, p.z - origin.z, 1.0);
            let relative = proj.transform_vec4(relative_view(&view, eye, origin).transform_vec4(shifted));
            worst_abs = worst_abs.max((px(absolute.x / absolute.w) - px(tclip.x / tclip.w)).abs());
            worst_rel = worst_rel.max((px(relative.x / relative.w) - px(tclip.x / tclip.w)).abs());
        }
        eprintln!("10 km, subject 2 m away: absolute f32 error {worst_abs:.3} px, camera-relative {worst_rel:.4} px");
        assert!(worst_rel < 0.02, "camera-relative {worst_rel}");
        assert!(worst_rel * 10.0 < worst_abs.max(0.01));
        assert_eq!(render_origin(vec3f(1500.0, 10.0, -1900.0)), Vec3f::default(), "near scenes are untouched");
    }
    use makepad_scene::Entity;

    fn cockpit_world(yaw: f32) -> World {
        let (s, c) = makepad_draw::makepad_math::deterministic::sincos(yaw * 0.5);
        let mut world = World::default();
        world.camera.fov = 58.0;
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
        }).unwrap();
        world
    }

    #[test]
    fn cockpit_eye_tracks_rigid_heading_and_uses_a_driver_height() {
        let yaw = 0.6;
        let world = cockpit_world(yaw);
        let rig = CameraRig {
            yaw: makepad_scene::heading_to_camera_yaw(yaw),
            pitch: -0.08,
            in_test: false,
            effects: CameraEffects::default(),
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
            effects: CameraEffects::default(),
        };
        let rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(640.0, 360.0),
        };
        assert!(vehicle_cockpit_scene_state(&world, rect, 0.0, &rig, 99).is_none());
        let scene = vehicle_cockpit_scene_state(&world, rect, 0.0, &rig, 7).unwrap();
        // Camera construction never mutates the shared entity or world view.
        assert_eq!(world.entity(7).unwrap().yaw, -1.1);
        assert_eq!(world.camera.fov, 58.0);
        assert_eq!(scene.viewport_rect, rect);
    }

    #[test]
    fn chase_camera_is_geometrically_behind_the_live_rigid_heading() {
        let yaw = -0.9;
        let mut world = cockpit_world(yaw);
        world.camera.third = 7;
        world.camera.height = 1.35;
        world.camera.boom = 7.8;
        world.camera.fov = 64.0;
        let rig = CameraRig {
            yaw: makepad_scene::heading_to_camera_yaw(yaw),
            pitch: -0.22,
            in_test: true,
            effects: CameraEffects::default(),
        };
        let rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(1280.0, 720.0),
        };
        let scene = scene_state(&world, rect, 0.0, &rig).unwrap();
        let pivot = world.entity(7).unwrap().pos + vec3f(0.0, world.camera.height, 0.0);
        let eye_to_car = pivot - scene.camera_pos;
        let horizontal = vec3f(eye_to_car.x, 0.0, eye_to_car.z).normalize();
        assert!(
            horizontal.dot(heading_to_forward(yaw)) > 0.999,
            "the chase eye must look forward through the rear of the car, not across its side",
        );
    }
}
