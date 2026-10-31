//! New palettes from a scene's palette, computed in OKLCH (lightness,
//! chroma, hue as the eye sees them), each colour kept in its role: a
//! generator moves the colours the scene has, it does not invent a list.
//!
//! - harmony from a base colour (complementary, analogous, triadic, split
//!   complementary, tetradic, monochrome): the hues the rule gives are
//!   dealt to the colours by how much each is used, each keeping its own
//!   lightness and chroma;
//! - moves: warmer or colder, a hue turn, more or less contrast (lightness
//!   spread round its middle), chroma, a lightness ramp (the colours spaced
//!   evenly in lightness, their order kept);
//! - random within constraints (lightness order kept, the accent kept,
//!   the darkest and lightest kept: contrast with the paper);
//! - shuffled roles, a mix between two palettes, colours taken from a
//!   frame, and the glow of light over 1.
//!
//! A colour of light over 1 (`color(3, 2, 1.2)`) is moved as the colour it
//! glows in, and keeps its intensity.

use crate::color_picker::split_hdr;

/// sRGB channel to linear light.
fn lin(c: f64) -> f64 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn gam(c: f64) -> f64 {
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.max(0.0).powf(1.0 / 2.4) - 0.055 }
}

/// An sRGB colour (0..1) as OKLCH: lightness 0..1, chroma, hue in degrees.
pub fn to_oklch(rgb: [f64; 3]) -> [f64; 3] {
    let (r, g, b) = (lin(rgb[0]), lin(rgb[1]), lin(rgb[2]));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    let ll = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s;
    let a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
    let bb = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;
    let c = (a * a + bb * bb).sqrt();
    let h = bb.atan2(a).to_degrees().rem_euclid(360.0);
    [ll, c, h]
}

/// OKLCH to sRGB, brought into gamut by lowering chroma.
pub fn from_oklch(lch: [f64; 3]) -> [f64; 3] {
    let raw = |c: f64| {
        let h = lch[2].to_radians();
        let (a, b) = (c * h.cos(), c * h.sin());
        let l_ = lch[0] + 0.3963377774 * a + 0.2158037573 * b;
        let m_ = lch[0] - 0.1055613458 * a - 0.0638541728 * b;
        let s_ = lch[0] - 0.0894841775 * a - 1.2914855480 * b;
        let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
        [
            4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
            -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
            -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
        ]
    };
    let mut c = lch[1].max(0.0);
    for _ in 0..24 {
        let v = raw(c);
        if v.iter().all(|x| (-1e-4..=1.0001).contains(x)) {
            return v.map(|x| gam(x.clamp(0.0, 1.0)));
        }
        c *= 0.9;
    }
    raw(0.0).map(|x| gam(x.clamp(0.0, 1.0)))
}

/// A colour as OKLCH, and the intensity it is light over 1 by.
fn lch_of(rgba: [f64; 4]) -> ([f64; 3], f64) {
    let (base, k) = split_hdr(rgba);
    (to_oklch([base[0], base[1], base[2]]), k)
}

fn rgba_of(lch: [f64; 3], k: f64, a: f64) -> [f64; 4] {
    let c = from_oklch(lch);
    [c[0] * k, c[1] * k, c[2] * k, a]
}

/// Each colour moved by `f` in OKLCH (its intensity and alpha kept).
fn each(colours: &[[f64; 4]], f: impl Fn(usize, [f64; 3]) -> [f64; 3]) -> Vec<[f64; 4]> {
    colours.iter().enumerate().map(|(i, c)| {
        let (lch, k) = lch_of(*c);
        rgba_of(f(i, lch), k, c[3])
    }).collect()
}

/// The harmony rules.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Harmony {
    Complementary,
    Analogous,
    Triadic,
    Split,
    Tetradic,
    Monochrome,
}

impl Harmony {
    pub const ALL: [Harmony; 6] = [Self::Complementary, Self::Analogous, Self::Triadic, Self::Split, Self::Tetradic, Self::Monochrome];

    pub fn name(self) -> &'static str {
        match self {
            Self::Complementary => "Complementary",
            Self::Analogous => "Analogous",
            Self::Triadic => "Triadic",
            Self::Split => "Split",
            Self::Tetradic => "Tetradic",
            Self::Monochrome => "Mono",
        }
    }

    /// The hue offsets (degrees) the rule gives from the base's.
    fn offsets(self) -> &'static [f64] {
        match self {
            Self::Complementary => &[0.0, 180.0],
            Self::Analogous => &[0.0, 30.0, -30.0, 60.0],
            Self::Triadic => &[0.0, 120.0, 240.0],
            Self::Split => &[0.0, 150.0, 210.0],
            Self::Tetradic => &[0.0, 90.0, 180.0, 270.0],
            Self::Monochrome => &[0.0],
        }
    }
}

/// The palette made by a harmony rule from `base`: the rule's hues dealt
/// to the colours in order (the most used first: the first gets the
/// base's hue), each keeping its lightness and chroma; greys stay grey.
pub fn harmony(colours: &[[f64; 4]], base: [f64; 4], rule: Harmony) -> Vec<[f64; 4]> {
    let (b, _) = lch_of(base);
    let offsets = rule.offsets();
    each(colours, |i, lch| {
        if lch[1] < 0.02 {
            return lch;
        }
        [lch[0], lch[1], (b[2] + offsets[i % offsets.len()]).rem_euclid(360.0)]
    })
}

/// Moves of the whole palette, each by `amount` (-1..1, 0: none).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Move {
    /// Hues toward orange (> 0) or toward blue (< 0).
    Warmth,
    /// Every hue turned by amount × 180°.
    Hue,
    /// Lightness spread round its middle (> 0 more).
    Contrast,
    /// Chroma scaled (> 0 more).
    Chroma,
    /// Lightness spaced evenly between the darkest and the lightest, order
    /// kept (amount: how far toward even).
    Ramp,
    /// Light over 1 scaled (> 0 brighter glows; plain colours stay).
    Glow,
}

impl Move {
    pub const ALL: [Move; 6] = [Self::Warmth, Self::Hue, Self::Contrast, Self::Chroma, Self::Ramp, Self::Glow];

    pub fn name(self) -> &'static str {
        match self {
            Self::Warmth => "Warmth",
            Self::Hue => "Hue",
            Self::Contrast => "Contrast",
            Self::Chroma => "Chroma",
            Self::Ramp => "Ramp",
            Self::Glow => "Glow",
        }
    }
}

/// Hue `h` moved toward `to` by `t` (0..1), the short way round.
fn hue_toward(h: f64, to: f64, t: f64) -> f64 {
    let d = ((to - h + 540.0) % 360.0) - 180.0;
    (h + d * t).rem_euclid(360.0)
}

pub fn moved(colours: &[[f64; 4]], m: Move, amount: f64) -> Vec<[f64; 4]> {
    let amount = amount.clamp(-1.0, 1.0);
    match m {
        Move::Warmth => each(colours, |_, lch| {
            if lch[1] < 0.02 {
                return lch;
            }
            let to = if amount >= 0.0 { 55.0 } else { 250.0 };
            [lch[0], lch[1], hue_toward(lch[2], to, amount.abs() * 0.5)]
        }),
        Move::Hue => each(colours, |_, lch| [lch[0], lch[1], (lch[2] + amount * 180.0).rem_euclid(360.0)]),
        Move::Contrast => {
            let ls: Vec<f64> = colours.iter().map(|c| lch_of(*c).0[0]).collect();
            let mid = ls.iter().sum::<f64>() / ls.len().max(1) as f64;
            let k = 1.0 + amount;
            each(colours, |_, lch| [(mid + (lch[0] - mid) * k).clamp(0.0, 1.0), lch[1], lch[2]])
        }
        Move::Chroma => each(colours, |_, lch| [lch[0], (lch[1] * (1.0 + amount)).max(0.0), lch[2]]),
        Move::Ramp => {
            let ls: Vec<f64> = colours.iter().map(|c| lch_of(*c).0[0]).collect();
            let mut order: Vec<usize> = (0..ls.len()).collect();
            order.sort_by(|a, b| ls[*a].total_cmp(&ls[*b]));
            let (lo, hi) = (ls.iter().cloned().fold(f64::MAX, f64::min), ls.iter().cloned().fold(f64::MIN, f64::max));
            let n = ls.len().max(2) - 1;
            let mut even = vec![0.0; ls.len()];
            for (rank, i) in order.iter().enumerate() {
                even[*i] = lo + (hi - lo) * rank as f64 / n as f64;
            }
            each(colours, |i, lch| [lch[0] + (even[i] - lch[0]) * amount.abs(), lch[1], lch[2]])
        }
        Move::Glow => colours.iter().map(|c| {
            let (base, k) = split_hdr(*c);
            if k <= 1.0 {
                return *c;
            }
            let k = (k * (1.0 + amount)).max(1.0);
            [base[0] * k, base[1] * k, base[2] * k, c[3]]
        }).collect(),
    }
}

/// A small hash: the same seed, the same palette.
fn rnd(seed: u64, i: u64) -> f64 {
    let mut x = seed.wrapping_mul(0x9e3779b97f4a7c15) ^ i.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 31;
    x = x.wrapping_mul(0x94d049bb133111eb);
    x ^= x >> 29;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

/// What a random palette keeps.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Keep {
    /// Each colour's lightness (so the order and the contrast hold).
    pub lightness: bool,
    /// The most saturated colour, as it is.
    pub accent: bool,
    /// The darkest and the lightest (the paper and the ink).
    pub paper: bool,
}

pub fn random(colours: &[[f64; 4]], seed: u64, keep: Keep) -> Vec<[f64; 4]> {
    let lchs: Vec<[f64; 3]> = colours.iter().map(|c| lch_of(*c).0).collect();
    let accent = lchs.iter().enumerate().max_by(|a, b| a.1[1].total_cmp(&b.1[1])).map(|(i, _)| i);
    let dark = lchs.iter().enumerate().min_by(|a, b| a.1[0].total_cmp(&b.1[0])).map(|(i, _)| i);
    let light = lchs.iter().enumerate().max_by(|a, b| a.1[0].total_cmp(&b.1[0])).map(|(i, _)| i);
    let base_hue = rnd(seed, 999) * 360.0;
    each(colours, |i, lch| {
        if (keep.accent && Some(i) == accent) || (keep.paper && (Some(i) == dark || Some(i) == light)) {
            return lch;
        }
        let r = |k: u64| rnd(seed, i as u64 * 7 + k);
        // Hues near a shared base, so the palette holds together.
        let h = (base_hue + (r(1) - 0.5) * 140.0).rem_euclid(360.0);
        let l = if keep.lightness { lch[0] } else { 0.25 + r(2) * 0.65 };
        let c = if lch[1] < 0.02 { lch[1] } else { 0.04 + r(3) * 0.16 };
        [l, c, h]
    })
}

/// The colours dealt to other roles (a rotation by the seed, never the
/// same order back unless there is one colour).
pub fn shuffled(colours: &[[f64; 4]], seed: u64) -> Vec<[f64; 4]> {
    let n = colours.len();
    if n < 2 {
        return colours.to_vec();
    }
    let mut order: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        let j = (rnd(seed, i as u64) * (i + 1) as f64) as usize;
        order.swap(i, j.min(i));
    }
    if order.iter().enumerate().all(|(i, j)| i == *j) {
        order.rotate_left(1);
    }
    order.iter().map(|&j| colours[j]).collect()
}

/// Between two palettes (`t` 0: a, 1: b), colour by colour in OKLab.
pub fn mix(a: &[[f64; 4]], b: &[[f64; 4]], t: f64) -> Vec<[f64; 4]> {
    a.iter().enumerate().map(|(i, ca)| {
        let Some(cb) = b.get(i % b.len().max(1)).copied() else { return *ca };
        let (la, ka) = lch_of(*ca);
        let (lb, kb) = lch_of(cb);
        let lab = |l: [f64; 3]| [l[0], l[1] * l[2].to_radians().cos(), l[1] * l[2].to_radians().sin()];
        let (pa, pb) = (lab(la), lab(lb));
        let p = [pa[0] + (pb[0] - pa[0]) * t, pa[1] + (pb[1] - pa[1]) * t, pa[2] + (pb[2] - pa[2]) * t];
        let lch = [p[0], (p[1] * p[1] + p[2] * p[2]).sqrt(), p[2].atan2(p[1]).to_degrees().rem_euclid(360.0)];
        rgba_of(lch, ka + (kb - ka) * t, ca[3] + (cb[3] - ca[3]) * t)
    }).collect()
}

/// Colours taken from elsewhere (a frame's, a saved palette's) dealt to
/// the scene's by lightness: the darkest to the darkest, and so on; with
/// fewer colours than roles, the nearest in lightness.
pub fn dealt(colours: &[[f64; 4]], from: &[[f64; 4]]) -> Vec<[f64; 4]> {
    if from.is_empty() {
        return colours.to_vec();
    }
    let l = |c: &[f64; 4]| lch_of(*c).0[0];
    let mut src: Vec<[f64; 4]> = from.to_vec();
    src.sort_by(|a, b| l(a).total_cmp(&l(b)));
    let mut order: Vec<usize> = (0..colours.len()).collect();
    order.sort_by(|a, b| l(&colours[*a]).total_cmp(&l(&colours[*b])));
    let mut out = colours.to_vec();
    for (rank, &i) in order.iter().enumerate() {
        let k = if order.len() <= 1 { 0 } else { rank * (src.len() - 1) / (order.len() - 1) };
        let (_, intensity) = split_hdr(colours[i]);
        let c = src[k];
        out[i] = [c[0] * intensity, c[1] * intensity, c[2] * intensity, colours[i][3]];
    }
    out
}
