//! GameWorld — the complete simulation state. Moved verbatim from gamemaker's
//! game_view.rs (M0 stage A extraction). The only semantic transformations:
//! script callbacks are opaque [`CallbackSlot`]s (the host owns the
//! slot→closure table), and reset_content no longer touches audio (the host
//! stops the synth alongside its calls).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use makepad_live_id::*;
use makepad_math::*;

use crate::entity::*;
use crate::terrain::*;
use crate::CallbackSlot;

/// Everything the script API reads/writes. Shared (Rc<RefCell>) between the
/// widget and the native `game` handle registered into the isolate, so script
/// calls mutate it synchronously — no async widget trampoline, deterministic
/// ordering, and world-building during eval completes before eval returns.
///
/// Clone is a full snapshot: plain data plus the box3d world via its
/// bit-exact snapshot round trip (see dynamics.rs) — the basis for the
/// rollback ring / replay / join-state dump (game.md).
#[derive(Default, Clone)]
pub struct GameWorld {
    pub entities: Vec<Entity>,
    /// The player roster (M2). Slot 0 is always this device, and its input
    /// still lives in the `held`/`pressed`/`pad`/`cam_yaw` fields below — a
    /// single-player world therefore evaluates exactly the expressions it did
    /// before players existed, which is what keeps input tapes byte-identical.
    /// Remote and bot players carry their own input in the roster.
    pub players: crate::player::Players,
    /// box3d dynamics layer (M1a): mirrored statics/kinematics + rigid
    /// bodies. Reconciled against `entities` each tick — never mutated by
    /// spawn/remove paths directly.
    pub dynamics: crate::dynamics::RigidDynamics,
    pub next_id: u64,
    pub gravity: f32,
    pub on_tick: Option<CallbackSlot>,
    pub on_touch: Option<CallbackSlot>,
    /// Fired when a player joins or leaves the room (M2). The session layer
    /// raises the events; the host resolves the slot and calls the closure.
    pub on_join: Option<CallbackSlot>,
    pub on_leave: Option<CallbackSlot>,
    pub timers: Vec<GameTimer>,
    /// HUD text, keyed by slot name ("center"/"top"/"hint" + any the script
    /// invents), each pinned to an anchor. Replace-on-set; empty text removes.
    pub hud_slots: Vec<(String, HudSlot)>,
    /// HUD gauges, keyed by name.
    pub hud_bars: Vec<HudBar>,
    pub crosshair: bool,
    /// Camera requests from script.
    pub cam_target: Vec3f,
    pub cam_distance: f32,
    pub cam_follow: u64,
    pub cam_side: bool,
    /// Third-person rig: pivot entity (0 = off), pivot height, boom length.
    pub cam_third: u64,
    pub cam_height: f32,
    pub cam_boom: f32,
    /// Chase rig (camera({chase: id})): renders exactly like third_person
    /// (setting chase also sets cam_third) but additionally eases the orbit
    /// yaw to sit BEHIND the target every tick. Authority: script write >
    /// mouse this-tick > rig easing. 0 = off (mouse owns the orbit again).
    pub cam_chase: u64,
    /// Ease time-constant in seconds (smaller = tighter).
    pub cam_lag: f32,
    /// Seconds of mouse authority after a drag ends before the rig resumes.
    pub cam_recenter: f32,
    /// Scales the ease rate up with target speed: rate = (1/lag)·(1+speed·this).
    pub cam_speed_tighten: f32,
    /// One-shot angle sets from script, consumed by the widget next tick —
    /// the writable half of the chase-cam API (the mouse owns the angles
    /// otherwise, so writes go through the same authoritative widget state).
    pub cam_pitch_request: Option<f32>,
    pub cam_yaw_request: Option<f32>,
    /// Widget state mirrored into the world each tick, so scripts can read
    /// the full camera pose and hand control back gracefully after drags.
    pub cam_pitch: f32,
    pub cam_dragging: bool,
    /// Mouse orbit delta accumulated since the last tick (0 while not
    /// dragging, always 0 under tapes).
    pub look_dx: f64,
    pub look_dy: f64,
    /// Vertical field of view (degrees). Racing games widen it with speed.
    pub cam_fov: f32,
    /// Decaying random camera offset amplitude (game.cam_shake).
    pub cam_shake: f32,
    /// game.save/game.load persistence. NOT cleared on eval — surviving
    /// edits is the whole point (best laps). Loaded from save_path at
    /// project switch; flushed by the widget at most once a second.
    pub save_data: HashMap<String, SaveVal>,
    pub save_path: Option<PathBuf>,
    pub save_dirty: bool,
    /// Immediate-mode cables, cleared at the top of every tick.
    pub beams: Vec<Beam>,
    /// Input state, written by the ActionMap / tape, read by script.
    pub held: HashSet<LiveId>,
    pub pressed: HashSet<LiveId>,
    /// Gamepad state, merged with the keyboard at read time (never into
    /// `held`, so a pad release can't cancel a held key). Stick is analog.
    pub pad: PadState,
    /// Decoration: visual-only child boxes and billboard nametags.
    pub parts: Vec<Part>,
    pub labels: Vec<LabelDef>,
    /// Smooth heightfield ground (game.terrain smooth mode).
    pub terrain: Option<Terrain>,
    /// Sky/fog, enabled by game.sky().
    pub sky: Option<SkyConfig>,
    /// What game.sun() asked for; the renderer resolves it (see SunConfig).
    /// Presentation only — the step never reads it.
    pub sun: SunConfig,
    /// Orbit-camera yaw, mirrored from the widget each tick so scripts can do
    /// camera-relative movement ("run where the camera looks").
    pub cam_yaw: f32,
    /// AUTHORITATIVE orbit rig (M0r camera-ownership consolidation): the DSL
    /// and the chase rig read/write these; the host feeds device deltas in.
    /// Deliberately NOT touched by reset_content — the camera pose survives
    /// re-evals exactly like it did when the widget owned it. Seeded by
    /// [`GameWorld::new`]; a plain Default world starts at 0/0.
    pub orbit_yaw: f32,
    pub orbit_pitch: f32,
    /// Chase-rig mouse authority: seconds left before easing resumes after a
    /// drag (refreshed while dragging, counts down after release).
    pub chase_hold: f32,
    /// Seeded per eval, so wander AI is repeatable under input tapes — an
    /// improvement over the Godot corpus, which called randomize().
    pub rng: u64,
    pub tick: u64,
    pub time: f64,
    pub log_pending: Vec<String>,
    /// PERF: bumped whenever anything a STATIC entity contributes to the
    /// screen changes (spawn/remove/restyle/sky). The renderer caches packed
    /// instance slabs for static content keyed by this — bump it or your
    /// static edit won't show.
    pub render_rev: u64,
}

impl GameWorld {
    /// A world with the canonical starting camera (the values the gamemaker
    /// widget historically seeded: yaw 0.6, pitch -0.35).
    pub fn new() -> Self {
        let mut world = Self::default();
        world.orbit_yaw = 0.6;
        world.orbit_pitch = -0.35;
        // xorshift64* is a fixed point at zero, so an unseeded world would
        // hand out rand() == 0 forever. reset_content seeds it on every eval,
        // which hid this from gamemaker; a world built straight through the
        // sim API (arcade, tests, blocks) never calls reset_content.
        world.rng = 0x9E37_79B9_7F4A_7C15;
        world
    }

    /// Keyboard OR gamepad. The pad's stick maps onto the four directions at
    /// half deflection so `held("left")` works the same on both.
    pub fn action_held(&self, action: LiveId) -> bool {
        if self.held.contains(&action) {
            return true;
        }
        match action {
            x if x == live_id!(jump) => self.pad.jump,
            x if x == live_id!(shoot) => self.pad.shoot,
            x if x == live_id!(grab) => self.pad.grab,
            x if x == live_id!(reset) => self.pad.reset,
            x if x == live_id!(left) => self.pad.axis_x < -0.5,
            x if x == live_id!(right) => self.pad.axis_x > 0.5,
            x if x == live_id!(up) => self.pad.axis_z < -0.5,
            x if x == live_id!(down) => self.pad.axis_z > 0.5,
            _ => false,
        }
    }

    pub fn action_pressed(&self, action: LiveId) -> bool {
        if self.pressed.contains(&action) {
            return true;
        }
        match action {
            x if x == live_id!(jump) => self.pad.jump_pressed,
            x if x == live_id!(shoot) => self.pad.shoot_pressed,
            x if x == live_id!(grab) => self.pad.grab_pressed,
            x if x == live_id!(reset) => self.pad.reset_pressed,
            _ => false,
        }
    }

    // ── players (M2) ────────────────────────────────────────────────────

    /// Mirror this device's input into player slot 0. Called once per tick by
    /// the host before script/blocks run, so `game.player_input(0)` and the
    /// on_tick input object agree. The device fields stay authoritative — this
    /// copies *out* of them, never into them.
    pub fn sync_local_player(&mut self) {
        let (held, pressed, pad) = (self.held.clone(), self.pressed.clone(), self.pad);
        let (yaw, dx, dy) = (self.cam_yaw, self.look_dx, self.look_dy);
        let local = self.players.local_mut();
        local.input.held = held;
        local.input.pressed = pressed;
        local.input.pad = pad;
        local.input.cam_yaw = yaw;
        local.input.look_dx = dx;
        local.input.look_dy = dy;
    }

    /// Is this action held for a specific player? Player 0 reads the device
    /// fields directly (identical to [`Self::action_held`]); everyone else
    /// reads their replicated input.
    pub fn action_held_for(&self, player: crate::player::PlayerId, action: LiveId) -> bool {
        if player.is_local_slot() {
            return self.action_held(action);
        }
        self.players
            .get(player)
            .map_or(false, |p| p.input.held(action))
    }

    pub fn action_pressed_for(&self, player: crate::player::PlayerId, action: LiveId) -> bool {
        if player.is_local_slot() {
            return self.action_pressed(action);
        }
        self.players
            .get(player)
            .map_or(false, |p| p.input.pressed(action))
    }

    /// Camera-relative movement for one player.
    ///
    /// This is the review's tightest knot resolved (game.md finding 2): the
    /// rotation uses *that player's* `cam_yaw`, which for a remote player
    /// arrived inside their input packet, so nothing about the camera rig has
    /// to replicate. For player 0 the yaw is the world camera's and the
    /// expression is character-for-character the one the input object has
    /// always evaluated — including the `f32::cos` widened to f64, which is
    /// why this helper exists rather than a "tidier" shared formula.
    pub fn player_move(&self, player: crate::player::PlayerId) -> (f64, f64) {
        let (axis_x, axis_z, yaw) = if player.is_local_slot() {
            let key = |name: LiveId| self.held.contains(&name);
            let axis_x = ((key(live_id!(right)) as i8 - key(live_id!(left)) as i8) as f64
                + self.pad.axis_x)
                .clamp(-1.0, 1.0);
            let axis_z = ((key(live_id!(down)) as i8 - key(live_id!(up)) as i8) as f64
                + self.pad.axis_z)
                .clamp(-1.0, 1.0);
            (axis_x, axis_z, self.cam_yaw)
        } else {
            match self.players.get(player) {
                Some(p) => {
                    let (x, z) = p.input.axes();
                    (x, z, p.input.cam_yaw)
                }
                None => return (0.0, 0.0),
            }
        };
        (
            axis_x * yaw.cos() as f64 - axis_z * yaw.sin() as f64,
            axis_x * yaw.sin() as f64 + axis_z * yaw.cos() as f64,
        )
    }

    /// Reset everything a re-eval rebuilds. The HOST is responsible for two
    /// things alongside this call: stopping sustained audio (the synth is not
    /// a sim concern) and releasing the callback slots this discards (the
    /// slot table lives host-side; see game_view's CallbackTable).
    pub fn reset_content(&mut self) {
        self.entities.clear();
        self.parts.clear();
        self.labels.clear();
        self.terrain = None;
        self.sky = None;
        self.sun = SunConfig::default();
        self.next_id = 0;
        self.gravity = 30.0;
        self.on_tick = None;
        self.on_touch = None;
        self.on_join = None;
        self.on_leave = None;
        self.timers.clear();
        self.hud_slots.clear();
        self.hud_bars.clear();
        self.crosshair = false;
        self.beams.clear();
        self.cam_target = vec3f(0.0, 2.0, 0.0);
        self.cam_distance = 18.0;
        self.cam_follow = 0;
        self.cam_side = false;
        self.cam_third = 0;
        self.cam_height = 1.6;
        self.cam_boom = 10.0;
        self.cam_chase = 0;
        self.cam_lag = 0.3;
        self.cam_recenter = 1.2;
        self.cam_speed_tighten = 0.0;
        self.cam_pitch_request = None;
        self.cam_yaw_request = None;
        self.cam_fov = 40.0;
        self.cam_shake = 0.0;
        self.rng = 0x9E37_79B9_7F4A_7C15;
        // Fresh box3d world; the mirror rebuilds from entities at the next
        // reconcile, so rollback/reset can never leak orphan bodies.
        self.dynamics = crate::dynamics::RigidDynamics::new();
        // Players are connections, not world content: an edit must not kick
        // the room. Their bodies are gone though, so the references go with
        // them — script re-spawns and re-assigns during the same eval.
        for player in self.players.iter_mut() {
            player.entity = 0;
            player.hud = crate::player::PlayerHud::default();
        }
        // The doc contract: game.time() restarts at 0 on every reload. The
        // tick counter stays monotonic (log stamps, timer scheduling).
        // save_data deliberately survives — that's what game.save is FOR.
        self.time = 0.0;
        // A rebuilt world must never alias a previous world's slab revision.
        self.mark_render_dirty();
    }

    /// Find a HUD slot by name (mut), or None.
    pub fn hud_slot_mut(&mut self, name: &str) -> Option<&mut HudSlot> {
        self.hud_slots
            .iter_mut()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s)
    }

    pub fn rand(&mut self) -> f64 {
        // xorshift64* — cheap, deterministic, plenty for wander timers.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Entity ids are handed out monotonically and entities are only ever
    /// appended ([`push_entity`](Self::push_entity)) or `retain`ed, so the
    /// Vec is always sorted by id — binary search, not a linear scan. The
    /// invariant is asserted on push (O(1)) and once per tick in step_world.
    pub fn entity(&self, id: u64) -> Option<&Entity> {
        entity_index_sorted(&self.entities, id).map(|i| &self.entities[i])
    }

    pub fn entity_mut(&mut self, id: u64) -> Option<&mut Entity> {
        entity_index_sorted(&self.entities, id).map(|i| &mut self.entities[i])
    }

    /// The only sanctioned way to add an entity: enforces the sorted-by-id
    /// invariant every lookup (and the renderer's binary searches) relies on.
    pub fn push_entity(&mut self, entity: Entity) {
        debug_assert!(
            self.entities.last().map_or(true, |last| last.id < entity.id),
            "entity ids must be pushed in ascending order (id {} after {})",
            entity.id,
            self.entities.last().map(|e| e.id).unwrap_or(0),
        );
        self.entities.push(entity);
    }

    /// Debug-only full check of the sorted-by-id invariant (used once per
    /// tick; push_entity covers the incremental case).
    pub fn entities_sorted_by_id(&self) -> bool {
        self.entities.windows(2).all(|w| w[0].id < w[1].id)
    }

    pub fn log(&mut self, line: String) {
        self.log_pending.push(line);
    }

    /// See `render_rev`. Call after mutating anything static-visible.
    pub fn mark_render_dirty(&mut self) {
        self.render_rev = self.render_rev.wrapping_add(1);
    }

    /// Does a mutation of this entity id invalidate the static slab?
    pub fn is_static_visual(&self, id: u64) -> bool {
        self.entity(id).map_or(false, |e| e.kind == BodyKind::Static)
    }
}

/// Binary search over the sorted-by-id entity slice. Shared by the world's
/// own lookups and the renderer's per-part owner resolution so every consumer
/// leans on ONE enforced invariant instead of private assumptions.
pub fn entity_index_sorted(entities: &[Entity], id: u64) -> Option<usize> {
    entities.binary_search_by_key(&id, |e| e.id).ok()
}

#[cfg(test)]
mod id_lookup_tests {
    use super::*;

    fn ent(id: u64) -> Entity {
        Entity {
            id,
            ..Default::default()
        }
    }

    #[test]
    fn lookup_after_push_and_retain() {
        let mut w = GameWorld::default();
        for id in [1u64, 2, 5, 9, 12] {
            w.push_entity(ent(id));
        }
        assert!(w.entities_sorted_by_id());
        assert_eq!(w.entity(5).map(|e| e.id), Some(5));
        assert!(w.entity(3).is_none());
        w.entities.retain(|e| e.id != 5);
        assert!(w.entities_sorted_by_id());
        assert!(w.entity(5).is_none());
        assert_eq!(w.entity_mut(12).map(|e| e.id), Some(12));
        assert_eq!(entity_index_sorted(&w.entities, 9), Some(2));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "ascending order")]
    fn out_of_order_push_asserts() {
        let mut w = GameWorld::default();
        w.push_entity(ent(7));
        w.push_entity(ent(3));
    }
}
