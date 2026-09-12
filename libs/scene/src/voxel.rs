//! Published voxel meshes; density and meshing remain with the producer.
use makepad_math::*;
use std::{collections::BTreeMap, sync::Arc};

/// Sites per chunk axis, used only for chunk coordinates.
const CHUNK: i32 = 32;

/// Chunk coordinate: `floor(site / CHUNK)` per axis. Ord = (x, y, z)
/// lexicographic — the deterministic iteration order everywhere.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ChunkKey {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl ChunkKey {
    pub fn of_site(s: [i32; 3]) -> ChunkKey {
        ChunkKey {
            x: s[0].div_euclid(CHUNK),
            y: s[1].div_euclid(CHUNK),
            z: s[2].div_euclid(CHUNK),
        }
    }
    /// Lowest site this chunk owns.
    pub fn base(&self) -> [i32; 3] {
        [self.x * CHUNK, self.y * CHUNK, self.z * CHUNK]
    }
}

/// One remeshed chunk, in the renderer's 16-float PbrVertex layout
/// (`pos3 | normal3 | uv2 | color4 | tangent4` — the exact layout the
/// heightfield tiles use, so the voxel mesh rides the same terrain shader).
/// The collider reads positions back out of the same vertices — collision
/// IS the visual, by construction.
#[derive(Clone, Debug, Default)]
pub struct ChunkMesh {
    /// Bumped on every rebuild: renderer re-uploads, dynamics hot-swaps.
    pub rev: u64,
    pub verts: Vec<f32>,
    pub indices: Vec<u32>,
    pub min: Vec3f,
    pub max: Vec3f,
}

pub const MESH_VERTEX_FLOATS: usize = 16;

#[derive(Clone, Debug, Default)]
pub struct VoxelView {
    pub meshes: BTreeMap<ChunkKey, Arc<ChunkMesh>>,
}
