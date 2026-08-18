//! CubeGrid / OctreeIndexer integer index math (Atom3d grid/, exact port).
//!
//! Conventions (verified against atom3d/grid/cube_grid.py + octree_indexer.py):
//! - linear cube index = i*r^2 + j*r + k (X-major)
//! - bounds fixed [-1, 1]^3; cell size at level L = 2/2^L computed in f32
//! - cube AABB min = ijk_f32 * cell + (-1); max = min + cell (f32 ops)
//! - vertex world = vertex_ijk_f32 * cell + (-1)
//! - CUBE_CORNERS order (also the subdivide child order): index c has
//!   coordinate (c&1, (c>>1)&1, (c>>2)&1)
//! - 12 edges: rows 0-3 X-aligned, 4-7 Y-aligned, 8-11 Z-aligned; global edge
//!   ids partitioned X | Y | Z with dims (r,p,p) / (p,r,p) / (p,p,r), p = r+1

use crate::math::V3;

pub const CUBE_CORNERS: [[i64; 3]; 8] = [
    [0, 0, 0], [1, 0, 0], [0, 1, 0], [1, 1, 0],
    [0, 0, 1], [1, 0, 1], [0, 1, 1], [1, 1, 1],
];

/// Per edge row: (axis, anchor offset within the cube).
/// anchor = min corner of the edge; axis = direction of the edge.
pub const EDGE_ANCHOR: [(usize, [i64; 3]); 12] = [
    // X-aligned (CUBE_EDGES rows [0,1],[2,3],[4,5],[6,7])
    (0, [0, 0, 0]), (0, [0, 1, 0]), (0, [0, 0, 1]), (0, [0, 1, 1]),
    // Y-aligned ([0,2],[1,3],[4,6],[5,7])
    (1, [0, 0, 0]), (1, [1, 0, 0]), (1, [0, 0, 1]), (1, [1, 0, 1]),
    // Z-aligned ([0,4],[1,5],[2,6],[3,7])
    (2, [0, 0, 0]), (2, [1, 0, 0]), (2, [0, 1, 0]), (2, [1, 1, 0]),
];

#[derive(Clone, Copy)]
pub struct Grid {
    pub res: i64,
    pub max_level: u32,
    pub cell: f32,
}

impl Grid {
    pub fn new(res: u32) -> Grid {
        assert!(res.is_power_of_two() && res >= 2, "resolution must be a power of two");
        Grid {
            res: res as i64,
            max_level: res.trailing_zeros(),
            cell: 2.0f32 / res as f32,
        }
    }

    #[inline]
    pub fn cell_at_level(level: u32) -> f32 {
        2.0f32 / (1i64 << level) as f32
    }

    #[inline]
    pub fn linear(&self, ijk: [i64; 3]) -> i64 {
        (ijk[0] * self.res + ijk[1]) * self.res + ijk[2]
    }

    #[inline]
    pub fn ijk_of(&self, lin: i64) -> [i64; 3] {
        let r = self.res;
        [lin / (r * r), (lin / r) % r, lin % r]
    }

    /// cube AABB at a level: min = ijk_f32 * cell_l + (-1), max = min + cell_l
    #[inline]
    pub fn cube_aabb_level(ijk: [i64; 3], level: u32) -> (V3, V3) {
        let cell = Self::cell_at_level(level);
        let mn = [
            ijk[0] as f32 * cell + -1.0f32,
            ijk[1] as f32 * cell + -1.0f32,
            ijk[2] as f32 * cell + -1.0f32,
        ];
        let mx = [mn[0] + cell, mn[1] + cell, mn[2] + cell];
        (mn, mx)
    }

    #[inline]
    pub fn cube_aabb(&self, ijk: [i64; 3]) -> (V3, V3) {
        Self::cube_aabb_level(ijk, self.max_level)
    }

    /// grid vertex world position = vertex_ijk_f32 * cell + (-1)
    #[inline]
    pub fn vertex_world(&self, ijk: [i64; 3]) -> V3 {
        [
            ijk[0] as f32 * self.cell + -1.0f32,
            ijk[1] as f32 * self.cell + -1.0f32,
            ijk[2] as f32 * self.cell + -1.0f32,
        ]
    }

    #[inline]
    fn edge_counts(&self) -> (i64, i64, i64) {
        let r = self.res;
        let p = r + 1;
        (r * p * p, p * r * p, p * p * r)
    }

    /// The 12 global edge ids of a cube, in CUBE_EDGES row order.
    pub fn cube_edge_ids(&self, ijk: [i64; 3]) -> [i64; 12] {
        let r = self.res;
        let p = r + 1;
        let (nx, ny, _) = self.edge_counts();
        let mut out = [0i64; 12];
        for (row, &(axis, anchor)) in EDGE_ANCHOR.iter().enumerate() {
            let a = [ijk[0] + anchor[0], ijk[1] + anchor[1], ijk[2] + anchor[2]];
            out[row] = match axis {
                0 => a[0] * (p * p) + a[1] * p + a[2],
                1 => a[0] * (r * p) + a[1] * p + a[2] + nx,
                _ => a[0] * (p * r) + a[1] * r + a[2] + nx + ny,
            };
        }
        out
    }

    /// Decompose a global edge id into (axis, anchor vertex ijk).
    pub fn edge_decompose(&self, edge_id: i64) -> (usize, [i64; 3]) {
        let r = self.res;
        let p = r + 1;
        let (nx, ny, _) = self.edge_counts();
        if edge_id < nx {
            // dims (r, p, p)
            let l = edge_id;
            (0, [l / (p * p), (l / p) % p, l % p])
        } else if edge_id < nx + ny {
            // dims (p, r, p)
            let l = edge_id - nx;
            (1, [l / (r * p), (l / p) % r, l % p])
        } else {
            // dims (p, p, r)
            let l = edge_id - nx - ny;
            (2, [l / (p * r), (l / r) % p, l % r])
        }
    }

    /// Edge endpoints in world coords (v0 = anchor, v1 = anchor + axis).
    pub fn edge_endpoints(&self, edge_id: i64) -> (V3, V3) {
        let (axis, a) = self.edge_decompose(edge_id);
        let mut b = a;
        b[axis] += 1;
        (self.vertex_world(a), self.vertex_world(b))
    }

    /// Up to 4 incident cubes (linear ids, -1 when out of bounds), in the
    /// reference's CCW-around-+axis order.
    pub fn edge_incident_cubes(&self, edge_id: i64) -> [i64; 4] {
        let r = self.res;
        let (axis, a) = self.edge_decompose(edge_id);
        let mut out = [-1i64; 4];
        let cubes: [[i64; 3]; 4] = match axis {
            0 => {
                // (ix, jv, kv): (cy, cz) = (jv - dy, kv - dz), dy=[0,0,1,1], dz=[0,1,1,0]
                let (ix, jv, kv) = (a[0], a[1], a[2]);
                [
                    [ix, jv, kv],
                    [ix, jv, kv - 1],
                    [ix, jv - 1, kv - 1],
                    [ix, jv - 1, kv],
                ]
            }
            1 => {
                // (iv, jy, kv): (cx, cz) = (iv + dx, kv + dz), dx=[-1,-1,0,0], dz=[0,-1,-1,0]
                let (iv, jy, kv) = (a[0], a[1], a[2]);
                [
                    [iv - 1, jy, kv],
                    [iv - 1, jy, kv - 1],
                    [iv, jy, kv - 1],
                    [iv, jy, kv],
                ]
            }
            _ => {
                // (iv, jv, kz): (cx, cy) = (iv + dx, jv + dy), dx=[-1,0,0,-1], dy=[0,0,-1,-1]
                let (iv, jv, kz) = (a[0], a[1], a[2]);
                [
                    [iv - 1, jv, kz],
                    [iv, jv, kz],
                    [iv, jv - 1, kz],
                    [iv - 1, jv - 1, kz],
                ]
            }
        };
        for (o, c) in out.iter_mut().zip(cubes.iter()) {
            if c[0] >= 0 && c[0] < r && c[1] >= 0 && c[1] < r && c[2] >= 0 && c[2] < r {
                *o = self.linear(*c);
            }
        }
        out
    }
}
