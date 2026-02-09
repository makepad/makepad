// TriMesh - indexed triangle mesh for CSG output
// Simple vertex array + triangle index array representation.

use makepad_csg_math::{dvec3, BBox3d, Mat4d, Vec3d};

#[derive(Clone, Debug)]
pub struct TriMesh {
    pub vertices: Vec<Vec3d>,
    pub triangles: Vec<[u32; 3]>,
}

impl TriMesh {
    pub fn new() -> TriMesh {
        TriMesh {
            vertices: Vec::new(),
            triangles: Vec::new(),
        }
    }

    pub fn with_capacity(num_verts: usize, num_tris: usize) -> TriMesh {
        TriMesh {
            vertices: Vec::with_capacity(num_verts),
            triangles: Vec::with_capacity(num_tris),
        }
    }

    pub fn add_vertex(&mut self, v: Vec3d) -> u32 {
        let idx = self.vertices.len() as u32;
        self.vertices.push(v);
        idx
    }

    pub fn add_triangle(&mut self, a: u32, b: u32, c: u32) {
        self.triangles.push([a, b, c]);
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    /// Face normal for triangle at index (not normalized).
    pub fn triangle_normal_unnormalized(&self, tri_idx: usize) -> Vec3d {
        let [a, b, c] = self.triangles[tri_idx];
        let va = self.vertices[a as usize];
        let vb = self.vertices[b as usize];
        let vc = self.vertices[c as usize];
        (vb - va).cross(vc - va)
    }

    /// Face normal for triangle at index (unit length).
    pub fn triangle_normal(&self, tri_idx: usize) -> Vec3d {
        self.triangle_normal_unnormalized(tri_idx).normalize()
    }

    /// Area of triangle at index.
    pub fn triangle_area(&self, tri_idx: usize) -> f64 {
        self.triangle_normal_unnormalized(tri_idx).length() * 0.5
    }

    /// Bounding box of all vertices.
    pub fn bounding_box(&self) -> BBox3d {
        BBox3d::from_points(&self.vertices)
    }

    /// Flip all triangle windings (reverses normals).
    pub fn flip_normals(&mut self) {
        for tri in &mut self.triangles {
            tri.swap(0, 1);
        }
    }

    /// Apply a transform to all vertices.
    /// Uses SIMD-batched transform when the `nightly` feature is enabled.
    pub fn transform(&mut self, mat: Mat4d) {
        makepad_csg_math::batch_transform_points(&mut self.vertices, &mat);
    }

    /// Append another mesh into this one.
    pub fn merge(&mut self, other: &TriMesh) {
        let offset = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&other.vertices);
        for &[a, b, c] in &other.triangles {
            self.triangles.push([a + offset, b + offset, c + offset]);
        }
    }

    /// Merge vertices that are closer than tolerance.
    /// Reindexes all triangles. Removes degenerate triangles.
    /// Uses a spatial hash grid for O(n) average-case performance.
    pub fn weld_vertices(&mut self, tolerance: f64) {
        let n = self.vertices.len();
        if n == 0 {
            return;
        }
        let tol_sq = tolerance * tolerance;
        // Cell size slightly larger than tolerance so that any two points within
        // tolerance must land in the same cell or adjacent cells.
        let cell = tolerance * 1.01;
        let inv_cell = 1.0 / cell;

        // Spatial hash grid: maps cell coordinate to list of (new_vert_index, position).
        let mut grid: std::collections::HashMap<(i64, i64, i64), Vec<(u32, Vec3d)>> =
            std::collections::HashMap::new();
        let mut remap = vec![0u32; n];
        let mut new_verts: Vec<Vec3d> = Vec::new();

        for i in 0..n {
            let vi = self.vertices[i];
            let cx = (vi.x * inv_cell).floor() as i64;
            let cy = (vi.y * inv_cell).floor() as i64;
            let cz = (vi.z * inv_cell).floor() as i64;

            // Search this cell and 26 neighbors (3x3x3 neighborhood)
            let mut found = false;
            'search: for dx in -1i64..=1 {
                for dy in -1i64..=1 {
                    for dz in -1i64..=1 {
                        let key = (cx + dx, cy + dy, cz + dz);
                        if let Some(bucket) = grid.get(&key) {
                            for &(idx, nv) in bucket {
                                if vi.distance_sq(nv) < tol_sq {
                                    remap[i] = idx;
                                    found = true;
                                    break 'search;
                                }
                            }
                        }
                    }
                }
            }
            if !found {
                let idx = new_verts.len() as u32;
                remap[i] = idx;
                new_verts.push(vi);
                grid.entry((cx, cy, cz)).or_default().push((idx, vi));
            }
        }

        self.vertices = new_verts;

        // Remap triangle indices and remove degenerate triangles
        let old_tris = std::mem::take(&mut self.triangles);
        for [a, b, c] in old_tris {
            let ra = remap[a as usize];
            let rb = remap[b as usize];
            let rc = remap[c as usize];
            if ra != rb && rb != rc && rc != ra {
                self.triangles.push([ra, rb, rc]);
            }
        }
    }

    /// Get the three vertex positions of a triangle.
    pub fn triangle_vertices(&self, tri_idx: usize) -> (Vec3d, Vec3d, Vec3d) {
        let [a, b, c] = self.triangles[tri_idx];
        (
            self.vertices[a as usize],
            self.vertices[b as usize],
            self.vertices[c as usize],
        )
    }
}

/// Build a unit cube mesh centered at origin (side length 1).
/// Useful for testing.
pub fn make_unit_cube() -> TriMesh {
    let mut m = TriMesh::with_capacity(8, 12);

    // 8 vertices of a unit cube centered at origin
    let v = [
        dvec3(-0.5, -0.5, -0.5), // 0
        dvec3(0.5, -0.5, -0.5),  // 1
        dvec3(0.5, 0.5, -0.5),   // 2
        dvec3(-0.5, 0.5, -0.5),  // 3
        dvec3(-0.5, -0.5, 0.5),  // 4
        dvec3(0.5, -0.5, 0.5),   // 5
        dvec3(0.5, 0.5, 0.5),    // 6
        dvec3(-0.5, 0.5, 0.5),   // 7
    ];
    for &vert in &v {
        m.add_vertex(vert);
    }

    // 12 triangles (2 per face, CCW winding = outward normal)
    // Front face (z = 0.5)
    m.add_triangle(4, 5, 6);
    m.add_triangle(4, 6, 7);
    // Back face (z = -0.5)
    m.add_triangle(1, 0, 3);
    m.add_triangle(1, 3, 2);
    // Right face (x = 0.5)
    m.add_triangle(5, 1, 2);
    m.add_triangle(5, 2, 6);
    // Left face (x = -0.5)
    m.add_triangle(0, 4, 7);
    m.add_triangle(0, 7, 3);
    // Top face (y = 0.5)
    m.add_triangle(7, 6, 2);
    m.add_triangle(7, 2, 3);
    // Bottom face (y = -0.5)
    m.add_triangle(0, 1, 5);
    m.add_triangle(0, 5, 4);

    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unit_cube() {
        let c = make_unit_cube();
        assert_eq!(c.vertex_count(), 8);
        assert_eq!(c.triangle_count(), 12);
    }

    #[test]
    fn test_bounding_box() {
        let c = make_unit_cube();
        let bb = c.bounding_box();
        assert_eq!(bb.min, dvec3(-0.5, -0.5, -0.5));
        assert_eq!(bb.max, dvec3(0.5, 0.5, 0.5));
    }

    #[test]
    fn test_cube_normals_point_outward() {
        let c = make_unit_cube();
        let center = dvec3(0.0, 0.0, 0.0);
        for i in 0..c.triangle_count() {
            let (va, vb, vc) = c.triangle_vertices(i);
            let tri_center = (va + vb + vc) / 3.0;
            let normal = c.triangle_normal(i);
            let to_center = center - tri_center;
            // Normal should point away from center
            assert!(
                normal.dot(to_center) < 0.0,
                "triangle {} normal points inward",
                i
            );
        }
    }

    #[test]
    fn test_triangle_area() {
        let c = make_unit_cube();
        let total_area: f64 = (0..c.triangle_count()).map(|i| c.triangle_area(i)).sum();
        // Unit cube: 6 faces, each 1x1 = total surface area 6.0
        assert!((total_area - 6.0).abs() < 1e-12);
    }

    #[test]
    fn test_flip_normals() {
        let mut c = make_unit_cube();
        let n_before = c.triangle_normal(0);
        c.flip_normals();
        let n_after = c.triangle_normal(0);
        assert!((n_before.dot(n_after) - (-1.0)).abs() < 1e-12);
    }

    #[test]
    fn test_transform() {
        let mut c = make_unit_cube();
        c.transform(Mat4d::translation(dvec3(10.0, 0.0, 0.0)));
        let bb = c.bounding_box();
        assert!((bb.min.x - 9.5).abs() < 1e-12);
        assert!((bb.max.x - 10.5).abs() < 1e-12);
    }

    #[test]
    fn test_merge() {
        let c1 = make_unit_cube();
        let mut c2 = make_unit_cube();
        c2.transform(Mat4d::translation(dvec3(5.0, 0.0, 0.0)));
        let mut merged = c1.clone();
        merged.merge(&c2);
        assert_eq!(merged.vertex_count(), 16);
        assert_eq!(merged.triangle_count(), 24);
    }

    #[test]
    fn test_weld() {
        // Two cubes sharing a face -> welding should merge 4 shared vertices
        let c1 = make_unit_cube();
        let mut c2 = make_unit_cube();
        c2.transform(Mat4d::translation(dvec3(1.0, 0.0, 0.0)));
        let mut merged = c1.clone();
        merged.merge(&c2);
        assert_eq!(merged.vertex_count(), 16);
        merged.weld_vertices(0.01);
        // Two unit cubes side by side share 4 vertices -> 16 - 4 = 12
        assert_eq!(merged.vertex_count(), 12);
    }
}
