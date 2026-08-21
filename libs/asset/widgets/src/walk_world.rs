//! The autonomous walkthrough: build a level's collision and navigation off
//! the frame thread, then let a walker tour it.
//!
//! The heavy lifting was already shared — `makepad_render::level` has the
//! triangle collision, the nav grid and the locomotion, and
//! `makepad_render::player_nav` has the room-level planner that decides
//! which unexplored exit to take. What was NOT shared was the glue: which
//! config to build the grid with, how a walker's door index becomes a
//! renderer part, what to do when a probe frame reports no floor, and how
//! the camera rides the walker. That glue lived in one app, so only that app
//! could show a world.
//!
//! It lives here now. A well hands over GLB bytes; this builds a tour and
//! ticks it.
//!
//! # ONE walker walks every game
//!
//! *"Since we have to layer a game engine on it, the maps must have enough
//! similarity that a single map-walker can do all of them."*
//!
//! That is the law, and the unified map contract is what makes it hold:
//! every classic importer emits the SAME structure — one GLB with
//! `door_N` / `lift_N` / `hazard_N` / `sky` nodes and declared clips, a
//! `.spawn` anchor superset (player_start, keys, exit, teleports), a prelit
//! `COLOR_0` marker, metres, Y-up, −Z-forward. So `LevelCollision`,
//! `NavGrid` and `player_nav` run Doom, Quake, Quake II/III and Duke without
//! knowing which one they are looking at.
//!
//! Per-game difference may enter in exactly TWO places:
//!
//! 1. **The style** — [`WalkerConfig::for_style`]: eye height, step rule,
//!    gravity, bob cadence, walk speed. One enum, picked from what the host
//!    knows about where the map came from.
//! 2. **Declared map facts** — a Quake door's axis and `offset`, a liquid
//!    volume's `solid: false`, a Duke door baked open. These are read from
//!    the DATA, per the contract.
//!
//! Anything else — an `if game == quake` in the walker, the preview, or the
//! nav planner — is a CONTRACT GAP, not a feature. It means an importer did
//! not declare something it knows, and the fix belongs there: the same
//! contract is what the game engine will layer on, so a special case here
//! is a special case in the engine later.
//!
//! Behind the `renderer` feature, because it needs `makepad-render` — see
//! the crate's dependency note for why that is not the default.

use makepad_render::level::{
    surface_kinds_from_glb, yaw_forward, BobStyle, LevelCollision, LevelWalker, NavGrid,
    SurfaceKind, UpAxis, WalkerConfig, WalkerEvent,
};
use makepad_render::model::MODEL_VERTEX_FLOATS;
use makepad_render::player_nav::{config_for_world, NavAnchor, PlayerNav};
use makepad_render::{Renderer, StaticModel};
use makepad_widgets::*;

/// What a worker thread produced from a level's geometry. Building the nav
/// grid is a capsule probe per cell and a wall probe per edge — seconds on a
/// real map — which is the whole reason this is a separate step a host runs
/// off the frame thread.
pub struct WalkPrep {
    pub level: Option<Box<LevelCollision>>,
    pub nav: Option<Box<NavGrid>>,
    /// Somewhere to stand: the middle of the biggest connected piece of the
    /// map, or a probed interior spot when there is no graph at all.
    pub start: Option<Vec3f>,
    /// Per-triangle surface kinds actually found, for a host that wants to
    /// say so. `(hazard, liquid, total)`.
    pub kinds: (usize, usize, usize),
}

/// Index the level's own triangles and build its navigation graph.
///
/// Call on a worker. `cfg` is the body that will walk it — the same one the
/// tour runs with, so the grid is not built for one set of legs and walked
/// by another.
pub fn build_level(model: &StaticModel, glb: &[u8], cfg: &WalkerConfig) -> WalkPrep {
    let none = WalkPrep { level: None, nav: None, start: None, kinds: (0, 0, 0) };
    // Every classic pack publishes Y-up (the importer converts).
    let Some(level) =
        LevelCollision::from_packed(&model.vertices, MODEL_VERTEX_FLOATS, &model.indices, UpAxis::Y)
    else {
        return none;
    };
    // Which floors hurt: the importer's `hazard_N` nodes, or the source
    // engine's flat names on older publications. Without this every floor is
    // plain and the tour happily paddles through the nukage.
    let (level, kinds) = match surface_kinds_from_glb(glb, model.triangle_count()) {
        Some(kinds) => {
            let hazard = kinds.iter().filter(|k| **k == SurfaceKind::Hazard).count();
            let liquid = kinds.iter().filter(|k| **k == SurfaceKind::Liquid).count();
            let total = kinds.len();
            (level.with_kinds(kinds), (hazard, liquid, total))
        }
        None => (level, (0, 0, 0)),
    };
    let nav = NavGrid::build(&level, cfg);
    let start = nav
        .best_start()
        .and_then(|c| nav.cell(c).map(|c| c.pos))
        .or_else(|| level.interior_start(cfg));
    let nav = (!nav.is_empty()).then(|| Box::new(nav));
    WalkPrep { level: Some(Box::new(level)), nav, start, kinds }
}

/// Where the camera is this tick, and whether something happened worth
/// showing.
#[derive(Clone, Copy, Debug)]
pub struct WalkMoment {
    pub eye: Vec3f,
    pub yaw: f32,
    pub roll: f32,
    /// A cut — a teleporter, or leaving a wing the tour has finished. Without
    /// a flash it reads as a mistake: the camera is simply somewhere else.
    pub flash: bool,
}

impl WalkMoment {
    /// Where the eye is looking, half a metre ahead — the point a preview
    /// camera targets when its lens sits at the walker's eye.
    pub fn target(&self) -> Vec3f {
        self.eye + yaw_forward(self.yaw) * 0.5
    }
}

/// A walkable level loaded at its authored scale, plus the tourist.
///
/// Collision is the level's own triangles: the renderer's prop collider is a
/// box decomposition (a few dozen boxes for a whole map), and a walker
/// standing on those stands in mid-air. The instance transform is identity
/// for a map, so model space IS world space.
pub struct WalkWorld {
    walker: LevelWalker,
    level: Box<LevelCollision>,
    /// Route graph over the whole map. Without it the walker falls back to
    /// scoring headings locally, which paces one corridor forever.
    nav: Option<Box<NavGrid>>,
    /// The player-behaviour planner — rooms, the entry look-around,
    /// unexplored-exit choices, backtracking, door requests, the
    /// hazard-never rule. It steers; the walker keeps doing all locomotion.
    /// `None` only when the level has no nav grid at all, and then the
    /// walker's built-in frontier tour keeps the picture alive.
    player: Option<PlayerNav>,
    /// Where the tour restarts from when the walker strands itself.
    home: Vec3f,
    /// Renderer model id of the level, for door commands.
    model: String,
    /// Door parts in the order `NavGrid::mark_doors` was given them, so a
    /// walker's door index names a part again.
    doors: Vec<String>,
    /// Consecutive ticks the walker reported no floor under its feet. One bad
    /// probe — a downward ray catching a wall triangle's top edge — must not
    /// cost the tour everything it has learned about the map.
    stranded_ticks: u32,
}

/// Ticks of genuinely nothing underfoot before the tour cuts home. Half a
/// second at 60 Hz.
const STRANDED_LIMIT: u32 = 30;

impl WalkWorld {
    /// Start a tour, or `None` when there is nowhere to stand — a sealed
    /// shell, or a mesh with no floors. The caller falls back to an orbit
    /// rather than showing a stuck camera.
    ///
    /// `doors` are the level's animated parts and their world-space boxes: a
    /// level's doors are animated nodes, NOT part of the static collision
    /// mesh, so their cells are walkable in the graph and the tour opens them
    /// on approach.
    pub fn new(
        model: String,
        prep: WalkPrep,
        cfg: WalkerConfig,
        anchors: &[NavAnchor],
        doors: Vec<(String, (Vec3f, Vec3f))>,
        seed: u64,
    ) -> Option<WalkWorld> {
        let level = prep.level?;
        let mut nav = prep.nav;
        if let Some(nav) = nav.as_mut() {
            let boxes: Vec<(Vec3f, Vec3f)> = doors.iter().map(|(_, b)| *b).collect();
            if !boxes.is_empty() {
                nav.mark_doors(&boxes);
            }
        }
        // Seeded by the caller, so one tour is repeatable while two wells
        // showing the same map do not walk in lockstep.
        let seed = seed ^ level.triangles() as u64;
        // The planner's anchored start (when the manifest carries one) beats
        // the grid's best guess.
        let player = nav.as_deref().and_then(|g| PlayerNav::new(g, &level, &cfg, anchors, seed));
        let (start, start_yaw) = match player.as_ref() {
            Some(p) => {
                let (feet, yaw) = p.start_hint();
                (Some(feet), yaw)
            }
            None => (prep.start, 0.0),
        };
        Some(WalkWorld {
            walker: LevelWalker::new(start?, start_yaw, cfg, seed),
            level,
            nav,
            player,
            home: start?,
            model,
            doors: doors.into_iter().map(|(p, _)| p).collect(),
            stranded_ticks: 0,
        })
    }

    pub fn triangles(&self) -> usize {
        self.level.triangles()
    }

    pub fn nav_cells(&self) -> usize {
        self.nav.as_ref().map(|g| g.len()).unwrap_or(0)
    }

    pub fn doors(&self) -> usize {
        self.doors.len()
    }

    pub fn feet(&self) -> Vec3f {
        self.walker.feet()
    }

    /// One fixed step of the tour: the planner thinks, the walker moves, the
    /// doors it wants are driven, and the camera lands where its eye is.
    pub fn tick(&mut self, dt: f32, renderer: &mut Renderer) -> WalkMoment {
        // The player's mind runs first — rooms, look-around pans, which exit
        // leads somewhere new, door requests, cuts. It steers the walker
        // through the level.rs seam; everything below is unchanged and also
        // serves the built-in tour when no planner exists.
        if let (Some(p), Some(grid)) = (self.player.as_mut(), self.nav.as_deref()) {
            p.steer(dt, &mut self.walker, grid, &self.level);
        }
        if self.walker.tick_in(dt, &self.level, self.nav.as_deref()) == WalkerEvent::Stranded {
            self.stranded_ticks += 1;
            if self.stranded_ticks >= STRANDED_LIMIT {
                self.stranded_ticks = 0;
                // `relocate` KEEPS everything the tour has learned;
                // rebuilding the walker threw away the whole map memory, and
                // one unlucky probe frame then reset the tour to zero.
                let (home, yaw) = (self.home, self.walker.yaw());
                self.walker.relocate(home, yaw);
            }
        } else {
            self.stranded_ticks = 0;
        }
        // A door the route wants: drive the part, then tell the walker it may
        // walk through once the part has settled on "open".
        if let Some(d) = self.walker.wanted_door() {
            match self.doors.get(d as usize).cloned() {
                Some(part) => {
                    let settled = renderer
                        .model_part_state(self.model.as_str(), &part)
                        .is_some_and(|s| s.state_name == "open" && s.settled);
                    if settled {
                        self.walker.set_door_open(d, true);
                    } else {
                        renderer.set_model_state(self.model.as_str(), &part, "open", 0.6);
                    }
                }
                // No such part (a grid marked from stale boxes): never let
                // the tour stand there waiting for a door that is not real.
                None => self.walker.set_door_open(d, true),
            }
        }
        renderer.tick_model_states(dt);
        let pose = self.walker.camera();
        WalkMoment {
            eye: pose.eye,
            yaw: pose.yaw,
            roll: pose.roll,
            flash: self.walker.take_flash(),
        }
    }

    /// A one-line coverage report, for a host that traces tours.
    pub fn coverage(&self) -> String {
        let s = self.walker.nav_stats();
        let rooms = match self.player.as_ref() {
            Some(p) => {
                let ps = p.stats();
                format!(
                    ", rooms {}/{} ({} portals), doors {}, jams {}, cuts {}, restarts {}",
                    ps.visited, ps.rooms, ps.portals, ps.doors_opened, ps.jammed, ps.region_cuts,
                    ps.restarts
                )
            }
            None => String::new(),
        };
        format!(
            "distinct {}/{} cells, reachable {} ({} unseen), goal {:?}, {} waypoints left{rooms}",
            s.distinct, s.cells, s.reachable, s.unseen, s.goal, s.path_left
        )
    }
}

/// The body, gait and gravity a world walks with — the ONE place a host
/// decides them, so a grid is never built for one set of legs and walked by
/// another.
///
/// Both halves of the contract's per-game allowance go in: the STYLE, from
/// whatever the host knows about the source engine (a tag, a path, a
/// manifest — anything unrecognised gets Doom's), and the map's own
/// DECLARED facts, which say what its metres are worth. A preset without
/// the declaration is a body in some other map's units.
pub fn config_for(source: &str, anchors: &[NavAnchor]) -> WalkerConfig {
    config_for_world(BobStyle::from_source(source), anchors)
}
