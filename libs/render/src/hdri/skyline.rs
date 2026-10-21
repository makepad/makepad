//! The skyline layer: a city strip the app renders on the GPU (makepad-skyline)
//! and hands over in `ImageSet::skyline`, laid over the base under every light
//! and caster, with the weather in front of it (`Env::base_at`).
//!
//! The engine never touches the GPU and does not depend on makepad-skyline: the
//! strip is plain data. Its convention is the skyline's: rows linear in
//! elevation from `top_deg` (the top row's top edge) to `bottom_deg` (the
//! bottom row's bottom edge), top row first, and x/w equal to the map's own
//! equirect u (`ibl::dir_to_equirect_uv`, north at u = 0.5) at Turn 0. Texels
//! are premultiplied linear RGBA; RGB may be non-zero where alpha is 0 (a
//! light's glow over the transparent sky).

use makepad_draw::*;
use makepad_render_material::ibl;
use super::arr;

/// The band's lowest FADE_DEG degrees fade its alpha (and, premultiplied, its
/// light) to 0, so the generated ground takes over below it without a seam.
pub const FADE_DEG: f32 = 1.0;

/// The skyline's strip, as the app rendered it.
#[derive(Clone, PartialEq)]
pub struct SkylineBand {
    pub w: usize,
    pub h: usize,
    /// Premultiplied linear RGBA, top row first, in the strip frame (Turn 0).
    pub texels: Vec<[f32; 4]>,
    /// Elevation of the top row's top edge and the bottom row's bottom edge, degrees.
    pub top_deg: f32,
    pub bottom_deg: f32,
}

/// The size and the range, not the texels: an `ImageSet` is printed whole.
impl std::fmt::Debug for SkylineBand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SkylineBand {{ {} x {}, {} to {} deg }}", self.w, self.h, self.top_deg, self.bottom_deg)
    }
}

impl SkylineBand {
    /// Bilinear premultiplied sample at map direction `d`, turned by `turn_deg`; 0 outside the band.
    /// Below the horizon the alpha fades to 0 over the band's lowest FADE_DEG so the generator's
    /// ground takes over without a seam. A positive turn moves the city toward a larger u
    /// (clockwise seen from above, the compass azimuth rising).
    pub fn sample(&self, d: Vec3f, turn_deg: f32) -> [f32; 4] {
        let (w, h) = (self.w, self.h);
        let span = self.top_deg - self.bottom_deg;
        if w == 0 || h == 0 || self.texels.len() < w * h || !(span > 0.0) {
            return [0.0; 4];
        }
        let el = d.y.clamp(-1.0, 1.0).asin().to_degrees();
        // The negated test also turns a NaN direction away.
        if !(el >= self.bottom_deg && el <= self.top_deg) {
            return [0.0; 4];
        }
        let u = (ibl::dir_to_equirect_uv(arr(d))[0] - turn_deg / 360.0).rem_euclid(1.0);
        let x = u * w as f32 - 0.5;
        let y = ((self.top_deg - el) / span * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let column = |i: f32| (i as i64).rem_euclid(w as i64) as usize;
        let (c0, c1) = (column(x0), column(x0 + 1.0));
        let (r0, r1) = (y0 as usize, (y0 as usize + 1).min(h - 1));
        let texel = |r: usize, c: usize| self.texels[r * w + c];
        // Lerps, not weights: four equal texels give exactly that texel back.
        let lerp = |a: [f32; 4], b: [f32; 4], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t];
        let mut s = lerp(lerp(texel(r0, c0), texel(r0, c1), fx), lerp(texel(r1, c0), texel(r1, c1), fx), fy);
        if el < self.bottom_deg + FADE_DEG {
            let fade = smooth((el - self.bottom_deg) / FADE_DEG);
            for channel in &mut s {
                *channel *= fade;
            }
        }
        s
    }

    /// The largest RGB value in the band (for bloom's base_peak).
    pub fn peak(&self) -> f32 {
        self.texels.iter().fold(0.0f32, |peak, t| peak.max(t[0]).max(t[1]).max(t[2]))
    }
}

/// Lays the strip over an already-veiled base: (1-a)·base + veil(s) - (1-a)·veil(0).
/// `veil` is the weather's veil at d (identity when there is no weather). Exact because
/// every veil is affine in its input at a fixed direction: veil(x) = M·x + k, so this is
/// veil((1-a)·c + s) for the base's own unveiled c, and what the base added after its
/// veil (a storm's bolt) is covered by the city as the sky is.
pub fn over_veiled(s: [f32; 4], base: Vec3f, veil: &dyn Fn(Vec3f) -> Vec3f) -> Vec3f {
    let behind = 1.0 - s[3];
    let lit = veil(vec3f(s[0], s[1], s[2]));
    // An opaque texel hides the base outright (0 x a huge base stays 0, not NaN).
    if behind > 0.0 {
        base * behind + lit - veil(Vec3f::default()) * behind
    } else {
        lit
    }
}

/// Smoothstep over 0..1.
fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdri::{dir_from_az_el, vec, CasterParams, Env, HdriParams, ImageSet, LightParams, SkylineParams, WeatherParams, SkyParams};
    use crate::hdri::weather::Weather;
    use makepad_render_material::ibl;
    use std::sync::Arc;

    fn band(top: f32, bottom: f32, rgba: [f32; 4]) -> SkylineBand {
        let (w, h) = (64, 16);
        SkylineBand { w, h, texels: vec![rgba; w * h], top_deg: top, bottom_deg: bottom }
    }
    fn with_band(p: &HdriParams, b: SkylineBand) -> (HdriParams, ImageSet) {
        let mut p = p.clone();
        p.skyline = Some(SkylineParams { on: true, seed: 7, turn_deg: 0.0, lights_by_day: 0.0, settings: Default::default() });
        let mut images = ImageSet::default();
        images.skyline = Some(Arc::new(b));
        (p, images)
    }
    /// The engine's compass direction (mod.rs's `dir_from_az_el`: azimuth clockwise from north).
    fn dir(az_deg: f32, el_deg: f32) -> Vec3f {
        dir_from_az_el(az_deg, el_deg)
    }
    /// The direction at equirect `u` and elevation `el_deg`, through the very mapping a bake
    /// evaluates its texels with (`EnvMap::from_fn` -> `ibl::equirect_uv_to_dir`, v = 0.5 - el/180).
    fn dir_at_u(u: f32, el_deg: f32) -> Vec3f {
        vec(ibl::equirect_uv_to_dir([u, 0.5 - el_deg / 180.0]))
    }
    fn rgb(s: [f32; 4]) -> Vec3f {
        vec3f(s[0], s[1], s[2])
    }
    fn close3(a: Vec3f, b: Vec3f, rel: f32) -> bool {
        (a - b).length() <= rel * (1.0 + a.length().max(b.length()))
    }
    /// A deterministic number in 0..1 (LCG), as mod.rs's tests draw theirs.
    fn next(state: &mut u32) -> f32 {
        *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*state >> 8) as f32 / 16_777_216.0
    }

    #[test]
    fn no_skyline_bakes_bit_identical() {
        let p = HdriParams::default();
        let base = Env::new(&p).bake(64);
        // a skyline block that is off, and a band with no block, both change nothing
        let (mut q, images) = with_band(&p, band(20.0, -5.0, [0.5, 0.5, 0.5, 1.0]));
        q.skyline.as_mut().unwrap().on = false;
        assert!(!Env::new_with(&q, &images).has_skyline());
        assert!(Env::new_with(&q, &images).bake(64) == base);
        assert!(Env::new_with(&p, &images).bake(64) == base);
        // and a block that is on with no band in the images
        let (q, _) = with_band(&p, band(20.0, -5.0, [0.5, 0.5, 0.5, 1.0]));
        assert!(!Env::new(&q).has_skyline());
        assert!(Env::new(&q).bake(64) == base);
    }

    #[test]
    fn an_opaque_band_replaces_the_sky_inside_it_and_nowhere_else() {
        let p = HdriParams::default();
        let (q, images) = with_band(&p, band(20.0, -5.0, [0.25, 0.5, 0.75, 1.0]));
        let (a, b) = (Env::new(&p), Env::new_with(&q, &images));
        assert!(b.has_skyline() && !a.has_skyline());
        let inside = b.radiance(dir(30.0, 10.0));
        assert!((inside - Vec3f { x: 0.25, y: 0.5, z: 0.75 } * b.scale()).length() < 1e-4);
        assert_eq!(a.radiance(dir(30.0, 40.0)), b.radiance(dir(30.0, 40.0)));   // above the band
        assert_eq!(a.radiance(dir(30.0, -30.0)), b.radiance(dir(30.0, -30.0))); // below the band
    }

    #[test]
    fn a_half_transparent_band_lays_over_the_sky() {
        let p = HdriParams::default();
        let s = [0.1, 0.1, 0.1, 0.5];
        let (q, images) = with_band(&p, band(20.0, -5.0, s));
        let d = dir(30.0, 10.0);
        let sky = Env::new(&p).radiance(d);
        let got = Env::new_with(&q, &images).radiance(d);
        let k = Env::new(&p).scale();
        let want = Vec3f { x: 0.1, y: 0.1, z: 0.1 } * k + sky * 0.5;
        assert!((got - want).length() < 1e-4 * (1.0 + want.length()));
    }

    #[test]
    fn the_band_fades_into_the_ground_at_its_bottom() {
        let b = band(20.0, -5.0, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(b.sample(dir(0.0, -5.0 + FADE_DEG + 0.01), 0.0)[3], 1.0);
        assert!(b.sample(dir(0.0, -5.0 + 0.05), 0.0)[3] < 0.1);
    }

    #[test]
    fn turn_rotates_the_band() {
        let (w, h) = (360, 4);
        let mut t = vec![[0.0; 4]; w * h];
        for y in 0..h { t[y * w + 90] = [1.0, 1.0, 1.0, 1.0]; } // a post at strip u = 90.5/360
        let b = SkylineBand { w, h, texels: t, top_deg: 2.0, bottom_deg: -2.0 };
        let u_post = 90.5 / 360.0;
        // direction whose equirect u is u_post (+ 30 deg after a 30-degree turn)
        assert!(b.sample(dir_at_u(u_post, 0.0), 0.0)[3] > 0.9);
        assert!(b.sample(dir_at_u(u_post + 30.0 / 360.0, 0.0), 30.0)[3] > 0.9);
        assert!(b.sample(dir_at_u(u_post, 0.0), 30.0)[3] < 0.1);
    }

    #[test]
    fn a_bake_texels_u_is_the_bands_x_over_w() {
        // The bake's centre column looks north (mod.rs's `the_centre_column_looks_north`):
        // the post in strip column 180 (x/w = 180.5/360) stands at compass azimuth 0.5 at
        // Turn 0, the band's x/w being the map's u = 0.5 + azimuth/360; outside the band is 0.
        let (w, h) = (360, 4);
        let mut t = vec![[0.0; 4]; w * h];
        for y in 0..h { t[y * w + 180] = [1.0, 1.0, 1.0, 1.0]; }
        let b = SkylineBand { w, h, texels: t, top_deg: 2.0, bottom_deg: -2.0 };
        assert!(b.sample(dir(0.5, 0.0), 0.0)[3] > 0.9, "x/w = 0.5 is north");
        assert_eq!(b.sample(dir(0.5, 3.0), 0.0), [0.0; 4], "above the band");
        assert_eq!(b.sample(dir(0.5, -3.0), 0.0), [0.0; 4], "below the band");
        assert!(b.peak() == 1.0 && band(1.0, -1.0, [0.2, 0.7, 0.4, 1.0]).peak() == 0.7);
    }

    #[test]
    fn lights_and_casters_draw_over_the_skyline() {
        // A light straight ahead at elevation 10, inside the band, and a solid caster at
        // half strength to the east of it, also inside the band.
        let light = LightParams { azimuth_deg: 0.0, elevation_deg: 10.0, width_deg: 10.0, height_deg: 8.0, ..LightParams::default() };
        let caster = LightParams {
            azimuth_deg: 60.0,
            elevation_deg: 10.0,
            width_deg: 20.0,
            height_deg: 10.0,
            gobo: None,
            caster: Some(CasterParams { strength: 0.5, ..CasterParams::default() }),
            ..LightParams::new_caster()
        };
        let p = HdriParams::default();
        let lit = HdriParams { lights: vec![light, caster], ..p.clone() };
        let city_band = band(20.0, -5.0, [0.3, 0.2, 0.1, 1.0]);
        let (q, images) = with_band(&p, city_band.clone());
        let (q_lit, _) = with_band(&lit, city_band);
        let (sky, sky_lit) = (Env::new(&p), Env::new(&lit));
        let (city, city_lit) = (Env::new_with(&q, &images), Env::new_with(&q_lit, &images));
        // The light's radiance adds over the band as it adds over the sky.
        let d = dir(0.0, 10.0);
        assert!(close3(city.radiance(d), vec3f(0.3, 0.2, 0.1) * city.scale(), 1e-5), "the band is under the light");
        let over_sky = sky_lit.radiance(d) - sky.radiance(d);
        let over_city = city_lit.radiance(d) - city.radiance(d);
        assert!(over_sky.length() > 0.1, "the light draws: {over_sky:?}");
        assert!(close3(over_city, over_sky, 1e-4), "over the city {over_city:?}, over the sky {over_sky:?}");
        // A caster in front of a band texel dims it the way it dims the sky: by its transmission, 0.5.
        let d = dir(60.0, 10.0);
        let (s0, s1, c0, c1) = (sky.radiance(d), sky_lit.radiance(d), city.radiance(d), city_lit.radiance(d));
        assert!(close3(c0, vec3f(0.3, 0.2, 0.1) * city.scale(), 1e-5), "the band is under the caster");
        for (k, (a0, a1, b0, b1)) in [(s0.x, s1.x, c0.x, c1.x), (s0.y, s1.y, c0.y, c1.y), (s0.z, s1.z, c0.z, c1.z)].into_iter().enumerate() {
            let (over_sky, over_city) = (a1 / a0, b1 / b0);
            assert!((over_sky - 0.5).abs() < 1e-4 && (over_city - over_sky).abs() < 1e-4, "channel {k}: the sky passes {over_sky}, the city {over_city}");
        }
    }

    #[test]
    fn weather_fog_veils_the_skyline() {
        // Dense fog (weather.rs's `sky_with("fog")`): an emissive band texel is dimmed toward
        // the fog colour exactly as a sky texel of the same unveiled value would be.
        let mut p = HdriParams::default();
        p.sky.weather = Some(WeatherParams { kind: "fog".to_string(), ..WeatherParams::default() });
        let e = [2.0, 1.0, 0.5, 1.0];
        let (q, images) = with_band(&p, band(20.0, -5.0, e));
        let city = Env::new_with(&q, &images);
        let fog = city.weather.as_ref().expect("fog draws");
        let veil = |d: Vec3f, c: Vec3f| fog.veil(d, c, city.weather_sun, city.dome) * city.scale();
        for el in [-3.0f32, 2.0, 10.0] {
            let d = dir(30.0, el);
            let got = city.radiance(d);
            assert!(close3(got, veil(d, rgb(e)), 1e-5), "el {el}: {got:?} is the veiled texel {:?}", veil(d, rgb(e)));
            // Dimmed toward the fog's own colour (the veil over black), not drawn bare.
            let haze = veil(d, Vec3f::default());
            assert!((got - haze).length() < 0.9 * (rgb(e) * city.scale() - haze).length(), "el {el}: the fog veils the city");
        }
        // Half transparent: the band over the unveiled sky, veiled as one. The fog's veil is
        // c * T + F per channel, so the sky's unveiled value comes back from the sky's map.
        let s = [0.4, 0.2, 0.1, 0.5];
        let (q, images) = with_band(&p, band(20.0, -5.0, s));
        let (sky, city) = (Env::new(&p), Env::new_with(&q, &images));
        let d = dir(30.0, 10.0);
        let k = sky.scale();
        let (f0, f1) = (veil(d, Vec3f::default()), veil(d, Vec3f::all(1.0)));
        let seen = sky.radiance(d);
        let unveiled = vec3f((seen.x - f0.x) / (f1.x - f0.x), (seen.y - f0.y) / (f1.y - f0.y), (seen.z - f0.z) / (f1.z - f0.z));
        assert!(close3(veil(d, unveiled), seen, 1e-5), "the sky is its own unveiled value, veiled");
        let want = veil(d, unveiled * (1.0 - s[3]) + rgb(s));
        assert!(close3(city.radiance(d), want, 1e-4), "{:?} vs {want:?}", city.radiance(d));
        assert!(k == city.scale());
    }

    #[test]
    fn every_veil_is_affine() {
        // For the rain, virga and fog veils at 20 random directions:
        // veil(a*x + b*y) == a*veil(x) + b*veil(y) - (a+b-1)*veil(0), within 1e-5 relative
        // (guards over_veiled's exactness).
        let sun = dir(180.0, 40.0);
        let sky_with = |weather: WeatherParams| SkyParams { weather: Some(weather), ..SkyParams::default() };
        let kinds = [
            ("rain", WeatherParams { kind: "rain".to_string(), ..WeatherParams::default() }),
            ("storm", WeatherParams { kind: "storm".to_string(), ..WeatherParams::default() }),
            ("snow", WeatherParams { kind: "snow".to_string(), ..WeatherParams::default() }),
            ("fog", WeatherParams { kind: "fog".to_string(), ..WeatherParams::default() }),
            ("rain in fog", WeatherParams { kind: "rain".to_string(), fog: 0.7, ..WeatherParams::default() }),
        ];
        let (sun_light, dome) = (vec3f(3.0, 2.8, 2.5), vec3f(0.3, 0.35, 0.5));
        let mut state = 11u32;
        for (name, params) in kinds {
            let w = Weather::new(&sky_with(params), sun).expect("this kind draws");
            let veil = |d: Vec3f, c: Vec3f| w.veil(d, c, sun_light, dome);
            for _ in 0..20 {
                // Mostly where the skyline stands (the virga hangs from the horizon up).
                let d = dir(360.0 * next(&mut state), -10.0 + 50.0 * next(&mut state));
                let mut colour = || vec3f(5.0 * next(&mut state), 5.0 * next(&mut state), 5.0 * next(&mut state));
                let (x, y) = (colour(), colour());
                let (a, b) = (-1.0 + 3.0 * next(&mut state), -1.0 + 3.0 * next(&mut state));
                let lhs = veil(d, x * a + y * b);
                let rhs = veil(d, x) * a + veil(d, y) * b - veil(d, Vec3f::default()) * (a + b - 1.0);
                assert!((lhs - rhs).length() <= 1e-5 * (1.0 + lhs.length().max(rhs.length())), "{name} at {d:?}: {lhs:?} vs {rhs:?}");
            }
        }
        // The virga is in that test: the rain's depth is a level slab (the elevation alone), so a
        // veil that changes round the horizon at one elevation is the shafts'.
        let rain = Weather::new(&sky_with(WeatherParams { kind: "rain".to_string(), ..WeatherParams::default() }), sun).unwrap();
        let through: Vec<f32> = (0..72).map(|i| rain.veil(dir(i as f32 * 5.0, 10.0), Vec3f::all(1.0), Vec3f::default(), Vec3f::default()).x).collect();
        let (lo, hi) = through.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &t| (lo.min(t), hi.max(t)));
        assert!(hi - lo > 1e-3, "the shafts darken some azimuths: {lo}..{hi}");
    }

    #[test]
    fn over_veiled_without_weather_is_the_premultiplied_over() {
        let base = vec3f(0.8, 0.4, 0.2);
        assert_eq!(over_veiled([0.0; 4], base, &|x| x), base);
        assert_eq!(over_veiled([0.1, 0.2, 0.3, 1.0], base, &|x| x), vec3f(0.1, 0.2, 0.3));
        assert!(close3(over_veiled([0.1, 0.1, 0.1, 0.25], base, &|x| x), vec3f(0.7, 0.4, 0.25), 1e-6));
        // Emission over a transparent texel adds.
        assert!(close3(over_veiled([0.5, 0.0, 0.0, 0.0], base, &|x| x), vec3f(1.3, 0.4, 0.2), 1e-6));
    }

    #[test]
    fn studio_mode_draws_no_skyline() {
        let mut p = HdriParams::default(); p.mode = "studio".into();
        let (q, images) = with_band(&p, band(20.0, -5.0, [1.0, 1.0, 1.0, 1.0]));
        assert!(!Env::new_with(&q, &images).has_skyline());
        assert!(Env::new_with(&q, &images).bake(64) == Env::new(&p).bake(64));
    }

    #[test]
    fn a_band_prints_its_size_not_its_texels() {
        let text = format!("{:?}", band(20.0, -5.0, [0.5; 4]));
        assert!(text.contains("64") && text.contains("16") && text.contains("20") && text.len() < 200, "{text}");
    }
}
