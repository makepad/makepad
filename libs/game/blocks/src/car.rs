//! `game.car` — an arcade raycast vehicle on a box3d rigid chassis.
//!
//! ## Relationship to `xr/src/scene/raycast_vehicle.rs`
//!
//! That file is a faithful port of Bullet's `btRaycastVehicle`. It was read and
//! audited before writing this (game.md audit-before-adoption rule); three real
//! defects came out of that pass, recorded here because they still affect the
//! xr scene:
//!
//! 1. **Determinism**: `rotation_from_scaled_axis` builds the steering rotation
//!    with `Quat::from_axis_angle`, i.e. platform libm `sin`/`cos`. Sim state
//!    computed from libm transcendentals is not bit-reproducible across
//!    OS/arch, so it could never be replicated or replayed. Everything here
//!    goes through [`makepad_game_math`].
//! 2. **Wrong friction denominator**: `resolve_single_bilateral` /
//!    `resolve_single_unilateral` compute `iaj.dot(iaj)`, but the inverse
//!    effective mass along a jacobian is `(I⁻¹ aJ) · aJ` — Bullet's
//!    `MinvJt0.dot(aJ)`. Squaring the inertia-transformed jacobian instead
//!    makes the side-impulse denominator wrong (and unit-inconsistent), so
//!    lateral grip scales incorrectly with the inertia tensor.
//! 3. **Unguarded division**: `calc_rolling_friction` does
//!    `relaxation / (denom0 + denom1)` with no zero check, while the same file
//!    defines `inv_or_zero` for exactly this. A degenerate contact yields
//!    inf/NaN impulses that poison the body.
//!
//! Rather than ship a corrected Bullet port, this block uses the same
//! *structure* (four suspension raycasts, per-wheel contact forces) with an
//! arcade force model, for one reason that matters more than fidelity: the
//! brief requires a car that does not flip. Bullet applies lateral tire
//! impulses at the contact patch, well below the centre of mass, which is
//! precisely the rollover moment — `roll_influence` exists to fudge it away.
//! Here, suspension acts at the contact point (so the car still pitches under
//! braking and leans over bumps), while grip and drive act through the centre
//! of mass and steering is a yaw torque. No lateral force ever generates a
//! roll moment, so the car is stable by construction instead of by tuning.

use makepad_box3d::body::*;
use makepad_box3d::id::{BodyId, ShapeId};
use makepad_box3d::math_functions::{pos as b3pos, vec3 as b3vec3, Pos as B3Pos, Vec3 as B3Vec3};
use makepad_box3d::physics_world::world_cast_ray;
use makepad_box3d::shape::shape_get_body;
use makepad_box3d::types::default_query_filter;
use makepad_game_math as gm;
use makepad_game_sim::{GameWorld, TICK_DT};
use makepad_math::*;

use crate::{ControlSource, DriveInput};

/// Tuning for a car. Defaults are a light, grippy arcade kart — forgiving for
/// a kid at a keyboard, which is the design target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarConfig {
    /// Cruising cap in units/second; engine force tapers to zero here.
    pub top_speed: f32,
    /// Forward acceleration in units/second² at zero speed.
    pub accel: f32,
    /// Braking deceleration in units/second².
    pub braking: f32,
    /// Lateral grip as a fraction of sideways velocity killed per second.
    /// 1.0 ≈ drifty, 8.0 ≈ on rails.
    pub grip: f32,
    /// Peak yaw rate in radians/second at the speed where steering is best.
    pub steer_rate: f32,
    /// Speed at which steering authority peaks (below it, authority scales up
    /// with speed so a parked car doesn't pirouette).
    pub steer_peak_speed: f32,
    /// Suspension rest length below the chassis centre.
    pub suspension_rest: f32,
    /// Spring constant, in units of gravity per unit of compression.
    pub suspension_stiffness: f32,
    /// Critical-damping fraction for the suspension spring.
    pub suspension_damping: f32,
    /// Wheel radius (extends the probe ray past the rest length).
    pub wheel_radius: f32,
    /// Half-extents of the wheel rectangle, chassis-local (x = half track,
    /// z = half wheelbase).
    pub half_track: f32,
    pub half_wheelbase: f32,
    /// Aerodynamic drag coefficient (per second, applied to velocity).
    pub drag: f32,
    /// Number of seats (riders attach; used by the race kit for standings).
    pub seats: u32,
}

impl Default for CarConfig {
    fn default() -> Self {
        Self {
            top_speed: 24.0,
            accel: 22.0,
            braking: 28.0,
            grip: 7.0,
            steer_rate: 2.2,
            steer_peak_speed: 9.0,
            suspension_rest: 0.55,
            suspension_stiffness: 26.0,
            suspension_damping: 0.65,
            wheel_radius: 0.35,
            half_track: 0.75,
            half_wheelbase: 1.15,
            drag: 0.35,
            seats: 1,
        }
    }
}

/// Per-wheel state, kept for rendering (spin, ride height) and for the grounded
/// count the force model divides by.
#[derive(Clone, Copy, Debug, Default)]
pub struct Wheel {
    /// Chassis-local mount point.
    pub mount: Vec3f,
    /// 0 = fully extended, 1 = fully compressed.
    pub compression: f32,
    /// Suspension length last tick, for damper velocity.
    pub last_length: f32,
    pub grounded: bool,
    /// World-space contact point (rendering, skid effects).
    pub contact: Vec3f,
    /// Accumulated spin angle for the wheel mesh.
    pub spin: f32,
}

#[derive(Clone, Debug)]
pub struct Car {
    pub entity: u64,
    pub config: CarConfig,
    pub control: ControlSource,
    pub input: DriveInput,
    pub wheels: [Wheel; 4],
    /// Signed forward speed (negative = reversing), units/second.
    pub speed: f32,
    /// Waypoint driver (`game.autodrive`): the AI opponents in a race.
    pub route: Vec<Vec3f>,
    pub route_at: usize,
    /// Fraction of `top_speed` the AI driver aims for.
    pub route_pace: f32,
}

impl Car {
    pub fn new(entity: u64, config: CarConfig, control: ControlSource) -> Self {
        let mut wheels = [Wheel::default(); 4];
        // FL, FR, RL, RR — front is -z (the engine's facing convention).
        let corners = [
            (-1.0, -1.0),
            (1.0, -1.0),
            (-1.0, 1.0),
            (1.0, 1.0),
        ];
        for (wheel, (sx, sz)) in wheels.iter_mut().zip(corners) {
            wheel.mount = vec3f(sx * config.half_track, 0.0, sz * config.half_wheelbase);
            wheel.last_length = config.suspension_rest;
        }
        Self {
            entity,
            config,
            control,
            input: DriveInput::default(),
            wheels,
            speed: 0.0,
            route: Vec::new(),
            route_at: 0,
            route_pace: 1.0,
        }
    }

    /// True while every wheel is on the ground — the "planted" test the
    /// scenario suite uses to prove the car never flips.
    pub fn all_wheels_down(&self) -> bool {
        self.wheels.iter().all(|w| w.grounded)
    }

    pub fn wheels_down(&self) -> usize {
        self.wheels.iter().filter(|w| w.grounded).count()
    }

    pub fn tick(&mut self, world: &mut GameWorld, player: &DriveInput) {
        if self.control == ControlSource::Player {
            self.input = *player;
        }
        if !self.route.is_empty() {
            self.steer_along_route(world);
        }
        let Some(body) = world.dynamics.rigid_body_of(self.entity) else {
            // Spawn tick: the mirror reconciles at the end of step_world, so
            // the chassis body exists from the next tick onward.
            return;
        };
        let dynamics = &mut world.dynamics;
        let w = &mut dynamics.world;

        let quat = from_b3_quat(body_get_rotation(w, body));
        let center = from_b3_pos(body_get_world_center_of_mass(w, body));
        let vel = from_b3_vec3(body_get_linear_velocity(w, body));
        let mass = body_get_mass(w, body);
        if mass <= 0.0 {
            return;
        }

        // Chassis basis: front at -z, matching entity yaw semantics.
        let forward = rotate(quat, vec3f(0.0, 0.0, -1.0));
        let right = rotate(quat, vec3f(1.0, 0.0, 0.0));
        let up = rotate(quat, vec3f(0.0, 1.0, 0.0));

        self.speed = vel.dot(forward);

        // ---- suspension: the only forces applied off the centre of mass ----
        let probe = self.config.suspension_rest + self.config.wheel_radius;
        let mut grounded = 0;
        let mut ground_normal = vec3f(0.0, 0.0, 0.0);
        for wheel in self.wheels.iter_mut() {
            let mount_ws = center + rotate(quat, wheel.mount);
            let hit = cast_down(w, body, mount_ws, up * -1.0, probe);
            match hit {
                Some((point, normal, distance)) => {
                    let length = (distance - self.config.wheel_radius)
                        .clamp(0.0, self.config.suspension_rest);
                    let compression =
                        (self.config.suspension_rest - length) / self.config.suspension_rest;
                    // Damper works on the rate of change of spring length; a
                    // fixed tick means (Δlength / dt) is exact and stable.
                    let rate = (wheel.last_length - length) / TICK_DT;
                    let spring = compression * self.config.suspension_stiffness;
                    let damper = rate * self.config.suspension_damping
                        * self.config.suspension_stiffness.sqrt()
                        * 2.0;
                    let force = ((spring + damper) * mass).max(0.0);
                    body_apply_force(
                        w,
                        body,
                        b3vec3(up.x * force, up.y * force, up.z * force),
                        b3pos(point.x, point.y, point.z),
                        true,
                    );
                    wheel.compression = compression;
                    wheel.last_length = length;
                    wheel.grounded = true;
                    wheel.contact = point;
                    ground_normal = ground_normal + normal;
                    grounded += 1;
                }
                None => {
                    wheel.compression = 0.0;
                    wheel.last_length = self.config.suspension_rest;
                    wheel.grounded = false;
                }
            }
            wheel.spin += self.speed / self.config.wheel_radius.max(0.05) * TICK_DT;
        }

        if grounded == 0 {
            // Airborne: only drag, so a jump keeps its arc.
            let drag = vel * (-self.config.drag * mass);
            body_apply_force_to_center(w, body, b3vec3(drag.x, drag.y, drag.z), true);
            return;
        }
        let traction = grounded as f32 * 0.25;

        // Drive along the ground plane, not the chassis nose: pointing uphill
        // shouldn't turn throttle into lift.
        let normal = normalize_or(ground_normal, vec3f(0.0, 1.0, 0.0));
        let drive_dir = normalize_or(forward - normal * forward.dot(normal), forward);
        let grip_dir = normalize_or(right - normal * right.dot(normal), right);

        // ---- engine / brake, tapering to zero at top speed ----
        let throttle = self.input.throttle.clamp(-1.0, 1.0);
        let headroom = (1.0 - (self.speed / self.config.top_speed).abs()).clamp(0.0, 1.0);
        let engine = throttle * self.config.accel * headroom * traction;
        // Reverse gets half authority, like every arcade racer.
        let engine = if throttle < 0.0 && self.speed > 0.0 {
            -self.config.braking * traction
        } else if throttle < 0.0 {
            engine * 0.5
        } else {
            engine
        };
        let brake = self.input.brake.clamp(0.0, 1.0) * self.config.braking * traction;
        let brake_force = if self.speed.abs() > 0.05 {
            -self.speed.signum() * brake
        } else {
            0.0
        };
        let longitudinal = (engine + brake_force) * mass;
        let f = drive_dir * longitudinal;
        body_apply_force_to_center(w, body, b3vec3(f.x, f.y, f.z), true);

        // ---- lateral grip through the centre of mass (no roll moment) ----
        let slip = vel.dot(grip_dir);
        let handbrake = 1.0 - 0.75 * self.input.handbrake.clamp(0.0, 1.0);
        let grip_force = -slip * self.config.grip * traction * handbrake * mass;
        let g = grip_dir * grip_force;
        body_apply_force_to_center(w, body, b3vec3(g.x, g.y, g.z), true);

        // ---- steering as a yaw torque about the contact-plane normal ----
        let steer = self.input.steer.clamp(-1.0, 1.0);
        let speed_factor = (self.speed.abs() / self.config.steer_peak_speed).clamp(0.0, 1.0);
        // Reversing steers the other way, exactly like a real car.
        let direction = if self.speed < -0.1 { -1.0 } else { 1.0 };
        let want_yaw_rate = steer * self.config.steer_rate * speed_factor * direction;
        let yaw_rate = from_b3_vec3(body_get_angular_velocity(w, body)).dot(normal);
        // Torque toward the wanted yaw rate; the same term damps the residual
        // spin when the wheel is centred, which is what keeps it from tank-slapping.
        let inertia_scale = mass * (self.config.half_wheelbase * self.config.half_wheelbase);
        let torque = (want_yaw_rate - yaw_rate) * inertia_scale * 6.0 * traction;
        let t = normal * torque;
        body_apply_torque(w, body, b3vec3(t.x, t.y, t.z), true);

        // ---- drag + rolling resistance ----
        let drag = vel * (-self.config.drag * mass);
        body_apply_force_to_center(w, body, b3vec3(drag.x, drag.y, drag.z), true);

        // Keep the chassis upright: a gentle restoring torque toward world up,
        // active only while grounded. Cheap insurance on top of the force model
        // for the one case it can't cover — landing crooked off a ramp.
        let tilt = Vec3f::cross(up, vec3f(0.0, 1.0, 0.0));
        let righting = tilt * (mass * 1.5 * traction);
        body_apply_torque(w, body, b3vec3(righting.x, righting.y, righting.z), true);
    }

    /// Waypoint driver: steer toward the next point, ease off for corners.
    fn steer_along_route(&mut self, world: &GameWorld) {
        let Some(entity) = world.entity(self.entity) else {
            return;
        };
        let pos = entity.pos;
        let target = self.route[self.route_at % self.route.len()];
        let to = vec3f(target.x - pos.x, 0.0, target.z - pos.z);
        if to.length() < 4.0 {
            self.route_at = (self.route_at + 1) % self.route.len();
        }
        // Heading error in the chassis frame; -z forward means the yaw of a
        // direction is atan2(-x, -z), the same convention step_world uses.
        let want = gm::atan2(-to.x, -to.z);
        let mut diff = want - entity.yaw_from_orient();
        while diff > std::f32::consts::PI {
            diff -= std::f32::consts::TAU;
        }
        while diff < -std::f32::consts::PI {
            diff += std::f32::consts::TAU;
        }
        self.input.steer = (diff * 1.8).clamp(-1.0, 1.0);
        // Slow into the tight stuff so the AI doesn't understeer off the road.
        let corner = 1.0 - (diff.abs() / std::f32::consts::FRAC_PI_2).clamp(0.0, 0.8);
        self.input.throttle = (self.route_pace * corner).clamp(0.0, 1.0);
        self.input.brake = 0.0;
    }
}

/// Cast straight down from a wheel mount, skipping the car's own chassis.
fn cast_down(
    world: &makepad_box3d::physics_world::World,
    chassis: BodyId,
    from: Vec3f,
    dir: Vec3f,
    distance: f32,
) -> Option<(Vec3f, Vec3f, f32)> {
    let translation = dir * distance;
    let mut best: Option<(Vec3f, Vec3f, f32)> = None;
    {
        let best = &mut best;
        let mut hit = |shape: ShapeId,
                       point: B3Pos,
                       normal: B3Vec3,
                       fraction: f32,
                       _material: u64,
                       _tri: i32,
                       _child: i32|
         -> f32 {
            if shape_get_body(world, shape) == chassis {
                // Ignore our own body, keep the current clip distance.
                return -1.0;
            }
            *best = Some((
                from_b3_pos(point),
                from_b3_vec3(normal),
                fraction * distance,
            ));
            fraction
        };
        world_cast_ray(
            world,
            b3pos(from.x, from.y, from.z),
            b3vec3(translation.x, translation.y, translation.z),
            default_query_filter(),
            &mut hit,
        );
    }
    best.map(|(point, normal, d)| {
        // A degenerate normal (ray starting inside geometry) falls back to the
        // probe direction, so the suspension still pushes the car out.
        let normal = if normal.length() < 1.0e-6 {
            dir * -1.0
        } else {
            normal
        };
        (point, normal, d)
    })
}

pub(crate) fn normalize_or(v: Vec3f, fallback: Vec3f) -> Vec3f {
    let length = v.length();
    if length > 1.0e-6 {
        v * (1.0 / length)
    } else {
        fallback
    }
}

pub(crate) fn rotate(q: Quat, v: Vec3f) -> Vec3f {
    // q * v * q⁻¹, expanded — pure arithmetic, no transcendentals.
    let u = vec3f(q.x, q.y, q.z);
    let s = q.w;
    u * (2.0 * u.dot(v)) + v * (s * s - u.dot(u)) + Vec3f::cross(u, v) * (2.0 * s)
}

pub(crate) fn from_b3_quat(q: makepad_box3d::math_functions::Quat) -> Quat {
    Quat {
        x: q.v.x,
        y: q.v.y,
        z: q.v.z,
        w: q.s,
    }
}

pub(crate) fn from_b3_vec3(v: B3Vec3) -> Vec3f {
    vec3f(v.x, v.y, v.z)
}

pub(crate) fn from_b3_pos(p: B3Pos) -> Vec3f {
    vec3f(p.x, p.y, p.z)
}

/// Yaw of an entity's visual facing, whichever field carries it: rigids get a
/// full orientation from box3d, everything else steers with `yaw`.
pub(crate) trait EntityYaw {
    fn yaw_from_orient(&self) -> f32;
}

impl EntityYaw for makepad_game_sim::Entity {
    fn yaw_from_orient(&self) -> f32 {
        if self.orient == Quat::default() {
            return self.yaw;
        }
        let forward = rotate(self.orient, vec3f(0.0, 0.0, -1.0));
        gm::atan2(-forward.x, -forward.z)
    }
}
