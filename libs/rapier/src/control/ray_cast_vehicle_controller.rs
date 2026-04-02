//! A vehicle controller based on ray-casting, ported and modified from Bullet’s `btRaycastVehicle`.

use crate::dynamics::{RigidBody, RigidBodyHandle, RigidBodySet};
use crate::geometry::{ColliderHandle, ColliderSet, Ray};
use crate::math::{Real, Vector, VectorExt, rotation_from_angle};
use crate::pipeline::QueryFilter;
use crate::prelude::QueryPipelineMut;
use crate::utils::{CrossProduct, DotProduct};

/// A character controller to simulate vehicles using ray-casting for the wheels.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Debug)]
pub struct DynamicRayCastVehicleController {
    wheels: Vec<Wheel>,
    forward_ws: Vec<Vector>,
    axle: Vec<Vector>,
    /// The current forward speed of the vehicle.
    pub current_vehicle_speed: Real,

    /// Handle of the vehicle’s chassis.
    pub chassis: RigidBodyHandle,
    /// The chassis’ local _up_ direction (`0 = x, 1 = y, 2 = z`)
    pub index_up_axis: usize,
    /// The chassis’ local _forward_ direction (`0 = x, 1 = y, 2 = z`)
    pub index_forward_axis: usize,
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq)]
/// Parameters affecting the physical behavior of a wheel.
pub struct WheelTuning {
    /// The suspension stiffness.
    ///
    /// Increase this value if the suspension appears to not push the vehicle strong enough.
    pub suspension_stiffness: Real,
    /// The suspension’s damping when it is being compressed.
    pub suspension_compression: Real,
    /// The suspension’s damping when it is being released.
    ///
    /// Increase this value if the suspension appears to overshoot.
    pub suspension_damping: Real,
    /// The maximum distance the suspension can travel before and after its resting length.
    pub max_suspension_travel: Real,
    /// The multiplier of friction between a tire and the collider it's on top of.
    pub side_friction_stiffness: Real,
    /// Parameter controlling how much traction the tire has.
    ///
    /// The larger the value, the more instantaneous braking will happen (with the risk of
    /// causing the vehicle to flip if it’s too strong).
    pub friction_slip: Real,
    /// The maximum force applied by the suspension.
    pub max_suspension_force: Real,
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

/// Objects used to initialize a wheel.
struct WheelDesc {
    /// The position of the wheel, relative to the chassis.
    pub chassis_connection_cs: Vector,
    /// The direction of the wheel’s suspension, relative to the chassis.
    ///
    /// The ray-casting will happen following this direction to detect the ground.
    pub direction_cs: Vector,
    /// The wheel’s axle axis, relative to the chassis.
    pub axle_cs: Vector,
    /// The rest length of the wheel’s suspension spring.
    pub suspension_rest_length: Real,
    /// The maximum distance the suspension can travel before and after its resting length.
    pub max_suspension_travel: Real,
    /// The wheel’s radius.
    pub radius: Real,

    /// The suspension stiffness.
    ///
    /// Increase this value if the suspension appears to not push the vehicle strong enough.
    pub suspension_stiffness: Real,
    /// The suspension’s damping when it is being compressed.
    pub damping_compression: Real,
    /// The suspension’s damping when it is being released.
    ///
    /// Increase this value if the suspension appears to overshoot.
    pub damping_relaxation: Real,
    /// Parameter controlling how much traction the tire has.
    ///
    /// The larger the value, the more instantaneous braking will happen (with the risk of
    /// causing the vehicle to flip if it’s too strong).
    pub friction_slip: Real,
    /// The maximum force applied by the suspension.
    pub max_suspension_force: Real,
    /// The multiplier of friction between a tire and the collider it's on top of.
    pub side_friction_stiffness: Real,
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq)]
/// A wheel attached to a vehicle.
pub struct Wheel {
    raycast_info: RayCastInfo,

    center: Vector,
    wheel_direction_ws: Vector,
    wheel_axle_ws: Vector,

    /// The position of the wheel, relative to the chassis.
    pub chassis_connection_point_cs: Vector,
    /// The direction of the wheel’s suspension, relative to the chassis.
    ///
    /// The ray-casting will happen following this direction to detect the ground.
    pub direction_cs: Vector,
    /// The wheel’s axle axis, relative to the chassis.
    pub axle_cs: Vector,
    /// The rest length of the wheel’s suspension spring.
    pub suspension_rest_length: Real,
    /// The maximum distance the suspension can travel before and after its resting length.
    pub max_suspension_travel: Real,
    /// The wheel’s radius.
    pub radius: Real,
    /// The suspension stiffness.
    ///
    /// Increase this value if the suspension appears to not push the vehicle strong enough.
    pub suspension_stiffness: Real,
    /// The suspension’s damping when it is being compressed.
    pub damping_compression: Real,
    /// The suspension’s damping when it is being released.
    ///
    /// Increase this value if the suspension appears to overshoot.
    pub damping_relaxation: Real,
    /// Parameter controlling how much traction the tire has.
    ///
    /// The larger the value, the more instantaneous braking will happen (with the risk of
    /// causing the vehicle to flip if it’s too strong).
    pub friction_slip: Real,
    /// The multiplier of friction between a tire and the collider it's on top of.
    pub side_friction_stiffness: Real,
    /// The wheel’s current rotation on its axle.
    pub rotation: Real,
    delta_rotation: Real,
    roll_influence: Real, // TODO: make this public?
    /// The maximum force applied by the suspension.
    pub max_suspension_force: Real,

    /// The forward impulses applied by the wheel on the chassis.
    pub forward_impulse: Real,
    /// The side impulses applied by the wheel on the chassis.
    pub side_impulse: Real,

    /// The steering angle for this wheel.
    pub steering: Real,
    /// The forward force applied by this wheel on the chassis.
    pub engine_force: Real,
    /// The maximum amount of braking impulse applied to slow down the vehicle.
    pub brake: Real,

    clipped_inv_contact_dot_suspension: Real,
    suspension_relative_velocity: Real,
    /// The force applied by the suspension.
    pub wheel_suspension_force: Real,
    skid_info: Real,
}

impl Wheel {
    fn new(info: WheelDesc) -> Self {
        Self {
            raycast_info: RayCastInfo::default(),
            suspension_rest_length: info.suspension_rest_length,
            max_suspension_travel: info.max_suspension_travel,
            radius: info.radius,
            suspension_stiffness: info.suspension_stiffness,
            damping_compression: info.damping_compression,
            damping_relaxation: info.damping_relaxation,
            chassis_connection_point_cs: info.chassis_connection_cs,
            direction_cs: info.direction_cs,
            axle_cs: info.axle_cs,
            wheel_direction_ws: info.direction_cs,
            wheel_axle_ws: info.axle_cs,
            center: Vector::ZERO,
            friction_slip: info.friction_slip,
            steering: 0.0,
            engine_force: 0.0,
            rotation: 0.0,
            delta_rotation: 0.0,
            brake: 0.0,
            roll_influence: 0.1,
            clipped_inv_contact_dot_suspension: 0.0,
            suspension_relative_velocity: 0.0,
            wheel_suspension_force: 0.0,
            max_suspension_force: info.max_suspension_force,
            skid_info: 0.0,
            side_impulse: 0.0,
            forward_impulse: 0.0,
            side_friction_stiffness: info.side_friction_stiffness,
        }
    }

    /// Information about suspension and the ground obtained from the ray-casting
    /// for this wheel.
    pub fn raycast_info(&self) -> &RayCastInfo {
        &self.raycast_info
    }

    /// The world-space center of the wheel.
    pub fn center(&self) -> Vector {
        self.center
    }

    /// The world-space direction of the wheel’s suspension.
    pub fn suspension(&self) -> Vector {
        self.wheel_direction_ws
    }

    /// The world-space direction of the wheel’s axle.
    pub fn axle(&self) -> Vector {
        self.wheel_axle_ws
    }
}

/// Information about suspension and the ground obtained from the ray-casting
/// to simulate a wheel’s suspension.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq, Default)]
pub struct RayCastInfo {
    /// The (world-space) contact normal between the wheel and the floor.
    pub contact_normal_ws: Vector,
    /// The (world-space) point hit by the wheel’s ray-cast.
    pub contact_point_ws: Vector,
    /// The suspension length for the wheel.
    pub suspension_length: Real,
    /// The (world-space) starting point of the ray-cast.
    pub hard_point_ws: Vector,
    /// Is the wheel in contact with the ground?
    pub is_in_contact: bool,
    /// The collider hit by the ray-cast.
    pub ground_object: Option<ColliderHandle>,
}

impl DynamicRayCastVehicleController {
    /// Creates a new vehicle represented by the given rigid-body.
    ///
    /// Wheels have to be attached afterwards calling [`Self::add_wheel`].
    pub fn new(chassis: RigidBodyHandle) -> Self {
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

    //
    // basically most of the code is general for 2 or 4 wheel vehicles, but some of it needs to be reviewed
    //
    /// Adds a wheel to this vehicle.
    pub fn add_wheel(
        &mut self,
        chassis_connection_cs: Vector,
        direction_cs: Vector,
        axle_cs: Vector,
        suspension_rest_length: Real,
        radius: Real,
        tuning: &WheelTuning,
    ) -> &mut Wheel {
        let ci = WheelDesc {
            chassis_connection_cs,
            direction_cs,
            axle_cs,
            suspension_rest_length,
            radius,
            suspension_stiffness: tuning.suspension_stiffness,
            damping_compression: tuning.suspension_compression,
            damping_relaxation: tuning.suspension_damping,
            friction_slip: tuning.friction_slip,
            max_suspension_travel: tuning.max_suspension_travel,
            max_suspension_force: tuning.max_suspension_force,
            side_friction_stiffness: tuning.side_friction_stiffness,
        };

        let wheel_id = self.wheels.len();
        self.wheels.push(Wheel::new(ci));

        &mut self.wheels[wheel_id]
    }

    #[cfg(feature = "dim2")]
    fn update_wheel_transform(&mut self, chassis: &RigidBody, wheel_index: usize) {
        self.update_wheel_transforms_ws(chassis, wheel_index);
        let wheel = &mut self.wheels[wheel_index];
        wheel.center = wheel.raycast_info.hard_point_ws
            + wheel.wheel_direction_ws * wheel.raycast_info.suspension_length;
    }

    #[cfg(feature = "dim3")]
    fn update_wheel_transform(&mut self, chassis: &RigidBody, wheel_index: usize) {
        self.update_wheel_transforms_ws(chassis, wheel_index);
        let wheel = &mut self.wheels[wheel_index];

        let steering_orn = rotation_from_angle(-wheel.wheel_direction_ws * wheel.steering);
        wheel.wheel_axle_ws = steering_orn * (chassis.position().rotation * wheel.axle_cs);
        wheel.center = wheel.raycast_info.hard_point_ws
            + wheel.wheel_direction_ws * wheel.raycast_info.suspension_length;
    }

    fn update_wheel_transforms_ws(&mut self, chassis: &RigidBody, wheel_id: usize) {
        let wheel = &mut self.wheels[wheel_id];
        wheel.raycast_info.is_in_contact = false;

        let chassis_transform = chassis.position();

        wheel.raycast_info.hard_point_ws = chassis_transform * wheel.chassis_connection_point_cs;
        wheel.wheel_direction_ws = chassis_transform.rotation * wheel.direction_cs;
        wheel.wheel_axle_ws = chassis_transform.rotation * wheel.axle_cs;
    }
    fn ray_cast<F>(
        &mut self,
        queries: &QueryPipelineMut,
        chassis: &RigidBody,
        wheel_id: usize,
        wheel_query_filter: &F,
    ) where
        F: Fn(usize, ColliderHandle, &crate::geometry::Collider) -> bool,
    {
        let wheel = &mut self.wheels[wheel_id];
        let raylen = wheel.suspension_rest_length + wheel.radius;
        let rayvector = wheel.wheel_direction_ws * raylen;
        let source = wheel.raycast_info.hard_point_ws;
        wheel.raycast_info.contact_point_ws = source + rayvector;
        let ray = Ray::new(source, rayvector);
        let base_filter = queries.filter;
        let predicate = |handle, collider: &crate::geometry::Collider| {
            base_filter.test(&*queries.bodies, handle, collider)
                && wheel_query_filter(wheel_id, handle, collider)
        };
        let filter = QueryFilter::new().predicate(&predicate);
        let hit = queries
            .as_ref()
            .with_filter(filter)
            .cast_ray_and_get_normal(&ray, 1.0, true);

        wheel.raycast_info.ground_object = None;

        if let Some((collider_hit, mut hit)) = hit {
            if hit.time_of_impact == 0.0 {
                let collider = &queries.colliders[collider_hit];
                let up_ray = Ray::new(source + rayvector, -rayvector);
                if let Some(hit2) =
                    collider
                        .shape
                        .cast_ray_and_get_normal(collider.position(), &up_ray, 1.0, false)
                {
                    hit.normal = -hit2.normal;
                }

                if hit.normal == Vector::ZERO {
                    // If the hit is still not defined, set the normal.
                    hit.normal = -wheel.wheel_direction_ws;
                }
            }

            wheel.raycast_info.contact_normal_ws = hit.normal;
            wheel.raycast_info.is_in_contact = true;
            wheel.raycast_info.ground_object = Some(collider_hit);

            let hit_distance = hit.time_of_impact * raylen;
            wheel.raycast_info.suspension_length = hit_distance - wheel.radius;

            // clamp on max suspension travel
            let min_suspension_length = wheel.suspension_rest_length - wheel.max_suspension_travel;
            let max_suspension_length = wheel.suspension_rest_length + wheel.max_suspension_travel;
            wheel.raycast_info.suspension_length = wheel
                .raycast_info
                .suspension_length
                .clamp(min_suspension_length, max_suspension_length);
            wheel.raycast_info.contact_point_ws = ray.point_at(hit.time_of_impact);

            let denominator = wheel
                .raycast_info
                .contact_normal_ws
                .dot(wheel.wheel_direction_ws);
            let chassis_velocity_at_contact_point =
                chassis.velocity_at_point(wheel.raycast_info.contact_point_ws);
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
            wheel.raycast_info.contact_normal_ws = -wheel.wheel_direction_ws;
            wheel.clipped_inv_contact_dot_suspension = 1.0;
        }
    }

    /// Updates the vehicle’s velocity based on its suspension, engine force, and brake.
    pub fn update_vehicle(&mut self, dt: Real, queries: QueryPipelineMut) {
        self.update_vehicle_with_filter(dt, queries, |_, _, _| true);
    }

    /// Updates the vehicle’s velocity based on its suspension, engine force, and brake,
    /// with an additional per-wheel collider filter applied to wheel ray casts.
    pub fn update_vehicle_with_filter<F>(
        &mut self,
        dt: Real,
        queries: QueryPipelineMut,
        wheel_query_filter: F,
    ) where
        F: Fn(usize, ColliderHandle, &crate::geometry::Collider) -> bool,
    {
        let num_wheels = self.wheels.len();
        let chassis = &queries.bodies[self.chassis];

        for i in 0..num_wheels {
            self.update_wheel_transform(chassis, i);
        }

        self.current_vehicle_speed = chassis.linvel().length();

        let forward_w = chassis.position().rotation * Vector::ith(self.index_forward_axis, 1.0);

        if forward_w.dot(chassis.linvel()) < 0.0 {
            self.current_vehicle_speed *= -1.0;
        }

        //
        // simulate suspension
        //

        for wheel_id in 0..self.wheels.len() {
            self.ray_cast(&queries, chassis, wheel_id, &wheel_query_filter);
        }

        let chassis_mass = chassis.mass();
        self.update_suspension(chassis_mass);

        let chassis = queries
            .bodies
            .get_mut_internal_with_modification_tracking(self.chassis)
            .unwrap();

        for wheel in &mut self.wheels {
            if wheel.engine_force > 0.0 {
                chassis.wake_up(true);
            }

            // apply suspension force
            let mut suspension_force = wheel.wheel_suspension_force;

            if suspension_force > wheel.max_suspension_force {
                suspension_force = wheel.max_suspension_force;
            }

            let impulse = wheel.raycast_info.contact_normal_ws * suspension_force * dt;
            chassis.apply_impulse_at_point(impulse, wheel.raycast_info.contact_point_ws, false);
        }

        self.update_friction(queries.bodies, queries.colliders, dt);

        let chassis = queries
            .bodies
            .get_mut_internal_with_modification_tracking(self.chassis)
            .unwrap();

        for wheel in &mut self.wheels {
            let vel = chassis.velocity_at_point(wheel.raycast_info.hard_point_ws);

            if wheel.raycast_info.is_in_contact {
                let mut fwd =
                    chassis.position().rotation * Vector::ith(self.index_forward_axis, 1.0);
                let proj = fwd.dot(wheel.raycast_info.contact_normal_ws);
                fwd -= wheel.raycast_info.contact_normal_ws * proj;

                let proj2 = fwd.dot(vel);

                wheel.delta_rotation = (proj2 * dt) / (wheel.radius);
                wheel.rotation += wheel.delta_rotation;
            } else {
                wheel.rotation += wheel.delta_rotation;
            }

            wheel.delta_rotation *= 0.99; //damping of rotation when not in contact
        }
    }

    /// Reference to all the wheels attached to this vehicle.
    pub fn wheels(&self) -> &[Wheel] {
        &self.wheels
    }

    /// Mutable reference to all the wheels attached to this vehicle.
    pub fn wheels_mut(&mut self) -> &mut [Wheel] {
        &mut self.wheels
    }

    fn update_suspension(&mut self, chassis_mass: Real) {
        for w_it in 0..self.wheels.len() {
            let wheels = &mut self.wheels[w_it];

            if wheels.raycast_info.is_in_contact {
                let mut force;
                //	Spring
                {
                    let rest_length = wheels.suspension_rest_length;
                    let current_length = wheels.raycast_info.suspension_length;
                    let length_diff = rest_length - current_length;

                    force = wheels.suspension_stiffness
                        * length_diff
                        * wheels.clipped_inv_contact_dot_suspension;
                }

                // Damper
                {
                    let projected_rel_vel = wheels.suspension_relative_velocity;
                    {
                        let susp_damping = if projected_rel_vel < 0.0 {
                            wheels.damping_compression
                        } else {
                            wheels.damping_relaxation
                        };
                        force -= susp_damping * projected_rel_vel;
                    }
                }

                // RESULT
                wheels.wheel_suspension_force = (force * chassis_mass).max(0.0);
            } else {
                wheels.wheel_suspension_force = 0.0;
            }
        }
    }
    fn update_friction(&mut self, bodies: &mut RigidBodySet, colliders: &ColliderSet, dt: Real) {
        let num_wheels = self.wheels.len();

        if num_wheels == 0 {
            return;
        }

        self.forward_ws.resize(num_wheels, Default::default());
        self.axle.resize(num_wheels, Default::default());

        let mut num_wheels_on_ground = 0;

        //TODO: collapse all those loops into one!
        for wheel in &mut self.wheels {
            let ground_object = wheel.raycast_info.ground_object;

            if ground_object.is_some() {
                num_wheels_on_ground += 1;
            }

            wheel.side_impulse = 0.0;
            wheel.forward_impulse = 0.0;
        }

        {
            for i in 0..num_wheels {
                let wheel = &mut self.wheels[i];
                let ground_object = wheel.raycast_info.ground_object;

                if ground_object.is_some() {
                    self.axle[i] = wheel.wheel_axle_ws;

                    let surf_normal_ws = wheel.raycast_info.contact_normal_ws;
                    let proj = self.axle[i].dot(surf_normal_ws);
                    self.axle[i] -= surf_normal_ws * proj;
                    self.axle[i] = self.axle[i].normalize_or_zero();
                    self.forward_ws[i] = surf_normal_ws.cross(self.axle[i]).normalize_or_zero();

                    if let Some(ground_body) = ground_object
                        .and_then(|h| colliders[h].parent())
                        .map(|h| &bodies[h])
                        .filter(|b| b.is_dynamic())
                    {
                        wheel.side_impulse = resolve_single_bilateral(
                            &bodies[self.chassis],
                            wheel.raycast_info.contact_point_ws,
                            ground_body,
                            wheel.raycast_info.contact_point_ws,
                            self.axle[i],
                        );
                    } else {
                        wheel.side_impulse = resolve_single_unilateral(
                            &bodies[self.chassis],
                            wheel.raycast_info.contact_point_ws,
                            self.axle[i],
                        );
                    }

                    wheel.side_impulse *= wheel.side_friction_stiffness;
                }
            }
        }

        let side_factor = 1.0;
        let fwd_factor = 0.5;

        let mut sliding = false;
        {
            for wheel_id in 0..num_wheels {
                let wheel = &mut self.wheels[wheel_id];
                let ground_object = wheel.raycast_info.ground_object;

                let mut rolling_friction = 0.0;

                if ground_object.is_some() {
                    if wheel.engine_force != 0.0 {
                        rolling_friction = wheel.engine_force * dt;
                    } else {
                        let default_rolling_friction_impulse = 0.0;
                        let max_impulse = if wheel.brake != 0.0 {
                            wheel.brake
                        } else {
                            default_rolling_friction_impulse
                        };
                        let contact_pt = WheelContactPoint::new(
                            &bodies[self.chassis],
                            ground_object
                                .and_then(|h| colliders[h].parent())
                                .map(|h| &bodies[h]),
                            wheel.raycast_info.contact_point_ws,
                            self.forward_ws[wheel_id],
                            max_impulse,
                        );
                        assert!(num_wheels_on_ground > 0);
                        rolling_friction = contact_pt.calc_rolling_friction(num_wheels_on_ground);
                    }
                }

                //switch between active rolling (throttle), braking and non-active rolling friction (no throttle/break)

                wheel.forward_impulse = 0.0;
                wheel.skid_info = 1.0;

                if ground_object.is_some() {
                    let max_imp = wheel.wheel_suspension_force * dt * wheel.friction_slip;
                    let max_imp_side = max_imp;
                    let max_imp_squared = max_imp * max_imp_side;
                    assert!(max_imp_squared >= 0.0);

                    wheel.forward_impulse = rolling_friction;

                    let x = wheel.forward_impulse * fwd_factor;
                    let y = wheel.side_impulse * side_factor;

                    let impulse_squared = x * x + y * y;

                    if impulse_squared > max_imp_squared {
                        sliding = true;

                        let factor = max_imp * crate::utils::inv(impulse_squared.sqrt());
                        wheel.skid_info *= factor;
                    }
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
        {
            let chassis = bodies
                .get_mut_internal_with_modification_tracking(self.chassis)
                .unwrap();

            for wheel_id in 0..num_wheels {
                let wheel = &self.wheels[wheel_id];
                let mut impulse_point = wheel.raycast_info.contact_point_ws;

                if wheel.forward_impulse != 0.0 {
                    chassis.apply_impulse_at_point(
                        self.forward_ws[wheel_id] * wheel.forward_impulse,
                        impulse_point,
                        false,
                    );
                }
                if wheel.side_impulse != 0.0 {
                    let side_impulse = self.axle[wheel_id] * wheel.side_impulse;

                    let v_chassis_world_up =
                        chassis.position().rotation * Vector::ith(self.index_up_axis, 1.0);
                    impulse_point -= v_chassis_world_up
                        * (v_chassis_world_up.dot(impulse_point - chassis.center_of_mass())
                            * (1.0 - wheel.roll_influence));

                    chassis.apply_impulse_at_point(side_impulse, impulse_point, false);

                    // TODO: apply friction impulse on the ground
                    // let ground_object = self.wheels[wheel_id].raycast_info.ground_object;
                    // ground_object.apply_impulse_at_point(
                    //     -side_impulse,
                    //     wheels.raycast_info.contact_point_ws,
                    //     false,
                    // );
                }
            }
        }
    }
}

struct WheelContactPoint<'a> {
    body0: &'a RigidBody,
    body1: Option<&'a RigidBody>,
    friction_position_world: Vector,
    friction_direction_world: Vector,
    jac_diag_ab_inv: Real,
    max_impulse: Real,
}

impl<'a> WheelContactPoint<'a> {
    pub fn new(
        body0: &'a RigidBody,
        body1: Option<&'a RigidBody>,
        friction_position_world: Vector,
        friction_direction_world: Vector,
        max_impulse: Real,
    ) -> Self {
        fn impulse_denominator(body: &RigidBody, pos: Vector, n: Vector) -> Real {
            let dpt = pos - body.center_of_mass();
            let gcross = dpt.gcross(n);
            let v = (body.mprops.effective_world_inv_inertia * gcross).gcross(dpt);
            // TODO: take the effective inv mass into account instead of the inv_mass?
            body.mprops.local_mprops.inv_mass + n.dot(v)
        }
        let denom0 = impulse_denominator(body0, friction_position_world, friction_direction_world);
        let denom1 = body1
            .map(|body1| {
                impulse_denominator(body1, friction_position_world, friction_direction_world)
            })
            .unwrap_or(0.0);
        let relaxation = 1.0;
        let jac_diag_ab_inv = relaxation / (denom0 + denom1);

        Self {
            body0,
            body1,
            friction_position_world,
            friction_direction_world,
            jac_diag_ab_inv,
            max_impulse,
        }
    }

    pub fn calc_rolling_friction(&self, num_wheels_on_ground: usize) -> Real {
        let contact_pos_world = self.friction_position_world;
        let max_impulse = self.max_impulse;

        let vel1 = self.body0.velocity_at_point(contact_pos_world);
        let vel2 = self
            .body1
            .map(|b| b.velocity_at_point(contact_pos_world))
            .unwrap_or_default();
        let vel = vel1 - vel2;
        let vrel = self.friction_direction_world.dot(vel);

        // calculate friction that moves us to zero relative velocity
        (-vrel * self.jac_diag_ab_inv / (num_wheels_on_ground as Real))
            .clamp(-max_impulse, max_impulse)
    }
}

fn resolve_single_bilateral(
    body1: &RigidBody,
    pt1: Vector,
    body2: &RigidBody,
    pt2: Vector,
    normal: Vector,
) -> Real {
    let vel1 = body1.velocity_at_point(pt1);
    let vel2 = body2.velocity_at_point(pt2);
    let dvel = vel1 - vel2;

    let dpt1 = pt1 - body1.center_of_mass();
    let dpt2 = pt2 - body2.center_of_mass();
    let aj = dpt1.gcross(normal);
    let bj = dpt2.gcross(-normal);
    let iaj = body1.mprops.effective_world_inv_inertia * aj;
    let ibj = body2.mprops.effective_world_inv_inertia * bj;

    // TODO: take the effective_inv_mass into account?
    let im1 = body1.mprops.local_mprops.inv_mass;
    let im2 = body2.mprops.local_mprops.inv_mass;

    let jac_diag_ab = im1 + im2 + iaj.gdot(iaj) + ibj.gdot(ibj);
    let jac_diag_ab_inv = crate::utils::inv(jac_diag_ab);
    let rel_vel = normal.dot(dvel);

    //todo: move this into proper structure
    let contact_damping = 0.2;
    -contact_damping * rel_vel * jac_diag_ab_inv
}

fn resolve_single_unilateral(body1: &RigidBody, pt1: Vector, normal: Vector) -> Real {
    let vel1 = body1.velocity_at_point(pt1);
    let dvel = vel1;
    let dpt1 = pt1 - body1.center_of_mass();
    let aj = dpt1.gcross(normal);
    let iaj = body1.mprops.effective_world_inv_inertia * aj;

    // TODO: take the effective_inv_mass into account?
    let im1 = body1.mprops.local_mprops.inv_mass;
    let jac_diag_ab = im1 + iaj.gdot(iaj);
    let jac_diag_ab_inv = crate::utils::inv(jac_diag_ab);
    let rel_vel = normal.dot(dvel);

    //todo: move this into proper structure
    let contact_damping = 0.2;
    -contact_damping * rel_vel * jac_diag_ab_inv
}
