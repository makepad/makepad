//! The skyline layer: a city strip the app renders on the GPU (makepad-skyline)
//! and hands over in `ImageSet::skyline`, laid over the base under every light
//! and caster, with the weather in front of it (`Env::base_at`): the rain haze
//! and the fog veil each texel over the distance to what it shows (`dist`, the
//! city 1-3 km off, the far ranges farther; plan hdri-6 R1), never more than
//! their whole path, and the rain's shafts hang behind it (§5b). A texel
//! without a distance is veiled over the whole path (`Env::city_veil`). Its
//! lights shine through (R4): the light in a texel (`light`) keeps the light
//! the rain and the fog scatter forward round it, over the distance it burns
//! at (`Env::city_lit`, `Weather::light_through`), a light over the sky too.
//! A wet ground mirrors the strip over the same distance and does not veil it
//! again along the floor (`Env::over_city`, `Env::base_layer`), and the strip's
//! own water, whose distance is what it mirrors, is veiled up the mirror as
//! a puddle is (`SkylineBand::mirrored`, `eye_m`).
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
/// fades from the bottom row's to 0 over this, and the generated ground takes over. A short fade
/// (the owner's ruling, plan hdri-5 Task 8; 30 before): long enough that the band's ground meets
/// the generated one without an edge, short enough that nothing of the strip runs on down.
pub const SKIRT_DEG: f32 = 4.0;

/// Azimuths a map looks its ground up at to hold its skirt to it (`Env::skirt_for`): every 2.8 degrees.
pub const SKIRT_HOLD_SAMPLES: usize = 128;

/// The skirt is the bottom row box-blurred over this many degrees of azimuth either side (15
/// with the 30 degree skirt): over a few degrees, a little blur is enough to keep a single bright
/// texel of the bottom row from drawing a streak down.
pub const SKIRT_BLUR_DEG: f32 = 2.0;

/// A texel below the horizon is taken for what the strip's water mirrors (`SkylineBand::mirrored`) from MIRROR_FROM
/// times the farthest the strip's own ground reads there, wholly from MIRROR_TO times it: past what a bilinear read
/// between two rows of ground makes of their distances (a third more by the horizon at 1 degree rows), short of what the
/// water mirrors 2.5 degrees down at Size 1 (the treeline 3.5 times as far, the city ten times). makepad_skyline holds
/// its bands to the same two (its MIRROR_FROM and MIRROR_TO; the app's test keeps them equal).
pub const MIRROR_FROM: f32 = 1.5;
pub const MIRROR_TO: f32 = 2.5;

/// The skyline's strip, as the app rendered it.
#[derive(Clone, PartialEq)]
pub struct SkylineBand {
    pub w: usize,
    pub h: usize,
    /// Premultiplied linear RGBA, top row first, in the strip frame (Turn 0).
    pub texels: Vec<[f32; 4]>,
    /// The distance (m) of what each texel shows, in the layout of `texels`: the coverage-weighted mean of its surfaces
    /// (makepad_skyline's `Band::dist`), 0 where its coverage is 0. Empty: a band without a distance plane, whose every
    /// read has no distance (`sample_skirted_far`).
    pub dist: Vec<f32>,
    /// The light in each texel (makepad_skyline's `Band::light`), in the layout of `texels`: [L, L D], L the luminance
    /// (Rec. 709) of the light the strip adds there over its surfaces (windows, beacons, lamps, the glow, what its water
    /// mirrors of them), in the texel's own units, and D the light-weighted distance (m) it burns at. Empty: a band
    /// without a light plane, every texel of it surface (`sample_skirted_lit`).
    pub light: Vec<[f32; 2]>,
    /// Elevation of the top row's top edge and the bottom row's bottom edge, degrees.
    pub top_deg: f32,
    pub bottom_deg: f32,
    /// The strip's eye over its own ground, m (makepad_skyline's `ground_eye_m`: its eye height, over the nearest its
    /// off-centre columns scale the ground by): at a depression al the strip's own ground, and what stands on it, reads
    /// no farther than eye_m / tan(al), so a texel below the horizon whose distance runs well past that shows what the
    /// strip's water mirrors (`mirrored`). 0: not handed, and no texel is taken for a mirror.
    pub eye_m: f32,
}

/// The size and the range, not the texels: an `ImageSet` is printed whole.
impl std::fmt::Debug for SkylineBand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SkylineBand {{ {} x {}, {} to {} deg }}", self.w, self.h, self.top_deg, self.bottom_deg)
    }
}

impl SkylineBand {
    /// A band from a renderer's texels and distances (the app's handoff, `to_skyline_band`), made safe for the map: a
    /// channel that is not finite (NaN, an infinity) is 0, and alpha is held to 0..1; a distance that is not finite or
    /// is negative is 0. One such texel in the bottom row would otherwise poison the skirt's running sum all the way
    /// round (`BandSkirt`), and a bilinear read would carry it into the bake and its IBL. `dist` may be empty (no
    /// distance plane).
    pub fn new(w: usize, h: usize, mut texels: Vec<[f32; 4]>, mut dist: Vec<f32>, top_deg: f32, bottom_deg: f32) -> SkylineBand {
        for t in texels.iter_mut() {
            for c in t.iter_mut() {
                if !c.is_finite() {
                    *c = 0.0;
                }
            }
            t[3] = t[3].clamp(0.0, 1.0);
        }
        for d in dist.iter_mut() {
            if !(d.is_finite() && *d >= 0.0) {
                *d = 0.0;
            }
        }
        SkylineBand { w, h, texels, dist, light: Vec::new(), top_deg, bottom_deg, eye_m: 0.0 }
    }

    /// This band with the strip's eye over its ground (`eye_m`, the app's handoff); one that is not finite or not
    /// above 0 is 0 (no texel taken for a mirror).
    pub fn with_eye(mut self, eye_m: f32) -> SkylineBand {
        self.eye_m = if eye_m.is_finite() && eye_m > 0.0 { eye_m } else { 0.0 };
        self
    }

    /// How much of a texel along `d` whose distance is `far` (m) shows what the strip's water mirrors, 0..1 (plan
    /// hdri-6 fix wave 2, F4): its distance against the farthest the strip's own ground reads there, eye_m / tan(al) (al
    /// the depression of `d`, held to the band's bottom row's in its skirt, which carries that row's distance): 0 up to
    /// MIRROR_FROM times that, 1 from MIRROR_TO times it, smooth between (a texel's distance is the mean of what it
    /// shows, and a lake's edge mixes the two). 0 above the horizon and for a band without `eye_m`.
    pub fn mirrored(&self, d: Vec3f, far: f32) -> f32 {
        if !(self.eye_m > 0.0 && d.y < 0.0) {
            return 0.0;
        }
        let al = (-d.y).min(1.0).asin().min((-self.bottom_deg).to_radians());
        if !(al > 0.0) {
            return 0.0;
        }
        let ground = self.eye_m / al.tan();
        smooth((far / ground - MIRROR_FROM) / (MIRROR_TO - MIRROR_FROM))
    }

    /// This band with the light plane `light` (`SkylineBand::light`, the app's handoff), made safe as `new` makes the
    /// texels: a value that is not finite or is negative is 0, and so is a light-weighted distance without light.
    pub fn with_light(mut self, mut light: Vec<[f32; 2]>) -> SkylineBand {
        for l in light.iter_mut() {
            for c in l.iter_mut() {
                if !(c.is_finite() && *c >= 0.0) {
                    *c = 0.0;
                }
            }
            if l[0] == 0.0 {
                l[1] = 0.0;
            }
        }
        self.light = light;
        self
    }

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
        self.skirted::<false>(skirt, d, turn_deg, false).0
    }

    /// `sample_skirted` with the distance (m) of what the texel shows: the coverage-weighted mean of its surfaces
    /// (bilinear over the premultiplied distance, a D, over the bilinear alpha; in the skirt the bottom row's, blurred as
    /// its colour, and the blend between the two over FADE_DEG). None where the coverage is 0 or the band has no
    /// distance plane. The colour is `sample_skirted`'s, bit for bit.
    pub fn sample_skirted_far(&self, skirt: &BandSkirt, d: Vec3f, turn_deg: f32) -> ([f32; 4], Option<f32>) {
        let (c, far, _) = self.skirted::<true>(skirt, d, turn_deg, false);
        (c, far)
    }

    /// `sample_skirted_far` with the light in the texel (`light`, bilinear as its colour): its luminance and the distance
    /// it burns at, the light-weighted mean (plan hdri-6 R4). Over the band's lowest FADE_DEG the band's share of it,
    /// none in the skirt below (the skirt is the strip's ground, all surface). None where there is no light or the band
    /// has no light plane. The colour and the distance are `sample_skirted_far`'s, bit for bit.
    pub fn sample_skirted_lit(&self, skirt: &BandSkirt, d: Vec3f, turn_deg: f32) -> ([f32; 4], Option<f32>, Option<TexelLight>) {
        self.skirted::<true>(skirt, d, turn_deg, false)
    }

    /// `sample_skirted_lit` with the band read on either side of the horizon apart (`rows_at`): a read above it blends
    /// only rows wholly above it, one below it only rows wholly below (the row the horizon crosses holds both, the
    /// ground below the horizon with what stands above it, and is read on neither side). For a strip that leaves part
    /// of its ground below the horizon clear (makepad_skyline's `SkyLight::floor_share`): a look just over the horizon,
    /// and the puddle that mirrors it, sees what stands there, not the strip's ground or the clear under it. Elsewhere
    /// `sample_skirted_lit`.
    pub fn sample_skirted_lit_parted(&self, skirt: &BandSkirt, d: Vec3f, turn_deg: f32) -> ([f32; 4], Option<f32>, Option<TexelLight>) {
        self.skirted::<true>(skirt, d, turn_deg, true)
    }

    /// The band's own share of what the map reads along `d` (`sample_skirted`): 1 in its rows (its lowest FADE_DEG
    /// included), in its skirt the skirt's fade, 0 below the skirt, above the band, or for a band that draws nothing.
    pub fn share_at(&self, d: Vec3f) -> f32 {
        let Some((el, _)) = self.place(d, 0.0) else { return 0.0 };
        let bottom = self.bottom_deg;
        if !(el <= self.top_deg && el >= bottom - SKIRT_DEG) {
            0.0
        } else if el >= bottom {
            1.0
        } else {
            smooth((el - (bottom - SKIRT_DEG)) / SKIRT_DEG)
        }
    }

    /// `sample_skirted`, and with `FAR` the distance and the light (`sample_skirted_lit`): one body, so the colours are one.
    /// `parted`: the band read on either side of the horizon apart (`sample_skirted_lit_parted`).
    fn skirted<const FAR: bool>(&self, skirt: &BandSkirt, d: Vec3f, turn_deg: f32, parted: bool) -> ([f32; 4], Option<f32>, Option<TexelLight>) {
        let bottom = self.bottom_deg;
        // Above the band or below its skirt, nothing (before the place's arcsine and
        // arctangent, which most of a map's texels would pay for nothing). The test
        // on d.y is looser than the one on el, which decides; NaN passes to it.
        let y_of = |deg: f32| if deg.abs() >= 89.0 { deg.signum() * f32::INFINITY } else { deg.to_radians().sin() };
        let (y_top, y_low) = (y_of(self.top_deg + 0.01), y_of(bottom - SKIRT_DEG - 0.01));
        if d.y > y_top || d.y < y_low {
            return ([0.0; 4], None, None);
        }
        let Some((el, u)) = self.place(d, turn_deg) else { return ([0.0; 4], None, None) };
        // The negated tests also turn a NaN direction away.
        if !(el <= self.top_deg && el >= bottom - SKIRT_DEG) {
            return ([0.0; 4], None, None);
        }
        // The distance of premultiplied distance pd under alpha a.
        let far = |pd: f32, a: f32| (FAR && self.has_dist() && a > 0.0).then(|| pd / a);
        if el >= bottom + FADE_DEG {
            let (c, pd) = self.bilinear_far::<FAR>(el, u, parted);
            return (c, far(pd, c[3]), self.light_at::<FAR>(el, u, 1.0, parted));
        }
        let (ground, gpd) = (skirt.at(u), if FAR { skirt.dist_at(u) } else { 0.0 });
        if el >= bottom {
            let (sharp, spd) = self.bilinear_far::<FAR>(el, u, parted);
            let t = smooth((el - bottom) / FADE_DEG);
            let c: [f32; 4] = std::array::from_fn(|c| ground[c] + (sharp[c] - ground[c]) * t);
            return (c, far(gpd + (spd - gpd) * t, c[3]), self.light_at::<FAR>(el, u, t, parted));
        }
        let fade = smooth((el - (bottom - SKIRT_DEG)) / SKIRT_DEG);
        let c = ground.map(|c| c * fade);
        (c, far(gpd * fade, c[3]), None)
    }

    /// Whether the band has a distance plane (`dist`, one per texel).
    fn has_dist(&self) -> bool {
        self.dist.len() >= self.w * self.h
    }

    /// Whether the band has a light plane (`light`, one per texel).
    fn has_light(&self) -> bool {
        self.light.len() >= self.w * self.h
    }

    /// With `FAR`, the light at elevation `el` (inside the band) and strip u, read with the colour's lerps (`parted` as
    /// theirs), `share` of it (the band's share over its lowest FADE_DEG); None without light or without a light plane.
    fn light_at<const FAR: bool>(&self, el: f32, u: f32, share: f32, parted: bool) -> Option<TexelLight> {
        if !FAR || !self.has_light() {
            return None;
        }
        let w = self.w;
        let x = u * w as f32 - 0.5;
        let x0 = x.floor();
        let fx = x - x0;
        let (r0, r1, fy) = self.rows_at(el, parted);
        let column = |i: f32| (i as i64).rem_euclid(w as i64) as usize;
        let (c0, c1) = (column(x0), column(x0 + 1.0));
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let at = |k: usize| {
            let l = |r: usize, c: usize| self.light[r * w + c][k];
            lerp(lerp(l(r0, c0), l(r0, c1), fx), lerp(l(r1, c0), l(r1, c1), fx), fy)
        };
        let (lum, ld) = (at(0), at(1));
        (lum > 0.0 && ld > 0.0).then(|| TexelLight { lum: lum * share, far: ld / lum })
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
        self.bilinear_far::<false>(el, u, false).0
    }

    /// The two rows a read at elevation `el` (inside the band) blends and the weight of the lower one. `parted`: never
    /// across the horizon (`sample_skirted_lit_parted`): above it only the rows wholly above it (their bottom edges at
    /// or over it), below it only those wholly below (their top edges at or under it), the nearest of them where the
    /// read falls between.
    fn rows_at(&self, el: f32, parted: bool) -> (usize, usize, f32) {
        let h = self.h;
        let span = self.top_deg - self.bottom_deg;
        let y = ((self.top_deg - el) / span * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let y0 = y.floor();
        let (r0, r1) = (y0 as usize, (y0 as usize + 1).min(h - 1));
        let fy = y - y0;
        if !parted {
            return (r0, r1, fy);
        }
        // the horizon's place in rows: rows 0 .. above are wholly over it, below .. h wholly under it
        let at = self.top_deg / span * h as f32;
        let (above, below) = (at.floor().clamp(1.0, h as f32) as usize, at.ceil().clamp(0.0, (h - 1) as f32) as usize);
        if el >= 0.0 && r1 >= above {
            (above - 1, above - 1, 0.0)
        } else if el < 0.0 && r0 < below {
            (below, below, 0.0)
        } else {
            (r0, r1, fy)
        }
    }

    /// The bilinear read at elevation `el` (inside the band) and strip u, and with `FAR` its premultiplied distance
    /// (a D, read with the same lerps; 0 without a distance plane). `parted`: as `rows_at`.
    fn bilinear_far<const FAR: bool>(&self, el: f32, u: f32, parted: bool) -> ([f32; 4], f32) {
        let w = self.w;
        let x = u * w as f32 - 0.5;
        let x0 = x.floor();
        let fx = x - x0;
        let (r0, r1, fy) = self.rows_at(el, parted);
        let column = |i: f32| (i as i64).rem_euclid(w as i64) as usize;
        let (c0, c1) = (column(x0), column(x0 + 1.0));
        let texel = |r: usize, c: usize| self.texels[r * w + c];
        // Lerps, not weights: four equal texels give exactly that texel back.
        let colour = lerp4(lerp4(texel(r0, c0), texel(r0, c1), fx), lerp4(texel(r1, c0), texel(r1, c1), fx), fy);
        if !FAR || !self.has_dist() {
            return (colour, 0.0);
        }
        let pd = |r: usize, c: usize| self.texels[r * w + c][3] * self.dist[r * w + c];
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        (colour, lerp(lerp(pd(r0, c0), pd(r0, c1), fx), lerp(pd(r1, c0), pd(r1, c1), fx), fy))
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

/// The light in a read of the band (`SkylineBand::sample_skirted_lit`): its luminance (Rec. 709, premultiplied as the
/// texel's colour) and the distance (m) it burns at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TexelLight {
    pub lum: f32,
    pub far: f32,
}

impl TexelLight {
    /// Texel `s` split into its surface (the texel less its light, its coverage kept) and its light: the light's share
    /// of the texel's luminance, at most all of it, in the texel's colour (the plane hands the light's luminance, not
    /// its hue).
    pub fn split(&self, s: [f32; 4]) -> ([f32; 4], Vec3f) {
        let all = 0.2126 * s[0] + 0.7152 * s[1] + 0.0722 * s[2];
        let k = if all > 0.0 { (self.lum / all).min(1.0) } else { 0.0 };
        let light = vec3f(s[0] * k, s[1] * k, s[2] * k);
        ([s[0] - light.x, s[1] - light.y, s[2] - light.z, s[3]], light)
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
    /// The bottom row's premultiplied distance (a D), blurred as `row`; empty for a band without a distance plane.
    dist: Vec<f32>,
    /// The brightest channel of the row, for a caster's bloom (`BandPeaks::near`).
    peak: f32,
}

impl BandSkirt {
    fn new(band: &SkylineBand) -> BandSkirt {
        let (w, h) = (band.w, band.h);
        if w == 0 || h == 0 || band.texels.len() < w * h {
            return BandSkirt { row: Vec::new(), dist: Vec::new(), peak: 0.0 };
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
        // the distance the same way, premultiplied
        let mut dist = Vec::new();
        if band.has_dist() {
            let pd = |x: usize| (bottom[x][3] * band.dist[(h - 1) * w + x]) as f64;
            let mut sum: f64 = (0..=2 * half).map(|i| pd((i + w - half) % w)).sum();
            dist.reserve(w);
            for x in 0..w {
                dist.push((sum / n) as f32);
                sum += pd((x + half + 1) % w) - pd((x + w - half) % w);
            }
        }
        BandSkirt { row, dist, peak }
    }

    /// This skirt with each texel's light held to `bound(u)` times its alpha (luminance, Rec. 709),
    /// the colour kept: a map holds it to its own ground (`Env::skirt_for`), so what the strip runs
    /// on into is never brighter than the ground there, the sea's bright sky reflection included.
    pub fn held_to(mut self, bound: impl Fn(f32) -> f32) -> BandSkirt {
        let w = self.row.len();
        for (x, t) in self.row.iter_mut().enumerate() {
            let most = t[3] * bound((x as f32 + 0.5) / w as f32);
            let light = 0.2126 * t[0] + 0.7152 * t[1] + 0.0722 * t[2];
            if light > most {
                let k = if most > 0.0 { most / light } else { 0.0 };
                for c in 0..3 { t[c] *= k; }
            }
        }
        self.peak = self.row.iter().fold(0.0f32, |peak, t| peak.max(t[0]).max(t[1]).max(t[2]));
        self
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

    /// The skirt's premultiplied distance (a D) at strip u, read as `at` reads its colour; 0 without a distance plane.
    fn dist_at(&self, u: f32) -> f32 {
        let w = self.dist.len();
        if w == 0 {
            return 0.0;
        }
        let x = u * w as f32 - 0.5;
        let x0 = x.floor();
        let column = |i: f32| (i as i64).rem_euclid(w as i64) as usize;
        let (a, b) = (self.dist[column(x0)], self.dist[column(x0 + 1.0)]);
        a + (b - a) * (x - x0)
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
/// veil (a storm's bolt) is covered by the city as the sky is. A host may pass a veil
/// without the rain's shafts, over the texel's own distance (`Env::city_veil`); the base
/// is then the fully veiled one, and the result is veil_D(s) + (1-a)·(base - veil_D(0)):
/// with veil_D(x) = T·x + (1-T)·L, the texel's coverage veiled over its distance
/// (T·s + a·(1-T)·L) over the base as the map veils it.
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
    /// clouds and volume included) before the weather's veil and its bolt, without the discs and
    /// the stars: the map veils the strip once, in front of it (`over_veiled`).
    pub ring_low: [[f32; 3]; RING],
    pub ring_high: [[f32; 3]; RING],
    /// The map's own ground round the turn at GROUND_RING_DEG below the horizon (its base as the map
    /// draws it there, without the skyline, before the veil as the rings): what the strip's near
    /// ground takes in the app on a dry ground (a wet one hands `ground_damp`), so the band's ground,
    /// veiled, is this map's and runs into it without an edge.
    pub ground_ring: [[f32; 3]; RING],
    /// The sky round the horizon at 1 and 15 degrees as the map shows it, through its weather's veil (the bolt
    /// aside, as the rings): what the strip's sea mirrors by day, since the light a far sea mirrors has come through
    /// the rain and fog to the water. Without a weather, `ring_low` and `ring_high` exactly.
    pub sea_low: [[f32; 3]; RING],
    pub sea_high: [[f32; 3]; RING],
    /// The puddles' mean share of the map's ground (`Weather::puddle_share`; 0 without a wet ground, 1 at Wet ground
    /// 1.00). Above 0 the strip leaves this share of its flat ground clear below the horizon, at every depression
    /// (makepad_skyline's `SkyLight::floor_share`, over the map's ground), where the map lays its own puddles, their
    /// water and what they mirror, as its field lays them, the strip's ground between them (`Env::under_band`).
    pub wet: f32,
    /// The map's ground ring as `ground_ring`, read with no puddles (`Weather::ground_damp`): the strip's own ground
    /// under a wet ground, the ground between the puddles (its near rise, its floor between the puddles, its lakes'
    /// bottoms). `ground_ring` exactly when the ground is dry.
    pub ground_damp: [[f32; 3]; RING],
    /// The aurora as the map shows it (through its clouds and weather) at +1 and +15 degrees, RING samples like the rings:
    /// what the strip's water mirrors of it at night. None when no aurora draws.
    pub aurora: Option<([[f32; 3]; RING], [[f32; 3]; RING])>,
}

/// The ground ring's elevation, degrees below the horizon: inside the strip's own ground (its bottom is
/// -6.4 degrees at the defaults), where it meets the map's.
pub(super) const GROUND_RING_DEG: f32 = 4.0;

/// Smoothstep over 0..1.
fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdri::{dir_from_az_el, vec, AuroraParams, CasterParams, Env, HdriParams, ImageSet, LightParams, Sampling, SkylineParams, WeatherParams, SkyParams};
    use crate::hdri::weather::{Weather, LIGHT_FORWARD};
    use makepad_render_material::ibl;
    use std::sync::Arc;

    fn band(top: f32, bottom: f32, rgba: [f32; 4]) -> SkylineBand {
        let (w, h) = (64, 16);
        SkylineBand { w, h, texels: vec![rgba; w * h], dist: Vec::new(), light: Vec::new(), top_deg: top, bottom_deg: bottom, eye_m: 0.0 }
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
        // in the skirt (a band of one colour blurs to itself): the colour, held to the ground's light at the band's
        // bottom, half way and at the skirt's end (`Env::skirt_for`), fading toward the ground beneath
        let el = -5.0 - 0.5 * SKIRT_DEG;
        let skirt = b.radiance(dir(30.0, el));
        let light = |c: Vec3f| 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
        let held = [-5.0, el, -5.0 - SKIRT_DEG].map(|e| light(a.radiance(dir(30.0, e))) / a.scale()).into_iter().fold(f32::INFINITY, f32::min);
        let colour = Vec3f { x: 0.25, y: 0.5, z: 0.75 };
        let colour = colour * (held / light(colour)).min(1.0);
        let (sky, t) = (a.radiance(dir(30.0, el)), smooth(0.5));
        let want = colour * (t * b.scale()) + sky * (1.0 - t);
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
        SkylineBand { w, h, texels, dist: Vec::new(), light: Vec::new(), top_deg: 12.0, bottom_deg: -4.0, eye_m: 0.0 }
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
        assert!((b.sample_skirted(&skirt, dir(0.0, -5.0 - 0.005), 0.0)[3] - 1.0).abs() < 1e-5);
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
        // 640 texels: the 2 degree blur spans 9 of them
        let b = a_band_with_a_lit_bottom_row(640);
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
        let sharp = b.sample(dir_at_u(0.5 / 640.0, -4.0 + 0.01), 0.0);
        assert!((sharp[0] - b.sample_skirted(&skirt, dir_at_u(0.5 / 640.0, -4.0 - eps), 0.0)[0]).abs() > 0.5, "the bottom row is not blurred: nothing to test");
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
        for el in [-4.5, -4.0 - 0.5 * SKIRT_DEG, -4.0 - 0.9 * SKIRT_DEG] {
            let d = dir(33.0, el);
            assert!(with.radiance(d) != plain.radiance(d), "no skirt at {el}");
            assert_eq!(with.radiance(d), later.radiance(d));
        }
        for el in [-4.0 - SKIRT_DEG - 0.01, -50.0] {
            assert_eq!(with.radiance(dir(33.0, el)), plain.radiance(dir(33.0, el)), "a skirt at {el}");
        }
        assert!(with.bake(64) == later.bake(64), "attached later, the skirt is the one built with");
    }

    /// The owner's ruling (plan hdri-5 Task 8): below the band nothing is brighter than the brighter of the map's own
    /// ground there and the band's land texels. A strip whose bottom row is a bright sea (its sky reflection) over a
    /// fifth of the turn and dark land elsewhere, at noon and in the night: down from the band's bottom to under its
    /// skirt, every direction round the turn; and the land's skirt still meets the band without an edge.
    #[test]
    fn nothing_below_the_band_is_brighter_than_the_ground_or_its_land() {
        let (w, h) = (360, 8);
        let land = [0.02, 0.02, 0.02, 1.0];
        let texels = (0..w * h).map(|i| if i / w == h - 1 && (144..216).contains(&(i % w)) { [3.0, 3.2, 3.5, 1.0] } else { land }).collect();
        let band = SkylineBand { w, h, texels, dist: Vec::new(), light: Vec::new(), top_deg: 20.0, bottom_deg: -6.0, eye_m: 0.0 };
        let light = |c: Vec3f| 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
        let mut night = HdriParams::default();
        night.sky.sun.mode = "manual".to_string();
        night.sky.sun.elevation_deg = -20.0;
        // and under the weather's veil (the final review: the skirt is held to the ground before the veil, so veiled
        // once it is still no brighter than the veiled ground), the land then seen through the veil too: fog and rain on
        // dry ground; and the fog and rain kinds and a wet ground alone, whose puddles change finer than the hold's
        // looks: the hold reads the wet ground's mean (the owner's ruling), so below the band nothing is brighter than
        // the brighter of the map's ground there (a puddle showing through the skirt's fade is the map's own), the wet
        // ground's mean there and the band's land, within WET_SLACK. The map's ground is the band-free map's, or the
        // floor the banded map draws there without the band and its skirt: its puddles mirror the city, which stands in
        // front of the rain's shafts (plan hdri-6 §5b), so under a shaft they can be brighter than the band-free sky's.
        let weather = |w: WeatherParams| {
            let mut p = HdriParams::default();
            p.sky.weather = Some(w);
            p
        };
        let kind = |kind: &str| WeatherParams { kind: kind.to_string(), ..WeatherParams::default() };
        let skies = [
            ("noon", HdriParams::default(), true),
            ("night", night, true),
            ("dry fog", weather(WeatherParams { fog: 0.7, ..WeatherParams::default() }), true),
            ("dry rain", weather(WeatherParams { rain: 0.6, fog: 0.1, ..WeatherParams::default() }), true),
            ("fog", weather(kind("fog")), true),
            ("rain", weather(kind("rain")), true),
            ("wet ground", weather(WeatherParams { wet_ground: 0.7, ..WeatherParams::default() }), true),
        ];
        const WET_SLACK: f32 = 1e-3;
        let even = Sampling { bare: true, even_ground: true, ..Sampling::default() };
        for (name, p, held) in skies {
            let (q, images) = with_band(&p, band.clone());
            let (plain, with) = (Env::new(&p), Env::new_with(&q, &images));
            let land = Vec3f { x: 0.02, y: 0.02, z: 0.02 };
            let (mut worst, mut over) = (0.0f32, 0usize);
            for i in 0..=8 * (SKIRT_DEG as usize + 2) {
                let el = -6.0 - 0.01 - i as f32 * 0.125;
                for k in 0..180 {
                    let az = k as f32 * 2.0 + 0.5;
                    let (got, ground) = (light(with.radiance(dir(az, el))), light(plain.radiance(dir(az, el))));
                    let land_light = light(veil_of(&with, dir(az, el), land)) * with.scale();
                    let mean = light(plain.base_under_skyline(dir(az, el), None, even)) * plain.scale();
                    let floor = light(with.base_under_skyline(dir(az, el), None, Sampling::default())) * with.scale();
                    let most = ground.max(floor).max(mean).max(land_light);
                    let slack = if plain.weather.is_some() { WET_SLACK } else { 1e-4 };
                    if got > most * (1.0 + 1e-4) + 1e-7 {
                        over += 1;
                        worst = worst.max(got / most - 1.0);
                    }
                    assert!(!held || got <= most * (1.0 + slack) + 1e-7, "{name}: az {az}, el {el}: {got} under the band, the ground {ground} (its mean {mean}, the banded floor {floor}), its land {land_light}");
                }
            }
            println!("{name}: below the band, {over} of {} looks brighter than the ground, at most by {:.1} %", (8 * (SKIRT_DEG as usize + 2) + 1) * 180, 100.0 * worst);
            // the land's skirt meets the band: just above and just below its bottom, alike
            let (above, below) = (with.radiance(dir(20.0, -6.0 + 1e-3)), with.radiance(dir(20.0, -6.0 - 1e-3)));
            assert!((above - below).length() < 1e-4 * (1.0 + above.length()), "{above:?} {below:?}");
        }
    }

    #[test]
    fn turn_rotates_the_band() {
        let (w, h) = (360, 4);
        let mut t = vec![[0.0; 4]; w * h];
        for y in 0..h { t[y * w + 90] = [1.0, 1.0, 1.0, 1.0]; } // a post at strip u = 90.5/360
        let b = SkylineBand { w, h, texels: t, dist: Vec::new(), light: Vec::new(), top_deg: 2.0, bottom_deg: -2.0, eye_m: 0.0 };
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
        let b = SkylineBand { w, h, texels: t, dist: Vec::new(), light: Vec::new(), top_deg: 2.0, bottom_deg: -2.0, eye_m: 0.0 };
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
                // and the veil the skyline's strip gets, the shafts left out (plan hdri-6 §5b)
                let bare = |c: Vec3f| w.veil_without_virga(d, c, sun_light, dome);
                let (lhs, rhs) = (bare(x * a + y * b), bare(x) * a + bare(y) * b - bare(Vec3f::default()) * (a + b - 1.0));
                assert!((lhs - rhs).length() <= 1e-5 * (1.0 + lhs.length().max(rhs.length())), "{name} without its shafts at {d:?}: {lhs:?} vs {rhs:?}");
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
        let (q, images) = with_band(&p, SkylineBand { w, h, texels, dist: Vec::new(), light: Vec::new(), top_deg: 10.0, bottom_deg: 0.0, eye_m: 0.0 });
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
        let b = SkylineBand { w, h, texels, dist: Vec::new(), light: Vec::new(), top_deg: 12.0, bottom_deg: -3.0, eye_m: 0.0 };
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

    /// The map's veil along `d` (identity without a weather), map units before intensity.
    fn veil_of(env: &Env, d: Vec3f, c: Vec3f) -> Vec3f {
        match &env.weather {
            Some(weather) => weather.veil(d, c, env.weather_sun, env.dome),
            None => c,
        }
    }

    #[test]
    fn the_ring_is_the_maps_own_horizon() {
        // Every entry of both rings and of the ground ring is the map's own base there before the weather's veil (lights
        // removed, over the intensity; its clouds, its volume and its wet ground included, the bolt left out): the strip
        // is veiled once, by the map (`over_veiled`). Veiled, each entry is what the map draws there, the bolt aside.
        // Without a weather the ring is the map's lookup itself, bit for bit.
        for (name, p) in skies() {
            let l = Env::new(&p).skyline_light();
            let env = Env::new(&HdriParams { lights: vec![], intensity_ev: 0.0, rotation_deg: 0.0, ..p.clone() });
            let unveiled = Sampling { bare: true, unveiled: true, even_ground: true, ..Sampling::default() };
            // the map's lookup with a wet ground's puddles at their mean share (the owner's ruling: the strip's ground
            // is the wet ground's mean), veiled; without a wet ground the map's own lookup
            let even = Sampling { bare: true, even_ground: true, ..Sampling::default() };
            let dry = ["clear", "clouds", "overcast"].contains(&name);
            let mut worst = 0.0f32;
            for i in 0..RING {
                let u = (i as f32 + 0.5) / RING as f32;
                for (el, ring) in [(1.0, &l.ring_low), (15.0, &l.ring_high), (-GROUND_RING_DEG, &l.ground_ring)] {
                    let d = dir_at_u(u, el);
                    let got = vec(ring[i]);
                    assert!(got == env.base_under_skyline(d, None, unveiled), "{name}: {i} at {el}: {got:?} is not the unveiled base");
                    let bolt = env.weather.as_ref().map_or(Vec3f::default(), |weather| weather.lightning(d, 0.0));
                    let map = env.radiance(d) / env.scale();
                    let want = if el < 0.0 { env.base_under_skyline(d, None, even) } else { map };
                    if dry || el > 0.0 {
                        assert!(close3(want, map, 1e-6), "{name}: {i} at {el}: the even lookup {want:?} is not the map's {map:?}");
                    }
                    let seen = veil_of(&env, d, got) + bolt;
                    worst = worst.max((seen - want).length() / want.length().max(1e-30));
                    if env.weather.is_none() {
                        assert!(got == want, "{name}: {i} at {el}: {got:?} vs the map's {want:?}");
                    }
                    assert!(close3(seen, want, 1e-5), "{name}: {i} at {el}: veiled {seen:?} vs the map's {want:?}");
                }
            }
            // the sky the strip's sea mirrors: the map's own, as it shows it (its veil and bolt included); without a weather
            // the rings themselves, bit for bit
            for i in 0..RING {
                let u = (i as f32 + 0.5) / RING as f32;
                for (el, sea, ring) in [(1.0, &l.sea_low, &l.ring_low), (15.0, &l.sea_high, &l.ring_high)] {
                    let map = env.radiance(dir_at_u(u, el)) / env.scale();
                    assert!(close3(vec(sea[i]), map, 1e-6), "{name}: the sea's sky {i} at {el}: {:?} vs the map's {map:?}", sea[i]);
                    if env.weather.is_none() {
                        assert!(sea[i] == ring[i], "{name}: without a weather the sea's sky is the ring");
                    }
                }
            }
            println!("{name}: the veiled ring against the map, worst relative error {worst:.1e}; ring_low[0] {:?}, sea_low[0] {:?}", l.ring_low[0], l.sea_low[0]);
        }
        // the premise: under fog the veil changes the ring (the ring is not the visible horizon)
        let fog = Env::new(&skies()[1].1);
        let d = dir_at_u(0.5 / RING as f32, 1.0);
        assert!((vec(fog.skyline_light().ring_low[0]) - fog.radiance(d) / fog.scale()).length() > 1e-3 * vec(fog.skyline_light().ring_low[0]).length());
    }

    /// A band laid out as the app's strip draws it at seed 7 under a wet ground (plan hdri-6 fix wave 1), 1024 texels
    /// round the turn (the preview bake's), square texels, the horizon 44.01 rows down: clear sky, the city's
    /// silhouettes 1.4 to 3.9 degrees up with the sky between them (two columns of three), over a dark treeline (rows
    /// 40-42; both 0.04 of the strip's own ground, the damp ring `ground_damp`, as dark against it as the app's at
    /// night), the near rise in row 43 wholly above the horizon (its face a bank under the trees by `share`: the damp
    /// ground giving way to the treeline's colour), and below the horizon the strip's flat ground, which
    /// gives way to the map's floor at every depression (makepad_skyline's `under_floor`, from half a texel under the
    /// horizon): its ground kept by 1 - `share` (the strip leaves the puddles' mean share of it clear,
    /// `SkyLight::floor_share`), to the band's bottom, 6.3 degrees down. This is the strip's contract, which the GPU
    /// band tests `over_a_hosts_wet_ground_the_band_gives_way_below_the_horizon` and
    /// `over_a_hosts_wet_floor_the_near_rise_is_a_bank_under_the_trees` hold the shaders to.
    fn wet_floor_band(l: &SkylineLight, share: f32) -> SkylineBand {
        let (w, h, row) = (1024usize, 62usize, 360.0 / 1024.0);
        let top = 44.01 * row;
        let texels = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let g = vec(l.ground_damp[x * RING / w]);
                // the texel's centre in rows under the horizon, and the share of it under the horizon (`under_floor`)
                let down = y as f32 + 0.5 - 44.01;
                let floor = if down > 0.0 { (down + 0.5).min(1.0) } else { 0.0 };
                let (t, a) = match y {
                    33..=39 if (x / 8) % 3 != 0 => (g * 0.04, 1.0),
                    0..=39 => (Vec3f::default(), 0.0),
                    40..=42 => (g * 0.04, 1.0),
                    // the near rise, 0.2 to 0.35 degrees high, the trees standing on it over the rest of the texel; over
                    // a wet floor its face is a bank under the trees, the damp ground giving way to the treeline's own
                    // colour by the share (makepad_skyline's `DrawSkyGround`)
                    43 => (g * (0.04 + 0.51 * (1.0 - share)), 1.0),
                    _ => (g * (1.0 - share * floor), 1.0 - share * floor),
                };
                [t.x, t.y, t.z, a]
            })
            .collect();
        SkylineBand::new(w, h, texels, Vec::new(), top, top - h as f32 * row)
    }

    /// Plan hdri-6 fix wave 1 (I1, the owner's D1 applied literally, the controller's ruling of 2026-10-24): under a wet
    /// ground no band stands between the silhouettes and their reflection. Below the horizon the strip's flat ground
    /// gives way to the map's floor by the puddles' share at every depression, only what stands keeping its own pixels
    /// (above the horizon), and the map lays its puddles there (`Env::under_band`). A band as the app's strip draws it
    /// at seed 7 (`wet_floor_band`), the map read as the app's preview shows it: baked 1024 x 512 (the mean round the
    /// turn of each of the bake's rows) and drawn between the bake's rows. By night and at noon, at Wet ground 0.2 (the
    /// fog kind's), 0.5 and 1.00: no row below the horizon down to the reflection (2 degrees) is brighter than both the
    /// silhouettes (the mean of the rows 2 to 3 degrees up) and their reflection (the mean of the rows 2 to 3 degrees
    /// down, at least its top row) by more than STRIPE_NOISE of it. Before (fix round 1: the strip's ground kept whole
    /// down to the reflection of its near rise's top, 1.8 degrees): a band of ground between the treeline and its
    /// reflection, by night at 1.00 29 % over both (at noon 26 %; at 0.5 2.8 and 1.7 %). And at Wet ground 1.00 no row
    /// within 0.45 degrees of the horizon (the near rise and its reflection) is brighter than both the treeline right
    /// above it (the mean of the rows 0.6 to 1.2 degrees up) and its reflection right below (0.6 to 1.2 down) by more
    /// than STRIPE_NOISE: the reflected treeline starts right below what stands. Before (fix round 2): by night 88 % and
    /// at noon 85 % over both, the rise's face in the damp ground's colour, its sky sheen included (with that face a
    /// bank under the trees, still 13 % by night: the band read across the horizon, the sky behind the clear row under
    /// it showing through over the horizon and in the puddle that mirrors it).
    #[test]
    fn under_a_wet_floor_no_row_by_the_horizon_stands_out() {
        const STRIPE_NOISE: f32 = 0.01;
        // the preview's bake: 512 rows, each 180 / 512 degrees, row j's centre 90 - (j + 0.5) 180 / 512 degrees up
        let bake_row = 180.0f32 / 512.0;
        let lum = |c: Vec3f| 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
        let mut out = vec![];
        for (name, sun) in [("night", -20.0f32), ("noon", 25.0)] {
            for wet in [0.2f32, 0.5, 1.0] {
                let mut p = HdriParams { lights: vec![], ..HdriParams::default() };
                sun_at(&mut p, sun, 140.0);
                p.sky.weather = Some(WeatherParams { wet_ground: wet, ..WeatherParams::default() });
                let l = Env::new(&p).skyline_light();
                let share = l.wet;
                assert!(share > 0.0, "the premise: {name} at {wet} the strip gives way by {share}");
                let (q, images) = with_band(&p, wet_floor_band(&l, share));
                let env = Env::new_with(&q, &images);
                // each bake row's mean round the turn (the rows within 3 degrees of the horizon), and the preview's read
                // between them
                let row_mean = |j: usize| {
                    let el = 90.0 - (j as f32 + 0.5) * bake_row;
                    (0..1024).map(|i| lum(env.radiance(dir_at_u((i as f32 + 0.5) / 1024.0, el)))).sum::<f32>() / 1024.0
                };
                let rows: Vec<(usize, f32)> = (244..268).map(|j| (j, row_mean(j))).collect();
                let mean = |el: f32| {
                    let y = (90.0 - el) / bake_row - 0.5;
                    let j = y.floor() as usize;
                    let at = |j: usize| rows.iter().find(|r| r.0 == j).expect("a bake row by the horizon").1;
                    at(j) + (at(j + 1) - at(j)) * (y - j as f32)
                };
                // the silhouettes, 2 to 3 degrees up, and their reflection, 2 to 3 degrees down: each the mean of its rows,
                // the reflection at least as bright as its top row (its Fresnel term falls with depth, so the rows of the
                // reflection just above it are brighter than its mean without standing out)
                let window = |from: i32| (from..=from + 20).map(|k| mean(k as f32 * 0.05)).sum::<f32>() / 21.0;
                let (silhouettes, reflection) = (window(40), window(-60).max(mean(-2.0)));
                let (mut worst, mut at) = (f32::MIN, 0.0);
                for k in 1..40 {
                    let el = -(k as f32) * 0.05;
                    let excess = (mean(el) - silhouettes.max(reflection)) / mean(el);
                    if excess > worst { (worst, at) = (excess, el); }
                }
                // the treeline right above the near rise, 0.6 to 1.2 degrees up, and its reflection right below, 0.6 to
                // 1.2 down: no row within 0.45 degrees of the horizon (the rise and its reflection) brighter than both
                let near = |from: i32| (from..=from + 12).map(|k| mean(k as f32 * 0.05)).sum::<f32>() / 13.0;
                let (treeline, mirrored) = (near(12), near(-24));
                let (mut by, mut by_at) = (f32::MIN, 0.0);
                for k in -9..=9 {
                    let el = k as f32 * 0.05;
                    let excess = (mean(el) - treeline.max(mirrored)) / mean(el);
                    if excess > by { (by, by_at) = (excess, el); }
                }
                println!("{name}, Wet ground {wet} (share {share:.3}): the silhouettes {silhouettes:.4e}, their reflection {reflection:.4e}; the most a row stands over both {:.2} % (at {at:.2} deg); the treeline {treeline:.4e}, its reflection {mirrored:.4e}; the most a row by the horizon stands over both {:.2} % (at {by_at:.2} deg)", 100.0 * worst, 100.0 * by);
                out.push((name, wet, worst, at, by, by_at));
            }
        }
        for (name, wet, worst, at, by, by_at) in out {
            assert!(worst <= STRIPE_NOISE, "{name}, Wet ground {wet}: the row at {at:.2} degrees stands {:.1} % over both the silhouettes and their reflection", 100.0 * worst);
            // (at Wet ground 1.00, where the floor mirrors everything: below it the puddles under the horizon mirror the
            // rise and the strip's ground lies between them, both lighter than the treeline's reflection further down)
            assert!(wet < 1.0 || by <= STRIPE_NOISE, "{name}, Wet ground {wet}: the row at {by_at:.2} degrees stands {:.1} % over both the treeline right above it and its reflection right below", 100.0 * by);
        }
    }

    /// Plan hdri-6 fix wave 1 (the owner's D1, spec §5 "the band and the map's floor below it agree"): over the strip's
    /// floor the puddles keep their shapes. The strip leaves the puddles' mean share of its floor clear and cannot see
    /// them; the map lays them as its own field does along each look (`Weather::puddle_cover`, `Env::under_band`). So
    /// in a puddle the map shows the floor's water, what the map shows there without a strip, and between the puddles
    /// the strip's own ground, what an opaque strip shows. `wet_floor_band` at Wet ground 0.5 (share 0.23), by night and
    /// at noon, 4.5 degrees down (25 m off, where the puddles are drawn in full), 4096 looks round the turn: every look
    /// in a puddle (its cover within 1e-4 of 1) and every look between them (within 1e-4 of 0) within PUDDLE_TOL of
    /// those. Before:
    /// the band's floor was an even blend of the strip's ground and the floor's water, the mean share of it everywhere,
    /// and the puddles began below the band.
    #[test]
    fn under_a_wet_floor_the_puddles_keep_their_shapes_over_the_band() {
        const PUDDLE_TOL: f32 = 1e-3;
        let lum = |c: Vec3f| 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
        for (name, sun) in [("night", -20.0f32), ("noon", 25.0)] {
            let mut p = HdriParams { lights: vec![], ..HdriParams::default() };
            sun_at(&mut p, sun, 140.0);
            p.sky.weather = Some(WeatherParams { wet_ground: 0.5, ..WeatherParams::default() });
            let bare = Env::new(&p);
            let l = bare.skyline_light();
            let (q, images) = with_band(&p, wet_floor_band(&l, l.wet));
            let env = Env::new_with(&q, &images);
            let (q, images) = with_band(&p, wet_floor_band(&l, 0.0));
            let opaque = Env::new_with(&q, &images);
            let weather = env.weather.as_ref().expect("a wet ground has a weather");
            let (mut wet, mut dry, mut worst) = (0usize, 0usize, 0.0f32);
            for i in 0..4096 {
                let d = dir_at_u((i as f32 + 0.5) / 4096.0, -4.5);
                let cover = weather.puddle_cover(d, crate::hdri::weather::Puddles::Field);
                let want = if cover >= 1.0 - 1e-4 { wet += 1; bare.radiance(d) } else if cover <= 1e-4 { dry += 1; opaque.radiance(d) } else { continue };
                let (got, want) = (lum(env.radiance(d)), lum(want));
                let e = (got - want).abs() / want.max(1e-9);
                worst = worst.max(e);
                assert!(e <= PUDDLE_TOL, "{name}: the look {i} (puddle cover {cover:.3}) shows {got:.4e}, its {} {want:.4e}", if cover > 0.5 { "puddle's water" } else { "strip's own ground" });
            }
            println!("{name}: {wet} looks in a puddle show the floor's water, {dry} between them the strip's own ground, to {:.2e}", worst);
            assert!(wet >= 100 && dry >= 100, "{name}, the premise: {wet} looks in a puddle and {dry} between them");
        }
    }

    /// The owner's ruling (wet ground): the ground ring reads a wet ground's mean, not its puddles, so the strip's near
    /// ground is even round the turn. Under the rain, storm and fog kinds and a wet ground alone, adjacent entries differ
    /// by at most RING_STEP of the brighter (before: the ring alternated damp and puddle, 0.163 and 0.330 under rain:
    /// 51 %).
    #[test]
    fn the_ground_ring_is_even_under_a_wet_ground() {
        // (the storm's mirror still shows its rain shafts and its flash, which do change round the turn: 3.3 %)
        const RING_STEP: f32 = 0.05;
        let light = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let mut clear = HdriParams::default();
        sun_at(&mut clear, 25.0, 180.0);
        let kind = |kind: &str| WeatherParams { kind: kind.to_string(), ..WeatherParams::default() };
        for (name, weather) in [("rain", kind("rain")), ("storm", kind("storm")), ("fog", kind("fog")), ("wet ground", WeatherParams { wet_ground: 0.7, ..WeatherParams::default() })] {
            let mut p = clear.clone();
            p.sky.weather = Some(weather);
            let l = Env::new(&p).skyline_light();
            let lum: Vec<f32> = l.ground_ring.iter().map(|c| light(*c)).collect();
            let worst = (0..RING).map(|i| (lum[i] - lum[(i + 1) % RING]).abs() / lum[i].max(lum[(i + 1) % RING])).fold(0.0f32, f32::max);
            let (lo, hi) = (lum.iter().cloned().fold(f32::MAX, f32::min), lum.iter().cloned().fold(0.0f32, f32::max));
            println!("{name}: the ground ring {lo:.3} .. {hi:.3}, adjacent entries at most {:.2} % apart", 100.0 * worst);
            assert!(worst <= RING_STEP, "{name}: adjacent ground ring entries {:.1} % apart: {lum:?}", 100.0 * worst);
        }
    }

    /// Plan hdri-6 §5: a wet ground mirrors the skyline at the mirrored elevation. Wet ground 1.00 (the whole floor water),
    /// the night, no light layers: a band clear but for one city light 12 degrees up. At -12 degrees the map shows that
    /// light by the water's Fresnel term; 10 degrees round the turn it shows none of it.
    #[test]
    fn a_puddle_mirrors_a_city_light_above_it() {
        let mut p = HdriParams { lights: vec![], ..HdriParams::default() };
        sun_at(&mut p, -20.0, 0.0);
        p.sky.weather = Some(WeatherParams { wet_ground: 1.0, ..WeatherParams::default() });
        let (w, h, top, bottom) = (360usize, 24usize, 20.0f32, -4.0f32);
        let row = ((top - 12.0) / (top - bottom) * h as f32 - 0.5).round() as usize;
        let el = top - (row as f32 + 0.5) * (top - bottom) / h as f32;
        let mut texels = vec![[0.0f32; 4]; w * h];
        texels[row * w + 90] = [40.0, 30.0, 20.0, 0.0];
        let (q, images) = with_band(&p, SkylineBand { w, h, texels, dist: Vec::new(), light: Vec::new(), top_deg: top, bottom_deg: bottom, eye_m: 0.0 });
        let (plain, city) = (Env::new(&p), Env::new_with(&q, &images));
        let u = 90.5 / w as f32;
        let (up, down) = (dir_at_u(u, el), dir_at_u(u, -el));
        let light = city.radiance(up) - plain.radiance(up);
        assert!(light.x > city.scale(), "the premise: the light shows above the horizon: {light:?}");
        let fresnel = 0.02 + 0.98 * (1.0 - el.to_radians().sin()).powi(5);
        let mirrored = city.radiance(down) - plain.radiance(down);
        assert!(close3(mirrored, light * fresnel, 1e-3), "the puddle shows {mirrored:?}, the light by Fresnel {:?}", light * fresnel);
        let aside = dir_at_u(u + 10.0 / 360.0, -el);
        assert_eq!(city.radiance(aside), plain.radiance(aside), "10 degrees round the turn the puddle shows none of it");
    }

    /// Under rain a puddle mirrors the city as the rain shows it (plan hdri-6 §5: weather is still drawn over everything).
    /// The rain kind at Wet ground 1.00, the night, no light layers, the band of `a_puddle_mirrors_a_city_light_above_it`:
    /// above the horizon the map shows the light through the veil along its direction; the puddle 12 degrees down shows that,
    /// by the water's Fresnel term, through the veil along its own direction once more. So where the rain hides the city
    /// the puddles hide it too; over a host's wet ground they are the only water below the band's sea and lakes (the strip
    /// gives way to them by their share of its land, makepad_skyline's `SkyLight::floor_share`).
    #[test]
    fn under_rain_a_puddle_mirrors_the_city_as_the_rain_shows_it() {
        let mut p = HdriParams { lights: vec![], ..HdriParams::default() };
        sun_at(&mut p, -20.0, 0.0);
        p.sky.weather = Some(WeatherParams { kind: "rain".to_string(), wet_ground: 1.0, ..WeatherParams::default() });
        let (w, h, top, bottom) = (360usize, 24usize, 20.0f32, -4.0f32);
        let row = ((top - 12.0) / (top - bottom) * h as f32 - 0.5).round() as usize;
        let el = top - (row as f32 + 0.5) * (top - bottom) / h as f32;
        let mut texels = vec![[0.0f32; 4]; w * h];
        texels[row * w + 90] = [40.0, 30.0, 20.0, 0.0];
        let (q, images) = with_band(&p, SkylineBand { w, h, texels, dist: Vec::new(), light: Vec::new(), top_deg: top, bottom_deg: bottom, eye_m: 0.0 });
        let (plain, city) = (Env::new(&p), Env::new_with(&q, &images));
        let u = 90.5 / w as f32;
        let (up, down) = (dir_at_u(u, el), dir_at_u(u, -el));
        // what passes the map's veil along a direction (it is affine and grey)
        let through = |d: Vec3f| veil_of(&city, d, Vec3f::all(1.0)) - veil_of(&city, d, Vec3f::default());
        let (light, t_up, t_down) = (city.radiance(up) - plain.radiance(up), through(up), through(down));
        println!("through the rain: {:.3} up, {:.3} down", t_up.x, t_down.x);
        assert!(light.x > 0.0 && t_up.x < 0.9, "the premise: the rain veils the light above the horizon: {light:?} through {t_up:?}");
        let fresnel = 0.02 + 0.98 * (1.0 - el.to_radians().sin()).powi(5);
        let mirrored = city.radiance(down) - plain.radiance(down);
        let want = vec3f(light.x * t_down.x, light.y * t_down.y, light.z * t_down.z) * fresnel;
        assert!(close3(mirrored, want, 1e-3), "the puddle shows {mirrored:?}, the light as the rain shows it by Fresnel {want:?}");
    }

    /// A caster's bloom asks the base for its fine detail (`Env::base_peak`). A city light a puddle mirrors is as fine as
    /// the light itself, so under a wet ground the bloom finds it at the mirrored elevation, as bright; on a dry ground
    /// nothing is there (the band ends 4 degrees down, its bottom row and so its skirt dark).
    #[test]
    fn a_casters_bloom_finds_a_city_light_in_the_puddles() {
        let (w, h) = (360usize, 24usize);
        let mut texels = vec![[0.0f32; 4]; w * h];
        // a light 11.5 degrees up, as in `a_puddle_mirrors_a_city_light_above_it`
        texels[8 * w + 90] = [40.0, 30.0, 20.0, 0.0];
        let band = SkylineBand { w, h, texels, dist: Vec::new(), light: Vec::new(), top_deg: 20.0, bottom_deg: -4.0, eye_m: 0.0 };
        let (u, radius) = (90.5 / w as f32, 0.5f32.to_radians());
        for wet in [0.0, 1.0] {
            let mut p = HdriParams { lights: vec![], ..HdriParams::default() };
            sun_at(&mut p, -20.0, 0.0);
            p.sky.weather = Some(WeatherParams { wet_ground: wet, ..WeatherParams::default() });
            let (q, images) = with_band(&p, band.clone());
            let env = Env::new_with(&q, &images);
            let light = 40.0 * env.scale();
            let up = env.base_peak(dir_at_u(u, 11.5), radius);
            assert!(up.is_some_and(|(most, _)| most >= light), "the premise: the light itself, {up:?}");
            let down = env.base_peak(dir_at_u(u, -11.5), radius);
            if wet > 0.0 {
                assert!(down.is_some_and(|(most, step)| most >= light && step == up.unwrap().1), "the puddle shows it: {down:?}");
            } else {
                assert_eq!(down, None, "a dry ground shows nothing there");
            }
        }
    }

    /// Skyline off (or absent): a wet ground mirrors only the sky, bit for bit as before, at Wet ground 1.00 too.
    #[test]
    fn without_a_skyline_a_wet_ground_mirrors_only_the_sky() {
        let mut p = HdriParams::default();
        p.sky.weather = Some(WeatherParams { wet_ground: 1.0, ..WeatherParams::default() });
        let base = Env::new(&p).bake(64);
        let (mut q, images) = with_band(&p, band(20.0, -5.0, [0.5, 0.5, 0.5, 1.0]));
        q.skyline.as_mut().unwrap().on = false;
        assert!(Env::new_with(&q, &images).bake(64) == base);
        assert!(Env::new_with(&p, &images).bake(64) == base);
    }

    /// The strip's wet ground (plan hdri-6 §5): the light hands it the puddles' share and the damp ground. A dry ground's
    /// damp ring is its ground ring, bit for bit, and its share is 0.
    #[test]
    fn the_strip_gets_the_wetness_and_the_damp_ground() {
        for (name, p) in skies() {
            let l = Env::new(&p).skyline_light();
            let env = Env::new(&p);
            let dry = env.weather.as_ref().map_or(true, |w| !w.mirrors());
            if dry {
                assert!(l.wet == 0.0 && l.ground_damp == l.ground_ring, "{name}");
                continue;
            }
            assert!(l.wet > 0.0 && l.wet == env.weather.as_ref().unwrap().puddle_share(), "{name}: {}", l.wet);
            let damp = Sampling { bare: true, unveiled: true, damp_ground: true, ..Sampling::default() };
            let turn = p.skyline.as_ref().map_or(0.0, |b| b.turn_deg);
            for i in 0..RING {
                let u = ((i as f32 + 0.5) / RING as f32 + turn / 360.0).rem_euclid(1.0);
                assert!(vec(l.ground_damp[i]) == env.base_under_skyline(dir_at_u(u, -GROUND_RING_DEG), None, damp), "{name}: {i}");
            }
        }
        let mut soaked = HdriParams::default();
        soaked.sky.weather = Some(WeatherParams { wet_ground: 1.0, ..WeatherParams::default() });
        assert_eq!(Env::new(&soaked).skyline_light().wet, 1.0);
    }

    /// The map under `weather` (fog, rain), the sun at 25 degrees: a band built as the app's strip is, from the map's own
    /// light (`skyline_light`): its far rows a fully hazed ridge (the low ring itself, opaque), its near rows the map's
    /// ground (the ground ring, opaque), both read at a column's centre. Veiled once by the map, the ridge is the map's
    /// sky there (where a rain shaft hangs, that sky without the shaft: the ridge stands in front of it, plan hdri-6 §5b)
    /// and the band's ground the map's ground, within 1e-4 (a wet ground's at its puddles' mean share, the
    /// owner's ruling: then the band's ground is also even round the turn within COLUMN_STEP, and its mean round the
    /// turn the map's own wet ground's, looked up, within MEAN_TOL); and the skirt held to the ground is no brighter
    /// than it (`held_to`, against the unveiled ground, so it holds under the veil too). With `dry` (no wet ground,
    /// whose puddles change the ground from one look to the next) the band's ground just above its bottom is the map's
    /// ground there within GROUND_STEP (the ground ring is read 1.5 degrees higher; the skirt is held to the darkest
    /// ground down its 4 degrees), and runs into the skirt without a seam. Before the fix the ridge showed
    /// veil(veil(sky)) and the ground veil(veil(ground)): under fog a lighter band at the horizon.
    fn the_band_under_weather_is_the_maps_own(name: &str, weather: WeatherParams, dry: bool) {
        const GROUND_STEP: f32 = 0.02;
        // (a wet ground only) round the turn, the band's ground against the map's own wet ground looked up 256 times
        // over each ring entry's span: the puddles' share is the fbm's mean taken as normal (`Weather::ground_even`),
        // a few per cent off the field's own (measured: fog 4.7 %, rain 2.2 %); one entry's span holds only a few
        // puddles, so entry by entry the look-ups scatter (up to 18 %) and only the turn's mean is held
        const MEAN_TOL: f32 = 0.06;
        // (a wet ground only) adjacent columns of the band's ground, as RING_STEP in `the_ground_ring_is_even_under_a_wet_ground`
        const COLUMN_STEP: f32 = 0.05;
        let mut columns = Vec::with_capacity(RING);
        let mut p = HdriParams::default();
        sun_at(&mut p, 25.0, 140.0);
        p.sky.weather = Some(weather);
        let plain = Env::new(&p);
        assert!(plain.weather.is_some(), "{name}: the premise: a weather draws");
        let l = plain.skyline_light();
        // rows a degree tall from 4.5 to -5.5: centres 4, 3, ..., -5; rows 0..=4 (4..0 degrees) the ridge, 5.. the ground
        let (w, h, top, bottom) = (RING, 10, 4.5f32, -5.5f32);
        let texels = (0..w * h).map(|i| {
            let (x, y) = (i % w, i / w);
            let c = if y <= 4 { l.ring_low[x] } else { l.ground_ring[x] };
            [c[0], c[1], c[2], 1.0]
        }).collect();
        let (q, images) = with_band(&p, SkylineBand { w, h, texels, dist: Vec::new(), light: Vec::new(), top_deg: top, bottom_deg: bottom, eye_m: 0.0 });
        let city = Env::new_with(&q, &images);
        let k = plain.scale();
        let light = |c: Vec3f| 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
        let (mut worst_far, mut worst_near, mut worst_step, mut worst_mean) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let (mut band_sum, mut looked_sum) = (0.0f64, 0.0f64);
        for i in 0..RING {
            let u = (i as f32 + 0.5) / RING as f32;
            // a fully hazed far texel and the map's sky beside it (the same direction, without the city); where a rain shaft
            // hangs, the ridge stands in front of it (plan hdri-6 §5b): the ring veiled without the shafts
            let d = dir_at_u(u, 1.0);
            let (far, sky) = match &plain.weather {
                Some(w) if w.virga(d) > 0.0 => (city.radiance(d), w.veil_without_virga(d, vec(l.ring_low[i]), plain.weather_sun, plain.dome) * k),
                _ => (city.radiance(d), plain.radiance(d)),
            };
            worst_far = worst_far.max((far - sky).length() / sky.length());
            assert!(close3(far, sky, 1e-4), "{name}: u {u}: the hazed ridge {far:?}, the map's sky {sky:?}");
            // the band's ground where the ground ring was read, and the map's ground there, a wet ground's puddles at their
            // mean share (the owner's ruling; a dry ground's mean is its own)
            let d = dir_at_u(u, -GROUND_RING_DEG);
            let near = city.radiance(d);
            columns.push(light(near));
            let ground = plain.base_under_skyline(d, None, Sampling { bare: true, even_ground: true, ..Sampling::default() }) * k;
            if dry {
                assert!(close3(ground, plain.radiance(d), 1e-6), "{name}: u {u}: a dry ground's mean is not its own");
            } else {
                // and that mean is the map's own wet ground averaged over the ring entry's span (its 5.6 degrees of
                // azimuth, a degree of elevation about -4): 16 x 16 looks
                let mut sum = Vec3f::default();
                for a in 0..16 {
                    for b in 0..16 {
                        let du = (a as f32 + 0.5) / 16.0 - 0.5;
                        sum += plain.radiance(dir_at_u(u + du / RING as f32, -GROUND_RING_DEG - 0.5 + (b as f32 + 0.5) / 16.0));
                    }
                }
                let looked = sum * (1.0 / 256.0);
                worst_mean = worst_mean.max((light(near) - light(looked)).abs() / light(looked));
                (band_sum, looked_sum) = (band_sum + light(near) as f64, looked_sum + light(looked) as f64);
            }
            worst_near = worst_near.max((near - ground).length() / ground.length());
            assert!(close3(near, ground, 1e-4), "{name}: u {u}: the band's ground {near:?}, the map's {ground:?}");
            if dry {
                // across the band's bottom: the strip's ground just above it against the map's own ground there, and
                // just below it (the skirt), without a seam
                let (above, below) = (city.radiance(dir_at_u(u, bottom + 0.01)), city.radiance(dir_at_u(u, bottom - 0.01)));
                let ground = plain.radiance(dir_at_u(u, bottom + 0.01));
                let step = (light(above) - light(ground)).abs() / light(ground);
                worst_step = worst_step.max(step);
                assert!(step < GROUND_STEP, "{name}: u {u}: the band's ground {above:?} over the map's {ground:?}: {step}");
                // (the veil itself changes fast there, near the horizon: the map's own change over the same step aside)
                let (plain_above, plain_below) = (plain.radiance(dir_at_u(u, bottom + 0.01)), plain.radiance(dir_at_u(u, bottom - 0.01)));
                let seam = ((above - below) - (plain_above - plain_below)).length() / above.length();
                assert!(seam < 1e-3, "{name}: u {u}: a seam at the band's bottom, {above:?} over {below:?} ({seam})");
            }
            // the skirt is never brighter than the map's own ground (nor than the band's ground, veiled)
            for el in [bottom - 0.01, bottom - 0.5 * SKIRT_DEG, bottom - 0.99 * SKIRT_DEG] {
                let d = dir_at_u(u, el);
                let most = light(plain.radiance(d)).max(light(veil_of(&plain, d, vec(l.ground_ring[i])) * k));
                assert!(light(city.radiance(d)) <= most * (1.0 + 1e-4) + 1e-7, "{name}: u {u}, el {el}: the skirt {:?} over the ground {most}", city.radiance(d));
            }
        }
        println!("{name}: the ridge against the sky {worst_far:.1e}, the band's ground against the map's {worst_near:.1e}, across the bottom {worst_step:.4}, against the looked-up mean {worst_mean:.4} at worst");
        if !dry {
            // the band's ground is even round the turn: no stripes of puddle and damp ground
            let step = (0..RING).map(|i| (columns[i] - columns[(i + 1) % RING]).abs() / columns[i].max(columns[(i + 1) % RING])).fold(0.0f32, f32::max);
            println!("{name}: the band's ground, adjacent columns at most {:.2} % apart", 100.0 * step);
            assert!(step <= COLUMN_STEP, "{name}: the band's ground has stripes: adjacent columns {:.1} % apart: {columns:?}", 100.0 * step);
            // round the turn, the band's ground is the map's wet ground's mean
            let off = ((band_sum - looked_sum) / looked_sum).abs() as f32;
            println!("{name}: round the turn the band's ground {:.4}, the map's wet ground looked up {:.4}: {:.2} % apart", band_sum / RING as f64, looked_sum / RING as f64, 100.0 * off);
            assert!(off <= MEAN_TOL, "{name}: round the turn the band's ground and the wet ground's mean are {:.1} % apart", 100.0 * off);
        }
    }

    #[test]
    fn under_fog_the_band_is_the_maps_own() {
        the_band_under_weather_is_the_maps_own("fog", WeatherParams { kind: "fog".to_string(), ..WeatherParams::default() }, false);
        the_band_under_weather_is_the_maps_own("dry fog", WeatherParams { fog: 0.7, ..WeatherParams::default() }, true);
    }

    #[test]
    fn under_rain_the_band_is_the_maps_own() {
        the_band_under_weather_is_the_maps_own("rain", WeatherParams { kind: "rain".to_string(), ..WeatherParams::default() }, false);
        the_band_under_weather_is_the_maps_own("dry rain", WeatherParams { rain: 0.6, fog: 0.1, ..WeatherParams::default() }, true);
    }

    /// Plan hdri-6 §5b: the rain's shafts hang behind the skyline. The rain kind, two opaque bands of other greys. The rain haze
    /// depends on the elevation alone, and the fog's glow is added, so their difference is the same at every azimuth when no
    /// shaft is in front of the city (before: the shafts' diagonal dashes across it). Each band reads as the shaft-free veil
    /// of its texel, and the sky above the band keeps its shafts.
    #[test]
    fn the_rain_shafts_hang_behind_the_skyline() {
        let mut p = HdriParams { lights: vec![], ..HdriParams::default() };
        sun_at(&mut p, 25.0, 180.0);
        p.sky.weather = Some(WeatherParams { kind: "rain".to_string(), ..WeatherParams::default() });
        let (a, b) = ([0.30, 0.30, 0.30, 1.0], [0.10, 0.10, 0.10, 1.0]);
        let env = |p: &HdriParams, s: [f32; 4]| { let (q, images) = with_band(p, band(20.0, -5.0, s)); Env::new_with(&q, &images) };
        let (ea, eb) = (env(&p, a), env(&p, b));
        let rain = ea.weather.as_ref().expect("rain draws");
        let diffs: Vec<f32> = (0..72).map(|i| { let d = dir(i as f32 * 5.0, 8.0); (ea.radiance(d) - eb.radiance(d)).x }).collect();
        let (lo, hi) = diffs.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)));
        assert!(hi - lo <= 1e-5 * hi.abs().max(1e-6), "a shaft crosses the city: {lo}..{hi}");
        for i in 0..72 {
            let d = dir(i as f32 * 5.0, 8.0);
            let want = rain.veil_without_virga(d, rgb(a), ea.weather_sun, ea.dome) * ea.scale();
            assert!(close3(ea.radiance(d), want, 1e-5), "az {}: {:?} vs {want:?}", i * 5, ea.radiance(d));
        }
        // the premise: shafts hang at 8 degrees (they darken the sky behind)
        assert!((0..72).any(|i| rain.virga(dir(i as f32 * 5.0, 8.0)) > 0.05));
        // and the wet ground's mirror of the city has no shafts in front of it either (rain with a full water floor)
        p.sky.weather = Some(WeatherParams { kind: "rain".to_string(), wet_ground: 1.0, ..WeatherParams::default() });
        let (ea, eb) = (env(&p, a), env(&p, b));
        let mirrored: Vec<f32> = (0..72).map(|i| { let d = dir(i as f32 * 5.0, -12.0); (ea.radiance(d) - eb.radiance(d)).x }).collect();
        let (lo, hi) = mirrored.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)));
        assert!(hi > 0.0 && hi - lo <= 1e-4 * hi, "the puddles show shafts across the city: {lo}..{hi}");
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
        // Under a weather the ground is the map's before its veil, as the rings are (the strip is veiled once, by the
        // map): veiled, it is the ground the map draws. Fog, and rain on dry ground (a wet ground's puddles mirror the
        // sky, which a white Lambertian facade does not); the rain's dimming of the sun is the ambient's then.
        for (name, fog, rain) in [("fog", 0.6, 0.0), ("rain", 0.0, 0.6)] {
            let mut p = HdriParams::default();
            sun_at(&mut p, 35.0, 120.0);
            p.sky.atmosphere.ground_color = [1.0; 3];
            p.sky.weather = Some(WeatherParams { fog, rain, wet_ground: 0.0, ..WeatherParams::default() });
            cases.push((name, 35.0, p));
        }
        for (name, el, p) in cases {
            let env = Env::new(&p);
            let l = env.skyline_light();
            let down = dir(0.0, -90.0);
            let seen = env.radiance(down) / env.scale();
            let ground = env.base_under_skyline(down, None, Sampling { bare: true, unveiled: true, ..Sampling::default() });
            let rise = el.to_radians().sin().max(0.0);
            let lit = vec(l.sun_rgb) * rise + vec(l.ambient_rgb);
            println!("{name}: sun {:?} ambient {:?}, the ground {ground:?} ({seen:?} veiled), gain {:?}", l.sun_rgb, l.ambient_rgb, env.ground_gain);
            assert!(env.weather.is_some() || ground == seen, "{name}: without a weather the unveiled ground is the map's");
            assert!(close3(lit, ground, 1e-4), "{name}: {lit:?} vs the ground {ground:?}");
            assert!(close3(veil_of(&env, down, lit), seen, 1e-4), "{name}: veiled {:?} vs the map's ground {seen:?}", veil_of(&env, down, lit));
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
        let city = SkylineBand { w, h, texels, dist: Vec::new(), light: Vec::new(), top_deg: 3.0, bottom_deg: -4.0, eye_m: 0.0 };
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

    /// The final review: one NaN and one infinite texel in the band's bottom row (and an alpha past 1). Unguarded, the
    /// skirt's running sum carries the NaN all the way round, and the map draws it there; through the handoff
    /// (`SkylineBand::new`) the channels are 0, alpha is held to 0..1, and the skirt and the bake are finite everywhere.
    #[test]
    fn a_non_finite_texel_is_made_safe_at_the_handoff() {
        let (w, h) = (128, 6);
        let mut texels = vec![[0.1, 0.2, 0.3, 1.0]; w * h];
        texels[(h - 1) * w + 10] = [f32::NAN, 0.5, 0.5, 1.0];
        texels[(h - 1) * w + 70] = [0.5, f32::INFINITY, 0.5, f32::NEG_INFINITY];
        texels[2 * w + 40] = [0.5, 0.5, 0.5, 7.0];
        let raw = SkylineBand { w, h, texels: texels.clone(), dist: Vec::new(), light: Vec::new(), top_deg: 10.0, bottom_deg: -5.0, eye_m: 0.0 };
        let below = |b: &SkylineBand, u: f32| b.sample_skirted(&b.skirt(), dir_at_u(u, -5.0 - 0.5 * SKIRT_DEG), 0.0);
        // the premise: far from both, half a turn round, the raw skirt is NaN
        assert!(below(&raw, 0.75).iter().any(|c| !c.is_finite()), "the premise: one NaN poisons the skirt round the turn");
        let safe = SkylineBand::new(w, h, texels, Vec::new(), 10.0, -5.0);
        assert_eq!(safe.texels[(h - 1) * w + 10], [0.0, 0.5, 0.5, 1.0]);
        assert_eq!(safe.texels[(h - 1) * w + 70], [0.5, 0.0, 0.5, 0.0]);
        assert_eq!(safe.texels[2 * w + 40], [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(safe.texels[0], [0.1, 0.2, 0.3, 1.0], "a finite texel is kept as it is");
        for i in 0..w {
            assert!(below(&safe, (i as f32 + 0.5) / w as f32).iter().all(|c| c.is_finite()), "the skirt at column {i}");
        }
        let p = HdriParams::default();
        let in_skirt = dir_at_u(0.75, -5.0 - 0.5 * SKIRT_DEG);
        let (q, images) = with_band(&p, raw);
        let seen = Env::new_with(&q, &images).radiance(in_skirt);
        assert!(!(seen.x.is_finite() && seen.y.is_finite() && seen.z.is_finite()), "the premise: the map draws it, {seen:?}");
        let (q, images) = with_band(&p, safe);
        let env = Env::new_with(&q, &images);
        let seen = env.radiance(in_skirt);
        assert!(seen.x.is_finite() && seen.y.is_finite() && seen.z.is_finite(), "{seen:?}");
        assert!(env.bake(64).data.iter().all(|t| t.iter().all(|c| c.is_finite())), "the bake is finite");
    }

    #[test]
    fn a_band_prints_its_size_not_its_texels() {
        let text = format!("{:?}", band(20.0, -5.0, [0.5; 4]));
        assert!(text.contains("64") && text.contains("16") && text.contains("20") && text.len() < 200, "{text}");
    }

    /// Plan hdri-6 Task 8: the distance a read hands is the coverage-weighted mean of what the texels show (bilinear over
    /// the premultiplied distance, a D, over the bilinear alpha). Two texels round the turn: one half covered at 1000 m,
    /// one opaque at 4000 m. Half way between them (0.5 x 1000 + 1 x 4000) / 1.5 = 3000 m; at each centre its own; the
    /// colour is `sample_skirted`'s bit for bit.
    #[test]
    fn the_distance_is_the_coverage_weighted_mean() {
        let b = SkylineBand::new(2, 1, vec![[0.1, 0.1, 0.1, 0.5], [0.4, 0.3, 0.2, 1.0]], vec![1000.0, 4000.0], 10.0, -10.0);
        let skirt = b.skirt();
        for (u, want) in [(0.5, 3000.0f32), (0.25, 1000.0), (0.75, 4000.0)] {
            let d = dir_at_u(u, 2.0);
            let (c, far) = b.sample_skirted_far(&skirt, d, 0.0);
            assert_eq!(c, b.sample_skirted(&skirt, d, 0.0), "u {u}: the colour is sample_skirted's");
            let far = far.expect("a covered texel has a distance");
            assert!((far - want).abs() < 1e-2, "u {u}: {far} m, not {want}");
        }
    }

    /// A band without a distance plane (an empty `dist`, as hand-built bands and old callers have it) reads no distance
    /// anywhere, its colour unchanged; a texel without coverage reads none either, whatever its plane holds.
    #[test]
    fn a_band_without_distances_reads_none() {
        let b = band(20.0, -5.0, [0.2, 0.2, 0.2, 1.0]);
        let skirt = b.skirt();
        for el in [10.0, -4.5, -5.0 - 0.5 * SKIRT_DEG] {
            let d = dir(30.0, el);
            assert_eq!(b.sample_skirted_far(&skirt, d, 0.0), (b.sample_skirted(&skirt, d, 0.0), None), "at {el}");
        }
        let clear = SkylineBand::new(4, 1, vec![[0.3, 0.2, 0.1, 0.0]; 4], vec![2000.0; 4], 10.0, -10.0);
        assert_eq!(clear.sample_skirted_far(&clear.skirt(), dir(30.0, 2.0), 0.0).1, None, "no coverage, no distance");
        assert_eq!(band(20.0, -5.0, [0.2; 4]).sample_skirted_far(&skirt, dir(30.0, 40.0), 0.0), ([0.0; 4], None), "above the band");
    }

    /// The handoff (`SkylineBand::new`) makes the distance plane safe as it does the texels: a NaN, an infinity or a
    /// negative distance is 0.
    #[test]
    fn a_bad_distance_is_made_safe_at_the_handoff() {
        let b = SkylineBand::new(5, 1, vec![[0.1, 0.1, 0.1, 1.0]; 5], vec![f32::NAN, f32::INFINITY, -3.0, f32::NEG_INFINITY, 1500.0], 10.0, -10.0);
        assert_eq!(b.dist, vec![0.0, 0.0, 0.0, 0.0, 1500.0]);
    }

    /// The skirt carries the bottom row's distance as it carries its colour: premultiplied (a D), box-blurred along the
    /// turn, so below the band a read is the coverage-weighted mean of the bottom row's texels in the blur (brute force
    /// here), and the blend into it just above the band's bottom edge meets it; the rows above (30 km) do not leak in. A
    /// bottom row of 2400 m opaque and 800 m half covered, every other texel, at 1 degree a texel.
    #[test]
    fn the_skirts_distance_is_the_bottom_rows() {
        let (w, h) = (360usize, 4usize);
        let texels: Vec<[f32; 4]> = (0..w * h).map(|i| if i / w < h - 1 || i % 2 == 0 { [0.1, 0.1, 0.1, 1.0] } else { [0.1, 0.1, 0.1, 0.5] }).collect();
        let dist: Vec<f32> = (0..w * h).map(|i| if i / w < h - 1 { 30000.0 } else if i % 2 == 0 { 2400.0 } else { 800.0 }).collect();
        let b = SkylineBand::new(w, h, texels.clone(), dist.clone(), 6.0, -6.0);
        let skirt = b.skirt();
        let half = (SKIRT_BLUR_DEG / 360.0 * w as f32).round() as i64;
        for x in [0usize, 1, 7, 180, 359] {
            let (mut pd, mut a) = (0.0f64, 0.0f64);
            for k in -half..=half {
                let i = (h - 1) * w + (x as i64 + k).rem_euclid(w as i64) as usize;
                pd += (texels[i][3] * dist[i]) as f64;
                a += texels[i][3] as f64;
            }
            let want = (pd / a) as f32;
            let u = (x as f32 + 0.5) / w as f32;
            for el in [-6.0 - 1e-3, -6.0 - 0.5 * SKIRT_DEG, -6.0 - 0.9 * SKIRT_DEG] {
                let far = b.sample_skirted_far(&skirt, dir_at_u(u, el), 0.0).1.expect("the skirt has a distance");
                assert!((far - want).abs() < 1e-3 * want, "x {x}, {el} deg: {far} m, not {want}");
            }
            let above = b.sample_skirted_far(&skirt, dir_at_u(u, -6.0 + 1e-3), 0.0).1.expect("the band's bottom has a distance");
            assert!((above - want).abs() < 1e-2 * want, "x {x}: just above the bottom edge {above} m, the skirt {want}");
        }
    }

    /// A band of 1 degree rows from 20.5 down to -3.5 degrees, 360 columns, transparent but for a 3 x 3 block of `s`
    /// centred at column 90 and elevation `el` (whole degrees), each texel of it `far` m off (None: no distance plane);
    /// and the strip's u at the block's centre. Near the centre a bilinear read sees the block alone.
    fn block_band(s: [f32; 4], el: f32, far: Option<f32>) -> (SkylineBand, f32) {
        let (w, h, top) = (360usize, 24usize, 20.5f32);
        let row = (top - el - 0.5).round() as usize;
        let in_block = |i: usize| (i / w).abs_diff(row) <= 1 && (i % w).abs_diff(90) <= 1;
        let texels = (0..w * h).map(|i| if in_block(i) { s } else { [0.0; 4] }).collect();
        let dist = match far {
            Some(far) => (0..w * h).map(|i| if in_block(i) { far } else { 0.0 }).collect(),
            None => Vec::new(),
        };
        (SkylineBand::new(w, h, texels, dist, top, top - h as f32), 90.5 / w as f32)
    }

    /// The rain kind with no light layers, its sun at `sun_el` degrees, its ground `wet_ground` wet (None: the kind's own).
    fn rain_kind(sun_el: f32, wet_ground: Option<f32>) -> HdriParams {
        let mut p = HdriParams { lights: vec![], ..HdriParams::default() };
        sun_at(&mut p, sun_el, 140.0);
        let w = WeatherParams { kind: "rain".to_string(), ..WeatherParams::default() };
        p.sky.weather = Some(match wet_ground { Some(wet_ground) => WeatherParams { wet_ground, ..w }, None => w });
        p
    }

    /// Plan hdri-6 R1: the weather veils a strip texel over the distance to what it shows. Under the rain kind at night an
    /// opaque city texel at 1 degree, 2 km off, is the veil over 2 km: T c + (1 - T) L, its transmittance T that of the
    /// rain and the fog over 2 km (`Weather::veil_within`; its own test holds T to the slabs' constants), at least e^3
    /// times the transmittance over the whole path the strip was veiled by before (about 3.5 km of fog, 11 km of rain).
    #[test]
    fn under_the_rain_kind_the_city_keeps_its_distance_light() {
        let p = rain_kind(-20.0, None);
        let c = [0.4, 0.3, 0.2, 1.0];
        let env = |s: [f32; 4]| { let (b, _) = block_band(s, 1.0, Some(2000.0)); let (q, images) = with_band(&p, b); Env::new_with(&q, &images) };
        let (city, black) = (env(c), env([0.0, 0.0, 0.0, 1.0]));
        let u = block_band(c, 1.0, None).1;
        let d = dir_at_u(u, 1.0);
        let rain = city.weather.as_ref().expect("rain draws");
        let k = city.scale();
        let want = rain.veil_within(d, rgb(c), city.weather_sun, city.dome, 2000.0) * k;
        let got = city.radiance(d);
        assert!(close3(got, want, 1e-5), "the city {got:?}, veiled over its 2 km {want:?}");
        // T c + (1 - T) L: what of the texel passes, against the black texel's in-scatter alone
        let t = (got - black.radiance(d)).x / (c[0] * k);
        let within = |far: f32| (rain.veil_within(d, Vec3f::all(1.0), city.weather_sun, city.dome, far) - rain.veil_within(d, Vec3f::default(), city.weather_sun, city.dome, far)).x;
        let whole = (rain.veil_without_virga(d, Vec3f::all(1.0), city.weather_sun, city.dome) - rain.veil_without_virga(d, Vec3f::default(), city.weather_sun, city.dome)).x;
        println!("through the rain at 1 degree: {t:.4} over 2 km ({:.4} by the veil), {whole:.2e} over the whole path", within(2000.0));
        assert!((t - within(2000.0)).abs() <= 1e-4 * t, "{t} passes, the veil over 2 km passes {}", within(2000.0));
        assert!(t >= 3.0f32.exp() * whole, "{t} passes over 2 km, {whole} over the whole path: not e^3 more");
    }

    /// At or beyond the slabs' path (the far ranges at the horizon) a texel is veiled as before, bit for bit: 1000 km and
    /// the ground's 60 km cap (makepad_skyline's FAR_M), against a band without a distance plane. (At the path itself:
    /// the weather's own test, `a_veil_within_a_distance_takes_the_slab_path_to_it`.)
    #[test]
    fn a_far_texel_veils_as_before() {
        for p in [rain_kind(-20.0, None), rain_kind(25.0, None), rain_kind(25.0, Some(1.0))] {
            let c = [0.4, 0.3, 0.2, 1.0];
            let env = |far: Option<f32>| { let (b, _) = block_band(c, 1.0, far); let (q, images) = with_band(&p, b); Env::new_with(&q, &images) };
            let (before, u) = (env(None), block_band(c, 1.0, None).1);
            for far in [1.0e6, 6.0e4] {
                let after = env(Some(far));
                for el in [0.5, 1.0, 1.5] {
                    let d = dir_at_u(u, el);
                    assert_eq!(after.radiance(d), before.radiance(d), "{far} m at {el} degrees");
                }
            }
        }
    }

    /// A texel without a distance is veiled as before: a band without a distance plane (the shaft-free veil over the whole
    /// path, plan hdri-6 §5b), and in a band with one, a light without coverage (the glow) and the base around it.
    #[test]
    fn a_texel_without_a_distance_veils_as_before() {
        let p = rain_kind(25.0, None);
        let c = [0.4, 0.3, 0.2, 1.0];
        let (b, u) = block_band(c, 1.0, None);
        let (q, images) = with_band(&p, b);
        let city = Env::new_with(&q, &images);
        let rain = city.weather.as_ref().expect("rain draws");
        let d = dir_at_u(u, 1.0);
        assert_eq!(city.radiance(d), rain.veil_without_virga(d, rgb(c), city.weather_sun, city.dome) * city.scale());
        let glow = [0.5, 0.3, 0.1, 0.0];
        let env = |far: Option<f32>| { let (b, _) = block_band(glow, 1.0, far); let (q, images) = with_band(&p, b); Env::new_with(&q, &images) };
        let (before, after) = (env(None), env(Some(2000.0)));
        for (du, el) in [(0.0, 1.0), (0.0, 0.5), (3.0 / 360.0, 1.0), (0.0, 5.0)] {
            let d = dir_at_u(u + du, el);
            assert_eq!(after.radiance(d), before.radiance(d), "{du} {el}");
        }
    }

    /// `over_veiled` with a veil over the texel's distance: veil_D(s) + (1 - a)(base - veil_D(0)). The veil is affine,
    /// veil_D(x) = T x + (1 - T) L, so this is T s + a (1 - T) L + (1 - a) base: the texel's coverage veiled over its own
    /// distance, a veil_D(s / a), and the base behind as the map veils it (its shafts and all). A dark half-covered texel
    /// 2 km off under the rain kind by day.
    #[test]
    fn a_half_covered_texel_veils_its_share_by_its_distance() {
        let p = rain_kind(25.0, None);
        let s = [0.02, 0.015, 0.01, 0.5];
        let (b, u) = block_band(s, 1.0, Some(2000.0));
        let (q, images) = with_band(&p, b);
        let (plain, city) = (Env::new(&p), Env::new_with(&q, &images));
        let rain = city.weather.as_ref().expect("rain draws");
        let k = city.scale();
        let d = dir_at_u(u, 1.0);
        let veil = |c: Vec3f| rain.veil_within(d, c, city.weather_sun, city.dome, 2000.0);
        let a = s[3];
        let base = plain.radiance(d) * (1.0 / k);
        let want = (veil(rgb(s) * (1.0 / a)) * a + base * (1.0 - a)) * k;
        let got = city.radiance(d);
        assert!(close3(got, want, 1e-5), "the half-covered texel {got:?}, its share veiled over 2 km over the veiled base {want:?}");
        // and so not its share veiled over the whole path
        let before = (rain.veil_without_virga(d, rgb(s) * (1.0 / a), city.weather_sun, city.dome) * a + base * (1.0 - a)) * k;
        println!("the half-covered texel: {:.4} over its distance, {:.4} over the whole path, the base behind {:.4}", got.x, before.x, plain.radiance(d).x);
        assert!(!close3(got, before, 1e-3), "{got:?}: as before {before:?}");
    }

    /// The wet ground mirrors the city over the city's distance (plan hdri-6 R1): the light runs city -> puddle -> eye,
    /// about the city's distance, and the veil is applied once. So the share of the puddle's view the mirror takes
    /// (Fresnel) runs to the city and is veiled over its 2 km, in place of the sky the floor's veil shows there; it is
    /// not veiled along the floor once more. The rain kind at night on a full water floor, an opaque lit block 2 km off,
    /// 12 degrees up (where the floor's own veil is thin) and 2 degrees up (where the floor's path is km long and a
    /// second veil would leave next to nothing): above the horizon the veil over 2 km; in the puddle as far down, that
    /// light by Fresnel, about the light above. A half-covered texel (2 degrees up) mirrors its share so, the sky beside
    /// it as before; by day (where the floor's in-scatter is bright) a lit and a dark block 2 degrees up too. The sky
    /// mirrored beside the block is unchanged.
    #[test]
    fn a_puddle_mirrors_the_city_through_its_distance() {
        let (night, day) = (rain_kind(-20.0, Some(1.0)), rain_kind(25.0, Some(1.0)));
        let lit = [40.0, 30.0, 20.0, 1.0];
        for (p, el, s) in [(&night, 2.0f32, lit), (&night, 12.0, lit), (&night, 2.0, [20.0, 15.0, 10.0, 0.5]), (&day, 2.0, lit), (&day, 2.0, [0.02, 0.015, 0.01, 1.0])] {
            let (b, u) = block_band(s, el, Some(2000.0));
            let (q, images) = with_band(p, b);
            let (plain, city) = (Env::new(p), Env::new_with(&q, &images));
            let rain = city.weather.as_ref().expect("rain draws");
            let k = city.scale();
            let a = s[3];
            let (up, down) = (dir_at_u(u, el), dir_at_u(u, -el));
            // the strip's own light there: its coverage veiled over its 2 km, veil_D(s) - (1 - a) veil_D(0)
            let veil = |c: Vec3f| rain.veil_within(up, c, city.weather_sun, city.dome, 2000.0);
            let own = (veil(rgb(s)) - veil(Vec3f::default()) * (1.0 - a)) * k;
            let above = own + plain.radiance(up) * (1.0 - a);
            assert!(close3(city.radiance(up), above, 1e-5), "{el} deg, a {a}: above the horizon {:?}, veiled over 2 km {above:?}", city.radiance(up));
            // the full water floor's mirror share is Fresnel's term
            let fresnel = 0.02 + 0.98 * (1.0 - el.to_radians().sin()).powi(5);
            // what the puddle showed of the sky up there, through the floor's veil
            let sky_seen = veil_of(&city, down, plain.radiance(up) * (1.0 / k)) * k;
            let mirrored = city.radiance(down) - plain.radiance(down);
            let want = (own - sky_seen * a) * fresnel;
            assert!(close3(mirrored, want, 1e-4), "{el} deg, a {a}: the puddle shows {mirrored:?}, the city veiled over its 2 km in place of the sky, by Fresnel {want:?}");
            // veiled along the floor once more, as before this fix
            let through = |d: Vec3f| veil_of(&city, d, Vec3f::all(1.0)) - veil_of(&city, d, Vec3f::default());
            let light = above - plain.radiance(up);
            let twice = light.x * through(down).x * fresnel;
            println!("{el} deg, a {a}: the mirrored city {:.4}, the light above by Fresnel {:.4}, veiled twice {twice:.2e}", mirrored.x, light.x * fresnel);
            assert!(light.x <= 0.0 || mirrored.x >= 0.5 * light.x * fresnel, "{el} deg, a {a}: the mirror {mirrored:?} keeps under half the light above by Fresnel ({light:?})");
            let aside = dir_at_u(u + 10.0 / 360.0, -el);
            assert_eq!(city.radiance(aside), plain.radiance(aside), "{el} deg, a {a}: 10 degrees round the turn the puddle mirrors the sky alone");
        }
    }

    /// A weather whose rain and fog are 0 veils nothing, and the wet ground's mirror of the strip is as before, bit for
    /// bit, with a distance plane or without (plan hdri-6 R1's must-hold): a clear night and a clear day at Wet ground
    /// 1.00 and 0.8, a lit and a dark block 2 and 12 degrees up, the puddles below them.
    #[test]
    fn with_no_rain_or_fog_the_mirror_is_as_before() {
        for (sun_el, wet_ground) in [(-20.0, 1.0), (25.0, 1.0), (-20.0, 0.8)] {
            let mut p = HdriParams { lights: vec![], ..HdriParams::default() };
            sun_at(&mut p, sun_el, 140.0);
            p.sky.weather = Some(WeatherParams { wet_ground, ..WeatherParams::default() });
            for (el, s) in [(2.0f32, [40.0, 30.0, 20.0, 1.0]), (12.0, [0.02, 0.015, 0.01, 1.0]), (2.0, [20.0, 15.0, 10.0, 0.5])] {
                let env = |far: Option<f32>| { let (b, _) = block_band(s, el, far); let (q, images) = with_band(&p, b); Env::new_with(&q, &images) };
                let (before, after, u) = (env(None), env(Some(2000.0)), block_band(s, el, None).1);
                for (du, e) in [(0.0, -el), (0.0, -el - 0.4), (0.5 / 360.0, -el), (0.0, el)] {
                    let d = dir_at_u(u + du, e);
                    assert_eq!(after.radiance(d), before.radiance(d), "sun {sun_el}, wet {wet_ground}, block {el} deg ({s:?}): at {e} deg");
                }
            }
        }
    }

    /// `b` with a light plane: each texel's light the `share` of its luminance (Rec. 709), burning `far` m off.
    fn lit(b: SkylineBand, share: f32, far: f32) -> SkylineBand {
        let light = b.texels.iter().map(|t| { let l = share * (0.2126 * t[0] + 0.7152 * t[1] + 0.0722 * t[2]); [l, l * far] }).collect();
        b.with_light(light)
    }

    /// The share of what stands `far` m along `d` that the strip's veil passes there (`Weather::veil_within`).
    fn through(env: &Env, d: Vec3f, far: f32) -> f32 {
        let w = env.weather.as_ref().expect("a weather");
        (w.veil_within(d, Vec3f::all(1.0), env.weather_sun, env.dome, far) - w.veil_within(d, Vec3f::default(), env.weather_sun, env.dome, far)).x
    }

    /// Plan hdri-6 R4: the city's lights carry farther through rain and fog than its buildings. Fog and rain droplets
    /// scatter forward, so a light seen through them keeps exp(-(1 - LIGHT_FORWARD) tau) of its light, where a surface
    /// keeps exp(-tau) (Task 9's veil, unchanged); the haze in front of both is added once. Under the rain kind at night,
    /// an opaque texel all light and one all surface, both 1655 m off at 1 degree (the city's p75), against an opaque
    /// black texel there (the in-scatter alone).
    #[test]
    fn under_the_rain_kind_the_city_lights_shine_through() {
        let p = rain_kind(-20.0, None);
        let (far, c) = (1655.0, [0.4, 0.3, 0.2, 1.0]);
        let env = |b: SkylineBand| { let (q, images) = with_band(&p, b); Env::new_with(&q, &images) };
        let (b, u) = block_band(c, 1.0, Some(far));
        let (light, surface, black) = (env(lit(b.clone(), 1.0, far)), env(b), env(block_band([0.0, 0.0, 0.0, 1.0], 1.0, Some(far)).0));
        let d = dir_at_u(u, 1.0);
        let k = surface.scale();
        let t = through(&surface, d, far);
        let kept = |env: &Env| (env.radiance(d) - black.radiance(d)).x / (c[0] * k);
        let (tl, ts) = (kept(&light), kept(&surface));
        let want = t.powf(1.0 - LIGHT_FORWARD);
        println!("1655 m at 1 degree under the rain kind: the light keeps {tl:.4} (exp(-(1 - F) tau) {want:.4}), the surface {ts:.4} (exp(-tau) {t:.4})");
        assert!((ts - t).abs() <= 1e-4 * t, "the surface keeps {ts}, its veil passes {t}");
        assert!((tl - want).abs() <= 1e-3 * want, "the light keeps {tl}, not exp(-(1 - F) tau) = {want}");
        assert!(close3(light.radiance(d), black.radiance(d) + rgb(c) * (want * k), 1e-4), "the light texel {:?}: the in-scatter once, the light by {want}", light.radiance(d));
    }

    /// Plan hdri-6 R4: light without coverage (the glow, a lamp or a beacon's halo over the sky) is veiled over the
    /// distance it burns at, the light-weighted one the plane hands, and not over the whole path. A light over the
    /// transparent sky 2 km off at 1 degree, under the rain kind at night: the map's own sky behind it, and its light by
    /// exp(-(1 - F) tau) over 2 km.
    #[test]
    fn a_light_over_the_sky_is_veiled_by_its_own_distance() {
        let p = rain_kind(-20.0, None);
        let glow = [0.5, 0.3, 0.1, 0.0];
        let (b, u) = block_band(glow, 1.0, Some(2000.0));
        let (q, images) = with_band(&p, lit(b, 1.0, 2000.0));
        let (plain, city) = (Env::new(&p), Env::new_with(&q, &images));
        let d = dir_at_u(u, 1.0);
        let k = city.scale();
        let want = through(&city, d, 2000.0).powf(1.0 - LIGHT_FORWARD);
        let w = city.weather.as_ref().unwrap();
        let whole = (w.veil_without_virga(d, Vec3f::all(1.0), city.weather_sun, city.dome) - w.veil_without_virga(d, Vec3f::default(), city.weather_sun, city.dome)).x;
        let got = city.radiance(d) - plain.radiance(d);
        println!("a light over the sky 2 km off: {:.4} of it kept (exp(-(1 - F) tau) over 2 km {want:.4}), {whole:.2e} over the whole path", got.x / (glow[0] * k));
        assert!(close3(got, rgb(glow) * (want * k), 1e-4), "the light over the sky {got:?}, veiled over its 2 km {:?}", rgb(glow) * (want * k));
    }

    /// Plan hdri-6 R4's must-hold: a weather whose rain and fog are 0, and no weather, draw a band with a light plane
    /// exactly as without one, bit for bit: directly and in the puddles of a wet ground, a lit, a dark, a half-covered
    /// block and a light over the sky, with a distance plane and without.
    #[test]
    fn the_lights_rule_is_bit_identical_without_fog_or_rain() {
        let cases: [(f32, Option<f32>); 5] = [(-20.0, None), (25.0, None), (-20.0, Some(1.0)), (25.0, Some(1.0)), (-20.0, Some(0.8))];
        for (sun_el, wet) in cases {
            let mut p = HdriParams { lights: vec![], ..HdriParams::default() };
            sun_at(&mut p, sun_el, 140.0);
            p.sky.weather = wet.map(|wet_ground| WeatherParams { wet_ground, ..WeatherParams::default() });
            for (el, s, far) in [(2.0f32, [40.0, 30.0, 20.0, 1.0], Some(2000.0)), (12.0, [0.02, 0.015, 0.01, 1.0], Some(2000.0)), (2.0, [20.0, 15.0, 10.0, 0.5], Some(2000.0)),
                (2.0, [0.5, 0.3, 0.1, 0.0], Some(2000.0)), (2.0, [40.0, 30.0, 20.0, 1.0], None)] {
                let (b, u) = block_band(s, el, far);
                let env = |b: SkylineBand| { let (q, images) = with_band(&p, b); Env::new_with(&q, &images) };
                let (before, after) = (env(b.clone()), env(lit(b, 0.8, 1500.0)));
                for (du, e) in [(0.0, -el), (0.0, -el - 0.4), (0.5 / 360.0, -el), (0.0, el), (0.0, el + 0.5)] {
                    let d = dir_at_u(u + du, e);
                    assert_eq!(after.radiance(d), before.radiance(d), "sun {sun_el}, wet {wet:?}, block {el} deg ({s:?}, {far:?}): at {e} deg");
                }
            }
        }
    }

    /// Plan hdri-6 R4 in the wet ground's mirror: a puddle mirrors the lights as the weather shows them, by the share of
    /// its view the mirror takes (Fresnel on a full water floor), veiled once: a light keeps exp(-(1 - F) tau) over its
    /// distance, its surface Task 9's veil. The rain kind at night on Wet ground 1.00, 2 km off: an opaque block all
    /// light 2 and 12 degrees up, one half light, and a light over the sky (no coverage, the puddle shows it over the
    /// sky it mirrors). The sky mirrored beside the block is unchanged.
    #[test]
    fn a_puddle_mirrors_the_lights_as_the_weather_shows_them() {
        let p = rain_kind(-20.0, Some(1.0));
        for (el, s, share) in [(2.0f32, [40.0, 30.0, 20.0, 1.0], 1.0f32), (12.0, [40.0, 30.0, 20.0, 1.0], 1.0), (2.0, [40.0, 30.0, 20.0, 1.0], 0.5), (2.0, [20.0, 15.0, 10.0, 0.0], 1.0)] {
            let (b, u) = block_band(s, el, Some(2000.0));
            let (q, images) = with_band(&p, lit(b, share, 2000.0));
            let (plain, city) = (Env::new(&p), Env::new_with(&q, &images));
            let rain = city.weather.as_ref().expect("rain draws");
            let k = city.scale();
            let a = s[3];
            let (up, down) = (dir_at_u(u, el), dir_at_u(u, -el));
            let veil = |c: Vec3f| rain.veil_within(up, c, city.weather_sun, city.dome, 2000.0);
            let (light, surface) = (rgb(s) * share, rgb(s) * (1.0 - share));
            let tl = through(&city, up, 2000.0).powf(1.0 - LIGHT_FORWARD);
            // the strip's own light there: its surface veiled over its 2 km (none for a light over the sky), its light by tl
            let covered = if a > 0.0 { veil(surface) - veil(Vec3f::default()) * (1.0 - a) } else { Vec3f::default() };
            let own = (covered + light * tl) * k;
            let above = own + plain.radiance(up) * (1.0 - a);
            assert!(close3(city.radiance(up), above, 1e-4), "{el} deg, a {a}, share {share}: above the horizon {:?}, {above:?}", city.radiance(up));
            let fresnel = 0.02 + 0.98 * (1.0 - el.to_radians().sin()).powi(5);
            let sky_seen = veil_of(&city, down, plain.radiance(up) * (1.0 / k)) * k;
            let mirrored = city.radiance(down) - plain.radiance(down);
            let want = (own - sky_seen * a) * fresnel;
            println!("{el} deg, a {a}, share {share}: the puddle mirrors {:.4}, the light above by Fresnel {:.4}", mirrored.x, (own.x - sky_seen.x * a) * fresnel);
            assert!(close3(mirrored, want, 1e-3), "{el} deg, a {a}, share {share}: the puddle shows {mirrored:?}, the lights as the rain shows them, by Fresnel {want:?}");
            let aside = dir_at_u(u + 10.0 / 360.0, -el);
            assert_eq!(city.radiance(aside), plain.radiance(aside), "{el} deg: 10 degrees round the turn the puddle mirrors the sky alone");
        }
    }

    /// Plan hdri-6 fix wave 2 (F4, the final review's M3): the strip's sea mirrors what stands above the horizon and its
    /// distance plane hands the distance of what it mirrors, which is veiled along the mirror's direction over that
    /// distance, as the puddle beside it is (`over_city`), not along the texel's own ray down, which the weather ends on
    /// its floor short of it. The rain kind at night (its own wetness), an opaque sea texel 2.5 degrees down at Size 1
    /// (the strip's eye 6.3 m up: its own ground lies 144 m off there) showing the city 1.5 km off: the veil over 1.5 km
    /// up the mirror, the city's own, and a light there through the rain up the mirror too. The strip's ground there
    /// (100 m off) as before, bit for bit, and a band handed without its eye along the ray down.
    #[test]
    fn under_the_rain_kind_a_sea_texel_veils_what_it_mirrors_over_its_distance() {
        let p = rain_kind(-20.0, None);
        let s = [0.4, 0.3, 0.2, 1.0];
        let env = |far: f32, eye: f32, share: f32| {
            let (b, _) = block_band(s, -2.0, Some(far));
            let b = if share > 0.0 { lit(b, share, far) } else { b };
            let (q, images) = with_band(&p, b.with_eye(eye));
            Env::new_with(&q, &images)
        };
        let u = block_band(s, -2.0, None).1;
        let (d, v) = (dir_at_u(u, -2.5), dir_at_u(u, 2.5));
        let sea = env(1500.0, 6.3, 0.0);
        let rain = sea.weather.as_ref().expect("rain draws");
        let k = sea.scale();
        let veil = |d: Vec3f, c: Vec3f, far: f32| rain.veil_within(d, c, sea.weather_sun, sea.dome, far);
        let (up, down) = (through(&sea, v, 1500.0), through(&sea, d, 1500.0));
        println!("a sea texel 2.5 degrees down mirroring the city 1.5 km off passes {up:.4} up the mirror, {down:.4} along the ray down");
        assert!(up < 0.5 * down, "the premise: the ray down ends on the floor well short of 1.5 km ({up} up the mirror, {down} down)");
        let (got, want) = (sea.radiance(d), veil(v, rgb(s), 1500.0) * k);
        assert!(close3(got, want, 1e-5), "the sea {got:?}, veiled up the mirror over 1.5 km {want:?} (along the ray down {:?})", veil(d, rgb(s), 1500.0) * k);
        // a light it mirrors: through the rain up the mirror over the distance it burns at
        let lights = env(1500.0, 6.3, 1.0);
        let want = (veil(v, Vec3f::default(), 1500.0) + rgb(s) * rain.light_through(v, 1500.0)) * k;
        assert!(close3(lights.radiance(d), want, 1e-5), "the sea's mirrored light {:?}, through up the mirror {want:?}", lights.radiance(d));
        // the strip's own ground there, as before; a band without its eye along the ray down
        assert_eq!(env(100.0, 6.3, 0.0).radiance(d), env(100.0, 0.0, 0.0).radiance(d), "the ground 100 m off");
        let blind = env(1500.0, 0.0, 0.0).radiance(d);
        assert!(close3(blind, veil(d, rgb(s), 1500.0) * k, 1e-5), "without its eye {blind:?}: along the ray down");
    }

    /// Plan hdri-6 fix wave 2 (F5, the final review's M4): at the rain kind's own wetness the puddles lie in a field, and
    /// where the mirror shows the strip the share of the floor's view it takes is the field's (`Weather::mirror_share`,
    /// `Puddles::Field`): Fresnel's term inside a puddle, the damp ground's dull sheen between them, not the puddles'
    /// mean share (`Puddles::Even`). Under the rain kind at night an opaque lit block 2 km off, 6 to 18 degrees up and
    /// 120 degrees round: the floor 12 degrees down mirrors it, veiled once over its 2 km, by the field's share, inside a
    /// puddle and between two, where the mean share would be off by more than the tolerance.
    #[test]
    fn at_the_rain_kinds_own_wetness_the_puddles_mirror_the_city_by_their_field() {
        use crate::hdri::weather::Puddles;
        let p = rain_kind(-20.0, None);
        let s = [40.0, 30.0, 20.0, 1.0];
        let (w, h, top) = (360usize, 24usize, 20.5f32);
        let in_block = |i: usize| (2..=14).contains(&(i / w)) && (30..150).contains(&(i % w));
        let texels = (0..w * h).map(|i| if in_block(i) { s } else { [0.0; 4] }).collect();
        let dist = (0..w * h).map(|i| if in_block(i) { 2000.0 } else { 0.0 }).collect();
        let (q, images) = with_band(&p, SkylineBand::new(w, h, texels, dist, top, top - h as f32));
        let (plain, city) = (Env::new(&p), Env::new_with(&q, &images));
        let rain = city.weather.as_ref().expect("rain draws");
        let k = city.scale();
        let el = 12.0f32;
        // a look into a puddle and one between two, round the block's middle
        let looks: Vec<f32> = (0..10000).map(|i| (40.0 + i as f32 * 0.01) / 360.0).collect();
        let cover = |u: f32| rain.puddle_cover(dir_at_u(u, -el), Puddles::Field);
        let inside = looks.iter().copied().find(|&u| cover(u) >= 0.99).expect("a puddle 12 degrees down");
        let between = looks.iter().copied().find(|&u| cover(u) <= 0.01).expect("ground between the puddles 12 degrees down");
        println!("the rain kind's own wet ground: the puddles' mean share {:.3}", rain.puddle_share());
        for (name, u) in [("inside a puddle", inside), ("between the puddles", between)] {
            let (up, down) = (dir_at_u(u, el), dir_at_u(u, -el));
            let own = rain.veil_within(up, rgb(s), city.weather_sun, city.dome, 2000.0) * k;
            let sky_seen = veil_of(&city, down, plain.radiance(up) * (1.0 / k)) * k;
            let mirrored = city.radiance(down) - plain.radiance(down);
            let (field, even) = (rain.mirror_share(down, Puddles::Field), rain.mirror_share(down, Puddles::Even));
            let (want, mean) = ((own - sky_seen) * field, (own - sky_seen) * even);
            println!("{name} (cover {:.3}): the floor mirrors {:.4}, by the field's share {field:.4} {:.4}, by the mean share {even:.4} {:.4}", cover(u), mirrored.x, want.x, mean.x);
            assert!(close3(mirrored, want, 1e-4), "{name}: the floor mirrors {mirrored:?}, the city veiled once by the field's share {want:?}");
            assert!(!close3(mean, want, 1e-2), "the premise, {name}: the mean share's {mean:?} is the field's {want:?}");
        }
    }

    /// Plan hdri-7 D12: the strip's water mirrors the aurora as the map shows it. The light hands the aurora's share of the
    /// sea's two rings (those rings less the same rings without it), only while it draws: none without it, none by day,
    /// next to none under a closed deck, green toward it and nothing away from it. Height 10: the +1 ring passes under
    /// the border, and the +15 ring meets the curtain about 40 km over it, in the green (a 5-degree border put the +15
    /// look at about 245 km, where the classic table has no green: it is cut above 220 km).
    #[test]
    fn the_strip_gets_the_aurora_as_the_map_shows_it() {
        let mut p = HdriParams::default();
        sun_at(&mut p, -30.0, 200.0);
        p.sky.night.moon = false;
        assert_eq!(Env::new(&p).skyline_light().aurora, None, "no aurora: none handed");
        let mut on = p.clone();
        on.sky.night.aurora = Some(AuroraParams { height_deg: 10.0, ..AuroraParams::default() });
        let env = Env::new(&on);
        let l = env.skyline_light();
        let (low, high) = l.aurora.expect("the aurora draws");
        let north = RING / 2;
        assert!(high[north][1] > 0.0 && high[north][1] > high[north][0], "green toward the north: {:?}", high[north]);
        assert!(high[0] == [0.0; 3] && low[0] == [0.0; 3], "nothing toward the south");
        let without = Sampling { bare: true, unveiled: true, even_ground: true, no_aurora: true, ..Sampling::default() };
        for i in 0..RING {
            let u = (i as f32 + 0.5) / RING as f32;
            for (el, ring, sea) in [(RING_LOW_DEG, &low, &l.sea_low), (RING_HIGH_DEG, &high, &l.sea_high)] {
                let bare = env.base_under_skyline(dir_at_u(u, el), None, without);
                let want = [(sea[i][0] - bare.x).max(0.0), (sea[i][1] - bare.y).max(0.0), (sea[i][2] - bare.z).max(0.0)];
                assert_eq!(ring[i], want, "{i} at {el}");
            }
        }
        let mut day = on.clone();
        sun_at(&mut day, 20.0, 200.0);
        assert_eq!(Env::new(&day).skyline_light().aurora, None, "none by day");
        let mut deck = on.clone();
        deck.sky.weather = Some(WeatherParams { kind: "overcast".to_string(), ..WeatherParams::default() });
        let covered = Env::new(&deck).skyline_light().aurora.map_or(0.0, |(_, h)| h[north][1]);
        assert!(covered < 0.05 * high[north][1], "under the deck {covered} of {}", high[north][1]);
    }
}
