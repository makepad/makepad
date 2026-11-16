use crate::{bvh::Bvh, TriMesh};
use makepad_csg_math::{thread_pool, Vec3d};
use std::sync::Arc;

/// Give each triangle of `to` the tag nearest its centroid on `from`.
/// An untagged source leaves the target untagged.
pub fn transfer_tags(from: &TriMesh, to: &mut TriMesh) {
    let tagged = from.is_tagged();
    if !tagged || to.triangles.is_empty() {
        to.tags.clear();
        return;
    }
    let tag = from.tags[0];
    if from.tags.iter().all(|&t| t == tag) {
        to.tags = vec![tag; to.triangle_count()];
        return;
    }
    let bvh = Bvh::build(from);
    let project = |triangles: &[[u32; 3]], vertices: &[Vec3d], bvh: &Bvh, tags: &[u32]| -> Vec<u32> {
        triangles.iter().map(|&[a, b, c]| {
            let (a, b, c) = (vertices[a as usize], vertices[b as usize], vertices[c as usize]);
            let p = a + ((b - a) + (c - a)) / 3.0;
            // Stable BVH construction and traversal keep equal-distance ties deterministic.
            bvh.closest(p).map_or(0, |(_, _, ti)| tags[ti as usize])
        }).collect()
    };
    to.tags = if to.triangle_count() < 1024 {
        project(&to.triangles, &to.vertices, &bvh, &from.tags)
    } else {
        let bvh = Arc::new(bvh);
        let vertices: Arc<[Vec3d]> = to.vertices.clone().into();
        let tags: Arc<[u32]> = from.tags.clone().into();
        thread_pool::parallel_map(&to.triangles, move |triangles| project(triangles, &vertices, &bvh, &tags))
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_csg_math::dvec3;

    #[test]
    fn transfer_preserves_face_tags_geometry_and_pool_order() {
        let mut source = crate::make_unit_cube();
        source.tags = (0..source.triangle_count()).map(|ti| 1 + ti as u32 / 2).collect();
        let mut fine = source.clone();
        for _ in 0..4 {
            let mut next = TriMesh::new();
            for ti in 0..fine.triangle_count() {
                let (a, b, c) = fine.triangle_vertices(ti);
                let (ab, bc, ca) = ((a + b) * 0.5, (b + c) * 0.5, (c + a) * 0.5);
                for points in [[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]] {
                    let [a, b, c] = points.map(|p| next.add_vertex(p));
                    next.add_tagged_triangle(a, b, c, fine.tag(ti));
                }
            }
            fine = next;
        }
        let expected = fine.tags.clone();
        fine.corner_normals = (0..fine.triangle_count()).flat_map(|ti| [fine.triangle_normal(ti); 3]).collect();
        let geometry = (fine.vertices.clone(), fine.triangles.clone(), fine.corner_normals.clone());
        fine.tags.clear();
        transfer_tags(&source, &mut fine);
        assert_eq!(fine.tags, expected, "fine box: every triangle keeps its face tag");
        assert_eq!((fine.vertices.clone(), fine.triangles.clone(), fine.corner_normals.clone()), geometry, "fine box: positions, indices and exact normals are unchanged");

        let mut tie = TriMesh::new();
        for p in [dvec3(-0.125, -0.125, 0.0), dvec3(0.125, -0.125, 0.0), dvec3(0.0, 0.25, 0.0)] {
            tie.add_vertex(p);
        }
        tie.add_triangle(0, 1, 2);
        transfer_tags(&source, &mut tie);
        let selected = tie.tag(0);
        let mut pooled = tie.clone();
        pooled.tags.clear();
        pooled.triangles.resize(1025, [0, 1, 2]);
        transfer_tags(&source, &mut pooled);
        assert!(pooled.tags.iter().all(|&tag| tag == selected), "equidistant faces: small and pooled passes choose the same tag");
        pooled.tags.clear();
        thread_pool::with_serial(|| transfer_tags(&source, &mut pooled));
        assert!(pooled.tags.iter().all(|&tag| tag == selected), "equidistant faces: serial scheduling preserves the pooled choice");

        source.tags.fill(u32::MAX);
        transfer_tags(&source, &mut fine);
        assert!(fine.tags.iter().all(|&tag| tag == u32::MAX), "uniform source: the full u32 tag is filled");
        source.tags.clear();
        transfer_tags(&source, &mut fine);
        assert!(!fine.is_tagged(), "untagged source: existing target tags are cleared");
        let mut plain = crate::make_unit_cube();
        transfer_tags(&source, &mut plain);
        assert_eq!(plain.tags.capacity(), 0, "untagged source: no tag storage is allocated");
        transfer_tags(&TriMesh::new(), &mut tie);
        assert!(!tie.is_tagged(), "empty source: target becomes untagged");
        source.tags = vec![7; source.triangle_count()];
        let mut empty = TriMesh::new();
        transfer_tags(&source, &mut empty);
        assert!(!empty.is_tagged(), "empty target: the channel stays empty");
    }
}
