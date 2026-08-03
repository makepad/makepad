//! Level generators: the layout half of "procedural layout + authored tiles".
//!
//! Each generator decides an occupancy grid and hands it to [`fit_grid`], which
//! turns cells into tiles. Nothing here knows a filename — swap the kit and the
//! same road network comes out in a different art style.
//!
//! Output is a [`Level`] of layers, one per kit, because a kit is also a
//! texture atlas: batching per layer is a draw call per kit.

use crate::kit::*;
use crate::rng::GenRng;
use crate::spline::Spline;
use makepad_math::*;

/// Tiles from one kit. A caller draws a layer as a single batch.
#[derive(Clone, Debug)]
pub struct LevelLayer {
    pub kit_id: String,
    pub tile_size: f32,
    pub placements: Vec<TilePlacement>,
}

/// A generated level.
#[derive(Clone, Debug, Default)]
pub struct Level {
    pub layers: Vec<LevelLayer>,
    /// Traversable cells of the primary layer, for spawn points and pathing.
    pub open_cells: Vec<(i32, i32)>,
    pub entrance: Option<Vec3f>,
    pub exit: Option<Vec3f>,
}

impl Level {
    pub fn placement_count(&self) -> usize {
        self.layers.iter().map(|l| l.placements.len()).sum()
    }

    /// Add placements, merging into an existing layer from the same kit.
    ///
    /// Merging is not tidiness — a kit is a texture atlas, so tiles from one
    /// kit belong in one batch however they were generated. It also keeps tile
    /// indices unambiguous: indices are kit-relative, so two layers claiming
    /// the same kit id would be impossible to resolve. Real packs make this
    /// concrete — Kenney's `city-kit-roads` holds both the road tiles and the
    /// cones and streetlights.
    fn add(&mut self, kit_id: &str, tile_size: f32, placements: Vec<TilePlacement>) {
        if placements.is_empty() {
            return;
        }
        if let Some(l) = self.layers.iter_mut().find(|l| l.kit_id == kit_id) {
            l.placements.extend(placements);
        } else {
            self.layers.push(LevelLayer {
                kit_id: kit_id.to_string(),
                tile_size,
                placements,
            });
        }
    }

    fn push(&mut self, layout: TileLayout) {
        self.add(&layout.kit_id, layout.tile_size, layout.placements);
    }
}

// ---------------------------------------------------------------- roads

/// How a road network should be laid out.
pub struct RoadParams<'a> {
    pub seed: u64,
    /// Polylines in world space. Each is rasterised onto the kit grid and they
    /// share junctions where they cross — which is what produces T-junctions
    /// and crossroads without anyone asking for them.
    pub paths: &'a [Vec<Vec3f>],
}

/// Rasterise polylines onto the kit's grid and fit tiles.
///
/// Junction type is never specified by the caller: it falls out of how many
/// occupied neighbours a cell has, so two paths that cross produce a crossroad
/// and one that tees into another produces a T.
pub fn road_network(kit: &Kit, p: &RoadParams) -> Level {
    let mut grid = CellGrid::new();
    for path in p.paths {
        let cells: Vec<(i32, i32)> = path.iter().map(|v| world_to_cell(kit.tile_size, *v)).collect();
        for w in cells.windows(2) {
            // Step in X then Z so the run is 4-connected: a diagonal jump
            // would leave a gap that reads as a broken road.
            grid.line(w[0], w[1]);
        }
        if cells.len() == 1 {
            grid.set(cells[0]);
        }
    }
    let layout = fit_grid(kit, &grid, p.seed);
    let mut level = Level {
        open_cells: layout.open_cells.clone(),
        ..Default::default()
    };
    level.push(layout);
    level
}

/// Rasterise a spline as a road: the authored-tile counterpart to the ribbon
/// mesh in [`crate::spline`].
///
/// A ribbon gives smooth banking and arbitrary width; tiles give Kenney's
/// markings, kerbs and junctions for free. Both are offered because a kart
/// track wants the first and a city street wants the second.
pub fn road_from_spline(kit: &Kit, spline: &Spline, samples_per_segment: usize, seed: u64) -> Level {
    let frames = spline.frames(samples_per_segment.max(2));
    let path: Vec<Vec3f> = frames.iter().map(|f| f.pos).collect();
    road_network(
        kit,
        &RoadParams {
            seed,
            paths: &[path],
        },
    )
}

fn world_to_cell(tile_size: f32, v: Vec3f) -> (i32, i32) {
    let s = tile_size.max(0.001);
    ((v.x / s).round() as i32, (v.z / s).round() as i32)
}

// ---------------------------------------------------------------- town

pub struct TownParams<'a> {
    pub seed: u64,
    /// Town size in grid cells.
    pub extent: (i32, i32),
    /// Cells between parallel streets. Larger blocks mean fewer, longer roads.
    pub block: i32,
    /// Chance a buildable lot actually gets a building.
    pub density: f32,
    /// Kit supplying `Building` tiles. May be the same kit as the roads.
    pub buildings: Option<&'a Kit>,
    /// Kit supplying `Prop` tiles for street corners.
    pub props: Option<&'a Kit>,
}

impl<'a> Default for TownParams<'a> {
    fn default() -> Self {
        Self {
            seed: 0,
            extent: (24, 24),
            block: 4,
            density: 0.8,
            buildings: None,
            props: None,
        }
    }
}

/// A street grid with buildings on the lots between.
///
/// Buildings face the street they front, which is the difference between a town
/// and a field of boxes. Lots are the cells adjacent to a road, so a building
/// never lands in the middle of a block where nothing can reach it.
pub fn town(roads_kit: &Kit, p: &TownParams) -> Level {
    let mut rng = GenRng::new(p.seed ^ 0x7011_1000);
    let (w, h) = (p.extent.0.max(2), p.extent.1.max(2));
    let block = p.block.max(2);

    let mut grid = CellGrid::new();
    for x in 0..=w {
        if x % block == 0 {
            for z in 0..=h {
                grid.set((x, z));
            }
        }
    }
    for z in 0..=h {
        if z % block == 0 {
            for x in 0..=w {
                grid.set((x, z));
            }
        }
    }

    let roads = fit_grid(roads_kit, &grid, p.seed);
    let mut level = Level {
        open_cells: roads.open_cells.clone(),
        ..Default::default()
    };
    // Roads first: layer 0 is the primary structure a caller reasons about.
    level.push(roads);

    // Buildings on the cells that front a street.
    if let Some(bkit) = p.buildings {
        let building_tiles = bkit.by_role(TileRole::Building);
        if !building_tiles.is_empty() {
            let mut placements = Vec::new();
            for z in 0..=h {
                for x in 0..=w {
                    let cell = (x, z);
                    if grid.contains(cell) {
                        continue;
                    }
                    // Which side is the street? That decides the facing.
                    let m = grid.mask_at(cell);
                    if m == 0 || !rng.chance(p.density) {
                        continue;
                    }
                    // Face the first street found, in N/E/S/W order. A tile's
                    // canonical facing is north, so quarters count clockwise
                    // from there.
                    let quarters = if m & N != 0 {
                        0
                    } else if m & E != 0 {
                        1
                    } else if m & S != 0 {
                        2
                    } else {
                        3
                    };
                    let tile = building_tiles[rng.index(building_tiles.len())];
                    placements.push(TilePlacement {
                        tile,
                        cell,
                        pos: TileLayout::cell_to_world(bkit.tile_size, cell),
                        quarters,
                        // Buildings are not structural: they connect to nothing.
                        mask: 0,
                    });
                }
            }
            level.add(&bkit.id, bkit.tile_size, placements);
        }
    }

    // Props at intersections, where a real street has lamps and signs.
    if let Some(pkit) = p.props {
        let prop_tiles = pkit.by_role(TileRole::Prop);
        if !prop_tiles.is_empty() {
            let mut placements = Vec::new();
            for cell in grid.iter() {
                if grid.mask_at(cell).count_ones() < 3 || !rng.chance(0.5) {
                    continue;
                }
                let tile = prop_tiles[rng.index(prop_tiles.len())];
                placements.push(TilePlacement {
                    tile,
                    cell,
                    pos: TileLayout::cell_to_world(pkit.tile_size, cell),
                    quarters: rng.index(4) as u32,
                    mask: 0,
                });
            }
            level.add(&pkit.id, pkit.tile_size, placements);
        }
    }

    level
}

// ---------------------------------------------------------------- dungeon

pub struct DungeonParams {
    pub seed: u64,
    /// Bounds in grid cells.
    pub extent: (i32, i32),
    /// Stop splitting once a partition is smaller than this.
    pub min_room: i32,
    /// How many times to try splitting. More splits, more rooms.
    pub depth: u32,
}

impl Default for DungeonParams {
    fn default() -> Self {
        Self {
            seed: 0,
            extent: (32, 32),
            min_room: 5,
            depth: 4,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    x0: i32,
    z0: i32,
    x1: i32,
    z1: i32,
}

impl Rect {
    fn w(&self) -> i32 {
        self.x1 - self.x0
    }
    fn h(&self) -> i32 {
        self.z1 - self.z0
    }
    fn centre(&self) -> (i32, i32) {
        ((self.x0 + self.x1) / 2, (self.z0 + self.z1) / 2)
    }
}

/// Rooms joined by corridors, connectivity guaranteed by construction.
///
/// BSP splitting produces a binary tree of partitions; joining each node's two
/// children and recursing gives a spanning tree over the rooms, so every room
/// is reachable without needing a repair pass. The flood-fill test proves it
/// rather than trusting the argument.
pub fn dungeon(kit: &Kit, p: &DungeonParams) -> Level {
    let mut rng = GenRng::new(p.seed);
    let bounds = Rect {
        x0: 0,
        z0: 0,
        x1: p.extent.0.max(6),
        z1: p.extent.1.max(6),
    };

    // Split into partitions.
    let mut parts = vec![bounds];
    for _ in 0..p.depth {
        let mut next = Vec::with_capacity(parts.len() * 2);
        for r in parts {
            let can_h = r.h() >= p.min_room * 2;
            let can_v = r.w() >= p.min_room * 2;
            if !can_h && !can_v {
                next.push(r);
                continue;
            }
            let split_vertically = if can_h && can_v {
                // Split the longer axis, so rooms stay roughly square rather
                // than degenerating into slots.
                r.w() >= r.h()
            } else {
                can_v
            };
            if split_vertically {
                let lo = r.x0 + p.min_room;
                let hi = r.x1 - p.min_room;
                let at = lo + (rng.index((hi - lo).max(1) as usize) as i32);
                next.push(Rect { x1: at, ..r });
                next.push(Rect { x0: at, ..r });
            } else {
                let lo = r.z0 + p.min_room;
                let hi = r.z1 - p.min_room;
                let at = lo + (rng.index((hi - lo).max(1) as usize) as i32);
                next.push(Rect { z1: at, ..r });
                next.push(Rect { z0: at, ..r });
            }
        }
        parts = next;
    }

    // Carve a room inside each partition, inset so rooms never touch.
    let mut rooms: Vec<Rect> = Vec::with_capacity(parts.len());
    for r in &parts {
        if r.w() < 3 || r.h() < 3 {
            continue;
        }
        let mw = (r.w() - 2).max(2);
        let mh = (r.h() - 2).max(2);
        let rw = 2 + rng.index((mw - 1).max(1) as usize) as i32;
        let rh = 2 + rng.index((mh - 1).max(1) as usize) as i32;
        let ox = r.x0 + 1 + rng.index((r.w() - rw - 1).max(1) as usize) as i32;
        let oz = r.z0 + 1 + rng.index((r.h() - rh - 1).max(1) as usize) as i32;
        rooms.push(Rect {
            x0: ox,
            z0: oz,
            x1: ox + rw,
            z1: oz + rh,
        });
    }
    if rooms.is_empty() {
        return Level::default();
    }

    let mut floor = CellGrid::new();
    for r in &rooms {
        floor.rect((r.x0, r.z0), (r.x1, r.z1));
    }

    // Corridors: chain consecutive rooms. Sorting by centre first makes the
    // chain a spanning tree over spatially near rooms rather than a tangle
    // across the whole map.
    let mut order: Vec<usize> = (0..rooms.len()).collect();
    order.sort_by_key(|&i| {
        let c = rooms[i].centre();
        (c.1, c.0)
    });
    let mut corridors = CellGrid::new();
    for w in order.windows(2) {
        let a = rooms[w[0]].centre();
        let b = rooms[w[1]].centre();
        corridors.line(a, b);
    }

    // Corridor cells that fall inside a room are room floor, not corridor.
    let mut corridor_only = CellGrid::new();
    for c in corridors.iter() {
        if !floor.contains(c) {
            corridor_only.set(c);
        }
    }

    // The union is what the player can walk on, and what connectivity is
    // proven over.
    let mut walkable = CellGrid::new();
    for c in floor.iter() {
        walkable.set(c);
    }
    for c in corridor_only.iter() {
        walkable.set(c);
    }

    // Fit corridors against the WALKABLE grid, so a corridor meeting a room
    // still reads its neighbour and caps correctly.
    let mut placements = Vec::new();
    let mut rng2 = GenRng::new(p.seed ^ 0xD006_0000);
    let floor_tiles = kit.by_role(TileRole::Floor);
    for cell in walkable.iter() {
        if floor.contains(cell) {
            if !floor_tiles.is_empty() {
                // Vary the floor variant so a large room does not tile
                // visibly; rotation is free variation on a square tile.
                placements.push(TilePlacement {
                    tile: floor_tiles[rng2.index(floor_tiles.len())],
                    cell,
                    pos: TileLayout::cell_to_world(kit.tile_size, cell),
                    quarters: rng2.index(4) as u32,
                    mask: 0,
                });
            }
        } else {
            let target = walkable.mask_at(cell);
            if let Some((tile, quarters)) = kit.fit(target, &mut rng2) {
                placements.push(TilePlacement {
                    tile,
                    cell,
                    pos: TileLayout::cell_to_world(kit.tile_size, cell),
                    quarters,
                    mask: rotate_mask(kit.tiles[tile as usize].mask, quarters),
                });
            }
        }
    }

    let entrance_cell = rooms[order[0]].centre();
    let exit_cell = rooms[order[order.len() - 1]].centre();

    let mut level = Level {
        open_cells: walkable.iter().collect(),
        entrance: Some(TileLayout::cell_to_world(kit.tile_size, entrance_cell)),
        exit: Some(TileLayout::cell_to_world(kit.tile_size, exit_cell)),
        ..Default::default()
    };
    level.add(&kit.id, kit.tile_size, placements);
    level
}

/// The walkable grid a dungeon produced, for callers that want to verify
/// connectivity or place spawns themselves.
pub fn walkable_grid(level: &Level) -> CellGrid {
    let mut g = CellGrid::new();
    for c in &level.open_cells {
        g.set(*c);
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    fn road_kit() -> Kit {
        Kit::new(
            "kenney/city-kit-roads",
            4.0,
            vec![
                TileDef::new("road-end", TileRole::End, 0.1),
                TileDef::new("road-straight", TileRole::Straight, 0.1),
                TileDef::new("road-bend", TileRole::Corner, 0.1),
                TileDef::new("road-intersection", TileRole::TJunction, 0.1),
                TileDef::new("road-crossroad", TileRole::Cross, 0.1),
            ],
        )
    }

    fn dungeon_kit() -> Kit {
        Kit::new(
            "kenney/modular-dungeon-kit",
            4.0,
            vec![
                TileDef::new("corridor-end", TileRole::End, 3.0),
                TileDef::new("corridor", TileRole::Straight, 3.0),
                TileDef::new("corridor-corner", TileRole::Corner, 3.0),
                TileDef::new("corridor-junction", TileRole::TJunction, 3.0),
                TileDef::new("corridor-intersection", TileRole::Cross, 3.0),
                TileDef::new("template-floor-big", TileRole::Floor, 0.2),
                TileDef::new("template-floor-detail", TileRole::Floor, 0.2),
            ],
        )
    }

    fn building_kit() -> Kit {
        Kit::new(
            "kenney/city-kit-commercial",
            4.0,
            vec![
                TileDef::new("building-a", TileRole::Building, 8.0),
                TileDef::new("building-b", TileRole::Building, 10.0),
                TileDef::new("building-skyscraper-a", TileRole::Building, 20.0),
            ],
        )
    }

    fn prop_kit() -> Kit {
        Kit::new(
            "kenney/city-kit-props",
            4.0,
            vec![
                TileDef::new("light-square", TileRole::Prop, 4.0),
                TileDef::new("construction-cone", TileRole::Prop, 0.5),
            ],
        )
    }

    #[test]
    fn crossing_paths_produce_a_crossroad_without_being_asked() {
        let kit = road_kit();
        let level = road_network(
            &kit,
            &RoadParams {
                seed: 1,
                paths: &[
                    vec![vec3f(-20.0, 0.0, 0.0), vec3f(20.0, 0.0, 0.0)],
                    vec![vec3f(0.0, 0.0, -20.0), vec3f(0.0, 0.0, 20.0)],
                ],
            },
        );
        let layer = &level.layers[0];
        let centre = layer.placements.iter().find(|p| p.cell == (0, 0)).unwrap();
        assert_eq!(kit.tiles[centre.tile as usize].role, TileRole::Cross);
    }

    #[test]
    fn a_tee_produces_a_t_junction() {
        let kit = road_kit();
        let level = road_network(
            &kit,
            &RoadParams {
                seed: 1,
                paths: &[
                    vec![vec3f(-20.0, 0.0, 0.0), vec3f(20.0, 0.0, 0.0)],
                    // Stops at the main road rather than crossing it.
                    vec![vec3f(0.0, 0.0, 0.0), vec3f(0.0, 0.0, 20.0)],
                ],
            },
        );
        let layer = &level.layers[0];
        let centre = layer.placements.iter().find(|p| p.cell == (0, 0)).unwrap();
        assert_eq!(kit.tiles[centre.tile as usize].role, TileRole::TJunction);
    }

    #[test]
    fn every_road_layer_is_adjacency_clean() {
        let kit = road_kit();
        let level = road_network(
            &kit,
            &RoadParams {
                seed: 12,
                paths: &[
                    vec![vec3f(-40.0, 0.0, -8.0), vec3f(40.0, 0.0, -8.0)],
                    vec![vec3f(-40.0, 0.0, 16.0), vec3f(40.0, 0.0, 16.0)],
                    vec![vec3f(-16.0, 0.0, -24.0), vec3f(-16.0, 0.0, 32.0)],
                    vec![vec3f(20.0, 0.0, -24.0), vec3f(20.0, 0.0, 32.0)],
                ],
            },
        );
        let layout = TileLayout {
            kit_id: level.layers[0].kit_id.clone(),
            tile_size: level.layers[0].tile_size,
            placements: level.layers[0].placements.clone(),
            open_cells: vec![],
            entrance: None,
            exit: None,
        };
        assert!(
            adjacency_errors(&layout).is_empty(),
            "road network has mismatched tile edges"
        );
    }

    #[test]
    fn a_closed_spline_becomes_a_loop_with_no_dead_ends() {
        let kit = road_kit();
        // An oval circuit, the racing case.
        let spline = Spline::new(
            vec![
                vec3f(-40.0, 0.0, -20.0),
                vec3f(40.0, 0.0, -20.0),
                vec3f(56.0, 0.0, 0.0),
                vec3f(40.0, 0.0, 20.0),
                vec3f(-40.0, 0.0, 20.0),
                vec3f(-56.0, 0.0, 0.0),
            ],
            true,
        );
        let level = road_from_spline(&kit, &spline, 12, 4);
        let layer = &level.layers[0];
        assert!(layer.placements.len() > 20, "circuit too short to be real");
        let ends = layer
            .placements
            .iter()
            .filter(|p| kit.tiles[p.tile as usize].role == TileRole::End)
            .count();
        assert_eq!(ends, 0, "a closed circuit must not contain dead ends");
    }

    #[test]
    fn town_places_buildings_that_face_a_street() {
        let roads = road_kit();
        let buildings = building_kit();
        let level = town(
            &roads,
            &TownParams {
                seed: 5,
                extent: (16, 16),
                block: 4,
                density: 1.0,
                buildings: Some(&buildings),
                props: None,
            },
        );
        let blayer = level
            .layers
            .iter()
            .find(|l| l.kit_id == buildings.id)
            .expect("building layer");
        assert!(!blayer.placements.is_empty());
        // Every building must sit on a lot that touches a road, or it is
        // unreachable scenery in the middle of a block.
        let roadcells: std::collections::BTreeSet<(i32, i32)> = level
            .layers
            .iter()
            .find(|l| l.kit_id == roads.id)
            .unwrap()
            .placements
            .iter()
            .map(|p| p.cell)
            .collect();
        for b in &blayer.placements {
            let (x, z) = b.cell;
            let touches = [(x, z - 1), (x + 1, z), (x, z + 1), (x - 1, z)]
                .iter()
                .any(|c| roadcells.contains(c));
            assert!(touches, "building at {:?} fronts no street", b.cell);
            assert!(!roadcells.contains(&b.cell), "building sits on the road");
        }
    }

    #[test]
    fn town_props_land_only_on_junctions() {
        let roads = road_kit();
        let props = prop_kit();
        let level = town(
            &roads,
            &TownParams {
                seed: 9,
                extent: (16, 16),
                block: 4,
                density: 0.0,
                buildings: None,
                props: Some(&props),
            },
        );
        // Tile indices are KIT-RELATIVE, so a layer may only be interpreted
        // against the kit that produced it — an earlier version of this test
        // indexed the props kit with road-layer indices and panicked, which is
        // exactly the mistake a caller can make.
        let road_cells: std::collections::BTreeSet<(i32, i32)> = level
            .layers
            .iter()
            .find(|l| l.kit_id == roads.id)
            .expect("road layer")
            .placements
            .iter()
            .map(|p| p.cell)
            .collect();
        let prop_layer = level
            .layers
            .iter()
            .find(|l| l.kit_id == props.id)
            .expect("prop layer");
        assert!(!prop_layer.placements.is_empty());
        for p in &prop_layer.placements {
            assert!(
                props.tiles[p.tile as usize].role == TileRole::Prop,
                "prop layer holds a non-prop tile"
            );
            // A street light belongs at a junction, not mid-block.
            let (x, z) = p.cell;
            let degree = [(x, z - 1), (x + 1, z), (x, z + 1), (x - 1, z)]
                .iter()
                .filter(|c| road_cells.contains(c))
                .count();
            assert!(
                degree >= 3,
                "prop at {:?} has road degree {degree}, not a junction",
                p.cell
            );
        }
    }

    #[test]
    fn tiles_from_one_kit_share_a_layer_and_keep_valid_indices() {
        // The real case this guards: Kenney's city-kit-roads holds the road
        // tiles AND the cones and streetlights. One pack is one atlas, so it
        // must come out as one batch — and because tile indices are
        // kit-relative, a second layer under the same id would be
        // unresolvable. A kit id must therefore identify exactly one tile list.
        let combined = Kit::new(
            "kenney/city-kit-roads",
            4.0,
            vec![
                TileDef::new("road-end", TileRole::End, 0.1),
                TileDef::new("road-straight", TileRole::Straight, 0.1),
                TileDef::new("road-bend", TileRole::Corner, 0.1),
                TileDef::new("road-intersection", TileRole::TJunction, 0.1),
                TileDef::new("road-crossroad", TileRole::Cross, 0.1),
                TileDef::new("light-square", TileRole::Prop, 4.0),
            ],
        );
        let level = town(
            &combined,
            &TownParams {
                seed: 4,
                extent: (16, 16),
                block: 4,
                density: 0.0,
                buildings: None,
                props: Some(&combined),
            },
        );
        assert_eq!(level.layers.len(), 1, "one kit must yield one batch");
        let layer = &level.layers[0];
        for p in &layer.placements {
            assert!(
                (p.tile as usize) < combined.tiles.len(),
                "index {} outside the kit that owns the layer",
                p.tile
            );
        }
        assert!(
            layer
                .placements
                .iter()
                .any(|p| combined.tiles[p.tile as usize].role == TileRole::Prop),
            "props should have merged into the road layer"
        );
    }

    #[test]
    fn every_dungeon_room_is_reachable_from_the_entrance() {
        let kit = dungeon_kit();
        // Several seeds, because connectivity that holds for one layout by
        // luck is not connectivity.
        for seed in 0..12u64 {
            let level = dungeon(
                &kit,
                &DungeonParams {
                    seed,
                    extent: (40, 40),
                    min_room: 5,
                    depth: 4,
                },
            );
            let grid = walkable_grid(&level);
            assert!(!grid.is_empty(), "seed {seed} produced nothing");
            let start = level.entrance.unwrap();
            let start_cell = (
                (start.x / kit.tile_size).round() as i32,
                (start.z / kit.tile_size).round() as i32,
            );
            let seen = grid.reachable_from(start_cell);
            assert_eq!(
                seen.len(),
                grid.len(),
                "seed {seed}: {} of {} cells unreachable",
                grid.len() - seen.len(),
                grid.len()
            );
        }
    }

    #[test]
    fn dungeon_corridors_are_adjacency_clean() {
        let kit = dungeon_kit();
        for seed in 0..8u64 {
            let level = dungeon(
                &kit,
                &DungeonParams {
                    seed,
                    ..Default::default()
                },
            );
            let layout = TileLayout {
                kit_id: level.layers[0].kit_id.clone(),
                tile_size: level.layers[0].tile_size,
                placements: level.layers[0].placements.clone(),
                open_cells: vec![],
                entrance: None,
                exit: None,
            };
            let bad = adjacency_errors(&layout);
            assert!(bad.is_empty(), "seed {seed}: mismatched edges at {bad:?}");
        }
    }

    #[test]
    fn dungeon_entrance_and_exit_are_distinct_and_walkable() {
        let kit = dungeon_kit();
        let level = dungeon(
            &kit,
            &DungeonParams {
                seed: 3,
                ..Default::default()
            },
        );
        let (a, b) = (level.entrance.unwrap(), level.exit.unwrap());
        assert!(
            (a.x - b.x).abs() + (a.z - b.z).abs() > kit.tile_size,
            "entrance and exit coincide"
        );
        let grid = walkable_grid(&level);
        for v in [a, b] {
            let cell = (
                (v.x / kit.tile_size).round() as i32,
                (v.z / kit.tile_size).round() as i32,
            );
            assert!(grid.contains(cell), "{v:?} is not walkable");
        }
    }

    #[test]
    fn generators_are_deterministic_for_a_seed() {
        let rk = road_kit();
        let dk = dungeon_kit();
        let bk = building_kit();

        let r1 = road_network(
            &rk,
            &RoadParams {
                seed: 21,
                paths: &[vec![vec3f(-20.0, 0.0, 0.0), vec3f(20.0, 0.0, 12.0)]],
            },
        );
        let r2 = road_network(
            &rk,
            &RoadParams {
                seed: 21,
                paths: &[vec![vec3f(-20.0, 0.0, 0.0), vec3f(20.0, 0.0, 12.0)]],
            },
        );
        assert_eq!(r1.layers[0].placements, r2.layers[0].placements);

        let d1 = dungeon(&dk, &DungeonParams { seed: 21, ..Default::default() });
        let d2 = dungeon(&dk, &DungeonParams { seed: 21, ..Default::default() });
        assert_eq!(d1.layers[0].placements, d2.layers[0].placements);
        assert_eq!(d1.entrance, d2.entrance);

        let t1 = town(&rk, &TownParams { seed: 21, buildings: Some(&bk), ..Default::default() });
        let t2 = town(&rk, &TownParams { seed: 21, buildings: Some(&bk), ..Default::default() });
        assert_eq!(t1.placement_count(), t2.placement_count());
        for (a, b) in t1.layers.iter().zip(t2.layers.iter()) {
            assert_eq!(a.placements, b.placements);
        }

        // Different seeds must actually differ, or the seed is decorative.
        let d3 = dungeon(&dk, &DungeonParams { seed: 22, ..Default::default() });
        assert_ne!(d1.layers[0].placements, d3.layers[0].placements);
    }

    #[test]
    fn realistic_levels_stay_within_a_sane_placement_budget() {
        let rk = road_kit();
        let dk = dungeon_kit();
        let bk = building_kit();
        let pk = prop_kit();

        let t = town(
            &rk,
            &TownParams {
                seed: 1,
                extent: (40, 40),
                block: 5,
                density: 0.7,
                buildings: Some(&bk),
                props: Some(&pk),
            },
        );
        let d = dungeon(
            &dk,
            &DungeonParams {
                seed: 1,
                extent: (48, 48),
                min_room: 5,
                depth: 5,
            },
        );
        // Not a perf assertion so much as a canary: an order-of-magnitude jump
        // here means a generator started filling area it should not.
        assert!(
            t.placement_count() < 4000,
            "town emitted {} placements",
            t.placement_count()
        );
        assert!(
            d.placement_count() < 4000,
            "dungeon emitted {} placements",
            d.placement_count()
        );
        assert!(t.placement_count() > 100 && d.placement_count() > 100);
    }
}
