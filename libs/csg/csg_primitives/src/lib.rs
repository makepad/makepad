// CSG primitive shape generators.
// All produce closed, manifold TriMesh with outward-facing normals (CCW winding).

use makepad_csg_math::{dvec3, Vec3d};
use makepad_csg_mesh::mesh::TriMesh;
use std::f64::consts::PI;

/// Create a box/cube mesh.
/// `size`: dimensions in x, y, z.
/// `center`: if true, centered at origin; otherwise, corner at origin.
pub fn cube(size: Vec3d, center: bool) -> TriMesh {
    let (ox, oy, oz) = if center {
        (-size.x * 0.5, -size.y * 0.5, -size.z * 0.5)
    } else {
        (0.0, 0.0, 0.0)
    };

    let mut m = TriMesh::with_capacity(8, 12);
    let v = [
        dvec3(ox, oy, oz),                            // 0
        dvec3(ox + size.x, oy, oz),                   // 1
        dvec3(ox + size.x, oy + size.y, oz),          // 2
        dvec3(ox, oy + size.y, oz),                   // 3
        dvec3(ox, oy, oz + size.z),                   // 4
        dvec3(ox + size.x, oy, oz + size.z),          // 5
        dvec3(ox + size.x, oy + size.y, oz + size.z), // 6
        dvec3(ox, oy + size.y, oz + size.z),          // 7
    ];
    for &vert in &v {
        m.add_vertex(vert);
    }
    // Front (+Z)
    m.add_triangle(4, 5, 6);
    m.add_triangle(4, 6, 7);
    // Back (-Z)
    m.add_triangle(1, 0, 3);
    m.add_triangle(1, 3, 2);
    // Right (+X)
    m.add_triangle(5, 1, 2);
    m.add_triangle(5, 2, 6);
    // Left (-X)
    m.add_triangle(0, 4, 7);
    m.add_triangle(0, 7, 3);
    // Top (+Y)
    m.add_triangle(7, 6, 2);
    m.add_triangle(7, 2, 3);
    // Bottom (-Y)
    m.add_triangle(0, 1, 5);
    m.add_triangle(0, 5, 4);
    m
}

/// Create a UV sphere mesh.
/// `radius`: sphere radius.
/// `segments`: longitude divisions (around Y axis).
/// `rings`: latitude divisions (top to bottom).
pub fn sphere(radius: f64, segments: u32, rings: u32) -> TriMesh {
    let segments = segments.max(3);
    let rings = rings.max(2);
    let num_verts = (segments * (rings - 1) + 2) as usize;
    let num_tris = (segments * 2 + segments * (rings - 2) * 2) as usize;
    let mut m = TriMesh::with_capacity(num_verts, num_tris);

    // Top pole
    let top = m.add_vertex(dvec3(0.0, radius, 0.0));

    // Ring vertices (from top to bottom, excluding poles)
    let mut ring_start = Vec::with_capacity(rings as usize - 1);
    for ring in 1..rings {
        let phi = PI * ring as f64 / rings as f64;
        let y = radius * phi.cos();
        let r = radius * phi.sin();
        let start = m.vertex_count() as u32;
        ring_start.push(start);
        for seg in 0..segments {
            let theta = 2.0 * PI * seg as f64 / segments as f64;
            let x = r * theta.cos();
            let z = r * theta.sin();
            m.add_vertex(dvec3(x, y, z));
        }
    }

    // Bottom pole
    let bottom = m.add_vertex(dvec3(0.0, -radius, 0.0));

    // Top cap triangles (outward = away from center = upward near pole)
    for seg in 0..segments {
        let next = (seg + 1) % segments;
        m.add_triangle(top, ring_start[0] + next, ring_start[0] + seg);
    }

    // Middle band triangles
    for ring in 0..(rings as usize - 2) {
        let r0 = ring_start[ring];
        let r1 = ring_start[ring + 1];
        for seg in 0..segments {
            let next = (seg + 1) % segments;
            m.add_triangle(r0 + seg, r1 + next, r1 + seg);
            m.add_triangle(r0 + seg, r0 + next, r1 + next);
        }
    }

    // Bottom cap triangles
    let last_ring = *ring_start.last().unwrap();
    for seg in 0..segments {
        let next = (seg + 1) % segments;
        m.add_triangle(last_ring + seg, last_ring + next, bottom);
    }

    m
}

/// Create a cylinder mesh along the Y axis.
/// `radius`: cylinder radius.
/// `height`: cylinder height.
/// `segments`: number of radial divisions.
/// `center`: if true, centered at origin; otherwise, base at origin.
pub fn cylinder(radius: f64, height: f64, segments: u32, center: bool) -> TriMesh {
    let segments = segments.max(3);
    let y0 = if center { -height * 0.5 } else { 0.0 };
    let y1 = y0 + height;

    let num_verts = (segments * 2 + 2) as usize;
    let num_tris = (segments * 4) as usize;
    let mut m = TriMesh::with_capacity(num_verts, num_tris);

    // Bottom center and top center
    let bc = m.add_vertex(dvec3(0.0, y0, 0.0));
    let tc = m.add_vertex(dvec3(0.0, y1, 0.0));

    // Bottom ring and top ring vertices
    let bottom_start = m.vertex_count() as u32;
    for seg in 0..segments {
        let theta = 2.0 * PI * seg as f64 / segments as f64;
        let x = radius * theta.cos();
        let z = radius * theta.sin();
        m.add_vertex(dvec3(x, y0, z));
    }
    let top_start = m.vertex_count() as u32;
    for seg in 0..segments {
        let theta = 2.0 * PI * seg as f64 / segments as f64;
        let x = radius * theta.cos();
        let z = radius * theta.sin();
        m.add_vertex(dvec3(x, y1, z));
    }

    for seg in 0..segments {
        let next = (seg + 1) % segments;

        // Bottom cap (outward -Y normal)
        m.add_triangle(bc, bottom_start + seg, bottom_start + next);

        // Top cap (outward +Y normal)
        m.add_triangle(tc, top_start + next, top_start + seg);

        // Side quads (two triangles, outward radial normal)
        m.add_triangle(bottom_start + seg, top_start + next, bottom_start + next);
        m.add_triangle(bottom_start + seg, top_start + seg, top_start + next);
    }

    m
}

/// Create a cone mesh along the Y axis.
/// `radius`: base radius.
/// `height`: cone height.
/// `segments`: number of radial divisions.
/// `center`: if true, centered at origin; otherwise, base at origin.
pub fn cone(radius: f64, height: f64, segments: u32, center: bool) -> TriMesh {
    let segments = segments.max(3);
    let y0 = if center { -height * 0.5 } else { 0.0 };
    let y1 = y0 + height;

    let mut m = TriMesh::with_capacity((segments + 2) as usize, (segments * 2) as usize);

    // Base center
    let bc = m.add_vertex(dvec3(0.0, y0, 0.0));
    // Apex
    let apex = m.add_vertex(dvec3(0.0, y1, 0.0));

    // Base ring
    let base_start = m.vertex_count() as u32;
    for seg in 0..segments {
        let theta = 2.0 * PI * seg as f64 / segments as f64;
        m.add_vertex(dvec3(radius * theta.cos(), y0, radius * theta.sin()));
    }

    for seg in 0..segments {
        let next = (seg + 1) % segments;
        // Base cap (outward -Y normal)
        m.add_triangle(bc, base_start + seg, base_start + next);
        // Side (outward normal)
        m.add_triangle(base_start + seg, apex, base_start + next);
    }

    m
}

/// Create a torus mesh in the XZ plane, centered at origin.
/// `major_radius`: distance from center of torus to center of tube.
/// `minor_radius`: radius of the tube.
/// `major_segments`: divisions around the main ring.
/// `minor_segments`: divisions around the tube cross-section.
pub fn torus(
    major_radius: f64,
    minor_radius: f64,
    major_segments: u32,
    minor_segments: u32,
) -> TriMesh {
    let major_segments = major_segments.max(3);
    let minor_segments = minor_segments.max(3);

    let num_verts = (major_segments * minor_segments) as usize;
    let num_tris = (major_segments * minor_segments * 2) as usize;
    let mut m = TriMesh::with_capacity(num_verts, num_tris);

    // Generate vertices
    for i in 0..major_segments {
        let theta = 2.0 * PI * i as f64 / major_segments as f64;
        let ct = theta.cos();
        let st = theta.sin();

        for j in 0..minor_segments {
            let phi = 2.0 * PI * j as f64 / minor_segments as f64;
            let cp = phi.cos();
            let sp = phi.sin();

            let x = (major_radius + minor_radius * cp) * ct;
            let y = minor_radius * sp;
            let z = (major_radius + minor_radius * cp) * st;
            m.add_vertex(dvec3(x, y, z));
        }
    }

    // Generate triangles
    for i in 0..major_segments {
        let i_next = (i + 1) % major_segments;
        for j in 0..minor_segments {
            let j_next = (j + 1) % minor_segments;
            let v00 = i * minor_segments + j;
            let v10 = i_next * minor_segments + j;
            let v01 = i * minor_segments + j_next;
            let v11 = i_next * minor_segments + j_next;
            m.add_triangle(v00, v11, v10);
            m.add_triangle(v00, v01, v11);
        }
    }

    m
}

/// Create a tapered cylinder (frustum) along the Y axis.
/// `r1`: bottom radius. `r2`: top radius.
/// When r1 == r2, this is a regular cylinder. When r2 == 0, this is a cone.
/// `height`: cylinder height.
/// `segments`: number of radial divisions.
/// `center`: if true, centered at origin; otherwise, base at origin.
pub fn tapered_cylinder(r1: f64, r2: f64, height: f64, segments: u32, center: bool) -> TriMesh {
    let segments = segments.max(3);
    let y0 = if center { -height * 0.5 } else { 0.0 };
    let y1 = y0 + height;
    let has_bottom_cap = r1.abs() > 1e-15;
    let has_top_cap = r2.abs() > 1e-15;

    // Vertex count: bottom ring + top ring + center vertices for caps
    let ring_verts = segments as usize * 2;
    let center_verts = has_bottom_cap as usize + has_top_cap as usize;
    let cap_tris = (if has_bottom_cap { segments } else { 0 }
        + if has_top_cap { segments } else { 0 }) as usize;
    let side_tris = if has_bottom_cap && has_top_cap {
        segments as usize * 2
    } else {
        segments as usize // triangle fan to apex
    };
    let mut m = TriMesh::with_capacity(ring_verts + center_verts, cap_tris + side_tris);

    // Center vertices for caps
    let bc = if has_bottom_cap {
        Some(m.add_vertex(dvec3(0.0, y0, 0.0)))
    } else {
        None
    };
    let tc = if has_top_cap {
        Some(m.add_vertex(dvec3(0.0, y1, 0.0)))
    } else {
        None
    };

    // Bottom ring
    let bottom_start = m.vertex_count() as u32;
    if has_bottom_cap {
        for seg in 0..segments {
            let theta = 2.0 * PI * seg as f64 / segments as f64;
            m.add_vertex(dvec3(r1 * theta.cos(), y0, r1 * theta.sin()));
        }
    } else {
        // Apex at bottom
        m.add_vertex(dvec3(0.0, y0, 0.0));
    }

    // Top ring
    let top_start = m.vertex_count() as u32;
    if has_top_cap {
        for seg in 0..segments {
            let theta = 2.0 * PI * seg as f64 / segments as f64;
            m.add_vertex(dvec3(r2 * theta.cos(), y1, r2 * theta.sin()));
        }
    } else {
        // Apex at top
        m.add_vertex(dvec3(0.0, y1, 0.0));
    }

    for seg in 0..segments {
        let next = (seg + 1) % segments;

        // Bottom cap
        if let Some(bc) = bc {
            m.add_triangle(bc, bottom_start + seg, bottom_start + next);
        }

        // Top cap
        if let Some(tc) = tc {
            m.add_triangle(tc, top_start + next, top_start + seg);
        }

        // Side faces
        if has_bottom_cap && has_top_cap {
            // Quad (two triangles)
            m.add_triangle(bottom_start + seg, top_start + next, bottom_start + next);
            m.add_triangle(bottom_start + seg, top_start + seg, top_start + next);
        } else if has_bottom_cap {
            // Triangle fan to top apex (outward normal)
            m.add_triangle(bottom_start + seg, top_start, bottom_start + next);
        } else {
            // Triangle fan from bottom apex (outward normal)
            m.add_triangle(bottom_start, top_start + seg, top_start + next);
        }
    }

    m
}

/// Triangulate a simple polygon using ear clipping.
/// Works for both convex and concave (non-self-intersecting) polygons.
/// `polygon`: 2D points in CCW order.
/// Returns triangle indices into the polygon array.
pub fn ear_clip_triangulate(polygon: &[[f64; 2]]) -> Vec<[usize; 3]> {
    let n = polygon.len();
    if n < 3 {
        return Vec::new();
    }
    if n == 3 {
        return vec![[0, 1, 2]];
    }

    let mut indices: Vec<usize> = (0..n).collect();
    let mut triangles = Vec::with_capacity(n - 2);

    // Ensure CCW winding
    let area2: f64 = indices
        .windows(2)
        .map(|w| {
            let a = polygon[w[0]];
            let b = polygon[w[1]];
            (b[0] - a[0]) * (b[1] + a[1])
        })
        .sum::<f64>()
        + {
            let a = polygon[*indices.last().unwrap()];
            let b = polygon[indices[0]];
            (b[0] - a[0]) * (b[1] + a[1])
        };
    if area2 > 0.0 {
        indices.reverse(); // Was CW, flip to CCW
    }

    let mut remaining = indices;

    let mut max_iter = remaining.len() * remaining.len();
    while remaining.len() > 2 && max_iter > 0 {
        max_iter -= 1;
        let n = remaining.len();
        let mut found_ear = false;
        for i in 0..n {
            let prev = remaining[(i + n - 1) % n];
            let curr = remaining[i];
            let next = remaining[(i + 1) % n];

            let a = polygon[prev];
            let b = polygon[curr];
            let c = polygon[next];

            // Check if this is a convex vertex (left turn)
            let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            if cross <= 0.0 {
                continue; // Reflex vertex, not an ear
            }

            // Check no other vertex is inside the triangle
            let mut has_point_inside = false;
            for j in 0..n {
                if j == (i + n - 1) % n || j == i || j == (i + 1) % n {
                    continue;
                }
                let p = polygon[remaining[j]];
                if point_in_triangle_2d(p, a, b, c) {
                    has_point_inside = true;
                    break;
                }
            }
            if has_point_inside {
                continue;
            }

            // Found an ear — clip it
            triangles.push([prev, curr, next]);
            remaining.remove(i);
            found_ear = true;
            break;
        }
        if !found_ear {
            break; // Degenerate polygon
        }
    }

    triangles
}

/// Point-in-triangle test for 2D ear clipping.
fn point_in_triangle_2d(p: [f64; 2], a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> bool {
    let d0 = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    let d1 = (c[0] - b[0]) * (p[1] - b[1]) - (c[1] - b[1]) * (p[0] - b[0]);
    let d2 = (a[0] - c[0]) * (p[1] - c[1]) - (a[1] - c[1]) * (p[0] - c[0]);
    let has_neg = d0 < 0.0 || d1 < 0.0 || d2 < 0.0;
    let has_pos = d0 > 0.0 || d1 > 0.0 || d2 > 0.0;
    !(has_neg && has_pos)
}

/// Extrude a 2D polygon along the Y axis with optional twist and scale.
/// `polygon`: list of 2D points (in XZ plane) forming a closed polygon (CCW).
/// `height`: extrusion height.
/// `twist_degrees`: total twist angle from bottom to top (0 = no twist).
/// `end_scale`: scale factor at top (1.0 = no taper, 0.5 = half size at top).
/// `slices`: number of intermediate layers (1 = just bottom and top).
/// Uses ear-clipping for non-convex polygon support.
pub fn linear_extrude_advanced(
    polygon: &[[f64; 2]],
    height: f64,
    twist_degrees: f64,
    end_scale: f64,
    slices: u32,
) -> TriMesh {
    let n = polygon.len();
    if n < 3 {
        return TriMesh::new();
    }
    let slices = slices.max(1);
    let twist_rad = twist_degrees.to_radians();

    let layers = slices + 1; // number of vertex layers (bottom + intermediate + top)
    let verts_per_layer = n;
    let num_verts = verts_per_layer * layers as usize;
    let cap_tris = ear_clip_triangulate(polygon);
    let side_tris = n * slices as usize * 2;
    let mut m = TriMesh::with_capacity(num_verts, cap_tris.len() * 2 + side_tris);

    // Generate vertices for each layer
    for layer in 0..layers {
        let t = layer as f64 / slices as f64;
        let y = height * t;
        let angle = twist_rad * t;
        let scale = 1.0 + (end_scale - 1.0) * t;
        let cos_a = angle.cos();
        let sin_a = angle.sin();

        for &[px, pz] in polygon {
            let sx = px * scale;
            let sz = pz * scale;
            // Rotate around Y axis
            let x = sx * cos_a - sz * sin_a;
            let z = sx * sin_a + sz * cos_a;
            m.add_vertex(dvec3(x, y, z));
        }
    }

    let n = n as u32;
    let top_is_point = end_scale.abs() < 1e-12;

    // Bottom cap (y = 0, layer 0)
    for &[a, b, c] in &cap_tris {
        m.add_triangle(a as u32, b as u32, c as u32);
    }

    // Top cap — skip if collapsed to a point
    if !top_is_point {
        let top_offset = n * slices;
        for &[a, b, c] in &cap_tris {
            m.add_triangle(
                top_offset + a as u32,
                top_offset + c as u32,
                top_offset + b as u32,
            );
        }
    }

    // Side quads between consecutive layers
    for layer in 0..slices {
        let base = n * layer;
        let next_base = n * (layer + 1);
        let next_is_point = top_is_point && layer == slices - 1;

        if next_is_point {
            // Triangle fan to apex (all top verts collapse to same point; use first)
            let apex = next_base;
            for i in 0..n {
                let next_i = (i + 1) % n;
                m.add_triangle(base + i, apex, base + next_i);
            }
        } else {
            for i in 0..n {
                let next_i = (i + 1) % n;
                m.add_triangle(base + i, next_base + next_i, base + next_i);
                m.add_triangle(base + i, next_base + i, next_base + next_i);
            }
        }
    }

    m
}

/// Revolve a 2D profile around the Y axis to create a solid of revolution (lathe).
/// `profile`: 2D points as (radius, y) pairs. The profile should be a closed
///            polygon where x values represent distance from the Y axis.
///            Points with x=0 are on the axis. Profile must be CCW.
/// `angle_degrees`: sweep angle (360 = full revolution).
/// `segments`: number of angular divisions.
pub fn rotate_extrude(profile: &[[f64; 2]], angle_degrees: f64, segments: u32) -> TriMesh {
    let n = profile.len();
    if n < 2 {
        return TriMesh::new();
    }
    let segments = segments.max(3);
    let angle_rad = angle_degrees.to_radians();
    let full_revolution = (angle_degrees - 360.0).abs() < 1e-10;

    let actual_segments = if full_revolution { segments } else { segments };
    let layers = if full_revolution {
        actual_segments
    } else {
        actual_segments + 1
    };

    let num_verts = n * layers as usize;
    let num_tris = n * actual_segments as usize * 2;
    let mut m = TriMesh::with_capacity(num_verts, num_tris);

    // Generate vertices for each angular position
    for layer in 0..layers {
        let t = layer as f64 / actual_segments as f64;
        let theta = angle_rad * t;
        let cos_t = theta.cos();
        let sin_t = theta.sin();

        for &[r, y] in profile {
            let x = r * cos_t;
            let z = r * sin_t;
            m.add_vertex(dvec3(x, y, z));
        }
    }

    let n = n as u32;

    // Side quads between consecutive layers
    let num_layer_pairs = if full_revolution { layers } else { layers - 1 };
    for layer in 0..num_layer_pairs {
        let base = n * layer;
        let next_base = if full_revolution && layer == layers - 1 {
            0 // Wrap around to first layer
        } else {
            n * (layer + 1)
        };
        for i in 0..n {
            let next_i = (i + 1) % n;
            let r_i = profile[i as usize][0].abs();
            let r_next = profile[next_i as usize][0].abs();

            if r_i < 1e-15 && r_next < 1e-15 {
                // Both on axis — skip degenerate quad
                continue;
            } else if r_i < 1e-15 {
                // Only current vertex on axis — single triangle
                m.add_triangle(base + i, base + next_i, next_base + next_i);
            } else if r_next < 1e-15 {
                // Only next vertex on axis — single triangle
                m.add_triangle(base + i, base + next_i, next_base + i);
            } else {
                // Normal quad
                m.add_triangle(base + i, base + next_i, next_base + next_i);
                m.add_triangle(base + i, next_base + next_i, next_base + i);
            }
        }
    }

    // End caps for partial revolution
    if !full_revolution {
        let cap_tris = ear_clip_triangulate(profile);
        // Start cap (layer 0)
        for &[a, b, c] in &cap_tris {
            m.add_triangle(c as u32, b as u32, a as u32);
        }
        // End cap (last layer)
        let end_offset = n * (layers - 1);
        for &[a, b, c] in &cap_tris {
            m.add_triangle(
                end_offset + a as u32,
                end_offset + b as u32,
                end_offset + c as u32,
            );
        }
    }

    m.weld_vertices(1e-12);
    m
}

/// Extrude a 2D polygon along the Y axis.
/// `polygon`: list of 2D points (in XZ plane) forming a closed polygon (CCW).
/// `height`: extrusion height.
/// Uses ear-clipping for non-convex polygon support.
pub fn extrude(polygon: &[[f64; 2]], height: f64) -> TriMesh {
    linear_extrude_advanced(polygon, height, 0.0, 1.0, 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_csg_mesh::{validate_mesh, volume::mesh_volume};

    fn check_manifold(mesh: &TriMesh, name: &str) {
        let report = validate_mesh(mesh);
        assert!(report.is_closed, "{} should be closed: {:?}", name, report);
        assert!(
            report.is_manifold,
            "{} should be manifold: {:?}",
            name, report
        );
        assert!(
            report.is_consistently_oriented,
            "{} should be consistently oriented: {:?}",
            name, report
        );
        assert_eq!(
            report.degenerate_triangles, 0,
            "{} has degenerate triangles",
            name
        );
    }

    #[test]
    fn test_cube_basic() {
        let c = cube(dvec3(1.0, 1.0, 1.0), true);
        assert_eq!(c.vertex_count(), 8);
        assert_eq!(c.triangle_count(), 12);
        check_manifold(&c, "cube");
    }

    #[test]
    fn test_cube_volume() {
        let c = cube(dvec3(2.0, 3.0, 4.0), true);
        let vol = mesh_volume(&c);
        assert!(
            (vol - 24.0).abs() < 1e-10,
            "2x3x4 cube volume should be 24, got {}",
            vol
        );
    }

    #[test]
    fn test_cube_not_centered() {
        let c = cube(dvec3(1.0, 1.0, 1.0), false);
        let bb = c.bounding_box();
        assert!((bb.min.x).abs() < 1e-12);
        assert!((bb.max.x - 1.0).abs() < 1e-12);
    }

    #[test]
    fn test_sphere_basic() {
        let s = sphere(1.0, 16, 8);
        check_manifold(&s, "sphere");
    }

    #[test]
    fn test_sphere_volume() {
        // At 32 segments / 16 rings, sphere volume should be close to 4/3 * pi * r^3
        let s = sphere(1.0, 32, 16);
        let vol = mesh_volume(&s);
        let expected = 4.0 / 3.0 * PI;
        let error = (vol - expected).abs() / expected;
        assert!(
            error < 0.02,
            "sphere volume error too large: {} vs {} ({}%)",
            vol,
            expected,
            error * 100.0
        );
    }

    #[test]
    fn test_sphere_bounding_box() {
        let s = sphere(5.0, 32, 16);
        let bb = s.bounding_box();
        assert!((bb.min.x - (-5.0)).abs() < 0.01);
        assert!((bb.max.x - 5.0).abs() < 0.01);
        assert!((bb.min.y - (-5.0)).abs() < 1e-10);
        assert!((bb.max.y - 5.0).abs() < 1e-10);
    }

    #[test]
    fn test_cylinder_basic() {
        let c = cylinder(1.0, 2.0, 16, true);
        check_manifold(&c, "cylinder");
    }

    #[test]
    fn test_cylinder_volume() {
        let c = cylinder(1.0, 2.0, 64, true);
        let vol = mesh_volume(&c);
        let expected = PI * 1.0 * 1.0 * 2.0;
        let error = (vol - expected).abs() / expected;
        assert!(
            error < 0.01,
            "cylinder volume error: {} vs {} ({}%)",
            vol,
            expected,
            error * 100.0
        );
    }

    #[test]
    fn test_cone_basic() {
        let c = cone(1.0, 2.0, 16, true);
        check_manifold(&c, "cone");
    }

    #[test]
    fn test_cone_volume() {
        let c = cone(1.0, 2.0, 64, true);
        let vol = mesh_volume(&c);
        let expected = PI * 1.0 * 1.0 * 2.0 / 3.0;
        let error = (vol - expected).abs() / expected;
        assert!(
            error < 0.01,
            "cone volume error: {} vs {} ({}%)",
            vol,
            expected,
            error * 100.0
        );
    }

    #[test]
    fn test_torus_basic() {
        let t = torus(2.0, 0.5, 16, 8);
        check_manifold(&t, "torus");
    }

    #[test]
    fn test_torus_volume() {
        let t = torus(2.0, 0.5, 64, 32);
        let vol = mesh_volume(&t);
        let expected = 2.0 * PI * PI * 2.0 * 0.5 * 0.5; // 2 * pi^2 * R * r^2
        let error = (vol - expected).abs() / expected;
        assert!(
            error < 0.01,
            "torus volume error: {} vs {} ({}%)",
            vol,
            expected,
            error * 100.0
        );
    }

    #[test]
    fn test_extrude_square() {
        let sq = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let e = extrude(&sq, 1.0);
        check_manifold(&e, "extruded square");
        let vol = mesh_volume(&e);
        assert!(
            (vol - 1.0).abs() < 1e-10,
            "extruded unit square should have volume 1.0, got {}",
            vol
        );
    }

    #[test]
    fn test_extrude_triangle() {
        let tri = vec![[0.0, 0.0], [1.0, 0.0], [0.5, 1.0]];
        let e = extrude(&tri, 2.0);
        check_manifold(&e, "extruded triangle");
        let vol = mesh_volume(&e);
        let expected = 0.5 * 1.0 * 2.0; // triangle area * height
        assert!(
            (vol - expected).abs() < 1e-10,
            "extruded triangle volume: {} vs {}",
            vol,
            expected
        );
    }

    // --- Tapered cylinder tests ---

    #[test]
    fn test_tapered_cylinder_is_cylinder() {
        // Equal radii = regular cylinder
        let tc = tapered_cylinder(1.0, 1.0, 2.0, 64, true);
        check_manifold(&tc, "tapered_cylinder(equal radii)");
        let vol = mesh_volume(&tc);
        let expected = PI * 1.0 * 1.0 * 2.0;
        let error = (vol - expected).abs() / expected;
        assert!(
            error < 0.01,
            "tapered cylinder (equal) volume error: {}%",
            error * 100.0
        );
    }

    #[test]
    fn test_tapered_cylinder_is_cone() {
        // r2=0 = cone
        let tc = tapered_cylinder(1.0, 0.0, 2.0, 64, true);
        check_manifold(&tc, "tapered_cylinder(cone)");
        let vol = mesh_volume(&tc);
        let expected = PI * 1.0 * 1.0 * 2.0 / 3.0;
        let error = (vol - expected).abs() / expected;
        assert!(
            error < 0.01,
            "tapered cylinder (cone) volume error: {}%",
            error * 100.0
        );
    }

    #[test]
    fn test_tapered_cylinder_frustum() {
        // Frustum: r1=2, r2=1, h=3
        let tc = tapered_cylinder(2.0, 1.0, 3.0, 64, true);
        check_manifold(&tc, "frustum");
        let vol = mesh_volume(&tc);
        // V = (pi*h/3) * (r1^2 + r1*r2 + r2^2)
        let expected = PI * 3.0 / 3.0 * (4.0 + 2.0 + 1.0);
        let error = (vol - expected).abs() / expected;
        assert!(
            error < 0.01,
            "frustum volume error: {}% (got {}, expected {})",
            error * 100.0,
            vol,
            expected
        );
    }

    #[test]
    fn test_tapered_cylinder_inverted_cone() {
        // r1=0, r2=1 = inverted cone
        let tc = tapered_cylinder(0.0, 1.0, 2.0, 64, true);
        check_manifold(&tc, "inverted cone");
        let vol = mesh_volume(&tc);
        let expected = PI * 1.0 * 1.0 * 2.0 / 3.0;
        let error = (vol - expected).abs() / expected;
        assert!(
            error < 0.01,
            "inverted cone volume error: {}%",
            error * 100.0
        );
    }

    // --- Ear clipping tests ---

    #[test]
    fn test_ear_clip_triangle() {
        let tri = vec![[0.0, 0.0], [1.0, 0.0], [0.5, 1.0]];
        let tris = ear_clip_triangulate(&tri);
        assert_eq!(tris.len(), 1);
        assert_eq!(tris[0], [0, 1, 2]);
    }

    #[test]
    fn test_ear_clip_square() {
        let sq = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let tris = ear_clip_triangulate(&sq);
        assert_eq!(tris.len(), 2);
    }

    #[test]
    fn test_ear_clip_concave_l_shape() {
        // L-shaped concave polygon
        let l = vec![
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 1.0],
            [1.0, 1.0],
            [1.0, 2.0],
            [0.0, 2.0],
        ];
        let tris = ear_clip_triangulate(&l);
        assert_eq!(tris.len(), 4); // 6 vertices -> 4 triangles
    }

    #[test]
    fn test_extrude_concave_l_shape() {
        // L-shaped concave polygon extruded should be manifold
        let l = vec![
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 1.0],
            [1.0, 1.0],
            [1.0, 2.0],
            [0.0, 2.0],
        ];
        let e = extrude(&l, 1.0);
        check_manifold(&e, "extruded L-shape");
        let vol = mesh_volume(&e);
        // L-shape area = 2*1 + 1*1 = 3, extruded by 1 = volume 3
        let expected = 3.0;
        assert!(
            (vol - expected).abs() < 1e-10,
            "L-shape extrude volume: {} vs {}",
            vol,
            expected
        );
    }

    // --- Linear extrude advanced tests ---

    #[test]
    fn test_linear_extrude_no_twist_no_scale() {
        // Should be identical to basic extrude
        let sq = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let e = linear_extrude_advanced(&sq, 1.0, 0.0, 1.0, 1);
        check_manifold(&e, "linear_extrude(no twist/scale)");
        let vol = mesh_volume(&e);
        assert!((vol - 1.0).abs() < 1e-10, "volume: {}", vol);
    }

    #[test]
    fn test_linear_extrude_with_twist() {
        let sq = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let e = linear_extrude_advanced(&sq, 2.0, 90.0, 1.0, 16);
        check_manifold(&e, "linear_extrude(twist=90)");
        let vol = mesh_volume(&e);
        // Twist preserves cross-section area, so volume should be same as no twist
        let expected = 1.0 * 2.0; // area * height
        let error = (vol - expected).abs() / expected;
        assert!(
            error < 0.05,
            "twisted extrude volume error: {}% (got {})",
            error * 100.0,
            vol
        );
    }

    #[test]
    fn test_linear_extrude_with_scale() {
        let sq = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        // Scale to 0 at top = pyramid
        let e = linear_extrude_advanced(&sq, 3.0, 0.0, 0.0, 16);
        check_manifold(&e, "linear_extrude(scale=0, pyramid)");
        let vol = mesh_volume(&e);
        // Pyramid volume = base_area * height / 3
        let expected = 1.0 * 3.0 / 3.0;
        let error = (vol - expected).abs() / expected;
        assert!(
            error < 0.05,
            "pyramid extrude volume error: {}% (got {}, expected {})",
            error * 100.0,
            vol,
            expected
        );
    }

    #[test]
    fn test_linear_extrude_half_scale() {
        let sq = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let e = linear_extrude_advanced(&sq, 2.0, 0.0, 0.5, 16);
        check_manifold(&e, "linear_extrude(scale=0.5)");
        let vol = mesh_volume(&e);
        // Frustum: V = h/3 * (A1 + A2 + sqrt(A1*A2))
        // A1 = 1.0, A2 = 0.25, h = 2.0
        let expected = 2.0 / 3.0 * (1.0 + 0.25 + (1.0 * 0.25_f64).sqrt());
        let error = (vol - expected).abs() / expected;
        assert!(
            error < 0.05,
            "half-scale extrude volume error: {}% (got {}, expected {})",
            error * 100.0,
            vol,
            expected
        );
    }

    // --- Rotate extrude tests ---

    #[test]
    fn test_rotate_extrude_torus() {
        // Revolving a circle profile should approximate a torus
        let n = 16;
        let r = 0.5; // minor radius
        let center_r = 2.0; // major radius
        let mut profile: Vec<[f64; 2]> = Vec::new();
        for i in 0..n {
            let angle = 2.0 * PI * i as f64 / n as f64;
            let x = center_r + r * angle.cos();
            let y = r * angle.sin();
            profile.push([x, y]);
        }
        let t = rotate_extrude(&profile, 360.0, 32);
        check_manifold(&t, "rotate_extrude(torus)");
        let vol = mesh_volume(&t);
        let expected = 2.0 * PI * PI * center_r * r * r;
        let error = (vol - expected).abs() / expected;
        assert!(
            error < 0.1,
            "revolved torus volume error: {}% (got {}, expected {})",
            error * 100.0,
            vol,
            expected
        );
    }

    #[test]
    fn test_rotate_extrude_cylinder() {
        // Revolving a rectangle around Y axis = hollow cylinder (annulus)
        // But if profile starts at axis: solid cylinder
        let profile = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 2.0], [0.0, 2.0]];
        let c = rotate_extrude(&profile, 360.0, 64);
        check_manifold(&c, "rotate_extrude(cylinder)");
        let vol = mesh_volume(&c);
        let expected = PI * 1.0 * 1.0 * 2.0;
        let error = (vol - expected).abs() / expected;
        assert!(
            error < 0.02,
            "revolved cylinder volume error: {}% (got {}, expected {})",
            error * 100.0,
            vol,
            expected
        );
    }
}
