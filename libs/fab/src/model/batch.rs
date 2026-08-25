//! GPU-ready CPU buffers. One [`RenderBatch`] per material (opaque and
//! transparent batches are separate) holding every triangle of every element
//! that uses that material, already in world space, Z up, meters.
//!
//! `vertices` is a **flat `Vec<f32>`** with stride [`VERTEX_STRIDE`] laid out
//! exactly like [`Vertex`], so the renderer (lane B) hands it to
//! `Geometry::update(cx, indices, vertices)` once per `Scene::generation`
//! without conversion. Per-element visibility/selection/explode is resolved
//! on the GPU through a per-element lookup indexed by [`Vertex::element`], so
//! hiding or selecting never re-uploads geometry.

use crate::model::ids::{ElementId, MaterialId};
use makepad_math::{Aabb, Vec3f};

/// Floats per vertex: position(3) element(1) normal(3) pad(1) uv(2) pad(2).
/// 48 bytes — std140-shaped so the shader POD (`fab_geom.FabVertex`) and
/// the Rust layout agree (vec3 aligns to 16, struct size to 16).
pub const VERTEX_STRIDE: usize = 12;

/// One vertex as the shader sees it (`fab_geom.FabVertex` on the DSL side).
/// `#[repr(C)]`, 48 bytes, all `f32`, field order == GPU order.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    /// `ElementId.0` as a float (exact below 2^24 elements). The renderer
    /// reads per-element flags/colors from a lookup texture at this index.
    pub element: f32,
    pub normal: [f32; 3],
    /// Padding (vec3 → 16 bytes).
    pub _pad: f32,
    pub uv: [f32; 2],
    /// Padding (struct size → 48 bytes).
    pub _pad2: [f32; 2],
}

impl Vertex {
    pub fn element_id(&self) -> ElementId {
        ElementId(self.element as u32)
    }

    pub fn position_v3(&self) -> Vec3f {
        Vec3f {
            x: self.position[0],
            y: self.position[1],
            z: self.position[2],
        }
    }
}

/// Which element owns which contiguous index range in a batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ElementRange {
    pub element: ElementId,
    /// Stable scene-global render-part id. A part is one mesh placement under
    /// one material; unlike `element`, it distinguishes source material parts.
    pub part: u32,
    pub first_index: u32,
    pub index_count: u32,
    /// Render-only coplanar conflict metadata. Canonical positions and indices
    /// are never changed to resolve an overlap.
    pub draw_priority: u16,
    pub coplanar_group: u32,
}

#[derive(Clone, Debug)]
pub struct RenderBatch {
    pub material: MaterialId,
    pub transparent: bool,
    /// Flat vertex stream, stride [`VERTEX_STRIDE`].
    pub vertices: Vec<f32>,
    /// Triangle list.
    pub indices: Vec<u32>,
    /// Sorted by `first_index`. Every index belongs to exactly one range.
    pub element_ranges: Vec<ElementRange>,
    pub bounds: Aabb,
}

impl Default for RenderBatch {
    fn default() -> Self {
        RenderBatch {
            material: MaterialId::NONE,
            transparent: false,
            vertices: Vec::new(),
            indices: Vec::new(),
            element_ranges: Vec::new(),
            bounds: crate::model::bounds::aabb_empty(),
        }
    }
}

impl RenderBatch {
    pub fn vertex_count(&self) -> usize {
        self.vertices.len() / VERTEX_STRIDE
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn push_vertex(&mut self, v: &Vertex) {
        self.vertices.extend_from_slice(&[
            v.position[0],
            v.position[1],
            v.position[2],
            v.element,
            v.normal[0],
            v.normal[1],
            v.normal[2],
            v._pad,
            v.uv[0],
            v.uv[1],
            v._pad2[0],
            v._pad2[1],
        ]);
    }

    #[inline]
    pub fn position(&self, vertex: u32) -> Vec3f {
        let o = vertex as usize * VERTEX_STRIDE;
        Vec3f {
            x: self.vertices[o],
            y: self.vertices[o + 1],
            z: self.vertices[o + 2],
        }
    }

    #[inline]
    pub fn normal(&self, vertex: u32) -> Vec3f {
        let o = vertex as usize * VERTEX_STRIDE + 4;
        Vec3f {
            x: self.vertices[o],
            y: self.vertices[o + 1],
            z: self.vertices[o + 2],
        }
    }

    #[inline]
    pub fn uv(&self, vertex: u32) -> [f32; 2] {
        let o = vertex as usize * VERTEX_STRIDE + 8;
        [self.vertices[o], self.vertices[o + 1]]
    }

    #[inline]
    pub fn element(&self, vertex: u32) -> ElementId {
        ElementId(self.vertices[vertex as usize * VERTEX_STRIDE + 3] as u32)
    }

    pub fn vertex(&self, vertex: u32) -> Vertex {
        let o = vertex as usize * VERTEX_STRIDE;
        let v = &self.vertices[o..o + VERTEX_STRIDE];
        Vertex {
            position: [v[0], v[1], v[2]],
            element: v[3],
            normal: [v[4], v[5], v[6]],
            _pad: v[7],
            uv: [v[8], v[9]],
            _pad2: [v[10], v[11]],
        }
    }

    /// The three corner positions of triangle `tri`.
    pub fn triangle(&self, tri: u32) -> (Vec3f, Vec3f, Vec3f) {
        let i = tri as usize * 3;
        (
            self.position(self.indices[i]),
            self.position(self.indices[i + 1]),
            self.position(self.indices[i + 2]),
        )
    }

    /// Element owning triangle `tri` (index-triplet number), by binary search.
    pub fn element_of_triangle(&self, tri: u32) -> Option<ElementId> {
        self.range_of_triangle(tri).map(|r| r.element)
    }

    /// Render part owning triangle `tri`.
    pub fn part_of_triangle(&self, tri: u32) -> Option<u32> {
        self.range_of_triangle(tri).map(|r| r.part)
    }

    /// Draw priority of triangle `tri`; zero means it has no coplanar conflict.
    pub fn draw_priority_of_triangle(&self, tri: u32) -> u16 {
        self.range_of_triangle(tri)
            .map(|r| r.draw_priority)
            .unwrap_or(0)
    }

    pub fn coplanar_group_of_triangle(&self, tri: u32) -> u32 {
        self.range_of_triangle(tri)
            .map(|r| r.coplanar_group)
            .unwrap_or(0)
    }

    fn range_of_triangle(&self, tri: u32) -> Option<ElementRange> {
        let first_index = tri * 3;
        let pos = self
            .element_ranges
            .partition_point(|r| r.first_index <= first_index);
        if pos == 0 {
            return None;
        }
        let r = self.element_ranges[pos - 1];
        if first_index < r.first_index + r.index_count {
            Some(r)
        } else {
            None
        }
    }
}
