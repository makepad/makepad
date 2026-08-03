//! A Zelda-scale world: several regions from several Kenney kits, connected by
//! roads, populated by a mixed cast.
//!
//! The build is split in two on purpose. [`plan`] is PURE — it picks model ids
//! and decides positions using the asset index and the generators, touching no
//! GPU and no `Cx`, so the whole layout is testable headlessly (region
//! placement, road connectivity, prop spacing, determinism). [`realise`] then
//! loads what the plan asked for and turns it into draw instances and
//! colliders, which is the only part that needs a renderer.
//!
//! That split is what lets a test assert "the village connects to the castle"
//! without a window, and it is also the shape a generated game would want: an
//! AI writes a plan, the engine realises it.
//!
//! # Why regions are single-kit
//!
//! Each region draws its props from ONE pack wherever it can. A level built
//! from one tile of each of five packs reads as a junk drawer; a level built
//! from one pack reads as designed. The asset index groups by kit precisely so
//! this is expressible — see `AssetIndex::kits`.

use makepad_game_assets::{AssetIndex, AssetKind, Filters, Spread, VarietyParams};
use makepad_game_render::ModelInstance;
use makepad_game_gen::kit::{Kit, TileDef, TileRole};
use makepad_game_gen::levelgen::{self, DungeonParams, RoadParams};
use makepad_game_gen::rng::GenRng;
use makepad_game_gen::scatter::{scatter, ScatterParams};
use makepad_widgets::makepad_math::*;

/// What a placed prop does to something walking into it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Blocking {
    /// Scenery: walk straight through. Ground decals, lamps, grass.
    None,
    /// The whole footprint stops you: buildings, walls, rocks, crates.
    Solid,
    /// Only the trunk. A canopy that blocked would make a wood impassable and
    /// read as invisible walls.
    Trunk,
}

/// One prop the plan wants in the world.
///
/// `target_h` rather than a scale factor: Kenney packs are authored at wildly
/// different native sizes, so a fixed multiplier gives a 12-unit bench beside a
/// 2-unit house. The realise step reads each model's own bounds and derives the
/// scale, which keeps proportions right whichever pack a query resolved to.
#[derive(Clone, Debug)]
pub struct Placement {
    pub model: String,
    pub pos: Vec3f,
    pub yaw: f32,
    pub target_h: f32,
    pub blocking: Blocking,
    /// Which region asked for it, for stats and for cutting by region later.
    pub region: Region,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Region {
    Village,
    Castle,
    Woods,
    Dungeon,
    Quarry,
    Roads,
}

impl Region {
    pub fn name(self) -> &'static str {
        match self {
            Region::Village => "village",
            Region::Castle => "castle",
            Region::Woods => "woods",
            Region::Dungeon => "dungeon",
            Region::Quarry => "quarry",
            Region::Roads => "roads",
        }
    }
}

/// A destination NPCs choose between, and a landmark a player can head for.
#[derive(Clone, Debug)]
pub struct PoiPlan {
    pub pos: Vec3f,
    pub tag: &'static str,
    pub capacity: u8,
}

/// An inhabitant the plan wants spawned.
#[derive(Clone, Debug)]
pub struct NpcPlan {
    pub pos: Vec3f,
    /// Asset id of the rigged character, or `None` if no cast was available.
    pub character: Option<String>,
    /// Which rig it belongs to, so the caller can group by skinned model.
    pub joints: u32,
    pub home: Vec3f,
    pub region: Region,
    /// Slower for a guard on a beat, quicker for a villager on an errand.
    pub speed: f32,
}

/// Something a player can walk up to and bump: a chest, a barrel, a door.
#[derive(Clone, Debug)]
pub struct Interactable {
    pub pos: Vec3f,
    pub kind: &'static str,
    pub region: Region,
}

#[derive(Clone, Debug, Default)]
pub struct PlanStats {
    pub gen_us: u64,
    pub tiles: usize,
    pub props: usize,
    pub npcs: usize,
    pub distinct_models: usize,
    /// Region -> prop count, for the budget report and for a governor that
    /// wants to cut the cheapest-looking region first.
    pub per_region: Vec<(Region, usize)>,
}

/// The whole world, decided but not yet realised.
#[derive(Clone, Debug, Default)]
pub struct WorldPlan {
    pub placements: Vec<Placement>,
    pub pois: Vec<PoiPlan>,
    pub npcs: Vec<NpcPlan>,
    pub interactables: Vec<Interactable>,
    /// Where a player should start: the village square, facing the castle.
    pub player_start: Vec3f,
    /// Good camera positions for captures and for an establishing shot.
    pub viewpoints: Vec<(Vec3f, Vec3f)>,
    pub stats: PlanStats,
}

// --------------------------------------------------------------- geography

/// Region centres. Laid out so the castle is visible down the road from the
/// village — a landmark you can walk toward is what gives a world orientation,
/// and it is the cheapest possible navigation aid.
pub const VILLAGE: Vec2f = Vec2f { x: 0.0, y: 0.0 };
pub const CASTLE: Vec2f = Vec2f { x: 78.0, y: -58.0 };
pub const WOODS: Vec2f = Vec2f { x: -74.0, y: -46.0 };
pub const DUNGEON: Vec2f = Vec2f { x: 74.0, y: 62.0 };
pub const QUARRY: Vec2f = Vec2f { x: -66.0, y: 56.0 };
/// Where the four roads meet. Everything is reachable from here on foot.
pub const CROSSROAD: Vec2f = Vec2f { x: 4.0, y: 6.0 };

fn v3(p: Vec2f, y: f32) -> Vec3f {
    vec3f(p.x, y, p.y)
}

fn dist2(a: Vec2f, b: Vec2f) -> f32 {
    let (dx, dz) = (a.x - b.x, a.y - b.y);
    dx * dx + dz * dz
}

// ------------------------------------------------------------ kit bridge

/// Build a [`Kit`] from the asset index.
///
/// The index parses a coarse role from each filename, but it lumps every
/// 3-and-4-way piece under "junction" — and a crossroad standing in for a tee
/// leaves a stub opening onto nothing. The connection MASK is what fitting
/// actually uses, so the finer classification is done here from the name,
/// which is exactly the override `TileDef::mask` exists for.
fn kit_from_index(index: &AssetIndex, pack: &str, tile_h: f32) -> Option<Kit> {
    let info = index.kits().into_iter().find(|k| k.pack == pack)?;
    let mut tiles = Vec::new();
    for e in index.kit_tiles(pack, None) {
        let stem = e.id.rsplit('/').next().unwrap_or("");
        let Some(role) = classify_tile(stem) else {
            continue;
        };
        tiles.push(TileDef::new(e.id.clone(), role, tile_h));
    }
    if tiles.is_empty() {
        return None;
    }
    // A kit whose tile size could not be measured has no grid pitch to
    // fit against, so it is not composable — say so rather than guessing a
    // number and producing a level with overlapping tiles.
    Some(Kit::new(pack, info.tile_size?, tiles))
}

/// Map a Kenney tile filename onto a connection role.
///
/// Order matters: `road-crossroad` must be tested before `road-cross`-anything
/// generic, and `corridor-wide-intersection` before `corridor-wide`. Returning
/// `None` drops decorative pieces (cones, lights, barriers) that would
/// otherwise be fitted into a road run.
fn classify_tile(stem: &str) -> Option<TileRole> {
    let s = stem;
    // Four-way.
    if s.contains("crossroad") || s.contains("intersection") {
        return Some(TileRole::Cross);
    }
    // Three-way.
    if s.contains("junction") || s.contains("-split") {
        return Some(TileRole::TJunction);
    }
    if s.contains("bend") || s.contains("curve") || s.contains("corner") {
        return Some(TileRole::Corner);
    }
    if s.contains("end") || s.contains("-cap") {
        return Some(TileRole::End);
    }
    if s.starts_with("gate-door") || s == "gate" {
        return Some(TileRole::Door);
    }
    if s.starts_with("stairs") {
        return Some(TileRole::Stairs);
    }
    if s.starts_with("room") || s.starts_with("floor") {
        return Some(TileRole::Floor);
    }
    if s.starts_with("wall") {
        return Some(TileRole::Wall);
    }
    // Plain runs last, so the qualified names above win.
    if s.starts_with("road-straight") || s == "road" || s == "corridor" || s == "corridor-wide" {
        return Some(TileRole::Straight);
    }
    None
}

// --------------------------------------------------------------- picking

/// Distinct models for a role, in the order the scene should use them.
///
/// Over-asks deliberately: some candidates belong to packs whose atlas never
/// downloaded and will fail to load, so the surplus keeps a region full rather
/// than leaving holes. `Spread::Variants` for anything that must stay one
/// recognisable object (a fence must not become a market stall); `Kinds` for
/// a set that should genuinely differ (five house designs).
fn pick(index: &AssetIndex, query: &str, count: usize, spread: Spread, seed: u64) -> Vec<String> {
    pick_from(index, query, &[], count, spread, seed)
}

/// As [`pick`], but preferring packs in `prefer`.
///
/// This is what keeps a region coherent. Search ranks by relevance across the
/// whole library, so "castle wall stone" legitimately returns a graveyard wall
/// and a tower-defense base above castle-kit's own — each is a stone wall. The
/// result is a castle assembled from four art styles, which is the junk-drawer
/// failure arrived at from a new direction: not one model repeated, but every
/// model from somewhere else.
///
/// The preference is applied AFTER ranking rather than as a hard filter, and
/// falls back to the unfiltered list when a pack has nothing suitable — a
/// region drawn from the wrong pack still looks better than an empty one.
fn pick_from(
    index: &AssetIndex,
    query: &str,
    prefer: &[&str],
    count: usize,
    spread: Spread,
    seed: u64,
) -> Vec<String> {
    let params = VarietyParams {
        // Over-ask, and much harder when a pack is preferred. Ranking is
        // global, so castle-kit's own `wall-corner` can sit below a graveyard
        // wall and a tower-defense base for "wall" — all three are walls. A
        // shallow pool then contains none of the preferred pack and the
        // fallback silently fires, which is how a castle ends up assembled
        // from four art styles. The surplus also covers packs whose atlas
        // never downloaded and cannot render.
        count: count + if prefer.is_empty() { 12 } else { 60 },
        spread,
        seed,
        filters: Filters {
            kind: Some(AssetKind::Model),
            ..Default::default()
        },
    };
    let hits: Vec<String> = index
        .find_many(query, &params)
        .into_iter()
        // Kit tiles welded to a chunk of ground read as floating islands when
        // placed on open grass — a hex-tile house is a house on a hex of turf.
        .filter(|e| !e.id.contains("hexagon-kit"))
        .map(|e| e.id.clone())
        .collect();
    if prefer.is_empty() {
        return hits;
    }
    let preferred: Vec<String> = hits
        .iter()
        .filter(|id| prefer.iter().any(|p| id.contains(p)))
        .cloned()
        .collect();
    if preferred.is_empty() {
        hits
    } else {
        preferred
    }
}

fn nth<'a>(ids: &'a [String], i: usize) -> Option<&'a String> {
    if ids.is_empty() {
        None
    } else {
        Some(&ids[i % ids.len()])
    }
}

// ------------------------------------------------------------------ plan

/// Decide the whole world. Pure: no GPU, no `Cx`, no filesystem beyond the
/// index that was already built.
pub fn plan(index: &AssetIndex, seed: u64) -> WorldPlan {
    let t0 = std::time::Instant::now();
    let mut w = WorldPlan {
        player_start: v3(VILLAGE, 0.9),
        ..Default::default()
    };
    let mut rng = GenRng::new(seed ^ 0xB16_0000_0000_0001);

    roads(index, seed, &mut w);
    village(index, seed, &mut rng, &mut w);
    castle(index, seed, &mut rng, &mut w);
    woods(index, seed, &mut w);
    dungeon(index, seed, &mut w);
    quarry(index, seed, &mut rng, &mut w);

    // Viewpoints for captures: an establishing shot down the road toward the
    // castle, then one per region.
    w.viewpoints = vec![
        (vec3f(-26.0, 34.0, 54.0), v3(CROSSROAD, 0.0)),
        (v3(VILLAGE + vec2f(-30.0, 30.0), 20.0), v3(VILLAGE, 0.0)),
        (v3(CASTLE + vec2f(-34.0, 30.0), 22.0), v3(CASTLE, 4.0)),
        (v3(DUNGEON + vec2f(-28.0, 26.0), 18.0), v3(DUNGEON, 0.0)),
    ];

    let mut per: Vec<(Region, usize)> = Vec::new();
    for r in [
        Region::Roads,
        Region::Village,
        Region::Castle,
        Region::Woods,
        Region::Dungeon,
        Region::Quarry,
    ] {
        let n = w.placements.iter().filter(|p| p.region == r).count();
        per.push((r, n));
    }
    let mut models: Vec<&str> = w.placements.iter().map(|p| p.model.as_str()).collect();
    models.sort_unstable();
    models.dedup();
    w.stats = PlanStats {
        gen_us: t0.elapsed().as_micros() as u64,
        tiles: w.stats.tiles,
        props: w.placements.len(),
        npcs: w.npcs.len(),
        distinct_models: models.len(),
        per_region: per,
    };
    w
}

/// Roads from the crossroad out to every region.
///
/// Junction type is never specified: the generator reads it off how many
/// occupied neighbours a cell has, so four paths meeting produce a crossroad
/// and a spur produces a tee, with adjacency correct by construction.
fn roads(index: &AssetIndex, seed: u64, w: &mut WorldPlan) {
    let Some(kit) = kit_from_index(index, "city-kit-roads", 0.12) else {
        return;
    };
    let cross = v3(CROSSROAD, 0.0);
    let paths: Vec<Vec<Vec3f>> = vec![
        vec![v3(VILLAGE, 0.0), cross],
        vec![cross, v3(CASTLE, 0.0)],
        vec![cross, v3(DUNGEON, 0.0)],
        vec![v3(VILLAGE, 0.0), v3(WOODS, 0.0)],
        vec![v3(VILLAGE, 0.0), v3(QUARRY, 0.0)],
    ];
    let level = levelgen::road_network(&kit, &RoadParams { seed, paths: &paths });
    for layer in &level.layers {
        w.stats.tiles += layer.placements.len();
        for p in &layer.placements {
            let Some(t) = kit.tiles.get(p.tile as usize) else {
                continue;
            };
            w.placements.push(Placement {
                model: t.id.clone(),
                pos: p.pos,
                yaw: p.yaw(),
                // Road tiles are authored at their own scale; a target height
                // would squash them. One tile-size tall keeps the pitch.
                target_h: layer.tile_size * 0.12,
                blocking: Blocking::None,
                region: Region::Roads,
            });
        }
    }
    // A signpost where the roads meet: the one place a player has to choose.
    if let Some(sign) = nth(&pick(index, "signpost sign wooden", 3, Spread::Variants, seed), 0) {
        w.placements.push(Placement {
            model: sign.clone(),
            pos: vec3f(cross.x + 2.2, 0.0, cross.z + 2.2),
            yaw: 0.6,
            target_h: 2.4,
            blocking: Blocking::Solid,
            region: Region::Roads,
        });
    }
    w.pois.push(PoiPlan {
        pos: cross,
        tag: "crossroad",
        capacity: 4,
    });
}

/// The village: varied houses fronting a square, a well, market stalls,
/// fences bounding gardens, and civilians who live there.
fn village(index: &AssetIndex, seed: u64, rng: &mut GenRng, w: &mut WorldPlan) {
    // One art pack for the whole region wherever it has the piece. Mixing a
    // suburban house with a fantasy stall reads as a bug even when each model
    // is individually correct.
    const TOWN: &[&str] = &["fantasy-town-kit", "city-kit-suburban"];
    let houses = pick_from(index, "house building home", TOWN, 6, Spread::Kinds, seed);
    let stalls = pick_from(index, "market stall awning", TOWN, 3, Spread::Kinds, seed ^ 7);
    let fences = pick_from(index, "fence panel wooden", TOWN, 2, Spread::Variants, seed ^ 11);
    let wells = pick_from(index, "well water", TOWN, 2, Spread::Variants, seed ^ 13);
    let barrels = pick_from(index, "barrel crate", TOWN, 3, Spread::Kinds, seed ^ 17);

    // Houses around a square, each FACING it. Uniform facing is the point:
    // random yaw reads as debris, and a village is defined by everything
    // addressing the same public space.
    let ring = 17.0f32;
    for i in 0..6 {
        let a = i as f32 * core::f32::consts::TAU / 6.0 + 0.35;
        let (sx, sz) = (a.sin(), a.cos());
        let pos = vec3f(VILLAGE.x + sx * ring, 0.0, VILLAGE.y + sz * ring);
        let Some(m) = nth(&houses, i) else { break };
        w.placements.push(Placement {
            model: m.clone(),
            pos,
            // Face inward: yaw toward the square centre.
            yaw: a + core::f32::consts::PI,
            target_h: 5.4 + rng.range(-0.4, 0.6),
            blocking: Blocking::Solid,
            region: Region::Village,
        });
        // A door to walk to, just outside the wall.
        w.pois.push(PoiPlan {
            pos: vec3f(pos.x - sx * 3.4, 0.0, pos.z - sz * 3.4),
            tag: "door",
            capacity: 1,
        });
    }

    // The well at the centre — the thing a square is built around.
    if let Some(m) = nth(&wells, 0) {
        w.placements.push(Placement {
            model: m.clone(),
            pos: v3(VILLAGE, 0.0),
            yaw: 0.0,
            target_h: 1.8,
            blocking: Blocking::Solid,
            region: Region::Village,
        });
        w.pois.push(PoiPlan {
            pos: v3(VILLAGE + vec2f(2.6, 0.0), 0.0),
            tag: "well",
            capacity: 3,
        });
        w.interactables.push(Interactable {
            pos: v3(VILLAGE, 0.0),
            kind: "well",
            region: Region::Village,
        });
    }

    // Market stalls along one edge, all square-on to the square.
    for i in 0..3 {
        let Some(m) = nth(&stalls, i) else { break };
        let pos = vec3f(VILLAGE.x - 8.0 + i as f32 * 8.0, 0.0, VILLAGE.y + 9.5);
        w.placements.push(Placement {
            model: m.clone(),
            pos,
            yaw: core::f32::consts::PI,
            target_h: 3.0,
            blocking: Blocking::Solid,
            region: Region::Village,
        });
        w.pois.push(PoiPlan {
            pos: vec3f(pos.x, 0.0, pos.z - 2.6),
            tag: "market",
            capacity: 2,
        });
    }

    // Barrels and crates against the stalls: things to bump into, and the
    // clutter that makes a place look used.
    for i in 0..6 {
        let Some(m) = nth(&barrels, i) else { break };
        let pos = vec3f(
            VILLAGE.x - 10.0 + i as f32 * 4.2 + rng.range(-0.6, 0.6),
            0.0,
            VILLAGE.y + 12.4 + rng.range(-0.5, 0.5),
        );
        w.placements.push(Placement {
            model: m.clone(),
            pos,
            yaw: rng.range(0.0, core::f32::consts::TAU),
            target_h: 1.0,
            blocking: Blocking::Solid,
            region: Region::Village,
        });
        w.interactables.push(Interactable {
            pos,
            kind: "barrel",
            region: Region::Village,
        });
    }

    // A fence bounding the garden strip behind the houses — a fence that
    // encloses something reads as a boundary; one crossing open ground reads
    // as debris left by a generator.
    if let Some(m) = nth(&fences, 0) {
        let (x0, z0) = (VILLAGE.x - 24.0, VILLAGE.y - 26.0);
        for i in 0..14 {
            w.placements.push(Placement {
                model: m.clone(),
                pos: vec3f(x0 + i as f32 * 2.0, 0.0, z0),
                yaw: 0.0,
                target_h: 1.3,
                blocking: Blocking::Solid,
                region: Region::Village,
            });
        }
        for i in 0..8 {
            w.placements.push(Placement {
                model: m.clone(),
                pos: vec3f(x0, 0.0, z0 + i as f32 * 2.0),
                yaw: core::f32::consts::FRAC_PI_2,
                target_h: 1.3,
                blocking: Blocking::Solid,
                region: Region::Village,
            });
        }
    }

    // Civilians. The 7-joint Kenney cast is the right one here — a village
    // should be people, not nine fantasy heroes.
    let cast = civilian_cast(index);
    for i in 0..8 {
        let a = i as f32 * core::f32::consts::TAU / 8.0;
        let pos = vec3f(VILLAGE.x + a.sin() * 7.0, 0.9, VILLAGE.y + a.cos() * 7.0);
        w.npcs.push(NpcPlan {
            pos,
            character: cast.1.get(i % cast.1.len().max(1)).cloned(),
            joints: cast.0,
            home: pos,
            region: Region::Village,
            speed: 2.4 + rng.range(-0.3, 0.5),
        });
    }
}

/// The castle: a wall run with towers and a gate, on the ridge north-east.
///
/// It is deliberately the tallest thing in the world. A landmark visible from
/// the village is what lets a player navigate without a map.
fn castle(index: &AssetIndex, seed: u64, rng: &mut GenRng, w: &mut WorldPlan) {
    // castle-kit or nothing: a keep assembled from a graveyard wall, a
    // tower-defense base and an arena gate is four art styles in one
    // silhouette, and the landmark is the one thing that must read cleanly.
    const CASTLE_KIT: &[&str] = &["castle-kit"];
    let walls = pick_from(index, "wall", CASTLE_KIT, 3, Spread::Variants, seed ^ 21);
    let towers = pick_from(index, "tower", CASTLE_KIT, 3, Spread::Kinds, seed ^ 23);
    let gates = pick_from(index, "gate", CASTLE_KIT, 2, Spread::Variants, seed ^ 27);
    let flags = pick_from(index, "flag banner", CASTLE_KIT, 2, Spread::Kinds, seed ^ 29);

    let half = 16.0f32;
    // Four wall runs with a gap on the village-facing side for the gate.
    let wall_at = |x: f32, z: f32, yaw: f32, w: &mut WorldPlan, i: usize| {
        if let Some(m) = nth(&walls, i) {
            w.placements.push(Placement {
                model: m.clone(),
                pos: vec3f(CASTLE.x + x, 0.0, CASTLE.y + z),
                yaw,
                target_h: 6.0,
                blocking: Blocking::Solid,
                region: Region::Castle,
            });
        }
    };
    let step = 4.0f32;
    let n = (half * 2.0 / step) as i32;
    for i in 0..n {
        let t = -half + i as f32 * step + step * 0.5;
        // North and south runs.
        wall_at(t, -half, 0.0, w, i as usize);
        // South run has the gateway: skip the middle two panels.
        if !(-step..=step).contains(&t) {
            wall_at(t, half, 0.0, w, i as usize + 1);
        }
        // East and west runs.
        wall_at(-half, t, core::f32::consts::FRAC_PI_2, w, i as usize + 2);
        wall_at(half, t, core::f32::consts::FRAC_PI_2, w, i as usize + 3);
    }
    // Corner towers, taller than the walls so the silhouette reads as a keep.
    for (i, (cx, cz)) in [(-half, -half), (half, -half), (-half, half), (half, half)]
        .into_iter()
        .enumerate()
    {
        if let Some(m) = nth(&towers, i) {
            w.placements.push(Placement {
                model: m.clone(),
                pos: vec3f(CASTLE.x + cx, 0.0, CASTLE.y + cz),
                yaw: 0.0,
                target_h: 11.0,
                blocking: Blocking::Solid,
                region: Region::Castle,
            });
        }
    }
    // The gate itself, facing the road in.
    if let Some(m) = nth(&gates, 0) {
        let pos = vec3f(CASTLE.x, 0.0, CASTLE.y + half);
        w.placements.push(Placement {
            model: m.clone(),
            pos,
            yaw: 0.0,
            // Not Solid: the gateway is the way in. A castle you cannot enter
            // is a wall with ambition.
            target_h: 6.5,
            blocking: Blocking::None,
            region: Region::Castle,
        });
        w.interactables.push(Interactable {
            pos,
            kind: "gate",
            region: Region::Castle,
        });
        w.pois.push(PoiPlan {
            pos: vec3f(pos.x, 0.0, pos.z + 4.0),
            tag: "gate",
            capacity: 2,
        });
    }
    // A keep in the courtyard so the inside is worth entering.
    if let Some(m) = nth(&towers, 1) {
        w.placements.push(Placement {
            model: m.clone(),
            pos: v3(CASTLE, 0.0),
            yaw: 0.4,
            target_h: 14.0,
            blocking: Blocking::Solid,
            region: Region::Castle,
        });
    }
    for (i, (fx, fz)) in [(-6.0f32, -6.0f32), (6.0, -6.0)].into_iter().enumerate() {
        if let Some(m) = nth(&flags, i) {
            w.placements.push(Placement {
                model: m.clone(),
                pos: vec3f(CASTLE.x + fx, 0.0, CASTLE.y + fz),
                yaw: rng.range(0.0, 1.0),
                target_h: 4.5,
                blocking: Blocking::None,
                region: Region::Castle,
            });
        }
    }
    // Guards on a beat inside the walls: the same NPC block, slower, with the
    // courtyard as home so they patrol rather than wander into the woods.
    let cast = hero_cast(index);
    for i in 0..3 {
        let a = i as f32 * core::f32::consts::TAU / 3.0;
        let pos = vec3f(CASTLE.x + a.sin() * 9.0, 0.9, CASTLE.y + a.cos() * 9.0);
        w.npcs.push(NpcPlan {
            pos,
            character: cast.1.get(i % cast.1.len().max(1)).cloned(),
            joints: cast.0,
            home: v3(CASTLE, 0.9),
            region: Region::Castle,
            speed: 1.9,
        });
    }
    w.pois.push(PoiPlan {
        pos: v3(CASTLE + vec2f(0.0, 8.0), 0.0),
        tag: "courtyard",
        capacity: 4,
    });
}

/// Woodland: mixed species AND variants, scattered with rules so nothing lands
/// on the roads or inside the village.
fn woods(index: &AssetIndex, seed: u64, w: &mut WorldPlan) {
    const NATURE: &[&str] = &["nature-kit"];
    let trees = pick_from(index, "pine tree conifer", NATURE, 5, Spread::Kinds, seed ^ 31);
    let rocks = pick_from(index, "rock boulder", NATURE, 4, Spread::Variants, seed ^ 37);
    if trees.is_empty() {
        return;
    }

    // Keep the wood off the roads and out of the built regions. A density
    // function is the right lever: the scatter rejects rather than placing and
    // then deleting, so spacing stays honest near the edges.
    let clear_of = |x: f32, z: f32| -> f32 {
        let p = vec2f(x, z);
        for (c, r) in [
            (VILLAGE, 30.0f32),
            (CASTLE, 26.0),
            (DUNGEON, 22.0),
            (QUARRY, 20.0),
            (CROSSROAD, 12.0),
        ] {
            if dist2(p, c) < r * r {
                return 0.0;
            }
        }
        // Thin out along the road corridors so a path stays legible.
        for (a, b) in [
            (VILLAGE, CROSSROAD),
            (CROSSROAD, CASTLE),
            (CROSSROAD, DUNGEON),
            (VILLAGE, WOODS),
            (VILLAGE, QUARRY),
        ] {
            if point_near_segment(p, a, b) < 7.0 {
                return 0.0;
            }
        }
        // Densest around the woods centre, thinning outward, so the map has a
        // forest rather than uniform sprinkling.
        let d = dist2(p, WOODS).sqrt();
        (1.0 - d / 120.0).clamp(0.12, 1.0)
    };

    let placements = scatter(&ScatterParams {
        seed: seed ^ 0x5EED,
        spacing: 7.0,
        extent: vec2f(150.0, 130.0),
        max_count: 260,
        scale_range: (0.8, 1.35),
        variants: trees.len().max(1) as u32,
        density_at: Some(&clear_of),
        ..Default::default()
    });
    for p in &placements {
        let Some(m) = nth(&trees, p.variant as usize) else {
            break;
        };
        w.placements.push(Placement {
            model: m.clone(),
            // Scatter is centred on the origin; shift it onto the map.
            pos: vec3f(p.pos.x - 18.0, 0.0, p.pos.z - 10.0),
            yaw: p.yaw,
            target_h: 6.5 * p.scale,
            blocking: Blocking::Trunk,
            region: Region::Woods,
        });
    }

    // Rocks at the wood's edge, sparser and larger.
    let rock_places = scatter(&ScatterParams {
        seed: seed ^ 0x0C_1234,
        spacing: 16.0,
        extent: vec2f(140.0, 120.0),
        max_count: 40,
        scale_range: (0.7, 1.5),
        variants: rocks.len().max(1) as u32,
        density_at: Some(&clear_of),
        ..Default::default()
    });
    for p in &rock_places {
        let Some(m) = nth(&rocks, p.variant as usize) else {
            break;
        };
        w.placements.push(Placement {
            model: m.clone(),
            pos: vec3f(p.pos.x - 18.0, 0.0, p.pos.z - 10.0),
            yaw: p.yaw,
            target_h: 1.6 * p.scale,
            blocking: Blocking::Solid,
            region: Region::Woods,
        });
    }
    w.pois.push(PoiPlan {
        pos: v3(WOODS + vec2f(8.0, 8.0), 0.0),
        tag: "clearing",
        capacity: 3,
    });
}

/// Distance from a point to a segment, for keeping scatter off the roads.
fn point_near_segment(p: Vec2f, a: Vec2f, b: Vec2f) -> f32 {
    let (abx, abz) = (b.x - a.x, b.y - a.y);
    let (apx, apz) = (p.x - a.x, p.y - a.y);
    let len2 = abx * abx + abz * abz;
    let t = if len2 <= 1e-6 {
        0.0
    } else {
        ((apx * abx + apz * abz) / len2).clamp(0.0, 1.0)
    };
    let (cx, cz) = (a.x + abx * t, a.y + abz * t);
    ((p.x - cx).powi(2) + (p.y - cz).powi(2)).sqrt()
}

/// A dungeon laid out with the BSP generator, its rooms and corridors placed
/// as real tiles, with an entrance a player can walk into from the road.
///
/// modular-dungeon / -cave / -space have IDENTICAL role histograms, so the kit
/// choice here is pure theming — the same call builds a cave.
fn dungeon(index: &AssetIndex, seed: u64, w: &mut WorldPlan) {
    let Some(kit) = kit_from_index(index, "modular-dungeon-kit", 0.4) else {
        return;
    };
    let level = levelgen::dungeon(
        &kit,
        &DungeonParams {
            seed,
            extent: (10, 10),
            min_room: 3,
            depth: 3,
        },
    );
    // Place it sunken and offset so it reads as a ruin floor open to the sky
    // rather than a slab dropped on the grass.
    let origin = vec3f(DUNGEON.x - 20.0, -0.35, DUNGEON.y - 20.0);
    for layer in &level.layers {
        w.stats.tiles += layer.placements.len();
        for p in &layer.placements {
            let Some(t) = kit.tiles.get(p.tile as usize) else {
                continue;
            };
            w.placements.push(Placement {
                model: t.id.clone(),
                pos: vec3f(origin.x + p.pos.x, origin.y, origin.z + p.pos.z),
                yaw: p.yaw(),
                target_h: 3.2,
                // Corridor and room pieces are walls as much as floors; letting
                // them block is what makes the ruin something to walk THROUGH
                // rather than over.
                blocking: Blocking::Solid,
                region: Region::Dungeon,
            });
        }
    }
    let entrance = level
        .entrance
        .map(|e| vec3f(origin.x + e.x, 0.0, origin.z + e.z))
        .unwrap_or_else(|| v3(DUNGEON, 0.0));
    w.interactables.push(Interactable {
        pos: entrance,
        kind: "dungeon-entrance",
        region: Region::Dungeon,
    });
    w.pois.push(PoiPlan {
        pos: entrance,
        tag: "dungeon",
        capacity: 2,
    });
    // Something hostile loitering at the mouth: the undead share the 41-joint
    // rig, so they animate through the same path as the guards.
    let cast = hero_cast(index);
    let undead: Vec<String> = cast
        .1
        .iter()
        .filter(|id| id.to_lowercase().contains("skeleton"))
        .cloned()
        .collect();
    let pool = if undead.is_empty() { cast.1.clone() } else { undead };
    for i in 0..3 {
        let a = i as f32 * 2.1;
        let pos = vec3f(entrance.x + a.sin() * 6.0, 0.9, entrance.z + a.cos() * 6.0);
        w.npcs.push(NpcPlan {
            pos,
            character: pool.get(i % pool.len().max(1)).cloned(),
            joints: cast.0,
            home: entrance,
            region: Region::Dungeon,
            speed: 2.1,
        });
    }
}

/// The quarry: where the rigid-body demo lives, dressed so it belongs.
///
/// The physics stack stays visible and playable — it is the only part of the
/// world that moves under its own rules — but a stack of crates in a worked
/// pit reads as a place, and the same stack on open grass reads as a test
/// harness.
fn quarry(index: &AssetIndex, seed: u64, rng: &mut GenRng, w: &mut WorldPlan) {
    const NATURE: &[&str] = &["nature-kit"];
    let rocks = pick_from(index, "rock boulder", NATURE, 4, Spread::Variants, seed ^ 41);
    let crates = pick_from(index, "crate barrel wooden", &["survival-kit", "fantasy-town-kit"], 3, Spread::Kinds, seed ^ 43);
    let fences = pick_from(index, "fence panel wooden", NATURE, 2, Spread::Variants, seed ^ 47);

    // A rock rim, so the pit has an edge.
    for i in 0..14 {
        let a = i as f32 * core::f32::consts::TAU / 14.0;
        let Some(m) = nth(&rocks, i) else { break };
        w.placements.push(Placement {
            model: m.clone(),
            pos: vec3f(
                QUARRY.x + a.sin() * 15.0 + rng.range(-1.0, 1.0),
                0.0,
                QUARRY.y + a.cos() * 15.0 + rng.range(-1.0, 1.0),
            ),
            yaw: rng.range(0.0, core::f32::consts::TAU),
            target_h: 2.2 + rng.range(0.0, 1.4),
            blocking: Blocking::Solid,
            region: Region::Quarry,
        });
    }
    // Stacked crates beside the working face.
    for i in 0..5 {
        let Some(m) = nth(&crates, i) else { break };
        w.placements.push(Placement {
            model: m.clone(),
            pos: vec3f(QUARRY.x - 5.0 + i as f32 * 2.4, 0.0, QUARRY.y + 6.0),
            yaw: rng.range(-0.3, 0.3),
            target_h: 1.1,
            blocking: Blocking::Solid,
            region: Region::Quarry,
        });
    }
    if let Some(m) = nth(&fences, 0) {
        for i in 0..10 {
            w.placements.push(Placement {
                model: m.clone(),
                pos: vec3f(QUARRY.x - 10.0 + i as f32 * 2.0, 0.0, QUARRY.y - 12.0),
                yaw: 0.0,
                target_h: 1.3,
                blocking: Blocking::Solid,
                region: Region::Quarry,
            });
        }
    }
    w.pois.push(PoiPlan {
        pos: v3(QUARRY, 0.0),
        tag: "quarry",
        capacity: 4,
    });
}

// ------------------------------------------------------------------- cast

/// The civilian cast: the largest rig, which is Kenney's 7-joint townsfolk.
///
/// Returns (joints, ids). Grouping by joint count rather than pack is the
/// index's own choice and the useful one — everything on a rig is
/// interchangeable in animation code.
pub fn civilian_cast(index: &AssetIndex) -> (u32, Vec<String>) {
    let casts = index.casts();
    casts
        .iter()
        .filter(|c| c.joints < 20)
        .max_by_key(|c| c.members.len())
        .map(|c| (c.joints, c.members.clone()))
        .unwrap_or((0, Vec::new()))
}

/// The hero/undead cast: the 41-joint KayKit rig, richest in clips.
pub fn hero_cast(index: &AssetIndex) -> (u32, Vec<String>) {
    let casts = index.casts();
    casts
        .iter()
        .filter(|c| c.joints >= 20)
        .max_by_key(|c| c.max_clips)
        .map(|c| (c.joints, c.members.clone()))
        .unwrap_or((0, Vec::new()))
}

// --------------------------------------------------------------- realise

/// A collider the realise step wants spawned. Separate from the visual
/// instance because a prop's silhouette and its obstruction are not the same
/// shape — a house is walls with a gap where its door is.
#[derive(Clone, Debug)]
pub struct PlacedCollider {
    pub pos: Vec3f,
    pub half: Vec3f,
}

/// What realising a plan produced.
#[derive(Clone, Default)]
pub struct Realised {
    pub instances: Vec<ModelInstance>,
    pub colliders: Vec<PlacedCollider>,
    pub triangles: usize,
    /// Placements whose model would not load — a pack whose atlas never
    /// downloaded, most often. Reported rather than silently dropped, because
    /// a hole in a region is otherwise indistinguishable from a layout bug.
    pub missing: Vec<String>,
}

/// Load every model the plan asked for and turn placements into draw
/// instances plus colliders.
///
/// `load` is a callback rather than a `&mut GameRenderer` so this stays
/// testable and so the caller keeps control of its own GPU state; it returns
/// the model's bounds and its per-primitive collider boxes once loaded, or
/// `None` if the model is unavailable.
pub fn realise(
    plan: &WorldPlan,
    mut load: impl FnMut(&str) -> Option<ModelGeometry>,
) -> Realised {
    let mut out = Realised::default();
    let mut seen: std::collections::HashMap<String, Option<ModelGeometry>> =
        std::collections::HashMap::new();

    for p in &plan.placements {
        let geo = seen
            .entry(p.model.clone())
            .or_insert_with(|| load(&p.model))
            .clone();
        let Some(geo) = geo else {
            if !out.missing.contains(&p.model) {
                out.missing.push(p.model.clone());
            }
            continue;
        };

        // Scale from the model's OWN bounds to the requested height, then cap
        // the footprint. Scaling by height alone assumes a model is roughly as
        // tall as it is wide; a short wide one — a ground patch, a fallen log —
        // otherwise blows up sideways into a coloured slab. Any library picked
        // by description eventually returns something oddly proportioned, so
        // the guard belongs here rather than in a list of models to avoid.
        let native_h = (geo.max.y - geo.min.y).max(0.001);
        let mut s = p.target_h / native_h;
        let native_w = (geo.max.x - geo.min.x).max(geo.max.z - geo.min.z).max(0.001);
        let max_w = p.target_h * 2.5;
        if native_w * s > max_w {
            s = max_w / native_w;
        }

        let (sin, cos) = (p.yaw.sin(), p.yaw.cos());
        let mut m = Mat4f::identity();
        m.v[0] = cos * s;
        m.v[2] = -sin * s;
        m.v[5] = s;
        m.v[8] = sin * s;
        m.v[10] = cos * s;
        // Sit the model on the ground: its own minimum, scaled, is its feet.
        m.v[12] = p.pos.x;
        m.v[13] = p.pos.y - geo.min.y * s;
        m.v[14] = p.pos.z;
        out.instances.push(ModelInstance {
            model: p.model.clone(),
            transform: m,
        });
        out.triangles += geo.triangles;

        if p.blocking == Blocking::None {
            continue;
        }
        let mut pushed = 0;
        for (a, b) in &geo.collider_parts {
            let (ex, ey, ez) = (
                (b.x - a.x) * 0.5 * s,
                (b.y - a.y) * 0.5 * s,
                (b.z - a.z) * 0.5 * s,
            );
            let (cx, cy, cz) = (
                (a.x + b.x) * 0.5 * s,
                (a.y + b.y) * 0.5 * s - geo.min.y * s,
                (a.z + b.z) * 0.5 * s,
            );
            // Trunk-only: a canopy sits high and wide, a trunk low and narrow.
            // Keeping the narrow low part is what lets a walker pass under
            // branches but not through the tree.
            if p.blocking == Blocking::Trunk {
                let slim = ex.max(ez) < native_w * 0.5 * s * 0.45;
                let low = cy < p.target_h * 0.6;
                if !(slim && low) {
                    continue;
                }
            }
            // Rotate the offset with the prop.
            let (ox, oz) = (cx * cos + cz * sin, -cx * sin + cz * cos);
            out.colliders.push(PlacedCollider {
                pos: vec3f(p.pos.x + ox, p.pos.y + cy.max(ey), p.pos.z + oz),
                half: vec3f(ex.max(0.08), ey.max(0.08), ez.max(0.08)),
            });
            pushed += 1;
        }
        // A model whose parts all filtered out still has to be solid, or the
        // prop silently becomes scenery again — which is the exact bug this
        // exists to prevent. Trees need it most: a pine modelled as one merged
        // mesh has no separate trunk to find.
        if pushed == 0 {
            let (frac, hy) = match p.blocking {
                Blocking::Trunk => (0.16, p.target_h * 0.35),
                _ => (0.5, p.target_h * 0.5),
            };
            let hw = (native_w * s * frac).max(0.18);
            out.colliders.push(PlacedCollider {
                pos: vec3f(p.pos.x, p.pos.y + hy, p.pos.z),
                half: vec3f(hw, hy, hw),
            });
        }
    }
    out
}

/// What `realise` needs to know about a loaded model.
#[derive(Clone, Debug)]
pub struct ModelGeometry {
    pub min: Vec3f,
    pub max: Vec3f,
    pub triangles: usize,
    /// Per-primitive boxes in model space, so a house's doorway is a gap.
    pub collider_parts: Vec<(Vec3f, Vec3f)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> Option<AssetIndex> {
        let root = std::path::Path::new("resources");
        if !root.join("models/kenney").is_dir() {
            eprintln!("skipping: run apps/arcade/download_assets.sh");
            return None;
        }
        Some(AssetIndex::build(root))
    }

    #[test]
    fn tile_classification_separates_junction_from_crossroad() {
        // The index lumps both under "junction"; fitting needs them apart, or
        // a crossroad stands in for a tee and leaves a stub onto nothing.
        assert_eq!(classify_tile("road-crossroad"), Some(TileRole::Cross));
        assert_eq!(classify_tile("corridor-intersection"), Some(TileRole::Cross));
        assert_eq!(classify_tile("road-junction"), Some(TileRole::TJunction));
        assert_eq!(classify_tile("corridor-junction"), Some(TileRole::TJunction));
        assert_eq!(classify_tile("road-bend"), Some(TileRole::Corner));
        assert_eq!(classify_tile("corridor-corner"), Some(TileRole::Corner));
        assert_eq!(classify_tile("corridor-end"), Some(TileRole::End));
        assert_eq!(classify_tile("corridor"), Some(TileRole::Straight));
        assert_eq!(classify_tile("gate-door"), Some(TileRole::Door));
        // Decoration must NOT be fitted into a road run.
        assert_eq!(classify_tile("construction-cone"), None);
        assert_eq!(classify_tile("light-square"), None);
    }

    #[test]
    fn corner_beats_the_generic_road_prefix() {
        // `road-curve-intersection` contains "curve" AND "intersection"; the
        // four-way test must win or a crossroad becomes a bend and the run
        // breaks.
        assert_eq!(
            classify_tile("road-curve-intersection"),
            Some(TileRole::Cross)
        );
    }

    #[test]
    fn segment_distance_is_sane() {
        let a = vec2f(0.0, 0.0);
        let b = vec2f(10.0, 0.0);
        assert!(point_near_segment(vec2f(5.0, 3.0), a, b) - 3.0 < 0.001);
        // Past the end clamps to the endpoint rather than extending the line.
        assert!(point_near_segment(vec2f(20.0, 0.0), a, b) - 10.0 < 0.001);
    }

    #[test]
    fn plan_is_deterministic() {
        let Some(idx) = index() else { return };
        let a = plan(&idx, 42);
        let b = plan(&idx, 42);
        assert_eq!(a.placements.len(), b.placements.len());
        for (x, y) in a.placements.iter().zip(b.placements.iter()) {
            assert_eq!(x.model, y.model);
            assert_eq!(x.pos.x.to_bits(), y.pos.x.to_bits());
            assert_eq!(x.pos.z.to_bits(), y.pos.z.to_bits());
        }
        let c = plan(&idx, 43);
        assert_ne!(a.placements.len(), c.placements.len(), "seed did nothing");
    }

    #[test]
    fn every_region_is_populated() {
        let Some(idx) = index() else { return };
        let w = plan(&idx, 7);
        for (region, n) in &w.stats.per_region {
            assert!(
                *n > 0,
                "region {} is empty — its queries resolved nothing",
                region.name()
            );
        }
    }

    #[test]
    fn the_world_uses_many_distinct_models() {
        let Some(idx) = index() else { return };
        let w = plan(&idx, 7);
        // The whole point of the variety work: a world that places one house
        // six times is the failure this guards against.
        assert!(
            w.stats.distinct_models >= 20,
            "only {} distinct models — the scene is copy-pasted",
            w.stats.distinct_models
        );
    }

    #[test]
    fn regions_do_not_overlap_each_other() {
        let Some(idx) = index() else { return };
        let w = plan(&idx, 7);
        // A castle wall standing in the village square means the geography
        // constants collided. Check each region's props stay near its centre.
        let centre = |r: Region| match r {
            Region::Village => Some(VILLAGE),
            Region::Castle => Some(CASTLE),
            Region::Quarry => Some(QUARRY),
            _ => None,
        };
        for p in &w.placements {
            let Some(c) = centre(p.region) else { continue };
            let d = dist2(vec2f(p.pos.x, p.pos.z), c).sqrt();
            assert!(
                d < 42.0,
                "{} prop {} is {d:.1} from its region centre",
                p.region.name(),
                p.model
            );
        }
    }

    #[test]
    fn woods_keep_clear_of_the_roads_and_the_built_regions() {
        let Some(idx) = index() else { return };
        let w = plan(&idx, 7);
        for p in w.placements.iter().filter(|p| p.region == Region::Woods) {
            let q = vec2f(p.pos.x, p.pos.z);
            assert!(
                dist2(q, VILLAGE).sqrt() > 25.0,
                "a tree grew in the village square"
            );
            let road = point_near_segment(q, VILLAGE, CROSSROAD)
                .min(point_near_segment(q, CROSSROAD, CASTLE))
                .min(point_near_segment(q, CROSSROAD, DUNGEON));
            assert!(road > 5.0, "a tree is standing in the road at {road:.1}");
        }
    }

    #[test]
    fn each_region_is_drawn_from_one_art_pack() {
        let Some(idx) = index() else { return };
        let w = plan(&idx, 7);
        // The junk-drawer failure reached from the other direction: not one
        // model repeated, but every model from a different pack. A castle made
        // of a graveyard wall, a tower-defense base and an arena gate is four
        // art styles in one silhouette.
        for region in [Region::Castle, Region::Woods, Region::Dungeon] {
            let mut packs: Vec<&str> = w
                .placements
                .iter()
                .filter(|p| p.region == region)
                .filter_map(|p| p.model.split('/').nth(1))
                .collect();
            packs.sort_unstable();
            packs.dedup();
            assert!(
                packs.len() <= 1,
                "{} draws from {} packs: {packs:?}",
                region.name(),
                packs.len()
            );
        }
    }

    #[test]
    fn a_walker_can_get_from_the_village_to_the_castle() {
        use makepad_game_sim::{step_world, BodyKind, Entity, GameWorld};
        let Some(idx) = index() else { return };
        let w = plan(&idx, 7);

        // Realise against nominal boxes: this tests the LAYOUT's walkability
        // (does a wall ring seal a region?) rather than any model's exact
        // silhouette, which is the part a plan can actually get wrong.
        let r = realise(&w, |_| {
            Some(ModelGeometry {
                min: vec3f(-0.5, 0.0, -0.5),
                max: vec3f(0.5, 1.0, 0.5),
                triangles: 10,
                collider_parts: Vec::new(),
            })
        });

        let mut world = GameWorld::new();
        world.reset_content();
        let mut id = 0u64;
        let mut push = |world: &mut GameWorld, kind, pos: Vec3f, half: Vec3f, id: &mut u64| {
            *id += 1;
            world.push_entity(Entity {
                id: *id,
                kind,
                pos,
                half,
                collide: true,
                gravity_scale: 1.0,
                speed_mult: 1.0,
                scale: vec3f(1.0, 1.0, 1.0),
                scale_target: vec3f(1.0, 1.0, 1.0),
                density: 1.0,
                friction: 0.6,
                ..Default::default()
            });
        };
        push(&mut world, BodyKind::Static, vec3f(0.0, -0.5, 0.0), vec3f(220.0, 0.5, 220.0), &mut id);
        for c in &r.colliders {
            push(&mut world, BodyKind::Static, c.pos, c.half, &mut id);
        }
        let walker_id = id + 1;
        push(&mut world, BodyKind::Mover, vec3f(w.player_start.x, 0.9, w.player_start.z), vec3f(0.3, 0.9, 0.3), &mut id);

        // Walk toward the castle, steering around whatever blocks — the same
        // sidestep an NPC would do. Arriving proves the regions are connected
        // and that nothing walled the player in at the start.
        let target = vec2f(CASTLE.x, CASTLE.y);
        let mut best = f32::INFINITY;
        for tick in 0..4000 {
            let p = world.entity(walker_id).map(|e| e.pos).unwrap();
            let here = vec2f(p.x, p.z);
            let d = dist2(here, target).sqrt();
            best = best.min(d);
            if d < 22.0 {
                return; // reached the castle grounds
            }
            let (dx, dz) = (target.x - here.x, target.y - here.y);
            let inv = 1.0 / d.max(0.001);
            // A slow oscillation across the goal direction gets round static
            // obstacles without implementing pathfinding in a test.
            let wob = ((tick as f32) * 0.04).sin() * 0.75;
            if let Some(e) = world.entity_mut(walker_id) {
                e.vel.x = (dx * inv - dz * inv * wob) * 6.0;
                e.vel.z = (dz * inv + dx * inv * wob) * 6.0;
            }
            step_world(&mut world);
        }
        panic!("walker never reached the castle; closest approach {best:.1} units");
    }

    #[test]
    fn there_are_things_to_do_and_people_to_meet() {
        let Some(idx) = index() else { return };
        let w = plan(&idx, 7);
        assert!(w.npcs.len() >= 10, "only {} inhabitants", w.npcs.len());
        assert!(
            w.interactables.len() >= 5,
            "only {} things to interact with",
            w.interactables.len()
        );
        // POIs are what stop NPCs degrading to aimless wandering.
        assert!(w.pois.len() >= 10, "only {} destinations", w.pois.len());
        // The cast must be varied: a world of one character is the same
        // failure as a street of one house.
        let mut chars: Vec<&str> = w
            .npcs
            .iter()
            .filter_map(|n| n.character.as_deref())
            .collect();
        chars.sort_unstable();
        chars.dedup();
        assert!(chars.len() >= 4, "only {} distinct characters", chars.len());
    }
}

#[cfg(test)]
mod realise_tests {
    use super::*;

    fn geo(w: f32, h: f32, parts: Vec<(Vec3f, Vec3f)>) -> ModelGeometry {
        ModelGeometry {
            min: vec3f(-w * 0.5, 0.0, -w * 0.5),
            max: vec3f(w * 0.5, h, w * 0.5),
            triangles: 100,
            collider_parts: parts,
        }
    }

    fn one(model: &str, blocking: Blocking, target_h: f32) -> WorldPlan {
        WorldPlan {
            placements: vec![Placement {
                model: model.to_string(),
                pos: vec3f(10.0, 0.0, -4.0),
                yaw: 0.0,
                target_h,
                blocking,
                region: Region::Village,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn a_prop_is_scaled_to_its_target_height_and_stands_on_the_ground() {
        // Native 4 units tall, asked for 2: scale 0.5, and its feet — not its
        // centre — sit at y = 0.
        let r = realise(&one("m", Blocking::Solid, 2.0), |_| Some(geo(2.0, 4.0, vec![])));
        assert_eq!(r.instances.len(), 1);
        let m = &r.instances[0].transform;
        assert!((m.v[5] - 0.5).abs() < 1e-5, "scale {}", m.v[5]);
        assert!(m.v[13].abs() < 1e-5, "feet at y={}", m.v[13]);
    }

    #[test]
    fn a_short_wide_model_cannot_blow_up_sideways() {
        // 20 wide, 1 tall, asked for 6 high. Height alone would scale x6 and
        // give a 120-unit slab lying across the world.
        let r = realise(&one("m", Blocking::Solid, 6.0), |_| Some(geo(20.0, 1.0, vec![])));
        let s = r.instances[0].transform.v[5];
        assert!(20.0 * s <= 6.0 * 2.5 + 0.01, "footprint {} too wide", 20.0 * s);
    }

    #[test]
    fn a_solid_prop_always_gets_a_collider_even_with_no_usable_parts() {
        // The silent-scenery regression: a model whose primitives all filter
        // out must still stop a walker.
        let r = realise(&one("m", Blocking::Solid, 3.0), |_| Some(geo(2.0, 4.0, vec![])));
        assert_eq!(r.colliders.len(), 1);
        assert!(r.colliders[0].half.y > 0.5);
    }

    #[test]
    fn a_tree_keeps_its_trunk_and_drops_its_canopy() {
        // Trunk: narrow and low. Canopy: wide and high. Only the first blocks,
        // or a wood becomes impassable and reads as invisible walls.
        let trunk = (vec3f(-0.2, 0.0, -0.2), vec3f(0.2, 2.0, 0.2));
        let canopy = (vec3f(-3.0, 2.0, -3.0), vec3f(3.0, 6.0, 3.0));
        let r = realise(&one("t", Blocking::Trunk, 6.0), |_| {
            Some(geo(6.0, 6.0, vec![trunk, canopy]))
        });
        assert_eq!(r.colliders.len(), 1, "canopy should not block");
        assert!(r.colliders[0].half.x < 0.6, "kept the canopy box");
    }

    #[test]
    fn scenery_gets_no_collider_at_all() {
        let r = realise(&one("lamp", Blocking::None, 3.0), |_| Some(geo(1.0, 3.0, vec![])));
        assert!(r.colliders.is_empty());
    }

    #[test]
    fn an_unloadable_model_is_reported_not_silently_dropped() {
        let r = realise(&one("gone", Blocking::Solid, 2.0), |_| None);
        assert!(r.instances.is_empty());
        assert_eq!(r.missing, vec!["gone".to_string()]);
    }

    #[test]
    fn each_model_is_loaded_once_however_often_it_is_placed() {
        // A forest of one tree must not re-parse the GLB per instance.
        let mut plan = WorldPlan::default();
        for i in 0..25 {
            plan.placements.push(Placement {
                model: "tree".to_string(),
                pos: vec3f(i as f32, 0.0, 0.0),
                yaw: 0.0,
                target_h: 5.0,
                blocking: Blocking::None,
                region: Region::Woods,
            });
        }
        let mut loads = 0;
        let r = realise(&plan, |_| {
            loads += 1;
            Some(geo(2.0, 4.0, vec![]))
        });
        assert_eq!(loads, 1, "loaded {loads} times");
        assert_eq!(r.instances.len(), 25);
    }
}
