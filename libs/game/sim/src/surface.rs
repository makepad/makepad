//! Per-prop walkable top surfaces — the roof counterpart of [`crate::terrain`].
//!
//! A static prop collider is an AABB and an AABB's lid is flat, so a walker
//! landing on a gabled roof stood on invisible air at ridge height. A
//! [`SurfaceGrid`] carries the prop's real top surface as a coarse column
//! grid, and the step resolves movers against it the way it already resolves
//! them against the terrain heightfield — any roof shape becomes walkable
//! without teaching the axis sweeps general sloped-solid collision.
//!
//! Surfaces are ONE-SIDED floors. A column only counts as ground for feet
//! already within step reach of it; higher columns are overhead geometry —
//! the eaves over a doorway — which is what keeps a covered doorway passable
//! while the roof above it still catches whoever drops onto it. Blocking the
//! prop's sides stays the job of its ordinary box parts (a house's walls).
//!
//! Everything here is pure f32 arithmetic over Vecs in entity order: safe for
//! lockstep. Grids are `Arc`'d on the entity so world snapshots stay cheap.

use std::sync::Arc;

use makepad_math::*;

/// Cells per axis when rasterizing a prop's mesh into a surface. 16 puts
/// half-unit columns under a typical house footprint; the bilinear nodes make
/// a planar pitch resolve exactly regardless, so more buys little.
pub const SURFACE_CELLS: usize = 16;

/// Node heights over a prop's footprint, ENTITY-LOCAL (world = entity pos +
/// local), row-major `z * (nx + 1) + x` and bilinearly interpolated like the
/// terrain — a planar roof pitch is reproduced exactly, not as a staircase.
/// `NEG_INFINITY` marks a node with no mesh below it (past a gable end); any
/// cell touching one reads as a hole, so walkers drop off where the roof ends.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceGrid {
    pub nx: usize,
    pub nz: usize,
    pub min_x: f32,
    pub min_z: f32,
    pub cell_x: f32,
    pub cell_z: f32,
    pub heights: Vec<f32>,
}

impl SurfaceGrid {
    /// Interpolated surface height at an entity-local (x, z), or `None`
    /// outside the footprint or over a hole.
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        let fx = (x - self.min_x) / self.cell_x;
        let fz = (z - self.min_z) / self.cell_z;
        if fx < 0.0 || fz < 0.0 || fx >= self.nx as f32 || fz >= self.nz as f32 {
            return None;
        }
        let ix = fx.floor() as usize;
        let iz = fz.floor() as usize;
        let u = fx - ix as f32;
        let v = fz - iz as f32;
        let n = self.nx + 1;
        let h00 = self.heights[iz * n + ix];
        let h10 = self.heights[iz * n + ix + 1];
        let h01 = self.heights[(iz + 1) * n + ix];
        let h11 = self.heights[(iz + 1) * n + ix + 1];
        if !(h00.is_finite() && h10.is_finite() && h01.is_finite() && h11.is_finite()) {
            return None;
        }
        Some(
            h00 * (1.0 - u) * (1.0 - v)
                + h10 * u * (1.0 - v)
                + h01 * (1.0 - u) * v
                + h11 * u * v,
        )
    }

    /// Re-express a world-space grid relative to the entity that will carry
    /// it, so the data rides the entity like every other pose-local quantity.
    pub fn rebased(mut self, pos: Vec3f) -> SurfaceGrid {
        self.min_x -= pos.x;
        self.min_z -= pos.z;
        for h in &mut self.heights {
            if h.is_finite() {
                *h -= pos.y;
            }
        }
        self
    }
}

/// The slice of a surface-carrying static the mover pass reads, snapshotted
/// once per tick alongside the `Solid` list (same borrow reasoning).
#[derive(Clone)]
pub struct SurfaceSolid {
    pub id: u64,
    pub pos: Vec3f,
    pub grid: Arc<SurfaceGrid>,
}

/// Highest REACHABLE surface under a mover's footprint, with the id of the
/// entity carrying it. Probes corners + centre exactly as `Terrain::floor_under`
/// does. `reach` is the feet height a step can climb to (feet + CLIMB);
/// columns above it never grab a mover walking beneath them — that filter is
/// the entire one-sidedness of a surface.
pub fn surface_floor_under(
    surfaces: &[SurfaceSolid],
    pos: Vec3f,
    half: Vec3f,
    reach: f32,
) -> Option<(f32, u64)> {
    let mut best: Option<(f32, u64)> = None;
    for s in surfaces {
        for (dx, dz) in [
            (0.0, 0.0),
            (-half.x, -half.z),
            (half.x, -half.z),
            (-half.x, half.z),
            (half.x, half.z),
        ] {
            if let Some(h) = s.grid.height_at(pos.x + dx - s.pos.x, pos.z + dz - s.pos.z) {
                let h = h + s.pos.y;
                if h <= reach && best.map_or(true, |(by, _)| h > by) {
                    best = Some((h, s.id));
                }
            }
        }
    }
    best
}

/// Does the surface this mover STANDS ON rise past `reach` under its moved
/// footprint? The gate on `floor_id` is deliberate: from the ground a roof's
/// columns are overhead geometry and the prop's wall boxes do the blocking,
/// but for someone already on the roof a chimney-sized jump in the grid must
/// block like a terrain cliff.
pub fn surface_rise_blocks(
    surfaces: &[SurfaceSolid],
    floor_id: u64,
    pos: Vec3f,
    half: Vec3f,
    reach: f32,
) -> bool {
    if floor_id == 0 {
        return false;
    }
    let Some(s) = surfaces.iter().find(|s| s.id == floor_id) else {
        return false;
    };
    for (dx, dz) in [
        (0.0, 0.0),
        (-half.x, -half.z),
        (half.x, -half.z),
        (-half.x, half.z),
        (half.x, half.z),
    ] {
        if let Some(h) = s.grid.height_at(pos.x + dx - s.pos.x, pos.z + dz - s.pos.z) {
            if h + s.pos.y > reach {
                return true;
            }
        }
    }
    false
}

/// Rasterize a triangle mesh's TOP surface into a node grid: per node, the
/// highest triangle covering it. Positions are taken as given (already in
/// whatever space the caller wants the grid in); the footprint is the mesh's
/// own xz bounds. `None` when the mesh is degenerate in x or z.
pub fn rasterize_top_grid(
    positions: &[Vec3f],
    indices: &[u32],
    nx: usize,
    nz: usize,
) -> Option<SurfaceGrid> {
    if positions.is_empty() || indices.len() < 3 || nx == 0 || nz == 0 {
        return None;
    }
    let (mut min_x, mut max_x) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut min_z, mut max_z) = (f32::INFINITY, f32::NEG_INFINITY);
    for p in positions {
        min_x = min_x.min(p.x);
        max_x = max_x.max(p.x);
        min_z = min_z.min(p.z);
        max_z = max_z.max(p.z);
    }
    let cell_x = (max_x - min_x) / nx as f32;
    let cell_z = (max_z - min_z) / nz as f32;
    if !(cell_x > 0.0) || !(cell_z > 0.0) {
        return None;
    }
    let mut heights = vec![f32::NEG_INFINITY; (nx + 1) * (nz + 1)];
    // Vertical faces (walls, gable ends) project to a sliver and are never a
    // top surface; the area floor drops them before 1/area blows up.
    let min_area = cell_x * cell_z * 1.0e-4;
    for tri in indices.chunks_exact(3) {
        let a = positions[tri[0] as usize];
        let b = positions[tri[1] as usize];
        let c = positions[tri[2] as usize];
        let area = (b.x - a.x) * (c.z - a.z) - (b.z - a.z) * (c.x - a.x);
        if area.abs() < min_area {
            continue;
        }
        let inv = 1.0 / area;
        let tx0 = a.x.min(b.x).min(c.x);
        let tx1 = a.x.max(b.x).max(c.x);
        let tz0 = a.z.min(b.z).min(c.z);
        let tz1 = a.z.max(b.z).max(c.z);
        let i0 = (((tx0 - min_x) / cell_x - 1.0e-3).ceil().max(0.0)) as usize;
        let i1 = ((((tx1 - min_x) / cell_x + 1.0e-3).floor()) as isize).min(nx as isize);
        let j0 = (((tz0 - min_z) / cell_z - 1.0e-3).ceil().max(0.0)) as usize;
        let j1 = ((((tz1 - min_z) / cell_z + 1.0e-3).floor()) as isize).min(nz as isize);
        if i1 < i0 as isize || j1 < j0 as isize {
            continue;
        }
        for j in j0..=j1 as usize {
            let z = min_z + j as f32 * cell_z;
            for i in i0..=i1 as usize {
                let x = min_x + i as f32 * cell_x;
                let wa = ((b.x - x) * (c.z - z) - (b.z - z) * (c.x - x)) * inv;
                let wb = ((c.x - x) * (a.z - z) - (c.z - z) * (a.x - x)) * inv;
                let wc = ((a.x - x) * (b.z - z) - (a.z - z) * (b.x - x)) * inv;
                // Tolerance keeps nodes ON the mesh outline covered — the
                // eave line is exactly the footprint edge.
                const TOL: f32 = 1.0e-3;
                if wa < -TOL || wb < -TOL || wc < -TOL {
                    continue;
                }
                let h = wa * a.y + wb * b.y + wc * c.y;
                let node = &mut heights[j * (nx + 1) + i];
                if h > *node {
                    *node = h;
                }
            }
        }
    }
    if !heights.iter().any(|h| h.is_finite()) {
        return None;
    }
    Some(SurfaceGrid {
        nx,
        nz,
        min_x,
        min_z,
        cell_x,
        cell_z,
        heights,
    })
}

#[cfg(test)]
mod grid_tests {
    use super::*;

    /// A gable running along x: eave height at z = ±half_z, ridge at z = 0.
    fn gable_mesh(half_x: f32, half_z: f32, eave: f32, ridge: f32) -> (Vec<Vec3f>, Vec<u32>) {
        let positions = vec![
            vec3f(-half_x, eave, -half_z),
            vec3f(half_x, eave, -half_z),
            vec3f(-half_x, ridge, 0.0),
            vec3f(half_x, ridge, 0.0),
            vec3f(-half_x, eave, half_z),
            vec3f(half_x, eave, half_z),
        ];
        let indices = vec![0, 1, 2, 1, 3, 2, 2, 3, 4, 3, 5, 4];
        (positions, indices)
    }

    #[test]
    fn rasterized_gable_reproduces_the_pitch() {
        let (pos, idx) = gable_mesh(2.0, 3.0, 1.0, 2.5);
        let g = rasterize_top_grid(&pos, &idx, 12, 12).unwrap();
        // Mid-slope: halfway from eave to ridge.
        let h = g.height_at(0.0, 1.5).unwrap();
        assert!((h - 1.75).abs() < 0.05, "mid-slope height {h}, expected 1.75");
        let r = g.height_at(0.5, 0.0).unwrap();
        assert!((r - 2.5).abs() < 0.05, "ridge height {r}, expected 2.5");
        let e = g.height_at(-1.0, -2.9).unwrap();
        assert!((e - 1.05).abs() < 0.08, "near-eave height {e}, expected ~1.05");
        assert!(g.height_at(2.5, 0.0).is_none(), "outside the footprint");
    }

    #[test]
    fn holes_and_edges_return_no_surface() {
        let g = SurfaceGrid {
            nx: 2,
            nz: 1,
            min_x: 0.0,
            min_z: 0.0,
            cell_x: 1.0,
            cell_z: 1.0,
            heights: vec![1.0, 1.0, f32::NEG_INFINITY, 1.0, 1.0, f32::NEG_INFINITY],
        };
        assert!((g.height_at(0.5, 0.5).unwrap() - 1.0).abs() < 1.0e-6);
        // The right cell touches -inf nodes: a hole, not a floor at -inf.
        assert!(g.height_at(1.5, 0.5).is_none());
        assert!(g.height_at(-0.1, 0.5).is_none());
        assert!(g.height_at(0.5, 1.1).is_none());
    }
}

#[cfg(test)]
mod roof_step_tests {
    use super::*;
    use crate::entity::*;
    use crate::step::step_world;
    use crate::world::GameWorld;

    /// A house stand-in: ground plane, one wall slab (z in [-1, 1], top at
    /// y=3) and a hidden roof entity whose box spans y 3.0..4.5 but whose
    /// SURFACE is a gable — eaves at 3.0 (z = ±1.8), ridge 4.5 (z = 0). The
    /// roof overhangs the wall by 0.8 on each side, like a porch.
    fn world_with_roof() -> (GameWorld, u64) {
        let mut w = GameWorld::new();
        let base = Entity {
            kind: BodyKind::Static,
            shape: Shape::Box,
            collide: true,
            push_mass: 1.0,
            speed_mult: 1.0,
            scale: vec3f(1.0, 1.0, 1.0),
            scale_target: vec3f(1.0, 1.0, 1.0),
            density: 1.0,
            friction: 0.6,
            ..Default::default()
        };
        w.next_id += 1;
        w.push_entity(Entity {
            id: w.next_id,
            pos: vec3f(0.0, -0.5, 0.0),
            half: vec3f(60.0, 0.5, 60.0),
            ..base.clone()
        });
        w.next_id += 1;
        w.push_entity(Entity {
            id: w.next_id,
            pos: vec3f(0.0, 1.5, 0.0),
            half: vec3f(2.0, 1.5, 1.0),
            ..base.clone()
        });
        w.next_id += 1;
        let roof = w.next_id;
        let pos = vec3f(0.0, 3.75, 0.0);
        let (nx, nz, half_x, half_z) = (8usize, 8usize, 2.2f32, 1.8f32);
        let mut heights = Vec::new();
        for iz in 0..=nz {
            let z = -half_z + iz as f32 * (2.0 * half_z / nz as f32);
            let world_h = 3.0 + (1.0 - z.abs() / half_z) * 1.5;
            for _ix in 0..=nx {
                heights.push(world_h - pos.y);
            }
        }
        w.push_entity(Entity {
            id: roof,
            pos,
            half: vec3f(half_x, 0.75, half_z),
            hidden: true,
            surface: Some(std::sync::Arc::new(SurfaceGrid {
                nx,
                nz,
                min_x: -half_x,
                min_z: -half_z,
                cell_x: 2.0 * half_x / nx as f32,
                cell_z: 2.0 * half_z / nz as f32,
                heights,
            })),
            ..base
        });
        w.next_id += 1;
        w.push_entity(Entity {
            id: w.next_id,
            kind: BodyKind::Mover,
            shape: Shape::Box,
            pos: vec3f(0.0, 0.9, 6.0),
            half: vec3f(0.35, 0.9, 0.35),
            collide: true,
            gravity_scale: 1.0,
            push_mass: 1.0,
            speed_mult: 1.0,
            scale: vec3f(1.0, 1.0, 1.0),
            scale_target: vec3f(1.0, 1.0, 1.0),
            density: 1.0,
            friction: 0.6,
            ..Default::default()
        });
        (w, roof)
    }

    fn walker(w: &GameWorld) -> Entity {
        w.entities.iter().find(|e| e.kind == BodyKind::Mover).unwrap().clone()
    }

    fn walker_mut(w: &mut GameWorld) -> &mut Entity {
        w.entities.iter_mut().find(|e| e.kind == BodyKind::Mover).unwrap()
    }

    /// The reported bug: landing on a house put you on the collider box's
    /// flat lid (ridge height everywhere) instead of on the roof. Dropped
    /// over the lower slope, the walker must come to rest ON the pitch.
    #[test]
    fn a_faller_lands_on_the_pitch_not_on_the_lid() {
        let (mut w, roof) = world_with_roof();
        walker_mut(&mut w).pos = vec3f(0.0, 8.0, 1.4);
        for _ in 0..120 {
            step_world(&mut w);
        }
        let end = walker(&w);
        assert!(end.on_floor, "not on_floor while resting on the roof");
        assert_eq!(end.floor_id, roof, "floor is {} — not the roof surface", end.floor_id);
        // Footprint probes stand on the uphill corner: surface at z = 1.05,
        // 3.0 + (1 - 1.05/1.8)·1.5 = 3.625, plus half a body.
        assert!(
            (end.pos.y - (3.625 + 0.9)).abs() < 0.15,
            "resting at y={:.2}, expected ~4.53 on the slope (the flat lid would be 5.40)",
            end.pos.y
        );
    }

    /// Walking uphill: y must follow the gradient to the ridge like a ramp.
    #[test]
    fn a_walker_climbs_the_roof_like_a_ramp() {
        let (mut w, _) = world_with_roof();
        walker_mut(&mut w).pos = vec3f(0.0, 4.4, 1.6);
        for _ in 0..30 {
            step_world(&mut w);
        }
        let start = walker(&w).pos;
        // 60 ticks at 1.5 u/s lands the walker at the ridge (z ≈ 0.1); any
        // longer and it correctly walks over and down the far slope.
        for _ in 0..60 {
            let e = walker_mut(&mut w);
            e.vel.x = 0.0;
            e.vel.z = -1.5;
            step_world(&mut w);
        }
        let end = walker(&w);
        assert!(
            end.pos.z < 0.3,
            "only advanced to z={:.2} — stuck on the pitch",
            end.pos.z
        );
        assert!(
            end.pos.y > start.y + 0.8 && end.pos.y > 5.0,
            "reached z={:.2} but y={:.2} (from {:.2}) — did not follow the gradient",
            end.pos.z,
            end.pos.y,
            start.y
        );
        assert!(end.on_floor, "not on_floor while walking the slope");
    }

    /// The house's side must still be a wall: the surface replaces the roof
    /// part's lid, never the wall parts beneath it.
    #[test]
    fn the_wall_below_still_blocks() {
        let (mut w, _) = world_with_roof();
        for _ in 0..180 {
            let e = walker_mut(&mut w);
            e.vel.x = 0.0;
            e.vel.z = -2.0;
            step_world(&mut w);
        }
        let end = walker(&w);
        assert!(
            end.pos.z > 1.3,
            "walker at z={:.2} pushed through the wall face at z=1.0",
            end.pos.z
        );
        assert!(
            end.pos.y < 1.2,
            "walker climbed to y={:.2} against a vertical wall",
            end.pos.y
        );
    }

    /// One-sidedness: under the eave overhang (roof above, no wall) a ground
    /// walker passes freely — neither blocked by the roof's columns nor
    /// teleported up onto them.
    #[test]
    fn a_covered_walkway_stays_passable() {
        let (mut w, _) = world_with_roof();
        walker_mut(&mut w).pos = vec3f(-4.0, 0.9, 1.45);
        for _ in 0..240 {
            let e = walker_mut(&mut w);
            e.vel.x = 2.0;
            e.vel.z = 0.0;
            step_world(&mut w);
        }
        let end = walker(&w);
        assert!(
            end.pos.x > 3.5,
            "walker stopped at x={:.2} under the eave — the overhang blocked like a wall",
            end.pos.x
        );
        assert!(
            end.pos.y < 1.2,
            "walker lifted to y={:.2} by a surface that should be out of reach",
            end.pos.y
        );
    }
}
