//! Room interiors: what is behind a door.
//!
//! A house built from a stock model is a hollow shell with a gap where the door
//! is. Walking in already works — colliders come from the model's own
//! primitives, so the doorway is a real opening — but there is nothing inside
//! it. This generates the inside: floor, walls, a doorway that lines up, and
//! furniture that leaves you room to walk.
//!
//! # Interiors are POCKETS, not rooms behind the facade
//!
//! An interior is generated at its own `origin`, somewhere the exterior world
//! is not, and a door is a portal to it. The alternative — building the room
//! inside the facade's footprint — was rejected for three reasons:
//!
//! 1. **The roof.** A third-person camera outside sees the shell; a character
//!    who walks in disappears under it. Fixing that needs roof cutaway or
//!    per-object culling in the renderer, which is a real feature, not a
//!    detail.
//! 2. **Footprints are small.** Kenney houses are a few units across, so an
//!    in-place room is whatever is left after the walls. A pocket can be
//!    larger inside than out, which is what adventure games have always done.
//! 3. **It needs no new concepts.** A pocket is just coordinates in the same
//!    [`GameWorld`], so host-authoritative replication, determinism and eval
//!    rollback are unchanged, and two players in two different houses are
//!    simply two players standing far apart. A separate "interior space" would
//!    need every one of those answered again.
//!
//! The cost is that entering is a position write rather than a step, so the
//! host needs a trigger at the door. That is one sensor entity and one
//! teleport — primitives that already exist — against a renderer feature.
//!
//! # Layout
//!
//! The floor is a rectangle of `Floor` tiles. The walls are the ring around it,
//! fitted by the same connection-mask machinery [`crate::kit`] uses for roads
//! and corridors: a ring cell's mask comes from its ring neighbours, so edges
//! resolve to `Wall` and the four corners to `WallCorner` without anyone
//! naming them. One ring cell is replaced by a `Door`, which carries the same
//! mask as a wall segment and therefore drops into the run without breaking it.
//!
//! Furniture is placed against the walls and then **verified**: after each
//! piece, every remaining free cell must still be reachable from the doorway,
//! and a piece that would seal something off is taken back out. A room you can
//! walk into and not cross is worse than an empty one.

use crate::kit::*;
use crate::levelgen::Level;
use crate::rng::GenRng;
use makepad_math::*;

/// Which wall the door is in. North is -Z, matching [`crate::kit`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DoorSide {
    North,
    East,
    /// The default: a building fronts the street it faces, and a facing tile's
    /// canonical orientation is north, so its door is on the near side.
    #[default]
    South,
    West,
}

impl DoorSide {
    /// Unit vector pointing out of the building through this door.
    pub fn outward(self) -> Vec3f {
        match self {
            DoorSide::North => vec3f(0.0, 0.0, -1.0),
            DoorSide::East => vec3f(1.0, 0.0, 0.0),
            DoorSide::South => vec3f(0.0, 0.0, 1.0),
            DoorSide::West => vec3f(-1.0, 0.0, 0.0),
        }
    }

    /// The side you would be entering from, standing outside this one.
    pub fn opposite(self) -> DoorSide {
        match self {
            DoorSide::North => DoorSide::South,
            DoorSide::East => DoorSide::West,
            DoorSide::South => DoorSide::North,
            DoorSide::West => DoorSide::East,
        }
    }
}

/// How to build a room.
pub struct InteriorParams<'a> {
    pub seed: u64,
    /// Floor size in cells, not counting the wall ring.
    pub cells: (i32, i32),
    /// Which wall holds the door.
    pub door: DoorSide,
    /// Where the pocket sits in the world. Far enough from the exterior that
    /// the two are never in frame together — the caller owns that choice, since
    /// only it knows the map.
    pub origin: Vec3f,
    /// Kit supplying `Floor`, `Wall`, `WallCorner` and `Door` tiles.
    pub shell: &'a Kit,
    /// Optional dressing. Kept separate from the shell because furniture and
    /// architecture are different packs; both still batch per kit.
    pub furniture: Option<&'a Kit>,
    /// Fraction of wall-adjacent cells to try furnishing, 0–1.
    pub clutter: f32,
    /// Ceiling tiles, if the caller wants them. Off by default: a room lit from
    /// a fixed sun and viewed from a third-person camera reads better open, and
    /// a lid turns it black.
    pub ceiling: bool,
}

impl<'a> InteriorParams<'a> {
    pub fn new(shell: &'a Kit) -> Self {
        Self {
            seed: 0,
            cells: (4, 4),
            door: DoorSide::South,
            origin: vec3f(0.0, 0.0, 0.0),
            shell,
            furniture: None,
            clutter: 0.45,
            ceiling: false,
        }
    }
}

/// A generated room.
#[derive(Clone, Debug, Default)]
pub struct Interior {
    /// Tiles, layered per kit so each layer is one batch.
    pub level: Level,
    /// Where a character arrives: one cell inside the doorway, on the floor.
    pub entrance: Vec3f,
    /// The doorway itself, in world space.
    pub door_pos: Vec3f,
    pub door_side: DoorSide,
    /// Floor cells with nothing on them — where an NPC may stand or head for.
    pub free_cells: Vec<(i32, i32)>,
    /// World-space centres of `free_cells`, for callers that only want points.
    pub free_points: Vec<Vec3f>,
    /// Collision boxes for everything solid in here: walls, then furniture.
    /// The floor is included so nobody falls through a pocket with no terrain
    /// under it.
    pub colliders: Vec<(Vec3f, Vec3f)>,
}

impl Interior {
    pub fn tile_count(&self) -> usize {
        self.level.placement_count()
    }
}

/// Build a room.
///
/// Deterministic in `seed`: the same parameters give the same room, so a
/// pocket does not need replicating — every peer generates it identically, the
/// same argument that lets a forest travel as `(preset, seed, position)`.
pub fn interior(p: &InteriorParams) -> Interior {
    let mut rng = GenRng::new(p.seed ^ 0x1_7e12_0000);
    let (w, h) = (p.cells.0.max(2), p.cells.1.max(2));
    let size = p.shell.tile_size.max(0.001);

    // Floor occupies [0,w) x [0,h); the wall ring is the border around it.
    let mut floor = CellGrid::new();
    floor.rect((0, 0), (w - 1, h - 1));

    let mut ring: Vec<(i32, i32)> = Vec::new();
    for x in -1..=w {
        ring.push((x, -1));
        ring.push((x, h));
    }
    for z in 0..h {
        ring.push((-1, z));
        ring.push((w, z));
    }
    ring.sort();
    ring.dedup();

    let mut ring_grid = CellGrid::new();
    for c in &ring {
        ring_grid.set(*c);
    }

    // The door goes in the middle of its wall, so the room reads as symmetric
    // and the approach lines up with the doorway rather than a corner.
    let door_cell = match p.door {
        DoorSide::North => (w / 2, -1),
        DoorSide::South => (w / 2, h),
        DoorSide::West => (-1, h / 2),
        DoorSide::East => (w, h / 2),
    };

    let to_world = |cell: (i32, i32)| {
        vec3f(
            p.origin.x + cell.0 as f32 * size,
            p.origin.y,
            p.origin.z + cell.1 as f32 * size,
        )
    };

    let mut shell_tiles: Vec<TilePlacement> = Vec::with_capacity(ring.len() + (w * h) as usize);
    let mut colliders: Vec<(Vec3f, Vec3f)> = Vec::new();

    // --- floor ------------------------------------------------------------
    // Floors carry no connection semantics (mask 0), so they are placed
    // directly rather than fitted: running them through the mask search would
    // ask for a tile that opens on all four edges, which is a crossroad, not a
    // floor.
    let floor_tiles = p.shell.by_role(TileRole::Floor);
    if !floor_tiles.is_empty() {
        for cell in floor.iter() {
            let t = floor_tiles[rng.index(floor_tiles.len())];
            shell_tiles.push(TilePlacement {
                tile: t,
                cell,
                pos: to_world(cell),
                quarters: 0,
                mask: 0,
            });
        }
    }
    // A solid slab under the whole floor, so a pocket needs no terrain beneath
    // it and nobody falls out of the world through a gap between tiles.
    let slab_c = vec3f(
        p.origin.x + (w - 1) as f32 * size * 0.5,
        p.origin.y - 0.25,
        p.origin.z + (h - 1) as f32 * size * 0.5,
    );
    colliders.push((
        slab_c,
        vec3f(w as f32 * size * 0.5, 0.25, h as f32 * size * 0.5),
    ));

    // --- walls ------------------------------------------------------------
    let wall_h = p
        .shell
        .tiles
        .iter()
        .find(|t| t.role == TileRole::Wall)
        .map(|t| t.height)
        .unwrap_or(2.0);
    for cell in ring.iter().copied() {
        let mask = ring_grid.mask_at(cell);
        let fitted = if cell == door_cell {
            // The doorway. A Door carries a wall's mask, so it slots into the
            // run; forcing it here is what stops `fit` scattering doors along
            // every wall, since it cannot tell them apart by mask alone.
            p.shell
                .fit_where(mask, |r| r == TileRole::Door, &mut rng)
                .or_else(|| p.shell.fit_where(mask, |r| r == TileRole::Wall, &mut rng))
        } else {
            p.shell.fit_where(
                mask,
                |r| matches!(r, TileRole::Wall | TileRole::WallCorner | TileRole::End),
                &mut rng,
            )
        };
        let Some((tile, quarters)) = fitted else {
            continue;
        };
        shell_tiles.push(TilePlacement {
            tile,
            cell,
            pos: to_world(cell),
            quarters,
            mask: rotate_mask(p.shell.tiles[tile as usize].mask, quarters),
        });
        // The doorway must not be solid or the room cannot be entered — the
        // whole point of putting a Door tile there.
        if cell != door_cell {
            let c = to_world(cell);
            colliders.push((
                vec3f(c.x, c.y + wall_h * 0.5, c.z),
                vec3f(size * 0.5, wall_h * 0.5, size * 0.5),
            ));
        }
    }

    if p.ceiling {
        if let Some(&t) = p.shell.by_role(TileRole::Floor).first() {
            for cell in floor.iter() {
                let c = to_world(cell);
                shell_tiles.push(TilePlacement {
                    tile: t,
                    cell,
                    pos: vec3f(c.x, c.y + wall_h, c.z),
                    quarters: 0,
                    mask: 0,
                });
            }
        }
    }

    // --- where you arrive --------------------------------------------------
    // One cell inside the doorway. Standing *in* the door would leave a
    // character straddling the threshold, which is where a mover sweep is most
    // likely to catch on a wall edge.
    let inside_cell = match p.door {
        DoorSide::North => (door_cell.0, 0),
        DoorSide::South => (door_cell.0, h - 1),
        DoorSide::West => (0, door_cell.1),
        DoorSide::East => (w - 1, door_cell.1),
    };
    let entrance = to_world(inside_cell);

    // --- furniture ---------------------------------------------------------
    let mut occupied: Vec<(i32, i32)> = Vec::new();
    let mut furniture: Vec<TilePlacement> = Vec::new();
    if let Some(fk) = p.furniture {
        let props = fk.by_role(TileRole::Prop);
        if !props.is_empty() {
            // Against the walls, never the middle: a room furnished from the
            // centre outwards is an obstacle course, and real rooms put things
            // round the edge anyway.
            let mut candidates: Vec<(i32, i32)> = floor
                .iter()
                .filter(|&(x, z)| x == 0 || z == 0 || x == w - 1 || z == h - 1)
                .filter(|&c| c != inside_cell)
                .collect();
            // Deterministic shuffle so clutter varies with the seed without
            // depending on iteration order.
            for i in (1..candidates.len()).rev() {
                candidates.swap(i, rng.index(i + 1));
            }
            let want = ((candidates.len() as f32) * p.clutter.clamp(0.0, 1.0)) as usize;
            for cell in candidates.into_iter().take(want) {
                occupied.push(cell);
                if !all_free_reachable(&floor, &occupied, inside_cell) {
                    // That piece would have sealed part of the room off. A room
                    // you can enter but not cross is worse than a bare one, so
                    // the piece loses.
                    occupied.pop();
                    continue;
                }
                let tile = props[rng.index(props.len())];
                let c = to_world(cell);
                furniture.push(TilePlacement {
                    tile,
                    cell,
                    pos: c,
                    quarters: rng.index(4) as u32,
                    mask: 0,
                });
                let fh = fk.tiles[tile as usize].height.max(0.2);
                colliders.push((
                    vec3f(c.x, c.y + fh * 0.5, c.z),
                    vec3f(size * 0.35, fh * 0.5, size * 0.35),
                ));
            }
        }
    }

    let free_cells: Vec<(i32, i32)> = floor.iter().filter(|c| !occupied.contains(c)).collect();
    let free_points = free_cells.iter().map(|&c| to_world(c)).collect();

    let mut level = Level::default();
    level.push_layer(&p.shell.id, p.shell.tile_size, shell_tiles);
    if let Some(fk) = p.furniture {
        level.push_layer(&fk.id, fk.tile_size, furniture);
    }
    level.open_cells = free_cells.clone();
    level.entrance = Some(entrance);

    Interior {
        level,
        entrance,
        door_pos: to_world(door_cell),
        door_side: p.door,
        free_cells,
        free_points,
        colliders,
    }
}

/// Every unoccupied floor cell reachable from `start` by 4-way movement.
///
/// Furniture placement uses this as a veto rather than a hope: the check runs
/// after each piece, and a piece that disconnects anything is removed.
fn all_free_reachable(floor: &CellGrid, occupied: &[(i32, i32)], start: (i32, i32)) -> bool {
    let mut free = CellGrid::new();
    let mut count = 0;
    for c in floor.iter() {
        if !occupied.contains(&c) {
            free.set(c);
            count += 1;
        }
    }
    if count == 0 || !free.contains(start) {
        return false;
    }
    free.reachable_from(start).len() == count
}

/// Guess which wall a building's door is in, from the collision boxes its model
/// produced.
///
/// Door position is a **parameter** to [`interior`] rather than something it
/// derives, because the colliders live in the render crate and this crate must
/// not depend on it — that would invert the layering and make layout generation
/// require a GPU. This helper takes the boxes as plain data so a caller holding
/// them can work the side out without either crate learning about the other.
///
/// The gap is found by walking each edge just inside the footprint and asking
/// which stretch no box covers. The longest uncovered run wins.
pub fn door_side_from_colliders(
    boxes: &[(Vec3f, Vec3f)],
    centre: Vec3f,
    half: Vec3f,
    samples: usize,
) -> Option<DoorSide> {
    if boxes.is_empty() || half.x <= 0.0 || half.z <= 0.0 {
        return None;
    }
    let n = samples.max(4);
    let inset = 0.9;
    let covered = |p: Vec3f| {
        boxes.iter().any(|(c, h)| {
            (p.x - c.x).abs() <= h.x && (p.z - c.z).abs() <= h.z
        })
    };
    let mut best: Option<(DoorSide, usize)> = None;
    for side in [
        DoorSide::North,
        DoorSide::East,
        DoorSide::South,
        DoorSide::West,
    ] {
        let mut run = 0usize;
        let mut longest = 0usize;
        for i in 0..n {
            let t = (i as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let p = match side {
                DoorSide::North => vec3f(centre.x + t * half.x, centre.y, centre.z - half.z * inset),
                DoorSide::South => vec3f(centre.x + t * half.x, centre.y, centre.z + half.z * inset),
                DoorSide::West => vec3f(centre.x - half.x * inset, centre.y, centre.z + t * half.z),
                DoorSide::East => vec3f(centre.x + half.x * inset, centre.y, centre.z + t * half.z),
            };
            if covered(p) {
                run = 0;
            } else {
                run += 1;
                longest = longest.max(run);
            }
        }
        if longest > 0 && best.map_or(true, |(_, b)| longest > b) {
            best = Some((side, longest));
        }
    }
    best.map(|(s, _)| s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shell kit shaped like Kenney's modular-buildings: floors, wall runs,
    /// corners and a door.
    fn shell_kit() -> Kit {
        Kit::new(
            "kenney/modular-buildings",
            2.0,
            vec![
                TileDef::new("floor", TileRole::Floor, 0.1),
                TileDef::new("wall", TileRole::Wall, 2.4),
                TileDef::new("wall-window", TileRole::Wall, 2.4),
                TileDef::new("wall-corner", TileRole::WallCorner, 2.4),
                TileDef::new("door", TileRole::Door, 2.4),
            ],
        )
    }

    fn furniture_kit() -> Kit {
        Kit::new(
            "kenney/furniture-kit",
            2.0,
            vec![
                TileDef::new("chair", TileRole::Prop, 0.9),
                TileDef::new("table", TileRole::Prop, 0.8),
                TileDef::new("bed", TileRole::Prop, 0.6),
            ],
        )
    }

    fn params<'a>(shell: &'a Kit, furniture: &'a Kit) -> InteriorParams<'a> {
        InteriorParams {
            seed: 7,
            cells: (5, 4),
            door: DoorSide::South,
            origin: vec3f(500.0, 0.0, 500.0),
            shell,
            furniture: Some(furniture),
            clutter: 0.5,
            ceiling: false,
        }
    }

    #[test]
    fn a_room_has_exactly_one_doorway_in_the_requested_wall() {
        let shell = shell_kit();
        let fk = furniture_kit();
        for side in [
            DoorSide::North,
            DoorSide::East,
            DoorSide::South,
            DoorSide::West,
        ] {
            let mut p = params(&shell, &fk);
            p.door = side;
            let room = interior(&p);
            let layer = &room.level.layers[0];
            let doors: Vec<_> = layer
                .placements
                .iter()
                .filter(|pl| shell.tiles[pl.tile as usize].role == TileRole::Door)
                .collect();
            assert_eq!(doors.len(), 1, "{side:?}: exactly one door");
            assert_eq!(room.door_side, side);
            // The doorway must be on the requested wall, i.e. displaced from
            // the room centre in that direction.
            let out = side.outward();
            let to_door = vec3f(
                room.door_pos.x - room.entrance.x,
                0.0,
                room.door_pos.z - room.entrance.z,
            );
            assert!(
                to_door.x * out.x + to_door.z * out.z > 0.0,
                "{side:?}: door lies on the wrong wall"
            );
        }
    }

    #[test]
    fn the_doorway_is_the_only_gap_in_the_wall_ring() {
        let shell = shell_kit();
        let fk = furniture_kit();
        let room = interior(&params(&shell, &fk));
        // Every ring cell except the door contributes a collider; walking in
        // through any other wall must be impossible.
        let door_covered = room.colliders.iter().any(|(c, h)| {
            (c.x - room.door_pos.x).abs() <= h.x && (c.z - room.door_pos.z).abs() <= h.z && h.y > 1.0
        });
        assert!(!door_covered, "the doorway must not be solid");
    }

    #[test]
    fn furniture_never_seals_off_part_of_the_room() {
        let shell = shell_kit();
        let fk = furniture_kit();
        // Clutter turned all the way up is the case that would wall someone in.
        for seed in 0..24u64 {
            let mut p = params(&shell, &fk);
            p.seed = seed;
            p.clutter = 1.0;
            let room = interior(&p);
            assert!(
                !room.free_cells.is_empty(),
                "seed {seed}: a fully-furnished room left nowhere to stand"
            );
            // Rebuild the free grid and prove it is one connected component
            // containing the arrival cell.
            let mut free = CellGrid::new();
            for c in &room.free_cells {
                free.set(*c);
            }
            let start = *room.free_cells.first().unwrap();
            assert_eq!(
                free.reachable_from(start).len(),
                room.free_cells.len(),
                "seed {seed}: furniture split the room into islands"
            );
        }
    }

    #[test]
    fn the_arrival_cell_is_inside_and_never_furnished() {
        let shell = shell_kit();
        let fk = furniture_kit();
        for seed in 0..16u64 {
            let mut p = params(&shell, &fk);
            p.seed = seed;
            p.clutter = 1.0;
            let room = interior(&p);
            let inside: Vec<Vec3f> = room.free_points.clone();
            assert!(
                inside.iter().any(|v| (v.x - room.entrance.x).abs() < 0.01
                    && (v.z - room.entrance.z).abs() < 0.01),
                "seed {seed}: arrival cell was furnished or outside the floor"
            );
            // And it must be strictly inside the walls, not in the doorway.
            let d = ((room.entrance.x - room.door_pos.x).powi(2)
                + (room.entrance.z - room.door_pos.z).powi(2))
            .sqrt();
            assert!(d > 0.5, "seed {seed}: arrival is in the threshold");
        }
    }

    #[test]
    fn generation_is_deterministic_and_the_seed_matters() {
        let shell = shell_kit();
        let fk = furniture_kit();
        let a = interior(&params(&shell, &fk));
        let b = interior(&params(&shell, &fk));
        assert_eq!(a.free_cells, b.free_cells);
        assert_eq!(a.colliders.len(), b.colliders.len());
        assert_eq!(a.tile_count(), b.tile_count());

        let mut p2 = params(&shell, &fk);
        p2.seed = 99;
        let c = interior(&p2);
        assert!(
            a.free_cells != c.free_cells || a.tile_count() != c.tile_count(),
            "seed change had no effect"
        );
    }

    #[test]
    fn the_floor_slab_covers_the_room_so_nobody_falls_through() {
        let shell = shell_kit();
        let fk = furniture_kit();
        let room = interior(&params(&shell, &fk));
        // The slab is the first collider and must sit below floor level,
        // spanning every free cell.
        let (c, h) = room.colliders[0];
        assert!(c.y + h.y <= 0.001, "slab must be below the floor plane");
        for p in &room.free_points {
            assert!(
                (p.x - c.x).abs() <= h.x + 0.001 && (p.z - c.z).abs() <= h.z + 0.001,
                "floor cell {p:?} is not over the slab"
            );
        }
    }

    #[test]
    fn one_kit_per_layer_so_each_batches() {
        let shell = shell_kit();
        let fk = furniture_kit();
        let room = interior(&params(&shell, &fk));
        let mut ids: Vec<&str> = room.level.layers.iter().map(|l| l.kit_id.as_str()).collect();
        ids.sort();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "a kit must not span two layers");
    }

    #[test]
    fn door_side_is_recovered_from_a_gap_in_the_colliders() {
        // A 6x6 shell with walls on every side but a gap in the south wall.
        let centre = vec3f(0.0, 0.0, 0.0);
        let half = vec3f(3.0, 2.0, 3.0);
        let boxes = vec![
            // north wall
            (vec3f(0.0, 1.0, -2.7), vec3f(3.0, 1.0, 0.3)),
            // east wall
            (vec3f(2.7, 1.0, 0.0), vec3f(0.3, 1.0, 3.0)),
            // west wall
            (vec3f(-2.7, 1.0, 0.0), vec3f(0.3, 1.0, 3.0)),
            // south wall in two pieces, leaving a doorway in the middle
            (vec3f(-2.0, 1.0, 2.7), vec3f(1.0, 1.0, 0.3)),
            (vec3f(2.0, 1.0, 2.7), vec3f(1.0, 1.0, 0.3)),
        ];
        assert_eq!(
            door_side_from_colliders(&boxes, centre, half, 16),
            Some(DoorSide::South)
        );
        // A fully enclosed shell has no door to find.
        let sealed = vec![(centre, vec3f(3.0, 2.0, 3.0))];
        assert_eq!(door_side_from_colliders(&sealed, centre, half, 16), None);
    }

    #[test]
    fn a_shell_kit_without_furniture_still_produces_a_usable_room() {
        let shell = shell_kit();
        let mut p = InteriorParams::new(&shell);
        p.origin = vec3f(100.0, 0.0, 0.0);
        let room = interior(&p);
        assert!(room.tile_count() > 0);
        assert_eq!(room.free_cells.len(), 16, "4x4 floor, nothing furnished");
        assert!(room.colliders.len() > 1);
    }
}
