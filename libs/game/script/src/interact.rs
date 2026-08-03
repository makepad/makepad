//! The primary activity button — one context-sensitive interact.
//!
//! A generated game should never invent three key bindings and three proximity
//! checks because it happens to contain a car, a house and a chest. There is
//! ONE button (`DriveInput::use_pressed`), and what it means comes from what
//! the player is standing in front of. Adding a new kind of interactable adds
//! a row here, not a branch in anybody's input path.
//!
//! # Derivation beats declaration
//!
//! The common cases need NO script call at all, because the engine already
//! knows what these things are:
//!
//! - a car block  → "Drive"   (and while seated → "Get out")
//! - a POI tagged `door` that `leads_to` an interior → "Enter"
//!
//! [`InteractSet`] exists only for the cases the engine *cannot* infer — a
//! chest, a switch, a lever — which script declares with `game.interactable`.
//! An AI writing a game with a car and a house has to remember nothing; the
//! same reasoning that made the racing fixture 72 lines.
//!
//! # Choosing
//!
//! Nearest is not enough: the nearest car can be behind you while you are
//! looking at a door, and taking the action behind you is the single most
//! annoying failure this mechanic has. [`choose`] keeps only candidates inside
//! a forward cone and picks the closest of those.
//!
//! # Dispatch
//!
//! [`choose`] answers "what would the button do right now" and nothing more —
//! the same answer drives BOTH the affordance prompt and the button press, so
//! the prompt can never disagree with what actually happens. The caller turns
//! the chosen [`Interact`] into an [`InteractAction`] and performs it; this
//! module never writes to the world, keeping the "blocks only ever write vel"
//! discipline that interiors already rely on (see `blocks::npc::DoorUse`).

use makepad_game_blocks::{Blocks, Seat};
use makepad_game_sim::*;
use makepad_widgets::makepad_math::*;
use makepad_widgets::LiveId;

/// How far a thing can be interacted with when it doesn't say otherwise.
pub const DEFAULT_RANGE: f32 = 3.5;

/// Half-angle of the forward cone, as a cosine. ~60° either side: wide enough
/// that you don't have to aim, narrow enough that you never take the action
/// behind you.
const FACING_COS: f32 = 0.5;

/// What the button would do. The prompt is part of the kind's identity rather
/// than a separate field, because a prompt that can drift from the action is a
/// prompt that will.
#[derive(Clone, Debug, PartialEq)]
pub enum InteractKind {
    /// Get into the vehicle you're standing next to.
    Drive,
    /// Leave the vehicle you're in. Always available while seated.
    Exit,
    /// Step through a door into the interior it leads to.
    Enter(Vec3f),
    /// Script declared this one; the id is handed back so the game can decide.
    Custom(LiveId),
}

impl InteractKind {
    /// Default verb shown to the player. `game.interactable({prompt})`
    /// overrides it for custom kinds.
    pub fn default_prompt(&self) -> &'static str {
        match self {
            InteractKind::Drive => "Drive",
            InteractKind::Exit => "Get out",
            InteractKind::Enter(_) => "Enter",
            InteractKind::Custom(_) => "Use",
        }
    }
}

/// One thing the player could interact with, once chosen.
#[derive(Clone, Debug, PartialEq)]
pub struct Interact {
    /// The entity to act on. 0 for a bare point (a door POI with no prop).
    pub entity: u64,
    pub kind: InteractKind,
    /// What to show the player. Never empty.
    pub prompt: String,
    /// Metres from the player, for the caller's own highlight falloff.
    pub distance: f32,
}

/// Something script declared interactable that the engine could not infer.
#[derive(Clone, Debug)]
pub struct Declared {
    pub entity: u64,
    pub kind: LiveId,
    pub prompt: String,
    pub range: f32,
}

/// Script-declared interactables. Derived ones are never stored — they are
/// read from the blocks that already describe them, so removing a car removes
/// its interaction with no bookkeeping.
#[derive(Clone, Debug, Default)]
pub struct InteractSet {
    pub declared: Vec<Declared>,
}

impl InteractSet {
    pub fn clear(&mut self) {
        self.declared.clear();
    }

    /// Declare or replace one. Re-declaring the same entity updates it rather
    /// than stacking, so a re-eval that runs the same line twice is idempotent.
    pub fn declare(&mut self, item: Declared) {
        match self.declared.iter_mut().find(|d| d.entity == item.entity) {
            Some(slot) => *slot = item,
            None => self.declared.push(item),
        }
    }

    pub fn remove(&mut self, entity: u64) {
        self.declared.retain(|d| d.entity != entity);
    }

    /// Drop declarations whose entity is gone, mirroring `Blocks::reconcile`.
    pub fn reconcile(&mut self, world: &GameWorld) {
        self.declared.retain(|d| world.entity(d.entity).is_some());
    }
}

/// What the caller should do about a chosen interact.
///
/// Returned rather than performed so this module never writes to the world:
/// the mount case belongs to `PlayerRig`, the teleport belongs to the host
/// (an interior is a position write, exactly like `blocks::npc::DoorUse`), and
/// the script case belongs to the callback table.
#[derive(Clone, Debug, PartialEq)]
pub enum InteractAction {
    /// Let the player rig run its mount state machine — it owns the seat.
    ToggleMount,
    /// Move `entity` to `to`. Interiors are pockets elsewhere in the same
    /// world, so going inside is a teleport.
    Teleport { entity: u64, to: Vec3f },
    /// Hand it to script; the game decides what a chest does.
    Script { entity: u64, kind: LiveId },
}

impl Interact {
    /// The action this interact implies.
    pub fn action(&self, player: u64) -> InteractAction {
        match self.kind {
            InteractKind::Drive | InteractKind::Exit => InteractAction::ToggleMount,
            InteractKind::Enter(to) => InteractAction::Teleport { entity: player, to },
            InteractKind::Custom(kind) => InteractAction::Script {
                entity: self.entity,
                kind,
            },
        }
    }
}

/// Squared planar distance. Interaction is a ground-plane question — a door
/// one storey up is not "near" even though it is close in 3D.
fn planar_dist_sq(a: Vec3f, b: Vec3f) -> f32 {
    let (dx, dz) = (a.x - b.x, a.z - b.z);
    dx * dx + dz * dz
}

/// Is `target` inside the forward cone from `origin`?
///
/// A candidate you are standing on top of always passes: at zero distance the
/// direction is meaningless, and failing the test there would make an
/// interactable un-interactable exactly when you are closest to it.
fn in_front(origin: Vec3f, facing: Vec3f, target: Vec3f) -> bool {
    let (dx, dz) = (target.x - origin.x, target.z - origin.z);
    let len_sq = dx * dx + dz * dz;
    if len_sq < 0.0625 {
        return true;
    }
    let inv = 1.0 / len_sq.sqrt();
    let flen_sq = facing.x * facing.x + facing.z * facing.z;
    if flen_sq < 1e-6 {
        return true;
    }
    let finv = 1.0 / flen_sq.sqrt();
    (dx * inv) * (facing.x * finv) + (dz * inv) * (facing.z * finv) >= FACING_COS
}

/// **What would the primary activity button do right now?**
///
/// `origin` is the player's position, `facing` the direction they are LOOKING
/// (the camera's forward, not the character's heading — the player aims with
/// the camera, and using the body's heading makes the prompt flicker while the
/// character turns to catch up).
///
/// Returns `None` when nothing is in range, which is a silent no-op, not an
/// error: pressing the button in an empty field should do nothing at all.
///
/// The result drives both the prompt and the press. Call it every frame for
/// the prompt and reuse the answer on the press, so the two cannot disagree.
pub fn choose(
    world: &GameWorld,
    blocks: &Blocks,
    set: &InteractSet,
    seat: Seat,
    origin: Vec3f,
    facing: Vec3f,
) -> Option<Interact> {
    // Seated beats everything: you cannot walk up to something while driving,
    // and "get out" must never be shadowed by a door you happen to be parked
    // beside. No range or cone test — the car is under you.
    if let Seat::Driving(car) = seat {
        return Some(Interact {
            entity: car,
            kind: InteractKind::Exit,
            prompt: InteractKind::Exit.default_prompt().to_string(),
            distance: 0.0,
        });
    }

    let mut best: Option<(f32, Interact)> = None;
    // Deterministic: candidates are considered in a fixed order and ties are
    // broken by first-seen, so two runs of the same tape choose identically.
    let mut consider = |pos: Vec3f, range: f32, entity: u64, kind: InteractKind, prompt: String| {
        let d_sq = planar_dist_sq(origin, pos);
        if d_sq > range * range || !in_front(origin, facing, pos) {
            return;
        }
        if best.as_ref().is_some_and(|(best_sq, _)| d_sq >= *best_sq) {
            return;
        }
        best = Some((
            d_sq,
            Interact {
                entity,
                kind,
                prompt,
                distance: d_sq.sqrt(),
            },
        ));
    };

    // Derived: any car is drivable. Nothing declares this.
    for car in &blocks.cars {
        if let Some(e) = world.entity(car.entity) {
            consider(
                e.pos,
                DEFAULT_RANGE,
                car.entity,
                InteractKind::Drive,
                InteractKind::Drive.default_prompt().to_string(),
            );
        }
    }

    // Derived: a door that leads somewhere is an entrance. A door POI with no
    // interior is scenery for the NPCs and is deliberately skipped.
    for poi in &blocks.pois.list {
        if poi.tag != "door" {
            continue;
        }
        let Some(to) = poi.leads_to else { continue };
        consider(
            poi.pos,
            DEFAULT_RANGE,
            poi.entity,
            InteractKind::Enter(to),
            InteractKind::Enter(to).default_prompt().to_string(),
        );
    }

    // Declared: everything the engine cannot infer.
    for d in &set.declared {
        if let Some(e) = world.entity(d.entity) {
            consider(
                e.pos,
                d.range,
                d.entity,
                InteractKind::Custom(d.kind),
                d.prompt.clone(),
            );
        }
    }

    best.map(|(_, interact)| interact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_game_blocks::{Car, CarConfig, ControlSource, Poi};

    fn world_with(entities: &[(u64, Vec3f)]) -> GameWorld {
        let mut world = GameWorld::default();
        for (id, pos) in entities {
            world.entities.push(Entity {
                id: *id,
                pos: *pos,
                kind: BodyKind::Rigid,
                ..Default::default()
            });
        }
        world.entities.sort_by_key(|e| e.id);
        world
    }

    fn with_car(id: u64, pos: Vec3f) -> (GameWorld, Blocks) {
        let world = world_with(&[(id, pos)]);
        let mut blocks = Blocks::new();
        blocks.cars.push(Car::new(id, CarConfig::default(), ControlSource::Script));
        (world, blocks)
    }

    /// Forward is -Z, so a player at the origin looking forward sees -Z.
    const FWD: Vec3f = Vec3f {
        x: 0.0,
        y: 0.0,
        z: -1.0,
    };
    const ORIGIN: Vec3f = Vec3f {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    #[test]
    fn a_car_needs_no_declaration_to_be_drivable() {
        let (world, blocks) = with_car(7, vec3f(0.0, 0.0, -2.0));
        let got = choose(
            &world,
            &blocks,
            &InteractSet::default(),
            Seat::OnFoot,
            ORIGIN,
            FWD,
        )
        .expect("a car two metres ahead is drivable");
        assert_eq!(got.entity, 7);
        assert_eq!(got.kind, InteractKind::Drive);
        assert_eq!(got.prompt, "Drive");
    }

    #[test]
    fn nothing_in_range_is_a_silent_none() {
        let (world, blocks) = with_car(7, vec3f(0.0, 0.0, -40.0));
        assert!(choose(
            &world,
            &blocks,
            &InteractSet::default(),
            Seat::OnFoot,
            ORIGIN,
            FWD
        )
        .is_none());
    }

    /// The failure this mechanic is most annoying without: acting on the thing
    /// behind you.
    #[test]
    fn a_car_behind_you_is_not_chosen() {
        let (world, blocks) = with_car(7, vec3f(0.0, 0.0, 2.0));
        assert!(
            choose(
                &world,
                &blocks,
                &InteractSet::default(),
                Seat::OnFoot,
                ORIGIN,
                FWD
            )
            .is_none(),
            "a car directly behind the player must not be offered"
        );
    }

    /// Nearest-in-front, not nearest: the close one is behind, so the far one
    /// in front wins. A plain nearest search fails this.
    #[test]
    fn facing_beats_proximity() {
        let mut world = world_with(&[(1, vec3f(0.0, 0.0, 1.0)), (2, vec3f(0.0, 0.0, -3.0))]);
        world.entities.sort_by_key(|e| e.id);
        let mut blocks = Blocks::new();
        blocks.cars.push(Car::new(1, CarConfig::default(), ControlSource::Script));
        blocks.cars.push(Car::new(2, CarConfig::default(), ControlSource::Script));
        let got = choose(
            &world,
            &blocks,
            &InteractSet::default(),
            Seat::OnFoot,
            ORIGIN,
            FWD,
        )
        .expect("the far car is still in front");
        assert_eq!(got.entity, 2, "must pick the one being looked at");
    }

    #[test]
    fn seated_always_offers_exit_even_beside_a_door() {
        let mut world = world_with(&[(7, vec3f(0.0, 0.0, -2.0))]);
        world.entities.sort_by_key(|e| e.id);
        let mut blocks = Blocks::new();
        blocks.cars.push(Car::new(7, CarConfig::default(), ControlSource::Script));
        blocks
            .pois
            .list
            .push(Poi::new(ORIGIN, "door").with_interior(vec3f(50.0, 0.0, 50.0)));
        let got = choose(
            &world,
            &blocks,
            &InteractSet::default(),
            Seat::Driving(7),
            ORIGIN,
            FWD,
        )
        .expect("seated always has an exit");
        assert_eq!(got.kind, InteractKind::Exit);
        assert_eq!(got.action(99), InteractAction::ToggleMount);
    }

    #[test]
    fn a_door_with_an_interior_is_an_entrance_and_teleports_the_player() {
        let world = world_with(&[]);
        let mut blocks = Blocks::new();
        let inside = vec3f(50.0, 0.0, 50.0);
        blocks
            .pois
            .list
            .push(Poi::new(vec3f(0.0, 0.0, -2.0), "door").with_interior(inside));
        let got = choose(
            &world,
            &blocks,
            &InteractSet::default(),
            Seat::OnFoot,
            ORIGIN,
            FWD,
        )
        .expect("a door ahead is an entrance");
        assert_eq!(got.kind, InteractKind::Enter(inside));
        assert_eq!(
            got.action(42),
            InteractAction::Teleport {
                entity: 42,
                to: inside
            },
            "the PLAYER moves, not the door"
        );
    }

    /// A door POI with no interior is scenery the NPCs use, not an entrance.
    #[test]
    fn a_door_that_leads_nowhere_is_not_offered() {
        let world = world_with(&[]);
        let mut blocks = Blocks::new();
        blocks.pois.list.push(Poi::new(vec3f(0.0, 0.0, -2.0), "door"));
        assert!(choose(
            &world,
            &blocks,
            &InteractSet::default(),
            Seat::OnFoot,
            ORIGIN,
            FWD
        )
        .is_none());
    }

    #[test]
    fn declared_interactables_carry_their_own_prompt_and_range() {
        let world = world_with(&[(5, vec3f(0.0, 0.0, -8.0))]);
        let mut set = InteractSet::default();
        set.declare(Declared {
            entity: 5,
            kind: LiveId::from_str("chest"),
            prompt: "Open".to_string(),
            range: 10.0,
        });
        let got = choose(
            &world,
            &Blocks::new(),
            &set,
            Seat::OnFoot,
            ORIGIN,
            FWD,
        )
        .expect("its own 10m range reaches 8m");
        assert_eq!(got.prompt, "Open");
        assert_eq!(
            got.action(1),
            InteractAction::Script {
                entity: 5,
                kind: LiveId::from_str("chest")
            }
        );
        // …and the default range would NOT have reached it.
        assert!(DEFAULT_RANGE < 8.0);
    }

    #[test]
    fn redeclaring_updates_rather_than_stacks() {
        let mut set = InteractSet::default();
        let mut item = Declared {
            entity: 5,
            kind: LiveId::from_str("chest"),
            prompt: "Open".to_string(),
            range: 4.0,
        };
        set.declare(item.clone());
        item.prompt = "Loot".to_string();
        set.declare(item);
        assert_eq!(set.declared.len(), 1, "a re-eval must be idempotent");
        assert_eq!(set.declared[0].prompt, "Loot");
    }

    #[test]
    fn standing_on_top_of_something_still_offers_it() {
        // Zero distance has no meaningful direction; the cone must not reject.
        let (world, blocks) = with_car(7, ORIGIN);
        assert!(choose(
            &world,
            &blocks,
            &InteractSet::default(),
            Seat::OnFoot,
            ORIGIN,
            FWD
        )
        .is_some());
    }

    #[test]
    fn reconcile_drops_declarations_whose_entity_vanished() {
        let world = world_with(&[]);
        let mut set = InteractSet::default();
        set.declare(Declared {
            entity: 5,
            kind: LiveId::from_str("chest"),
            prompt: "Open".to_string(),
            range: 4.0,
        });
        set.reconcile(&world);
        assert!(set.declared.is_empty());
    }
}
