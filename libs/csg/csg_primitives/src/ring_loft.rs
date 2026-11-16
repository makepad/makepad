use makepad_csg_math::Vec3d;
use makepad_csg_mesh::mesh::TriMesh;

/// A planar ring, counter-clockwise and star-shaped about its origin in its (u, v) frame.
#[derive(Clone, Debug, PartialEq)]
pub struct LoftRing {
    pub origin: Vec3d,
    pub u: Vec3d,
    pub v: Vec3d,
    pub points: Vec<[f64; 2]>,
    /// A sharp side edge at this ring for exact back ends; the mesh is unchanged.
    pub crease: bool,
}

/// Skin equal-sized rings with triangle pairs and flat caps fanned from their origins.
/// `prism` and ring creases describe exact surfaces; they do not change this mesh.
pub fn ring_loft(rings: &[LoftRing], _prism: bool) -> Result<TriMesh, String> {
    if rings.len() < 2 {
        return Err("a ring loft needs at least 2 rings".into());
    }
    let n = rings[0].points.len();
    let w = rings[0].u.cross(rings[0].v);
    for (k, ring) in rings.iter().enumerate() {
        if ring.points.len() < 3 {
            return Err("a ring loft needs at least 3 points per ring".into());
        }
        if ring.points.len() != n {
            return Err("ring loft rings have different point counts".into());
        }
        if !((ring.u.length() - 1.0).abs() <= 1e-9) {
            return Err(format!("ring loft ring {k} needs unit-length u within 1e-9"));
        }
        if !((ring.v.length() - 1.0).abs() <= 1e-9) {
            return Err(format!("ring loft ring {k} needs unit-length v within 1e-9"));
        }
        if !(ring.u.dot(ring.v).abs() <= 1e-9) {
            return Err(format!("ring loft ring {k} needs orthogonal u and v within 1e-9"));
        }
        if !ring.origin.is_finite() || ring.points.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
            return Err(format!("ring loft ring {k} needs finite origin and points"));
        }
        if k > 0 && !((ring.origin - rings[k - 1].origin).dot(w) > 0.0) {
            return Err("ring loft origins are not strictly increasing along the first ring normal".into());
        }
    }

    let mut mesh = TriMesh::with_capacity(rings.len() * n + 2, 2 * rings.len() * n);
    for ring in rings {
        for p in &ring.points {
            let point = ring.origin + ring.u * p[0] + ring.v * p[1];
            if !point.is_finite() {
                return Err("ring loft world points must be finite".into());
            }
            mesh.add_vertex(point);
        }
    }
    let c0 = mesh.add_vertex(rings[0].origin);
    let c1 = mesh.add_vertex(rings[rings.len() - 1].origin);
    for k in 0..rings.len() - 1 {
        for i in 0..n {
            let j = (i + 1) % n;
            let (a, b, c, d) = ((k * n + i) as u32, (k * n + j) as u32, ((k + 1) * n + j) as u32, ((k + 1) * n + i) as u32);
            mesh.add_triangle(a, b, c);
            mesh.add_triangle(a, c, d);
        }
    }
    let top = ((rings.len() - 1) * n) as u32;
    for i in 0..n as u32 {
        let j = (i + 1) % n as u32;
        mesh.add_triangle(c0, j, i);
        mesh.add_triangle(c1, top + i, top + j);
    }
    Ok(mesh)
}
