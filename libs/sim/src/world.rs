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

/// One answer from the world-surface seam — see
/// [`GameWorld::surface_sample_at`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SurfaceSample {
    /// Ground at this height.
    Surface(f32),
    /// The column is open: carved through, or a punched heightfield cell.
    Hole,
    /// Beyond the heightfield (and no voxel ownership).
    Outside,
}

impl SurfaceSample {
    /// The height where there is ground; `None` for a hole or the outside.
    pub fn height(self) -> Option<f32> {
        match self {
            SurfaceSample::Surface(h) => Some(h),
            _ => None,
        }
    }
    pub fn is_ground(self) -> bool {
        matches!(self, SurfaceSample::Surface(_))
    }
}

/// THE composed world-surface seam as a free function, so a derived system
/// (navigation) can sample it while mutably borrowing its own cache out of
/// [`GameWorld`]. Sources compose in this order: a loaded level's floor
/// raster wherever it reaches (its floors are ground, its pits holes, its
/// walls and stacked storeys `Outside` — see [`crate::meshfloor`]), then
/// the voxel field where a chunk owns the surface, then the heightfield
/// (a punched cell is a hole). `Outside` past all of them. Navigation,
/// corridors, lots and spawns all read this one truth.
pub(crate) fn composed_surface_sample_at(
    floor: Option<&crate::meshfloor::FloorRaster>,
    terrain: Option<&Terrain>,
    terrain_materials: Option<&TerrainMaterials>,
    voxel: Option<&crate::voxel::VoxelField>,
    x: f32,
    z: f32,
) -> SurfaceSample {
    if let Some(floor) = floor {
        if let Some(sample) = floor.sample(x, z) {
            return sample;
        }
    }
    let base = terrain.and_then(|t| t.height_at(x, z));
    if let Some(v) = voxel {
        // No terrain = the voxel base layer's y=0 ground plane.
        let base_h = base.unwrap_or(0.0);
        if v.chunk_count() > 0 && v.owns_surface(x, z, base_h) {
            return match v.surface_at(x, z, base_h) {
                Some(h) => SurfaceSample::Surface(h),
                None => SurfaceSample::Hole,
            };
        }
    }
    match base {
        None => SurfaceSample::Outside,
        Some(h) => {
            let punched = match (terrain, terrain_materials) {
                (Some(t), Some(m)) => m.is_hole_at(t, x, z),
                _ => false,
            };
            if punched {
                SurfaceSample::Hole
            } else {
                SurfaceSample::Surface(h)
            }
        }
    }
}

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
    /// still lives in the `held`/`pressed`/`pad`/`cam_yaw`/`cam_pitch` fields
    /// below — a single-player world therefore evaluates exactly the
    /// expressions it did before players existed, which is what keeps input
    /// tapes byte-identical.
    /// Remote and bot players carry their own input in the roster.
    pub players: crate::player::Players,
    /// box3d dynamics layer (M1a): mirrored statics/kinematics + rigid
    /// bodies. Reconciled against `entities` each tick — never mutated by
    /// spawn/remove paths directly.
    pub dynamics: crate::dynamics::RigidDynamics,
    /// Derived walkability grid (mix.md D6): chunked, lazily derived from
    /// terrain + statics + water, dirty-flagged. Never replicated — only the
    /// simulating host runs brains/units. See [`crate::nav`].
    pub nav: crate::nav::NavMap,
    /// Line-of-sight authority beyond the sim's own bodies (a streamed
    /// level's walls). Installed by the host; `None` means creature sight
    /// is a distance test, exactly as it was before providers existed. An
    /// `Arc` so a world snapshot (rollback ring, replay) shares the same
    /// read-only geometry instead of copying it.
    pub los: Option<std::sync::Arc<dyn crate::providers::LosProvider>>,
    /// Route authority beyond the entity grid (a streamed level's own
    /// graph). ActorKit asks it for the next waypoint; `None` keeps the
    /// entity-derived walkability grid as the only routing answer.
    pub nav_provider: Option<std::sync::Arc<dyn crate::providers::NavProvider>>,
    pub next_id: u64,
    pub gravity: f32,
    pub on_tick: Option<CallbackSlot>,
    /// Per-commandable-unit decision hook. Called once per unit per tick,
    /// BEFORE the kit steers, with that unit's own situation — the seam a
    /// script needs to give one unit behaviour of its own rather than tuning
    /// the whole kit. Pair it with `unit(id, {control: "script"})` when the
    /// script means to drive the body itself.
    pub on_unit: Option<CallbackSlot>,
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
    /// The composed HUD: panels, gauges, readouts, icons, slot grids, the
    /// message log and the screen flash. The layer above `hud_slots`/
    /// `hud_bars`, which stay exactly as they are for the games that use
    /// them.
    pub hud: crate::hud::HudDoc,
    /// Camera requests from script.
    pub cam_target: Vec3f,
    pub cam_distance: f32,
    pub cam_follow: u64,
    pub cam_side: bool,
    /// Top-down strategy camera: the view looks straight down at
    /// `cam_target` from `cam_distance` metres, panned and zoomed by the
    /// player rather than following a body. A tiled strategy level turns it
    /// on the way an `fps` map turns on first person — the presentation is
    /// part of what the level IS, not a thing every script must remember.
    pub cam_top_down: bool,
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
    /// Near clip plane in metres (0 = the renderer's stock 0.15). The
    /// NORMALIZATION RULE: the near plane's frustum-corner reach
    /// (near x sqrt(1 + tan^2(fovx/2) + tan^2(fovy/2))) must stay inside
    /// the walker's wall margin, or a head against a wall sees through
    /// it. Classic-import maps shrink this to fit their declared body
    /// (a Doom body's 0.25 m radius needs ~0.10).
    pub cam_near: f32,
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
    /// Engine-default bullet marks. This fixed ring is presentation-only:
    /// shots never become entities and never invalidate the static slabs.
    pub bullet_decals: crate::decal::BulletDecals,
    /// Smooth heightfield ground (game.terrain smooth mode).
    pub terrain: Option<Terrain>,
    /// Per-cell surface materials for `terrain` (F10). None = uniform default
    /// surface, byte-identical to the pre-materials mirror. Bump
    /// `terrain.revision` after changing this — the box3d heightfield only
    /// rebuilds when the revision moves.
    pub terrain_materials: Option<TerrainMaterials>,
    /// Editable voxel terrain (mix.md D5/T1-T7), layered over the authored
    /// heightfield. None until `game.terrain_volume` declares a region —
    /// worlds that never do carry a null pointer and every pre-voxel code
    /// path byte-identically.
    pub voxel: Option<Box<crate::voxel::VoxelField>>,
    /// True while the host evaluates the level source (a PLAN solve). Ops
    /// issued inside it are plan products — re-derived by the next eval,
    /// retracted when their line is gone, never history; ops issued outside
    /// it (a brush, an excavator, a hand controller) are history. Set and
    /// cleared by the script host around eval.
    pub in_plan_eval: bool,
    /// Solve epochs (worldgen DESIGN.md, amendment E): `plan_revision`
    /// advances once per level eval, `history_revision` once per HISTORY
    /// terrain op (dig, tunnel, brush landform). A solve records the history
    /// epoch it read and can refuse to commit products derived from older
    /// ground once solves run off the play thread.
    pub plan_revision: u64,
    pub history_revision: u64,
    /// Water volumes with an analytic wave surface (mix.md D7/W1). None
    /// until `game.water` declares one — same null-pointer contract as
    /// `voxel`: worlds without it run every pre-water path byte-identically.
    /// (The legacy `terrain water:` sheet is NOT in here — it stays the flat
    /// sensor slab it always was.)
    pub water: Option<Box<crate::water::WaterState>>,
    /// A streamed level's solid geometry (`game.map`), for the queries that
    /// must see a map's walls and floors DURING the tick — the wheel
    /// raycasts of a vehicle, the camera boom sweep. None until the host
    /// installs a map; worlds without one run every pre-map path
    /// byte-identically. Shared by pointer across snapshots (a level is
    /// immutable while installed). See [`crate::level_solid`].
    pub level: Option<crate::level_solid::LevelSolidRef>,
    /// A streamed level's floors rasterised to one storey per column (see
    /// [`crate::meshfloor`]): the surface the generators drape on while a
    /// map is installed — its floors are ground, its pits holes, its
    /// walls `Outside`. None otherwise, so worlds without a map run every
    /// pre-map path byte-identically. Installed and cleared with `level`;
    /// an eval's `reset_content` keeps both.
    pub map_floor: Option<std::sync::Arc<crate::meshfloor::FloorRaster>>,
    /// The walk surfaces of the laid corridors (roads, rails, bridge
    /// decks): a mover on one stands at the drawn deck height exactly —
    /// see [`crate::deck`]. Empty in every world without corridors, so
    /// pre-corridor content runs the floor rules byte-identically.
    pub decks: Vec<crate::deck::DeckStrip>,
    /// Sky/fog, enabled by game.sky().
    pub sky: Option<SkyConfig>,
    /// What game.sun() asked for; the renderer resolves it (see SunConfig).
    /// Presentation only — the step never reads it.
    pub sun: SunConfig,
    /// World-level gameplay knobs (`game.tune({car_speed: 0.6})`): scalars
    /// every block of a kind multiplies its authored config by, each tick.
    /// Retroactive by construction — a car spawned before the tune reads the
    /// same scalar as one spawned after — and idempotent, so a re-declaration
    /// on the addon lane is the whole persistence story (see WorldTuning).
    pub tuning: WorldTuning,
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
    /// PERF: bumped whenever the static GEOMETRY of the world changes —
    /// a static entity spawned, removed, moved, resized, its parts settled,
    /// the sky or sun restyled. The renderer keys everything derived from
    /// static geometry on this: the packed instance slabs, the CPU
    /// occlusion bake, the shadow receivers and the GPU lightmap kick. Bump
    /// it (`mark_render_dirty`) or your static edit won't show.
    pub render_rev: u64,
    /// PERF: bumped when a static entity is only REPAINTED — colour or glow,
    /// nothing moved. Only the packed slabs key on this; a repaint never
    /// re-bakes light. `mark_paint_dirty`.
    pub paint_rev: u64,
}

impl GameWorld {
    /// The ground under `(x, z)` for a body that wants to rest near `near_y`:
    /// the terrain heightfield when the world has one, else the installed
    /// level's floor there ([`crate::level_solid::LevelSolid::ground_under`]),
    /// else `None` — the flat y = 0 of a world with no ground at all is the
    /// caller's default, not this function's.
    ///
    /// This is the one place a spawn asks "where is the floor" so a verb
    /// grounded on the terrain today grounds on a map's floor tomorrow
    /// without learning what a map is.
    pub fn ground_height_at(&self, x: f32, z: f32, near_y: f32) -> Option<f32> {
        // Under an installed map the level's own mesh answers first: it
        // knows which storey `near_y` is on, which the one-storey floor
        // raster cannot (a balcony's spawn must not drop to the hall).
        if self.map_floor.is_some() {
            if let Some(h) = self.level.as_ref().and_then(|level| level.ground_under(x, z, near_y)) {
                return Some(h);
            }
        }
        if let Some(h) = self.surface_height_at(x, z) {
            return Some(h);
        }
        self.level.as_ref().and_then(|level| level.ground_under(x, z, near_y))
    }

    /// THE world-surface seam: composed ground height at (x, z) —
    /// heightfield where no voxel chunk owns the surface, voxel surface
    /// where one does (a dug pit answers with its floor, a filled mound or
    /// a landform raised over carved ground with its top; a deep tunnel
    /// under an untouched ridge changes nothing). Everything that grounds
    /// gameplay on "the terrain" — spawns, draping, scatter, AI ground
    /// probes — samples through here so terrain edits compose everywhere
    /// at once. `None` outside the heightfield (and outside any voxel
    /// ownership): flat/streamed worlds keep their own floor rules.
    pub fn surface_height_at(&self, x: f32, z: f32) -> Option<f32> {
        self.surface_sample_at(x, z).height()
    }

    /// The seam with its edges spelled out (worldgen DESIGN.md, amendment
    /// B): `Surface(h)` where ground exists, `Hole` where the column has
    /// been carved through or the heightfield cell is punched out, `Outside`
    /// beyond the heightfield. A hole is never the heightfield underneath
    /// it (placement used to see ground over a pit) and the border is
    /// never height 0.
    pub fn surface_sample_at(&self, x: f32, z: f32) -> SurfaceSample {
        composed_surface_sample_at(
            self.map_floor.as_deref(),
            self.terrain.as_ref(),
            self.terrain_materials.as_ref(),
            self.voxel.as_deref(),
            x,
            z,
        )
    }

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
        // Gravity is the same trap and was missed by the same reasoning: it is
        // set by `reset_content` only, so every world built through the sim API
        // floated until its caller happened to know to set it. Four separate
        // test files had each grown their own `world.gravity = 30.0` line —
        // when a workaround gets copy-pasted, the default is the bug.
        //
        // A floating character reports no `on_floor`, so the symptom is a
        // controller that silently refuses to jump rather than anything that
        // looks like a physics problem. Matches `reset_content` deliberately:
        // two constructors that disagree about gravity is a worse trap again.
        world.gravity = 30.0;
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
            x if x == live_id!(punch) => self.pad.punch,
            x if x == live_id!(kick) => self.pad.kick,
            x if x == live_id!(guard) => self.pad.guard,
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
            x if x == live_id!(punch) => self.pad.punch_pressed,
            x if x == live_id!(kick) => self.pad.kick_pressed,
            x if x == live_id!(guard) => self.pad.guard_pressed,
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
        let (yaw, pitch, dx, dy) = (self.cam_yaw, self.cam_pitch, self.look_dx, self.look_dy);
        let local = self.players.local_mut();
        local.input.held = held;
        local.input.pressed = pressed;
        local.input.pad = pad;
        local.input.cam_yaw = yaw;
        local.input.cam_pitch = pitch;
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
    /// expression keeps the original f32 trig domain widened to f64, but now
    /// evaluates it through game-math so peers agree bit-for-bit.
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
        camera_relative_move(axis_x, axis_z, yaw)
    }

    /// Reset everything a re-eval rebuilds. The HOST is responsible for two
    /// things alongside this call: stopping sustained audio (the synth is not
    /// a sim concern) and releasing the callback slots this discards (the
    /// slot table lives host-side; see game_view's CallbackTable).
    pub fn reset_content(&mut self) {
        self.entities.clear();
        self.parts.clear();
        self.labels.clear();
        self.bullet_decals.clear();
        self.terrain = None;
        self.terrain_materials = None;
        // Voxel EDITS survive a re-eval — they are player state, like
        // save_data (mix.md D5: "edits survive script hot-reload"). Script
        // content (volumes, palette) clears and the new script re-declares.
        if let Some(voxel) = self.voxel.as_mut() {
            voxel.on_reset_content();
        }
        // Water is script content: the new eval re-declares its volumes.
        self.water = None;
        self.sky = None;
        self.sun = SunConfig::default();
        // World knobs are script content like the sun: the re-eval re-declares
        // them (`world.tune` appends an idempotent line, so a reload does).
        self.tuning = WorldTuning::default();
        self.next_id = 0;
        self.gravity = 30.0;
        self.on_tick = None;
        self.on_touch = None;
        self.on_join = None;
        self.on_leave = None;
        self.timers.clear();
        self.hud_slots.clear();
        self.hud_bars.clear();
        self.hud.clear();
        self.crosshair = false;
        self.beams.clear();
        self.cam_target = vec3f(0.0, 2.0, 0.0);
        self.cam_distance = 18.0;
        self.cam_follow = 0;
        self.cam_side = false;
        self.cam_top_down = false;
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
        self.cam_near = 0.15;
        self.cam_shake = 0.0;
        self.rng = 0x9E37_79B9_7F4A_7C15;
        // Fresh box3d world; the mirror rebuilds from entities at the next
        // reconcile, so rollback/reset can never leak orphan bodies.
        self.dynamics = crate::dynamics::RigidDynamics::new();
        // Nav re-derives from the rebuilt world on first query.
        self.nav = crate::nav::NavMap::default();
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

    /// Begin one authored simulation tick.
    ///
    /// Immediate-mode presentation primitives must be cleared before script
    /// callbacks run, not inside [`step_world`]: callbacks then repopulate the
    /// exact set the renderer should see for this tick. Keeping this boundary
    /// on the world also gives every host (local, LAN authority, or tests) the
    /// same lifecycle contract.
    pub fn begin_tick(&mut self) {
        self.beams.clear();
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

    /// See `paint_rev`. Call after restyling a static entity's colour or
    /// glow WITHOUT moving, resizing, spawning or removing anything: the
    /// packed slabs repaint, the light bake stays. A lamp head that turns
    /// red every few seconds is the case this exists for — through
    /// `mark_render_dirty` it rebaked the whole map's lightmap on every
    /// phase change (Crossroads, 2026-09-02: 68 bakes a minute).
    pub fn mark_paint_dirty(&mut self) {
        self.paint_rev = self.paint_rev.wrapping_add(1);
    }

    /// Bring the box3d mirror up to date for an exact query (F7): entities
    /// spawned or teleported since the last tick get their bodies before the
    /// cast runs, so `game.raycast` sees the world the script just built —
    /// the same immediacy the old AABB march had. A merge walk over already-
    /// synced state is near-free, so verbs call this unconditionally.
    ///
    /// One known lag, documented: a MOVER's capsule reaches a pose during
    /// the box3d step, so a mover moved by script mid-tick answers queries
    /// at its last stepped pose until the next step_world.
    pub fn sync_queries(&mut self) {
        let GameWorld {
            dynamics,
            entities,
            terrain,
            terrain_materials,
            voxel,
            gravity,
            ..
        } = self;
        crate::dynamics::reconcile(
            dynamics,
            entities,
            terrain.as_ref(),
            terrain_materials.as_ref(),
            voxel.as_deref(),
            *gravity,
        );
    }

    /// Does a mutation of this entity id invalidate the static slab?
    pub fn is_static_visual(&self, id: u64) -> bool {
        self.entity(id).map_or(false, |e| e.kind == BodyKind::Static)
    }

    /// Apply one voxel edit op with full authority: materialize chunks from
    /// the base heightfield under the brush, mutate, queue for replication.
    /// The verb layer and the host both come through here, in tick order —
    /// which IS the determinism story (mix.md D5: edits are ops). The whole
    /// world is implicitly editable: a first edit on a world without a
    /// field creates one (default lattice; `game.terrain_volume` still
    /// pre-creates with finer cells/palette). Landform ops route to
    /// [`crate::landform`], which owns their heightfield/voxel composition.
    pub fn apply_voxel_op(&mut self, op: crate::voxel::VoxelOp) {
        if let crate::voxel::VoxelOp::Landform { .. } = op {
            crate::landform::host_apply_landform(self, op);
            return;
        }
        let GameWorld {
            voxel,
            terrain,
            log_pending,
            ..
        } = self;
        let field = voxel
            .get_or_insert_with(|| Box::new(crate::voxel::VoxelField::new(0.5)));
        field.apply_op(op, terrain.as_ref(), true, true, log_pending);
        // A press is plan (routed into the patch inside apply_op); every
        // other op is HISTORY and moves the epoch a solve commits against —
        // unless the op is the EVAL'S OWN (a `game.dig`/`game.tunnel` line
        // in the level source, or persisted edits replayed by `game.terrain`
        // mid-eval): those are what the solve is reading, not something
        // that changed under it, and counting them refused every level
        // that dug its own ground on its rebuild.
        if !matches!(op, crate::voxel::VoxelOp::Press { .. }) && !self.in_plan_eval {
            self.history_revision = self.history_revision.wrapping_add(1);
        }
    }
}

/// Rotate raw stick/WASD axes into world movement by a camera yaw — "what the
/// player MEANS by forward is where the camera is looking".
///
/// Shared rather than inlined: [`GameWorld::player_move`] uses it for players
/// whose axes live in the world or in a replicated input packet, and the player
/// prefab uses it for a host that reads its own devices. Two copies of this
/// expression is how one of them ends up mirrored — the same class of bug as
/// the inverted steering, which is why [`crate::heading`] exists.
///
/// The deterministic f32 sine/cosine results are deliberately widened to f64:
/// this preserves the input object's numeric domain while removing platform
/// libm from replicated movement.
///
/// # The yaw is a VIEW yaw, not a heading
///
/// `yaw` is the angle you hand the renderer's camera rig, which is the negation
/// of a [`crate::heading`] yaw on both axes — the renderer looks along
/// `(sin y, −cos y)` where a heading faces `(−sin y, −cos y)`. Passing an
/// entity's `yaw` here compiles fine and mirrors the controls about the Z axis,
/// so if you hold a heading, negate it once at a named boundary rather than
/// here. With a view yaw, `axis_z = −1` ("forward") comes out along the camera's
/// own look direction, which is the whole point of the function.
pub fn camera_relative_move(axis_x: f64, axis_z: f64, yaw: f32) -> (f64, f64) {
    let (sin_yaw, cos_yaw) = crate::math::sincos(yaw);
    let (sin_yaw, cos_yaw) = (sin_yaw as f64, cos_yaw as f64);
    (
        axis_x * cos_yaw - axis_z * sin_yaw,
        axis_x * sin_yaw + axis_z * cos_yaw,
    )
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

    /// The two static revisions are separate on purpose: a repaint reaches
    /// the packed slabs and nothing else, a geometry change reaches
    /// everything derived from static geometry (see the fields' docs).
    #[test]
    fn a_repaint_moves_paint_rev_and_leaves_the_geometry_revision_alone() {
        let mut w = GameWorld::new();
        let (render, paint) = (w.render_rev, w.paint_rev);
        w.mark_paint_dirty();
        assert_eq!(w.render_rev, render);
        assert_eq!(w.paint_rev, paint.wrapping_add(1));
        w.mark_render_dirty();
        assert_eq!(w.render_rev, render.wrapping_add(1));
        assert_eq!(w.paint_rev, paint.wrapping_add(1));
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
    fn tick_begin_clears_immediate_mode_beams() {
        let mut w = GameWorld::default();
        w.beams.push(Beam {
            from: vec3f(0.0, 0.0, 0.0),
            to: vec3f(1.0, 0.0, 0.0),
            size: 0.1,
            color: vec4f(1.0, 1.0, 1.0, 1.0),
            glow: 0.0,
        });

        w.begin_tick();

        assert!(w.beams.is_empty());
    }

    #[test]
    fn camera_relative_move_has_stable_bits() {
        // This input is intentionally near zero: common libm implementations
        // round its sine one bit away from the fixed game-math kernel.
        let yaw = f32::from_bits(0x3ad9_099e);
        let (x, z) = camera_relative_move(0.375, -0.8125, yaw);
        assert_eq!(x.to_bits(), 0x3fd8_1608_d156_0000);
        assert_eq!(z.to_bits(), 0xbfe9_fae7_7076_0000);
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
