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

/// Over the band's lowest FADE_DEG degrees its sharp texels blend into its skirt (`BandSkirt`), so
/// the strip's ground runs on below it without a seam.
pub const FADE_DEG: f32 = 1.0;

/// How far below the band its skirt reaches, degrees: its alpha (and, premultiplied, its light)
/// fades from the bottom row's to 0 over this, and the generated ground takes over.
pub const SKIRT_DEG: f32 = 30.0;

/// The skirt is the bottom row box-blurred over this many degrees of azimuth either side.
pub const SKIRT_BLUR_DEG: f32 = 15.0;

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
    /// The band alone, sharp to its bottom edge: the map reads it through `sample_skirted`. A
    /// positive turn moves the city toward a larger u (clockwise seen from above, the compass
    /// azimuth rising).
    pub fn sample(&self, d: Vec3f, turn_deg: f32) -> [f32; 4] {
        let Some((el, u)) = self.place(d, turn_deg) else { return [0.0; 4] };
        if !(el >= self.bottom_deg && el <= self.top_deg) {
            return [0.0; 4];
        }
        self.bilinear(el, u)
    }

    /// What the map draws at `d`: the band, its lowest FADE_DEG blending into the skirt, and below
    /// it the skirt fading out over SKIRT_DEG (smoothstep), continuous across the band's bottom
    /// edge. `skirt` is this band's own (`skirt()`).
    pub fn sample_skirted(&self, skirt: &BandSkirt, d: Vec3f, turn_deg: f32) -> [f32; 4] {
        let bottom = self.bottom_deg;
        // Above the band or below its skirt, nothing (before the place's arcsine and
        // arctangent, which most of a map's texels would pay for nothing). The test
        // on d.y is looser than the one on el, which decides; NaN passes to it.
        let y_of = |deg: f32| if deg.abs() >= 89.0 { deg.signum() * f32::INFINITY } else { deg.to_radians().sin() };
        let (y_top, y_low) = (y_of(self.top_deg + 0.01), y_of(bottom - SKIRT_DEG - 0.01));
        if d.y > y_top || d.y < y_low {
            return [0.0; 4];
        }
        let Some((el, u)) = self.place(d, turn_deg) else { return [0.0; 4] };
        // The negated tests also turn a NaN direction away.
        if !(el <= self.top_deg && el >= bottom - SKIRT_DEG) {
            return [0.0; 4];
        }
        if el >= bottom + FADE_DEG {
            return self.bilinear(el, u);
        }
        let ground = skirt.at(u);
        if el >= bottom {
            let sharp = self.bilinear(el, u);
            let t = smooth((el - bottom) / FADE_DEG);
            return std::array::from_fn(|c| ground[c] + (sharp[c] - ground[c]) * t);
        }
        let fade = smooth((el - (bottom - SKIRT_DEG)) / SKIRT_DEG);
        ground.map(|c| c * fade)
    }

    /// The band's skirt: its bottom row blurred (`BandSkirt`), built once with the map.
    pub fn skirt(&self) -> BandSkirt {
        BandSkirt::new(self)
    }

    /// `d`'s elevation (degrees) and the strip's u; None for a band that draws nothing.
    fn place(&self, d: Vec3f, turn_deg: f32) -> Option<(f32, f32)> {
        if self.w == 0 || self.h == 0 || self.texels.len() < self.w * self.h || !(self.top_deg - self.bottom_deg > 0.0) {
            return None;
        }
        let el = d.y.clamp(-1.0, 1.0).asin().to_degrees();
        Some((el, (ibl::dir_to_equirect_uv(arr(d))[0] - turn_deg / 360.0).rem_euclid(1.0)))
    }

    /// The bilinear read at elevation `el` (inside the band) and strip u.
    fn bilinear(&self, el: f32, u: f32) -> [f32; 4] {
        let (w, h) = (self.w, self.h);
        let span = self.top_deg - self.bottom_deg;
        let x = u * w as f32 - 0.5;
        let y = ((self.top_deg - el) / span * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let column = |i: f32| (i as i64).rem_euclid(w as i64) as usize;
        let (c0, c1) = (column(x0), column(x0 + 1.0));
        let (r0, r1) = (y0 as usize, (y0 as usize + 1).min(h - 1));
        let texel = |r: usize, c: usize| self.texels[r * w + c];
        // Lerps, not weights: four equal texels give exactly that texel back.
        lerp4(lerp4(texel(r0, c0), texel(r0, c1), fx), lerp4(texel(r1, c0), texel(r1, c1), fx), fy)
    }

    /// The largest RGB value in the band (for bloom's base_peak).
    pub fn peak(&self) -> f32 {
        self.texels.iter().fold(0.0f32, |peak, t| peak.max(t[0]).max(t[1]).max(t[2]))
    }

    /// The band's brightest texels by region, for a caster's bloom (`BandPeaks::near`), built
    /// once per map.
    pub fn peaks(&self) -> BandPeaks {
        BandPeaks::new(self)
    }
}

/// `a + (b - a) t` per channel.
fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}

/// The ground a band runs on into below its bottom row: that row (premultiplied), box-blurred over
/// SKIRT_BLUR_DEG of azimuth either side, wrapping round the turn. Built once per map, beside the
/// band's `BandPeaks`, so a map built with the band and one it is attached to later read the same.
pub struct BandSkirt {
    row: Vec<[f32; 4]>,
    /// The brightest channel of the row, for a caster's bloom (`BandPeaks::near`).
    peak: f32,
}

impl BandSkirt {
    fn new(band: &SkylineBand) -> BandSkirt {
        let (w, h) = (band.w, band.h);
        if w == 0 || h == 0 || band.texels.len() < w * h {
            return BandSkirt { row: Vec::new(), peak: 0.0 };
        }
        let bottom = &band.texels[(h - 1) * w..h * w];
        // Texels either side: SKIRT_BLUR_DEG of the turn, at least the texel itself, at most the turn.
        let half = ((SKIRT_BLUR_DEG / 360.0 * w as f32).round() as usize).min((w - 1) / 2);
        let n = (2 * half + 1) as f64;
        // A running sum in f64 round the wrapped row.
        let mut sum = [0.0f64; 4];
        for i in 0..=2 * half {
            let t = bottom[(i + w - half) % w];
            for c in 0..4 { sum[c] += t[c] as f64; }
        }
        let mut row = Vec::with_capacity(w);
        for x in 0..w {
            row.push(sum.map(|c| (c / n) as f32));
            let (out, inn) = (bottom[(x + w - half) % w], bottom[(x + half + 1) % w]);
            for c in 0..4 { sum[c] += inn[c] as f64 - out[c] as f64; }
        }
        let peak = row.iter().fold(0.0f32, |peak, t| peak.max(t[0]).max(t[1]).max(t[2]));
        BandSkirt { row, peak }
    }

    /// The skirt at strip u: linear between its two nearest texels, wrapping.
    fn at(&self, u: f32) -> [f32; 4] {
        let w = self.row.len();
        if w == 0 {
            return [0.0; 4];
        }
        let x = u * w as f32 - 0.5;
        let x0 = x.floor();
        let column = |i: f32| (i as i64).rem_euclid(w as i64) as usize;
        lerp4(self.row[column(x0)], self.row[column(x0 + 1.0)], x - x0)
    }
}

/// Texels a side of a [`BandPeaks`] level-0 block.
const PEAK_BLOCK: usize = 4;

/// Upper bounds of the band's light over its regions, for a caster's bloom (`Env::base_peak`),
/// as `composite::Peaks` holds a base image's: level 0 the brightest channel of the texels in
/// each PEAK_BLOCK x PEAK_BLOCK block, each later level the most of 2 x 2 of the one before
/// (the last block of a row or a column may hold fewer). A city light far thinner than a
/// raster cell asks only the cells round it to be sampled finely, so a few lights are kept as
/// points (`caster::BLOOM_FINE_POINTS_MAX`); the band's single brightest texel would ask it of
/// every cell over the band, and the budget would leave each its own coarse samples.
pub struct BandPeaks {
    /// (blocks a row, rows of blocks, the most of each block, row by row) per level.
    levels: Vec<(usize, usize, Vec<f32>)>,
    /// The skirt's brightest channel (`BandSkirt`), which draws below the band.
    skirt_peak: f32,
    w: usize,
    h: usize,
    top_deg: f32,
    bottom_deg: f32,
}

impl BandPeaks {
    fn new(band: &SkylineBand) -> BandPeaks {
        let (w, h) = (band.w, band.h);
        let mut peaks = BandPeaks { levels: Vec::new(), skirt_peak: 0.0, w, h, top_deg: band.top_deg, bottom_deg: band.bottom_deg };
        if w == 0 || h == 0 || band.texels.len() < w * h || !(band.top_deg - band.bottom_deg > 0.0) {
            return peaks;
        }
        let (gw, gh) = (w.div_ceil(PEAK_BLOCK), h.div_ceil(PEAK_BLOCK));
        let mut top = vec![0.0f32; gw * gh];
        for y in 0..h {
            for x in 0..w {
                let t = band.texels[y * w + x];
                // A NaN texel counts as none (max keeps the other side).
                let block = &mut top[(y / PEAK_BLOCK) * gw + x / PEAK_BLOCK];
                *block = block.max(t[0]).max(t[1]).max(t[2]);
            }
        }
        peaks.levels.push((gw, gh, top));
        peaks.skirt_peak = BandSkirt::new(band).peak;
        while let Some((lw, lh, below)) = peaks.levels.last().filter(|(lw, lh, _)| *lw > 1 || *lh > 1) {
            let (nw, nh) = (lw.div_ceil(2), lh.div_ceil(2));
            let mut next = vec![0.0f32; nw * nh];
            for y in 0..*lh {
                for x in 0..*lw {
                    let at = &mut next[(y / 2) * nw + x / 2];
                    *at = at.max(below[y * lw + x]);
                }
            }
            peaks.levels.push((nw, nh, next));
        }
        peaks
    }

    /// What the band may draw within `radius` (radians) of `d` (map frame, the band turned by
    /// `turn_deg` as `SkylineBand::sample` turns it), a bilinear read's reach included: the
    /// most any channel of its texels there holds, never less than a read there returns, and
    /// the spacing its detail asks for (radians), half its texel, as `BaseImage::peak` asks.
    /// None where no row of the band is within reach, or nothing there is lit. Below the band (its
    /// lowest FADE_DEG and its skirt's SKIRT_DEG) the skirt's brightest channel counts too, at the
    /// spacing of a quarter of its blur: the skirt is a box-blurred row, smooth but for its edges.
    pub fn near(&self, d: Vec3f, turn_deg: f32, radius: f32) -> Option<(f32, f32)> {
        let (tau, w, h) = (std::f32::consts::TAU, self.w as i64, self.h as i64);
        if self.levels.is_empty() {
            return None;
        }
        let span = (self.top_deg - self.bottom_deg).to_radians();
        let texel = (tau / self.w as f32).max(span / self.h as f32);
        let r = radius.max(0.0) + 1.5 * texel;
        let el = d.y.clamp(-1.0, 1.0).asin();
        let (top, bottom) = (self.top_deg.to_radians(), self.bottom_deg.to_radians());
        let skirt = (self.skirt_peak > 0.0 && el - r <= bottom + FADE_DEG.to_radians() && el + r >= bottom - SKIRT_DEG.to_radians())
            .then_some((self.skirt_peak, 0.25 * SKIRT_BLUR_DEG.to_radians()));
        if !(el - r <= top && el + r >= bottom) {
            return skirt;
        }
        let row = |e: f32| (((top - e) / span * self.h as f32).floor() as i64).clamp(0, h - 1);
        let (y0, y1) = (row(el + r), row(el - r));
        // The cap's reach along the band, asin(sin r / cos el), while it holds neither pole.
        let reach = r.sin() / el.cos();
        let (x0, x1) = if !(reach < 1.0) || el.abs() + r >= 0.5 * std::f32::consts::PI {
            (0, w - 1)
        } else {
            let u = ibl::dir_to_equirect_uv(arr(d))[0] - turn_deg / 360.0;
            let du = reach.asin() / tau;
            (((u - du) * self.w as f32).floor() as i64, ((u + du) * self.w as f32).floor() as i64)
        };
        // The columns as at most two runs inside 0..w (the band wraps round).
        let runs: Vec<(i64, i64)> = if x1 - x0 + 1 >= w {
            vec![(0, w - 1)]
        } else {
            let (a, b) = (x0.rem_euclid(w), x1.rem_euclid(w));
            if a <= b { vec![(a, b)] } else { vec![(a, w - 1), (0, b)] }
        };
        let (bx, by) = (|x: i64| x / PEAK_BLOCK as i64, |y: i64| y / PEAK_BLOCK as i64);
        // The level at which the region spans at most four entries a side.
        let widest = runs.iter().map(|&(a, b)| bx(b) - bx(a)).max().unwrap_or(0).max(by(y1) - by(y0));
        let mut k = 0usize;
        while (widest >> k) > 3 && k + 1 < self.levels.len() {
            k += 1;
        }
        let (lw, lh, values) = &self.levels[k];
        let (lw, lh) = (*lw as i64, *lh as i64);
        let mut most = 0.0f32;
        for y in (by(y0) >> k).min(lh - 1)..=(by(y1) >> k).min(lh - 1) {
            for &(a, b) in &runs {
                for x in (bx(a) >> k).min(lw - 1)..=(bx(b) >> k).min(lw - 1) {
                    most = most.max(values[(y * lw + x) as usize]);
                }
            }
        }
        let band = (most > 0.0).then_some((most, 0.5 * (tau / self.w as f32).min(span / self.h as f32)));
        match (band, skirt) {
            (Some((a, step)), Some((b, _))) => Some((a.max(b), step)),
            (band, skirt) => band.or(skirt),
        }
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

/// Entries of [`SkylineLight`]'s horizon rings; equals makepad_skyline::RING (the app asserts it).
pub const RING: usize = 64;

/// The ring's two elevations, degrees: just over the horizon and over the city's roofs.
pub(super) const RING_LOW_DEG: f32 = 1.0;
pub(super) const RING_HIGH_DEG: f32 = 15.0;

/// The sky's light on the city, as the skyline lights its strip by day (makepad_skyline's
/// `SkyLight`, field by field, the app adding `lights_by_day` and `design_gain`), from the map
/// itself (`Env::skyline_light`). In the strip's frame: u is the strip's x/w
/// (`SkylineBand::sample`'s, turned by the block's turn_deg), and ring entry i stands at
/// u = (i + 0.5) / RING. Map units before intensity.
#[derive(Clone, Debug, PartialEq)]
pub struct SkylineLight {
    /// Where the sun stands: the strip's u, and its elevation in degrees (below 0 at night).
    pub sun_u: f32,
    pub sun_el_deg: f32,
    /// The radiance off a white Lambertian surface facing the sun (its irradiance / pi),
    /// through the cover the key is dimmed by (clouds, volume, rain and fog), fading out with
    /// the key as the disc sets.
    pub sun_rgb: [f32; 3],
    /// The rest of the light on a white level surface (its irradiance / pi): the sky, the
    /// night's glow and moonlight, under the map's own ground gain.
    pub ambient_rgb: [f32; 3],
    /// The sky round the horizon at 1 and at 15 degrees, as the map draws it there (its
    /// clouds, volume and weather veil included), without the discs and the stars.
    pub ring_low: [[f32; 3]; RING],
    pub ring_high: [[f32; 3]; RING],
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
        assert_eq!(a.radiance(dir(30.0, -40.0)), b.radiance(dir(30.0, -40.0))); // below the band and its skirt
        // in the skirt (a band of one colour blurs to itself): the colour, fading toward the sky beneath
        let skirt = b.radiance(dir(30.0, -20.0));
        let (sky, t) = (a.radiance(dir(30.0, -20.0)), smooth(15.0 / SKIRT_DEG));
        let want = Vec3f { x: 0.25, y: 0.5, z: 0.75 } * (t * b.scale()) + sky * (1.0 - t);
        assert!((skirt - want).length() < 1e-4 * (1.0 + want.length()), "{skirt:?} {want:?}");
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

    /// A band whose bottom row varies along the turn (one lit texel in eight, half transparent between), the rest of it
    /// another colour: what the skirt tests read.
    fn a_band_with_a_lit_bottom_row(w: usize) -> SkylineBand {
        let h = 6;
        let texels = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if y < h - 1 { [0.1, 0.2, 0.3, 1.0] } else if x % 8 == 0 { [4.0, 3.0, 2.0, 1.0] } else { [0.05, 0.05, 0.1, 0.5] }
            })
            .collect();
        SkylineBand { w, h, texels, top_deg: 12.0, bottom_deg: -4.0 }
    }

    /// The band runs on into its skirt: over its lowest FADE_DEG its texels blend into the skirt, which below the band's
    /// bottom fades out over SKIRT_DEG; nothing is drawn at or below bottom - SKIRT_DEG.
    #[test]
    fn the_band_runs_on_into_its_skirt_at_its_bottom() {
        let b = band(20.0, -5.0, [1.0, 1.0, 1.0, 1.0]);
        let skirt = b.skirt();
        assert_eq!(b.sample_skirted(&skirt, dir(0.0, -5.0 + FADE_DEG + 0.01), 0.0), [1.0; 4], "above the blend, the band");
        // a band of one colour blurs to itself: no change across the bottom edge, then the fade
        assert_eq!(b.sample_skirted(&skirt, dir(0.0, -5.0 + 0.05), 0.0), [1.0; 4]);
        assert!((b.sample_skirted(&skirt, dir(0.0, -5.0 - 0.05), 0.0)[3] - 1.0).abs() < 1e-5);
        let mid = b.sample_skirted(&skirt, dir(0.0, -5.0 - 0.5 * SKIRT_DEG), 0.0);
        assert!((mid[3] - 0.5).abs() < 1e-5 && mid[0] == mid[3], "half way down, half its alpha (premultiplied): {mid:?}");
        for el in [-5.0 - SKIRT_DEG, -5.0 - SKIRT_DEG - 0.01, -60.0, -90.0] {
            assert_eq!(b.sample_skirted(&skirt, dir(0.0, el), 0.0), [0.0; 4], "at {el}");
        }
        // the band alone still stops at its edge
        assert_eq!(b.sample(dir(0.0, -5.0 - 0.05), 0.0), [0.0; 4]);
    }

    /// No seam: with a bottom row that varies, the map just above the band's bottom edge and just below it differ by
    /// less than 1e-4 (of a texel of 4), everywhere round the turn.
    #[test]
    fn the_skirt_meets_the_band_without_a_seam() {
        let b = a_band_with_a_lit_bottom_row(64);
        let skirt = b.skirt();
        let eps = 1e-3;
        for i in 0..640 {
            let u = (i as f32 + 0.37) / 640.0;
            let (above, below) = (b.sample_skirted(&skirt, dir_at_u(u, -4.0 + eps), 0.0), b.sample_skirted(&skirt, dir_at_u(u, -4.0 - eps), 0.0));
            for c in 0..4 {
                assert!((above[c] - below[c]).abs() < 1e-4, "u {u}, channel {c}: {} above, {} below", above[c], below[c]);
            }
            // and the sharp bottom row itself does differ from it there (the premise)
        }
        let sharp = b.sample(dir_at_u(0.5 / 64.0, -4.0 + 0.01), 0.0);
        assert!((sharp[0] - b.sample_skirted(&skirt, dir_at_u(0.5 / 64.0, -4.0 - eps), 0.0)[0]).abs() > 0.5, "the bottom row is not blurred: nothing to test");
    }

    /// The quick test on a direction's height (plan hdri-5 Task 8: most of a map's texels lie above the band or below
    /// its skirt and skip the arcsine) turns away nothing the band or its skirt draws: just inside the top and just
    /// above the skirt's end something is drawn, just outside nothing, round the turn; also for a band up to the zenith.
    #[test]
    fn the_quick_height_test_keeps_the_band_and_its_skirt_whole() {
        for top in [12.0f32, 89.5] {
            let b = SkylineBand { top_deg: top, ..a_band_with_a_lit_bottom_row(64) };
            let skirt = b.skirt();
            let end = b.bottom_deg - SKIRT_DEG;
            for i in 0..64 {
                let u = (i as f32 + 0.5) / 64.0;
                let at = |el: f32| b.sample_skirted(&skirt, dir_at_u(u, el), 0.0);
                // (an f32 arcsine near the zenith is good to a few hundredths of a degree)
                assert!(at(top - if top < 89.0 { 0.002 } else { 0.1 })[3] > 0.0, "top {top}, u {u}: nothing just under the band's top");
                assert!(at(end + 0.05)[3] > 0.0, "top {top}, u {u}: nothing just above the skirt's end");
                if top < 89.0 {
                    assert_eq!(at(top + 0.002), [0.0; 4], "u {u}: something just over the band's top");
                }
                assert_eq!(at(end - 0.002), [0.0; 4], "u {u}: something just under the skirt's end");
            }
        }
    }

    /// The skirt is the bottom row box-blurred over SKIRT_BLUR_DEG either side (wrapping), premultiplied: read just below
    /// the band at a texel's centre, it is the mean of that texel's neighbours.
    #[test]
    fn the_skirt_is_the_blurred_bottom_row() {
        let w = 96;
        let b = a_band_with_a_lit_bottom_row(w);
        let skirt = b.skirt();
        let half = (SKIRT_BLUR_DEG / 360.0 * w as f32).round() as i64;
        let bottom = &b.texels[(b.h - 1) * w..];
        for x in [0usize, 1, 5, 47, 95] {
            let mut want = [0.0f64; 4];
            for k in -half..=half {
                let t = bottom[(x as i64 + k).rem_euclid(w as i64) as usize];
                for c in 0..4 { want[c] += t[c] as f64 / (2 * half + 1) as f64; }
            }
            let got = b.sample_skirted(&skirt, dir_at_u((x as f32 + 0.5) / w as f32, -4.0 - 1e-4), 0.0);
            for c in 0..4 {
                assert!((got[c] as f64 - want[c]).abs() < 1e-5, "x {x}, channel {c}: {} not {}", got[c], want[c]);
            }
        }
    }

    /// In the map: below the band the skirt lies over the generated ground, the two paths (built with the band, attached
    /// later) draw it the same, and below bottom - SKIRT_DEG the ground is the sky's own.
    #[test]
    fn the_map_draws_the_skirt_under_the_band() {
        let p = HdriParams::default();
        let (q, images) = with_band(&p, a_band_with_a_lit_bottom_row(64));
        let (plain, with) = (Env::new(&p), Env::new_with(&q, &images));
        let later = Env::new(&q).with_skyline(images.skyline.clone().unwrap());
        for el in [-4.5, -10.0, -25.0] {
            let d = dir(33.0, el);
            assert!(with.radiance(d) != plain.radiance(d), "no skirt at {el}");
            assert_eq!(with.radiance(d), later.radiance(d));
        }
        for el in [-4.0 - SKIRT_DEG - 0.01, -50.0] {
            assert_eq!(with.radiance(dir(33.0, el)), plain.radiance(dir(33.0, el)), "a skirt at {el}");
        }
        assert!(with.bake(64) == later.bake(64), "attached later, the skirt is the one built with");
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

    /// The sun set by hand at `elevation_deg` and `azimuth_deg` (compass).
    fn sun_at(p: &mut HdriParams, elevation_deg: f32, azimuth_deg: f32) {
        p.sky.sun.mode = "manual".to_string();
        p.sky.sun.elevation_deg = elevation_deg;
        p.sky.sun.azimuth_deg = azimuth_deg;
    }

    #[test]
    fn an_opaque_skyline_over_the_sun_takes_the_key_away() {
        let mut p = HdriParams::default();
        sun_at(&mut p, 3.0, 0.0);
        let (q, images) = with_band(&p, band(10.0, -5.0, [0.0, 0.0, 0.0, 1.0]));
        let key = Env::new_with(&q, &images).sun();
        let open = Env::new(&p).sun().expect("the sun is up").radiance.max_elem();
        assert!(key.map_or(true, |k| k.radiance.max_elem() < 1e-3 * open), "{key:?} against {open}");
    }

    #[test]
    fn a_half_covered_sun_keeps_about_half_its_key() {
        // The band's top exactly at the sun's centre: the disc is symmetric, so half of it shows.
        let mut p = HdriParams::default();
        sun_at(&mut p, 3.0, 0.0);
        let open = Env::new(&p).sun().expect("the sun is up").radiance;
        let (q, images) = with_band(&p, band(3.0, -5.0, [0.0, 0.0, 0.0, 1.0]));
        let half = Env::new_with(&q, &images).sun().expect("half the sun shows").radiance;
        for (k, (h, o)) in [(half.x, open.x), (half.y, open.y), (half.z, open.z)].into_iter().enumerate() {
            assert!((h / o - 0.5).abs() <= 0.1, "channel {k}: {h} of {o} is {}", h / o);
        }
    }

    #[test]
    fn a_skyline_away_from_the_sun_leaves_the_key_alone() {
        let mut p = HdriParams::default();
        sun_at(&mut p, 40.0, 0.0);
        let (q, images) = with_band(&p, band(10.0, -5.0, [5.0, 4.0, 3.0, 1.0])); // a bright night city
        assert_eq!(Env::new_with(&q, &images).sun(), Env::new(&p).sun());
    }

    #[test]
    fn the_strips_emission_is_never_the_sun() {
        // A glowing transparent band over the sun's disc adds light to the map there, not to the key.
        let mut p = HdriParams::default();
        sun_at(&mut p, 3.0, 0.0);
        let (q, images) = with_band(&p, band(10.0, -5.0, [50.0, 50.0, 50.0, 0.0]));
        assert_eq!(Env::new_with(&q, &images).sun(), Env::new(&p).sun());
    }

    /// A night sky with nothing bright in it (no moon, no stars) behind a strip of `texel_deg`
    /// texels from 0 to 10 degrees, transparent but for one texel of `bright`, centred at compass
    /// azimuth 30 + texel_deg/2, elevation 5 + texel_deg/2 (its bilinear read two texels across).
    fn one_bright_texel(bright: f32, texel_deg: f32) -> (HdriParams, ImageSet, f32, f32) {
        let mut p = HdriParams::default();
        sun_at(&mut p, -20.0, 180.0);
        p.sky.night.moon = false;
        p.sky.night.stars = 0.0;
        let (w, h) = ((360.0 / texel_deg) as usize, (10.0 / texel_deg) as usize);
        let mut texels = vec![[0.0; 4]; w * h];
        // Column c is x/w = (c + 0.5)/w, azimuth (x/w - 0.5) * 360; row r spans 10 - r t .. 10 - (r + 1) t.
        let (column, row) = ((210.0 / texel_deg) as usize, (5.0 / texel_deg) as usize - 1);
        texels[row * w + column] = [bright, bright, bright, 1.0];
        let (q, images) = with_band(&p, SkylineBand { w, h, texels, top_deg: 10.0, bottom_deg: 0.0 });
        (q, images, 30.0 + 0.5 * texel_deg, 5.0 + 0.5 * texel_deg)
    }

    /// The bright light a solid 40 x 30 degree caster with 20 degrees of bloom (cells of 3.5
    /// degrees), its threshold 2 EV under `one_bright_texel`'s light, holds at nine places 0.4
    /// degrees apart (a cell in all), its left edge about 3 degrees right of the light.
    fn bloom_by_place(texel_deg: f32) -> Vec<f64> {
        use crate::hdri::caster::fixtures::caster_at;
        let bright = 64.0;
        let (mut q, images, az, el) = one_bright_texel(bright, texel_deg);
        (0..9)
            .map(|k| {
                let settings = CasterParams { bloom_strength: 1.0, bloom_length_deg: 20.0, bloom_threshold_ev: bright.log2() - 2.0, ..CasterParams::default() };
                q.lights = vec![caster_at(az + 23.0 + 0.4 * k as f32, el, 40.0, 30.0, None, settings)];
                let env = Env::new_with(&q, &images);
                let bloom = env.studio.caster(0).and_then(|c| c.bloom());
                if let Some(cell) = bloom.map(|b| b.cell()) {
                    assert!(cell > (2.0 * texel_deg + 0.5).to_radians().tan(), "the premise: a cell ({} degrees) is wider than the light", cell.atan().to_degrees());
                }
                bloom.map_or(0.0, |b| b.energy())
            })
            .collect()
    }

    #[test]
    fn a_blooming_caster_sees_a_skyline_light_finer_than_its_cells() {
        // Bloom's fine detail (`Env::base_peak`): a single city light thinner than a raster
        // cell, on a dark night, 2 EV over the threshold as caster.rs's highlight in a base
        // image is, 2 degrees across (1-degree texels) and half a degree (0.25-degree texels,
        // under the 0.22 degrees BLOOM_LIGHT_SAMPLES_MAX a side samples a cell at). At every
        // place each blooms, within 15 % of its mean, as caster.rs's storm's bolt does, and as
        // its own area: the small one a sixteenth of the large one's, within 15 %. (Read at the
        // cells' corners alone they bloomed at none of some places; with the band's single
        // brightest texel as the bound of every cell over the band, all of them asked to
        // sample finely, the budget left each its coarse samples, and the small light was
        // missed: that needs the band's light by region, `BandPeaks`.)
        let mut means = Vec::new();
        for texel_deg in [1.0, 0.25] {
            let energies = bloom_by_place(texel_deg);
            let mean = energies.iter().sum::<f64>() / energies.len() as f64;
            let by_place: Vec<String> = energies.iter().map(|e| format!("{:.3}", e / mean.max(1e-30))).collect();
            println!("a {texel_deg}-degree city light's bloom by place, of its mean {mean:.4e}: {}", by_place.join(" "));
            assert!(energies.iter().all(|&e| e > 0.0 && (e / mean - 1.0).abs() <= 0.15), "the {texel_deg}-degree light does not bloom alike as it moves: {} of the mean {mean:.4e}", by_place.join(" "));
            means.push(mean);
        }
        let ratio = means[1] / means[0];
        assert!((ratio * 16.0 - 1.0).abs() <= 0.15, "the small light blooms {ratio} of the large one's, not a sixteenth");
    }

    #[test]
    fn the_bands_peaks_bound_every_read_near_them() {
        // An odd-sized band of random texels (some dark), so blocks end short and the wrap is
        // uneven; at random places, reaches and turns, whatever the band draws within the reach
        // is never over what `near` reports, and `near` reports nothing only where it draws none.
        let mut state = 5u32;
        let (w, h) = (97, 13);
        let texels: Vec<[f32; 4]> = (0..w * h)
            .map(|_| {
                let lit = next(&mut state) < 0.3;
                let mut c = || if lit { 10.0 * next(&mut state) } else { 0.0 };
                [c(), c(), c(), 1.0]
            })
            .collect();
        let b = SkylineBand { w, h, texels, top_deg: 12.0, bottom_deg: -3.0 };
        let (peaks, skirt) = (b.peaks(), b.skirt());
        for _ in 0..300 {
            // the band and its skirt: -40 to +20 degrees
            let centre = dir(360.0 * next(&mut state), -40.0 + 60.0 * next(&mut state));
            let radius = (0.2 + 8.0 * next(&mut state)).to_radians();
            let turn = -180.0 + 360.0 * next(&mut state);
            let bound = peaks.near(centre, turn, radius);
            // Directions within the reach: the centre turned off by up to `radius` either way.
            let side = Vec3f::cross(centre, vec3f(0.0, 1.0, 0.0)).normalize();
            let up = Vec3f::cross(side, centre).normalize();
            for _ in 0..40 {
                let (a, r) = (std::f32::consts::TAU * next(&mut state), radius * next(&mut state).sqrt());
                let d = (centre * r.cos() + (side * a.cos() + up * a.sin()) * r.sin()).normalize();
                let s = b.sample_skirted(&skirt, d, turn);
                let most = s[0].max(s[1]).max(s[2]);
                match bound {
                    Some((peak, step)) => assert!(most <= peak * (1.0 + 1e-6) && step > 0.0, "{most} drawn under a bound of {peak}"),
                    None => assert!(most == 0.0, "{most} drawn where nothing is reported"),
                }
            }
        }
        // Far from the band's rows there is nothing to report.
        assert!(peaks.near(dir(0.0, 40.0), 0.0, 1f32.to_radians()).is_none());
        assert!(peaks.near(dir(0.0, -3.0 - SKIRT_DEG - 15.0), 0.0, 1f32.to_radians()).is_none(), "below the skirt (a texel of this band is 3.7 degrees)");
    }

    fn skyline_on() -> SkylineParams {
        SkylineParams { on: true, seed: 7, turn_deg: 0.0, lights_by_day: 0.0, settings: Default::default() }
    }

    /// The skies the strip's light is checked under: clear, fog, rain, a 2D deck with cirrus
    /// reaching the horizon, and the overcast and storm kinds, the sun at 25 degrees.
    fn skies() -> Vec<(&'static str, HdriParams)> {
        let mut clear = HdriParams::default();
        sun_at(&mut clear, 25.0, 140.0);
        let weather = |kind: &str| {
            let mut p = clear.clone();
            p.sky.weather = Some(WeatherParams { kind: kind.to_string(), ..WeatherParams::default() });
            p
        };
        let mut clouds = clear.clone();
        clouds.sky.clouds.coverage = 0.6;
        clouds.sky.clouds.cirrus = 0.4;
        vec![("clear", clear.clone()), ("fog", weather("fog")), ("rain", weather("rain")), ("clouds", clouds), ("overcast", weather("overcast")), ("storm", weather("storm"))]
    }

    #[test]
    fn the_ring_is_the_maps_own_horizon() {
        // Every entry of both rings is what the map draws there (lights removed, over the
        // intensity), its clouds, its weather and their veil included: the map's own lookup.
        for (name, p) in skies() {
            let l = Env::new(&p).skyline_light();
            let env = Env::new(&HdriParams { lights: vec![], intensity_ev: 0.0, rotation_deg: 0.0, ..p.clone() });
            let mut worst = 0.0f32;
            for i in 0..RING {
                let u = (i as f32 + 0.5) / RING as f32;
                for (el, ring) in [(1.0, &l.ring_low), (15.0, &l.ring_high)] {
                    let want = env.radiance(dir_at_u(u, el)) / env.scale();
                    let got = vec(ring[i]);
                    worst = worst.max((got - want).length() / want.length().max(1e-30));
                    assert!(got == want, "{name}: {i} at {el}: {got:?} vs the map's {want:?}");
                }
            }
            println!("{name}: the ring against the map, worst relative error {worst:.1e}; ring_low[0] {:?}", l.ring_low[0]);
        }
    }

    #[test]
    fn the_ring_leaves_out_the_sun_sitting_on_it() {
        // The sun's disc on a ring direction (u = 0.5/64 at 1 degree) is the sun's light, which
        // the strip takes as sun_rgb: the ring there is the sky round it.
        let mut p = HdriParams::default();
        sun_at(&mut p, 1.0, (0.5 / RING as f32 - 0.5) * 360.0 + 360.0);
        let env = Env::new(&p);
        let l = env.skyline_light();
        assert!(env.radiance(dir_at_u(0.5 / RING as f32, 1.0)).max_elem() > 100.0 * vec(l.ring_low[0]).max_elem(), "the premise: the disc is there");
        let beside = vec(l.ring_low[1]).max_elem().max(vec(l.ring_low[RING - 1]).max_elem());
        assert!(vec(l.ring_low[0]).max_elem() < 2.0 * beside, "{:?} beside {beside}", l.ring_low[0]);
    }

    #[test]
    fn the_suns_place_is_handed_over_in_the_strip_frame() {
        let mut p = HdriParams::default();
        sun_at(&mut p, 20.0, 0.0);
        let a = Env::new(&p).skyline_light();
        p.skyline = Some(SkylineParams { turn_deg: 90.0, ..skyline_on() });
        let b = Env::new(&p).skyline_light();
        assert!((a.sun_el_deg - 20.0).abs() < 0.01, "{}", a.sun_el_deg);
        // Compass azimuth 0 is the map's u = 0.5; a turn of 90 moves the city a quarter toward
        // a larger u, so in the strip's frame the sun stands a quarter back.
        assert!((a.sun_u - 0.5).abs() < 1e-4, "{}", a.sun_u);
        assert!(((a.sun_u - b.sun_u).rem_euclid(1.0) - 0.25).abs() < 1e-4, "{} then {}", a.sun_u, b.sun_u);
        // The ring turned by a quarter the same way: the strip's entry i looks where the
        // unturned strip's entry i + 16 does.
        for i in 0..RING {
            assert_eq!(b.ring_low[i], a.ring_low[(i + 16) % RING], "{i}");
            assert_eq!(b.ring_high[i], a.ring_high[(i + 16) % RING], "{i}");
        }
        assert!(a.ring_low[0] != a.ring_low[32], "the premise: the ring is not even round the horizon");
        assert_eq!((a.sun_rgb, a.ambient_rgb), (b.sun_rgb, b.ambient_rgb));
        // The map's rotation turns the band with the map, and the intensity is the map's last
        // factor: neither changes the strip's light.
        let turned = HdriParams { rotation_deg: 40.0, intensity_ev: 2.0, ..p.clone() };
        assert_eq!(Env::new(&turned).skyline_light(), b);
    }

    #[test]
    fn noon_lights_the_strip_and_night_does_not() {
        let mut p = HdriParams::default();
        sun_at(&mut p, 50.0, 0.0);
        let day = Env::new(&p).skyline_light();
        sun_at(&mut p, -20.0, 0.0);
        let night = Env::new(&p).skyline_light();
        assert!(day.sun_rgb[1] > 0.5 && day.ambient_rgb[1] > 0.05, "{day:?}");
        assert!(night.sun_rgb == [0.0; 3] && night.ambient_rgb[1] < 0.01, "{night:?}");
    }

    #[test]
    fn the_strips_sun_is_the_keys() {
        // sun_rgb is the key's light per channel (no lights stand in front of the disc here):
        // their ratio is the disc's alone, the same under every sky and at every elevation, so
        // a deck that hides the key hides the strip's sun, and the strip's sun fades over the
        // horizon with the key.
        let ratio = |p: &HdriParams| -> Option<Vec3f> {
            let env = Env::new(p);
            let (l, key) = (env.skyline_light(), env.sun());
            match key {
                Some(key) if key.radiance.max_elem() > 0.0 => Some(vec3f(l.sun_rgb[0] / key.radiance.x, l.sun_rgb[1] / key.radiance.y, l.sun_rgb[2] / key.radiance.z)),
                // No key, or one the cover hides outright (dense fog): no sun on the strip.
                _ => {
                    assert!(l.sun_rgb == [0.0; 3], "no key, yet the strip's sun is {:?}", l.sun_rgb);
                    None
                }
            }
        };
        let clear = ratio(&skies()[0].1).expect("the sun is up");
        // Thinner covers, which pass part of the sun: light fog, a drizzle, a thin deck.
        let mut thin = Vec::new();
        for (name, fog, rain, coverage) in [("light fog", 0.1, 0.0, 0.0), ("drizzle", 0.0, 0.15, 0.0), ("thin deck", 0.0, 0.0, 0.25)] {
            let mut p = skies()[0].1.clone();
            if fog > 0.0 || rain > 0.0 {
                p.sky.weather = Some(WeatherParams { fog, rain, ..WeatherParams::default() });
            }
            p.sky.clouds.coverage = coverage;
            thin.push((name, p));
        }
        let open_key = Env::new(&skies()[0].1).sun().unwrap().radiance.y;
        let partly = thin.iter().filter(|(_, p)| Env::new(p).sun().map_or(0.0, |k| k.radiance.y / open_key) > 0.05 && Env::new(p).sun().map_or(0.0, |k| k.radiance.y / open_key) < 0.95).count();
        assert!(partly >= 2, "the premise: thin covers pass part of the key ({partly} of 3)");
        for (name, p) in skies().into_iter().chain(thin) {
            let env = Env::new(&p);
            let (key, sun) = (env.sun().map_or(0.0, |k| k.radiance.y), env.skyline_light().sun_rgb[1]);
            println!("{name}: the key's green {key:.4e} ({:.3} of the open sky's), the strip's sun {sun:.4e}", key / open_key);
            if let Some(r) = ratio(&p) {
                assert!(close3(r, clear, 1e-4), "{name}: {r:?} vs the clear sky's {clear:?}");
            }
        }
        for el in [-0.5f32, -0.2, -0.1, 0.0, 0.1, 0.2, 5.0] {
            let mut p = HdriParams::default();
            sun_at(&mut p, el, 140.0);
            if let Some(r) = ratio(&p) {
                assert!(close3(r, clear, 1e-4), "at {el}: {r:?} vs {clear:?}");
            }
        }
        // A closed deck: the overcast kind's key is all but gone, and so is the strip's sun.
        let overcast = Env::new(&skies()[4].1);
        let open = Env::new(&skies()[0].1).skyline_light().sun_rgb[1];
        assert!(overcast.skyline_light().sun_rgb[1] < 0.05 * open, "{:?} under the overcast against {open}", overcast.skyline_light().sun_rgb);
    }

    #[test]
    fn the_strips_light_is_the_maps_white_ground() {
        // A white ground: sun_rgb x sin(el) + ambient_rgb is the map's ground straight down,
        // the cover's ground gain included (D7), by day, under a deck and by moonlight.
        let mut cases = Vec::new();
        for (name, el, coverage) in [("clear", 35.0, 0.0), ("cloudy", 35.0, 0.7), ("overcast clouds", 20.0, 1.0), ("moonlit", -20.0, 0.0)] {
            let mut p = HdriParams::default();
            sun_at(&mut p, el, 120.0);
            p.sky.atmosphere.ground_color = [1.0; 3];
            p.sky.clouds.coverage = coverage;
            cases.push((name, el, p));
        }
        for (name, el, p) in cases {
            let env = Env::new(&p);
            let l = env.skyline_light();
            let ground = env.radiance(dir(0.0, -90.0)) / env.scale();
            let rise = el.to_radians().sin().max(0.0);
            let lit = vec(l.sun_rgb) * rise + vec(l.ambient_rgb);
            println!("{name}: sun {:?} ambient {:?}, the ground {ground:?}, gain {:?}", l.sun_rgb, l.ambient_rgb, env.ground_gain);
            assert!(close3(lit, ground, 1e-4), "{name}: {lit:?} vs the ground {ground:?}");
        }
    }

    #[test]
    fn a_skyline_attached_later_is_the_skyline_built_with() {
        // The app's two phases: the map built without the strip, its light handed over, the
        // strip attached. A blooming caster over a city with lights in it and the low sun
        // beside it: the bake, the key and the bloom are those of the map built with the strip
        // in its ImageSet, bit for bit, also when the key was asked for before the strip came.
        use crate::hdri::caster::fixtures::caster_at;
        let mut p = HdriParams::default();
        sun_at(&mut p, 3.0, 0.0);
        let settings = CasterParams { bloom_strength: 1.0, bloom_length_deg: 15.0, bloom_threshold_ev: 3.0, ..CasterParams::default() };
        p.lights = vec![caster_at(12.0, 14.0, 20.0, 14.0, None, settings)];
        let (w, h) = (256, 24);
        let mut texels = vec![[0.05, 0.05, 0.06, 1.0]; w * h];
        for k in 0..40 {
            texels[(4 + k % 16) * w + 100 + 3 * k] = [40.0, 30.0, 20.0, 1.0];
        }
        let city = SkylineBand { w, h, texels, top_deg: 3.0, bottom_deg: -4.0 };
        let (q, images) = with_band(&p, city.clone());
        let built = Env::new_with(&q, &images);
        let bloom = |env: &Env| env.studio.caster(0).and_then(|c| c.bloom()).map(|b| b.energy());
        assert!(bloom(&built).is_some_and(|e| e > 0.0), "the premise: the caster blooms");
        let (map, key) = (built.bake(64), built.sun());
        let asked = Env::new(&q);
        let before = asked.sun();
        let later = asked.with_skyline(Arc::new(city.clone()));
        assert!(later.has_skyline());
        assert!(later.bake(64) == map, "the bake");
        assert_eq!(later.sun(), key, "the key");
        assert_ne!(before, key, "the premise: the city changes the key");
        assert_eq!(bloom(&later), bloom(&built), "the bloom");
        assert_eq!(later.skyline_light(), built.skyline_light(), "the strip's light is the base's, without the strip");
        // A block that is off draws no strip, attached or not.
        let mut off = q.clone();
        off.skyline.as_mut().unwrap().on = false;
        let unchanged = Env::new(&off).with_skyline(Arc::new(city));
        assert!(!unchanged.has_skyline() && unchanged.bake(64) == Env::new(&off).bake(64));
    }

    #[test]
    fn no_skyline_blooms_and_keys_bit_identical() {
        // A blooming caster beside the sun: a skyline block that is off, or a band with no
        // block, leaves its bloom, the bake and the key as they are.
        use crate::hdri::caster::fixtures::caster_at;
        let mut p = HdriParams::default();
        sun_at(&mut p, 30.0, 200.0);
        let settings = CasterParams { bloom_strength: 1.0, bloom_length_deg: 10.0, bloom_threshold_ev: 2.0, ..CasterParams::default() };
        p.lights = vec![caster_at(200.0, 47.0, 40.0, 30.0, None, settings)];
        let base = Env::new(&p);
        let (base_map, base_key) = (base.bake(64), base.sun());
        assert!(base.studio.caster(0).and_then(|c| c.bloom()).is_some(), "the premise: the sun blooms over the caster");
        let (mut q, images) = with_band(&p, band(20.0, -5.0, [3.0, 3.0, 3.0, 1.0]));
        q.skyline.as_mut().unwrap().on = false;
        for env in [Env::new_with(&q, &images), Env::new_with(&p, &images)] {
            assert!(!env.has_skyline());
            assert!(env.bake(64) == base_map);
            assert_eq!(env.sun(), base_key);
        }
    }

    #[test]
    fn a_band_prints_its_size_not_its_texels() {
        let text = format!("{:?}", band(20.0, -5.0, [0.5; 4]));
        assert!(text.contains("64") && text.contains("16") && text.contains("20") && text.len() < 200, "{text}");
    }
}
