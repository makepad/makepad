// Conversion between TriMesh and polygon soup (BSP representation).

use crate::polygon::Polygon;
use makepad_csg_math::Vec3d;
use makepad_csg_mesh::mesh::TriMesh;

/// Convert a TriMesh into a polygon soup (one polygon per triangle).
pub fn mesh_to_polygons(mesh: &TriMesh) -> Vec<Polygon> {
    let mut polys = Vec::with_capacity(mesh.triangle_count());
    for i in 0..mesh.triangle_count() {
        let (a, b, c) = mesh.triangle_vertices(i);
        if let Some(p) = Polygon::from_triangle(a, b, c) {
            polys.push(p);
        }
    }
    polys
}

/// Convert a polygon soup back into a TriMesh.
/// Fan-triangulates any polygons with more than 3 vertices.
pub fn polygons_to_mesh(polygons: &[Polygon]) -> TriMesh {
    let mut mesh = TriMesh::new();

    for poly in polygons {
        let tris = poly.triangulate();
        for [a, b, c] in tris {
            let ia = mesh.add_vertex(a);
            let ib = mesh.add_vertex(b);
            let ic = mesh.add_vertex(c);
            mesh.add_triangle(ia, ib, ic);
        }
    }

    mesh
}

/// Convert a polygon soup to a TriMesh, welding duplicate vertices.
pub fn polygons_to_mesh_welded(polygons: &[Polygon], tolerance: f64) -> TriMesh {
    let mut mesh = polygons_to_mesh(polygons);
    mesh.weld_vertices(tolerance);
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_csg_mesh::mesh::make_unit_cube;

    #[test]
    fn test_roundtrip() {
        let cube = make_unit_cube();
        let polys = mesh_to_polygons(&cube);
        assert_eq!(polys.len(), 12);

        let mesh = polygons_to_mesh(&polys);
        assert_eq!(mesh.triangle_count(), 12);
        // Vertices are duplicated (3 per triangle)
        assert_eq!(mesh.vertex_count(), 36);
    }

    #[test]
    fn test_roundtrip_welded() {
        let cube = make_unit_cube();
        let polys = mesh_to_polygons(&cube);
        let mesh = polygons_to_mesh_welded(&polys, 0.001);
        assert_eq!(mesh.vertex_count(), 8);
        assert_eq!(mesh.triangle_count(), 12);
    }

    #[test]
    fn test_volume_preserved() {
        let cube = make_unit_cube();
        let vol1 = makepad_csg_mesh::volume::mesh_volume(&cube);

        let polys = mesh_to_polygons(&cube);
        let mesh = polygons_to_mesh(&polys);
        let vol2 = makepad_csg_mesh::volume::mesh_volume(&mesh);

        assert!(
            (vol1 - vol2).abs() < 1e-12,
            "volume should be preserved: {} vs {}",
            vol1,
            vol2
        );
    }
}
