//! Cast shadows without a shadow map.
//!
//! The platform cannot sample a pass depth texture on any backend (game.md
//! §Rendering), so there is no shadow map to sample. Instead a caster's box
//! silhouette is projected onto the ground along the sun direction and drawn
//! as one dark alpha quad — same instance count as the blob shadow it
//! replaces, same batch, no extra pass, and unlike a blob it moves and
//! stretches as the sun moves.
//!
//! The projection of a box along a direction is a hexagon; this fits the
//! tight oriented bound of that hexagon in the sun's ground frame (u along
//! the shadow, v across it). That is exact in the direction that matters —
//! shadow length — and at most slightly generous across it.

use crate::sun::GameSun;
use makepad_draw::*;

/// A ground-plane shadow quad, oriented in the sun's ground frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowQuad {
    /// World-space centre, already lifted clear of z-fighting.
    pub center: Vec3f,
    /// Half-extent along the shadow direction (u).
    pub half_u: f32,
    /// Half-extent across the shadow direction (v).
    pub half_v: f32,
    /// Ground-frame basis: u = along the shadow, v = across it.
    pub u: Vec2f,
    pub v: Vec2f,
    pub alpha: f32,
}

impl ShadowQuad {
    /// Column-major model matrix mapping the unit cube onto this quad:
    /// local X follows `u`, local Z follows `v`, local Y stays up.
    pub fn transform(&self) -> Mat4f {
        let mut m = Mat4f::identity();
        m.v[0] = self.u.x;
        m.v[1] = 0.0;
        m.v[2] = self.u.y;
        m.v[4] = 0.0;
        m.v[5] = 1.0;
        m.v[6] = 0.0;
        m.v[8] = self.v.x;
        m.v[9] = 0.0;
        m.v[10] = self.v.y;
        m.v[12] = self.center.x;
        m.v[13] = self.center.y;
        m.v[14] = self.center.z;
        m
    }

    /// Cube size for the flat quad (a thin slab, drawn by the cube batch).
    pub fn size(&self) -> Vec3f {
        vec3f(self.half_u * 2.0, SHADOW_THICKNESS, self.half_v * 2.0)
    }
}

/// Thin enough to read as flat, thick enough to survive depth precision.
pub const SHADOW_THICKNESS: f32 = 0.02;
/// Lift off the ground so the quad wins the depth test against it.
const SHADOW_LIFT: f32 = 0.03;
/// Casters higher than this stop casting — a shadow that far from its
/// object reads as a stain, and the fade has reached zero anyway.
pub const MAX_SHADOW_DROP: f32 = 8.0;
/// Darkest a shadow gets directly under its caster.
const BASE_SHADOW_ALPHA: f32 = 0.35;

/// Project an axis-aligned box's silhouette onto the ground plane at
/// `ground_y`. Returns `None` when the caster is below the ground, too far
/// above it to read, or the sun is at the horizon.
///
/// `half` is the caster's half-extent, already scaled.
pub fn project_box_shadow(
    pos: Vec3f,
    half: Vec3f,
    ground_y: f32,
    sun: &GameSun,
) -> Option<ShadowQuad> {
    let feet = pos.y - half.y;
    let drop = feet - ground_y;
    if !(0.0..MAX_SHADOW_DROP).contains(&drop) {
        return None;
    }
    // Fade with height, then by the sun's own shadow strength: a low
    // evening sun casts softer shadows than a hard noon one.
    let alpha = (1.0 - drop / MAX_SHADOW_DROP) * BASE_SHADOW_ALPHA * (sun.shadow_alpha / 0.35);
    if alpha <= 0.001 {
        return None;
    }

    // With the sun overhead the ground frame is arbitrary; align it to the
    // world axes so the shadow is exactly the box footprint instead of a
    // rotated bound of it (which would be up to sqrt(2) too wide).
    let horiz = (sun.dir.x * sun.dir.x + sun.dir.z * sun.dir.z).sqrt();
    let (u, v) = if horiz < 1e-4 {
        (vec2f(1.0, 0.0), vec2f(0.0, 1.0))
    } else {
        let u = sun.dir_ground();
        (u, vec2f(-u.y, u.x))
    };
    let len_per_unit = sun.shadow_len_per_unit();

    // Project all 8 corners along the sun onto the ground plane, measuring
    // the hull's extent in the (u, v) frame. A corner at height h lands at
    // xz - h * len_per_unit * u (the sun direction points toward the sun,
    // so the shadow runs the other way).
    let (mut min_u, mut max_u) = (f32::MAX, f32::MIN);
    let (mut min_v, mut max_v) = (f32::MAX, f32::MIN);
    for sx in [-1.0f32, 1.0] {
        for sy in [-1.0f32, 1.0] {
            for sz in [-1.0f32, 1.0] {
                let cx = pos.x + sx * half.x;
                let cy = pos.y + sy * half.y;
                let cz = pos.z + sz * half.z;
                let h = (cy - ground_y).max(0.0);
                let gx = cx - h * len_per_unit * u.x;
                let gz = cz - h * len_per_unit * u.y;
                let du = gx * u.x + gz * u.y;
                let dv = gx * v.x + gz * v.y;
                min_u = min_u.min(du);
                max_u = max_u.max(du);
                min_v = min_v.min(dv);
                max_v = max_v.max(dv);
            }
        }
    }
    let cu = (min_u + max_u) * 0.5;
    let cv = (min_v + max_v) * 0.5;
    // Back to world from the (u, v) frame — u and v are orthonormal, so the
    // inverse is the transpose.
    let center = vec3f(
        cu * u.x + cv * v.x,
        ground_y + SHADOW_LIFT,
        cu * u.y + cv * v.y,
    );
    Some(ShadowQuad {
        center,
        half_u: (max_u - min_u) * 0.5,
        half_v: (max_v - min_v) * 0.5,
        u,
        v,
        alpha,
    })
}

/// The cheap tier: a fixed blob centred under the caster, no elongation.
/// What every mover used to get; still what scenery and over-budget casters
/// get. Same one-instance cost, so the difference is purely fidelity.
pub fn blob_shadow(pos: Vec3f, half: Vec3f, ground_y: f32, sun: &GameSun) -> Option<ShadowQuad> {
    let feet = pos.y - half.y;
    let drop = feet - ground_y;
    if !(0.0..MAX_SHADOW_DROP).contains(&drop) {
        return None;
    }
    let alpha = (1.0 - drop / MAX_SHADOW_DROP) * BASE_SHADOW_ALPHA * (sun.shadow_alpha / 0.35);
    if alpha <= 0.001 {
        return None;
    }
    Some(ShadowQuad {
        center: vec3f(pos.x, ground_y + SHADOW_LIFT, pos.z),
        half_u: half.x * 1.1,
        half_v: half.z * 1.1,
        u: vec2f(1.0, 0.0),
        v: vec2f(0.0, 1.0),
        alpha,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn sun_from(dir: Vec3f) -> GameSun {
        GameSun {
            dir: dir.normalize(),
            ..GameSun::default()
        }
    }

    #[test]
    fn straight_overhead_sun_puts_the_shadow_under_the_caster() {
        let sun = sun_from(vec3f(0.0, 1.0, 0.0));
        let q = project_box_shadow(vec3f(3.0, 5.0, -2.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun).unwrap();
        assert!(approx(q.center.x, 3.0), "x {}", q.center.x);
        assert!(approx(q.center.z, -2.0), "z {}", q.center.z);
        // No elongation: the silhouette is just the box footprint.
        assert!(approx(q.half_u, 1.0) && approx(q.half_v, 1.0));
    }

    #[test]
    fn a_low_sun_pushes_the_shadow_away_and_stretches_it() {
        // Sun low in +x: shadow runs toward -x and gets longer than wide.
        let sun = sun_from(vec3f(1.0, 0.5, 0.0));
        let q = project_box_shadow(vec3f(0.0, 2.0, 0.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun).unwrap();
        assert!(q.center.x < -0.5, "shadow should fall away from the sun: {}", q.center.x);
        assert!(q.half_u > q.half_v, "u {} v {}", q.half_u, q.half_v);
        assert!(approx(q.half_v, 1.0), "across-sun width is the box width: {}", q.half_v);
    }

    #[test]
    fn shadow_length_matches_the_analytic_projection() {
        // Sun at 45 degrees in +x: every unit of height offsets one unit of x.
        let sun = sun_from(vec3f(1.0, 1.0, 0.0));
        assert!(approx(sun.shadow_len_per_unit(), 1.0));
        let (pos, half) = (vec3f(0.0, 1.0, 0.0), vec3f(0.5, 1.0, 0.5));
        let q = project_box_shadow(pos, half, 0.0, &sun).unwrap();
        // Corners span heights 0..2, so the hull spans x in [-0.5, 0.5]
        // (footprint) shifted by [0, -2] -> [-2.5, 0.5]: half_u = 1.5.
        assert!(approx(q.half_u, 1.5), "half_u {}", q.half_u);
        assert!(approx(q.center.x, -1.0), "center.x {}", q.center.x);
    }

    #[test]
    fn diagonal_sun_keeps_the_quad_tight_via_its_own_frame() {
        let sun = sun_from(vec3f(1.0, 1.0, 1.0));
        let q = project_box_shadow(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun).unwrap();
        // u must follow the sun's ground direction, v perpendicular to it.
        assert!(approx(q.u.x * q.v.x + q.u.y * q.v.y, 0.0));
        assert!(approx((q.u.x * q.u.x + q.u.y * q.u.y).sqrt(), 1.0));
        // The shadow still runs away from the sun.
        assert!(q.center.x < 0.0 && q.center.z < 0.0);
    }

    #[test]
    fn transform_maps_the_unit_cube_onto_the_quad_axes() {
        let sun = sun_from(vec3f(1.0, 1.0, 0.0));
        let q = project_box_shadow(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun).unwrap();
        let m = q.transform();
        // Local X column follows u, local Z column follows v, Y stays up.
        assert!(approx(m.v[0], q.u.x) && approx(m.v[2], q.u.y));
        assert!(approx(m.v[8], q.v.x) && approx(m.v[10], q.v.y));
        assert!(approx(m.v[5], 1.0));
        assert!(approx(m.v[12], q.center.x) && approx(m.v[14], q.center.z));
    }

    #[test]
    fn fades_with_height_and_vanishes_past_the_limit() {
        let sun = sun_from(vec3f(0.0, 1.0, 0.0));
        let low = project_box_shadow(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun).unwrap();
        let high = project_box_shadow(vec3f(0.0, 5.0, 0.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun).unwrap();
        assert!(high.alpha < low.alpha);
        // Above the cutoff there is no shadow at all.
        assert!(project_box_shadow(vec3f(0.0, 20.0, 0.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun).is_none());
        // Below the ground either.
        assert!(project_box_shadow(vec3f(0.0, -3.0, 0.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun).is_none());
    }

    #[test]
    fn sun_shadow_alpha_scales_the_darkness() {
        let mut sun = sun_from(vec3f(0.0, 1.0, 0.0));
        let hard = project_box_shadow(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun)
            .unwrap()
            .alpha;
        sun.shadow_alpha = 0.1;
        let soft = project_box_shadow(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun)
            .unwrap()
            .alpha;
        assert!(soft < hard);
    }

    #[test]
    fn blob_is_the_axis_aligned_cheap_tier() {
        let sun = sun_from(vec3f(1.0, 0.5, 0.0));
        let b = blob_shadow(vec3f(2.0, 1.0, 3.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun).unwrap();
        // Centred under the caster, never displaced by the sun.
        assert!(approx(b.center.x, 2.0) && approx(b.center.z, 3.0));
        assert_eq!(b.u, vec2f(1.0, 0.0));
    }
}
