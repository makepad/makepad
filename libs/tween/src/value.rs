//! Animatable values and the colour maths used to interpolate them.
//!
//! GSAP tweens numbers and parses colours out of strings; here a value is a
//! small `Copy` enum and colours carry their interpolation space explicitly
//! (sRGB, linear light, HSV, OKLab or OKLCH).

use std::f64::consts::PI;

/// A straight-alpha, sRGB-encoded colour with channels in 0..1 (the Makepad
/// `Vec4f` colour convention).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rgba {
    /// Red, sRGB-encoded.
    pub r: f64,
    /// Green, sRGB-encoded.
    pub g: f64,
    /// Blue, sRGB-encoded.
    pub b: f64,
    /// Straight (not premultiplied) alpha.
    pub a: f64,
}

impl Rgba {
    /// A colour from its four channels.
    pub const fn new(r: f64, g: f64, b: f64, a: f64) -> Self {
        Self { r, g, b, a }
    }

    /// From packed `0xRRGGBBAA`, each byte divided by 255.
    pub fn from_u32(rgba: u32) -> Self {
        let ch = |shift: u32| ((rgba >> shift) & 0xff) as f64 / 255.0;
        Self::new(ch(24), ch(16), ch(8), ch(0))
    }

    /// To packed `0xRRGGBBAA`. Every channel is clamped to 0..1 and ROUNDED to
    /// the nearest byte (never truncated), so 0.5 packs as 0x80.
    pub fn to_u32(self) -> u32 {
        let ch = |c: f64| (c.clamp(0.0, 1.0) * 255.0).round() as u32;
        (ch(self.r) << 24) | (ch(self.g) << 16) | (ch(self.b) << 8) | ch(self.a)
    }

    /// From an `[r, g, b, a]` f32 quad (a shader `vec4`).
    pub fn from_f32(v: [f32; 4]) -> Self {
        Self::new(v[0] as f64, v[1] as f64, v[2] as f64, v[3] as f64)
    }

    /// To an `[r, g, b, a]` f32 quad (a shader `vec4`).
    pub fn to_f32(self) -> [f32; 4] {
        [self.r as f32, self.g as f32, self.b as f32, self.a as f32]
    }
}

/// A value a tween can move: the right-hand side of a GSAP vars entry.
///
/// Every kind is carried as up to four f64 lanes while it animates
/// ([`TweenValue::to_lanes`]); `Int` rounds on the way out and `Color`
/// interpolates in its [`ColorSpace`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TweenValue {
    /// A plain number.
    F64(f64),
    /// Two lanes (a point, a size).
    Vec2([f64; 2]),
    /// Three lanes.
    Vec3([f64; 3]),
    /// Four lanes (a rect, an inset).
    Vec4([f64; 4]),
    /// An integer: interpolated as f64, rounded half away from zero on output
    /// (GSAP `snap: 1` / `roundProps`).
    Int(i64),
    /// A colour, interpolated in the tween's colour space.
    Color(Rgba),
}

/// The kind of a [`TweenValue`], without its payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ValueKind {
    /// [`TweenValue::F64`].
    #[default]
    F64,
    /// [`TweenValue::Vec2`].
    Vec2,
    /// [`TweenValue::Vec3`].
    Vec3,
    /// [`TweenValue::Vec4`].
    Vec4,
    /// [`TweenValue::Int`].
    Int,
    /// [`TweenValue::Color`].
    Color,
}

/// The space a colour tween interpolates in. GSAP interpolates colour
/// channels in sRGB (or HSL when the values are `hsl()` strings); the
/// perceptual spaces are an extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColorSpace {
    /// Straight lerp of the sRGB-encoded channels (GSAP's behaviour).
    #[default]
    Srgb,
    /// Lerp in linear light (physically correct blending; brighter midpoints).
    Linear,
    /// Hue, saturation, value; hue takes the short way round.
    Hsv,
    /// OKLab (perceptually uniform lightness).
    Oklab,
    /// OKLCH (OKLab in polar form); hue takes the short way round.
    Oklch,
}

impl TweenValue {
    /// The kind of this value.
    pub fn kind(&self) -> ValueKind {
        match self {
            Self::F64(_) => ValueKind::F64,
            Self::Vec2(_) => ValueKind::Vec2,
            Self::Vec3(_) => ValueKind::Vec3,
            Self::Vec4(_) => ValueKind::Vec4,
            Self::Int(_) => ValueKind::Int,
            Self::Color(_) => ValueKind::Color,
        }
    }

    /// The value as four lanes: unused lanes are 0, `Int` becomes f64 and a
    /// colour is `[r, g, b, a]` (still sRGB-encoded).
    pub fn to_lanes(&self) -> [f64; 4] {
        match *self {
            Self::F64(v) => [v, 0.0, 0.0, 0.0],
            Self::Vec2([x, y]) => [x, y, 0.0, 0.0],
            Self::Vec3([x, y, z]) => [x, y, z, 0.0],
            Self::Vec4(v) => v,
            Self::Int(i) => [i as f64, 0.0, 0.0, 0.0],
            Self::Color(c) => [c.r, c.g, c.b, c.a],
        }
    }

    /// A value of `kind` from four lanes. `Int` rounds lane 0 half away from
    /// zero (`2.5 -> 3`, `-2.5 -> -3`).
    pub fn from_lanes(kind: ValueKind, l: [f64; 4]) -> Self {
        match kind {
            ValueKind::F64 => Self::F64(l[0]),
            ValueKind::Vec2 => Self::Vec2([l[0], l[1]]),
            ValueKind::Vec3 => Self::Vec3([l[0], l[1], l[2]]),
            ValueKind::Vec4 => Self::Vec4(l),
            ValueKind::Int => Self::Int(l[0].round() as i64),
            ValueKind::Color => Self::Color(Rgba::new(l[0], l[1], l[2], l[3])),
        }
    }

    /// Lane 0 as a number (the whole value for `F64` and `Int`).
    pub fn as_f64(&self) -> f64 {
        self.to_lanes()[0]
    }

    /// The value `r` of the way from `a` to `b` (r is the eased ratio, so it
    /// may leave 0..1), with the same maths the engine uses per frame:
    /// `from + (to - from) * r` per lane, colours in `space` (see
    /// [`encode_pair`]), `Int` rounded. The result has `a`'s kind.
    pub fn lerp(a: &Self, b: &Self, r: f64, space: ColorSpace) -> Self {
        match (a, b) {
            (Self::Color(ca), Self::Color(cb)) => {
                let (fa, fb) = encode_pair(*ca, *cb, space);
                Self::Color(decode(lerp_lanes(fa, fb, r), space))
            }
            _ => Self::from_lanes(a.kind(), lerp_lanes(a.to_lanes(), b.to_lanes(), r)),
        }
    }
}

/// Per-lane `a + (b - a) * r`: the one interpolation formula of the engine.
#[inline]
pub fn lerp_lanes(a: [f64; 4], b: [f64; 4], r: f64) -> [f64; 4] {
    [
        a[0] + (b[0] - a[0]) * r,
        a[1] + (b[1] - a[1]) * r,
        a[2] + (b[2] - a[2]) * r,
        a[3] + (b[3] - a[3]) * r,
    ]
}

impl From<f64> for TweenValue {
    fn from(v: f64) -> Self {
        Self::F64(v)
    }
}

impl From<[f64; 2]> for TweenValue {
    fn from(v: [f64; 2]) -> Self {
        Self::Vec2(v)
    }
}

impl From<[f64; 3]> for TweenValue {
    fn from(v: [f64; 3]) -> Self {
        Self::Vec3(v)
    }
}

impl From<[f64; 4]> for TweenValue {
    fn from(v: [f64; 4]) -> Self {
        Self::Vec4(v)
    }
}

impl From<i64> for TweenValue {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}

impl From<Rgba> for TweenValue {
    fn from(v: Rgba) -> Self {
        Self::Color(v)
    }
}

/// The sRGB transfer function, encoded to linear light, for one channel.
#[inline]
pub fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The inverse sRGB transfer function, linear light to encoded, for one channel.
#[inline]
pub fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB-encoded `[r, g, b]` to `[h, s, v]` with the hue in degrees `[0, 360)`
/// (0 for greys).
pub fn rgb_to_hsv(rgb: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = rgb;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let h = if delta <= 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let s = if max <= 0.0 { 0.0 } else { delta / max };
    [below_360(h), s, max]
}

/// `rem_euclid(360)` can return exactly 360 for a hue a hair below 0.
#[inline]
fn below_360(h: f64) -> f64 {
    if h >= 360.0 {
        0.0
    } else {
        h
    }
}

/// `[h, s, v]` (hue in degrees, any range) to sRGB-encoded `[r, g, b]`.
pub fn hsv_to_rgb(hsv: [f64; 3]) -> [f64; 3] {
    let [h, s, v] = hsv;
    let c = v * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
}

/// Linear-light `[r, g, b]` to OKLab `[L, a, b]` (Björn Ottosson's matrices).
pub fn linear_to_oklab(rgb: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = rgb;
    let l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
    let m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
    let s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

/// OKLab `[L, a, b]` to linear-light `[r, g, b]` (not clamped).
///
/// The matrices are the exact inverses of the forward ones in
/// [`linear_to_oklab`] (Ottosson's published 10-digit inverses are only
/// accurate to about 5e-9 after the sRGB transfer), so a colour survives an
/// encode / decode round trip to 1e-9.
pub fn oklab_to_linear(lab: [f64; 3]) -> [f64; 3] {
    let [ll, a, b] = lab;
    let l = 0.9999999984505198 * ll + 0.39633779217376786 * a + 0.2158037580607588 * b;
    let m = 1.0000000088817609 * ll - 0.10556134232365635 * a - 0.06385417477170591 * b;
    let s = 1.0000000546724108 * ll - 0.08948418209496575 * a - 1.2914855378640917 * b;
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [
        4.076741661347994 * l - 3.3077115904081933 * m + 0.2309699287294279 * s,
        -1.268438004092176 * l + 2.6097574006633715 * m - 0.3413193963102196 * s,
        -0.004196086541837109 * l - 0.7034186144594496 * m + 1.7076147009309448 * s,
    ]
}

/// OKLab `[L, a, b]` to OKLCH `[L, C, h]`, hue in degrees `[0, 360)`.
pub fn oklab_to_oklch(lab: [f64; 3]) -> [f64; 3] {
    let [l, a, b] = lab;
    [
        l,
        a.hypot(b),
        below_360((b.atan2(a) * (180.0 / PI)).rem_euclid(360.0)),
    ]
}

/// OKLCH `[L, C, h]` (hue in degrees) to OKLab `[L, a, b]`.
pub fn oklch_to_oklab(lch: [f64; 3]) -> [f64; 3] {
    let [l, c, h] = lch;
    let (sin, cos) = (h * (PI / 180.0)).sin_cos();
    [l, c * cos, c * sin]
}

/// A colour in the lanes of its interpolation space, alpha in lane 3:
/// Srgb `[r, g, b, a]`, Linear `[r, g, b, a]` in linear light, Hsv
/// `[h, s, v, a]`, Oklab `[L, a, b, alpha]`, Oklch `[L, C, h, alpha]`.
pub fn encode(c: Rgba, space: ColorSpace) -> [f64; 4] {
    let rgb = [c.r, c.g, c.b];
    let [x, y, z] = match space {
        ColorSpace::Srgb => rgb,
        ColorSpace::Linear => rgb.map(srgb_to_linear),
        ColorSpace::Hsv => rgb_to_hsv(rgb),
        ColorSpace::Oklab => linear_to_oklab(rgb.map(srgb_to_linear)),
        ColorSpace::Oklch => oklab_to_oklch(linear_to_oklab(rgb.map(srgb_to_linear))),
    };
    [x, y, z, c.a]
}

/// Lanes in `space` back to an sRGB colour: hue renormalised to `[0, 360)`,
/// rgb clamped to 0..1 before the transfer function (out-of-gamut OKLab and
/// overshooting eases stay displayable). Srgb lanes pass through untouched.
pub fn decode(l: [f64; 4], space: ColorSpace) -> Rgba {
    let clamp = |v: [f64; 3]| v.map(|c| c.clamp(0.0, 1.0));
    let [r, g, b] = match space {
        ColorSpace::Srgb => [l[0], l[1], l[2]],
        ColorSpace::Linear => clamp([l[0], l[1], l[2]]).map(linear_to_srgb),
        ColorSpace::Hsv => clamp(hsv_to_rgb([l[0], l[1], l[2]])),
        ColorSpace::Oklab => clamp(oklab_to_linear([l[0], l[1], l[2]])).map(linear_to_srgb),
        ColorSpace::Oklch => {
            let lab = oklch_to_oklab([l[0], l[1], l[2]]);
            clamp(oklab_to_linear(lab)).map(linear_to_srgb)
        }
    };
    Rgba::new(r, g, b, l[3])
}

/// Chroma below which a colour counts as achromatic (grey) and has no hue of
/// its own.
const ACHROMATIC: f64 = 1e-6;

/// Encodes both ends of a colour tween, ready for a plain per-lane lerp.
///
/// In the polar spaces (Hsv, Oklch) an end without chroma borrows the other
/// end's hue (both grey: hue 0), then the `to` hue is unwrapped so the lerp
/// takes the short way round (`to.h = from.h + d`, `d` in `(-180, 180]`).
/// [`decode`] renormalises the hue. The engine does this once, at init.
pub fn encode_pair(from: Rgba, to: Rgba, space: ColorSpace) -> ([f64; 4], [f64; 4]) {
    let mut a = encode(from, space);
    let mut b = encode(to, space);
    let (hue, chroma) = match space {
        ColorSpace::Hsv => (0, 1),
        ColorSpace::Oklch => (2, 1),
        _ => return (a, b),
    };
    let grey_a = a[chroma] < ACHROMATIC;
    let grey_b = b[chroma] < ACHROMATIC;
    match (grey_a, grey_b) {
        (true, true) => {
            a[hue] = 0.0;
            b[hue] = 0.0;
        }
        (true, false) => a[hue] = b[hue],
        (false, true) => b[hue] = a[hue],
        (false, false) => {}
    }
    let mut d = (b[hue] - a[hue]).rem_euclid(360.0);
    if d > 180.0 {
        d -= 360.0;
    }
    b[hue] = a[hue] + d;
    (a, b)
}
