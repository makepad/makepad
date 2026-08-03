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
pub use controller::{
    movement_intent, CameraConfig, FollowCamera, Mount, PlayerRig, RawInput, Seat,
};
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
    /// Sprint, 0..1. Separate from throttle so a keyboard and a stick produce
    /// the same intent.
    ///
    /// Analog rather than a bool so stick deflection gives a walk→run
    /// continuum. A keyboard fills 1.0 for shift and 0.0 otherwise and is
    /// unchanged; a pad fills it from deflection and gets the richer input,
    /// which is the point of holding a pad.
    pub run: f32,
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
    /// Per-player camera + seat. One entry per player who controls a character
    /// directly; a game that only drives blocks from script has none.
    ///
    /// Lives here rather than host-side because the rig decides *which entity*
    /// a player is driving, and `pre_step` needs that to give a player's input
    /// to exactly one block. A host-side rig would leave this file guessing.
    pub player_rigs: std::collections::HashMap<makepad_game_sim::PlayerId, controller::PlayerRig>,
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
            // Rigs survive for the same reason, and a stronger one: where the
            // camera is pointing and whether you are in the car are the
            // player's position in the game, not the game's content. Dropping
            // them would snap the view and eject a driver mid-corner on every
            // edit — the jarring-reload problem, in the most visible place it
            // could possibly happen. `reconcile` repairs any rig whose entity
            // the re-eval did not recreate.
            player_rigs: std::mem::take(&mut self.player_rigs),
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
        // A rig whose character is gone has nothing left to control. The
        // seat-vs-vanished-car case needs a mutable world, so it is repaired in
        // `tick_player_rigs` instead.
        self.player_rigs.retain(|_, r| alive(r.mount.character));
    }

    /// Advance every player's camera and seat, and turn their device input into
    /// block intent. The host calls this once per tick BEFORE [`Self::pre_step`].
    ///
    /// Ordering is not incidental. This must also run before
    /// `GameWorld::sync_local_player`, because it writes `world.cam_yaw` and
    /// that sync copies the yaw out into player slot 0 for replication — run it
    /// after and every remote observer sees camera-relative movement lag by a
    /// frame, which looks like latency and is not.
    pub fn tick_player_rigs(
        &mut self,
        world: &mut GameWorld,
        raw: &std::collections::HashMap<makepad_game_sim::PlayerId, controller::RawInput>,
    ) {
        // Sorted, because rig ticks can mutate shared world state (a mount
        // hides a character; a dismount claims a spot) and a HashMap's order
        // is not stable across runs. Determinism is not optional here — this
        // feeds the replicated input stream.
        let mut players: Vec<_> = self.player_rigs.keys().copied().collect();
        players.sort_by_key(|p| p.0);
        // A player whose car vanished — despawned, rolled back, not recreated
        // by a re-eval — must be put back on their feet before anything else
        // runs. Left alone they keep "driving" a ghost: no camera subject, no
        // block receiving their input, and no button that gets them out, since
        // the get-out reads the car it can no longer find. Being stuck holding
        // dead controls is worse than any reset.
        for player in players.iter() {
            let Some(rig) = self.player_rigs.get(player) else {
                continue;
            };
            if let controller::Seat::Driving(car) = rig.mount.seat {
                if world.entity(car).is_none() {
                    if let Some(rig) = self.player_rigs.get_mut(player) {
                        let mut r = *rig;
                        r.eject(world);
                        self.player_rigs.insert(*player, r);
                    }
                }
            }
        }
        for player in players {
            let Some(mut rig) = self.player_rigs.remove(&player) else {
                continue;
            };
            let input = rig.tick(world, raw.get(&player).copied().unwrap_or_default());
            self.player_inputs.insert(player, input);
            self.player_rigs.insert(player, rig);
        }
    }

    /// Phase 1: turn intent into motion — runs BEFORE the sim step.
    pub fn pre_step(&mut self, world: &mut GameWorld) {
        self.reconcile(world);
        let fallback = self.player_input;
        let inputs = &self.player_inputs;
        let rigs = &self.player_rigs;
        // One player's intent, whichever source filled it. Single-player leaves
        // the map empty and every block reads `player_input`, exactly as before.
        //
        // The `entity` argument is the modality gate, and it is the whole
        // reason this is one closure rather than three lookups. A player who
        // owns both a character and a car matches BOTH as an owner, so without
        // it, climbing into a car leaves your parked character being walked by
        // the same stick that is steering you — invisible until you get out
        // somewhere you never chose to be. A player who has a rig drives
        // exactly what that rig is sitting in; everything else they own goes
        // inert. Players with no rig are unaffected, which keeps script-driven
        // and AI-owned blocks exactly as they were.
        let intent = |owner: makepad_game_sim::PlayerId, entity: u64| -> DriveInput {
            if rigs.get(&owner).is_some_and(|r| r.mount.subject() != entity) {
                return DriveInput::default();
            }
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
            let input = intent(car.owner, car.entity);
            car.tick(world, &input);
        }
        for character in self.characters.iter_mut() {
            let input = intent(character.owner, character.entity);
            character.tick(world, &input);
        }
        for plane in self.planes.iter_mut() {
            let input = intent(plane.owner, plane.entity);
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
