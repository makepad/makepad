use crate::math::Real;
use crate::shape::ConvexPolyhedron;
use crate::math::Vector3;

impl ConvexPolyhedron {
    /// Outlines this convex polyhedron’s shape using polylines.
    pub fn to_outline(&self) -> (Vec<Vector>, Vec<[u32; 3]>) {
        let mut indices = Vec::new();

        for face in self.faces() {
            let i1 = face.first_vertex_or_edge;
            let i2 = i1 + face.num_vertices_or_edges;
            let first_id = self.vertices_adj_to_face()[i1 as usize] as u32;

            for idx in self.vertices_adj_to_face()[i1 as usize + 1..i2 as usize].windows(2) {
                indices.push([first_id, idx[0] as u32, idx[1] as u32]);
            }
        }

        (self.points().to_vec(), indices)
    }
}
