use crate::math::Vector3;
use crate::shape::HeightField;
use alloc::vec::Vec;

impl HeightField {
    /// Discretize the boundary of this heightfield as a triangle-mesh.
    pub fn to_trimesh(&self) -> (Vec<Vector3>, Vec<[u32; 3]>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        for (i, tri) in self.triangles().enumerate() {
            vertices.push(tri.a);
            vertices.push(tri.b);
            vertices.push(tri.c);

            let i = i as u32;
            indices.push([i * 3, i * 3 + 1, i * 3 + 2])
        }

        (vertices, indices)
    }
}
