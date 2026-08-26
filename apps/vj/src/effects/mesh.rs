//! The shared vertex stream every effect engine writes into.
//!
//! One layout for every engine, chosen to be byte-compatible with the
//! already-registered `geom.CubeVertex` shader vertex (12 floats), so no new
//! pod type has to be registered and every fx shader declares
//! `geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)` exactly like the
//! firework/flare shaders do:
//!
//! | floats | CubeVertex name     | fx meaning                                  |
//! |--------|---------------------|---------------------------------------------|
//! | 0..3   | `geom_pos`          | position (mesh) / billboard corner (sprites)|
//! | 3      | `geom_id`           | `a_id`   — particle id, branch depth, …     |
//! | 4..7   | `geom_normal`       | normal / radial dir / ribbon tangent        |
//! | 7      | `geom_pad`          | `a_aux`  — birth order, spawn phase, …      |
//! | 8..10  | `geom_uv`           | uv                                          |
//! | 10     | `geom_tail_pad_0`   | `a_r0`   — per-element random seed          |
//! | 11     | `geom_tail_pad_1`   | `a_r1`   — radius / side / second seed      |
//!
//! This is the heart of the architecture: engines ENCODE what they know onto
//! the vertex stream once (at build/regen time), and the vertex shader
//! animates off those attributes every frame from time/beat uniforms. Mesh
//! regeneration is the slow path; attribute-driven shader animation is the
//! fast path.
//!
//! # Buffer-size stability
//!
//! The Metal backend re-allocates a GPU buffer whenever the byte length of a
//! geometry changes, and memcpy's in place when it does not. Engines that
//! regenerate per frame (metaballs, ribbons) therefore pad their buffers to a
//! high-water capacity with degenerate triangles: the byte length stays
//! constant and every upload takes the cheap path.

use makepad_widgets::*;

/// Floats per vertex — the CubeVertex layout.
pub const VERT_FLOATS: usize = 12;

/// A growable CPU-side mesh with recycled allocations.
#[derive(Default)]
pub struct FxMesh {
    pub verts: Vec<f32>,
    pub idx: Vec<u32>,
    /// High-water marks (in floats / indices) used to pad regenerating
    /// engines to a stable buffer size.
    vert_cap: usize,
    idx_cap: usize,
}

impl FxMesh {
    pub fn clear(&mut self) {
        self.verts.clear();
        self.idx.clear();
    }

    pub fn vertex_count(&self) -> usize {
        self.verts.len() / VERT_FLOATS
    }

    pub fn triangle_count(&self) -> usize {
        self.idx.len() / 3
    }

    /// Push one vertex, returning its index.
    #[inline]
    pub fn push_vert(
        &mut self,
        pos: Vec3f,
        a_id: f32,
        normal: Vec3f,
        a_aux: f32,
        uv: Vec2f,
        a_r0: f32,
        a_r1: f32,
    ) -> u32 {
        let index = (self.verts.len() / VERT_FLOATS) as u32;
        self.verts.extend_from_slice(&[
            pos.x, pos.y, pos.z, a_id, normal.x, normal.y, normal.z, a_aux, uv.x, uv.y, a_r0,
            a_r1,
        ]);
        index
    }

    #[inline]
    pub fn push_tri(&mut self, a: u32, b: u32, c: u32) {
        self.idx.extend_from_slice(&[a, b, c]);
    }

    #[inline]
    pub fn push_quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.idx.extend_from_slice(&[a, b, c, a, c, d]);
    }

    /// Pad to the running high-water capacity with degenerate data so the GPU
    /// buffer byte length never shrinks (see module docs). Call after filling
    /// a per-frame regenerated mesh.
    pub fn pad_to_high_water(&mut self) {
        if self.verts.len() > self.vert_cap {
            self.vert_cap = self.verts.len();
        }
        if self.idx.len() > self.idx_cap {
            self.idx_cap = self.idx.len();
        }
        self.verts.resize(self.vert_cap, 0.0);
        // Degenerate triangles (0,0,0) draw nothing.
        self.idx.resize(self.idx_cap, 0);
    }

    /// Upload into a [`Geometry`], recycling this mesh's buffers so the next
    /// frame reuses their capacity (no per-frame allocation storm).
    pub fn upload_recycle(&mut self, cx: &mut Cx, geometry: &Geometry) {
        geometry.update_with_recycled_buffers(cx, &mut self.idx, &mut self.verts);
    }

    /// Upload a static (built-once) mesh by cloning the data; the CPU copy
    /// stays around so a rebuild can diff/replace it.
    pub fn upload_clone(&self, cx: &mut Cx, geometry: &Geometry) {
        geometry.update(cx, self.idx.clone(), self.verts.clone());
    }
}

/// The xorshift generator every engine uses — deterministic per seed, no
/// std rand dependency.
#[derive(Clone)]
pub struct FxRng(pub u64);

impl FxRng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let v = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        ((v >> 11) as f32) / ((1u64 << 53) as f32)
    }
    #[inline]
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }
}
