use crate::math::Vector;
use crate::shape::RoundConvexPolyhedron;
use crate::transformation::utils;
use alloc::{vec, vec::Vec};

impl RoundConvexPolyhedron {
    /// Outlines this round convex polyhedron’s shape using polylines.
    pub fn to_outline(&self, nsubdivs: u32) -> (Vec<Vector>, Vec<[u32; 2]>) {
        let mut out_vtx = vec![];
        let mut out_idx = vec![];
        let poly = &self.inner_shape;

        // 1. Compute the offset vertices.
        for (vid, ref_vtx) in poly.vertices().iter().enumerate() {
            let ref_pt = poly.points()[vid];
            let range = ref_vtx.first_adj_face_or_edge as usize
                ..(ref_vtx.first_adj_face_or_edge + ref_vtx.num_adj_faces_or_edge) as usize;
            let adj_faces = &poly.faces_adj_to_vertex()[range];

            for fid in adj_faces {
                let face = poly.faces()[*fid as usize];
                out_vtx.push(ref_pt + face.normal * self.border_radius);
            }
        }

        // 2. Compute the straight edges.
        // TODO: right now, each vertex of the final polyline will be duplicated
        //       here, to simplify the index buffer generation for the straight edges.
        for face in poly.faces() {
            let i1 = face.first_vertex_or_edge;
            let i2 = i1 + face.num_vertices_or_edges;
            let base = out_vtx.len() as u32;

            for idx in &poly.vertices_adj_to_face()[i1 as usize..i2 as usize] {
                out_vtx.push(poly.points()[*idx as usize] + face.normal * self.border_radius);
            }

            for i in 0..face.num_vertices_or_edges - 1 {
                out_idx.push([base + i, base + i + 1]);
            }

            out_idx.push([base, base + face.num_vertices_or_edges - 1]);
        }

        // 3. Compute the arcs.
        let mut arc_base = 0;
        for (vid, ref_vtx) in poly.vertices().iter().enumerate() {
            let ref_pt = poly.points()[vid];
            let n = ref_vtx.num_adj_faces_or_edge;

            if n > 0 {
                for k1 in 0..n {
                    for k2 in k1 + 1..n {
                        utils::push_arc_and_idx(
                            ref_pt,
                            arc_base + k1,
                            arc_base + k2,
                            nsubdivs,
                            &mut out_vtx,
                            &mut out_idx,
                        );
                    }
                }

                arc_base += n;
            }
        }

        (out_vtx, out_idx)
    }
}
