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

use crate::DriveInput;
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
            // Walking has no speed to outrun, so this boom is sized by how
            // much of your surroundings you can see rather than by reaction
            // time: at 6.5m the character filled a fifth of the frame height
            // and the world was mostly hidden behind them. 9.0m keeps them
            // legible while showing enough around them to steer toward
            // something — the thing a walking camera is actually for.
            //
            // Still well inside the driving boom, which is the relationship
            // that matters: getting into a car should widen the view.
            distance: 9.0,
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
            // Sized against how far the car travels, not against how big it
            // looks parked. `CarConfig::top_speed` is 24 m/s, so a 9m boom
            // showed well under a second of road ahead — the reported "a bit
            // too close", and the reason a corner arrives before you can plan
            // for it. 13m is a shade over half a second at speed, and
            // `speed_pullback` takes the effective range to ~17m at cruise
            // while leaving it tight enough to park with.
            //
            // If `top_speed` ever moves, this wants to move with it: the boom
            // is a time budget, not a length.
            distance: 13.0,
            // Deliberately NOT raised alongside the boom. The eye sits at
            // `pivot.y + sin(pitch)·boom`, so a longer boom already lifts the
            // camera — about 1.2m more at the resting pitch — and the car
            // drops in frame on its own, which is the thing a higher pivot
            // would have been for. Driving stays lower than walking, which is
            // what makes a car feel planted.
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

    /// This camera's aim in the *renderer's* angle convention.
    ///
    /// The rig thinks in the sim's heading convention, because that is the only
    /// way `self.yaw = e.yaw` and recentring toward a velocity heading can mean
    /// anything. The renderer uses the opposite sign on BOTH axes. Deriving it
    /// rather than asserting it, since a guessed sign here is how the car ended
    /// up steering backwards:
    ///
    /// - This rig places the eye at
    ///   `pivot + (sin yaw·cos pitch, sin pitch, cos yaw·cos pitch)·boom`
    ///   (see [`Self::eye`]), so `+pitch` lifts the camera and looks down.
    /// - The renderer places it at `target − forward·distance` with
    ///   `forward = (sin y·cos p, sin p, −cos y·cos p)`, i.e. at
    ///   `target + (−sin y·cos p, −sin p, cos y·cos p)·distance`.
    ///
    /// Matching the two offsets component-wise gives `sin y = −sin yaw`,
    /// `cos y = cos yaw` and `sin p = −sin pitch` — so `y = −yaw`, `p = −pitch`.
    ///
    /// The conversion itself lives in [`makepad_game_sim::heading`], which owns
    /// every sign in this engine, rather than being a negation written here.
    pub fn view_angles(&self) -> (f32, f32) {
        (
            makepad_game_sim::heading_to_camera_yaw(self.yaw),
            makepad_game_sim::heading_to_camera_pitch(self.pitch),
        )
    }

    /// Yaw to rotate movement axes by — see [`Self::view_angles`].
    ///
    /// Separate accessor because [`makepad_game_sim::camera_relative_move`]
    /// takes a *view* yaw, not a heading, and passing `self.yaw` to it compiles
    /// perfectly while mirroring the player's controls about the Z axis.
    pub fn view_yaw(&self) -> f32 {
        makepad_game_sim::heading_to_camera_yaw(self.yaw)
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

/// How far to the side of a car the driver is placed when getting out. Wide
/// enough to clear a typical body's half-extent plus the character's own, so
/// the dismount does not start life overlapping the car it just left.
const DISMOUNT_REACH: f32 = 2.2;

/// Mount state: which seat, and the character parked while driving.
#[derive(Clone, Copy, Debug)]
pub struct Mount {
    pub seat: Seat,
    pub character: u64,
    /// How close the player must be to a vehicle to get in.
    pub reach: f32,
    /// Camera blend length when changing seats.
    pub blend: f32,
    /// What `hidden` was before we sat down, so getting out restores it rather
    /// than asserting a value.
    ///
    /// `hidden` is the HOST's field, meaning "my appearance comes from a mesh,
    /// so do not also draw my collider box". Every game that uses a model
    /// rather than a coloured box sets it — which is every real game. Clearing
    /// it on dismount pops a grey collision slab into the world next to the
    /// car, and the host cannot fix that without re-asserting `hidden` every
    /// tick forever. Seating is our state; visibility is theirs.
    was_hidden: bool,
}

impl Mount {
    pub fn new(character: u64) -> Self {
        Self {
            seat: Seat::OnFoot,
            character,
            reach: 3.5,
            blend: 0.35,
            was_hidden: false,
        }
    }

    /// The vehicle this player would get into if they pressed the button now,
    /// or `None` when nothing is in reach — or when they are already driving.
    ///
    /// Split out of [`Self::toggle`] so an affordance prompt ("Press E to get
    /// in") and the button itself run the SAME search. Two searches drift, and
    /// a prompt that disagrees with the button is worse than no prompt: it
    /// tells the player the game is broken at the exact moment they are trying
    /// to learn it.
    pub fn candidate(&self, world: &GameWorld) -> Option<u64> {
        if self.seat != Seat::OnFoot {
            return None;
        }
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
        best.map(|(id, _)| id)
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
                let car = self.candidate(world)?;
                // The character is parked, not destroyed: keeping the entity
                // means the walk state, the camera history and anything the
                // game attached to it all survive the round trip.
                //
                // Parked means RIDING, via the sim's own seat pin. `hidden`
                // alone would not do: it means "solid to everything, drawn by
                // nothing", so the driver's body stayed behind as an invisible
                // wall at the spot they boarded — drive back past it later and
                // you hit nothing you can see. `attached_to` takes the body out
                // of physics and carries it with the car, and releases itself
                // if the car despawns.
                if let Some(c) = world.entity_mut(self.character) {
                    self.was_hidden = c.hidden;
                    c.hidden = true;
                    c.attached_to = car;
                    c.attach_offset = vec3f(0.0, 0.0, 0.0);
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
                // Driver's door first, then the passenger side, then on top of
                // the car as a last resort. Preferring one side unconditionally
                // is how a player ends up inside a wall after parking against
                // one — and being dumped inside geometry is the single most
                // broken-feeling thing a get-out can do.
                let right = makepad_game_sim::heading_to_right(cyaw);
                let half = world
                    .entity(self.character)
                    .map_or(vec3f(0.4, 0.9, 0.4), |c| c.half);
                let door = |s: f32| {
                    vec3f(
                        cpos.x + right.x * DISMOUNT_REACH * s,
                        cpos.y + 0.5,
                        cpos.z + right.z * DISMOUNT_REACH * s,
                    )
                };
                let out = [door(-1.0), door(1.0)]
                    .into_iter()
                    .find(|p| makepad_game_sim::sense::spot_clear(world, self.character, *p, half))
                    .unwrap_or_else(|| vec3f(cpos.x, cpos.y + 0.5, cpos.z));
                if let Some(c) = world.entity_mut(self.character) {
                    // Restored, not cleared — see `was_hidden`.
                    c.hidden = self.was_hidden;
                    // Off the seat pin before the position write, or the next
                    // step would snap the body straight back onto the car.
                    c.attached_to = 0;
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

/// What a device reports this tick, before any interpretation.
///
/// Deliberately device-agnostic: a keyboard fills `move_x/move_y` with -1/0/+1,
/// a stick with its analogue value, a headset with its own mapping. The rig
/// normalises and deadzones them identically, so every input path feels the
/// same and none of them needs to know about the camera.
#[derive(Clone, Copy, Debug, Default)]
pub struct RawInput {
    /// Strafe / forward, unnormalised. +y is forward.
    pub move_x: f32,
    pub move_y: f32,
    /// Look delta THIS TICK (mouse pixels or stick × dt), not an absolute.
    pub look_dx: f32,
    pub look_dy: f32,
    pub jump: bool,
    pub jump_pressed: bool,
    /// Sprint, 0..1. A keyboard sends 1.0 for shift; a pad sends deflection.
    pub run: f32,
    /// Enter/exit a vehicle. Edge-triggered by the caller.
    pub use_pressed: bool,
    /// Analog throttle, −1 reverse … +1 forward. A pad fills this from
    /// `RT − LT`, a keyboard from W/S.
    ///
    /// Separate from `move_y` rather than derived from it, because a pad
    /// drives with the *triggers* and walks with the *stick*, and those are
    /// genuinely different axes on the device. The host fills both from the
    /// same keys and the rig picks by seat, so there is still no second code
    /// path — and analog triggers are most of why a pad beats a keyboard in a
    /// car. Binary throttle is a tell.
    pub throttle: f32,
    pub brake: f32,
    /// Handbrake, 0..1 — kills rear grip for a deliberate slide.
    pub handbrake: f32,
}

/// **The player prefab.** Character on foot, vehicle when seated, one camera
/// that follows whichever it is, and the mount between them.
///
/// This is the whole surface a game needs: build it once, call [`Self::tick`]
/// each frame with what the device reported, feed the returned [`DriveInput`]
/// to the blocks, then [`Self::apply_camera`]. A generated game should never
/// assemble a camera rig or a mount state machine by hand — if using this
/// takes more than a few lines, the prefab is the thing that needs fixing.
#[derive(Clone, Copy, Debug)]
pub struct PlayerRig {
    pub camera: FollowCamera,
    pub mount: Mount,
    /// Deadzone applied to the movement axes.
    pub deadzone: f32,
}

impl PlayerRig {
    /// `character` is the walker this player owns. Starts on foot.
    pub fn new(character: u64) -> Self {
        Self {
            camera: FollowCamera::new(CameraConfig::on_foot()),
            mount: Mount::new(character),
            deadzone: 0.12,
        }
    }

    /// Advance the rig and translate the device into game intent.
    ///
    /// Order matters: the seat is resolved first (so a mount this tick steers
    /// the thing you just got into rather than the one you left), then the
    /// camera, and only then are the movement axes rotated — because they are
    /// rotated by the camera's NEW yaw. Doing it the other way walks the player
    /// in the direction the camera was pointing a frame ago, which reads as
    /// lag on every direction change.
    pub fn tick(&mut self, world: &mut GameWorld, raw: RawInput) -> DriveInput {
        if raw.use_pressed {
            if let Some(seat) = self.mount.toggle(world) {
                let cfg = match seat {
                    Seat::OnFoot => CameraConfig::on_foot(),
                    Seat::Driving(_) => CameraConfig::in_vehicle(),
                };
                self.camera.transition_to(cfg, self.mount.blend);
            }
        }

        let subject = self.mount.subject();
        self.camera.tick(world, subject, raw.look_dx, raw.look_dy);
        // The rest of the engine reads movement intent through the world's own
        // camera yaw, so publish it rather than keeping a private copy — a
        // second source of "where is the player looking" is how the walking and
        // driving paths drift apart. `cam_yaw` feeds `player_move`, which
        // rotates by the same helper used below, so it takes the VIEW yaw.
        world.cam_yaw = self.camera.view_yaw();

        let (mx, my) = movement_intent(raw.move_x, raw.move_y, self.deadzone);
        // -my: screen "up"/W is forward, which is -z when the camera faces down
        // the axis. The yaw is the view yaw, not the rig's heading — see
        // `FollowCamera::view_angles` for why they differ.
        let (move_x, move_z) =
            makepad_game_sim::camera_relative_move(mx as f64, -my as f64, self.camera.view_yaw());

        DriveInput {
            // Driving reads steer/throttle; walking reads move_x/move_z. Both
            // are filled every tick so the seat switch needs no second path —
            // whichever block is listening simply reads its own fields.
            steer: mx,
            throttle: raw.throttle,
            brake: raw.brake,
            handbrake: raw.handbrake,
            move_x: move_x as f32,
            move_z: move_z as f32,
            jump: raw.jump,
            jump_pressed: raw.jump_pressed,
            pitch: my,
            roll: mx,
            look_dx: raw.look_dx,
            look_dy: raw.look_dy,
            run: raw.run,
            use_pressed: raw.use_pressed,
        }
    }

    /// Put the player back on foot without consulting the vehicle.
    ///
    /// The get-out in [`Mount::toggle`] reads the car to find a door to step
    /// out of, so it cannot run when the car is the thing that vanished. This
    /// is the recovery path for exactly that: the character reappears where it
    /// last was and the camera blends back to the walking rig, rather than the
    /// player being left holding controls attached to nothing.
    pub fn eject(&mut self, world: &mut GameWorld) {
        if self.mount.seat == Seat::OnFoot {
            return;
        }
        self.mount.seat = Seat::OnFoot;
        if let Some(c) = world.entity_mut(self.mount.character) {
            c.hidden = self.mount.was_hidden;
            // The sim clears this itself when an owner despawns, but eject is
            // also reachable by other routes; clearing it twice is free.
            c.attached_to = 0;
            c.vel = vec3f(0.0, 0.0, 0.0);
        }
        self.camera
            .transition_to(CameraConfig::on_foot(), self.mount.blend);
    }

    /// Publish the camera to the world for the renderer to read.
    ///
    /// Writes the *smoothed* pivot and the *already-obstruction-limited* boom,
    /// so the shipped feel survives: the renderer's generic path places the eye
    /// at `target - forward * distance`, which is exactly this rig's model.
    pub fn apply_camera(&self, world: &mut GameWorld) {
        world.cam_third = 0;
        world.cam_follow = 0;
        world.cam_target = self.camera.target();
        world.cam_distance = self.camera.boom;
    }

    /// Yaw/pitch for the renderer's camera rig, ready to hand straight to it.
    pub fn view_angles(&self) -> (f32, f32) {
        self.camera.view_angles()
    }

    pub fn seat(&self) -> Seat {
        self.mount.seat
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

    /// A world with one mover at the origin for the camera to follow.
    ///
    /// Not optional scaffolding: `FollowCamera::tick` returns early when its
    /// subject is missing, so a camera test run against an empty world silently
    /// exercises nothing and passes whatever it asserts.
    fn world_with_subject() -> GameWorld {
        use makepad_game_sim::{BodyKind, Entity};
        let mut world = GameWorld::new();
        world.push_entity(Entity {
            id: 1,
            kind: BodyKind::Mover,
            pos: vec3f(0.0, 0.0, 0.0),
            half: vec3f(0.4, 0.9, 0.4),
            scale: vec3f(1.0, 1.0, 1.0),
            ..Default::default()
        });
        world
    }

    /// Ground-plane forward and right for a *render* yaw. This is the basis
    /// `script/src/input.rs` documents and the one `scene.rs` builds its view
    /// matrix from — tests compare against it rather than against a sign
    /// written out by hand here.
    fn view_basis(yaw: f32) -> ((f32, f32), (f32, f32)) {
        let fwd = (yaw.sin(), -yaw.cos());
        // forward x up, i.e. (-f.z, f.x).
        (fwd, (yaw.cos(), yaw.sin()))
    }

    #[test]
    fn forward_walks_where_the_camera_looks() {
        // Deliberately convention-independent: this test never names a sign.
        // It asks the rig where its eye is, what it is aimed at, and which way
        // "forward" moves the player, then requires the third to agree with the
        // first two. A mirrored yaw, an inverted pitch, or a rotation handed a
        // heading where it wanted a view angle all fail it — and all three of
        // those pass a test that hand-writes the expected sign, because such a
        // test is written by the same person making the same sign assumption.
        use makepad_game_sim::{BodyKind, Entity};
        let mut world = GameWorld::new();
        world.push_entity(Entity {
            id: 1,
            kind: BodyKind::Mover,
            pos: vec3f(0.0, 0.0, 0.0),
            half: vec3f(0.4, 0.9, 0.4),
            scale: vec3f(1.0, 1.0, 1.0),
            ..Default::default()
        });
        // Several distinct aims: at yaw 0 a mirrored sign cancels itself out.
        for look in [0.0f32, 140.0, -260.0, 90.0] {
            let mut rig = PlayerRig::new(1);
            let mut drive = DriveInput::default();
            for _ in 0..8 {
                drive = rig.tick(
                    &mut world,
                    RawInput {
                        move_y: 1.0,
                        look_dx: look,
                        ..Default::default()
                    },
                );
            }
            let aim = rig.camera.target() - rig.camera.eye();
            let an = (aim.x * aim.x + aim.z * aim.z).sqrt();
            let (mx, mz) = (drive.move_x, drive.move_z);
            let mn = (mx * mx + mz * mz).sqrt();
            assert!(an > 1e-3 && mn > 0.5, "no aim/movement at look {look}");
            let dot = (aim.x / an) * (mx / mn) + (aim.z / an) * (mz / mn);
            assert!(
                dot > 0.999,
                "W walks {dot} off the camera's own aim at look {look}"
            );
        }
    }

    #[test]
    fn strafing_right_moves_toward_the_screen_right() {
        // The companion to the above: agreeing with the aim only pins the
        // forward axis, and a left/right mirror keeps forward exactly right
        // while swapping A and D. Screen-right is `aim × up` for the
        // right-handed frame the renderer builds its view matrix in.
        use makepad_game_sim::{BodyKind, Entity};
        let mut world = GameWorld::new();
        world.push_entity(Entity {
            id: 1,
            kind: BodyKind::Mover,
            pos: vec3f(0.0, 0.0, 0.0),
            half: vec3f(0.4, 0.9, 0.4),
            scale: vec3f(1.0, 1.0, 1.0),
            ..Default::default()
        });
        for look in [0.0f32, 140.0, -260.0] {
            let mut rig = PlayerRig::new(1);
            let mut drive = DriveInput::default();
            for _ in 0..8 {
                drive = rig.tick(
                    &mut world,
                    RawInput {
                        move_x: 1.0,
                        look_dx: look,
                        ..Default::default()
                    },
                );
            }
            let aim = rig.camera.target() - rig.camera.eye();
            // aim × (0,1,0) on the ground plane.
            let (rx, rz) = (-aim.z, aim.x);
            let rn = (rx * rx + rz * rz).sqrt();
            let (mx, mz) = (drive.move_x, drive.move_z);
            let mn = (mx * mx + mz * mz).sqrt();
            assert!(rn > 1e-3 && mn > 0.5, "no aim/movement at look {look}");
            let dot = (rx / rn) * (mx / mn) + (rz / rn) * (mz / mn);
            assert!(dot > 0.999, "D strafes {dot} off screen-right at {look}");
        }
    }

    #[test]
    fn looking_right_turns_the_view_right() {
        // Mouse-look, not drag-orbit. A drag-orbit camera moves the world with
        // the cursor (drag right, subject spins right, camera goes left); this
        // is a controller-style look, where right means right. The check runs
        // in the renderer's frame, not the rig's internal one, because that is
        // the frame the player's eyes are actually in.
        let world = world_with_subject();
        // From several starting aims — at yaw 0 a mirrored sign can still
        // produce a plausible-looking x, and only an off-axis start separates
        // "turned right" from "turned by the right amount in the wrong way".
        for start in [0.0f32, 1.1, -2.3] {
            let mut cam = FollowCamera::new(CameraConfig::on_foot());
            cam.tick(&world, 1, 0.0, 0.0); // initialise off the subject
            cam.yaw = start;
            let (before, _) = cam.view_angles();
            let (_, before_right) = view_basis(before);
            cam.tick(&world, 1, 200.0, 0.0);
            let (after, _) = cam.view_angles();
            let (after_fwd, _) = view_basis(after);
            // Did the aim swing toward where the player's right hand was?
            let d = after_fwd.0 * before_right.0 + after_fwd.1 * before_right.1;
            assert!(d > 0.0, "mouse right turned the view left at {start}: {d}");
        }
    }

    #[test]
    fn looking_down_lowers_the_view() {
        // Non-inverted by default: pushing the mouse away from you (screen y
        // grows downward) looks down. Checked as the render pitch's effect on
        // eye height rather than as a raw sign, since the two conventions
        // disagree about which sign "down" even is.
        let world = world_with_subject();
        let mut cam = FollowCamera::new(CameraConfig::on_foot());
        cam.tick(&world, 1, 0.0, 0.0);
        let (_, p0) = cam.view_angles();
        cam.tick(&world, 1, 0.0, 100.0);
        let (_, p1) = cam.view_angles();
        // eye.y = target.y - sin(pitch) * distance, so a lower render pitch
        // lifts the camera, and a camera lifted is a camera looking down.
        assert!(
            p1 < p0,
            "mouse down should raise the eye and look down: {p0} -> {p1}"
        );
        assert!(cam.eye().y > cam.target().y, "eye did not rise above target");
    }

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
        // Subject id 1 against a populated world, not 0 against an empty one:
        // `tick` returns before touching pitch when the subject is missing, so
        // the empty-world version of this test asserted the initial value of a
        // camera that had never run and would have passed against any clamp at
        // all — including none.
        let mut cam = FollowCamera::new(CameraConfig::on_foot());
        let world = world_with_subject();
        for _ in 0..600 {
            cam.tick(&world, 1, 0.0, 10_000.0);
        }
        assert!(cam.pitch <= cam.config.pitch_max + 1e-5, "{}", cam.pitch);
        assert!(cam.pitch > cam.config.pitch_max - 1e-3, "never reached max");
        for _ in 0..600 {
            cam.tick(&world, 1, 0.0, -10_000.0);
        }
        assert!(cam.pitch >= cam.config.pitch_min - 1e-5, "{}", cam.pitch);
        assert!(cam.pitch < cam.config.pitch_min + 1e-3, "never reached min");
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
    fn the_chase_camera_shows_enough_road_to_plan_a_turn() {
        // The boom is a time budget, not a length. Against a 24 m/s top speed
        // the old 9m showed under half a second of road, which is the reported
        // "a bit too close" and why a corner arrives before you can plan for
        // it. Expressed against `top_speed` so that raising the car's speed
        // fails here instead of quietly making the camera too tight again.
        let car = CameraConfig::in_vehicle();
        let top = crate::CarConfig::default().top_speed;
        let seconds = car.distance / top;
        assert!(seconds > 0.5, "only {seconds:.2}s of road ahead at top speed");
        // The speed pullback must not be the only thing keeping it usable:
        // parking and manoeuvring happen at zero speed, where it contributes
        // nothing.
        //
        // Stated as a MARGIN, not a ratio. This started life as
        // `> on_foot * 1.5`, which read as a decision but was really just the
        // quotient of two numbers that happened to be current — so correcting
        // the walking boom on its own merits broke it while nothing about
        // driving had changed. What actually has to hold is that getting into
        // a car visibly widens the view; several metres does that at any
        // walking distance, and a ratio does not.
        let gain = car.distance - CameraConfig::on_foot().distance;
        assert!(
            gain >= 3.5,
            "getting into a car only widens the view by {gain:.1}m — not enough to read as a change"
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
