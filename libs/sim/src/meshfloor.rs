//! A streamed level's floors as a SURFACE — the seam that lets the
//! procedural generators (railways, roads, race circuits, lots, scatter)
//! treat a loaded Doom/Duke/C&C map as the ground they build on.
//!
//! The host rasterises the level's own walkable graph once per map
//! install — one storey per xz column — into a [`FloorRaster`] and puts it
//! on the world; [`crate::GameWorld::surface_sample_at`] answers from it
//! before the heightfield. The raster is data: it knows nothing about the
//! level format, the nav crate or the renderer, so the sim never gains a
//! dependency on either.
//!
//! Column kinds:
//! - `Floor`: exactly one floor in this column — ground at that height.
//! - `Pit`: one floor, but damaging or liquid (nukage, lava, water). A
//!   `Hole`: nothing is laid at grade; a corridor bridges it or refuses.
//! - `Stacked`: two or more floors in the column (a balcony over a hall).
//!   A 2-D surface cannot name one honestly, so the column answers
//!   `Outside`, and the level's own `ground_under(near_y)` keeps answering
//!   every y-aware query exactly as it did before the raster existed.
//! - `Solid`: no floor at all — wall, rock, or beyond the level.

use crate::world::SurfaceSample;
use makepad_math::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloorKind {
    /// No floor in this column: wall, rock, or beyond the level.
    Solid,
    /// Exactly one floor — ground at [`FloorRaster::height_of`].
    Floor,
    /// One floor of damaging or liquid ground (nukage, lava, water).
    Pit,
    /// Two or more floors stacked in the column (a balcony over a hall).
    Stacked,
}

/// A way in or out of the level — a door, a lift, a teleporter pad — by
/// the name the level gave it. The generators' access points.
#[derive(Clone, Debug, PartialEq)]
pub struct FloorAccess {
    pub name: String,
    pub pos: Vec3f,
}

#[derive(Clone, Debug)]
pub struct FloorRaster {
    min_x: f32,
    min_z: f32,
    cell: f32,
    nx: usize,
    nz: usize,
    kinds: Vec<FloorKind>,
    /// Floor height per column. A column without a floor of its own
    /// carries its nearest floor's height after [`FloorRaster::finish`] —
    /// what a wall's occupied band starts from.
    heights: Vec<f32>,
    pub access: Vec<FloorAccess>,
    /// Where the level says a body starts (feet).
    pub start: Option<Vec3f>,
    /// Bumped by the host on every install, so a cache keyed on the raster
    /// can tell one map load from the next.
    pub revision: u64,
}

impl FloorRaster {
    /// An all-`Solid` raster whose cell (0, 0) has its minimum corner at
    /// (`min_x`, `min_z`).
    pub fn new(min_x: f32, min_z: f32, cell: f32, nx: usize, nz: usize) -> Self {
        let n = nx.max(1) * nz.max(1);
        FloorRaster {
            min_x,
            min_z,
            cell: cell.max(0.001),
            nx: nx.max(1),
            nz: nz.max(1),
            kinds: vec![FloorKind::Solid; n],
            heights: vec![f32::NAN; n],
            access: Vec::new(),
            start: None,
            revision: 0,
        }
    }

    pub fn set(&mut self, ix: usize, iz: usize, kind: FloorKind, height: f32) {
        if ix < self.nx && iz < self.nz {
            let i = iz * self.nx + ix;
            self.kinds[i] = kind;
            self.heights[i] = height;
        }
    }

    /// Give every column without a height of its own the height of its
    /// nearest column that has one (breadth-first, so a wall two cells from
    /// a floor reads that floor, not zero). A raster with no floor at all
    /// reads 0 everywhere.
    pub fn finish(&mut self) {
        let (nx, nz) = (self.nx, self.nz);
        let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
        for (i, h) in self.heights.iter().enumerate() {
            if h.is_finite() {
                queue.push_back(i);
            }
        }
        if queue.is_empty() {
            for h in self.heights.iter_mut() {
                *h = 0.0;
            }
            return;
        }
        while let Some(i) = queue.pop_front() {
            let here = self.heights[i];
            let (x, z) = ((i % nx) as i32, (i / nx) as i32);
            for (dx, dz) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                let (x2, z2) = (x + dx, z + dz);
                if x2 < 0 || z2 < 0 || x2 as usize >= nx || z2 as usize >= nz {
                    continue;
                }
                let j = z2 as usize * nx + x2 as usize;
                if self.heights[j].is_finite() {
                    continue;
                }
                self.heights[j] = here;
                queue.push_back(j);
            }
        }
    }

    pub fn cell_size(&self) -> f32 {
        self.cell
    }

    pub fn dims(&self) -> (usize, usize) {
        (self.nx, self.nz)
    }

    /// The window the raster covers: (min_x, min_z, max_x, max_z).
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        (
            self.min_x,
            self.min_z,
            self.min_x + self.nx as f32 * self.cell,
            self.min_z + self.nz as f32 * self.cell,
        )
    }

    pub fn cell_centre(&self, ix: usize, iz: usize) -> (f32, f32) {
        (
            self.min_x + (ix as f32 + 0.5) * self.cell,
            self.min_z + (iz as f32 + 0.5) * self.cell,
        )
    }

    /// The column under (x, z); `None` outside the raster.
    pub fn index_at(&self, x: f32, z: f32) -> Option<usize> {
        let fx = (x - self.min_x) / self.cell;
        let fz = (z - self.min_z) / self.cell;
        if !fx.is_finite() || !fz.is_finite() || fx < 0.0 || fz < 0.0 {
            return None;
        }
        let (ix, iz) = (fx as usize, fz as usize);
        if ix >= self.nx || iz >= self.nz {
            return None;
        }
        Some(iz * self.nx + ix)
    }

    pub fn kind_of(&self, i: usize) -> FloorKind {
        self.kinds.get(i).copied().unwrap_or(FloorKind::Solid)
    }

    pub fn height_of(&self, i: usize) -> f32 {
        let h = self.heights.get(i).copied().unwrap_or(0.0);
        if h.is_finite() {
            h
        } else {
            0.0
        }
    }

    pub fn kind_at(&self, x: f32, z: f32) -> Option<FloorKind> {
        self.index_at(x, z).map(|i| self.kind_of(i))
    }

    /// The nearest floor's height under (x, z), walls included; `None`
    /// outside the raster.
    pub fn height_near(&self, x: f32, z: f32) -> Option<f32> {
        self.index_at(x, z).map(|i| self.height_of(i))
    }

    /// The surface answer for (x, z): ground on a floor, a hole over a pit,
    /// `Outside` on a wall or a stacked column. `None` where the raster
    /// does not reach — the caller's own composition answers there.
    pub fn sample(&self, x: f32, z: f32) -> Option<SurfaceSample> {
        let i = self.index_at(x, z)?;
        Some(match self.kinds[i] {
            FloorKind::Floor => SurfaceSample::Surface(self.height_of(i)),
            FloorKind::Pit => SurfaceSample::Hole,
            FloorKind::Solid | FloorKind::Stacked => SurfaceSample::Outside,
        })
    }

    pub fn count(&self, kind: FloorKind) -> usize {
        self.kinds.iter().filter(|k| **k == kind).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hall() -> FloorRaster {
        // A 10 x 6 m hall at y = 2 with a wall column at ix 4..6, iz 2 and a
        // pit at (8, 1); everything past the raster is not covered.
        let mut r = FloorRaster::new(0.0, 0.0, 1.0, 10, 6);
        for iz in 0..6 {
            for ix in 0..10 {
                r.set(ix, iz, FloorKind::Floor, 2.0);
            }
        }
        r.set(4, 2, FloorKind::Solid, f32::NAN);
        r.set(5, 2, FloorKind::Solid, f32::NAN);
        r.set(8, 1, FloorKind::Pit, 1.0);
        r.set(0, 5, FloorKind::Stacked, 2.0);
        r.finish();
        r
    }

    #[test]
    fn floors_are_ground_pits_are_holes_walls_and_stacks_are_outside() {
        let r = hall();
        assert_eq!(r.sample(1.5, 1.5), Some(SurfaceSample::Surface(2.0)));
        assert_eq!(r.sample(8.5, 1.5), Some(SurfaceSample::Hole));
        assert_eq!(r.sample(4.5, 2.5), Some(SurfaceSample::Outside));
        assert_eq!(r.sample(0.5, 5.5), Some(SurfaceSample::Outside));
        assert_eq!(r.sample(-0.5, 1.0), None, "west of the raster is not covered");
        assert_eq!(r.sample(10.5, 1.0), None, "east of the raster is not covered");
    }

    #[test]
    fn a_wall_carries_its_neighbouring_floors_height() {
        let r = hall();
        assert_eq!(r.height_near(4.5, 2.5), Some(2.0));
        assert_eq!(r.kind_at(4.5, 2.5), Some(FloorKind::Solid));
        assert_eq!(r.count(FloorKind::Solid), 2);
        assert_eq!(r.count(FloorKind::Pit), 1);
        assert_eq!(r.count(FloorKind::Stacked), 1);
        assert_eq!(r.count(FloorKind::Floor), 56);
    }

    #[test]
    fn an_empty_raster_reads_zero_not_nan() {
        let mut r = FloorRaster::new(0.0, 0.0, 1.0, 3, 3);
        r.finish();
        assert_eq!(r.height_of(4), 0.0);
        assert_eq!(r.sample(1.5, 1.5), Some(SurfaceSample::Outside));
    }
}
