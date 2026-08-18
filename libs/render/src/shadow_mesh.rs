//! The per-frame dynamic shadow MESH — the OnChange tier's fallback layer
//! for casters that cannot ride the prebaked SDF quads:
//!
//! * [`build_caster_shadow`]: sun-projected convex-hull fans for primitive
//!   ENTITY bodies (crates, movers) — runtime-spawned shapes with no model
//!   and therefore no offline `.shadowsdf` sidecar to sample.
//! * [`build_blob_shadow`]: the M64 contact blob under a character or car
//!   whose rig/model has no loadable sidecar.
//!
//! In Realtime mode none of this draws — every dynamic caster rasterizes
//! into the GPU lightmap's depth passes instead (gpu_lightmap.rs). Statics
//! never touch this layer either: their sun shadows live in the baked
//! lightmap and their grounding in their own AO atlases.
//!
//! The hull fan is the alpha-blend-safe construction: it has **no
//! self-overlap**, so every interior pixel is covered exactly once and the
//! shadow has one uniform alpha — projecting the caster's triangles
//! directly would double-darken every overlap. Draping and z-fighting are
//! handled structurally rather than by fudge factors — see [`Receiver`] and
//! [`SHADOW_NORMAL_BIAS`].

use makepad_draw::*;
use makepad_game_sim::{Shape, Terrain};

use crate::shadow::{BASE_SHADOW_ALPHA, MAX_SHADOW_DROP};
use crate::sun::SunLight;

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
    /// World AABBs of static geometry whose TOPS also receive: road slabs,
    /// platforms, crate lids. Without these a fence's shadow vanished the
    /// moment it crossed a raised road — the slab sat above the draped
    /// vertices and simply occluded them. Empty is always valid.
    pub statics: &'a [(Vec3f, Vec3f)],
}

/// How far above the base plane a static top may sit and still catch a
/// draped shadow. Past this the geometry is a roof, and a ground shadow
/// smeared onto a roof reads as an error, not as draping.
const RECEIVER_TOP_MAX: f32 = 2.5;

impl Receiver<'_> {
    /// Surface height and normal under a world x/z. Falls back to the flat
    /// plane wherever the terrain is absent or below it (a box top over a
    /// valley receives on the box, not the valley floor). Static tops win
    /// over both when they are higher — the road slab over the grass.
    pub fn sample(&self, x: f32, z: f32) -> (f32, Vec3f) {
        let mut y = self.base_y;
        let mut n = vec3f(0.0, 1.0, 0.0);
        if let Some(t) = self.terrain {
            if let Some(h) = t.height_at(x, z) {
                if h >= self.base_y - 1.0e-3 {
                    y = h;
                    n = t.normal_at(x, z).unwrap_or(vec3f(0.0, 1.0, 0.0));
                }
            }
        }
        for (lo, hi) in self.statics {
            if x >= lo.x && x <= hi.x && z >= lo.z && z <= hi.z {
                let top = hi.y;
                if top > y && top <= self.base_y + RECEIVER_TOP_MAX {
                    y = top;
                    n = vec3f(0.0, 1.0, 0.0);
                }
            }
        }
        (y, n)
    }

    /// Is this receiver flat everywhere the shadow could land? Enables the
    /// no-subdivision fast path.
    pub fn is_flat(&self) -> bool {
        self.terrain.is_none() && self.statics.is_empty()
    }
}

/// The drop-fade parameters every dynamic shadow fan shares, factored out so
/// the GPU-instanced fan (renderer.rs) computes exactly what the CPU builders
/// below do: `(ground, ground normal, alpha, spread)` for a caster anchored
/// at `anchor`, or `None` when it is out of shadow range of its receiver.
pub fn shadow_drop_params(
    anchor: Vec3f,
    receiver: &Receiver,
    sun: &SunLight,
) -> Option<(f32, Vec3f, f32, f32)> {
    let (ground, normal) = receiver.sample(anchor.x, anchor.z);
    let gap = anchor.y - ground;
    if !(-0.5..MAX_SHADOW_DROP).contains(&gap) {
        return None;
    }
    let fade = (1.0 - (gap.max(0.0) / MAX_SHADOW_DROP)).clamp(0.0, 1.0);
    let alpha = fade * BASE_SHADOW_ALPHA * (sun.shadow_alpha / 0.35);
    if alpha <= 0.002 {
        return None;
    }
    let spread = 1.0 - 0.35 * (gap.max(0.0) / MAX_SHADOW_DROP);
    Some((ground, normal, alpha, spread))
}

/// Floats per vertex in the packed `geom.GameMeshVertex` layout.
pub const SHADOW_VERTEX_FLOATS: usize = 6;

/// Accumulates every caster's shadow triangles into ONE mesh, so all the
/// shadows in a frame cost a single draw call rather than one each.
///
/// Packed layout (6 floats, not PbrVertex's 16): a shadow needs only a
/// position and an alpha, so carrying a normal, uv and tangent per vertex was
/// 40 bytes of nothing. Alpha rides in the unorm8 colour slot.
#[derive(Default)]
pub struct ShadowMeshBuilder {
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

    pub(crate) fn push_vertex(&mut self, p: Vec3f, alpha: f32) -> u32 {
        let index = (self.vertices.len() / SHADOW_VERTEX_FLOATS) as u32;
        self.vertices.extend_from_slice(&[
            p.x,
            p.y,
            p.z,
            // Normal is unused by the shadow shader; oct(0,0) keeps the slot
            // well-formed for anything that later reads this layout.
            makepad_draw::pack_pair_f16(0.0, 0.0),
            makepad_draw::pack_pair_f16(0.0, 0.0),
            // Premultiplied black: RGB 0, coverage in alpha.
            makepad_draw::pack_unorm8x4(0.0, 0.0, 0.0, alpha),
        ]);
        index
    }

    pub(crate) fn push_tri(&mut self, a: u32, b: u32, c: u32) {
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

/// Andrew's monotone chain, on the ground plane. Returns CCW hull points.
pub(crate) fn convex_hull(mut pts: Vec<Vec2f>) -> Vec<Vec2f> {
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
    sun: &SunLight,
    receiver: &Receiver,
    out: &mut ShadowMeshBuilder,
) -> bool {
    if points.len() < 3 {
        return false;
    }
    let mut lowest = f32::MAX;
    let mut cx = 0.0f32;
    let mut cz = 0.0f32;
    for p in points {
        lowest = lowest.min(p.y);
        cx += p.x;
        cz += p.z;
    }
    cx /= points.len() as f32;
    cz /= points.len() as f32;
    // The plane the shadow falls on is the ground UNDER THE CASTER — not
    // the receiver's global base. Projecting to base_y slid a character on
    // any raised floor sideways by (height/tan(sun)) — a detached shadow
    // metres away ("not even wrong"). One sample fixes it.
    let (base, _) = receiver.sample(cx, cz);
    let drop = lowest - base;
    if !(-0.5..MAX_SHADOW_DROP).contains(&drop) {
        return false;
    }
    let mut fade = (1.0 - (drop.max(0.0) / MAX_SHADOW_DROP)).clamp(0.0, 1.0);

    // Project along the sun onto that local plane. A point at height h
    // lands h * len_per_unit away, opposite the sun.
    let len_per_unit = sun.shadow_len_per_unit();
    let g = sun.dir_ground();
    let flat: Vec<Vec2f> = points
        .iter()
        .map(|p| {
            let h = (p.y - base).max(0.0);
            vec2f(p.x - h * len_per_unit * g.x, p.z - h * len_per_unit * g.y)
        })
        .collect();

    // FLAT-ONLY dynamic shadows: probe the landing footprint; where the
    // ground steps or slopes under it, fade the drape out entirely rather
    // than wrap it wrongly — an absent shadow reads as soft lighting, a
    // broken one reads as a bug (user call, 2026-08-12).
    {
        let (mut lo2, mut hi2) = (vec2f(f32::MAX, f32::MAX), vec2f(f32::MIN, f32::MIN));
        for p in &flat {
            lo2 = vec2f(lo2.x.min(p.x), lo2.y.min(p.y));
            hi2 = vec2f(hi2.x.max(p.x), hi2.y.max(p.y));
        }
        let probes = [
            vec2f(lo2.x, lo2.y),
            vec2f(hi2.x, lo2.y),
            vec2f(lo2.x, hi2.y),
            vec2f(hi2.x, hi2.y),
            vec2f((lo2.x + hi2.x) * 0.5, (lo2.y + hi2.y) * 0.5),
        ];
        let (mut ymin, mut ymax) = (f32::MAX, f32::MIN);
        for q in probes {
            let (y, _) = receiver.sample(q.x, q.y);
            ymin = ymin.min(y);
            ymax = ymax.max(y);
        }
        let spread = ymax - ymin;
        fade *= (1.0 - ((spread - 0.05) / 0.10).clamp(0.0, 1.0)).clamp(0.0, 1.0);
    }
    let alpha = fade * BASE_SHADOW_ALPHA * (sun.shadow_alpha / 0.35);
    if alpha <= 0.002 {
        return false;
    }

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

/// Rings of the character blob, centre out. Three alpha stations sampled
/// from the smooth profile `(1-r²)²` — with vertex interpolation between
/// them that is enough for a gradient with no visible polygon rim.
const BLOB_SEGMENTS: usize = 16;

/// A soft elliptical blob under a character — the Mario 64 shadow.
///
/// No projection, no convex hull, no posed vertices: characters previously
/// re-ran decimate → project-along-sun → hull → fan EVERY FRAME each, and
/// bought with it a hard-edged convex smear that read as a hole in the
/// ground. A blob is what that CPU work was approximating badly anyway at
/// this art scale: it tracks the character's position for free, shrinks and
/// fades as they jump (`gap`), and its gradient is genuinely soft.
///
/// Deliberately NOT sun-oriented — a fixed under-foot shadow is also the
/// gameplay aid (where will I land), which is why the reference is M64.
pub fn build_blob_shadow(
    centre: Vec3f,
    half_x: f32,
    half_z: f32,
    sun: &SunLight,
    receiver: &Receiver,
    out: &mut ShadowMeshBuilder,
) -> bool {
    let (hx, hz) = (half_x.abs(), half_z.abs());
    if hx < 1.0e-3 || hz < 1.0e-3 {
        return false;
    }
    let (ground, _) = receiver.sample(centre.x, centre.z);
    let gap = centre.y - ground;
    if !(-0.5..MAX_SHADOW_DROP).contains(&gap) {
        return false;
    }
    // Same strength law as the hull shadows, so a character standing beside
    // a house reads as lit by the same sun.
    let fade = (1.0 - (gap.max(0.0) / MAX_SHADOW_DROP)).clamp(0.0, 1.0);
    let alpha = fade * BASE_SHADOW_ALPHA * (sun.shadow_alpha / 0.35);
    if alpha <= 0.002 {
        return false;
    }
    // Airborne: the blob tightens toward the landing point as well as fading.
    let spread = 1.0 - 0.35 * (gap.max(0.0) / MAX_SHADOW_DROP);

    // (1-r²)² at r = 0, 0.55, 1: full, ~half, zero.
    let mut pts = Vec::with_capacity(BLOB_SEGMENTS * 2 + 1);
    let mut als = Vec::with_capacity(BLOB_SEGMENTS * 2 + 1);
    pts.push(vec2f(centre.x, centre.z));
    als.push(alpha);
    for i in 0..BLOB_SEGMENTS {
        let t = i as f32 / BLOB_SEGMENTS as f32 * std::f32::consts::TAU;
        let (s, co) = (t.sin(), t.cos());
        let (dx, dz) = (co * hx * spread, s * hz * spread);
        pts.push(vec2f(centre.x + dx * 0.55, centre.z + dz * 0.55));
        als.push(alpha * 0.49);
        pts.push(vec2f(centre.x + dx, centre.z + dz));
        als.push(0.0);
    }
    let mut tris = Vec::with_capacity(BLOB_SEGMENTS * 9);
    for i in 0..BLOB_SEGMENTS {
        let j = (i + 1) % BLOB_SEGMENTS;
        let (mi, ri, mj, rj) = (1 + i * 2, 2 + i * 2, 1 + j * 2, 2 + j * 2);
        tris.extend_from_slice(&[0, mi, mj]);
        tris.extend_from_slice(&[mi, ri, rj]);
        tris.extend_from_slice(&[mi, rj, mj]);
    }
    drape_emit(&pts, &als, &tris, receiver, out);
    true
}

/// Emit a small caster's fan geometry onto the receiver, subdividing where
/// the ground actually varies.
///
/// Dynamic shadows draw big triangles (a car's ring spans metres); on hills
/// those cut chords through the terrain — sunk on one side, floating on the
/// other. Subdividing every frame everywhere would waste the flat plaza
/// case, so the receiver is probed first: flat under the caster means the
/// triangles go through untouched, and the caster's own midpoint cache
/// keeps subdivided neighbours crack-free. The receiver's statics are
/// prefiltered to the caster's bounds once — `sample` walks that short
/// list per vertex instead of the whole scene's.
pub(crate) fn drape_emit(
    pts2: &[Vec2f],
    alphas: &[f32],
    tris: &[usize],
    receiver: &Receiver,
    out: &mut ShadowMeshBuilder,
) {
    let (mut lo, mut hi) = (vec2f(f32::MAX, f32::MAX), vec2f(f32::MIN, f32::MIN));
    for p in pts2 {
        lo = vec2f(lo.x.min(p.x), lo.y.min(p.y));
        hi = vec2f(hi.x.max(p.x), hi.y.max(p.y));
    }
    let local_boxes: Vec<(Vec3f, Vec3f)> = receiver
        .statics
        .iter()
        .filter(|(bl, bh)| {
            bl.x <= hi.x && bh.x >= lo.x && bl.z <= hi.y && bh.z >= lo.y
        })
        .copied()
        .collect();
    let receiver = Receiver {
        base_y: receiver.base_y,
        terrain: receiver.terrain,
        statics: &local_boxes,
    };
    let place = |p: Vec2f, a: f32, out: &mut ShadowMeshBuilder| -> u32 {
        let (y, n) = receiver.sample(p.x, p.y);
        let slope = (1.0 - n.y.clamp(0.0, 1.0)).clamp(0.0, 1.0);
        let lift = SHADOW_NORMAL_BIAS + SHADOW_SLOPE_BIAS * slope;
        out.push_vertex(vec3f(p.x + n.x * lift, y + n.y * lift, p.y + n.z * lift), a)
    };

    // Flat under the whole footprint: no subdivision, no midpoint cache.
    let mut flat = receiver.is_flat();
    if !flat {
        let probes = [
            vec2f(lo.x, lo.y),
            vec2f(hi.x, lo.y),
            vec2f(lo.x, hi.y),
            vec2f(hi.x, hi.y),
            vec2f((lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5),
        ];
        let (mut ymin, mut ymax) = (f32::MAX, f32::MIN);
        for p in probes {
            let (y, _) = receiver.sample(p.x, p.y);
            ymin = ymin.min(y);
            ymax = ymax.max(y);
        }
        flat = ymax - ymin < 0.03;
    }

    let mut pts: Vec<Vec2f> = pts2.to_vec();
    let mut als: Vec<f32> = alphas.to_vec();
    let mut ids: std::collections::HashMap<usize, u32> = std::collections::HashMap::new();
    let mut midpoints: std::collections::HashMap<(usize, usize), usize> =
        std::collections::HashMap::new();
    let mut queue: Vec<[usize; 3]> = tris.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
    let mut guard = 0usize;
    while let Some([i0, i1, i2]) = queue.pop() {
        guard += 1;
        let (a, b, c) = (pts[i0], pts[i1], pts[i2]);
        let e = [
            (i0, i1, (b.x - a.x).powi(2) + (b.y - a.y).powi(2)),
            (i1, i2, (c.x - b.x).powi(2) + (c.y - b.y).powi(2)),
            (i2, i0, (a.x - c.x).powi(2) + (a.y - c.y).powi(2)),
        ];
        let longest = e
            .iter()
            .max_by(|x, y| x.2.partial_cmp(&y.2).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap();
        if !flat && longest.2 > 0.35 * 0.35 && guard < 4_000 {
            let key = if longest.0 < longest.1 {
                (longest.0, longest.1)
            } else {
                (longest.1, longest.0)
            };
            let m = *midpoints.entry(key).or_insert_with(|| {
                let (p, q) = (pts[key.0], pts[key.1]);
                pts.push(vec2f((p.x + q.x) * 0.5, (p.y + q.y) * 0.5));
                als.push((als[key.0] + als[key.1]) * 0.5);
                pts.len() - 1
            });
            let (s0, s1) = (longest.0, longest.1);
            let other = if s0 != i0 && s1 != i0 {
                i0
            } else if s0 != i1 && s1 != i1 {
                i1
            } else {
                i2
            };
            queue.push([s0, m, other]);
            queue.push([m, s1, other]);
            continue;
        }
        // Same interior-aware curtain rule as the silhouette path: probe
        // corners, edge midpoints and centroid; on a step, subdivide while
        // coarse and drop at the floor — no ribbons, no buried
        // corner-crossers.
        if !flat {
            let probe = |q: Vec2f| receiver.sample(q.x, q.y).0;
            let hs = [
                probe(pts[i0]),
                probe(pts[i1]),
                probe(pts[i2]),
                probe(vec2f((pts[i0].x + pts[i1].x) * 0.5, (pts[i0].y + pts[i1].y) * 0.5)),
                probe(vec2f((pts[i1].x + pts[i2].x) * 0.5, (pts[i1].y + pts[i2].y) * 0.5)),
                probe(vec2f((pts[i2].x + pts[i0].x) * 0.5, (pts[i2].y + pts[i0].y) * 0.5)),
                probe(vec2f(
                    (pts[i0].x + pts[i1].x + pts[i2].x) / 3.0,
                    (pts[i0].y + pts[i1].y + pts[i2].y) / 3.0,
                )),
            ];
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for h in hs {
                lo = lo.min(h);
                hi = hi.max(h);
            }
            if hi - lo > 0.06 {
                let (aq, bq, cq) = (pts[i0], pts[i1], pts[i2]);
                let l01 = (bq.x - aq.x).powi(2) + (bq.y - aq.y).powi(2);
                let l12 = (cq.x - bq.x).powi(2) + (cq.y - bq.y).powi(2);
                let l20 = (aq.x - cq.x).powi(2) + (aq.y - cq.y).powi(2);
                if l01.max(l12).max(l20) > 0.04 && guard < 4_000 {
                    let key = if i0 < i1 { (i0, i1) } else { (i1, i0) };
                    let m = *midpoints.entry(key).or_insert_with(|| {
                        let (pq, qq) = (pts[i0], pts[i1]);
                        pts.push(vec2f((pq.x + qq.x) * 0.5, (pq.y + qq.y) * 0.5));
                        als.push((als[i0] + als[i1]) * 0.5);
                        pts.len() - 1
                    });
                    queue.push([i0, m, i2]);
                    queue.push([m, i1, i2]);
                    continue;
                }
                continue;
            }
        }
        let mut id_of = |i: usize, out: &mut ShadowMeshBuilder| {
            *ids.entry(i).or_insert_with(|| place(pts[i], als[i], out))
        };
        let (v0, v1, v2) = (id_of(i0, out), id_of(i1, out), id_of(i2, out));
        out.push_tri(v0, v1, v2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sun_from(dir: Vec3f) -> SunLight {
        SunLight {
            dir: dir.normalize(),
            ..SunLight::default()
        }
    }

    fn flat() -> Receiver<'static> {
        Receiver {
            base_y: 0.0,
            terrain: None,
            statics: &[],
        }
    }

    /// Vertex positions in the built mesh, as (x, y, z, alpha).
    fn verts(b: &ShadowMeshBuilder) -> Vec<(f32, f32, f32, f32)> {
        b.vertices
            .chunks(SHADOW_VERTEX_FLOATS)
            .map(|v| {
                // alpha is the top unorm8 lane of the packed colour slot
                let a = ((v[5].to_bits() >> 24) & 0xff) as f32 / 255.0;
                (v[0], v[1], v[2], a)
            })
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

    /// The blob is the character shadow now: soft rim, no projection. It must
    /// fade and tighten as the caster rises, and vanish past the drop limit —
    /// the jump cue the hull path used to buy with per-frame CPU.
    #[test]
    fn blob_shadow_fades_and_tightens_with_height() {
        let sun = sun_from(vec3f(0.3, 0.8, 0.3));
        let peak = |b: &ShadowMeshBuilder| {
            b.vertices
                .chunks(SHADOW_VERTEX_FLOATS)
                .map(|v| ((v[5].to_bits() >> 24) & 0xff) as f32 / 255.0)
                .fold(0.0f32, f32::max)
        };
        let mut grounded = ShadowMeshBuilder::default();
        assert!(build_blob_shadow(vec3f(0.0, 0.0, 0.0), 0.45, 0.45, &sun, &flat(), &mut grounded));
        let mut airborne = ShadowMeshBuilder::default();
        assert!(build_blob_shadow(vec3f(0.0, 2.0, 0.0), 0.45, 0.45, &sun, &flat(), &mut airborne));
        assert!(
            peak(&airborne) < peak(&grounded),
            "jumping did not lighten the blob ({} vs {})",
            peak(&airborne),
            peak(&grounded)
        );
        let mut gone = ShadowMeshBuilder::default();
        assert!(
            !build_blob_shadow(vec3f(0.0, 50.0, 0.0), 0.45, 0.45, &sun, &flat(), &mut gone),
            "a caster far above the ground still cast a blob"
        );
    }

}
