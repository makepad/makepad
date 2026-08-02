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
        let speed = config.speed;
        // speed_mult is the engine-side debuff the walking code never sees —
        // same contract game.walk honours.
        entity.vel.x = input.move_x * speed * entity.speed_mult;
        entity.vel.z = input.move_z * speed * entity.speed_mult;
        entity.turn_rate = config.turn_rate;
        if input.jump_pressed && entity.on_floor {
            entity.vel.y = config.jump;
        }
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
