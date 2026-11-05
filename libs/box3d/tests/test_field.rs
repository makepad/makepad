use makepad_box3d::body::*;
use makepad_box3d::id::BodyId;
use makepad_box3d::field::*;
use makepad_box3d::math_functions::*;
use makepad_box3d::physics_world::*;
use makepad_box3d::shape::*;
use makepad_box3d::types::*;
use std::sync::Arc;

/// All below z = 0, within a box.
#[derive(Debug)]
struct Below;

impl DistanceField for Below {
    fn distance(&self, p: Vec3) -> (f32, Vec3) {
        (p.z, vec3(0.0, 0.0, 1.0))
    }
    fn bounds(&self) -> AABB {
        AABB { lower_bound: vec3(-10.0, -10.0, -2.0), upper_bound: vec3(10.0, 10.0, 0.0) }
    }
}

fn ball(world: &mut World, z: f32) -> BodyId {
    let mut def = default_body_def();
    def.body_type = BodyType::Dynamic;
    def.position = pos(0.0, 0.0, z);
    let body = create_body(world, &def);
    create_sphere_shape(world, body, &default_shape_def(), &Sphere { center: Vec3::ZERO, radius: 0.5 });
    body
}

fn field() -> Arc<dyn DistanceField> {
    Arc::new(Below)
}

/// A ball lands on a field and rests on it at its radius.
#[test]
fn a_ball_rests_on_a_static_field() {
    let mut def = default_world_def();
    def.gravity = vec3(0.0, 0.0, -10.0);
    let mut world = create_world(&def);
    let ground = create_body(&mut world, &default_body_def());
    create_field_shape(&mut world, ground, &default_shape_def(), &field());
    let body = ball(&mut world, 2.0);
    for _ in 0..240 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }
    let z = sub_pos(body_get_position(&world, body), POS_ZERO).z;
    assert!((z - 0.5).abs() < 0.02, "at z = {z}");
}

/// A field on a moving body takes the blow: momentum passes from the ball to it.
#[test]
fn a_ball_pushes_a_field_on_a_moving_body() {
    let mut world = create_world(&default_world_def_no_gravity());
    let mut def = default_body_def();
    def.body_type = BodyType::Dynamic;
    let held = create_body(&mut world, &def);
    let mut shape = default_shape_def();
    shape.update_body_mass = false;
    create_field_shape(&mut world, held, &shape, &field());
    body_set_mass_data(&mut world, held, MassData { mass: 1.0, center: Vec3::ZERO, inertia: Matrix3 { cx: vec3(1.0, 0.0, 0.0), cy: vec3(0.0, 1.0, 0.0), cz: vec3(0.0, 0.0, 1.0) } });
    let body = ball(&mut world, 2.0);
    body_set_linear_velocity(&mut world, body, vec3(0.0, 0.0, -5.0));
    for _ in 0..120 {
        world_step(&mut world, 1.0 / 60.0, 4);
    }
    assert!(body_get_linear_velocity(&world, held).z < -0.1, "the field's body was not pushed");
    assert!(sub_pos(body_get_position(&world, body), POS_ZERO).z > sub_pos(body_get_position(&world, held), POS_ZERO).z, "the ball went through");
}

/// A thin wall at z = 0, in a box much bigger than it, so a ball is always near enough to ask.
#[derive(Debug)]
struct Wall;

impl DistanceField for Wall {
    fn distance(&self, p: Vec3) -> (f32, Vec3) {
        (p.z.abs() - 0.1, vec3(0.0, 0.0, p.z.signum()))
    }
    fn bounds(&self) -> AABB {
        AABB { lower_bound: vec3(-10.0, -10.0, -10.0), upper_bound: vec3(10.0, 10.0, 10.0) }
    }
}

/// A wall coming many times its own thickness a step takes a ball along instead of passing it.
#[test]
fn a_fast_wall_does_not_pass_a_ball() {
    let mut world = create_world(&default_world_def_no_gravity());
    let mut def = default_body_def();
    def.body_type = BodyType::Kinematic;
    def.position = pos(0.0, 0.0, 3.3);
    let wall = create_body(&mut world, &def);
    let field: Arc<dyn DistanceField> = Arc::new(Wall);
    create_field_shape(&mut world, wall, &default_shape_def(), &field);
    body_set_linear_velocity(&mut world, wall, vec3(0.0, 0.0, -60.0));
    let mut small = default_body_def();
    small.body_type = BodyType::Dynamic;
    let body = create_body(&mut world, &small);
    create_sphere_shape(&mut world, body, &default_shape_def(), &Sphere { center: Vec3::ZERO, radius: 0.05 });
    for step in 0..30 {
        world_step(&mut world, 1.0 / 60.0, 4);
        let (z, wall_z) = (sub_pos(body_get_position(&world, body), POS_ZERO).z, sub_pos(body_get_position(&world, wall), POS_ZERO).z);
        assert!(z < wall_z, "the wall passed the ball at step {step}: {z} against {wall_z}");
    }
}

fn default_world_def_no_gravity() -> WorldDef {
    let mut def = default_world_def();
    def.gravity = Vec3::ZERO;
    def
}

/// The queries the shape answers.
#[test]
fn a_field_answers_queries() {
    let mut world = create_world(&default_world_def());
    let ground = create_body(&mut world, &default_body_def());
    let shape = create_field_shape(&mut world, ground, &default_shape_def(), &field());
    let hit = shape_ray_cast(&world, shape, pos(1.0, 1.0, 4.0), vec3(0.0, 0.0, -8.0));
    assert!(hit.hit && (hit.fraction - 0.5).abs() < 0.01, "{hit:?}");
    assert!(!shape_ray_cast(&world, shape, pos(1.0, 1.0, 4.0), vec3(0.0, 0.0, 2.0)).hit);
    let aabb = shape_get_aabb(&world, shape);
    assert!(aabb.upper_bound.x >= 10.0 && aabb.lower_bound.z <= -2.0, "{aabb:?}");
}
