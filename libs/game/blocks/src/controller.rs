//! The player prefab: a third-person camera that follows a character or a
//! vehicle, and the mount/dismount between them.
//!
//! **This prefab's feel is the engine's feel.** Every generated game inherits
//! it, and the AI will pass no knobs at all — so the defaults here are the
//! product, not a starting point. A mushy controller makes every kid's game
//! mushy.
//!
//! Everything in this module is **Local tier** (game.md §Multiplayer model):
//! where a player is looking never enters the world, never travels over the
//! network, and never affects the simulation. Two devices in one room can hold
//! different cameras and stay in sync — which is also what lets a Quest player
//! see the same race as a diorama.
//!
//! The character movement itself lives in [`crate::character`], on the mover
//! sweep. This module owns only the camera and the seat.

use makepad_game_sim::{camera_boom_limit, heading_delta, heading_to_forward, GameWorld, TICK_DT};
use makepad_math::*;

/// How the follow camera behaves. Defaults are the shipped feel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraConfig {
    /// Boom length behind the subject when unobstructed.
    pub distance: f32,
    /// Height of the pivot above the subject's origin — roughly head height,
    /// so the camera looks at the character rather than at its feet.
    pub pivot_height: f32,
    /// How fast the camera's POSITION catches up, per second. Deliberately
    /// slower than rotation: position lag reads as weight, rotation lag reads
    /// as sluggish, so they must not share a constant.
    pub follow_rate: f32,
    /// How fast the camera's AIM catches up, per second.
    pub rotate_rate: f32,
    /// Pitch limits, radians. Clamped so the player can neither invert the
    /// camera nor bury it in the floor.
    pub pitch_min: f32,
    pub pitch_max: f32,
    /// Radians per unit of look input.
    pub sensitivity: f32,
    /// The pivot leads the subject by this much of its velocity, so you see
    /// where you are going rather than where you have been.
    pub look_ahead: f32,
    /// Boom growth with speed: at `speed_ref` the camera has pulled back by
    /// this fraction. Pulling back with speed is what sells velocity when the
    /// road ahead is empty.
    pub speed_pullback: f32,
    pub speed_ref: f32,
    /// After the player stops steering the look, the camera drifts back behind
    /// the subject's heading at this rate. Zero disables recentring, which is
    /// what a walking camera wants and a driving camera does not.
    pub recentre: f32,
    /// Seconds of no look input before recentring starts — so the camera
    /// yields to the player's hand and only takes over once they let go.
    pub recentre_delay: f32,
}

impl CameraConfig {
    /// Walking: close, responsive, no recentring — the player aims the camera
    /// and it stays where they put it.
    pub fn on_foot() -> Self {
        Self {
            distance: 6.5,
            pivot_height: 1.5,
            follow_rate: 9.0,
            rotate_rate: 16.0,
            pitch_min: -0.45,
            pitch_max: 1.15,
            sensitivity: 0.0032,
            look_ahead: 0.18,
            speed_pullback: 0.0,
            speed_ref: 8.0,
            recentre: 0.0,
            recentre_delay: 0.0,
        }
    }

    /// Driving: further back, lower, pulls back with speed, and drifts behind
    /// the car once the player stops looking around.
    pub fn in_vehicle() -> Self {
        Self {
            distance: 9.0,
            pivot_height: 1.1,
            follow_rate: 6.5,
            rotate_rate: 9.0,
            pitch_min: -0.30,
            pitch_max: 0.85,
            sensitivity: 0.0032,
            look_ahead: 0.30,
            speed_pullback: 0.35,
            speed_ref: 22.0,
            recentre: 2.2,
            recentre_delay: 0.9,
        }
    }
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self::on_foot()
    }
}

/// A smoothed third-person camera. Hold one per player.
#[derive(Clone, Copy, Debug)]
pub struct FollowCamera {
    pub config: CameraConfig,
    /// Where the camera is aiming — smoothed toward the subject.
    pub pivot: Vec3f,
    pub yaw: f32,
    pub pitch: f32,
    /// Current boom length after obstruction. Eased rather than assigned:
    /// snapping IN is acceptable (a wall arrived), snapping OUT is a pop.
    pub boom: f32,
    /// Seconds since the player last moved the look. Drives recentring.
    idle: f32,
    /// Blend remaining when swapping rigs, in seconds. While non-zero the
    /// camera interpolates its config, so a mount is never a cut.
    blend: f32,
    /// The blend's original length. Progress needs BOTH — with only the
    /// remainder, `blend / blend` is 1.0 on every tick, the interpolation
    /// never advances, and the rig holds its old shape and then snaps: the
    /// exact cut the blend exists to avoid, and invisible in a test that only
    /// checks the endpoints.
    blend_total: f32,
    blend_from: CameraConfig,
    initialised: bool,
}

impl Default for FollowCamera {
    fn default() -> Self {
        Self::new(CameraConfig::on_foot())
    }
}

impl FollowCamera {
    pub fn new(config: CameraConfig) -> Self {
        Self {
            config,
            pivot: vec3f(0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.25,
            boom: config.distance,
            idle: 999.0,
            blend: 0.0,
            blend_total: 0.0,
            blend_from: config,
            initialised: false,
        }
    }

    /// Swap rigs over `seconds`. The old config is kept and interpolated from,
    /// so mounting a car eases the camera back and down instead of cutting.
    ///
    /// Interrupting a blend is safe: `effective()` is sampled first, so a second
    /// swap starts from where the camera actually is rather than from the rig it
    /// was heading toward.
    pub fn transition_to(&mut self, config: CameraConfig, seconds: f32) {
        self.blend_from = self.effective();
        self.config = config;
        self.blend = seconds.max(0.0);
        self.blend_total = self.blend;
    }

    /// The config actually in force this tick, mid-blend.
    fn effective(&self) -> CameraConfig {
        if self.blend <= 0.0 || self.blend_total <= 0.0 {
            return self.config;
        }
        // `blend` counts DOWN from `blend_total`, so t runs 0→1 as it expires.
        // Smoothstep rather than linear: a linear rig swap starts and stops
        // abruptly, which reads as two small cuts instead of one.
        let raw = (1.0 - self.blend / self.blend_total).clamp(0.0, 1.0);
        let t = raw * raw * (3.0 - 2.0 * raw);
        let l = |a: f32, b: f32| a + (b - a) * t;
        let (a, b) = (self.blend_from, self.config);
        CameraConfig {
            distance: l(a.distance, b.distance),
            pivot_height: l(a.pivot_height, b.pivot_height),
            follow_rate: l(a.follow_rate, b.follow_rate),
            rotate_rate: l(a.rotate_rate, b.rotate_rate),
            pitch_min: l(a.pitch_min, b.pitch_min),
            pitch_max: l(a.pitch_max, b.pitch_max),
            sensitivity: l(a.sensitivity, b.sensitivity),
            look_ahead: l(a.look_ahead, b.look_ahead),
            speed_pullback: l(a.speed_pullback, b.speed_pullback),
            speed_ref: l(a.speed_ref, b.speed_ref),
            recentre: l(a.recentre, b.recentre),
            recentre_delay: l(a.recentre_delay, b.recentre_delay),
        }
    }

    /// Advance one tick. `look_dx/look_dy` are this tick's raw look input;
    /// `subject` is the entity being followed.
    pub fn tick(&mut self, world: &GameWorld, subject: u64, look_dx: f32, look_dy: f32) {
        // Time advances before the subject lookup, and unconditionally. Bailing
        // out first would freeze a rig swap the moment its subject went away —
        // and a subject going away (a car despawned, an entity not yet spawned
        // on the first frame) is exactly when a blend is in flight.
        let cfg = self.effective();
        self.blend = (self.blend - TICK_DT).max(0.0);
        let Some(e) = world.entity(subject) else { return };

        // ---- aim ----------------------------------------------------------
        // No acceleration curve: mouse acceleration makes a camera feel like
        // it is fighting you, and a flat mapping is what players can learn.
        let looked = look_dx != 0.0 || look_dy != 0.0;
        self.yaw -= look_dx * cfg.sensitivity;
        self.pitch = (self.pitch + look_dy * cfg.sensitivity).clamp(cfg.pitch_min, cfg.pitch_max);
        self.idle = if looked { 0.0 } else { self.idle + TICK_DT };

        // Recentring yields to the hand: only after the player has stopped
        // looking for `recentre_delay`, and only for rigs that ask for it.
        if cfg.recentre > 0.0 && self.idle > cfg.recentre_delay {
            let planar = (e.vel.x * e.vel.x + e.vel.z * e.vel.z).sqrt();
            // Reversing should not spin the camera round to face backwards.
            if planar > 1.5 {
                let want = makepad_game_sim::forward_to_heading(vec3f(e.vel.x, 0.0, e.vel.z));
                let d = heading_delta(self.yaw, want);
                self.yaw += d * (cfg.recentre * TICK_DT).min(1.0);
            }
        }

        // ---- pivot --------------------------------------------------------
        // Lead the subject by part of its velocity so the player sees where
        // they are going. Smoothed, or the lead itself would jitter.
        let want_pivot = vec3f(
            e.pos.x + e.vel.x * cfg.look_ahead,
            e.pos.y + cfg.pivot_height,
            e.pos.z + e.vel.z * cfg.look_ahead,
        );
        if !self.initialised {
            self.pivot = want_pivot;
            self.yaw = e.yaw;
            self.initialised = true;
        }
        let k = (cfg.follow_rate * TICK_DT).min(1.0);
        self.pivot.x += (want_pivot.x - self.pivot.x) * k;
        self.pivot.y += (want_pivot.y - self.pivot.y) * k;
        self.pivot.z += (want_pivot.z - self.pivot.z) * k;

        // ---- boom ---------------------------------------------------------
        let planar = (e.vel.x * e.vel.x + e.vel.z * e.vel.z).sqrt();
        let speed_t = (planar / cfg.speed_ref.max(0.001)).clamp(0.0, 1.0);
        let want_len = cfg.distance * (1.0 + cfg.speed_pullback * speed_t);
        // Back along the aim, then pull in against whatever is in the way.
        let back = -heading_to_forward(self.yaw);
        let dir = vec3f(
            back.x * self.pitch.cos(),
            self.pitch.sin(),
            back.z * self.pitch.cos(),
        );
        let clear = camera_boom_limit(world, self.pivot, dir, want_len);
        // Snap IN when something arrives — a camera that eases into a wall
        // spends those frames inside it — but ease OUT when it clears, or the
        // shot pops the instant the player rounds a corner.
        self.boom = if clear < self.boom {
            clear
        } else {
            self.boom + (clear - self.boom) * (4.0 * TICK_DT).min(1.0)
        };
    }

    /// Eye position this tick.
    pub fn eye(&self) -> Vec3f {
        let back = -heading_to_forward(self.yaw);
        vec3f(
            self.pivot.x + back.x * self.pitch.cos() * self.boom,
            self.pivot.y + self.pitch.sin() * self.boom,
            self.pivot.z + back.z * self.pitch.cos() * self.boom,
        )
    }

    /// What the camera is looking at.
    pub fn target(&self) -> Vec3f {
        self.pivot
    }
}

/// What the local player is currently driving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seat {
    OnFoot,
    Driving(u64),
}

/// Mount state: which seat, and the character parked while driving.
#[derive(Clone, Copy, Debug)]
pub struct Mount {
    pub seat: Seat,
    pub character: u64,
    /// How close the player must be to a vehicle to get in.
    pub reach: f32,
    /// Camera blend length when changing seats.
    pub blend: f32,
}

impl Mount {
    pub fn new(character: u64) -> Self {
        Self {
            seat: Seat::OnFoot,
            character,
            reach: 3.5,
            blend: 0.35,
        }
    }

    /// Handle a mount/dismount press. Returns the new seat if it changed, so
    /// the caller can swap the camera rig and re-point input.
    ///
    /// Getting OUT places the character beside the vehicle rather than inside
    /// it: spawning in the seat means spawning inside a collider, and the
    /// separation pass would then shove the player through the nearest wall.
    pub fn toggle(&mut self, world: &mut GameWorld) -> Option<Seat> {
        match self.seat {
            Seat::OnFoot => {
                let pos = world.entity(self.character)?.pos;
                // Nearest vehicle within reach.
                let mut best: Option<(u64, f32)> = None;
                for e in world.entities.iter().filter(|e| e.tag == "car") {
                    let (dx, dz) = (e.pos.x - pos.x, e.pos.z - pos.z);
                    let d2 = dx * dx + dz * dz;
                    if d2 < self.reach * self.reach && best.map_or(true, |(_, b)| d2 < b) {
                        best = Some((e.id, d2));
                    }
                }
                let (car, _) = best?;
                // The character is parked, not destroyed: keeping the entity
                // means the walk state, the camera history and anything the
                // game attached to it all survive the round trip.
                if let Some(c) = world.entity_mut(self.character) {
                    c.hidden = true;
                    c.vel = vec3f(0.0, 0.0, 0.0);
                }
                self.seat = Seat::Driving(car);
                Some(self.seat)
            }
            Seat::Driving(car) => {
                let (cpos, cyaw) = {
                    let e = world.entity(car)?;
                    (e.pos, e.yaw)
                };
                // Step out to the left of the car's heading, on the ground.
                let side = makepad_game_sim::heading_to_right(cyaw);
                let out = vec3f(cpos.x - side.x * 2.2, cpos.y + 0.5, cpos.z - side.z * 2.2);
                if let Some(c) = world.entity_mut(self.character) {
                    c.hidden = false;
                    c.pos = out;
                    c.vel = vec3f(0.0, 0.0, 0.0);
                }
                self.seat = Seat::OnFoot;
                Some(self.seat)
            }
        }
    }

    /// The entity the camera should follow right now.
    pub fn subject(&self) -> u64 {
        match self.seat {
            Seat::OnFoot => self.character,
            Seat::Driving(car) => car,
        }
    }
}

/// Normalise a raw two-axis stick/WASD pair into movement intent.
///
/// Diagonals must not outrun cardinals — pressing W+D and travelling 1.41×
/// speed is the oldest bug in the genre — and a stick needs a deadzone or a
/// resting controller walks the player into a wall.
pub fn movement_intent(x: f32, y: f32, deadzone: f32) -> (f32, f32) {
    let mag = (x * x + y * y).sqrt();
    if mag < deadzone {
        return (0.0, 0.0);
    }
    // Rescale so intent ramps from 0 at the deadzone edge rather than jumping
    // to `deadzone` — otherwise the stick has a visible step just off centre.
    let live = ((mag - deadzone) / (1.0 - deadzone).max(0.0001)).clamp(0.0, 1.0);
    let s = live / mag;
    (x * s, y * s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagonal_is_not_faster_than_cardinal() {
        let (cx, cy) = movement_intent(1.0, 0.0, 0.1);
        let (dx, dy) = movement_intent(1.0, 1.0, 0.1);
        let card = (cx * cx + cy * cy).sqrt();
        let diag = (dx * dx + dy * dy).sqrt();
        assert!(
            (diag - card).abs() < 1e-5,
            "diagonal {diag} vs cardinal {card}"
        );
    }

    #[test]
    fn deadzone_rejects_rest_and_ramps_from_its_edge() {
        assert_eq!(movement_intent(0.05, 0.0, 0.2), (0.0, 0.0));
        // Just past the edge must be near zero, not a step to 0.2.
        let (x, _) = movement_intent(0.22, 0.0, 0.2);
        assert!(x > 0.0 && x < 0.05, "step off the deadzone edge: {x}");
    }

    #[test]
    fn pitch_cannot_invert_or_bury_the_camera() {
        let mut cam = FollowCamera::new(CameraConfig::on_foot());
        let world = GameWorld::new();
        for _ in 0..600 {
            cam.tick(&world, 0, 0.0, 10_000.0);
        }
        assert!(cam.pitch <= cam.config.pitch_max + 1e-5, "{}", cam.pitch);
        for _ in 0..600 {
            cam.tick(&world, 0, 0.0, -10_000.0);
        }
        assert!(cam.pitch >= cam.config.pitch_min - 1e-5, "{}", cam.pitch);
    }

    #[test]
    fn a_rig_swap_interpolates_instead_of_cutting() {
        let mut cam = FollowCamera::new(CameraConfig::on_foot());
        let foot = CameraConfig::on_foot().distance;
        let car = CameraConfig::in_vehicle().distance;
        cam.transition_to(CameraConfig::in_vehicle(), 0.4);

        // Sample the MIDDLE of the blend, not the endpoints. Checking only
        // where it starts and ends passes even when the rig holds its old
        // shape for the whole transition and snaps at the last tick — which is
        // precisely the bug this guards.
        let world = GameWorld::new();
        let mut seen_between = false;
        for _ in 0..24 {
            cam.tick(&world, 0, 0.0, 0.0);
            let d = cam.effective().distance;
            if d > foot + 0.15 && d < car - 0.15 {
                seen_between = true;
            }
        }
        assert!(
            seen_between,
            "camera never took an intermediate distance: {foot} -> {car} was a cut"
        );
        // And it must actually arrive.
        assert!(
            (cam.effective().distance - car).abs() < 1e-4,
            "blend did not finish: {}",
            cam.effective().distance
        );
    }

    #[test]
    fn vehicle_camera_sits_further_back_and_lower_than_the_walking_one() {
        let foot = CameraConfig::on_foot();
        let car = CameraConfig::in_vehicle();
        assert!(car.distance > foot.distance);
        assert!(car.pivot_height < foot.pivot_height);
        // And only the vehicle recentres: a walking camera must stay where the
        // player put it.
        assert_eq!(foot.recentre, 0.0);
        assert!(car.recentre > 0.0);
    }
}
