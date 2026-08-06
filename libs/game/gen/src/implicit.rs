//! Implicit surfaces (metaballs) polygonised with surface nets.
//!
//! Surface nets rather than marching cubes on purpose: marching cubes places
//! vertices *on* cell edges and routinely emits slivers, which is exactly the
//! wrong output for a low-poly target. Surface nets place ONE vertex per cell,
//! at the average of that cell's edge crossings, and connect neighbours — the
//! result is quad-dominant with well-shaped triangles at a fraction of the
//! count.
//!
//! Primitives blend with a polynomial smooth-minimum, the same smooth-union
//! idea the shadow work uses, so a pile of spheres reads as one organic body
//! instead of visibly separate balls.

use crate::mesh::{GenMesh, GenVertex, MeshBuilder};
use crate::rng::GenRng;
use makepad_game_math as gm;
use makepad_math::*;

/// One field primitive. Distances are signed: negative inside.
#[derive(Clone, Copy, Debug)]
pub enum Primitive {
    Sphere { center: Vec3f, radius: f32 },
    Capsule { a: Vec3f, b: Vec3f, radius: f32 },
    /// Rounded box: `half` extents, `round` added as a corner radius.
    Box { center: Vec3f, half: Vec3f, round: f32 },
}

impl Primitive {
    pub fn distance(&self, p: Vec3f) -> f32 {
        match *self {
            Primitive::Sphere { center, radius } => {
                let d = vec3f(p.x - center.x, p.y - center.y, p.z - center.z);
                (d.x * d.x + d.y * d.y + d.z * d.z).sqrt() - radius
            }
            Primitive::Capsule { a, b, radius } => {
                let ab = vec3f(b.x - a.x, b.y - a.y, b.z - a.z);
                let ap = vec3f(p.x - a.x, p.y - a.y, p.z - a.z);
                let denom = ab.x * ab.x + ab.y * ab.y + ab.z * ab.z;
                let t = if denom > 1.0e-8 {
                    ((ap.x * ab.x + ap.y * ab.y + ap.z * ab.z) / denom).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let c = vec3f(a.x + ab.x * t, a.y + ab.y * t, a.z + ab.z * t);
                let d = vec3f(p.x - c.x, p.y - c.y, p.z - c.z);
                (d.x * d.x + d.y * d.y + d.z * d.z).sqrt() - radius
            }
            Primitive::Box {
                center,
                half,
                round,
            } => {
                let q = vec3f(
                    (p.x - center.x).abs() - half.x + round,
                    (p.y - center.y).abs() - half.y + round,
                    (p.z - center.z).abs() - half.z + round,
                );
                let outside = vec3f(q.x.max(0.0), q.y.max(0.0), q.z.max(0.0));
                let len =
                    (outside.x * outside.x + outside.y * outside.y + outside.z * outside.z).sqrt();
                len + q.x.max(q.y).max(q.z).min(0.0) - round
            }
        }
    }
}

/// Polynomial smooth minimum. `k` is the blend width in world units.
fn smin(a: f32, b: f32, k: f32) -> f32 {
    if k <= 1.0e-6 {
        return a.min(b);
    }
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    // The quadratic term is what rounds the join instead of creasing it.
    a * h + b * (1.0 - h) - k * h * (1.0 - h)
}

/// A blended field of primitives.
#[derive(Clone, Debug, Default)]
pub struct Field {
    pub parts: Vec<Primitive>,
    /// Smooth-union blend width.
    pub blend: f32,
}

impl Field {
    pub fn distance(&self, p: Vec3f) -> f32 {
        let mut d = f32::INFINITY;
        for part in &self.parts {
            let pd = part.distance(p);
            d = if d.is_finite() {
                smin(d, pd, self.blend)
            } else {
                pd
            };
        }
        d
    }

    /// Bounds that comfortably contain the surface, for grid sizing.
    pub fn bounds(&self) -> (Vec3f, Vec3f) {
        let mut min = vec3f(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut max = vec3f(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        let mut grow = |c: Vec3f, r: f32| {
            min.x = min.x.min(c.x - r);
            min.y = min.y.min(c.y - r);
            min.z = min.z.min(c.z - r);
            max.x = max.x.max(c.x + r);
            max.y = max.y.max(c.y + r);
            max.z = max.z.max(c.z + r);
        };
        for part in &self.parts {
            match *part {
                Primitive::Sphere { center, radius } => grow(center, radius),
                Primitive::Capsule { a, b, radius } => {
                    grow(a, radius);
                    grow(b, radius);
                }
                Primitive::Box {
                    center,
                    half,
                    round,
                } => grow(center, half.x.max(half.y).max(half.z) + round),
            }
        }
        if !min.x.is_finite() {
            return (Vec3f::default(), Vec3f::default());
        }
        // The blend pushes the surface outward, so pad by it.
        let pad = self.blend + 1.0e-3;
        (
            vec3f(min.x - pad, min.y - pad, min.z - pad),
            vec3f(max.x + pad, max.y + pad, max.z + pad),
        )
    }
}

/// Polygonise a field with surface nets at the given grid resolution
/// (cells along the longest axis).
pub fn surface_net(field: &Field, resolution: usize, color: [f32; 3]) -> GenMesh {
    let mut b = MeshBuilder::new();
    if field.parts.is_empty() {
        return b.finish();
    }
    let res = resolution.clamp(4, 96);
    let (min, max) = field.bounds();
    let span = vec3f(max.x - min.x, max.y - min.y, max.z - min.z);
    let longest = span.x.max(span.y).max(span.z).max(1.0e-4);
    let cell = longest / res as f32;
    let nx = (span.x / cell).ceil() as usize + 2;
    let ny = (span.y / cell).ceil() as usize + 2;
    let nz = (span.z / cell).ceil() as usize + 2;
    // Cap total work: a caller asking for a huge blend across a huge bound
    // should get a coarse mesh, not a multi-second stall.
    if nx * ny * nz > 2_000_000 {
        return b.finish();
    }

    let corner = |ix: usize, iy: usize, iz: usize| -> Vec3f {
        vec3f(
            min.x + (ix as f32 - 0.5) * cell,
            min.y + (iy as f32 - 0.5) * cell,
            min.z + (iz as f32 - 0.5) * cell,
        )
    };

    // Sample the field at every grid corner once.
    let mut sample = vec![0.0f32; (nx + 1) * (ny + 1) * (nz + 1)];
    let sidx = |x: usize, y: usize, z: usize| (z * (ny + 1) + y) * (nx + 1) + x;
    for z in 0..=nz {
        for y in 0..=ny {
            for x in 0..=nx {
                sample[sidx(x, y, z)] = field.distance(corner(x, y, z));
            }
        }
    }

    // One vertex per cell that straddles the surface.
    let mut cell_vertex = vec![u32::MAX; nx * ny * nz];
    let cidx = |x: usize, y: usize, z: usize| (z * ny + y) * nx + x;
    // The 12 edges of a cell, as pairs of corner offsets.
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    const OFFS: [(usize, usize, usize); 8] = [
        (0, 0, 0),
        (1, 0, 0),
        (0, 1, 0),
        (1, 1, 0),
        (0, 0, 1),
        (1, 0, 1),
        (0, 1, 1),
        (1, 1, 1),
    ];

    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let mut d = [0.0f32; 8];
                let mut neg = 0;
                for (i, (ox, oy, oz)) in OFFS.iter().enumerate() {
                    d[i] = sample[sidx(x + ox, y + oy, z + oz)];
                    if d[i] < 0.0 {
                        neg += 1;
                    }
                }
                if neg == 0 || neg == 8 {
                    continue;
                }
                // Average the zero crossings on this cell's edges — this is
                // what puts the vertex near the true surface rather than at
                // the cell centre, and what makes the mesh look smooth at low
                // resolution.
                let mut acc = Vec3f::default();
                let mut count = 0.0f32;
                for (a, c) in EDGES {
                    let (da, dc) = (d[a], d[c]);
                    if (da < 0.0) == (dc < 0.0) {
                        continue;
                    }
                    let t = if (dc - da).abs() > 1.0e-8 {
                        da / (da - dc)
                    } else {
                        0.5
                    };
                    let (ax, ay, az) = OFFS[a];
                    let (cx, cy, cz) = OFFS[c];
                    let pa = corner(x + ax, y + ay, z + az);
                    let pc = corner(x + cx, y + cy, z + cz);
                    acc.x += pa.x + (pc.x - pa.x) * t;
                    acc.y += pa.y + (pc.y - pa.y) * t;
                    acc.z += pa.z + (pc.z - pa.z) * t;
                    count += 1.0;
                }
                if count == 0.0 {
                    continue;
                }
                let pos = vec3f(acc.x / count, acc.y / count, acc.z / count);
                let idx = b.vertex(GenVertex {
                    pos,
                    normal: vec3f(0.0, 1.0, 0.0),
                    uv: [pos.x * 0.25, pos.z * 0.25],
                    color,
                    growth: 1.0,
                    flex: 0.0,
                });
                cell_vertex[cidx(x, y, z)] = idx;
            }
        }
    }

    // Each grid edge with a sign change becomes a quad joining the four cells
    // around it. Winding follows the sign so faces point outward.
    let mut quad = |a: u32, bb: u32, c: u32, dd: u32, flip: bool| {
        if a == u32::MAX || bb == u32::MAX || c == u32::MAX || dd == u32::MAX {
            return;
        }
        if flip {
            b.quad(a, bb, c, dd);
        } else {
            b.quad(dd, c, bb, a);
        }
    };
    for z in 1..nz {
        for y in 1..ny {
            for x in 1..nx {
                let d0 = sample[sidx(x, y, z)];
                // +X edge
                if x < nx {
                    let d1 = sample[sidx(x + 1, y, z)];
                    if (d0 < 0.0) != (d1 < 0.0) {
                        quad(
                            cell_vertex[cidx(x, y - 1, z - 1)],
                            cell_vertex[cidx(x, y, z - 1)],
                            cell_vertex[cidx(x, y, z)],
                            cell_vertex[cidx(x, y - 1, z)],
                            d0 < 0.0,
                        );
                    }
                }
                // +Y edge
                if y < ny {
                    let d1 = sample[sidx(x, y + 1, z)];
                    if (d0 < 0.0) != (d1 < 0.0) {
                        quad(
                            cell_vertex[cidx(x - 1, y, z - 1)],
                            cell_vertex[cidx(x - 1, y, z)],
                            cell_vertex[cidx(x, y, z)],
                            cell_vertex[cidx(x, y, z - 1)],
                            d0 < 0.0,
                        );
                    }
                }
                // +Z edge
                if z < nz {
                    let d1 = sample[sidx(x, y, z + 1)];
                    if (d0 < 0.0) != (d1 < 0.0) {
                        quad(
                            cell_vertex[cidx(x - 1, y - 1, z)],
                            cell_vertex[cidx(x, y - 1, z)],
                            cell_vertex[cidx(x, y, z)],
                            cell_vertex[cidx(x - 1, y, z)],
                            d0 < 0.0,
                        );
                    }
                }
            }
        }
    }

    b.recompute_normals();
    b.bake_ambient(0.4, 0.65);
    b.finish()
}

/// Knobs for the blob presets.
#[derive(Clone, Copy, Debug)]
pub struct BlobParams {
    pub seed: u64,
    pub size: f32,
    /// Grid cells along the longest axis. Low = chunky, high = smooth.
    pub resolution: usize,
    pub color: [f32; 3],
}

impl Default for BlobParams {
    fn default() -> Self {
        Self {
            seed: 0,
            size: 1.0,
            resolution: 16,
            color: [0.55, 0.53, 0.50],
        }
    }
}

/// Names the `kind:` knob accepts.
pub const BLOBS: &[&str] = &["rock", "boulder", "mushroom", "cloud", "blob"];

/// Build one of the blob presets. Deterministic in `params.seed`.
pub fn blob(kind: &str, params: BlobParams) -> GenMesh {
    let mut rng = GenRng::new(params.seed);
    let s = params.size.max(0.05);
    let mut f = Field {
        parts: Vec::new(),
        blend: s * 0.35,
    };
    let mut color = params.color;

    match kind {
        "boulder" => {
            f.blend = s * 0.5;
            for _ in 0..5 {
                f.parts.push(Primitive::Sphere {
                    center: vec3f(
                        rng.jitter(s * 0.45),
                        rng.range(0.1, 0.7) * s,
                        rng.jitter(s * 0.45),
                    ),
                    radius: rng.range(0.42, 0.68) * s,
                });
            }
        }
        "mushroom" => {
            color = [0.86, 0.82, 0.70];
            f.blend = s * 0.18;
            f.parts.push(Primitive::Capsule {
                a: vec3f(0.0, 0.0, 0.0),
                b: vec3f(0.0, s * 0.75, 0.0),
                radius: s * 0.17,
            });
            f.parts.push(Primitive::Box {
                center: vec3f(0.0, s * 0.85, 0.0),
                half: vec3f(s * 0.5, s * 0.12, s * 0.5),
                round: s * 0.28,
            });
        }
        "cloud" => {
            color = [0.94, 0.95, 0.98];
            f.blend = s * 0.55;
            for i in 0..6 {
                f.parts.push(Primitive::Sphere {
                    center: vec3f(
                        (i as f32 - 2.5) * s * 0.42 + rng.jitter(s * 0.12),
                        rng.jitter(s * 0.16),
                        rng.jitter(s * 0.22),
                    ),
                    radius: rng.range(0.34, 0.56) * s,
                });
            }
        }
        "blob" => {
            color = [0.35, 0.75, 0.45];
            f.blend = s * 0.6;
            for _ in 0..3 {
                f.parts.push(Primitive::Sphere {
                    center: vec3f(rng.jitter(s * 0.3), rng.range(0.2, 0.6) * s, rng.jitter(s * 0.3)),
                    radius: rng.range(0.45, 0.62) * s,
                });
            }
        }
        // "rock" and anything unrecognised: a squat, chipped stone.
        _ => {
            f.blend = s * 0.3;
            f.parts.push(Primitive::Box {
                center: vec3f(0.0, s * 0.32, 0.0),
                half: vec3f(s * 0.42, s * 0.28, s * 0.38),
                round: s * 0.14,
            });
            for _ in 0..3 {
                f.parts.push(Primitive::Sphere {
                    center: vec3f(
                        rng.jitter(s * 0.32),
                        rng.range(0.15, 0.5) * s,
                        rng.jitter(s * 0.32),
                    ),
                    radius: rng.range(0.24, 0.4) * s,
                });
            }
        }
    }

    // Rotate the whole field by a seeded yaw so instances of one preset do
    // not all present the same profile.
    let yaw = rng.range(0.0, 6.283_185_3);
    let (sy, cy) = gm::sincos(yaw);
    let rot = |v: Vec3f| vec3f(v.x * cy - v.z * sy, v.y, v.x * sy + v.z * cy);
    for part in &mut f.parts {
        match part {
            Primitive::Sphere { center, .. } => *center = rot(*center),
            Primitive::Capsule { a, b, .. } => {
                *a = rot(*a);
                *b = rot(*b);
            }
            Primitive::Box { center, .. } => *center = rot(*center),
        }
    }

    surface_net(&f, params.resolution, color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sphere_distance_is_signed_correctly() {
        let s = Primitive::Sphere {
            center: Vec3f::default(),
            radius: 2.0,
        };
        assert!((s.distance(vec3f(0.0, 0.0, 0.0)) + 2.0).abs() < 1.0e-5);
        assert!(s.distance(vec3f(2.0, 0.0, 0.0)).abs() < 1.0e-5);
        assert!(s.distance(vec3f(5.0, 0.0, 0.0)) > 0.0);
    }

    #[test]
    fn smin_is_smoother_than_min_but_bounded_by_it() {
        // The blend may only pull the surface outward, never push it in past
        // the true minimum by more than k/4 (the polynomial's maximum dip).
        for i in 0..50 {
            let a = i as f32 * 0.1 - 2.5;
            let b = 1.0;
            let k = 0.5;
            let s = smin(a, b, k);
            assert!(s <= a.min(b) + 1.0e-5);
            assert!(s >= a.min(b) - k * 0.25 - 1.0e-5);
        }
    }

    #[test]
    fn a_single_sphere_polygonises_near_its_radius() {
        let f = Field {
            parts: vec![Primitive::Sphere {
                center: Vec3f::default(),
                radius: 1.0,
            }],
            blend: 0.0,
        };
        let m = surface_net(&f, 24, [1.0, 1.0, 1.0]);
        assert!(m.triangle_count() > 100, "too coarse: {}", m.triangle_count());
        // Every vertex should sit close to the sphere's surface.
        for v in m.vertices.chunks_exact(crate::mesh::MESH_VERTEX_FLOATS) {
            let r = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            assert!((r - 1.0).abs() < 0.15, "vertex at radius {r}");
        }
    }

    #[test]
    fn output_stays_inside_the_field_bounds() {
        let f = Field {
            parts: vec![
                Primitive::Sphere {
                    center: vec3f(-1.0, 0.0, 0.0),
                    radius: 1.0,
                },
                Primitive::Sphere {
                    center: vec3f(1.0, 0.0, 0.0),
                    radius: 1.0,
                },
            ],
            blend: 0.4,
        };
        let (min, max) = f.bounds();
        let m = surface_net(&f, 20, [1.0, 1.0, 1.0]);
        assert!(m.triangle_count() > 0);
        for v in m.vertices.chunks_exact(crate::mesh::MESH_VERTEX_FLOATS) {
            assert!(v[0] >= min.x - 1.0e-3 && v[0] <= max.x + 1.0e-3);
            assert!(v[1] >= min.y - 1.0e-3 && v[1] <= max.y + 1.0e-3);
            assert!(v[2] >= min.z - 1.0e-3 && v[2] <= max.z + 1.0e-3);
        }
    }

    #[test]
    fn every_blob_preset_builds() {
        for kind in BLOBS {
            let m = blob(kind, BlobParams::default());
            assert!(m.triangle_count() > 0, "{kind} produced nothing");
            for f in &m.vertices {
                assert!(f.is_finite(), "{kind} emitted non-finite float");
            }
            for i in &m.indices {
                assert!((*i as usize) < m.vertex_count(), "{kind} index out of range");
            }
        }
    }

    #[test]
    fn blobs_are_seed_deterministic() {
        let a = blob("rock", BlobParams { seed: 3, ..Default::default() });
        let b = blob("rock", BlobParams { seed: 3, ..Default::default() });
        assert_eq!(a.vertices, b.vertices);
        let c = blob("rock", BlobParams { seed: 4, ..Default::default() });
        assert_ne!(a.vertices, c.vertices, "seed had no effect");
    }

    #[test]
    fn resolution_controls_density() {
        let lo = blob("boulder", BlobParams { seed: 1, resolution: 8, ..Default::default() })
            .triangle_count();
        let hi = blob("boulder", BlobParams { seed: 1, resolution: 24, ..Default::default() })
            .triangle_count();
        assert!(hi > lo, "resolution ignored: {lo} vs {hi}");
    }

    #[test]
    fn an_empty_field_yields_an_empty_mesh_rather_than_panicking() {
        let m = surface_net(&Field::default(), 16, [1.0, 1.0, 1.0]);
        assert_eq!(m.triangle_count(), 0);
    }
}
