//! The sky's light on the skyline: the sun, the sky's ambient and a ring of horizon colours, which the strip render
//! takes as inputs (`Skyline::draw_view`, `request_band`). [`SkyLight::example`] is the example's own night.
//!
//! On the GPU (`SceneCtx::set_uniforms`): `sun_u`, `sun_el` (rad), `sun_rgb`, `ambient_rgb`, `lit` ([`SkyLight::lights_factor`])
//! and `design_gain` as uniforms (`gpu_gain`, and `sil` scaled by it), `host_ground`, the two rings and the host's ground as the texture
//! `ring_tex` ([`SkyLight::ring_texels`]); the scene shaders read them through `SkyScene`'s `haze_at`, `sky_at` and `sun_dir`. A host that
//! shows the night as the example's display does (`decode`) gets the band back through [`SkyLight::to_host`].
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
    /// Show the night as the example's display shows it (a host's choice; false in the example). By night
    /// the strip is shaded in the example's own colour codes: the designed colours as designed (`gpu_gain`
    /// 1) and the host's light turned into codes (`ring_texels`: / design_gain, encoded; the haze also x
    /// `night_haze`); and handed over decoded as the display decodes the example's window, times
    /// design_gain, over the host's own sky (`to_host`). By day nothing changes. Between (`night_share`)
    /// the curve runs from the identity to the sRGB decode and the gain from design_gain to 1, so the
    /// codes stay in one scale all the way (`to_code`, `from_code`). The owner's ruling (plan hdri-5
    /// Task 8): the night "like the example".
    pub decode: bool,
    /// The night haze's share of the host's horizon ring: the example's haze is this share of its sky,
    /// as its display shows them (`example_haze_share`); toward 1 by day. 1 in the example (its ring is
    /// its haze).
    pub night_haze: f32,
    /// The host's own ground round the horizon (a few degrees down, `RING` samples like the rings), which
    /// the strip's near ground takes when `host_ground`, so the band's ground is the map's and meets it
    /// without an edge (the owner's ruling: the seam invisible). Unused in the example.
    pub ground: [[f32; 3]; RING],
    pub host_ground: bool,
}

/// The example's sky at its horizon step as the codes its window shows (`DrawSkyBackdrop`, shaders.rs:
/// `#x8a5a55`; a `#x` literal is the byte over 255, written to an 8-bit UNORM swapchain as it is).
pub const BACKDROP_HORIZON: [f32; 3] = [138.0 / 255.0, 90.0 / 255.0, 85.0 / 255.0];

/// The light a display shows for code `c` (the sRGB curve); past 1, linear on (the example's codes
/// above 1 clip on its display; a host keeps the light).
pub fn srgb_decode(c: f32) -> f32 {
    if c <= 0.04045 { c.max(0.0) / 12.92 } else if c < 1.0 { ((c + 0.055) / 1.055).powf(2.4) } else { c }
}

/// The code a display shows as light `x` (`srgb_decode`'s inverse).
pub fn srgb_encode(x: f32) -> f32 {
    if x <= 0.0031308 { 12.92 * x.max(0.0) } else if x < 1.0 { 1.055 * x.powf(1.0 / 2.4) - 0.055 } else { x }
}

fn luminance(c: [f32; 3]) -> f32 { 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2] }

/// The example's haze (`HAZE_COL`, its rings) as a share of its sky at the horizon (`BACKDROP_HORIZON`),
/// both as its display shows them, by luminance: about 0.216.
pub fn example_haze_share() -> f32 {
    luminance(HAZE_COL.map(srgb_decode)) / luminance(BACKDROP_HORIZON.map(srgb_decode))
}

impl SkyLight {
    /// Today's look: night, the fixed HAZE_COL ring, no sun, no ambient, design gain 1.
    pub fn example() -> Self {
        Self { sun_u: 0.0, sun_el_deg: -90.0, sun_rgb: [0.0; 3], ambient_rgb: [0.0; 3], ring_low: [HAZE_COL; RING], ring_high: [HAZE_COL; RING],
            lights_by_day: 0.0, design_gain: 1.0, decode: false, night_haze: 1.0, ground: [[0.0; 3]; RING], host_ground: false }
    }

    /// The night's share of the look (`decode`): 0 above +2 degrees sun, 1 below -6, smooth between (the shaders'
    /// `1 - day()`).
    pub fn night_share(&self) -> f32 {
        let t = ((2.0 - self.sun_el_deg) / 8.0).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// The scale the shaders give the designed colours: `design_gain`; with `decode`, design_gain^(1 - night):
    /// 1 by night (the codes as designed).
    pub fn gpu_gain(&self) -> f32 {
        if !self.decode || self.design_gain <= 0.0 { return self.design_gain; }
        self.design_gain.powf(1.0 - self.night_share())
    }

    /// The display curve at night share n: c^(1-n) x srgb_decode(c)^n, the identity by day and the sRGB decode by night.
    fn curve(c: f32, n: f32) -> f32 {
        if c <= 0.0 { 0.0 } else if n >= 1.0 { srgb_decode(c) } else { c.powf(1.0 - n) * srgb_decode(c).powf(n) }
    }

    /// `curve`'s inverse (it is increasing, and the identity from 1 on).
    fn curve_inverse(v: f32, n: f32) -> f32 {
        if v <= 0.0 { return 0.0; }
        if v >= 1.0 { return v; }
        if n >= 1.0 { return srgb_encode(v); }
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if Self::curve(mid, n) < v { lo = mid } else { hi = mid }
        }
        0.5 * (lo + hi)
    }

    /// A host light as a code (`decode`): `gpu_gain` x curve^-1(x / design_gain). Itself by day.
    pub fn to_code(&self, x: f32) -> f32 {
        let (n, g) = (self.night_share(), self.design_gain);
        if !self.decode || n <= 0.0 || g <= 0.0 { return x; }
        self.gpu_gain() * Self::curve_inverse(x / g, n)
    }

    /// A code as the host's light (`to_code`'s inverse): design_gain x curve(y / gpu_gain).
    pub fn from_code(&self, y: f32) -> f32 {
        let (n, g) = (self.night_share(), self.design_gain);
        if !self.decode || n <= 0.0 || g <= 0.0 { return y; }
        g * Self::curve(y / self.gpu_gain(), n)
    }

    /// The haze's share of the host's ring as the shaders take it: `night_haze` by night, 1 by day.
    fn haze_share(&self) -> f32 {
        1.0 + (self.night_haze - 1.0) * self.night_share()
    }

    /// A host colour as the strip's shaders take it (`to_code`), the haze's share applied first when `haze`.
    fn gpu_colour(&self, c: [f32; 3], haze: bool) -> [f32; 3] {
        if !self.decode || self.night_share() <= 0.0 || self.design_gain <= 0.0 { return c; }
        let k = if haze { self.haze_share() } else { 1.0 };
        c.map(|x| self.to_code(k * x))
    }

    /// The host's sky at strip u and elevation `el` (rad) in codes (`to_code`, without the haze's share): its rings
    /// so turned, the low one toward the high one by +15 degrees, as the shaders' `sky_at` reads its texture.
    fn sky_code(low: &[[f32; 3]; RING], high: &[[f32; 3]; RING], u: f32, el: f32) -> [f32; 3] {
        let at = |ring: &[[f32; 3]; RING]| -> [f32; 3] {
            let x = u.rem_euclid(1.0) * RING as f32 - 0.5;
            let i0 = x.floor();
            let a = ring[(i0 as i64).rem_euclid(RING as i64) as usize];
            let b = ring[(i0 as i64 + 1).rem_euclid(RING as i64) as usize];
            let f = x - i0;
            std::array::from_fn(|c| a[c] + (b[c] - a[c]) * f)
        };
        let (a, b) = (at(low), at(high));
        let t = (el / 0.2617994).clamp(0.0, 1.0);
        std::array::from_fn(|c| a[c] + (b[c] - a[c]) * t)
    }

    /// The strip as the host takes it (`decode`): each texel laid over the host's sky in codes, turned back into
    /// the host's light (`from_code`: by night the sRGB decode, as the example's display decodes its window, times
    /// design_gain), less that sky, so laid over the host's own sky the host sees the example's look. `w` x `h`
    /// texels of the full turn, top row first, the horizon `horizon` of the way down (`ViewSpec::strip`). A no-op
    /// without `decode`, or by day.
    pub fn to_host(&self, texels: &mut [[f32; 4]], w: usize, h: usize, horizon: f64) {
        let (n, g) = (self.night_share(), self.design_gain);
        if !self.decode || n <= 0.0 || g <= 0.0 || w == 0 || texels.len() < w * h { return; }
        let (low, high) = (self.ring_low.map(|c| self.gpu_colour(c, false)), self.ring_high.map(|c| self.gpu_colour(c, false)));
        let px = std::f64::consts::TAU / w as f64;
        let rows = |y0: usize, chunk: &mut [[f32; 4]]| {
            for (k, t) in chunk.iter_mut().enumerate() {
                let (x, y) = (k % w, y0 + k / w);
                let el = ((horizon * h as f64 - (y as f64 + 0.5)) * px) as f32;
                let sky = Self::sky_code(&low, &high, (x as f32 + 0.5) / w as f32, el);
                let behind = 1.0 - t[3].clamp(0.0, 1.0);
                for c in 0..3 {
                    t[c] = (self.from_code(t[c] + behind * sky[c]) - behind * self.from_code(sky[c])).max(0.0);
                }
            }
        };
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).max(1);
        let per = h.div_ceil(threads).max(1);
        std::thread::scope(|s| {
            for (i, chunk) in texels[..w * h].chunks_mut(per * w).enumerate() {
                let rows = &rows;
                s.spawn(move || rows(i * per, chunk));
            }
        });
    }

    /// How much of the city's lights are on: 0 above +2 degrees sun, 1 below -6, smooth between,
    /// raised to at least `lights_by_day`.
    pub fn lights_factor(&self) -> f32 {
        let t = ((2.0 - self.sun_el_deg) / 8.0).clamp(0.0, 1.0);
        (t * t * (3.0 - 2.0 * t)).max(self.lights_by_day.clamp(0.0, 1.0))
    }

    /// A hash of every field's bits, for cache keys.
    pub fn hash(&self) -> u64 {
        let rings = self.ring_low.iter().chain(&self.ring_high).chain(&self.ground).flatten();
        let floats = [&self.sun_u, &self.sun_el_deg].into_iter().chain(&self.sun_rgb).chain(&self.ambient_rgb).chain(rings).chain([&self.lights_by_day, &self.design_gain, &self.night_haze]);
        hash_bits(0x5c71_19b7_0000_0002, floats.map(|x| x.to_bits() as u64).chain([self.decode as u64, self.host_ground as u64]))
    }

    /// The rings as the texels of a `RING` x 3 RGBA float texture: row 0 `ring_low`, row 1 `ring_high`, row 2
    /// `ground`, alpha 1; with `decode`, as the shaders take them (`gpu_colour`; the two sky rings are the haze).
    pub fn ring_texels(&self) -> Vec<f32> {
        let sky = self.ring_low.iter().chain(&self.ring_high).map(|c| self.gpu_colour(*c, true));
        let ground = self.ground.iter().map(|c| self.gpu_colour(*c, false));
        sky.chain(ground).flat_map(|c| [c[0], c[1], c[2], 1.0]).collect()
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
    fn the_example_opts_out_of_the_decode() {
        let l = SkyLight::example();
        assert!(!l.decode && l.night_haze == 1.0 && l.gpu_gain() == 1.0);
        let mut t = vec![[0.3, 0.2, 0.1, 0.5]; 8];
        l.to_host(&mut t, 4, 2, 0.8);
        assert_eq!(t, vec![[0.3, 0.2, 0.1, 0.5]; 8]);
        assert_eq!(l.ring_texels()[..3], HAZE_COL);
    }
    #[test]
    fn the_curves_invert_and_the_haze_share_is_the_examples() {
        for i in 0..=200 { let c = i as f32 / 100.0; assert!((srgb_encode(srgb_decode(c)) - c).abs() < 2e-6, "{c}"); }
        let k = example_haze_share();
        assert!((k - 0.216).abs() < 0.002, "{k}");
    }
    /// By night (sun below -6) with `decode`: the shaders get the designed colours as designed, and a texel laid over
    /// the host's sky in codes comes back as the display shows it; by day nothing changes.
    #[test]
    fn a_decoded_night_is_the_display_of_the_codes() {
        let g = 0.02;
        let sky = [0.004, 0.005, 0.008];
        let night = SkyLight { design_gain: g, decode: true, night_haze: 0.25, ring_low: [sky; RING], ring_high: [sky; RING], sun_el_deg: -20.0, ..SkyLight::example() };
        assert_eq!(night.gpu_gain(), 1.0);
        let ring = night.ring_texels();
        for c in 0..3 { assert!((ring[c] - srgb_encode(0.25 * sky[c] / g)).abs() < 1e-6); }
        // an opaque texel: its code decoded, x g; a clear one: nothing (the sky behind is the host's own)
        let mut t = vec![[0.5, 0.4, 0.3, 1.0], [0.0, 0.0, 0.0, 0.0]];
        night.to_host(&mut t, 2, 1, 0.5);
        for c in 0..3 { assert!((t[0][c] - g * srgb_decode([0.5, 0.4, 0.3][c])).abs() < 1e-7); }
        assert!(t[1].iter().all(|v| v.abs() < 1e-9), "{:?}", t[1]);
        let day = SkyLight { sun_el_deg: 30.0, ..night.clone() };
        assert_eq!(day.gpu_gain(), g);
        let mut d = vec![[0.5, 0.4, 0.3, 1.0]];
        day.to_host(&mut d, 1, 1, 0.5);
        assert_eq!(d, vec![[0.5, 0.4, 0.3, 1.0]]);
        assert_eq!(day.ring_texels()[..3], sky);
    }
    /// Between day and night the codes stay in one scale: a host light turned into a code and back is itself at every
    /// sun height, and the code runs from the light itself (by day) to its sRGB code / design_gain (by night).
    #[test]
    fn twilight_codes_turn_back_into_their_light() {
        let g = 0.0264;
        for el in [10.0, 2.0, 0.0, -2.0, -4.0, -6.0, -20.0] {
            let l = SkyLight { design_gain: g, decode: true, sun_el_deg: el, ..SkyLight::example() };
            for x in [0.0, 1e-5, 3e-4, 0.0035, 0.02, 0.5, 3.0] {
                let back = l.from_code(l.to_code(x));
                assert!((back - x).abs() <= 1e-4 * x + 1e-9, "sun {el}: {x} -> {} -> {back}", l.to_code(x));
            }
        }
        let night = SkyLight { design_gain: g, decode: true, sun_el_deg: -20.0, ..SkyLight::example() };
        assert!((night.to_code(0.0035) - srgb_encode(0.0035 / g)).abs() < 1e-5);
        let day = SkyLight { sun_el_deg: 20.0, ..night.clone() };
        assert_eq!(day.to_code(0.0035), 0.0035);
    }
    #[test]
    fn the_hash_sees_every_field() {
        let a = SkyLight::example();
        let mut b = a.clone(); b.ring_low[17][1] += 1e-6;
        assert_ne!(a.hash(), b.hash());
        let mut c = a.clone(); c.design_gain = 2.0;
        assert_ne!(a.hash(), c.hash());
        let mut d = a.clone(); d.decode = true;
        assert_ne!(a.hash(), d.hash());
        let mut e = a.clone(); e.night_haze = 0.5;
        assert_ne!(a.hash(), e.hash());
        let mut f = a.clone(); f.host_ground = true;
        assert_ne!(a.hash(), f.hash());
        let mut gr = a.clone(); gr.ground[3][2] = 0.1;
        assert_ne!(a.hash(), gr.hash());
    }
}
