//! The sky's light on the skyline: the sun, the sky's ambient and a ring of horizon colours, which the strip render
//! takes as inputs (`Skyline::draw_view`, `request_band`). [`SkyLight::example`] is the example's own night.
//!
//! On the GPU (`SceneCtx::set_uniforms`): `sun_u`, `sun_el` (rad), `sun_rgb`, `ambient_rgb`, `lit` ([`SkyLight::lights_factor`])
//! and `design_gain` as uniforms (and `sil` scaled by it), the two rings as the texture `ring_tex` ([`SkyLight::ring_texels`]); the scene shaders read
//! them through `SkyScene`'s `haze_at`, `sky_at` and `sun_dir`.
use crate::params::{hash_bits, HAZE_COL};

/// Samples in each horizon ring.
pub const RING: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct SkyLight {
    /// The sun's azimuth as a strip u (0..1, same convention as the strip's x/w).
    pub sun_u: f32,
    /// The sun's elevation in degrees (negative below the horizon).
    pub sun_el_deg: f32,
    /// Sun irradiance on a surface facing it, divided by pi (radiance off a white
    /// Lambertian surface), linear Rec.709, map units.
    pub sun_rgb: [f32; 3],
    /// Sky ambient off a white upward surface, divided by pi, linear, map units.
    pub ambient_rgb: [f32; 3],
    /// Sky radiance at +1 degree elevation, RING samples round the strip (index i at u = (i+0.5)/RING).
    pub ring_low: [[f32; 3]; RING],
    /// Sky radiance at +15 degrees, for the sea's reflection by day.
    pub ring_high: [[f32; 3]; RING],
    /// Share of the lights kept on by day (0..1).
    pub lights_by_day: f32,
    /// Scale from the skyline's designed colours to the host sky's units: every colour the
    /// skyline was designed with (the night silhouettes, the night facade glow, every light,
    /// the smoke's glow) is multiplied by it, so the night looks the same against any host
    /// sky; the host's own light (the rings, the sun, the ambient) is not. 1 in the example.
    pub design_gain: f32,
}

impl SkyLight {
    /// Today's look: night, the fixed HAZE_COL ring, no sun, no ambient, design gain 1.
    pub fn example() -> Self {
        Self { sun_u: 0.0, sun_el_deg: -90.0, sun_rgb: [0.0; 3], ambient_rgb: [0.0; 3], ring_low: [HAZE_COL; RING], ring_high: [HAZE_COL; RING],
            lights_by_day: 0.0, design_gain: 1.0 }
    }

    /// How much of the city's lights are on: 0 above +2 degrees sun, 1 below -6, smooth between,
    /// raised to at least `lights_by_day`.
    pub fn lights_factor(&self) -> f32 {
        let t = ((2.0 - self.sun_el_deg) / 8.0).clamp(0.0, 1.0);
        (t * t * (3.0 - 2.0 * t)).max(self.lights_by_day.clamp(0.0, 1.0))
    }

    /// A hash of every field's bits, for cache keys.
    pub fn hash(&self) -> u64 {
        let rings = self.ring_low.iter().chain(&self.ring_high).flatten();
        let floats = [&self.sun_u, &self.sun_el_deg].into_iter().chain(&self.sun_rgb).chain(&self.ambient_rgb).chain(rings).chain([&self.lights_by_day, &self.design_gain]);
        hash_bits(0x5c71_19b7_0000_0002, floats.map(|x| x.to_bits() as u64))
    }

    /// The rings as the texels of a `RING` x 2 RGBA float texture: row 0 `ring_low`, row 1 `ring_high`, alpha 1.
    pub fn ring_texels(&self) -> Vec<f32> {
        self.ring_low.iter().chain(&self.ring_high).flat_map(|c| [c[0], c[1], c[2], 1.0]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(el: f32, by_day: f32) -> f32 {
        SkyLight { sun_el_deg: el, lights_by_day: by_day, ..SkyLight::example() }.lights_factor()
    }
    #[test]
    fn lights_are_off_by_day_full_at_night_and_smooth_between() {
        assert_eq!(at(30.0, 0.0), 0.0);
        assert_eq!(at(2.0, 0.0), 0.0);
        assert_eq!(at(-6.0, 0.0), 1.0);
        assert_eq!(at(-40.0, 0.0), 1.0);
        let mut last = 0.0;
        for i in 0..=80 { let f = at(2.0 - i as f32 * 0.1, 0.0); assert!(f >= last); last = f; }
    }
    #[test]
    fn lights_by_day_keeps_a_share_on() {
        assert_eq!(at(30.0, 0.25), 0.25);
        assert_eq!(at(-10.0, 0.25), 1.0);
    }
    #[test]
    fn the_example_light_is_night_with_the_fixed_haze() {
        let l = SkyLight::example();
        assert_eq!(l.lights_factor(), 1.0);
        assert_eq!(l.sun_rgb, [0.0; 3]);
        assert_eq!(l.ambient_rgb, [0.0; 3]);
        assert!(l.ring_low.iter().all(|c| *c == crate::params::HAZE_COL));
        assert_eq!(l.design_gain, 1.0);
    }
    #[test]
    fn the_hash_sees_every_field() {
        let a = SkyLight::example();
        let mut b = a.clone(); b.ring_low[17][1] += 1e-6;
        assert_ne!(a.hash(), b.hash());
        let mut c = a.clone(); c.design_gain = 2.0;
        assert_ne!(a.hash(), c.hash());
    }
}
