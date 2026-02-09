// Volume computation for closed triangle meshes.
// Uses the signed tetrahedron volume method (divergence theorem).

use crate::mesh::TriMesh;

/// Compute the signed volume of a closed triangle mesh.
/// Positive for outward-facing normals (CCW winding).
/// Based on the divergence theorem: sum of signed tetrahedra volumes
/// formed by each triangle and the origin.
pub fn mesh_volume(mesh: &TriMesh) -> f64 {
    let mut vol = 0.0;
    for i in 0..mesh.triangle_count() {
        let (a, b, c) = mesh.triangle_vertices(i);
        // Signed volume of tetrahedron (origin, a, b, c) = (a . (b x c)) / 6
        vol += a.dot(b.cross(c));
    }
    vol / 6.0
}

/// Compute the surface area of a triangle mesh.
pub fn mesh_surface_area(mesh: &TriMesh) -> f64 {
    let mut area = 0.0;
    for i in 0..mesh.triangle_count() {
        area += mesh.triangle_area(i);
    }
    area
}

/// Compute the centroid of a closed triangle mesh (volume-weighted).
pub fn mesh_centroid(mesh: &TriMesh) -> makepad_csg_math::Vec3d {
    use makepad_csg_math::Vec3d;

    let mut cx = 0.0;
    let mut cy = 0.0;
    let mut cz = 0.0;
    let mut total_vol = 0.0;

    for i in 0..mesh.triangle_count() {
        let (a, b, c) = mesh.triangle_vertices(i);
        let vol = a.dot(b.cross(c));
        total_vol += vol;
        // Centroid contribution of this tetrahedron
        cx += vol * (a.x + b.x + c.x);
        cy += vol * (a.y + b.y + c.y);
        cz += vol * (a.z + b.z + c.z);
    }

    if total_vol.abs() < 1e-30 {
        return Vec3d::ZERO;
    }

    let inv = 1.0 / (4.0 * total_vol);
    Vec3d::new(cx * inv, cy * inv, cz * inv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_unit_cube;

    #[test]
    fn test_cube_volume() {
        let cube = make_unit_cube();
        let vol = mesh_volume(&cube);
        assert!(
            (vol - 1.0).abs() < 1e-12,
            "unit cube volume should be 1.0, got {}",
            vol
        );
    }

    #[test]
    fn test_cube_surface_area() {
        let cube = make_unit_cube();
        let area = mesh_surface_area(&cube);
        assert!(
            (area - 6.0).abs() < 1e-12,
            "unit cube surface area should be 6.0, got {}",
            area
        );
    }

    #[test]
    fn test_cube_centroid() {
        let cube = make_unit_cube();
        let c = mesh_centroid(&cube);
        assert!(c.x.abs() < 1e-12);
        assert!(c.y.abs() < 1e-12);
        assert!(c.z.abs() < 1e-12);
    }

    #[test]
    fn test_flipped_volume() {
        let mut cube = make_unit_cube();
        cube.flip_normals();
        let vol = mesh_volume(&cube);
        assert!(
            (vol - (-1.0)).abs() < 1e-12,
            "flipped cube volume should be -1.0, got {}",
            vol
        );
    }
}
