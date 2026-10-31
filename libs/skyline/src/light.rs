//! The sky's light on the skyline: the sun, the sky's ambient and a ring of horizon colours, which the strip render
//! takes as inputs (`Skyline::draw_view`, `request_band`). [`SkyLight::example`] is the example's own night.
//!
//! On the GPU (`SceneCtx::set_uniforms`): `sun_u`, `sun_el` (rad), `sun_rgb`, `ambient_rgb`, `lit` ([`SkyLight::lights_factor`])
//! and `design_gain` as uniforms (`gpu_gain`, and `sil` scaled by it), `host_ground`, `wet`, the two rings and the host's ground as the texture
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
    /// The share of the host's own light (its sun and ambient) the skyline takes by night (`decode`): its light is
    /// scaled by mix(1, this, `night_share`), so by day it is the host's whole and by night a small share, which keeps
    /// the night's buildings near the example's dark (the owner's ruling, plan hdri-5 Task 8: "a share";
    /// `HOST_LIGHT_NIGHT_SHARE`). 1 in the example (its light is 0 anyway).
    pub night_light_share: f32,
    /// The sky the day's sea mirrors, at +1 and +15 degrees (`RING` samples like the rings), when it is not the rings:
    /// a host whose rings are the sky behind its weather (its rain and fog in front of the strip, laid over it by the
    /// host) hands the sky as it shows it, since the light a far sea mirrors has come through the weather to the water.
    /// None (the example, a clear sky): the sea mirrors the rings.
    pub sea_sky: Option<([[f32; 3]; RING], [[f32; 3]; RING])>,
    /// The share of the host's ground under standing water (its puddles' mean share, `Weather::puddle_share` in the
    /// engine; 1 at Wet ground 1.00): the strip's land and shore below the horizon are water by this share, the ground
    /// darkened through it and the scene mirrored by Fresnel (`DrawSkyGround`, `DrawSkyWater`). 0 in the example. Over
    /// a host's ground (`host_ground`) the host's own floor holds the water instead: the strip keeps its own ground
    /// there and gives way to the floor by this share (`leaves_land`, `floor_share`), and draws no water on its land
    /// itself (`own_wet`).
    pub wet: f32,
    /// The host's aurora at +1 and +15 degrees (`RING` samples like the rings), in the host's own units: the open and
    /// the standing water mirror it by Fresnel at night, x (1 - day), in a pass of its own (`post::DrawSkyAuroraWater`).
    /// Host light, never coded: `plan()` carries it in the host's part (`Split`; never `Unit` while it is handed), the
    /// designed part has none. None in the example. Plan hdri-7 D12.
    pub aurora_sky: Option<([[f32; 3]; RING], [[f32; 3]; RING])>,
    /// A host's beacons are points (`point_core`): a red aviation light's core is POINT_SIGMA_DEG in sigma at POINT_GAIN
    /// times its designed peak, and where that is under half a pixel of the strip (a map's strip at 2048 px and less, a
    /// few tenths of a degree a texel) it lights only the texel its centre falls in, so in a map it stays a small red
    /// point, one texel, clear of the sky behind it. False (the example): a core of 1.22 px of the view at its designed
    /// peak, as the example draws it on a screen. Plan hdri-7 Task 9.
    pub point_lights: bool,
}

/// A host's beacon (`SkyLight::point_lights`): its core's sigma, degrees (0.19 degrees at half its peak; under half a
/// pixel of the strip, at 2048 px and less, the one texel its centre falls in: the shaders' `beacon_core`), and its peak
/// over the designed one (0.07 x Lights at its core, dimmed by the haze with distance): with it the core stands over
/// the night sky behind it as the light it is, red, not a plum smudge of red light and blue sky. Plan hdri-7 Task 9.
pub const POINT_SIGMA_DEG: f32 = 0.08;
pub const POINT_GAIN: f32 = 5.0;

/// The share of a host's own light the buildings take by night (`SkyLight::night_light_share`): on the generator's
/// default night (Clear noon, the sun 20 degrees down) the facades then stand at about 5 % of the sky's brightness
/// (`band::the_decoded_night_matches_the_example_over_the_generators_night`).
pub const HOST_LIGHT_NIGHT_SHARE: f32 = 0.1;

/// How a strip is rendered for a light (`SkyLight::plan`).
#[derive(Clone, Debug, PartialEq)]
pub enum StripPlan {
    /// One render (no decode, or no host light).
    One(SkyLight),
    /// The designed part in codes, decoded, plus the host's light alone in its units (`add_light`).
    Split { designed: SkyLight, host: SkyLight },
    /// By full night with no sun: the designed part, plus the host's light for a white ambient of 1 (`unit`, which
    /// depends on the scene alone, so a host keeps it across held steps) times `ambient` (`add_scaled`).
    Unit { designed: SkyLight, unit: SkyLight, ambient: [f32; 3] },
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
            lights_by_day: 0.0, design_gain: 1.0, decode: false, night_haze: 1.0, ground: [[0.0; 3]; RING], host_ground: false, night_light_share: 1.0,
            sea_sky: None, wet: 0.0, aurora_sky: None, point_lights: false }
    }

    /// The shaders' `point_core` (`SceneCtx::set_uniforms`): with `point_lights` a beacon's core sigma (rad) and its peak
    /// over the designed one; else 0 (the example's core).
    pub fn point_core(&self) -> [f32; 2] {
        if self.point_lights { [POINT_SIGMA_DEG.to_radians(), POINT_GAIN] } else { [0.0; 2] }
    }

    /// The wetness the strip draws itself (the shaders' `wet`, the reflection run's): `wet`, but 0 over a host's ground,
    /// whose own floor holds the water (`leaves_land`), so there the band is the dry band with its floor given way.
    pub fn own_wet(&self) -> f32 {
        if self.host_ground { 0.0 } else { self.wet }
    }

    /// Whether the strip gives way to the host's floor below the horizon (`DrawSkyGround`): a host's ground
    /// (`host_ground`) under standing water (`wet` > 0). Only what stands keeps its own pixels, above the horizon; below
    /// it the strip's flat ground leaves the puddles' share clear at every depression (`floor_share`) and the host lays
    /// its puddles as its own field lays them, their water and what they mirror (the skyline at the mirrored elevation,
    /// through the host's weather), the strip's ground between them: so the band and the host's floor below it are one
    /// floor (plan hdri-6 §5), the floor's dry ground never showing through and the reflection starting right under
    /// what stands (fix wave 1, the owner's D1 applied literally). The lakes and the sea stay the strip's. The near
    /// rise's face, which stands, is a bank under the trees: by the same share its colour gives way from the host's
    /// ground to the front treeline's own, so at Wet ground 1.00 the treeline and its reflection meet at the horizon.
    pub fn leaves_land(&self) -> bool {
        self.host_ground && self.wet > 0.0
    }

    /// The share of its flat ground below the horizon (the shaders' `under_floor`) the strip leaves clear for the host's
    /// floor (the shaders' `host_wet`): the puddles' mean share (`wet`, at most 1) where it gives way (`leaves_land`),
    /// else 0; the host lays its own puddles over that floor by it. The rest it keeps, its own ground; its open water
    /// (the sea, the lakes) it keeps whole.
    pub fn floor_share(&self) -> f32 {
        if self.leaves_land() { self.wet.min(1.0) } else { 0.0 }
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

    /// The host's rings in codes (`to_code`): the sky rings as the haze (x the haze's share) and as the sky behind
    /// (without it), and the ground; built once per light (`hash`) and kept, as twilight's inverse curve is a search.
    fn codes(&self) -> std::sync::Arc<Codes> {
        static LAST: std::sync::Mutex<Option<(u64, std::sync::Arc<Codes>)>> = std::sync::Mutex::new(None);
        let key = self.hash();
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((k, c)) = last.as_ref() {
            if *k == key { return c.clone(); }
        }
        let k = self.haze_share();
        let code = |ring: &[[f32; 3]; RING], share: f32| ring.map(|c| c.map(|x| self.to_code(share * x)));
        let (sea_low, sea_high) = self.sea_rings();
        let c = std::sync::Arc::new(Codes {
            haze_low: code(&self.ring_low, k), haze_high: code(&self.ring_high, k),
            low: code(&self.ring_low, 1.0), high: code(&self.ring_high, 1.0), ground: code(&self.ground, 1.0),
            sea_low: code(sea_low, k), sea_high: code(sea_high, k),
        });
        *last = Some((key, c.clone()));
        c
    }

    /// Whether the rings go to the shaders as codes (`decode` by night, `design_gain` above 0).
    fn coded(&self) -> bool {
        self.decode && self.night_share() > 0.0 && self.design_gain > 0.0
    }

    /// The host's sky at strip u and elevation `el` (rad) from its rings `low` and `high`: the low ring toward the
    /// high one by +15 degrees, as the shaders' `sky_at` reads its texture.
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

    /// The share of the host's own light the skyline takes (`decode`): mix(1, `night_light_share`, `night_share`).
    pub fn host_share(&self) -> f32 {
        if !self.decode { return 1.0; }
        1.0 + (self.night_light_share - 1.0) * self.night_share()
    }

    /// How the strip is rendered for this light. Where the host's own light would otherwise go through the decode
    /// (`decode` by night or dusk, with a sun or an ambient) it is rendered apart, in its own units, at its night share
    /// (`host_share`): the strip is linear in its lights, so the parts make the whole. By full night without a sun the
    /// host's part is a white ambient's times the ambient (`StripPlan::Unit`), which does not change with the sky.
    pub fn plan(&self) -> StripPlan {
        let lit = self.sun_rgb.iter().chain(&self.ambient_rgb).any(|c| *c != 0.0) || self.aurora_sky.is_some();
        if !self.coded() || !lit { return StripPlan::One(self.clone()); }
        let k = self.host_share();
        // the designed part is in codes: the host's light and its aurora go to the host's part
        let designed = SkyLight { sun_rgb: [0.0; 3], ambient_rgb: [0.0; 3], aurora_sky: None, ..self.clone() };
        let host_only = |sun: [f32; 3], ambient: [f32; 3]| SkyLight {
            sun_rgb: sun, ambient_rgb: ambient, design_gain: 0.0, decode: false, night_haze: 1.0, night_light_share: 1.0, lights_by_day: 0.0,
            ring_low: [[0.0; 3]; RING], ring_high: [[0.0; 3]; RING], ground: [[0.0; 3]; RING], sea_sky: None, ..self.clone()
        };
        if self.night_share() >= 1.0 && self.sun_rgb == [0.0; 3] && self.aurora_sky.is_none() {
            // the sun's height does not reach the host's part by full night (the shaders' day() is 0): one unit for any
            let unit = SkyLight { sun_u: 0.0, sun_el_deg: -90.0, ..host_only([0.0; 3], [1.0; 3]) };
            return StripPlan::Unit { designed, unit, ambient: self.ambient_rgb.map(|c| c * k) };
        }
        StripPlan::Split { designed, host: host_only(self.sun_rgb.map(|c| c * k), self.ambient_rgb.map(|c| c * k)) }
    }

    /// The strip as the host takes it (`decode`): each texel laid over the host's sky in codes, turned back into
    /// the host's light (`from_code`: by night the sRGB decode, as the example's display decodes its window, times
    /// design_gain), less that sky, so laid over the host's own sky the host sees the example's look. `w` x `h`
    /// texels of the full turn, top row first, the horizon `horizon` of the way down (`ViewSpec::strip`). A no-op
    /// without `decode`, or by day.
    pub fn to_host(&self, texels: &mut [[f32; 4]], w: usize, h: usize, horizon: f64) {
        if !self.coded() || w == 0 || texels.len() < w * h { return; }
        let codes = self.codes();
        let rows = |y0: usize, chunk: &mut [[f32; 4]]| {
            for (k, t) in chunk.iter_mut().enumerate() {
                let rgb = self.host_texel(&codes, [t[0], t[1], t[2]], t[3], (k % w, y0 + k / w), (w, h), horizon);
                t[..3].copy_from_slice(&rgb);
            }
        };
        let texels = &mut texels[..w * h];
        if w * h < crate::export::PAR_MIN {
            return rows(0, texels);
        }
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).max(1);
        let per = h.div_ceil(threads).max(1);
        std::thread::scope(|s| {
            for (i, chunk) in texels.chunks_mut(per * w).enumerate() {
                let rows = &rows;
                s.spawn(move || rows(i * per, chunk));
            }
        });
    }

    /// Colour `rgb` of the texel at (x, y) of `w` x `h` with coverage `a` (premultiplied codes) as the host takes it
    /// (`to_host`, while `coded`): laid over the host's sky in codes, turned back into the host's light, less that sky.
    fn host_texel(&self, codes: &Codes, rgb: [f32; 3], a: f32, (x, y): (usize, usize), (w, h): (usize, usize), horizon: f64) -> [f32; 3] {
        let behind = 1.0 - a.clamp(0.0, 1.0);
        if behind <= 0.0 {
            // opaque: the sky behind does not count
            return rgb.map(|c| self.from_code(c).max(0.0));
        }
        let el = ((horizon * h as f64 - (y as f64 + 0.5)) * (std::f64::consts::TAU / w as f64)) as f32;
        if el < 0.0 && behind < 1.0 && self.floor_share() > 0.0 {
            // below the horizon where the strip gives way to the host's floor (`floor_share`): behind the texel is that
            // floor, not the sky; its own ground, in codes, as the host takes it, by its coverage
            let a = 1.0 - behind;
            return rgb.map(|c| (a * self.from_code(c / a)).max(0.0));
        }
        let sky = Self::sky_code(&codes.low, &codes.high, (x as f32 + 0.5) / w as f32, el);
        std::array::from_fn(|c| (self.from_code(rgb[c] + behind * sky[c]) - behind * self.from_code(sky[c])).max(0.0))
    }

    /// The light plane (`Band::light`, [L, L D] a texel) as the host takes it beside `to_host`'s texels, from the strip's
    /// `texels` as drawn (before `to_host`): L becomes the luminance the light adds to the texel the host takes, that
    /// texel less the one without its light (its share of the texel's luminance, at most all of it, taken off each
    /// channel, as the map splits it: `TexelLight::split`); D is kept. A no-op where `to_host` is one.
    pub fn light_to_host(&self, texels: &[[f32; 4]], light: &mut [[f32; 2]], w: usize, h: usize, horizon: f64) {
        if !self.coded() || w == 0 || texels.len() < w * h || light.len() < w * h { return; }
        let codes = self.codes();
        for (i, (t, l)) in texels[..w * h].iter().zip(&mut light[..w * h]).enumerate() {
            if !(l[0] > 0.0) { continue; }
            let all = luminance([t[0], t[1], t[2]]);
            let k = if all > 0.0 { (l[0] / all).min(1.0) } else { 0.0 };
            let host = |rgb: [f32; 3]| luminance(self.host_texel(&codes, rgb, t[3], (i % w, i / w), (w, h), horizon));
            let lit = (host([t[0], t[1], t[2]]) - host([t[0] * (1.0 - k), t[1] * (1.0 - k), t[2] * (1.0 - k)])).max(0.0);
            *l = [lit, lit * (l[1] / l[0])];
        }
    }

    /// How much of the city's lights are on: 0 above +2 degrees sun, 1 below -6, smooth between,
    /// raised to at least `lights_by_day`.
    pub fn lights_factor(&self) -> f32 {
        let t = ((2.0 - self.sun_el_deg) / 8.0).clamp(0.0, 1.0);
        (t * t * (3.0 - 2.0 * t)).max(self.lights_by_day.clamp(0.0, 1.0))
    }

    /// A hash of every field's bits, for cache keys.
    pub fn hash(&self) -> u64 {
        // (the sea's sky only when it is handed, the wetness only when there is some, the point lights only when on: a
        // light without them hashes as it did)
        let sea = self.sea_sky.iter().flat_map(|(low, high)| low.iter().chain(high)).flatten();
        let rings = self.ring_low.iter().chain(&self.ring_high).chain(&self.ground).flatten().chain(sea);
        let floats = [&self.sun_u, &self.sun_el_deg].into_iter().chain(&self.sun_rgb).chain(&self.ambient_rgb).chain(rings).chain([&self.lights_by_day, &self.design_gain, &self.night_haze, &self.night_light_share]);
        // (the aurora only when it is handed, marked so it never hashes as a sea sky of the same numbers)
        let aurora = self.aurora_sky.iter().flat_map(|(low, high)| low.iter().chain(high)).flatten().map(|x| (x.to_bits() as u64) | (1u64 << 40));
        hash_bits(0x5c71_19b7_0000_0002, floats.map(|x| x.to_bits() as u64).chain([self.decode as u64, self.host_ground as u64])
            .chain((self.wet != 0.0).then(|| self.wet.to_bits() as u64)).chain(aurora).chain(self.point_lights.then_some(1u64 << 41)))
    }

    /// The rings as the first three rows of the shaders' ring texture (`RING` wide, RGBA): row 0 `ring_low`, row 1
    /// `ring_high`, row 2 `ground`, alpha 1; with `decode` by night, in codes (`codes`; the two sky rings are the haze).
    pub fn ring_texels(&self) -> Vec<f32> {
        let rows: Vec<[f32; 3]> = if self.coded() {
            let c = self.codes();
            c.haze_low.iter().chain(&c.haze_high).chain(&c.ground).copied().collect()
        } else {
            self.ring_low.iter().chain(&self.ring_high).chain(&self.ground).copied().collect()
        };
        rows.into_iter().flat_map(|c| [c[0], c[1], c[2], 1.0]).collect()
    }

    /// The sky the day's sea mirrors: `sea_sky`, else the rings.
    fn sea_rings(&self) -> (&[[f32; 3]; RING], &[[f32; 3]; RING]) {
        match &self.sea_sky {
            Some((low, high)) => (low, high),
            None => (&self.ring_low, &self.ring_high),
        }
    }

    /// The sea's sky as the water shader's sea texture (`RING` wide, two rows, RGBA): row 0 at +1 degree, row 1 at +15,
    /// alpha 1, coded as `ring_texels` codes its sky rows. Without `sea_sky` these are `ring_texels`' first two rows
    /// exactly, so the sea mirrors what it did.
    pub fn sea_texels(&self) -> Vec<f32> {
        let rows: Vec<[f32; 3]> = if self.coded() {
            let c = self.codes();
            c.sea_low.iter().chain(&c.sea_high).copied().collect()
        } else {
            let (low, high) = self.sea_rings();
            low.iter().chain(high).copied().collect()
        };
        rows.into_iter().flat_map(|c| [c[0], c[1], c[2], 1.0]).collect()
    }

    /// The aurora as the water's aurora texture (`RING` wide, two rows, RGBA): row 0 at +1 degree, row 1 at +15, alpha 1,
    /// linear host light; None without one.
    pub fn aurora_texels(&self) -> Option<Vec<f32>> {
        self.aurora_sky.as_ref().map(|(low, high)| low.iter().chain(high).flat_map(|c| [c[0], c[1], c[2], 1.0]).collect())
    }
}

/// The host's rings in codes (`SkyLight::codes`).
struct Codes {
    haze_low: [[f32; 3]; RING], haze_high: [[f32; 3]; RING], low: [[f32; 3]; RING], high: [[f32; 3]; RING], ground: [[f32; 3]; RING],
    /// The sea's sky (`SkyLight::sea_sky`, else the rings) as the haze rows are coded.
    sea_low: [[f32; 3]; RING], sea_high: [[f32; 3]; RING],
}

/// Adds the host light's band (`StripPlan::Split`) to the designed one, texel by texel, its light only: the two are the
/// same scene, so their coverage is the same.
pub fn add_light(designed: &mut [[f32; 4]], host: &[[f32; 4]]) {
    add_scaled(designed, host, [1.0; 3]);
}

/// Adds `scale` times the host light's band (`StripPlan::Unit`: a white ambient's band x the ambient) to the designed one.
pub fn add_scaled(designed: &mut [[f32; 4]], host: &[[f32; 4]], scale: [f32; 3]) {
    for (d, h) in designed.iter_mut().zip(host) {
        for c in 0..3 { d[c] += h[c] * scale[c]; }
    }
}

/// The coast ring: at each ring sample (u = (i + 0.5) / RING) 1 on land and fading to 0 at SHORE_DEG out to sea
/// (smooth), by the land envelope (`Zones::land`; sea where the water shader's `sea` passes a half). The shaders blend
/// a host's ground and fade the day's water by it, so the land meets the sea without an edge across azimuth.
pub fn coast_ring(land: impl Fn(f64) -> f64) -> [f32; RING] {
    const N: usize = 1440;
    let on: Vec<bool> = (0..N).map(|i| land((i as f64 + 0.5) / N as f64) >= 0.175).collect();
    let mut out = [0.0f32; RING];
    if !on.iter().any(|x| *x) { return out; }
    // the circular distance (samples) to the nearest land sample, both ways round
    let mut dist = vec![f64::INFINITY; N];
    for pass in 0..2 {
        let mut last = f64::INFINITY;
        for k in 0..2 * N {
            let i = if pass == 0 { k % N } else { (2 * N - 1 - k) % N };
            last = if on[i] { 0.0 } else { last + 1.0 };
            dist[i] = dist[i].min(last);
        }
    }
    for (j, o) in out.iter_mut().enumerate() {
        let i = ((j as f64 + 0.5) / RING as f64 * N as f64) as usize % N;
        let deg = dist[i] * 360.0 / N as f64;
        let t = (1.0 - deg / SHORE_DEG).clamp(0.0, 1.0);
        *o = (t * t * (3.0 - 2.0 * t)) as f32;
    }
    out
}

/// How far out to sea a host's ground runs on and the day's water fades in from the coast (`coast_ring`), degrees.
pub const SHORE_DEG: f64 = 15.0;

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
        assert_eq!(l.wet, 0.0);
    }
    /// Plan hdri-6 §5: the strip draws its own wet ground (`own_wet`) unless it lies over a host's ground, which holds
    /// its own water: then a wet light gives way to the host's floor (`leaves_land`) by the puddles' share
    /// (`floor_share`, fix wave 1) and the strip draws no water there.
    #[test]
    fn a_hosts_wet_ground_is_the_hosts_own() {
        let wet = SkyLight { wet: 0.5, ..SkyLight::example() };
        assert_eq!((wet.own_wet(), wet.leaves_land(), wet.floor_share()), (0.5, false, 0.0), "the strip's own wet ground (the example's)");
        let host = SkyLight { host_ground: true, ..wet.clone() };
        assert_eq!((host.own_wet(), host.leaves_land(), host.floor_share()), (0.0, true, 0.5), "a host's wet ground: the host's floor holds the water");
        let dry = SkyLight { wet: 0.0, ..host.clone() };
        assert_eq!((dry.own_wet(), dry.leaves_land(), dry.floor_share()), (0.0, false, 0.0), "a host's dry ground is the strip's, as before");
        assert_eq!(SkyLight { wet: 1.5, ..host.clone() }.floor_share(), 1.0, "at most all of it");
        let nan = SkyLight { wet: f32::NAN, ..host };
        assert!(!nan.leaves_land() && nan.floor_share() == 0.0, "a NaN wetness counts as dry");
    }
    /// Plan hdri-6 fix wave 1: by a decoded night, a texel below the horizon that the strip keeps by a share where it
    /// gives way to a host's floor (`floor_share`) is its own ground by its coverage, as the host takes it: the floor
    /// is behind it, not the sky. Above the horizon, and on a host's dry ground, a texel is laid over the host's sky as
    /// before.
    #[test]
    fn a_texel_kept_by_a_share_over_the_hosts_floor_is_its_own_ground() {
        let (g, sky) = (0.02, [0.004, 0.005, 0.008]);
        let night = SkyLight { design_gain: g, decode: true, night_haze: 0.25, ring_low: [sky; RING], ring_high: [sky; RING], sun_el_deg: -20.0,
            host_ground: true, wet: 0.25, ..SkyLight::example() };
        // one column, the horizon between its two rows: above it a half-covered texel, below it the ground kept by 0.75
        let (above, below) = ([0.15, 0.1, 0.05, 0.5], [0.3, 0.225, 0.15, 0.75]);
        let mut t = vec![above, below];
        night.to_host(&mut t, 1, 2, 0.5);
        for c in 0..3 { assert!((t[1][c] - 0.75 * g * srgb_decode(below[c] / 0.75)).abs() < 1e-7, "{:?}", t[1]); }
        let dry = SkyLight { wet: 0.0, ..night.clone() };
        let mut d = vec![above, below];
        dry.to_host(&mut d, 1, 2, 0.5);
        assert_eq!(t[0], d[0], "above the horizon: over the host's sky");
        assert!((0..3).any(|c| (d[1][c] - t[1][c]).abs() > 1e-6), "a host's dry ground: over the host's sky, as before");
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
    /// Plan hdri-6 R4: the light plane goes to the host with the texels. By a decoded night a texel's light is what it
    /// adds to the texel the host takes, that texel less the one without its light (its share of the texel's
    /// luminance off each channel): more than its share of the decoded texel, the decode being convex; a light over the
    /// sky likewise over the host's sky; its distance is kept, a texel without light keeps none. By day, or without
    /// `decode`, the plane is as drawn.
    #[test]
    fn the_light_plane_goes_to_the_host_with_the_texels() {
        let (g, sky) = (0.02, [0.004, 0.005, 0.008]);
        let night = SkyLight { design_gain: g, decode: true, night_haze: 0.25, ring_low: [sky; RING], ring_high: [sky; RING], sun_el_deg: -20.0, ..SkyLight::example() };
        // an opaque window, half its luminance light; a light over the sky, all of it; a surface without light
        let texels = vec![[0.5, 0.4, 0.3, 1.0], [0.3, 0.1, 0.05, 0.0], [0.2, 0.2, 0.2, 1.0]];
        let (l0, l1) = (0.5 * luminance([0.5, 0.4, 0.3]), luminance([0.3, 0.1, 0.05]));
        let drawn = vec![[l0, l0 * 1500.0], [l1, l1 * 2200.0], [0.0, 0.0]];
        let mut light = drawn.clone();
        night.light_to_host(&texels, &mut light, 3, 1, 0.5);
        let mut host = texels.clone();
        night.to_host(&mut host, 3, 1, 0.5);
        // the opaque window: the decoded texel less the decoded half of it
        let rest = night.host_texel(&night.codes(), [0.25, 0.2, 0.15], 1.0, (0, 0), (3, 1), 0.5);
        let want = luminance([host[0][0], host[0][1], host[0][2]]) - luminance(rest);
        assert!((light[0][0] - want).abs() <= 1e-6 * want && (light[0][1] / light[0][0] - 1500.0).abs() < 1e-2, "{:?}, want {want}", light[0]);
        assert!(light[0][0] > 0.5 * luminance([host[0][0], host[0][1], host[0][2]]), "{:?}: the decode is convex", light[0]);
        // the light over the sky: all of what the host takes of it
        assert!((light[1][0] - luminance([host[1][0], host[1][1], host[1][2]])).abs() <= 1e-6, "{:?} {:?}", light[1], host[1]);
        assert!((light[1][1] / light[1][0] - 2200.0).abs() < 1e-2);
        assert_eq!(light[2], [0.0, 0.0]);
        let mut day = drawn.clone();
        SkyLight { sun_el_deg: 30.0, ..night.clone() }.light_to_host(&texels, &mut day, 3, 1, 0.5);
        assert_eq!(day, drawn, "by day the plane is as drawn");
        let mut example = drawn.clone();
        SkyLight::example().light_to_host(&texels, &mut example, 3, 1, 0.5);
        assert_eq!(example, drawn, "without decode the plane is as drawn");
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
    /// The host's own light is rendered apart where it would go through the decode (by night or dusk, with a sun or
    /// an ambient), and only there, at its night share; by full night as a unit ambient's band times the ambient.
    #[test]
    fn the_hosts_light_is_rendered_apart_at_its_share() {
        assert!(matches!(SkyLight::example().plan(), StripPlan::One(_)));
        let night = SkyLight { design_gain: 0.0264, decode: true, sun_el_deg: -20.0, night_light_share: 0.1, ..SkyLight::example() };
        assert!(matches!(night.plan(), StripPlan::One(_)), "no host light: one render");
        let lit = SkyLight { ambient_rgb: [0.002, 0.004, 0.006], ..night.clone() };
        match lit.plan() {
            StripPlan::Unit { designed, unit, ambient } => {
                assert_eq!((designed.sun_rgb, designed.ambient_rgb, designed.decode), ([0.0; 3], [0.0; 3], true));
                assert_eq!((unit.ambient_rgb, unit.decode, unit.design_gain), ([1.0; 3], false, 0.0));
                assert!(unit.ring_low.iter().chain(&unit.ring_high).chain(&unit.ground).all(|c| *c == [0.0; 3]));
                for c in 0..3 { assert!((ambient[c] - 0.1 * lit.ambient_rgb[c]).abs() < 1e-9); }
                // the unit is the same whatever the night's sun or ambient: a host keeps it
                let other = SkyLight { sun_el_deg: -30.0, sun_u: 0.3, ambient_rgb: [0.01; 3], ..lit.clone() };
                let StripPlan::Unit { unit: unit2, .. } = other.plan() else { panic!() };
                assert_eq!(unit.hash(), unit2.hash());
            }
            plan => panic!("{plan:?}"),
        }
        let dusk = SkyLight { sun_el_deg: -4.0, ..lit.clone() };
        let k = dusk.host_share();
        assert!(k > 0.1 && k < 1.0, "{k}");
        match dusk.plan() {
            StripPlan::Split { host, .. } => for c in 0..3 { assert!((host.ambient_rgb[c] - k * lit.ambient_rgb[c]).abs() < 1e-9) },
            plan => panic!("{plan:?}"),
        }
        assert!(matches!(SkyLight { sun_el_deg: 20.0, ..lit.clone() }.plan(), StripPlan::One(_)), "by day nothing is decoded");
        assert_eq!(SkyLight { sun_el_deg: 20.0, ..lit }.host_share(), 1.0);
        let mut a = vec![[0.1, 0.2, 0.3, 1.0]];
        add_scaled(&mut a, &[[0.25, 0.5, 0.03, 1.0]], [1.0, 2.0, 0.0]);
        assert_eq!(a, vec![[0.35, 1.2, 0.3, 1.0]]);
    }
    /// The coast ring: 1 on land, falling smoothly to 0 SHORE_DEG out to sea.
    #[test]
    fn the_coast_ring_falls_off_out_to_sea() {
        let land = |u: f64| if (0.25..0.75).contains(&u) { 1.0 } else { 0.0 };
        let c = coast_ring(land);
        assert_eq!(c[RING / 2], 1.0);
        assert_eq!(c[0], 0.0, "a quarter turn out to sea");
        let i = (0.75 * RING as f64) as usize; // the first sample past the coast at 0.75 (2.8 degrees out)
        assert!(c[i] > 0.5 && c[i] < 1.0, "{}", c[i]);
        assert!(c[i + 3] == 0.0, "more than SHORE_DEG out: {}", c[i + 3]);
        assert!(coast_ring(|_| 0.0).iter().all(|x| *x == 0.0));
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
        let mut sh = a.clone(); sh.night_light_share = 0.5;
        assert_ne!(a.hash(), sh.hash());
        let mut w = a.clone(); w.wet = 0.5;
        assert_ne!(a.hash(), w.hash());
        let mut pl = a.clone(); pl.point_lights = true;
        assert_ne!(a.hash(), pl.hash());
    }
    /// Plan hdri-7 Task 9: a host's beacons reach the shaders as points (`point_core`: the core's sigma in radians and the
    /// peak's gain), the example's as they were (0: its 1.22 px core).
    #[test]
    fn a_hosts_beacons_reach_the_shaders_as_points() {
        let example = SkyLight::example();
        assert_eq!(example.point_core(), [0.0; 2]);
        assert_eq!(SkyLight { point_lights: true, ..example }.point_core(), [POINT_SIGMA_DEG.to_radians(), POINT_GAIN]);
    }
    /// A dry light hashes as it did before the wet ground (plan hdri-6 §5), so a dry sky keeps its keys: the example and
    /// a day light with every other field set, against their hashes recorded at makepad 40bee3851.
    #[test]
    fn a_dry_light_hashes_as_it_did() {
        let day = SkyLight { sun_u: 0.3, sun_el_deg: 25.0, sun_rgb: [1.2, 1.1, 0.9], ambient_rgb: [0.3, 0.35, 0.45],
            ring_low: std::array::from_fn(|i| [0.2 + i as f32 / 256.0, 0.3, 0.5]), ground: [[0.1, 0.09, 0.08]; RING],
            lights_by_day: 0.25, design_gain: 2.0, decode: true, night_haze: 0.8, host_ground: true, night_light_share: 0.6,
            sea_sky: Some(([[0.5, 0.6, 0.7]; RING], [[0.3, 0.4, 0.6]; RING])), ..SkyLight::example() };
        assert_eq!(SkyLight::example().hash(), 0x90ed_383a_d3e6_f7e2, "the example");
        assert_eq!(day.hash(), 0xb5d8_4398_a902_f253, "a day light");
    }
    /// Plan hdri-7 D12: the aurora the host hands is host light, never coded. With `decode` by night the strip is split
    /// and the host's part carries it (never Unit while it is handed), the designed part none; without `decode` one render.
    /// A light without it hashes as it did; with it apart, and apart from a sea sky of the same numbers.
    #[test]
    fn the_aurora_mirror_is_host_light() {
        let ring = [[0.01f32, 0.05, 0.02]; RING];
        let night = SkyLight { design_gain: 0.0264, decode: true, sun_el_deg: -20.0, night_light_share: 0.1, ..SkyLight::example() };
        let lit = SkyLight { aurora_sky: Some((ring, ring)), ..night.clone() };
        match lit.plan() {
            StripPlan::Split { designed, host } => {
                assert_eq!(designed.aurora_sky, None, "the designed part is coded: no aurora in it");
                assert_eq!(host.aurora_sky, Some((ring, ring)));
                assert_eq!((host.decode, host.design_gain), (false, 0.0));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(SkyLight { ambient_rgb: [0.01; 3], ..lit.clone() }.plan(), StripPlan::Split { .. }), "never Unit while an aurora is handed");
        assert!(matches!(SkyLight { ambient_rgb: [0.01; 3], ..night.clone() }.plan(), StripPlan::Unit { .. }), "the premise: without it, Unit");
        assert!(matches!(SkyLight { decode: false, ..lit.clone() }.plan(), StripPlan::One(_)));
        assert_eq!(SkyLight::example().aurora_sky, None);
        assert_ne!(lit.hash(), night.hash());
        assert_ne!(lit.hash(), SkyLight { sea_sky: Some((ring, ring)), ..night.clone() }.hash());
        let texels = lit.aurora_texels().unwrap();
        assert_eq!(texels.len(), 2 * RING * 4);
        assert_eq!(&texels[..4], &[0.01, 0.05, 0.02, 1.0]);
        assert_eq!(night.aurora_texels(), None);
    }
}
