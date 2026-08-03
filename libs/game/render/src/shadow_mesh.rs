//! Real silhouette shadows, without a shadow map, a stencil buffer or a
//! second pass.
//!
//! The flat oriented quad in [`crate::shadow`] can only ever be a rectangle,
//! so a person and a car cast the same smear. This builds the caster's actual
//! outline instead:
//!
//! 1. Project the caster's own points along the sun onto the receiver.
//! 2. Take the 2D convex hull of those points in the ground plane.
//! 3. Fan-triangulate the hull.
//!
//! Step 3 is why this is an alpha-blend-safe approach: a hull fan has **no
//! self-overlap**, so every interior pixel is covered exactly once and the
//! shadow has one uniform alpha. Projecting the caster's triangles directly
//! would overlap them and each overlap would darken again, turning a solid
//! object into a blotchy stain.
//!
//! Draping and z-fighting are handled structurally rather than by fudge
//! factors — see [`Receiver`] and [`SHADOW_NORMAL_BIAS`].

use makepad_draw::*;
use makepad_game_sim::{Shape, Terrain};

use crate::shadow::{BASE_SHADOW_ALPHA, MAX_SHADOW_DROP};
use crate::sun::GameSun;

/// Lift along the RECEIVER's normal, not world up. On a slope, lifting
/// straight up moves the shadow along the surface as well as off it, which
/// both misplaces it and leaves the downhill edge buried. Along the normal it
/// stays put and clears evenly.
pub const SHADOW_NORMAL_BIAS: f32 = 0.012;
/// Extra lift proportional to how steep the receiver is under this vertex.
/// A steep triangle covers more depth range per pixel, so it needs more
/// clearance for the same on-screen result — the standard slope-scaled bias,
/// applied on the CPU because we are placing the geometry ourselves.
pub const SHADOW_SLOPE_BIAS: f32 = 0.05;
/// Hull edges longer than this are split so a draped shadow follows the
/// ground instead of cutting a chord through a hill.
const MAX_EDGE_LEN: f32 = 0.75;
/// Ceiling on the subdivision, so a huge shadow on rough ground cannot
/// explode the vertex count.
const MAX_HULL_POINTS: usize = 96;

/// What a shadow falls on. Sampling this rather than assuming a plane is what
/// lets a shadow bend over a hill.
#[derive(Clone, Copy)]
pub struct Receiver<'a> {
    /// Flat fallback: the terrain floor, or the top of the static the caster
    /// stands on.
    pub base_y: f32,
    /// Present when the caster stands over the heightfield.
    pub terrain: Option<&'a Terrain>,
}

impl Receiver<'_> {
    /// Surface height and normal under a world x/z. Falls back to the flat
    /// plane wherever the terrain is absent or below it (a box top over a
    /// valley receives on the box, not the valley floor).
    pub fn sample(&self, x: f32, z: f32) -> (f32, Vec3f) {
        if let Some(t) = self.terrain {
            if let Some(h) = t.height_at(x, z) {
                if h >= self.base_y - 1.0e-3 {
                    let n = t.normal_at(x, z).unwrap_or(vec3f(0.0, 1.0, 0.0));
                    return (h, n);
                }
            }
        }
        (self.base_y, vec3f(0.0, 1.0, 0.0))
    }

    /// Is this receiver flat everywhere the shadow could land? Enables the
    /// no-subdivision fast path.
    pub fn is_flat(&self) -> bool {
        self.terrain.is_none()
    }
}

/// Accumulates every caster's shadow triangles into ONE mesh, so all the
/// shadows in a frame cost a single draw call rather than one each.
#[derive(Default)]
pub struct ShadowMeshBuilder {
    /// PbrVertex layout (16 floats): position in `pos_nx.xyz`, per-vertex
    /// alpha in `color.w`. Premultiplied black, so RGB stays 0.
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
}

impl ShadowMeshBuilder {
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Copy another mesh in, re-basing its indices. Used to splice the cached
    /// static shadows into the per-frame mesh so everything still ships as
    /// one geometry and one draw call.
    pub fn append(&mut self, other: &ShadowMeshBuilder) {
        let base = (self.vertices.len() / 16) as u32;
        self.vertices.extend_from_slice(&other.vertices);
        self.indices
            .extend(other.indices.iter().map(|i| i + base));
    }

    fn push_vertex(&mut self, p: Vec3f, alpha: f32) -> u32 {
        let index = (self.vertices.len() / 16) as u32;
        self.vertices.extend_from_slice(&[
            p.x, p.y, p.z, 0.0, // pos + normal.x (unused)
            1.0, 0.0, 0.0, 0.0, // normal.yz + uv
            0.0, 0.0, 0.0, alpha, // colour: premultiplied black
            1.0, 0.0, 0.0, 1.0, // tangent
        ]);
        index
    }

    fn push_tri(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend_from_slice(&[a, b, c]);
    }
}

/// Local-space silhouette points for a primitive shape, spanning
/// [-0.5, 0.5] like the unit geometries. Enough points to read as the shape;
/// the hull discards the interior ones for free.
fn shape_points(shape: Shape) -> Vec<Vec3f> {
    let ring = |y: f32, r: f32, n: usize| -> Vec<Vec3f> {
        (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                vec3f(a.cos() * r, y, a.sin() * r)
            })
            .collect()
    };
    match shape {
        Shape::Box => {
            let mut p = Vec::with_capacity(8);
            for sx in [-0.5f32, 0.5] {
                for sy in [-0.5f32, 0.5] {
                    for sz in [-0.5f32, 0.5] {
                        p.push(vec3f(sx, sy, sz));
                    }
                }
            }
            p
        }
        // A sphere's outline is a circle from every angle; three rings
        // capture that under any sun without needing a view-dependent rim.
        Shape::Sphere => {
            let mut p = ring(0.0, 0.5, 10);
            p.extend(ring(0.35, 0.36, 8));
            p.extend(ring(-0.35, 0.36, 8));
            p.push(vec3f(0.0, 0.5, 0.0));
            p.push(vec3f(0.0, -0.5, 0.0));
            p
        }
        Shape::Cylinder => {
            let mut p = ring(0.5, 0.5, 10);
            p.extend(ring(-0.5, 0.5, 10));
            p
        }
        Shape::Cone => {
            let mut p = ring(-0.5, 0.5, 10);
            p.push(vec3f(0.0, 0.5, 0.0));
            p
        }
        // Ramp: the box with its top edge collapsed to one side.
        Shape::Wedge => vec![
            vec3f(-0.5, -0.5, -0.5),
            vec3f(0.5, -0.5, -0.5),
            vec3f(0.5, -0.5, 0.5),
            vec3f(-0.5, -0.5, 0.5),
            vec3f(-0.5, 0.5, -0.5),
            vec3f(-0.5, 0.5, 0.5),
        ],
    }
}

/// World-space silhouette points for a primitive caster.
pub fn caster_points(shape: Shape, transform: &Mat4f, size: Vec3f, out: &mut Vec<Vec3f>) {
    out.clear();
    for p in shape_points(shape) {
        let l = vec3f(p.x * size.x, p.y * size.y, p.z * size.z);
        out.push(vec3f(
            transform.v[0] * l.x + transform.v[4] * l.y + transform.v[8] * l.z + transform.v[12],
            transform.v[1] * l.x + transform.v[5] * l.y + transform.v[9] * l.z + transform.v[13],
            transform.v[2] * l.x + transform.v[6] * l.y + transform.v[10] * l.z + transform.v[14],
        ));
    }
}

/// A shadow proxy for a skinned character: every `stride`-th posed vertex.
///
/// Deliberately NOT the full mesh — projecting 3716 vertices per character
/// per frame would cost more than the shadow is worth, and the hull only
/// keeps the outline anyway. Because the samples come from the *posed*
/// vertices, the silhouette walks when the character walks.
pub fn skinned_proxy_points(pbr_vertices: &[f32], transform: &Mat4f, out: &mut Vec<Vec3f>) {
    out.clear();
    let count = pbr_vertices.len() / 16;
    if count == 0 {
        return;
    }
    // Aim for ~64 samples whatever the mesh size.
    let stride = (count / 64).max(1);
    let mut i = 0;
    while i < count {
        let b = i * 16;
        let l = vec3f(pbr_vertices[b], pbr_vertices[b + 1], pbr_vertices[b + 2]);
        out.push(vec3f(
            transform.v[0] * l.x + transform.v[4] * l.y + transform.v[8] * l.z + transform.v[12],
            transform.v[1] * l.x + transform.v[5] * l.y + transform.v[9] * l.z + transform.v[13],
            transform.v[2] * l.x + transform.v[6] * l.y + transform.v[10] * l.z + transform.v[14],
        ));
        i += stride;
    }
}

/// Andrew's monotone chain, on the ground plane. Returns CCW hull points.
fn convex_hull(mut pts: Vec<Vec2f>) -> Vec<Vec2f> {
    if pts.len() < 3 {
        return pts;
    }
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup_by(|a, b| (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6);
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: Vec2f, a: Vec2f, b: Vec2f| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut hull: Vec<Vec2f> = Vec::with_capacity(pts.len() * 2);
    for &p in pts.iter() {
        while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }
    let lower = hull.len() + 1;
    for &p in pts.iter().rev() {
        while hull.len() >= lower && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }
    hull.pop();
    hull
}

/// Split hull edges longer than `MAX_EDGE_LEN` so draping follows the ground
/// rather than cutting through it. Skipped entirely on flat receivers.
fn subdivide(hull: &[Vec2f]) -> Vec<Vec2f> {
    let mut out = Vec::with_capacity(hull.len() * 2);
    for i in 0..hull.len() {
        let a = hull[i];
        let b = hull[(i + 1) % hull.len()];
        out.push(a);
        let d = vec2f(b.x - a.x, b.y - a.y);
        let len = (d.x * d.x + d.y * d.y).sqrt();
        let splits = ((len / MAX_EDGE_LEN).floor() as usize).min(8);
        for s in 1..splits {
            let t = s as f32 / splits as f32;
            out.push(vec2f(a.x + d.x * t, a.y + d.y * t));
            if out.len() >= MAX_HULL_POINTS {
                return out;
            }
        }
    }
    out
}

/// Build one caster's shadow into the shared mesh. Returns false when the
/// caster casts nothing (too high, below the receiver, degenerate hull).
///
/// `drop` is the caster's height above the receiver and drives both the fade
/// and the penumbra width: a resting box gets a tight dark shadow, a jumping
/// character a wide faint one, which is the cue that reads as "off the
/// ground" without costing anything to compute.
pub fn build_caster_shadow(
    points: &[Vec3f],
    sun: &GameSun,
    receiver: &Receiver,
    out: &mut ShadowMeshBuilder,
) -> bool {
    if points.len() < 3 {
        return false;
    }
    let mut lowest = f32::MAX;
    for p in points {
        lowest = lowest.min(p.y);
    }
    let drop = lowest - receiver.base_y;
    if !(-0.5..MAX_SHADOW_DROP).contains(&drop) {
        return false;
    }
    let fade = (1.0 - (drop.max(0.0) / MAX_SHADOW_DROP)).clamp(0.0, 1.0);
    let alpha = fade * BASE_SHADOW_ALPHA * (sun.shadow_alpha / 0.35);
    if alpha <= 0.002 {
        return false;
    }

    // Project along the sun onto the receiver's base plane. A point at height
    // h lands h * len_per_unit away, opposite the sun.
    let len_per_unit = sun.shadow_len_per_unit();
    let g = sun.dir_ground();
    let flat: Vec<Vec2f> = points
        .iter()
        .map(|p| {
            let h = (p.y - receiver.base_y).max(0.0);
            vec2f(p.x - h * len_per_unit * g.x, p.z - h * len_per_unit * g.y)
        })
        .collect();

    let hull = convex_hull(flat);
    if hull.len() < 3 {
        return false;
    }
    let hull = if receiver.is_flat() {
        hull
    } else {
        subdivide(&hull)
    };

    let n = hull.len();
    let inv = 1.0 / n as f32;
    let centroid = hull
        .iter()
        .fold(vec2f(0.0, 0.0), |a, p| vec2f(a.x + p.x, a.y + p.y));
    let centroid = vec2f(centroid.x * inv, centroid.y * inv);

    // Penumbra: the soft rim widens as the caster rises. One lerp, no cost.
    let penumbra = (0.12 + 0.55 * (drop.max(0.0) / MAX_SHADOW_DROP)).clamp(0.12, 0.7);

    // Place a vertex on the receiver surface, lifted along ITS normal.
    let place = |x: f32, z: f32, a: f32, out: &mut ShadowMeshBuilder| -> u32 {
        let (y, normal) = receiver.sample(x, z);
        // Slope-scaled: flat ground needs the base bias, a steep face needs
        // more because it spans more depth per pixel.
        let slope = (1.0 - normal.y.clamp(0.0, 1.0)).clamp(0.0, 1.0);
        let lift = SHADOW_NORMAL_BIAS + SHADOW_SLOPE_BIAS * slope;
        out.push_vertex(
            vec3f(x + normal.x * lift, y + normal.y * lift, z + normal.z * lift),
            a,
        )
    };

    let c = place(centroid.x, centroid.y, alpha, out);
    let mut inner = Vec::with_capacity(n);
    let mut outer = Vec::with_capacity(n);
    for p in hull.iter() {
        let ix = centroid.x + (p.x - centroid.x) * (1.0 - penumbra);
        let iz = centroid.y + (p.y - centroid.y) * (1.0 - penumbra);
        inner.push(place(ix, iz, alpha, out));
        // The rim fades to nothing, which is the whole soft edge.
        outer.push(place(p.x, p.y, 0.0, out));
    }
    for i in 0..n {
        let j = (i + 1) % n;
        // Core fan: disjoint triangles, so the interior is exactly one
        // layer of `alpha` everywhere — no seams, no double darkening.
        out.push_tri(c, inner[i], inner[j]);
        // Penumbra ring.
        out.push_tri(inner[i], outer[i], outer[j]);
        out.push_tri(inner[i], outer[j], inner[j]);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sun_from(dir: Vec3f) -> GameSun {
        GameSun {
            dir: dir.normalize(),
            ..GameSun::default()
        }
    }

    fn flat() -> Receiver<'static> {
        Receiver {
            base_y: 0.0,
            terrain: None,
        }
    }

    /// Vertex positions in the built mesh, as (x, y, z, alpha).
    fn verts(b: &ShadowMeshBuilder) -> Vec<(f32, f32, f32, f32)> {
        b.vertices
            .chunks(16)
            .map(|v| (v[0], v[1], v[2], v[11]))
            .collect()
    }

    #[test]
    fn a_box_under_an_overhead_sun_casts_its_own_footprint() {
        let sun = sun_from(vec3f(0.0, 1.0, 0.0));
        let mut pts = Vec::new();
        let mut t = Mat4f::identity();
        t.v[12] = 2.0;
        t.v[13] = 1.0;
        t.v[14] = -3.0;
        caster_points(Shape::Box, &t, vec3f(2.0, 2.0, 2.0), &mut pts);
        let mut mesh = ShadowMeshBuilder::default();
        assert!(build_caster_shadow(&pts, &sun, &flat(), &mut mesh));
        // Outline spans the box footprint: x in [1,3], z in [-4,-2].
        let vs = verts(&mesh);
        let (min_x, max_x) = vs.iter().fold((f32::MAX, f32::MIN), |a, v| {
            (a.0.min(v.0), a.1.max(v.0))
        });
        assert!((min_x - 1.0).abs() < 0.05, "min x {min_x}");
        assert!((max_x - 3.0).abs() < 0.05, "max x {max_x}");
    }

    /// The property that makes fan triangulation the right choice: every
    /// interior triangle carries the same alpha, so overlapping coverage
    /// cannot double-darken.
    #[test]
    fn the_shadow_interior_has_one_uniform_alpha() {
        let sun = sun_from(vec3f(0.4, 1.0, 0.2));
        let mut pts = Vec::new();
        caster_points(
            Shape::Box,
            &Mat4f::identity(),
            vec3f(1.0, 1.0, 1.0),
            &mut pts,
        );
        let mut mesh = ShadowMeshBuilder::default();
        assert!(build_caster_shadow(&pts, &sun, &flat(), &mut mesh));
        let vs = verts(&mesh);
        let interior: Vec<f32> = vs.iter().map(|v| v.3).filter(|a| *a > 0.0).collect();
        assert!(!interior.is_empty());
        let first = interior[0];
        for a in &interior {
            assert!(
                (a - first).abs() < 1e-6,
                "interior alpha varies: {a} vs {first}"
            );
        }
        // And the rim really does reach zero, or there is no soft edge.
        assert!(vs.iter().any(|v| v.3 == 0.0));
    }

    #[test]
    fn a_cone_casts_a_pointed_shadow_not_a_rectangle() {
        // Low sun: the apex projects far, the base stays put, so the hull
        // must be longer than it is wide — a rectangle could not show this.
        let sun = sun_from(vec3f(1.0, 0.6, 0.0));
        let mut pts = Vec::new();
        let mut t = Mat4f::identity();
        t.v[13] = 1.0;
        caster_points(Shape::Cone, &t, vec3f(2.0, 2.0, 2.0), &mut pts);
        let mut mesh = ShadowMeshBuilder::default();
        assert!(build_caster_shadow(&pts, &sun, &flat(), &mut mesh));
        let vs = verts(&mesh);
        let (min_x, max_x, min_z, max_z) = vs.iter().fold(
            (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
            |a, v| (a.0.min(v.0), a.1.max(v.0), a.2.min(v.2), a.3.max(v.2)),
        );
        assert!(
            (max_x - min_x) > (max_z - min_z) * 1.3,
            "cone shadow should stretch along the sun: x {} z {}",
            max_x - min_x,
            max_z - min_z
        );
    }

    #[test]
    fn a_sphere_shadow_is_round_rather_than_square() {
        let sun = sun_from(vec3f(0.0, 1.0, 0.0));
        let mut pts = Vec::new();
        let mut t = Mat4f::identity();
        t.v[13] = 1.0;
        caster_points(Shape::Sphere, &t, vec3f(2.0, 2.0, 2.0), &mut pts);
        let mut mesh = ShadowMeshBuilder::default();
        assert!(build_caster_shadow(&pts, &sun, &flat(), &mut mesh));
        // A round hull has many boundary vertices; a quad would have four.
        let rim = verts(&mesh).iter().filter(|v| v.3 == 0.0).count();
        assert!(rim >= 8, "sphere hull should be round, got {rim} rim points");
    }

    #[test]
    fn a_higher_caster_gets_a_fainter_and_softer_shadow() {
        let sun = sun_from(vec3f(0.0, 1.0, 0.0));
        let mut low = ShadowMeshBuilder::default();
        let mut high = ShadowMeshBuilder::default();
        for (y, mesh) in [(0.5f32, &mut low), (4.0f32, &mut high)] {
            let mut pts = Vec::new();
            let mut t = Mat4f::identity();
            t.v[13] = y;
            caster_points(Shape::Box, &t, vec3f(1.0, 1.0, 1.0), &mut pts);
            assert!(build_caster_shadow(&pts, &sun, &flat(), mesh));
        }
        let a_low = verts(&low).iter().map(|v| v.3).fold(0.0f32, f32::max);
        let a_high = verts(&high).iter().map(|v| v.3).fold(0.0f32, f32::max);
        assert!(a_high < a_low, "high {a_high} should be fainter than low {a_low}");
    }

    #[test]
    fn nothing_is_cast_from_above_the_cutoff_or_below_the_ground() {
        let sun = sun_from(vec3f(0.0, 1.0, 0.0));
        for y in [40.0f32, -6.0] {
            let mut pts = Vec::new();
            let mut t = Mat4f::identity();
            t.v[13] = y;
            caster_points(Shape::Box, &t, vec3f(1.0, 1.0, 1.0), &mut pts);
            let mut mesh = ShadowMeshBuilder::default();
            assert!(!build_caster_shadow(&pts, &sun, &flat(), &mut mesh));
            assert!(mesh.is_empty());
        }
    }

    #[test]
    fn the_hull_ignores_interior_points() {
        // A dense cloud inside a square must hull to the square's corners.
        let mut pts = vec![
            vec2f(-1.0, -1.0),
            vec2f(1.0, -1.0),
            vec2f(1.0, 1.0),
            vec2f(-1.0, 1.0),
        ];
        for i in 0..20 {
            let t = i as f32 / 20.0;
            pts.push(vec2f(t * 0.5, t * 0.3));
        }
        assert_eq!(convex_hull(pts).len(), 4);
    }

    #[test]
    fn skinned_proxy_decimates_instead_of_projecting_every_vertex() {
        // 4000 vertices in PbrVertex layout.
        let mut verts = Vec::new();
        for i in 0..4000 {
            let f = i as f32 * 0.001;
            verts.extend_from_slice(&[f, f, f, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0]);
        }
        let mut out = Vec::new();
        skinned_proxy_points(&verts, &Mat4f::identity(), &mut out);
        assert!(
            (60..=70).contains(&out.len()),
            "proxy should be ~64 points, got {}",
            out.len()
        );
    }
}
