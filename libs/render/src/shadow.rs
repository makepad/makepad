//! The blob-quad shadow tier: the CHEAPEST cast shadow, one dark alpha quad
//! in the ordinary alpha cube batch.
//!
//! This is the OnChange fallback for entity casters past the hull budget
//! and for the small scurrying movers a hull never earned — primitive
//! runtime bodies with no model, so no offline `.shadowsdf` sidecar exists
//! for them, and the OnChange lightmap (statics only, edit-triggered)
//! cannot carry them either. In Realtime none of this draws: every dynamic
//! caster rasterizes into the GPU lightmap's depth passes instead.
//! Shared constants ([`MAX_SHADOW_DROP`], [`BASE_SHADOW_ALPHA`]) also feed
//! the hull/blob mesh tier in shadow_mesh.rs so the two fade identically.

use crate::sun::SunLight;
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
pub const BASE_SHADOW_ALPHA: f32 = 0.35;

/// A fixed blob centred under the caster, no elongation. Same one-instance
/// cost as any alpha cube; fades and vanishes with height exactly like the
/// mesh tier.
pub fn blob_shadow(pos: Vec3f, half: Vec3f, ground_y: f32, sun: &SunLight) -> Option<ShadowQuad> {
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

    fn sun_from(dir: Vec3f) -> SunLight {
        SunLight {
            dir: dir.normalize(),
            ..SunLight::default()
        }
    }

    #[test]
    fn transform_maps_the_unit_cube_onto_the_quad_axes() {
        let sun = sun_from(vec3f(1.0, 0.5, 0.0));
        let q = blob_shadow(vec3f(2.0, 1.0, 3.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun).unwrap();
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
        let low = blob_shadow(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun).unwrap();
        let high = blob_shadow(vec3f(0.0, 5.0, 0.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun).unwrap();
        assert!(high.alpha < low.alpha);
        // Above the cutoff there is no shadow at all.
        assert!(blob_shadow(vec3f(0.0, 20.0, 0.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun).is_none());
        // Below the ground either.
        assert!(blob_shadow(vec3f(0.0, -3.0, 0.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun).is_none());
    }

    #[test]
    fn sun_shadow_alpha_scales_the_darkness() {
        let mut sun = sun_from(vec3f(0.0, 1.0, 0.0));
        let hard = blob_shadow(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun)
            .unwrap()
            .alpha;
        sun.shadow_alpha = 0.1;
        let soft = blob_shadow(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun)
            .unwrap()
            .alpha;
        assert!(soft < hard);
    }

    #[test]
    fn blob_is_centred_and_axis_aligned() {
        let sun = sun_from(vec3f(1.0, 0.5, 0.0));
        let b = blob_shadow(vec3f(2.0, 1.0, 3.0), vec3f(1.0, 1.0, 1.0), 0.0, &sun).unwrap();
        // Centred under the caster, never displaced by the sun.
        assert!(approx(b.center.x, 2.0) && approx(b.center.z, 3.0));
        assert_eq!(b.u, vec2f(1.0, 0.0));
    }
}
