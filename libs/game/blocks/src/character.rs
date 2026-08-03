//! `game.character` — the kinematic walker, formalized as a block.
//!
//! The movement itself is NOT reimplemented here: it stays the mover sweep in
//! `step_world` (axis-separated x/z/y, 0.55 step-up, movers pass through each
//! other). That sweep is Godot-parity-proven and tape-locked, so a block that
//! forked it would be a second, subtly different character controller. This
//! block sets `vel` before the sweep and reads the result after — everything
//! else it owns is animation state.
//!
//! Animation is **Derived tier** (game.md §Multiplayer model): the clip blend
//! is recomputed from horizontal speed and airborne state, so it never travels
//! over the network — a client reproduces a remote player's walk cycle from the
//! position stream alone.

use makepad_game_sim::{BodyKind, GameWorld, TICK_DT};
use crate::{ControlSource, DriveInput};

/// Which camera the host should mount for this character.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Third,
    First,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterConfig {
    pub speed: f32,
    /// Multiplier applied when the run input is held (kept for parity with the
    /// clip set: KayKit rigs ship idle/walk/run).
    pub run_multiplier: f32,
    pub jump: f32,
    /// Step this tall is walked up for free. 0.55 = the shipped feel.
    pub step_up: f32,
    pub view: ViewMode,
    /// Turn rate toward the movement direction (radians/second).
    pub turn_rate: f32,

    // ---- feel ----
    //
    // These are the difference between a character that works and one that
    // feels good. Every generated game inherits them, so the DEFAULTS are the
    // product: `game.player_character({})` must feel right with nothing passed.
    /// Ground acceleration, u/s². Instant velocity reads as sliding on ice.
    pub accel: f32,
    /// Ground deceleration when the stick is released. Higher than accel so a
    /// stop is crisp while a start still ramps.
    pub decel: f32,
    /// Fraction of ground acceleration available in the air. Zero air control
    /// feels broken; full air control feels like flying.
    pub air_control: f32,
    /// Jump still fires this long after walking off a ledge. Without it
    /// players are certain the jump "didn't register" — they pressed it two
    /// frames late and the game was right, which is no comfort.
    pub coyote_time: f32,
    /// A jump pressed this long BEFORE landing fires on touchdown. Same
    /// complaint from the other side.
    pub jump_buffer: f32,
    /// Upward velocity is multiplied by this when the button is released while
    /// still rising, giving a short hop for a tap and full height for a hold.
    pub jump_cut: f32,
    /// Gravity multiplier while falling. Symmetric gravity reads as floaty:
    /// the rise is the part the player controls, the fall should be brisk.
    pub fall_gravity: f32,
    /// Horizontal speed is damped by this on touchdown for `land_recovery`
    /// seconds, so landing has weight instead of continuing as if nothing
    /// happened.
    pub land_damp: f32,
    pub land_recovery: f32,
}

impl Default for CharacterConfig {
    fn default() -> Self {
        Self {
            speed: 6.0,
            run_multiplier: 1.7,
            jump: 11.0,
            step_up: 0.55,
            view: ViewMode::Third,
            turn_rate: 9.0,
            // Tuned by feel, then pinned by the shape tests in this module.
            accel: 55.0,
            decel: 75.0,
            air_control: 0.45,
            coyote_time: 0.12,
            jump_buffer: 0.14,
            jump_cut: 0.45,
            fall_gravity: 1.7,
            land_damp: 0.72,
            land_recovery: 0.09,
        }
    }
}

/// What the renderer needs to pose a skinned mesh. Blend weights sum to 1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CharacterPose {
    /// 0 = idle, 1 = walk (the idle↔walk crossfade).
    pub walk_blend: f32,
    /// 0 = walk, 1 = run (layered on top of walk_blend).
    pub run_blend: f32,
    pub airborne: bool,
    /// Seconds accumulated in the current locomotion cycle — the clip clock.
    pub clip_time: f32,
}

#[derive(Clone, Debug)]
pub struct Character {
    /// Audio-only state: stride phase and the fall we were in last tick.
    /// Kept here rather than in the sim because sound is Local tier.
    pub step: crate::audio_emit::StepTimer,
    pub audio_airborne: bool,
    pub audio_fall_speed: f32,
    pub entity: u64,
    /// Which player drives this when `control` is `Player`. Defaults to the
    /// local device (player 0), so single-player games never mention it.
    pub owner: makepad_game_sim::PlayerId,
    pub config: CharacterConfig,
    pub control: ControlSource,
    pub input: DriveInput,
    pub pose: CharacterPose,
    /// Optional skinned model name/path the host resolves (stock rig or a
    /// downloaded glTF); None renders the primitive body.
    pub model: Option<String>,

    // ---- jump craft state ----
    /// Seconds of ledge grace remaining (coyote time).
    coyote: f32,
    /// Seconds an unserviced jump press stays queued (jump buffer).
    buffered: f32,
    /// True between takeoff and apex — the window where releasing the button
    /// still shortens the jump.
    rising: bool,
    /// Seconds of post-landing horizontal damping remaining.
    landing: f32,
    /// `on_floor` last tick, to detect the takeoff and touchdown edges.
    was_grounded: bool,
}

impl Character {
    pub fn new(
        entity: u64,
        config: CharacterConfig,
        control: ControlSource,
        model: Option<String>,
    ) -> Self {
        Self {
            entity,
            owner: makepad_game_sim::PlayerId::LOCAL,
            config,
            control,
            input: DriveInput::default(),
            pose: CharacterPose::default(),
            model,
            step: crate::audio_emit::StepTimer::default(),
            audio_airborne: false,
            audio_fall_speed: 0.0,
            coyote: 0.0,
            buffered: 0.0,
            rising: false,
            landing: 0.0,
            was_grounded: false,
        }
    }

    /// Drive phase: intent → velocity, consumed by the mover sweep.
    pub fn tick(&mut self, world: &mut GameWorld, player: &DriveInput) {
        if self.control == ControlSource::Player {
            self.input = *player;
        }
        let input = self.input;
        let config = self.config;
        let Some(entity) = world.entity_mut(self.entity) else {
            return;
        };
        if entity.kind != BodyKind::Mover {
            return;
        }
        // Riders (a seated passenger) are pinned by the sweep; don't fight it.
        if entity.attached_to != 0 {
            return;
        }
        let grounded = entity.on_floor;

        // ---- timers -------------------------------------------------------
        // Coyote runs from the moment the ground is lost, NOT from the jump;
        // buffering runs from the press. Both are decremented before use so a
        // press and a landing on the same tick still connect.
        if grounded {
            self.coyote = config.coyote_time;
        } else {
            self.coyote = (self.coyote - TICK_DT).max(0.0);
        }
        if input.jump_pressed {
            self.buffered = config.jump_buffer;
        } else {
            self.buffered = (self.buffered - TICK_DT).max(0.0);
        }
        // Touchdown: brief horizontal damping so landing has weight.
        if grounded && !self.was_grounded {
            self.landing = config.land_recovery;
        }
        self.landing = (self.landing - TICK_DT).max(0.0);
        self.was_grounded = grounded;

        // ---- horizontal: ramp toward intent, never snap -------------------
        // Diagonals are normalised by the caller, so `intent` is already a
        // unit-or-less vector: a diagonal must not outrun a cardinal.
        let speed = if input.run {
            config.speed * config.run_multiplier
        } else {
            config.speed
        } * entity.speed_mult;
        let want_x = input.move_x * speed;
        let want_z = input.move_z * speed;
        let moving = input.move_x != 0.0 || input.move_z != 0.0;
        // Accelerating and stopping are different gestures: a start should
        // ramp, a stop should be crisp. In the air the player keeps SOME
        // authority — none feels broken, all feels like flying.
        let rate = if moving { config.accel } else { config.decel }
            * if grounded { 1.0 } else { config.air_control }
            * if self.landing > 0.0 { config.land_damp } else { 1.0 };
        let step = rate * TICK_DT;
        entity.vel.x += (want_x - entity.vel.x).clamp(-step, step);
        entity.vel.z += (want_z - entity.vel.z).clamp(-step, step);
        entity.turn_rate = config.turn_rate;

        // ---- jump ---------------------------------------------------------
        // Fires on the BUFFERED press against the COYOTE window, which is what
        // makes both grace periods work in the same expression. Consuming both
        // stops one press producing two jumps.
        if self.buffered > 0.0 && self.coyote > 0.0 {
            entity.vel.y = config.jump;
            self.buffered = 0.0;
            self.coyote = 0.0;
            // Variable height only applies to callers who actually report the
            // button being HELD. A caller that sends `jump_pressed` alone —
            // the older single-flag convention, and what a generated game will
            // most likely write — would otherwise get its jump cut on the very
            // next tick and never understand why. Opting in on evidence keeps
            // the expressive control for those who wire it and full height for
            // everyone else.
            self.rising = input.jump;
        }
        // Variable height: releasing while still rising cuts the ascent. Once
        // past the apex there is nothing left to cut.
        if self.rising {
            if entity.vel.y <= 0.0 {
                self.rising = false;
            } else if !input.jump {
                entity.vel.y *= config.jump_cut;
                self.rising = false;
            }
        }
        // Asymmetric gravity: the rise is the part the player steers, the fall
        // should be brisk. gravity_scale is the sim's own knob, so this needs
        // no second integrator that could disagree with the sweep.
        entity.gravity_scale = if entity.vel.y < 0.0 && !grounded {
            config.fall_gravity
        } else {
            1.0
        };
    }

    /// Observe phase: the sweep has run, so velocity and `on_floor` are final.
    pub fn post_tick(&mut self, world: &mut GameWorld) {
        let Some(entity) = world.entity(self.entity) else {
            return;
        };
        let planar = (entity.vel.x * entity.vel.x + entity.vel.z * entity.vel.z).sqrt();
        let walk_speed = self.config.speed.max(0.001);
        // Idle below a scuff, full walk at the configured speed, run past it.
        let target_walk = (planar / (walk_speed * 0.6)).clamp(0.0, 1.0);
        let target_run =
            ((planar - walk_speed) / (walk_speed * (self.config.run_multiplier - 1.0)).max(0.001))
                .clamp(0.0, 1.0);
        // Exponential ease at a fixed dt — deterministic, no wall clock.
        let ease = (12.0 * TICK_DT).min(1.0);
        self.pose.walk_blend += (target_walk - self.pose.walk_blend) * ease;
        self.pose.run_blend += (target_run - self.pose.run_blend) * ease;
        self.pose.airborne = !entity.on_floor;
        // The clip clock advances with distance travelled, so feet don't skate.
        let cycle_rate = 0.55 + planar / walk_speed.max(0.001) * 0.85;
        self.pose.clip_time += TICK_DT * cycle_rate;
        if self.pose.clip_time > 1000.0 {
            self.pose.clip_time -= 1000.0;
        }
    }
}

impl Character {
    /// Footsteps, jumps and landings, driven by the walk cycle rather than a
    /// timer so feet and sound stay together when the character slows down.
    pub fn emit_audio(&mut self, world: &GameWorld, out: &mut crate::audio_emit::AudioEmitter) {
        use makepad_game_audio::director::Category;
        let Some(entity) = world.entity(self.entity) else {
            return;
        };
        let planar = (entity.vel.x * entity.vel.x + entity.vel.z * entity.vel.z).sqrt();
        let grounded = entity.on_floor;

        // Landing: airborne last tick, on the floor now. Louder the harder
        // the fall, which is the cue a player actually reads.
        if grounded && self.audio_airborne {
            let impact = (-self.audio_fall_speed / 12.0).clamp(0.15, 1.0);
            out.cue("land", Category::Movement, entity.pos, impact, 1.0);
            self.step.advance(0.0, 0.0, 1.0); // resync the stride
        }
        // Jump: left the floor with upward velocity.
        if !grounded && !self.audio_airborne && entity.vel.y > 0.5 {
            out.cue("jump", Category::Movement, entity.pos, 0.7, 1.0);
        }
        if grounded && self.step.advance(TICK_DT, planar, self.config.speed.max(0.5) * 0.75) {
            // Running steps land harder than walking ones.
            let effort = (planar / self.config.speed.max(0.001)).clamp(0.3, 1.4);
            out.cue("footstep", Category::Movement, entity.pos, effort * 0.6, 1.0);
        }
        self.audio_airborne = !grounded;
        self.audio_fall_speed = entity.vel.y;
    }
}
