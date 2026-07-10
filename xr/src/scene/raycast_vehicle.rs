//! A vehicle controller based on ray-casting: a port of Bullet's
//! `btRaycastVehicle` (via the previous physics engine's
//! `DynamicRayCastVehicleController`), rewritten on top of makepad-box3d.
//!
//! Also hosts the small makepad <-> box3d math conversion helpers shared by
//! the physics scene.

use crate::prelude::*;
use makepad_box3d::body as b3body;
use makepad_box3d::id::ShapeId;
use makepad_box3d::math_functions as b3m;
use makepad_box3d::physics_world as b3world;
use makepad_box3d::physics_world::World;
use makepad_box3d::shape as b3shape;
use makepad_box3d::types as b3t;

// ---------------------------------------------------------------------------
// makepad <-> box3d conversions
// ---------------------------------------------------------------------------

#[inline]
pub(crate) fn b3_vec3(v: Vec3f) -> b3m::Vec3 {
    b3m::vec3(v.x, v.y, v.z)
}

#[inline]
pub(crate) fn b3_pos(v: Vec3f) -> b3m::Pos {
    b3m::pos(v.x, v.y, v.z)
}

#[inline]
pub(crate) fn b3_quat(q: Quat) -> b3m::Quat {
    b3m::Quat {
        v: b3m::vec3(q.x, q.y, q.z),
        s: q.w,
    }
}

#[inline]
pub(crate) fn b3_transform(pose: Pose) -> b3m::WorldTransform {
    b3m::WorldTransform {
        p: b3_pos(pose.position),
        q: b3_quat(pose.orientation),
    }
}

#[inline]
pub(crate) fn from_b3_vec3(v: b3m::Vec3) -> Vec3f {
    vec3f(v.x, v.y, v.z)
}

#[inline]
#[allow(clippy::unnecessary_cast)]
pub(crate) fn from_b3_pos(p: b3m::Pos) -> Vec3f {
    vec3f(p.x as f32, p.y as f32, p.z as f32)
}

#[inline]
pub(crate) fn from_b3_quat(q: b3m::Quat) -> Quat {
    Quat {
        x: q.v.x,
        y: q.v.y,
        z: q.v.z,
        w: q.s,
    }
}

#[inline]
pub(crate) fn from_b3_transform(t: b3m::WorldTransform) -> Pose {
    Pose::new(from_b3_quat(t.q), from_b3_pos(t.p))
}

#[inline]
fn mat3_mul_vec3(m: &b3m::Matrix3, v: Vec3f) -> Vec3f {
    from_b3_vec3(m.cx) * v.x + from_b3_vec3(m.cy) * v.y + from_b3_vec3(m.cz) * v.z
}

#[inline]
fn normalize_or_zero(v: Vec3f) -> Vec3f {
    let length = v.length();
    if length > 1.0e-6 {
        v * (1.0 / length)
    } else {
        vec3f(0.0, 0.0, 0.0)
    }
}

#[inline]
fn inv_or_zero(x: f32) -> f32 {
    if x.abs() > 1.0e-12 {
        1.0 / x
    } else {
        0.0
    }
}

#[inline]
fn axis_vector(index: usize) -> Vec3f {
    match index {
        0 => vec3f(1.0, 0.0, 0.0),
        1 => vec3f(0.0, 1.0, 0.0),
        _ => vec3f(0.0, 0.0, 1.0),
    }
}

/// Rotation from an axis-angle vector whose magnitude is the rotation angle.
#[inline]
fn rotation_from_scaled_axis(v: Vec3f) -> Quat {
    let angle = v.length();
    if angle <= 1.0e-6 {
        Quat::default()
    } else {
        Quat::from_axis_angle(v * (1.0 / angle), angle)
    }
}

#[inline]
fn body_velocity_at_point(world: &World, body: makepad_box3d::id::BodyId, point: Vec3f) -> Vec3f {
    from_b3_vec3(b3body::body_get_world_point_velocity(
        world,
        body,
        b3_pos(point),
    ))
}

// ---------------------------------------------------------------------------
// Vehicle controller
// ---------------------------------------------------------------------------

pub(crate) type BodyHandle = makepad_box3d::id::BodyId;

/// A vehicle controller that simulates wheels with ray-casts.
pub(crate) struct DynamicRayCastVehicleController {
    wheels: Vec<Wheel>,
    forward_ws: Vec<Vec3f>,
    axle: Vec<Vec3f>,
    /// The current forward speed of the vehicle.
    pub(crate) current_vehicle_speed: f32,
    /// Handle of the vehicle's chassis.
    pub(crate) chassis: BodyHandle,
    /// The chassis' local _up_ direction (`0 = x, 1 = y, 2 = z`).
    pub(crate) index_up_axis: usize,
    /// The chassis' local _forward_ direction (`0 = x, 1 = y, 2 = z`).
    pub(crate) index_forward_axis: usize,
}

/// Parameters affecting the physical behavior of a wheel.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct WheelTuning {
    pub suspension_stiffness: f32,
    pub suspension_compression: f32,
    pub suspension_damping: f32,
    pub max_suspension_travel: f32,
    pub side_friction_stiffness: f32,
    pub friction_slip: f32,
    pub max_suspension_force: f32,
}

impl Default for WheelTuning {
    fn default() -> Self {
        Self {
            suspension_stiffness: 5.88,
            suspension_compression: 0.83,
            suspension_damping: 0.88,
            max_suspension_travel: 5.0,
            side_friction_stiffness: 1.0,
            friction_slip: 10.5,
            max_suspension_force: 6000.0,
        }
    }
}

/// A wheel attached to a vehicle.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Wheel {
    raycast_info: RayCastInfo,

    center: Vec3f,
    wheel_direction_ws: Vec3f,
    wheel_axle_ws: Vec3f,

    /// The position of the wheel, relative to the chassis.
    pub chassis_connection_point_cs: Vec3f,
    /// The direction of the wheel's suspension, relative to the chassis.
    pub direction_cs: Vec3f,
    /// The wheel's axle axis, relative to the chassis.
    pub axle_cs: Vec3f,
    /// The rest length of the wheel's suspension spring.
    pub suspension_rest_length: f32,
    /// The maximum distance the suspension can travel before and after its resting length.
    pub max_suspension_travel: f32,
    /// The wheel's radius.
    pub radius: f32,
    /// The suspension stiffness.
    pub suspension_stiffness: f32,
    /// The suspension's damping when it is being compressed.
    pub damping_compression: f32,
    /// The suspension's damping when it is being released.
    pub damping_relaxation: f32,
    /// Parameter controlling how much traction the tire has.
    pub friction_slip: f32,
    /// The multiplier of friction between a tire and the collider it's on top of.
    pub side_friction_stiffness: f32,
    /// The wheel's current rotation on its axle.
    pub rotation: f32,
    delta_rotation: f32,
    roll_influence: f32,
    /// The maximum force applied by the suspension.
    pub max_suspension_force: f32,

    /// The forward impulses applied by the wheel on the chassis.
    pub forward_impulse: f32,
    /// The side impulses applied by the wheel on the chassis.
    pub side_impulse: f32,

    /// The steering angle for this wheel.
    pub steering: f32,
    /// The forward force applied by this wheel on the chassis.
    pub engine_force: f32,
    /// The maximum amount of braking impulse applied to slow down the vehicle.
    pub brake: f32,

    clipped_inv_contact_dot_suspension: f32,
    suspension_relative_velocity: f32,
    /// The force applied by the suspension.
    pub wheel_suspension_force: f32,
    skid_info: f32,
}

impl Wheel {
    /// Information about suspension and the ground obtained from the ray-casting.
    pub fn raycast_info(&self) -> &RayCastInfo {
        &self.raycast_info
    }

    /// The world-space center of the wheel.
    pub fn center(&self) -> Vec3f {
        self.center
    }

    /// The world-space direction of the wheel's axle.
    #[allow(dead_code)]
    pub fn axle(&self) -> Vec3f {
        self.wheel_axle_ws
    }
}

/// Information about suspension and the ground obtained from the ray-casting
/// to simulate a wheel's suspension.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct RayCastInfo {
    /// The (world-space) contact normal between the wheel and the floor.
    pub contact_normal_ws: Vec3f,
    /// The (world-space) point hit by the wheel's ray-cast.
    pub contact_point_ws: Vec3f,
    /// The suspension length for the wheel.
    pub suspension_length: f32,
    /// The (world-space) starting point of the ray-cast.
    pub hard_point_ws: Vec3f,
    /// Is the wheel in contact with the ground?
    pub is_in_contact: bool,
    /// The shape hit by the ray-cast.
    pub ground_object: Option<ShapeId>,
}

impl DynamicRayCastVehicleController {
    /// Creates a new vehicle represented by the given rigid-body.
    pub(crate) fn new(chassis: BodyHandle) -> Self {
        Self {
            wheels: vec![],
            forward_ws: vec![],
            axle: vec![],
            current_vehicle_speed: 0.0,
            chassis,
            index_up_axis: 1,
            index_forward_axis: 0,
        }
    }

    /// Adds a wheel to this vehicle.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn add_wheel(
        &mut self,
        chassis_connection_cs: Vec3f,
        direction_cs: Vec3f,
        axle_cs: Vec3f,
        suspension_rest_length: f32,
        radius: f32,
        tuning: &WheelTuning,
    ) -> &mut Wheel {
        let wheel_id = self.wheels.len();
        self.wheels.push(Wheel {
            raycast_info: RayCastInfo::default(),
            suspension_rest_length,
            max_suspension_travel: tuning.max_suspension_travel,
            radius,
            suspension_stiffness: tuning.suspension_stiffness,
            damping_compression: tuning.suspension_compression,
            damping_relaxation: tuning.suspension_damping,
            chassis_connection_point_cs: chassis_connection_cs,
            direction_cs,
            axle_cs,
            wheel_direction_ws: direction_cs,
            wheel_axle_ws: axle_cs,
            center: vec3f(0.0, 0.0, 0.0),
            friction_slip: tuning.friction_slip,
            steering: 0.0,
            engine_force: 0.0,
            rotation: 0.0,
            delta_rotation: 0.0,
            brake: 0.0,
            roll_influence: 0.1,
            clipped_inv_contact_dot_suspension: 0.0,
            suspension_relative_velocity: 0.0,
            wheel_suspension_force: 0.0,
            max_suspension_force: tuning.max_suspension_force,
            skid_info: 0.0,
            side_impulse: 0.0,
            forward_impulse: 0.0,
            side_friction_stiffness: tuning.side_friction_stiffness,
        });
        &mut self.wheels[wheel_id]
    }

    /// Reference to all the wheels attached to this vehicle.
    pub(crate) fn wheels(&self) -> &[Wheel] {
        &self.wheels
    }

    /// Mutable reference to all the wheels attached to this vehicle.
    pub(crate) fn wheels_mut(&mut self) -> &mut [Wheel] {
        &mut self.wheels
    }

    fn update_wheel_transform(&mut self, chassis_pose: &Pose, wheel_index: usize) {
        let wheel = &mut self.wheels[wheel_index];
        wheel.raycast_info.is_in_contact = false;
        wheel.raycast_info.hard_point_ws =
            chassis_pose.transform_vec3(&wheel.chassis_connection_point_cs);
        wheel.wheel_direction_ws = chassis_pose.orientation.rotate_vec3(&wheel.direction_cs);
        wheel.wheel_axle_ws = chassis_pose.orientation.rotate_vec3(&wheel.axle_cs);

        let steering_orn =
            rotation_from_scaled_axis(wheel.wheel_direction_ws * (-1.0) * wheel.steering);
        wheel.wheel_axle_ws =
            steering_orn.rotate_vec3(&chassis_pose.orientation.rotate_vec3(&wheel.axle_cs));
        wheel.center = wheel.raycast_info.hard_point_ws
            + wheel.wheel_direction_ws * wheel.raycast_info.suspension_length;
    }

    fn ray_cast(
        &mut self,
        world: &World,
        chassis_pose: &Pose,
        query_filter: b3t::QueryFilter,
        wheel_id: usize,
        wheel_query_filter: &dyn Fn(usize, ShapeId, &World) -> bool,
    ) {
        let _ = chassis_pose;
        let wheel = &mut self.wheels[wheel_id];
        let raylen = wheel.suspension_rest_length + wheel.radius;
        let rayvector = wheel.wheel_direction_ws * raylen;
        let source = wheel.raycast_info.hard_point_ws;
        wheel.raycast_info.contact_point_ws = source + rayvector;
        wheel.raycast_info.ground_object = None;

        let mut best: Option<(ShapeId, Vec3f, Vec3f, f32)> = None;
        {
            let best = &mut best;
            let mut callback = |shape_id: ShapeId,
                                point: b3m::Pos,
                                normal: b3m::Vec3,
                                fraction: f32,
                                _user_material_id: u64,
                                _triangle_index: i32,
                                _child_index: i32|
             -> f32 {
                if !wheel_query_filter(wheel_id, shape_id, world) {
                    // Skip this shape, keep the current clip fraction.
                    return -1.0;
                }
                *best = Some((shape_id, from_b3_pos(point), from_b3_vec3(normal), fraction));
                fraction
            };
            b3world::world_cast_ray(
                world,
                b3_pos(source),
                b3_vec3(rayvector),
                query_filter,
                &mut callback,
            );
        }

        if let Some((shape_hit, hit_point, mut hit_normal, toi)) = best {
            if toi == 0.0 || hit_normal.length() < 1.0e-6 {
                // Note: box3d ray casts generally miss when starting inside a
                // shape, so this solid-hit recovery path rarely triggers; fall
                // back to the suspension direction.
                hit_normal = wheel.wheel_direction_ws * -1.0;
            }

            wheel.raycast_info.contact_normal_ws = hit_normal;
            wheel.raycast_info.is_in_contact = true;
            wheel.raycast_info.ground_object = Some(shape_hit);

            let hit_distance = toi * raylen;
            wheel.raycast_info.suspension_length = hit_distance - wheel.radius;

            // clamp on max suspension travel
            let min_suspension_length = wheel.suspension_rest_length - wheel.max_suspension_travel;
            let max_suspension_length = wheel.suspension_rest_length + wheel.max_suspension_travel;
            wheel.raycast_info.suspension_length = wheel
                .raycast_info
                .suspension_length
                .clamp(min_suspension_length, max_suspension_length);
            wheel.raycast_info.contact_point_ws = hit_point;

            let denominator = wheel
                .raycast_info
                .contact_normal_ws
                .dot(wheel.wheel_direction_ws);
            let chassis_velocity_at_contact_point =
                body_velocity_at_point(world, self.chassis, wheel.raycast_info.contact_point_ws);
            let proj_vel = wheel
                .raycast_info
                .contact_normal_ws
                .dot(chassis_velocity_at_contact_point);

            if denominator >= -0.1 {
                wheel.suspension_relative_velocity = 0.0;
                wheel.clipped_inv_contact_dot_suspension = 1.0 / 0.1;
            } else {
                let inv = -1.0 / denominator;
                wheel.suspension_relative_velocity = proj_vel * inv;
                wheel.clipped_inv_contact_dot_suspension = inv;
            }
        } else {
            // No contact, put wheel info as in rest position
            wheel.raycast_info.suspension_length = wheel.suspension_rest_length;
            wheel.suspension_relative_velocity = 0.0;
            wheel.raycast_info.contact_normal_ws = wheel.wheel_direction_ws * -1.0;
            wheel.clipped_inv_contact_dot_suspension = 1.0;
        }
    }

    /// Updates the vehicle's velocity based on its suspension, engine force, and brake,
    /// with an additional per-wheel shape filter applied to wheel ray casts.
    pub(crate) fn update_vehicle_with_filter(
        &mut self,
        world: &mut World,
        dt: f32,
        query_filter: b3t::QueryFilter,
        wheel_query_filter: &dyn Fn(usize, ShapeId, &World) -> bool,
    ) {
        if !b3world::body_is_valid(world, self.chassis) {
            return;
        }
        let num_wheels = self.wheels.len();
        let chassis_pose = from_b3_transform(b3body::body_get_transform(world, self.chassis));
        let chassis_linvel = from_b3_vec3(b3body::body_get_linear_velocity(world, self.chassis));

        for i in 0..num_wheels {
            self.update_wheel_transform(&chassis_pose, i);
        }

        self.current_vehicle_speed = chassis_linvel.length();

        let forward_w = chassis_pose
            .orientation
            .rotate_vec3(&axis_vector(self.index_forward_axis));
        if forward_w.dot(chassis_linvel) < 0.0 {
            self.current_vehicle_speed *= -1.0;
        }

        //
        // simulate suspension
        //
        for wheel_id in 0..num_wheels {
            self.ray_cast(
                world,
                &chassis_pose,
                query_filter,
                wheel_id,
                wheel_query_filter,
            );
        }

        let chassis_mass = b3body::body_get_mass(world, self.chassis);
        self.update_suspension(chassis_mass);

        for wheel in &mut self.wheels {
            if wheel.engine_force > 0.0 {
                b3body::body_set_awake(world, self.chassis, true);
            }

            // apply suspension force
            let suspension_force = wheel
                .wheel_suspension_force
                .min(wheel.max_suspension_force);
            let impulse = wheel.raycast_info.contact_normal_ws * suspension_force * dt;
            b3body::body_apply_linear_impulse(
                world,
                self.chassis,
                b3_vec3(impulse),
                b3_pos(wheel.raycast_info.contact_point_ws),
                false,
            );
        }

        self.update_friction(world, dt);

        for wheel in &mut self.wheels {
            let vel = body_velocity_at_point(world, self.chassis, wheel.raycast_info.hard_point_ws);

            if wheel.raycast_info.is_in_contact {
                let chassis_pose =
                    from_b3_transform(b3body::body_get_transform(world, self.chassis));
                let mut fwd = chassis_pose
                    .orientation
                    .rotate_vec3(&axis_vector(self.index_forward_axis));
                let proj = fwd.dot(wheel.raycast_info.contact_normal_ws);
                fwd -= wheel.raycast_info.contact_normal_ws * proj;

                let proj2 = fwd.dot(vel);

                wheel.delta_rotation = (proj2 * dt) / wheel.radius;
                wheel.rotation += wheel.delta_rotation;
            } else {
                wheel.rotation += wheel.delta_rotation;
            }

            wheel.delta_rotation *= 0.99; // damping of rotation when not in contact
        }
    }

    fn update_suspension(&mut self, chassis_mass: f32) {
        for wheel in &mut self.wheels {
            if wheel.raycast_info.is_in_contact {
                // Spring
                let rest_length = wheel.suspension_rest_length;
                let current_length = wheel.raycast_info.suspension_length;
                let length_diff = rest_length - current_length;
                let mut force = wheel.suspension_stiffness
                    * length_diff
                    * wheel.clipped_inv_contact_dot_suspension;

                // Damper
                let projected_rel_vel = wheel.suspension_relative_velocity;
                let susp_damping = if projected_rel_vel < 0.0 {
                    wheel.damping_compression
                } else {
                    wheel.damping_relaxation
                };
                force -= susp_damping * projected_rel_vel;

                wheel.wheel_suspension_force = (force * chassis_mass).max(0.0);
            } else {
                wheel.wheel_suspension_force = 0.0;
            }
        }
    }

    fn update_friction(&mut self, world: &mut World, dt: f32) {
        let num_wheels = self.wheels.len();
        if num_wheels == 0 {
            return;
        }

        self.forward_ws.resize(num_wheels, vec3f(0.0, 0.0, 0.0));
        self.axle.resize(num_wheels, vec3f(0.0, 0.0, 0.0));

        let mut num_wheels_on_ground = 0;

        for wheel in &mut self.wheels {
            if wheel.raycast_info.ground_object.is_some() {
                num_wheels_on_ground += 1;
            }
            wheel.side_impulse = 0.0;
            wheel.forward_impulse = 0.0;
        }

        for i in 0..num_wheels {
            let wheel = &mut self.wheels[i];
            let Some(ground_shape) = wheel.raycast_info.ground_object else {
                continue;
            };

            self.axle[i] = wheel.wheel_axle_ws;
            let surf_normal_ws = wheel.raycast_info.contact_normal_ws;
            let proj = self.axle[i].dot(surf_normal_ws);
            self.axle[i] -= surf_normal_ws * proj;
            self.axle[i] = normalize_or_zero(self.axle[i]);
            self.forward_ws[i] = normalize_or_zero(Vec3f::cross(surf_normal_ws, self.axle[i]));

            let ground_body = b3shape::shape_get_body(world, ground_shape);
            let ground_dynamic = b3world::body_is_valid(world, ground_body)
                && b3body::body_get_type(world, ground_body) == b3t::BodyType::Dynamic;

            wheel.side_impulse = if ground_dynamic {
                resolve_single_bilateral(
                    world,
                    self.chassis,
                    wheel.raycast_info.contact_point_ws,
                    ground_body,
                    wheel.raycast_info.contact_point_ws,
                    self.axle[i],
                )
            } else {
                resolve_single_unilateral(
                    world,
                    self.chassis,
                    wheel.raycast_info.contact_point_ws,
                    self.axle[i],
                )
            };
            wheel.side_impulse *= wheel.side_friction_stiffness;
        }

        let side_factor = 1.0;
        let fwd_factor = 0.5;
        let mut sliding = false;

        for wheel_id in 0..num_wheels {
            let ground_object = self.wheels[wheel_id].raycast_info.ground_object;
            let mut rolling_friction = 0.0;

            if let Some(ground_shape) = ground_object {
                let wheel = &self.wheels[wheel_id];
                if wheel.engine_force != 0.0 {
                    rolling_friction = wheel.engine_force * dt;
                } else {
                    let default_rolling_friction_impulse = 0.0;
                    let max_impulse = if wheel.brake != 0.0 {
                        wheel.brake
                    } else {
                        default_rolling_friction_impulse
                    };
                    let ground_body = b3shape::shape_get_body(world, ground_shape);
                    let ground_body = (b3world::body_is_valid(world, ground_body)
                        && b3body::body_get_type(world, ground_body) == b3t::BodyType::Dynamic)
                        .then_some(ground_body);
                    rolling_friction = calc_rolling_friction(
                        world,
                        self.chassis,
                        ground_body,
                        wheel.raycast_info.contact_point_ws,
                        self.forward_ws[wheel_id],
                        max_impulse,
                        num_wheels_on_ground,
                    );
                }
            }

            // switch between active rolling (throttle), braking and non-active
            // rolling friction (no throttle/brake)
            let wheel = &mut self.wheels[wheel_id];
            wheel.forward_impulse = 0.0;
            wheel.skid_info = 1.0;

            if ground_object.is_some() {
                let max_imp = wheel.wheel_suspension_force * dt * wheel.friction_slip;
                let max_imp_side = max_imp;
                let max_imp_squared = max_imp * max_imp_side;

                wheel.forward_impulse = rolling_friction;

                let x = wheel.forward_impulse * fwd_factor;
                let y = wheel.side_impulse * side_factor;

                let impulse_squared = x * x + y * y;

                if impulse_squared > max_imp_squared {
                    sliding = true;
                    let factor = max_imp * inv_or_zero(impulse_squared.sqrt());
                    wheel.skid_info *= factor;
                }
            }
        }

        if sliding {
            for wheel in &mut self.wheels {
                if wheel.side_impulse != 0.0 && wheel.skid_info < 1.0 {
                    wheel.forward_impulse *= wheel.skid_info;
                    wheel.side_impulse *= wheel.skid_info;
                }
            }
        }

        // apply the impulses
        let chassis_pose = from_b3_transform(b3body::body_get_transform(world, self.chassis));
        let chassis_com = from_b3_pos(b3body::body_get_world_center_of_mass(world, self.chassis));
        for wheel_id in 0..num_wheels {
            let wheel = &self.wheels[wheel_id];
            let mut impulse_point = wheel.raycast_info.contact_point_ws;

            if wheel.forward_impulse != 0.0 {
                b3body::body_apply_linear_impulse(
                    world,
                    self.chassis,
                    b3_vec3(self.forward_ws[wheel_id] * wheel.forward_impulse),
                    b3_pos(impulse_point),
                    false,
                );
            }
            if wheel.side_impulse != 0.0 {
                let side_impulse = self.axle[wheel_id] * wheel.side_impulse;

                let v_chassis_world_up = chassis_pose
                    .orientation
                    .rotate_vec3(&axis_vector(self.index_up_axis));
                impulse_point -= v_chassis_world_up
                    * (v_chassis_world_up.dot(impulse_point - chassis_com)
                        * (1.0 - wheel.roll_influence));

                b3body::body_apply_linear_impulse(
                    world,
                    self.chassis,
                    b3_vec3(side_impulse),
                    b3_pos(impulse_point),
                    false,
                );
            }
        }
    }
}

fn impulse_denominator(world: &World, body: BodyHandle, pos: Vec3f, n: Vec3f) -> f32 {
    let com = from_b3_pos(b3body::body_get_world_center_of_mass(world, body));
    let dpt = pos - com;
    let gcross = Vec3f::cross(dpt, n);
    let inv_inertia = b3body::body_get_world_inverse_rotational_inertia(world, body);
    let v = Vec3f::cross(mat3_mul_vec3(&inv_inertia, gcross), dpt);
    b3body::body_get_inverse_mass(world, body) + n.dot(v)
}

fn calc_rolling_friction(
    world: &World,
    body0: BodyHandle,
    body1: Option<BodyHandle>,
    contact_pos_world: Vec3f,
    friction_direction_world: Vec3f,
    max_impulse: f32,
    num_wheels_on_ground: usize,
) -> f32 {
    let denom0 = impulse_denominator(world, body0, contact_pos_world, friction_direction_world);
    let denom1 = body1
        .map(|body1| impulse_denominator(world, body1, contact_pos_world, friction_direction_world))
        .unwrap_or(0.0);
    let relaxation = 1.0;
    let jac_diag_ab_inv = relaxation / (denom0 + denom1);

    let vel1 = body_velocity_at_point(world, body0, contact_pos_world);
    let vel2 = body1
        .map(|b| body_velocity_at_point(world, b, contact_pos_world))
        .unwrap_or(vec3f(0.0, 0.0, 0.0));
    let vel = vel1 - vel2;
    let vrel = friction_direction_world.dot(vel);

    // calculate friction that moves us to zero relative velocity
    (-vrel * jac_diag_ab_inv / (num_wheels_on_ground.max(1) as f32))
        .clamp(-max_impulse, max_impulse)
}

fn resolve_single_bilateral(
    world: &World,
    body1: BodyHandle,
    pt1: Vec3f,
    body2: BodyHandle,
    pt2: Vec3f,
    normal: Vec3f,
) -> f32 {
    let vel1 = body_velocity_at_point(world, body1, pt1);
    let vel2 = body_velocity_at_point(world, body2, pt2);
    let dvel = vel1 - vel2;

    let com1 = from_b3_pos(b3body::body_get_world_center_of_mass(world, body1));
    let com2 = from_b3_pos(b3body::body_get_world_center_of_mass(world, body2));
    let dpt1 = pt1 - com1;
    let dpt2 = pt2 - com2;
    let aj = Vec3f::cross(dpt1, normal);
    let bj = Vec3f::cross(dpt2, normal * -1.0);
    let iaj = mat3_mul_vec3(
        &b3body::body_get_world_inverse_rotational_inertia(world, body1),
        aj,
    );
    let ibj = mat3_mul_vec3(
        &b3body::body_get_world_inverse_rotational_inertia(world, body2),
        bj,
    );

    let im1 = b3body::body_get_inverse_mass(world, body1);
    let im2 = b3body::body_get_inverse_mass(world, body2);

    let jac_diag_ab = im1 + im2 + iaj.dot(iaj) + ibj.dot(ibj);
    let jac_diag_ab_inv = inv_or_zero(jac_diag_ab);
    let rel_vel = normal.dot(dvel);

    let contact_damping = 0.2;
    -contact_damping * rel_vel * jac_diag_ab_inv
}

fn resolve_single_unilateral(world: &World, body1: BodyHandle, pt1: Vec3f, normal: Vec3f) -> f32 {
    let vel1 = body_velocity_at_point(world, body1, pt1);
    let dvel = vel1;
    let com1 = from_b3_pos(b3body::body_get_world_center_of_mass(world, body1));
    let dpt1 = pt1 - com1;
    let aj = Vec3f::cross(dpt1, normal);
    let iaj = mat3_mul_vec3(
        &b3body::body_get_world_inverse_rotational_inertia(world, body1),
        aj,
    );

    let im1 = b3body::body_get_inverse_mass(world, body1);
    let jac_diag_ab = im1 + iaj.dot(iaj);
    let jac_diag_ab_inv = inv_or_zero(jac_diag_ab);
    let rel_vel = normal.dot(dvel);

    let contact_damping = 0.2;
    -contact_damping * rel_vel * jac_diag_ab_inv
}
