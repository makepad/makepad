//! Makepad Arcade building blocks (game.md §"Building blocks").
//!
//! The thesis: **script orchestrates, engine simulates**. A game says
//! `game.car({pos: …, player: true})` once; from then on this crate drives
//! that car at 60Hz in Rust. No per-entity-per-tick script calls — the splash
//! program stays a layout + rules document, which is exactly the part an AI is
//! good at writing.
//!
//! Each block is: a config struct (what script passes), engine state (what the
//! block remembers between ticks), and a tick function. Blocks are keyed by
//! entity id and reconciled against the entity list, so `game.remove`, eval
//! rollback and reset all drop their block state automatically — the same
//! discipline [`makepad_game_sim::dynamics`] uses for box3d bodies.
//!
//! Ordering inside one tick (host calls both, sim step in between):
//! ```text
//!   script on_tick  →  Blocks::pre_step   (drive: inputs, forces, brains)
//!                   →  step_world         (mover sweep + box3d solve)
//!                   →  Blocks::post_step  (observe: laps, standings, anim)
//! ```
//! Driving before the sim means a block's decisions land in the same tick;
//! observing after means lap/standings data reflects final positions.
//!
//! Determinism: every transcendental goes through [`makepad_game_math`], never
//! platform libm (game.md determinism rule) — blocks state is Shared-tier and
//! must replicate bit-exactly.

use makepad_game_sim::GameWorld;

pub mod brain;
pub mod car;
pub mod character;
pub mod plane;
pub mod race;

pub use brain::{Brain, BrainKind};
pub use car::{Car, CarConfig};
pub use character::{Character, CharacterConfig, CharacterPose};
pub use plane::{Plane, PlaneConfig};
pub use race::{Checkpoint, RaceKit, SpawnPoint, Standing};

/// One frame of control intent for a driveable block. The host fills this from
/// the keyboard/pad for the local player; script fills it for AI (`game.drive`);
/// M2 will fill it from a network input packet — same struct, three sources.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DriveInput {
    /// -1 left … +1 right.
    pub steer: f32,
    /// -1 reverse … +1 forward.
    pub throttle: f32,
    /// 0..1 foot brake.
    pub brake: f32,
    /// 0..1 handbrake (kills rear grip).
    pub handbrake: f32,
    /// Camera-relative movement for characters (already yaw-rotated).
    pub move_x: f32,
    pub move_z: f32,
    pub jump: bool,
    pub jump_pressed: bool,
    /// Aircraft: -1 nose down … +1 nose up.
    pub pitch: f32,
    /// Aircraft: -1 roll left … +1 roll right.
    pub roll: f32,
}

/// Where a block's [`DriveInput`] comes from each tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ControlSource {
    /// The local player's device input (host fills `Blocks::player_input`).
    Player,
    /// Whatever was last written by `game.drive` / an AI driver.
    #[default]
    Script,
}

/// All live blocks in one world. Cloneable so the host can snapshot it beside
/// [`GameWorld`] for eval rollback — a failed edit restores blocks and world
/// together or the two would disagree about which entities exist.
#[derive(Clone, Debug, Default)]
pub struct Blocks {
    pub cars: Vec<Car>,
    pub characters: Vec<Character>,
    pub planes: Vec<Plane>,
    pub brains: Vec<Brain>,
    pub race: RaceKit,
    /// This device's input, refreshed by the host before `pre_step`.
    pub player_input: DriveInput,
}

impl Blocks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop everything — the host calls this alongside `reset_content` so a
    /// re-eval rebuilds blocks from scratch (script re-runs top to bottom).
    pub fn clear(&mut self) {
        *self = Self {
            // The player's live input is device state, not world content: it
            // survives a reload exactly like a held key does.
            player_input: self.player_input,
            ..Default::default()
        };
    }

    pub fn is_empty(&self) -> bool {
        self.cars.is_empty()
            && self.characters.is_empty()
            && self.planes.is_empty()
            && self.brains.is_empty()
            && self.race.is_empty()
    }

    /// Drop block state whose entity is gone (removed, expired, rolled back).
    fn reconcile(&mut self, world: &GameWorld) {
        let alive = |id: u64| world.entity(id).is_some();
        self.cars.retain(|c| alive(c.entity));
        self.characters.retain(|c| alive(c.entity));
        self.planes.retain(|p| alive(p.entity));
        self.brains.retain(|b| alive(b.entity));
        self.race.reconcile(world);
    }

    /// Phase 1: turn intent into motion — runs BEFORE the sim step.
    pub fn pre_step(&mut self, world: &mut GameWorld) {
        self.reconcile(world);
        let player = self.player_input;
        for brain in self.brains.iter_mut() {
            brain.tick(world);
        }
        for car in self.cars.iter_mut() {
            car.tick(world, &player);
        }
        for character in self.characters.iter_mut() {
            character.tick(world, &player);
        }
        for plane in self.planes.iter_mut() {
            plane.tick(world, &player);
        }
    }

    /// Phase 2: observe the settled world — runs AFTER the sim step.
    pub fn post_step(&mut self, world: &mut GameWorld) {
        for character in self.characters.iter_mut() {
            character.post_tick(world);
        }
        self.race.post_tick(world);
    }

    pub fn car_mut(&mut self, entity: u64) -> Option<&mut Car> {
        self.cars.iter_mut().find(|c| c.entity == entity)
    }

    pub fn character_mut(&mut self, entity: u64) -> Option<&mut Character> {
        self.characters.iter_mut().find(|c| c.entity == entity)
    }

    pub fn plane_mut(&mut self, entity: u64) -> Option<&mut Plane> {
        self.planes.iter_mut().find(|p| p.entity == entity)
    }

    /// `game.drive` — set control intent on whichever driveable owns this id.
    pub fn drive(&mut self, entity: u64, apply: impl Fn(&mut DriveInput)) -> bool {
        if let Some(car) = self.car_mut(entity) {
            apply(&mut car.input);
            car.control = ControlSource::Script;
            return true;
        }
        if let Some(plane) = self.plane_mut(entity) {
            apply(&mut plane.input);
            plane.control = ControlSource::Script;
            return true;
        }
        if let Some(character) = self.character_mut(entity) {
            apply(&mut character.input);
            character.control = ControlSource::Script;
            return true;
        }
        false
    }

    /// Content hash for determinism tests (game.md: a scenario run twice must
    /// hash identically). Covers the block state the sim doesn't already own.
    pub fn hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x100_0000_01b3);
        };
        for car in &self.cars {
            mix(car.entity);
            mix(car.speed.to_bits() as u64);
            for wheel in &car.wheels {
                mix(wheel.compression.to_bits() as u64);
                mix(wheel.grounded as u64);
            }
        }
        for character in &self.characters {
            mix(character.entity);
            mix(character.pose.walk_blend.to_bits() as u64);
            mix(character.pose.airborne as u64);
        }
        for plane in &self.planes {
            mix(plane.entity);
            mix(plane.throttle.to_bits() as u64);
        }
        for brain in &self.brains {
            mix(brain.entity);
            mix(brain.timer.to_bits() as u64);
            mix(brain.waypoint as u64);
        }
        mix(self.race.hash());
        h
    }
}
