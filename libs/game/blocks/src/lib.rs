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

pub mod audio_emit;
pub mod brain;
pub mod car;
pub mod character;
pub mod controller;
pub mod npc;
pub mod plane;
pub mod race;

pub use brain::{Brain, BrainKind};
pub use car::{Car, CarConfig};
pub use character::{Character, CharacterConfig, CharacterPose};
pub use controller::{CameraConfig, FollowCamera, Mount, Seat, movement_intent};
pub use npc::{Activity, DoorUse, Npc, NpcConfig, Personality, Poi, PoiSet, DAY_SECONDS};
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
    /// Mouse/stick look delta THIS TICK, radians. Yaw then pitch. The camera
    /// consumes it; nothing in the sim reads it, because where a player is
    /// looking is presentation (Local tier) and must never reach the world.
    pub look_dx: f32,
    pub look_dy: f32,
    /// Sprint held. Separate from throttle so a keyboard and a stick produce
    /// the same intent.
    pub run: bool,
    /// Mount/dismount pressed this tick (get in or out of the nearest vehicle).
    pub use_pressed: bool,
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
    pub npcs: Vec<Npc>,
    /// Destinations NPCs choose between. Lives beside them so eval rollback
    /// restores villagers and the places they were heading for together.
    pub pois: PoiSet,
    pub race: RaceKit,
    /// This device's input, refreshed by the host before `pre_step`.
    pub player_input: DriveInput,
    /// Per-player intent (M2). The host fills one entry per connected player
    /// and bot each tick; a block driven by `ControlSource::Player` reads the
    /// entry for its `owner`. An empty map means single-player, where every
    /// owner resolves to `player_input` — so the local path is unchanged.
    pub player_inputs: std::collections::HashMap<makepad_game_sim::PlayerId, DriveInput>,
    /// Sounds produced this tick. The host drains these after `post_step` and
    /// hands them to its mixer; blocks never touch an audio device.
    pub audio: audio_emit::AudioEmitter,
    /// Doors NPCs want to go through this tick. The host drains these and
    /// performs the position write — an interior is a pocket elsewhere in the
    /// same world, so entering one is a teleport. Blocks never reposition an
    /// entity themselves; see [`npc::DoorUse`].
    pub door_uses: Vec<npc::DoorUse>,
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
            player_inputs: std::mem::take(&mut self.player_inputs),
            ..Default::default()
        };
    }

    pub fn is_empty(&self) -> bool {
        self.cars.is_empty()
            && self.characters.is_empty()
            && self.planes.is_empty()
            && self.brains.is_empty()
            && self.npcs.is_empty()
            && self.race.is_empty()
    }

    /// Drop block state whose entity is gone (removed, expired, rolled back).
    /// Public because a network client rebuilds its entity list wholesale from
    /// replicated state and must drop block state for anything that vanished.
    pub fn reconcile(&mut self, world: &GameWorld) {
        let alive = |id: u64| world.entity(id).is_some();
        self.cars.retain(|c| alive(c.entity));
        self.characters.retain(|c| alive(c.entity));
        self.planes.retain(|p| alive(p.entity));
        self.brains.retain(|b| alive(b.entity));
        self.npcs.retain(|n| alive(n.entity));
        self.pois.reconcile(world);
        self.race.reconcile(world);
    }

    /// Phase 1: turn intent into motion — runs BEFORE the sim step.
    pub fn pre_step(&mut self, world: &mut GameWorld) {
        self.reconcile(world);
        let fallback = self.player_input;
        let inputs = &self.player_inputs;
        // One player's intent, whichever source filled it. Single-player leaves
        // the map empty and every block reads `player_input`, exactly as before.
        let intent = |owner: makepad_game_sim::PlayerId| -> DriveInput {
            inputs.get(&owner).copied().unwrap_or(fallback)
        };
        for brain in self.brains.iter_mut() {
            brain.tick(world);
        }
        // NPCs share one POI set, so they must be ticked in a fixed order for
        // slot claims (who gets the last seat on the bench) to be deterministic.
        for npc in self.npcs.iter_mut() {
            npc.tick(world, &mut self.pois, &mut self.door_uses);
        }
        for car in self.cars.iter_mut() {
            let input = intent(car.owner);
            car.tick(world, &input);
        }
        for character in self.characters.iter_mut() {
            let input = intent(character.owner);
            character.tick(world, &input);
        }
        for plane in self.planes.iter_mut() {
            let input = intent(plane.owner);
            plane.tick(world, &input);
        }
    }

    /// Phase 2: observe the settled world — runs AFTER the sim step.
    pub fn post_step(&mut self, world: &mut GameWorld) {
        for character in self.characters.iter_mut() {
            character.post_tick(world);
            character.emit_audio(world, &mut self.audio);
        }
        for car in self.cars.iter_mut() {
            car.emit_audio(world, &mut self.audio);
        }
        for plane in self.planes.iter_mut() {
            plane.emit_audio(world, &mut self.audio);
        }
        self.race.post_tick(world);
        self.race.emit_audio(&mut self.audio);
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
        for npc in &self.npcs {
            mix(npc.entity);
            // The activity discriminant plus its goal is what a divergence
            // would show up in first — two runs that agree on position but
            // disagree on intent are about to diverge on position too.
            mix(npc.activity.name().len() as u64);
            if let Activity::Travel { goal, .. } = npc.activity {
                mix(goal.x.to_bits() as u64);
                mix(goal.z.to_bits() as u64);
            }
        }
        for poi in &self.pois.list {
            mix(poi.taken as u64);
        }
        mix(self.race.hash());
        h
    }
}
