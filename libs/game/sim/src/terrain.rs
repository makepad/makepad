//! Smooth heightfield terrain — moved verbatim from gamemaker's game_view.rs
//! (M0 stage A extraction). Float expression order preserved for tape parity.

use makepad_math::*;

/// Smooth heightfield: vertex heights on an N×N grid (row-major z*cells+x),
/// rendered as one triangulated mesh (flat per-tri normals, Godot-style) and
/// collided by height lookup instead of per-column AABBs.
#[derive(Clone)]
pub struct Terrain {
    /// Vertices per side (the API's `cells` value).
    pub cells: usize,
    pub cell_size: f32,
    /// World x/z of vertex (0,0); the grid is square and centered.
    pub origin: f32,
    pub heights: Vec<f32>,
    pub colors: Vec<Vec4f>,
    /// Bumped on every rebuild so the GPU mesh regenerates lazily.
    pub revision: u64,
}

/// Reported as the hit id when a sweep stops against the terrain.
pub const TERRAIN_ID: u64 = u64::MAX;

impl Terrain {
    /// Piecewise-planar ground height at (x, z): the two triangles per cell,
    /// same split the mesh uses, so collision and pixels agree. None outside.
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        let fx = (x - self.origin) / self.cell_size;
        let fz = (z - self.origin) / self.cell_size;
        if fx < 0.0 || fz < 0.0 {
            return None;
        }
        let max = (self.cells - 1) as f32;
        if fx >= max || fz >= max {
            return None;
        }
        let ix = fx.floor() as usize;
        let iz = fz.floor() as usize;
        let u = fx - ix as f32;
        let v = fz - iz as f32;
        let h = |gx: usize, gz: usize| self.heights[gz * self.cells + gx];
        let (h00, h10, h01, h11) = (h(ix, iz), h(ix + 1, iz), h(ix, iz + 1), h(ix + 1, iz + 1));
        Some(if u + v < 1.0 {
            h00 + (h10 - h00) * u + (h01 - h00) * v
        } else {
            h11 + (h01 - h11) * (1.0 - u) + (h10 - h11) * (1.0 - v)
        })
    }

    /// Surface normal of the triangle under (x, z) — the SAME triangle
    /// height_at picks, so cars can align to the slope they collide with.
    pub fn normal_at(&self, x: f32, z: f32) -> Option<Vec3f> {
        let fx = (x - self.origin) / self.cell_size;
        let fz = (z - self.origin) / self.cell_size;
        if fx < 0.0 || fz < 0.0 {
            return None;
        }
        let max = (self.cells - 1) as f32;
        if fx >= max || fz >= max {
            return None;
        }
        let ix = fx.floor() as usize;
        let iz = fz.floor() as usize;
        let u = fx - ix as f32;
        let v = fz - iz as f32;
        let h = |gx: usize, gz: usize| self.heights[gz * self.cells + gx];
        let s = self.cell_size;
        let x0 = self.origin + ix as f32 * s;
        let z0 = self.origin + iz as f32 * s;
        // Same diagonal split as height_at / the mesh.
        let (a, b, c) = if u + v < 1.0 {
            (
                vec3f(x0, h(ix, iz), z0),
                vec3f(x0 + s, h(ix + 1, iz), z0),
                vec3f(x0, h(ix, iz + 1), z0 + s),
            )
        } else {
            (
                vec3f(x0 + s, h(ix + 1, iz + 1), z0 + s),
                vec3f(x0, h(ix, iz + 1), z0 + s),
                vec3f(x0 + s, h(ix + 1, iz), z0),
            )
        };
        let normal = Vec3f::cross(b - a, c - a);
        let normal = if normal.y < 0.0 { normal * -1.0 } else { normal };
        let len = normal.length();
        if len <= 1.0e-6 {
            return Some(vec3f(0.0, 1.0, 0.0));
        }
        Some(normal * (1.0 / len))
    }

    /// Max ground height under an AABB footprint (corners + center) — what a
    /// box standing here rests on.
    pub fn floor_under(&self, pos: Vec3f, half: Vec3f) -> Option<f32> {
        let probes = [
            (pos.x, pos.z),
            (pos.x - half.x, pos.z - half.z),
            (pos.x + half.x, pos.z - half.z),
            (pos.x - half.x, pos.z + half.z),
            (pos.x + half.x, pos.z + half.z),
        ];
        let mut best: Option<f32> = None;
        for (x, z) in probes {
            if let Some(h) = self.height_at(x, z) {
                best = Some(best.map_or(h, |b: f32| b.max(h)));
            }
        }
        best
    }
}
