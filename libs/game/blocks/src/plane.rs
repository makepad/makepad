//! `game.plane` — arcade flight on a box3d rigid body.
//!
//! Deliberately not a flight simulator: no angle-of-attack curve, no stall
//! break, no spin. Lift is generated from forward airspeed and always points
//! along the aircraft's up axis, so pulling back always climbs and letting go
//! always levels out. A kid should be able to fly it with two keys, and a
//! dogfight should never end because someone departed controlled flight.

use makepad_box3d::body::*;
use makepad_box3d::math_functions::vec3 as b3vec3;
use makepad_game_sim::GameWorld;
use makepad_math::*;

use crate::car::{from_b3_quat, from_b3_vec3, normalize_or, rotate};
use crate::{ControlSource, DriveInput};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaneConfig {
    /// Thrust at full throttle, in units/second².
    pub thrust: f32,
    /// Airspeed at which lift exactly cancels gravity — the "it flies" speed.
    pub lift_speed: f32,
    /// Cap on level-flight airspeed.
    pub top_speed: f32,
    /// Control authority in radians/second.
    pub pitch_rate: f32,
    pub roll_rate: f32,
    pub yaw_rate: f32,
    /// Drag coefficient along the flight path.
    pub drag: f32,
    /// How strongly the nose is pulled toward the velocity vector — the
    /// weathervane that makes the plane point where it is going.
    pub stability: f32,
    /// Auto-level strength when roll input is released (0 disables).
    pub auto_level: f32,
}

impl Default for PlaneConfig {
    fn default() -> Self {
        Self {
            thrust: 40.0,
            lift_speed: 16.0,
            top_speed: 46.0,
            pitch_rate: 1.5,
            roll_rate: 2.6,
            yaw_rate: 0.7,
            drag: 0.25,
            stability: 2.4,
            auto_level: 1.1,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Plane {
    pub entity: u64,
    /// Which player drives this when `control` is `Player`. Defaults to the
    /// local device (player 0), so single-player games never mention it.
    pub owner: makepad_game_sim::PlayerId,
    pub config: PlaneConfig,
    pub control: ControlSource,
    pub input: DriveInput,
    /// 0..1, eased toward the throttle input so it can't be slammed.
    pub throttle: f32,
    /// Current airspeed along the nose (diagnostics, HUD).
    pub airspeed: f32,
    /// Audio-only retrigger clock (Local tier).
    pub audio_t: f32,
}

impl Plane {
    pub fn new(entity: u64, config: PlaneConfig, control: ControlSource) -> Self {
        Self {
            entity,
            owner: makepad_game_sim::PlayerId::LOCAL,
            config,
            control,
            input: DriveInput::default(),
            // Planes start with the engine running; a cold start at zero
            // throttle just falls out of the sky.
            throttle: 0.6,
            airspeed: 0.0,
            audio_t: 0.0,
        }
    }

    pub fn tick(&mut self, world: &mut GameWorld, player: &DriveInput) {
        if self.control == ControlSource::Player {
            self.input = *player;
        }
        let Some(body) = world.dynamics.rigid_body_of(self.entity) else {
            return;
        };
        let w = &mut world.dynamics.world;
        let mass = body_get_mass(w, body);
        if mass <= 0.0 {
            return;
        }
        let quat = from_b3_quat(body_get_rotation(w, body));
        let vel = from_b3_vec3(body_get_linear_velocity(w, body));
        let spin = from_b3_vec3(body_get_angular_velocity(w, body));

        let forward = rotate(quat, vec3f(0.0, 0.0, -1.0));
        let right = rotate(quat, vec3f(1.0, 0.0, 0.0));
        let up = rotate(quat, vec3f(0.0, 1.0, 0.0));

        // Throttle eases; `throttle` input maps 0..1 onto the lever.
        let want = ((self.input.throttle + 1.0) * 0.5).clamp(0.0, 1.0);
        let want = if self.input.throttle == 0.0 {
            self.throttle
        } else {
            want
        };
        self.throttle += (want - self.throttle) * 0.05;

        self.airspeed = vel.dot(forward);
        let speed = vel.length();

        // Thrust, capped so level flight settles at top_speed.
        let headroom = (1.0 - (speed / self.config.top_speed)).clamp(0.0, 1.0);
        let thrust = forward * (self.throttle * self.config.thrust * headroom * mass);
        body_apply_force_to_center(w, body, b3vec3(thrust.x, thrust.y, thrust.z), true);

        // Lift along the aircraft up axis, proportional to airspeed² and
        // normalised so lift_speed exactly holds altitude. Capped at exactly
        // 1g: any more and the wing out-pulls gravity at every speed, so the
        // plane balloons upward, the weathervane follows the climbing velocity
        // vector, and it loops over the top from level flight. Climb comes
        // from pointing the nose up and spending thrust — an energy trade,
        // which is also what makes the throttle feel like something.
        let gravity = world.gravity;
        let ratio = (self.airspeed.max(0.0) / self.config.lift_speed).clamp(0.0, 1.0);
        let lift = up * (ratio * ratio * gravity * mass);
        body_apply_force_to_center(w, body, b3vec3(lift.x, lift.y, lift.z), true);

        // Linear drag. Deliberately NOT a v² term: the thrust headroom above
        // already caps top speed, and stacking both made the equilibrium
        // airspeed fall under lift_speed — the plane sank at full throttle.
        let drag = vel * (-self.config.drag * mass);
        body_apply_force_to_center(w, body, b3vec3(drag.x, drag.y, drag.z), true);

        // Control authority scales with airspeed — no elevator authority when
        // parked, full authority in the air.
        let authority = (self.airspeed / self.config.lift_speed).clamp(0.0, 1.0);
        let inertia = mass * 2.0;

        let want_pitch = self.input.pitch.clamp(-1.0, 1.0) * self.config.pitch_rate * authority;
        let want_roll = self.input.roll.clamp(-1.0, 1.0) * self.config.roll_rate * authority;
        // Rudder is the steer axis, so a gamepad stick flies it.
        let want_yaw = self.input.steer.clamp(-1.0, 1.0) * self.config.yaw_rate * authority;

        let torque = right * ((want_pitch - spin.dot(right)) * inertia)
            + forward * ((-want_roll - spin.dot(forward)) * inertia)
            + up * ((want_yaw - spin.dot(up)) * inertia);
        body_apply_torque(w, body, b3vec3(torque.x, torque.y, torque.z), true);

        // Weathervane: rotate the nose toward the flight path. This is what
        // removes stalls — the plane always ends up pointing where it moves.
        if speed > 1.0 {
            let flight = normalize_or(vel, forward);
            let align = Vec3f::cross(forward, flight) * (self.config.stability * inertia * authority);
            body_apply_torque(w, body, b3vec3(align.x, align.y, align.z), true);
        }

        // Auto-level: with hands off the roll axis, right the wings.
        if self.config.auto_level > 0.0 && self.input.roll.abs() < 0.05 {
            let level = Vec3f::cross(right, normalize_or(
                vec3f(right.x, 0.0, right.z),
                vec3f(1.0, 0.0, 0.0),
            ));
            let level = level * (self.config.auto_level * inertia * authority);
            body_apply_torque(w, body, b3vec3(level.x, level.y, level.z), true);
        }
    }
}

impl Plane {
    /// Engine note by throttle. Planes carry further than cars, so the host
    /// places these with a wider range.
    pub fn emit_audio(&mut self, world: &GameWorld, out: &mut crate::audio_emit::AudioEmitter) {
        use makepad_game_audio::director::Category;
        let Some(entity) = world.entity(self.entity) else {
            return;
        };
        self.audio_t += makepad_game_sim::TICK_DT;
        if self.audio_t < 0.15 {
            return;
        }
        self.audio_t = 0.0;
        let speed = (entity.vel.x * entity.vel.x
            + entity.vel.y * entity.vel.y
            + entity.vel.z * entity.vel.z)
            .sqrt();
        let (gain, pitch) =
            crate::audio_emit::engine_note(speed, self.config.top_speed, self.throttle);
        out.cue("engine-air", Category::Movement, entity.pos, gain * 0.5, pitch);
    }
}
