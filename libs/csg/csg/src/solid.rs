// Solid - top-level CSG API
//
// Wraps a TriMesh and provides OpenSCAD-like boolean operations.
// Usage:
//   let result = Solid::cube(1.0, true)
//       .difference(&Solid::sphere(0.7, 32, 16))
//       .translate(10.0, 0.0, 0.0);
//   result.write_stl("output.stl").unwrap();

use makepad_csg_boolean::boolean as corefine_boolean;
use makepad_csg_math::{dvec3, BBox3d, Mat4d, Vec3d};
use makepad_csg_mesh::mesh::TriMesh;
use makepad_csg_mesh::validate::{validate_mesh, MeshReport};
use makepad_csg_mesh::volume::{mesh_centroid, mesh_surface_area, mesh_volume};
use makepad_csg_sdf::Sdf3;

#[derive(Clone, Debug)]
pub struct Solid {
    pub(crate) mesh: TriMesh,
}

impl Solid {
    /// Create a Solid from an existing TriMesh.
    pub fn from_mesh(mesh: TriMesh) -> Solid {
        Solid { mesh }
    }

    /// Create an empty Solid (no geometry).
    pub fn empty() -> Solid {
        Solid {
            mesh: TriMesh::new(),
        }
    }

    /// True if this Solid has no triangles.
    pub fn is_empty(&self) -> bool {
        self.mesh.triangle_count() == 0
    }

    /// Access the underlying triangle mesh (read-only).
    pub fn mesh(&self) -> &TriMesh {
        &self.mesh
    }

    /// Consume the Solid and return the underlying TriMesh.
    pub fn into_mesh(self) -> TriMesh {
        self.mesh
    }

    // --- Primitives ---

    /// Axis-aligned box.
    /// `sx, sy, sz`: dimensions. `center`: if true, centered at origin.
    pub fn cube(sx: f64, sy: f64, sz: f64, center: bool) -> Solid {
        Solid {
            mesh: makepad_csg_primitives::cube(dvec3(sx, sy, sz), center),
        }
    }

    /// Shorthand for a cube with equal sides.
    pub fn cube_uniform(size: f64, center: bool) -> Solid {
        Self::cube(size, size, size, center)
    }

    /// UV sphere.
    /// `radius`: sphere radius.
    /// `segments`: longitude divisions. `rings`: latitude divisions.
    pub fn sphere(radius: f64, segments: u32, rings: u32) -> Solid {
        Solid {
            mesh: makepad_csg_primitives::sphere(radius, segments, rings),
        }
    }

    /// Cylinder along the Y axis.
    /// `radius`: cylinder radius. `height`: cylinder height.
    /// `segments`: radial divisions. `center`: if true, centered at origin.
    pub fn cylinder(radius: f64, height: f64, segments: u32, center: bool) -> Solid {
        Solid {
            mesh: makepad_csg_primitives::cylinder(radius, height, segments, center),
        }
    }

    /// Cone along the Y axis.
    /// `radius`: base radius. `height`: cone height.
    /// `segments`: radial divisions. `center`: if true, centered at origin.
    pub fn cone(radius: f64, height: f64, segments: u32, center: bool) -> Solid {
        Solid {
            mesh: makepad_csg_primitives::cone(radius, height, segments, center),
        }
    }

    /// Torus in the XZ plane, centered at origin.
    /// `major_radius`: ring radius. `minor_radius`: tube radius.
    pub fn torus(
        major_radius: f64,
        minor_radius: f64,
        major_segments: u32,
        minor_segments: u32,
    ) -> Solid {
        Solid {
            mesh: makepad_csg_primitives::torus(
                major_radius,
                minor_radius,
                major_segments,
                minor_segments,
            ),
        }
    }

    /// Extrude a 2D polygon (XZ plane) along the Y axis.
    /// `polygon`: closed CCW polygon as (x, z) pairs.
    pub fn extrude(polygon: &[[f64; 2]], height: f64) -> Solid {
        Solid {
            mesh: makepad_csg_primitives::extrude(polygon, height),
        }
    }

    /// Tapered cylinder (frustum) along Y axis.
    /// `r1`: bottom radius, `r2`: top radius.
    /// When r1 == r2, produces a cylinder. When r2 == 0, produces a cone.
    pub fn tapered_cylinder(r1: f64, r2: f64, height: f64, segments: u32, center: bool) -> Solid {
        Solid {
            mesh: makepad_csg_primitives::tapered_cylinder(r1, r2, height, segments, center),
        }
    }

    /// Construct a solid from explicit vertices and triangular faces.
    /// `vertices`: list of 3D points.
    /// `faces`: list of triangle index triples (0-based).
    pub fn polyhedron(vertices: &[Vec3d], faces: &[[u32; 3]]) -> Solid {
        let mut mesh = TriMesh::with_capacity(vertices.len(), faces.len());
        for &v in vertices {
            mesh.add_vertex(v);
        }
        for &[a, b, c] in faces {
            mesh.add_triangle(a, b, c);
        }
        Solid { mesh }
    }

    /// Extrude a 2D polygon with twist and/or scale.
    /// `twist_degrees`: total rotation from bottom to top.
    /// `end_scale`: scale factor at top (1.0 = uniform).
    /// `slices`: number of intermediate layers.
    pub fn linear_extrude(
        polygon: &[[f64; 2]],
        height: f64,
        twist_degrees: f64,
        end_scale: f64,
        slices: u32,
    ) -> Solid {
        Solid {
            mesh: makepad_csg_primitives::linear_extrude_advanced(
                polygon,
                height,
                twist_degrees,
                end_scale,
                slices,
            ),
        }
    }

    /// Revolve a 2D profile around the Y axis (lathe).
    /// `profile`: 2D points as (radius, y) pairs forming a closed polygon.
    /// `angle_degrees`: sweep angle (360 = full revolution).
    /// `segments`: angular divisions.
    pub fn rotate_extrude(profile: &[[f64; 2]], angle_degrees: f64, segments: u32) -> Solid {
        Solid {
            mesh: makepad_csg_primitives::rotate_extrude(profile, angle_degrees, segments),
        }
    }

    // --- SDF meshing ---

    /// Create a Solid from a signed distance field via dual contouring.
    ///
    /// - `sdf`: any type implementing `Sdf3`
    /// - `min`, `max`: bounding box for the meshing volume
    /// - `depth`: octree depth (6-8 typical; higher = more triangles/detail)
    pub fn from_sdf(
        sdf: impl Sdf3 + Send + Sync + 'static,
        min: Vec3d,
        max: Vec3d,
        depth: usize,
    ) -> Solid {
        Solid {
            mesh: makepad_csg_sdf::sdf_to_mesh(sdf, min, max, depth),
        }
    }

    // --- Boolean operations (corefinement-based) ---

    /// Union: combine volumes of self and other.
    pub fn union(&self, other: &Solid) -> Solid {
        if self.is_empty() {
            return other.clone();
        }
        if other.is_empty() {
            return self.clone();
        }
        Solid {
            mesh: corefine_boolean::union(&self.mesh, &other.mesh),
        }
    }

    /// Difference: subtract other from self.
    pub fn difference(&self, other: &Solid) -> Solid {
        if self.is_empty() {
            return Solid::empty();
        }
        if other.is_empty() {
            return self.clone();
        }
        Solid {
            mesh: corefine_boolean::difference(&self.mesh, &other.mesh),
        }
    }

    /// Intersection: keep only overlapping volume.
    pub fn intersection(&self, other: &Solid) -> Solid {
        if self.is_empty() || other.is_empty() {
            return Solid::empty();
        }
        Solid {
            mesh: corefine_boolean::intersection(&self.mesh, &other.mesh),
        }
    }

    /// Symmetric difference (XOR): volume in either but not both.
    pub fn symmetric_difference(&self, other: &Solid) -> Solid {
        self.union(other).difference(&self.intersection(other))
    }

    /// Alias for `union` (kept for backwards compatibility).
    pub fn union_corefine(&self, other: &Solid) -> Solid {
        self.union(other)
    }
    /// Alias for `difference` (kept for backwards compatibility).
    pub fn difference_corefine(&self, other: &Solid) -> Solid {
        self.difference(other)
    }
    /// Alias for `intersection` (kept for backwards compatibility).
    pub fn intersection_corefine(&self, other: &Solid) -> Solid {
        self.intersection(other)
    }
    /// Alias for `symmetric_difference` (kept for backwards compatibility).
    pub fn symmetric_difference_corefine(&self, other: &Solid) -> Solid {
        self.symmetric_difference(other)
    }

    // --- Transforms ---

    /// Translate (move) this solid.
    pub fn translate(&self, x: f64, y: f64, z: f64) -> Solid {
        let mut mesh = self.mesh.clone();
        mesh.transform(Mat4d::translation(dvec3(x, y, z)));
        Solid { mesh }
    }

    /// Rotate around the X axis (angle in degrees).
    pub fn rotate_x(&self, degrees: f64) -> Solid {
        let mut mesh = self.mesh.clone();
        mesh.transform(Mat4d::rotate_x(degrees.to_radians()));
        Solid { mesh }
    }

    /// Rotate around the Y axis (angle in degrees).
    pub fn rotate_y(&self, degrees: f64) -> Solid {
        let mut mesh = self.mesh.clone();
        mesh.transform(Mat4d::rotate_y(degrees.to_radians()));
        Solid { mesh }
    }

    /// Rotate around the Z axis (angle in degrees).
    pub fn rotate_z(&self, degrees: f64) -> Solid {
        let mut mesh = self.mesh.clone();
        mesh.transform(Mat4d::rotate_z(degrees.to_radians()));
        Solid { mesh }
    }

    /// Rotate around an arbitrary axis (angle in degrees).
    pub fn rotate(&self, axis: Vec3d, degrees: f64) -> Solid {
        let mut mesh = self.mesh.clone();
        mesh.transform(Mat4d::rotation(axis, degrees.to_radians()));
        Solid { mesh }
    }

    /// Non-uniform scale.
    pub fn scale(&self, sx: f64, sy: f64, sz: f64) -> Solid {
        let mut mesh = self.mesh.clone();
        mesh.transform(Mat4d::scale_xyz(dvec3(sx, sy, sz)));
        Solid { mesh }
    }

    /// Uniform scale.
    pub fn scale_uniform(&self, s: f64) -> Solid {
        self.scale(s, s, s)
    }

    /// Apply an arbitrary 4x4 transform matrix.
    pub fn transform(&self, mat: Mat4d) -> Solid {
        let mut mesh = self.mesh.clone();
        mesh.transform(mat);
        Solid { mesh }
    }

    /// Mirror across a plane defined by axis (flips geometry and normals).
    /// `axis`: 0=X (YZ plane), 1=Y (XZ plane), 2=Z (XY plane).
    pub fn mirror(&self, axis: usize) -> Solid {
        let (sx, sy, sz) = match axis {
            0 => (-1.0, 1.0, 1.0),
            1 => (1.0, -1.0, 1.0),
            _ => (1.0, 1.0, -1.0),
        };
        let mut mesh = self.mesh.clone();
        mesh.transform(Mat4d::scale_xyz(dvec3(sx, sy, sz)));
        mesh.flip_normals(); // Negative scale inverts winding; flip to restore
        Solid { mesh }
    }

    /// Resize to fit within the given bounding box dimensions.
    /// Zero values in target preserve the original aspect ratio.
    pub fn resize(&self, target_x: f64, target_y: f64, target_z: f64) -> Solid {
        let bb = self.mesh.bounding_box();
        let cur = dvec3(
            bb.max.x - bb.min.x,
            bb.max.y - bb.min.y,
            bb.max.z - bb.min.z,
        );
        // Compute scale factors; zero means "auto from other axes"
        let mut sx = if target_x > 0.0 && cur.x > 1e-15 {
            target_x / cur.x
        } else {
            0.0
        };
        let mut sy = if target_y > 0.0 && cur.y > 1e-15 {
            target_y / cur.y
        } else {
            0.0
        };
        let mut sz = if target_z > 0.0 && cur.z > 1e-15 {
            target_z / cur.z
        } else {
            0.0
        };

        // For zero (auto) axes, use the uniform scale of the specified axes
        let specified: Vec<f64> = [sx, sy, sz].iter().copied().filter(|&s| s > 0.0).collect();
        if !specified.is_empty() {
            let uniform = specified.iter().sum::<f64>() / specified.len() as f64;
            if sx == 0.0 {
                sx = uniform;
            }
            if sy == 0.0 {
                sy = uniform;
            }
            if sz == 0.0 {
                sz = uniform;
            }
        } else {
            return self.clone(); // All zero, nothing to resize
        }

        self.scale(sx, sy, sz)
    }

    // --- Queries ---

    /// Signed volume of the solid (positive for outward-facing normals).
    pub fn volume(&self) -> f64 {
        mesh_volume(&self.mesh)
    }

    /// Total surface area.
    pub fn surface_area(&self) -> f64 {
        mesh_surface_area(&self.mesh)
    }

    /// Volume-weighted centroid.
    pub fn centroid(&self) -> Vec3d {
        mesh_centroid(&self.mesh)
    }

    /// Axis-aligned bounding box.
    pub fn bounding_box(&self) -> BBox3d {
        self.mesh.bounding_box()
    }

    /// Number of triangles in the mesh.
    pub fn triangle_count(&self) -> usize {
        self.mesh.triangle_count()
    }

    /// Number of vertices in the mesh.
    pub fn vertex_count(&self) -> usize {
        self.mesh.vertex_count()
    }

    /// Validate mesh topology (manifold, closed, oriented).
    pub fn validate(&self) -> MeshReport {
        validate_mesh(&self.mesh)
    }

    /// Check if the mesh is a valid closed manifold.
    pub fn is_valid(&self) -> bool {
        let report = self.validate();
        report.is_closed && report.is_manifold && report.is_consistently_oriented
    }

    // --- Mesh manipulation ---

    /// Flip all face normals (turns the solid inside out).
    pub fn flip(&self) -> Solid {
        let mut mesh = self.mesh.clone();
        mesh.flip_normals();
        Solid { mesh }
    }

    /// Merge another solid's geometry without performing boolean ops.
    /// Useful for building multi-part assemblies.
    pub fn merge(&self, other: &Solid) -> Solid {
        let mut mesh = self.mesh.clone();
        mesh.merge(&other.mesh);
        Solid { mesh }
    }

    /// Weld duplicate vertices within the given tolerance.
    pub fn weld(&self, tolerance: f64) -> Solid {
        let mut mesh = self.mesh.clone();
        mesh.weld_vertices(tolerance);
        Solid { mesh }
    }

    // --- I/O ---

    /// Write to binary STL (standard 3D printing format).
    pub fn write_stl(&self, path: &str) -> std::io::Result<()> {
        makepad_csg_mesh::stl::write_stl_binary(&self.mesh, path)
    }

    /// Write to ASCII STL.
    pub fn write_stl_ascii(&self, path: &str) -> std::io::Result<()> {
        makepad_csg_mesh::stl::write_stl_ascii(&self.mesh, path)
    }

    /// Read from binary STL.
    pub fn read_stl(path: &str) -> std::io::Result<Solid> {
        let mut mesh = makepad_csg_mesh::stl::read_stl_binary(path)?;
        mesh.weld_vertices(1e-6); // STL is unindexed; weld at f32 precision
        Ok(Solid { mesh })
    }

    /// Read from ASCII STL.
    pub fn read_stl_ascii(path: &str) -> std::io::Result<Solid> {
        let mut mesh = makepad_csg_mesh::stl::read_stl_ascii(path)?;
        mesh.weld_vertices(1e-6);
        Ok(Solid { mesh })
    }

    /// Write to OBJ format.
    pub fn write_obj(&self, path: &str) -> std::io::Result<()> {
        makepad_csg_mesh::obj::write_obj(&self.mesh, path)
    }

    /// Read from OBJ format.
    pub fn read_obj(path: &str) -> std::io::Result<Solid> {
        let mesh = makepad_csg_mesh::obj::read_obj(path)?;
        Ok(Solid { mesh })
    }
}

// --- Multi-solid operations ---

/// Union of multiple solids.
pub fn union_all(solids: &[Solid]) -> Solid {
    if solids.is_empty() {
        return Solid::empty();
    }
    let mut result = solids[0].clone();
    for s in &solids[1..] {
        result = result.union(s);
    }
    result
}

/// Difference: subtract all subsequent solids from the first.
pub fn difference_all(solids: &[Solid]) -> Solid {
    if solids.is_empty() {
        return Solid::empty();
    }
    let mut result = solids[0].clone();
    for s in &solids[1..] {
        result = result.difference(s);
    }
    result
}

/// Intersection of multiple solids.
pub fn intersection_all(solids: &[Solid]) -> Solid {
    if solids.is_empty() {
        return Solid::empty();
    }
    let mut result = solids[0].clone();
    for s in &solids[1..] {
        result = result.intersection(s);
    }
    result
}
