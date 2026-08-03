//! Modular kit composition: assembling authored tiles into coherent levels.
//!
//! Kenney's kits are not prop bags — they are tile sets designed to snap on a
//! grid (`road-straight`, `road-bend`, `road-crossroad`, `corridor-junction`,
//! `building-corner`...). That makes a different generation model available
//! than the rest of this crate uses: procedural LAYOUT plus AUTHORED tiles.
//! The algorithm decides where things go; an artist already decided how they
//! look. It beats both random prop scatter and purely procedural geometry, and
//! it is how low-poly games are actually built.
//!
//! Everything here is layout only — placements come out as (tile, grid cell,
//! quarter-turn) triples plus a box for collision. Meshing, batching and
//! spawning belong to the caller.
//!
//! # The connection mask
//!
//! The invariant that makes a level coherent is that adjacent tiles must
//! MATCH: a straight may meet a corner, a corridor may not open onto a blank
//! wall face. Encoding that as rules between named roles ("a bend accepts a
//! straight to its north") is fragile and grows quadratically.
//!
//! Instead every tile carries a 4-bit mask of which EDGES it connects on, and
//! fitting works in two steps: the layout marks which cells are occupied, then
//! each cell's target mask is read off its occupied neighbours and a tile is
//! chosen whose mask matches at some rotation. Adjacency then holds *by
//! construction* — both sides of every shared edge were derived from the same
//! grid — and the only thing left to get wrong is the rotation arithmetic,
//! which is exactly what the tests pin.

use crate::rng::GenRng;
use makepad_math::*;

/// Edge bits, clockwise from north. North is -Z, east is +X, matching the
/// engine's right-handed Y-up convention.
pub const N: u8 = 1;
pub const E: u8 = 2;
pub const S: u8 = 4;
pub const W: u8 = 8;

/// Rotate a connection mask by `quarters` 90° clockwise turns.
///
/// A clockwise turn sends north to east, so each bit moves up one position
/// and the top bit wraps — a 4-bit rotate-left.
pub fn rotate_mask(mask: u8, quarters: u32) -> u8 {
    let k = (quarters % 4) as u8;
    let m = mask & 0xF;
    ((m << k) | (m >> (4 - k))) & 0xF
}

/// What a tile is for. Parsed from Kenney's systematic filenames by the asset
/// index; kept as a small closed set because the generators switch on it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TileRole {
    /// One open edge — a dead end or terminal cap.
    End,
    /// Two opposite edges: `road-straight`, `corridor`.
    Straight,
    /// Two adjacent edges: `road-bend`, `road-curve`, `corridor-corner`.
    Corner,
    /// Three edges: `road-intersection`, `corridor-junction`.
    TJunction,
    /// Four edges: `road-crossroad`, `corridor-intersection`.
    Cross,
    /// Open floor with no connection semantics — fills room interiors.
    Floor,
    /// A solid wall segment, connecting along its own run.
    Wall,
    /// An outside/inside corner of a wall run.
    WallCorner,
    /// A traversable opening in a wall: `gate-door`, `building-door`.
    Door,
    /// Vertical link between levels.
    Stairs,
    /// A whole building that occupies one or more lots.
    Building,
    /// Scenery with no structural role: cones, benches, barrels.
    Prop,
}

impl TileRole {
    /// The mask this role connects on in its unrotated, canonical orientation.
    ///
    /// Canonical means "first connection points north", so a corner is N|E and
    /// a T is N|E|S. Fitting rotates from here; nothing else depends on the
    /// choice, but it must be consistent or the rotation search fails.
    pub fn canonical_mask(self) -> u8 {
        match self {
            TileRole::End => N,
            TileRole::Straight => N | S,
            TileRole::Corner => N | E,
            TileRole::TJunction => N | E | S,
            TileRole::Cross => N | E | S | W,
            TileRole::Wall => N | S,
            TileRole::WallCorner => N | E,
            TileRole::Door => N | S,
            // Floors, stairs, buildings and props have no edge semantics.
            TileRole::Floor | TileRole::Stairs | TileRole::Building | TileRole::Prop => 0,
        }
    }

    /// The role that serves a given number of connections, which is how a
    /// layout's occupancy grid maps onto a tile set.
    pub fn for_degree(degree: u32, opposite: bool) -> TileRole {
        match degree {
            0 => TileRole::Floor,
            1 => TileRole::End,
            2 if opposite => TileRole::Straight,
            2 => TileRole::Corner,
            3 => TileRole::TJunction,
            _ => TileRole::Cross,
        }
    }
}

/// One tile in a kit.
#[derive(Clone, Debug)]
pub struct TileDef {
    /// Asset id, e.g. `kenney/city-kit-roads/road-straight`. Opaque here — the
    /// caller resolves it against the asset index.
    pub id: String,
    pub role: TileRole,
    /// Edges this tile connects on, unrotated. Normally
    /// `role.canonical_mask()`, but carried explicitly so the asset index can
    /// override for tiles whose geometry disagrees with their name.
    pub mask: u8,
    /// Height in world units, for the collision box. Footprint comes from the
    /// kit's tile size.
    pub height: f32,
}

impl TileDef {
    pub fn new(id: impl Into<String>, role: TileRole, height: f32) -> Self {
        Self {
            id: id.into(),
            role,
            mask: role.canonical_mask(),
            height,
        }
    }
}

/// A set of tiles that were authored together and therefore look right beside
/// each other.
///
/// Grouping by kit is not bookkeeping — it is the whole point. A per-model
/// search for "road tile" returns one tile from each of five kits and the
/// result reads as a junk drawer; a kit is a coherent visual set.
///
/// # Invariant
///
/// A kit id identifies exactly one tile list. [`TilePlacement::tile`] is an
/// index into `tiles`, and generated levels merge placements by kit id (one
/// pack is one atlas, so one batch), so two different tile lists sharing an id
/// would make those indices unresolvable. Where one pack supplies several
/// roles — Kenney's `city-kit-roads` holds the roads *and* the cones and
/// streetlights — build one `Kit` containing all of them rather than two that
/// share a name.
#[derive(Clone, Debug)]
pub struct Kit {
    pub id: String,
    /// Grid pitch in world units. Kenney kits are consistent within a pack;
    /// the asset index measures it from the GLB bounds.
    pub tile_size: f32,
    pub tiles: Vec<TileDef>,
}

impl Kit {
    pub fn new(id: impl Into<String>, tile_size: f32, tiles: Vec<TileDef>) -> Self {
        Self {
            id: id.into(),
            tile_size,
            tiles,
        }
    }

    /// Every tile with this role, as indices into `tiles`.
    pub fn by_role(&self, role: TileRole) -> Vec<u32> {
        self.tiles
            .iter()
            .enumerate()
            .filter(|(_, t)| t.role == role)
            .map(|(i, _)| i as u32)
            .collect()
    }

    pub fn has_role(&self, role: TileRole) -> bool {
        self.tiles.iter().any(|t| t.role == role)
    }

    /// Find a tile whose mask matches `target` at some rotation, preferring the
    /// role that the connection count implies.
    ///
    /// Returns the tile index and the quarter-turns to apply. `rng` picks among
    /// equally valid variants so a long road is not visibly stamped.
    ///
    /// If no tile matches exactly, a tile that opens on a SUPERSET of the
    /// required edges is accepted. An incomplete kit is common — plenty of
    /// packs ship no dead-end cap — and a crossroad standing in for a tee
    /// leaves one stub opening onto nothing, which reads as an unfinished road
    /// rather than as a hole in the world. Adjacency stays valid either way,
    /// because the extra opening faces a cell where no tile exists.
    pub fn fit(&self, target: u8, rng: &mut GenRng) -> Option<(u32, u32)> {
        let target = target & 0xF;
        // Count, pick, then re-scan. Collecting candidates into a Vec would
        // allocate on every cell of every level, and a Cross tile is a
        // superset of every target — so the fallback pass in particular would
        // push constantly. Two cheap passes over a handful of tiles beat one
        // pass plus a heap allocation.
        for exact in [true, false] {
            let n = (0..self.tiles.len())
                .filter(|&i| self.match_rotation(i, target, exact).is_some())
                .count();
            if n == 0 {
                continue;
            }
            let pick = rng.index(n);
            let mut seen = 0;
            for i in 0..self.tiles.len() {
                if let Some(q) = self.match_rotation(i, target, exact) {
                    if seen == pick {
                        return Some((i as u32, q));
                    }
                    seen += 1;
                }
            }
        }
        None
    }

    /// First rotation at which tile `i` presents `target` (exactly, or as a
    /// superset of it). One rotation per tile keeps the variant draw uniform
    /// over TILES rather than over (tile, rotation) pairs — otherwise a
    /// straight, which matches at two rotations, would be picked twice as
    /// often as a tile that matches at one.
    fn match_rotation(&self, i: usize, target: u8, exact: bool) -> Option<u32> {
        let mask = self.tiles[i].mask;
        if mask == 0 {
            return None;
        }
        (0..4).find(|&q| {
            let m = rotate_mask(mask, q);
            if exact {
                m == target
            } else {
                m & target == target
            }
        })
    }
}

/// A tile placed in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TilePlacement {
    /// Index into the kit's `tiles`.
    pub tile: u32,
    /// Grid cell, for adjacency checks and debugging.
    pub cell: (i32, i32),
    /// World position of the tile's centre.
    pub pos: Vec3f,
    /// Quarter-turns clockwise about Y.
    pub quarters: u32,
    /// The mask this placement actually presents to its neighbours, i.e. the
    /// tile's mask already rotated. Carried so a caller (or a test) can verify
    /// adjacency without re-deriving the rotation.
    pub mask: u8,
}

impl TilePlacement {
    /// Yaw in radians for the renderer.
    pub fn yaw(&self) -> f32 {
        self.quarters as f32 * core::f32::consts::FRAC_PI_2
    }

    /// Axis-aligned collision box: (centre, half-extents). Square footprint
    /// from the kit pitch, height from the tile.
    pub fn bounds(&self, tile_size: f32, height: f32) -> (Vec3f, Vec3f) {
        let h = tile_size * 0.5;
        (
            vec3f(self.pos.x, self.pos.y + height * 0.5, self.pos.z),
            vec3f(h, height * 0.5, h),
        )
    }
}

/// A finished level: placements plus the kit they index into.
#[derive(Clone, Debug, Default)]
pub struct TileLayout {
    pub kit_id: String,
    pub tile_size: f32,
    pub placements: Vec<TilePlacement>,
    /// Cells the generator marked traversable, for spawn points and pathing.
    pub open_cells: Vec<(i32, i32)>,
    /// Where a player should start, if the generator has an opinion.
    pub entrance: Option<(i32, i32)>,
    pub exit: Option<(i32, i32)>,
}

impl TileLayout {
    /// World position of a grid cell centre.
    pub fn cell_to_world(tile_size: f32, cell: (i32, i32)) -> Vec3f {
        vec3f(cell.0 as f32 * tile_size, 0.0, cell.1 as f32 * tile_size)
    }
}

/// An occupancy grid: the abstract layout, before any tile is chosen.
///
/// Separating this from tile fitting is what keeps the generators readable —
/// a dungeon decides which cells are corridor, and knows nothing about
/// `corridor-junction.glb`.
#[derive(Clone, Debug, Default)]
pub struct CellGrid {
    cells: std::collections::BTreeSet<(i32, i32)>,
}

impl CellGrid {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, cell: (i32, i32)) {
        self.cells.insert(cell);
    }

    pub fn contains(&self, cell: (i32, i32)) -> bool {
        self.cells.contains(&cell)
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Iteration is sorted, not hash order — placements must be deterministic
    /// across runs and platforms, and a HashSet would not be.
    pub fn iter(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.cells.iter().copied()
    }

    /// Which of the four neighbours are also occupied.
    pub fn mask_at(&self, cell: (i32, i32)) -> u8 {
        let (x, z) = cell;
        let mut m = 0;
        if self.contains((x, z - 1)) {
            m |= N;
        }
        if self.contains((x + 1, z)) {
            m |= E;
        }
        if self.contains((x, z + 1)) {
            m |= S;
        }
        if self.contains((x - 1, z)) {
            m |= W;
        }
        m
    }

    /// Straight line of cells between two points, stepping in X then Z. Used
    /// by corridor and street generators; an L is two runs.
    pub fn line(&mut self, from: (i32, i32), to: (i32, i32)) {
        let (mut x, z0) = from;
        let (x1, z1) = to;
        while x != x1 {
            self.set((x, z0));
            x += (x1 - x).signum();
        }
        let mut z = z0;
        while z != z1 {
            self.set((x1, z));
            z += (z1 - z).signum();
        }
        self.set((x1, z1));
    }

    pub fn rect(&mut self, min: (i32, i32), max: (i32, i32)) {
        for z in min.1..=max.1 {
            for x in min.0..=max.0 {
                self.set((x, z));
            }
        }
    }

    /// Cells reachable from `start` by 4-way movement. The connectivity proof
    /// for generated dungeons.
    pub fn reachable_from(&self, start: (i32, i32)) -> std::collections::BTreeSet<(i32, i32)> {
        let mut seen = std::collections::BTreeSet::new();
        if !self.contains(start) {
            return seen;
        }
        let mut stack = vec![start];
        seen.insert(start);
        while let Some((x, z)) = stack.pop() {
            for n in [(x, z - 1), (x + 1, z), (x, z + 1), (x - 1, z)] {
                if self.contains(n) && seen.insert(n) {
                    stack.push(n);
                }
            }
        }
        seen
    }
}

/// Choose a tile for every occupied cell.
///
/// This is the whole bridge from layout to art. Each cell's target mask comes
/// from its occupied neighbours, so any two adjacent placements agree about
/// their shared edge before a tile is even looked at.
///
/// Cells whose mask has no matching tile are skipped rather than filled with
/// something wrong — a hole is a visible, debuggable failure, while a mismatched
/// tile is a subtle one that reads as a broken level.
pub fn fit_grid(kit: &Kit, grid: &CellGrid, seed: u64) -> TileLayout {
    let mut rng = GenRng::new(seed);
    let mut placements = Vec::with_capacity(grid.len());
    let mut open_cells = Vec::with_capacity(grid.len());

    for cell in grid.iter() {
        let target = grid.mask_at(cell);
        open_cells.push(cell);
        let fitted = if target == 0 {
            // Isolated cell: no connections to satisfy, so a floor tile if the
            // kit has one, else nothing.
            kit.by_role(TileRole::Floor)
                .first()
                .map(|&i| (i, 0))
                .or_else(|| kit.by_role(TileRole::End).first().map(|&i| (i, 0)))
        } else {
            kit.fit(target, &mut rng)
        };
        if let Some((tile, quarters)) = fitted {
            placements.push(TilePlacement {
                tile,
                cell,
                pos: TileLayout::cell_to_world(kit.tile_size, cell),
                quarters,
                mask: rotate_mask(kit.tiles[tile as usize].mask, quarters),
            });
        }
    }

    TileLayout {
        kit_id: kit.id.clone(),
        tile_size: kit.tile_size,
        placements,
        open_cells,
        entrance: None,
        exit: None,
    }
}

/// Place a single tile by hand, for composition the generators do not cover.
///
/// The escape hatch that keeps this layer from being a closed set of three
/// level types: a game can lay its own tiles and still get the kit's grid
/// pitch and rotation convention for free.
pub fn place_tile(kit: &Kit, tile: u32, cell: (i32, i32), quarters: u32) -> Option<TilePlacement> {
    let def = kit.tiles.get(tile as usize)?;
    Some(TilePlacement {
        tile,
        cell,
        pos: TileLayout::cell_to_world(kit.tile_size, cell),
        quarters: quarters % 4,
        mask: rotate_mask(def.mask, quarters),
    })
}

/// Verify that every pair of adjacent placements agrees about their shared
/// edge: if A opens toward B, B must open back toward A.
///
/// Tiles with a zero mask (floors, props, buildings) have no structural edges
/// and are open to anything, so pairs involving them are skipped — a corridor
/// ending at a room floor is correct, not a mismatch.
///
/// Exposed rather than kept in the tests because it is cheap and a game
/// assembling tiles by hand wants the same check.
pub fn adjacency_errors(layout: &TileLayout) -> Vec<((i32, i32), (i32, i32))> {
    use std::collections::BTreeMap;
    let by_cell: BTreeMap<(i32, i32), &TilePlacement> =
        layout.placements.iter().map(|p| (p.cell, p)).collect();
    let mut bad = Vec::new();
    for p in &layout.placements {
        if p.mask == 0 {
            continue;
        }
        let (x, z) = p.cell;
        // Only test east and south, so each shared edge is checked once.
        for (nbr, out_bit, back_bit) in [((x + 1, z), E, W), ((x, z + 1), S, N)] {
            let Some(q) = by_cell.get(&nbr) else { continue };
            if q.mask == 0 {
                continue;
            }
            let a_opens = p.mask & out_bit != 0;
            let b_opens = q.mask & back_bit != 0;
            if a_opens != b_opens {
                bad.push((p.cell, nbr));
            }
        }
    }
    bad
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A road kit shaped like Kenney's city-kit-roads, for testing without the
    /// asset index.
    pub(crate) fn road_kit() -> Kit {
        Kit::new(
            "kenney/city-kit-roads",
            4.0,
            vec![
                TileDef::new("road-end", TileRole::End, 0.1),
                TileDef::new("road-straight", TileRole::Straight, 0.1),
                TileDef::new("road-straight-barrier", TileRole::Straight, 0.6),
                TileDef::new("road-bend", TileRole::Corner, 0.1),
                TileDef::new("road-curve", TileRole::Corner, 0.1),
                TileDef::new("road-intersection", TileRole::TJunction, 0.1),
                TileDef::new("road-crossroad", TileRole::Cross, 0.1),
            ],
        )
    }

    #[test]
    fn mask_rotation_is_a_four_bit_rotate() {
        assert_eq!(rotate_mask(N, 1), E);
        assert_eq!(rotate_mask(E, 1), S);
        assert_eq!(rotate_mask(S, 1), W);
        assert_eq!(rotate_mask(W, 1), N);
        // A straight is invariant under a half turn; a corner is not.
        assert_eq!(rotate_mask(N | S, 2), N | S);
        assert_eq!(rotate_mask(N | E, 1), E | S);
        // Four turns is identity for every mask.
        for m in 0..16u8 {
            assert_eq!(rotate_mask(m, 4), m);
        }
    }

    #[test]
    fn roles_map_to_the_expected_degree() {
        assert_eq!(TileRole::End.canonical_mask().count_ones(), 1);
        assert_eq!(TileRole::Straight.canonical_mask().count_ones(), 2);
        assert_eq!(TileRole::Corner.canonical_mask().count_ones(), 2);
        assert_eq!(TileRole::TJunction.canonical_mask().count_ones(), 3);
        assert_eq!(TileRole::Cross.canonical_mask().count_ones(), 4);
        // Straight and corner differ by opposite-vs-adjacent, which is what
        // for_degree disambiguates.
        assert_eq!(TileRole::for_degree(2, true), TileRole::Straight);
        assert_eq!(TileRole::for_degree(2, false), TileRole::Corner);
    }

    #[test]
    fn fit_finds_a_rotation_for_every_reachable_mask() {
        let kit = road_kit();
        let mut rng = GenRng::new(1);
        // Every non-zero mask must be satisfiable by some tile at some
        // rotation, or a layout will develop holes.
        for target in 1..16u8 {
            let (tile, q) = kit
                .fit(target, &mut rng)
                .unwrap_or_else(|| panic!("no tile fits mask {target:#06b}"));
            assert_eq!(
                rotate_mask(kit.tiles[tile as usize].mask, q),
                target,
                "tile {} rotated {q} does not present {target:#06b}",
                kit.tiles[tile as usize].id
            );
        }
    }

    #[test]
    fn an_incomplete_kit_falls_back_to_a_superset_rather_than_a_hole() {
        // A kit with no tee and no end cap — common enough in real packs.
        let kit = Kit::new(
            "sparse",
            4.0,
            vec![
                TileDef::new("straight", TileRole::Straight, 0.1),
                TileDef::new("corner", TileRole::Corner, 0.1),
                TileDef::new("cross", TileRole::Cross, 0.1),
            ],
        );
        let mut rng = GenRng::new(1);
        // A tee must still place something: the cross opens on a superset of
        // the required edges, so one stub faces a cell with no tile in it.
        let (tile, q) = kit.fit(N | E | S, &mut rng).expect("tee must fall back");
        let presented = rotate_mask(kit.tiles[tile as usize].mask, q);
        assert_eq!(presented & (N | E | S), N | E | S, "required edges must open");
        assert_eq!(kit.tiles[tile as usize].role, TileRole::Cross);
        // A dead end likewise: any tile covering that single edge will do.
        let (tile, q) = kit.fit(N, &mut rng).expect("end must fall back");
        assert!(rotate_mask(kit.tiles[tile as usize].mask, q) & N != 0);
    }

    #[test]
    fn an_exact_match_is_always_preferred_over_a_superset() {
        let kit = road_kit();
        let mut rng = GenRng::new(2);
        // The kit has a real tee, so the cross must never be chosen for one.
        for _ in 0..32 {
            let (tile, _) = kit.fit(N | E | S, &mut rng).unwrap();
            assert_eq!(kit.tiles[tile as usize].role, TileRole::TJunction);
        }
    }

    #[test]
    fn place_tile_reports_its_rotated_mask() {
        let kit = road_kit();
        // road-bend is canonical N|E; a single clockwise turn makes it E|S.
        let p = place_tile(&kit, 3, (2, -1), 1).unwrap();
        assert_eq!(p.mask, E | S);
        assert_eq!(p.cell, (2, -1));
        assert_eq!(p.pos, vec3f(8.0, 0.0, -4.0));
        assert!((p.yaw() - core::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }

    #[test]
    fn grid_mask_reads_occupied_neighbours() {
        let mut g = CellGrid::new();
        g.set((0, 0));
        assert_eq!(g.mask_at((0, 0)), 0);
        g.set((0, -1));
        assert_eq!(g.mask_at((0, 0)), N);
        g.set((1, 0));
        assert_eq!(g.mask_at((0, 0)), N | E);
        g.set((0, 1));
        g.set((-1, 0));
        assert_eq!(g.mask_at((0, 0)), N | E | S | W);
    }

    #[test]
    fn cell_grid_iteration_is_sorted_not_hash_order() {
        let mut a = CellGrid::new();
        let mut b = CellGrid::new();
        for c in [(3, 1), (0, 0), (-2, 5), (1, 1)] {
            a.set(c);
        }
        for c in [(1, 1), (-2, 5), (0, 0), (3, 1)] {
            b.set(c);
        }
        let av: Vec<_> = a.iter().collect();
        let bv: Vec<_> = b.iter().collect();
        assert_eq!(av, bv, "insertion order must not affect iteration");
    }

    #[test]
    fn a_straight_run_uses_straights_and_caps_with_ends() {
        let kit = road_kit();
        let mut g = CellGrid::new();
        g.line((0, 0), (5, 0));
        let layout = fit_grid(&kit, &g, 42);
        assert_eq!(layout.placements.len(), 6);
        let role_of = |p: &TilePlacement| kit.tiles[p.tile as usize].role;
        let ends = layout.placements.iter().filter(|p| role_of(p) == TileRole::End).count();
        let straights = layout
            .placements
            .iter()
            .filter(|p| role_of(p) == TileRole::Straight)
            .count();
        assert_eq!(ends, 2, "both terminals should cap");
        assert_eq!(straights, 4);
        assert!(adjacency_errors(&layout).is_empty());
    }

    #[test]
    fn a_corner_run_uses_a_corner_at_the_turn() {
        let kit = road_kit();
        let mut g = CellGrid::new();
        // An L: east along z=0, then south down x=3.
        g.line((0, 0), (3, 0));
        g.line((3, 0), (3, 3));
        let layout = fit_grid(&kit, &g, 7);
        let corner = layout
            .placements
            .iter()
            .find(|p| p.cell == (3, 0))
            .expect("turn cell placed");
        assert_eq!(kit.tiles[corner.tile as usize].role, TileRole::Corner);
        // It must open west (back along the run) and south (down the new one).
        assert_eq!(corner.mask, W | S);
        assert!(adjacency_errors(&layout).is_empty());
    }

    #[test]
    fn junction_degree_picks_t_and_cross() {
        let kit = road_kit();
        let mut g = CellGrid::new();
        // A plus centred at origin: degree 4.
        g.line((-2, 0), (2, 0));
        g.line((0, -2), (0, 2));
        let layout = fit_grid(&kit, &g, 3);
        let centre = layout.placements.iter().find(|p| p.cell == (0, 0)).unwrap();
        assert_eq!(kit.tiles[centre.tile as usize].role, TileRole::Cross);

        // A T: a run with one stub.
        let mut g2 = CellGrid::new();
        g2.line((-2, 0), (2, 0));
        g2.line((0, 0), (0, 2));
        let l2 = fit_grid(&kit, &g2, 3);
        let t = l2.placements.iter().find(|p| p.cell == (0, 0)).unwrap();
        assert_eq!(kit.tiles[t.tile as usize].role, TileRole::TJunction);
        assert_eq!(t.mask, E | S | W);
        assert!(adjacency_errors(&l2).is_empty());
    }

    #[test]
    fn fitting_is_deterministic_for_a_seed() {
        let kit = road_kit();
        let mut g = CellGrid::new();
        g.line((0, 0), (9, 0));
        g.line((4, -4), (4, 4));
        let a = fit_grid(&kit, &g, 99);
        let b = fit_grid(&kit, &g, 99);
        assert_eq!(a.placements, b.placements);
        // A different seed should pick different variants somewhere, or the
        // variant draw is not doing anything.
        let c = fit_grid(&kit, &g, 100);
        assert!(
            a.placements != c.placements,
            "seed change had no effect on variant choice"
        );
    }

    #[test]
    fn grid_snapping_is_exact_multiples_of_the_pitch() {
        let kit = road_kit();
        let mut g = CellGrid::new();
        g.rect((-3, -3), (3, 3));
        let layout = fit_grid(&kit, &g, 5);
        for p in &layout.placements {
            assert_eq!(p.pos.x, p.cell.0 as f32 * 4.0);
            assert_eq!(p.pos.z, p.cell.1 as f32 * 4.0);
            assert_eq!(p.pos.y, 0.0);
        }
    }

    #[test]
    fn collision_box_sits_on_the_ground_and_matches_the_pitch() {
        let kit = road_kit();
        let p = place_tile(&kit, 2, (1, 1), 0).unwrap();
        let (centre, half) = p.bounds(kit.tile_size, 0.6);
        assert_eq!(half.x, 2.0);
        assert_eq!(half.z, 2.0);
        assert_eq!(half.y, 0.3);
        // Box rests on y=0 rather than straddling it.
        assert!((centre.y - half.y).abs() < 1e-6);
    }

    #[test]
    fn reachability_flood_fill_finds_disconnection() {
        let mut g = CellGrid::new();
        g.line((0, 0), (3, 0));
        g.line((6, 0), (9, 0));
        let seen = g.reachable_from((0, 0));
        assert_eq!(seen.len(), 4, "must not jump the gap");
        assert!(!seen.contains(&(6, 0)));
    }
}
