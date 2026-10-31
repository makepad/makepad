//! Light-source spectra (phase 1b): what a lamp emits per wavelength, and the
//! colour that makes in the map.
//!
//! A spectrum here is relative power in 81 bands of 5 nm from 380 to 780 nm.
//! It is integrated against the CIE 1931 2-degree colour-matching functions
//! to XYZ and taken to linear Rec.709 at luminance 1, so a source is a drop-in
//! for `kelvin_to_rgb`. Spectra only derive a light's RGB (and its CRI
//! readout, cri.rs); the map itself stays RGB.
//!
//! The sources:
//! - **Planck**: a blackbody, for tungsten and halogen.
//! - **CIE daylight** (D series): S0 + M1 S1 + M2 S2 for a correlated colour
//!   temperature from 4000 K to 25000 K.
//! - **LED**: a blue pump with a green and a red phosphor, mixed to sit on
//!   the white locus at the asked colour temperature. One *quality* knob
//!   moves the phosphors apart and widens them: 0 is a cheap yellow-phosphor
//!   LED (CRI about 53), 0.5 a CRI-80 one, 1 a full-spectrum one (CRI about 95).
//!   The named LEDs fix both numbers; the source `led` takes the light's own
//!   Kelvin and its `source_quality` (the parametric LED).
//! - **Fluorescent**: a three-band tube (narrow red and green rare-earth
//!   phosphors, a blue one and the mercury lines), mixed the same way.
//! - **Sodium**: the low-pressure lamp's single 589 nm line, and the
//!   high-pressure lamp's broadened, self-reversed version of it.
//!
//! **Why a table and not the analytic fit.** The colour-matching functions
//! are the CIE's own 5 nm table rather than the Wyman-Sloan-Shirley (2013)
//! Gaussian fit. The CIE 13.3 reflectances and the daylight components are
//! 5 nm tables anyway, so the table makes every integral a plain 81-term sum
//! on one grid, and it is exact: D65 comes out at x, y = 0.3127, 0.3290 and
//! white within 0.1 %, and a blackbody matches `kelvin_to_rgb` to 0.002. The
//! fit is off by up to 1 % of the peak, which would show as a tint on
//! exactly the sources that should be neutral. It costs 243 numbers.
//!
//! Data: CIE 1931 standard colorimetric observer and the CIE daylight
//! components S0, S1, S2 (CIE 15, Colorimetry), published tables. The lamp
//! models are generic shapes, not measurements of any product.

use makepad_draw::*;
use super::LightParams;
use crate::sky::{luminance, xyz_to_linear_rgb};

/// Bands in a spectrum: 380, 385, ... 780 nm.
pub const SAMPLES: usize = 81;
/// The first band's wavelength, nm.
pub const LAMBDA_MIN_NM: f32 = 380.0;
/// The band width, nm.
pub const LAMBDA_STEP_NM: f32 = 5.0;

/// Relative spectral power per band. Only the shape matters: every reader
/// normalises (to luminance 1, or to Y = 100).
pub type Spectrum = [f32; SAMPLES];

/// The wavelength of band `i`, nm.
pub fn wavelength_nm(i: usize) -> f32 {
    LAMBDA_MIN_NM + LAMBDA_STEP_NM * i as f32
}

/// The CIE 1931 2-degree colour-matching functions (x, y, z bar), 380..=780 nm
/// in 5 nm steps.
pub const CMF_1931: [[f32; 3]; SAMPLES] = [
    [0.001368, 0.000039, 0.006450], [0.002236, 0.000064, 0.010550], [0.004243, 0.000120, 0.020050],
    [0.007650, 0.000217, 0.036210], [0.014310, 0.000396, 0.067850], [0.023190, 0.000640, 0.110200],
    [0.043510, 0.001210, 0.207400], [0.077630, 0.002180, 0.371300], [0.134380, 0.004000, 0.645600],
    [0.214770, 0.007300, 1.039050], [0.283900, 0.011600, 1.385600], [0.328500, 0.016840, 1.622960],
    [0.348280, 0.023000, 1.747060], [0.348060, 0.029800, 1.782600], [0.336200, 0.038000, 1.772110],
    [0.318700, 0.048000, 1.744100], [0.290800, 0.060000, 1.669200], [0.251100, 0.073900, 1.528100],
    [0.195360, 0.090980, 1.287640], [0.142100, 0.112600, 1.041900], [0.095640, 0.139020, 0.812950],
    [0.057950, 0.169300, 0.616200], [0.032010, 0.208020, 0.465180], [0.014700, 0.258600, 0.353300],
    [0.004900, 0.323000, 0.272000], [0.002400, 0.407300, 0.212300], [0.009300, 0.503000, 0.158200],
    [0.029100, 0.608200, 0.111700], [0.063270, 0.710000, 0.078250], [0.109600, 0.793200, 0.057250],
    [0.165500, 0.862000, 0.042160], [0.225750, 0.914850, 0.029840], [0.290400, 0.954000, 0.020300],
    [0.359700, 0.980300, 0.013400], [0.433450, 0.994950, 0.008750], [0.512050, 1.000000, 0.005750],
    [0.594500, 0.995000, 0.003900], [0.678400, 0.978600, 0.002750], [0.762100, 0.952000, 0.002100],
    [0.842500, 0.915400, 0.001800], [0.916300, 0.870000, 0.001650], [0.978600, 0.816300, 0.001400],
    [1.026300, 0.757000, 0.001100], [1.056700, 0.694900, 0.001000], [1.062200, 0.631000, 0.000800],
    [1.045600, 0.566800, 0.000600], [1.002600, 0.503000, 0.000340], [0.938400, 0.441200, 0.000240],
    [0.854450, 0.381000, 0.000190], [0.751400, 0.321000, 0.000100], [0.642400, 0.265000, 0.000050],
    [0.541900, 0.217000, 0.000030], [0.447900, 0.175000, 0.000020], [0.360800, 0.138200, 0.000010],
    [0.283500, 0.107000, 0.000000], [0.218700, 0.081600, 0.000000], [0.164900, 0.061000, 0.000000],
    [0.121200, 0.044580, 0.000000], [0.087400, 0.032000, 0.000000], [0.063600, 0.023200, 0.000000],
    [0.046770, 0.017000, 0.000000], [0.032900, 0.011920, 0.000000], [0.022700, 0.008210, 0.000000],
    [0.015840, 0.005723, 0.000000], [0.011359, 0.004102, 0.000000], [0.008111, 0.002929, 0.000000],
    [0.005790, 0.002091, 0.000000], [0.004109, 0.001484, 0.000000], [0.002899, 0.001047, 0.000000],
    [0.002049, 0.000740, 0.000000], [0.001440, 0.000520, 0.000000], [0.001000, 0.000361, 0.000000],
    [0.000690, 0.000249, 0.000000], [0.000476, 0.000172, 0.000000], [0.000332, 0.000120, 0.000000],
    [0.000235, 0.000085, 0.000000], [0.000166, 0.000060, 0.000000], [0.000117, 0.000042, 0.000000],
    [0.000083, 0.000030, 0.000000], [0.000059, 0.000021, 0.000000], [0.000042, 0.000015, 0.000000],
];

/// The CIE daylight components S0, S1, S2 at 10 nm from 380 to 780 nm. The
/// 5 nm bands between them are linear interpolations, as in the CIE's own
/// 5 nm tables.
const DAYLIGHT_S: [[f32; 3]; 41] = [
    [63.4, 38.5, 3.0], [65.8, 35.0, 1.2], [94.8, 43.4, -1.1], [104.8, 46.3, -0.5], [105.9, 43.9, -0.7],
    [96.8, 37.1, -1.2], [113.9, 36.7, -2.6], [125.6, 35.9, -2.9], [125.5, 32.6, -2.8], [121.3, 27.9, -2.6],
    [121.3, 24.3, -2.6], [113.5, 20.1, -1.8], [113.1, 16.2, -1.5], [110.8, 13.2, -1.3], [106.5, 8.6, -1.2],
    [108.8, 6.1, -1.0], [105.3, 4.2, -0.5], [104.4, 1.9, -0.3], [100.0, 0.0, 0.0], [96.0, -1.6, 0.2],
    [95.1, -3.5, 0.5], [89.1, -3.5, 2.1], [90.5, -5.8, 3.2], [90.3, -7.2, 4.1], [88.4, -8.6, 4.7],
    [84.0, -9.5, 5.1], [85.1, -10.9, 6.7], [81.9, -10.7, 7.3], [82.6, -12.0, 8.6], [84.9, -14.0, 9.8],
    [81.3, -13.6, 10.2], [71.9, -12.0, 8.3], [74.3, -13.3, 9.6], [76.4, -12.9, 8.5], [63.3, -10.6, 7.0],
    [71.7, -11.6, 7.6], [77.0, -12.2, 8.0], [65.2, -10.2, 6.7], [47.7, -7.8, 5.2], [68.6, -11.2, 7.4],
    [65.0, -10.4, 6.8],
];

/// Planck's second radiation constant, in nm K.
const C2_NM_K: f64 = 1.438_776_877e7;

/// A blackbody at `kelvin`, scaled to 1 at 560 nm. Held to 500..=100000 K;
/// NaN reads as 6500 K.
pub fn planck(kelvin: f32) -> Spectrum {
    let t = if kelvin.is_finite() { kelvin.clamp(500.0, 100_000.0) as f64 } else { 6500.0 };
    // f64: the exponential reaches e^75 at 380 nm and 500 K.
    let radiance = |nm: f64| 1.0 / (nm.powi(5) * ((C2_NM_K / (nm * t)).exp() - 1.0));
    let norm = radiance(560.0);
    std::array::from_fn(|i| (radiance(wavelength_nm(i) as f64) / norm) as f32)
}

/// CIE daylight at the correlated colour temperature `kelvin` (D65 is
/// 6504 K, D55 5503 K), scaled to 1 at 560 nm. The CIE defines it from
/// 4000 K to 25000 K; the input is held to that range, and NaN reads as 6504 K.
pub fn daylight(kelvin: f32) -> Spectrum {
    let t = if kelvin.is_finite() { kelvin.clamp(4000.0, 25_000.0) as f64 } else { 6504.0 };
    // The daylight locus (CIE 15): x from the temperature, y from x.
    let x = if t <= 7000.0 {
        -4.6070e9 / (t * t * t) + 2.9678e6 / (t * t) + 0.09911e3 / t + 0.244063
    } else {
        -2.0064e9 / (t * t * t) + 1.9018e6 / (t * t) + 0.24748e3 / t + 0.237040
    };
    let y = -3.000 * x * x + 2.870 * x - 0.275;
    let m = 0.0241 + 0.2562 * x - 0.7341 * y;
    let m1 = (-1.3515 - 1.7703 * x + 5.9114 * y) / m;
    let m2 = (0.0300 - 31.4424 * x + 30.0717 * y) / m;
    std::array::from_fn(|i| {
        let a = DAYLIGHT_S[i / 2];
        // Odd bands lie half way between two table rows.
        let b = DAYLIGHT_S[(i + 1) / 2];
        let s = |k: usize| 0.5 * (a[k] as f64 + b[k] as f64);
        // S0 is 100 at 560 nm, where S1 and S2 are 0.
        ((s(0) + m1 * s(1) + m2 * s(2)) / 100.0).max(0.0) as f32
    })
}

/// The CIE's reference illuminant for a colour temperature `kelvin`, what
/// CIE 13.3 renders the test colours under (cri.rs): a blackbody below
/// 5000 K, daylight from there up.
pub fn reference_white(kelvin: f32) -> Spectrum {
    if kelvin < 5000.0 {
        planck(kelvin)
    } else {
        daylight(kelvin)
    }
}

/// Where [`lamp_white`] slides from the blackbody to daylight, kelvin.
const LAMP_WHITE_SLIDE: (f32, f32) = (4500.0, 5500.0);

/// The white a lamp of colour temperature `kelvin` is built to match: the
/// reference illuminant, a blackbody below 4500 K and daylight from 5500 K
/// up, and between the two a mix at equal luminance that slides from one to
/// the other (a smoothstep). So the knob LED, whose Kelvin is a slider, does
/// not jump at 5000 K (D10, L:88), where `reference_white` switches by a
/// duv of about 0.003, and every named lamp (3000, 4000, 5600, 6500 K) keeps
/// its spectrum bit for bit.
pub fn lamp_white(kelvin: f32) -> Spectrum {
    let (lo, hi) = LAMP_WHITE_SLIDE;
    if kelvin <= lo {
        return planck(kelvin);
    }
    // Written negated so a NaN reads as daylight, as reference_white has it.
    if !(kelvin < hi) {
        return daylight(kelvin);
    }
    let w = smoothstep(lo, hi, kelvin);
    let (warm, cool) = (planck(kelvin), daylight(kelvin));
    // mix_to reads only the chromaticity: each at Y = 1, mixed by weight.
    let (y_warm, y_cool) = (xyz(&warm)[1], xyz(&cool)[1]);
    std::array::from_fn(|i| (1.0 - w) * warm[i] / y_warm + w * cool[i] / y_cool)
}

/// Adds a Gaussian emission band to `s`: `weight` at `centre_nm`, falling
/// with standard deviation `sigma_nm`.
fn add_band(s: &mut Spectrum, centre_nm: f32, sigma_nm: f32, weight: f32) {
    for (i, v) in s.iter_mut().enumerate() {
        let d = (wavelength_nm(i) - centre_nm) / sigma_nm;
        *v += weight * (-0.5 * d * d).exp();
    }
}

/// CIE XYZ of a spectrum (relative: the band width is left out).
pub fn xyz(s: &Spectrum) -> [f32; 3] {
    let mut sum = [0.0f64; 3];
    for (v, cmf) in s.iter().zip(CMF_1931.iter()) {
        for k in 0..3 {
            sum[k] += *v as f64 * cmf[k] as f64;
        }
    }
    [sum[0] as f32, sum[1] as f32, sum[2] as f32]
}

/// CIE XYZ to linear Rec.709: sky.rs's matrix (`xyz_to_linear_rgb`), so the
/// two cannot drift (L:94). Out-of-gamut colours (a sodium lamp's yellow)
/// clip at 0 per channel, as they do there.
pub fn xyz_to_rgb(xyz: [f32; 3]) -> Vec3f {
    xyz_to_linear_rgb(xyz)
}

/// The colour of a spectrum: linear Rec.709 at luminance 1. A spectrum with
/// no visible power is white, so a broken source cannot turn a light black
/// or NaN.
pub fn spectrum_rgb(s: &Spectrum) -> Vec3f {
    let rgb = xyz_to_rgb(xyz(s));
    let y = luminance(rgb);
    if y.is_finite() && y > 1.0e-12 {
        rgb / y
    } else {
        Vec3f::all(1.0)
    }
}

/// The CIE 1931 2-degree functions at `nm`, linearly interpolated in CMF_1931 (0 outside 380..=780). Plan hdri-7: a
/// line's colour at its exact wavelength, which a narrow band on the 5 nm grid would alias.
pub fn cmf_at(nm: f32) -> [f32; 3] {
    let x = (nm - LAMBDA_MIN_NM) / LAMBDA_STEP_NM;
    if !(x >= 0.0 && x <= (SAMPLES - 1) as f32) {
        return [0.0; 3];
    }
    let i = (x as usize).min(SAMPLES - 2);
    let t = x - i as f32;
    let (a, b) = (CMF_1931[i], CMF_1931[i + 1]);
    std::array::from_fn(|c| a[c] + (b[c] - a[c]) * t)
}

/// One rayleigh's photons per second per square metre per steradian (10^10 / 4 pi) times Planck's constant times the
/// speed of light (J m): a rayleigh's radiance at a wavelength of L metres is this over L, in W m^-2 sr^-1.
const RAYLEIGH_HC: f32 = 7.957_747e8 * 1.986_445_9e-25;

/// XYZ of one rayleigh of an emission line at `nm` as unadapted engine radiance: its radiance through the
/// colour-matching functions at 683 lm/W, in cd/m², over the engine's cd/m² per unit (`night::cd_m2_per_engine_unit`,
/// the stars' own anchor). Y is the line's luminance (refs §4.3: 557.7 nm about 3.06e-11 per R).
pub fn line_xyz_per_rayleigh(nm: f32) -> [f32; 3] {
    let watts = RAYLEIGH_HC / (nm * 1.0e-9);
    let k = 683.0 * watts / super::night::cd_m2_per_engine_unit();
    cmf_at(nm).map(|c| c * k)
}

/// Linear Rec.709 of `xyz` with its luminance kept: the matrix (`xyz_to_rgb`, which clips each negative channel to 0),
/// then rescaled to Y. A line outside Rec.709 keeps its brightness and loses only hue.
pub fn xyz_to_rgb_keep_luminance(xyz: [f32; 3]) -> Vec3f {
    let rgb = xyz_to_rgb(xyz);
    let y = luminance(rgb);
    if y > 0.0 && xyz[1] > 0.0 {
        rgb * (xyz[1] / y)
    } else {
        Vec3f::default()
    }
}

/// Weights for three emitters so that their mix has the chromaticity of
/// `target` (and Y = 1): a 3x3 solve in XYZ. A weight that comes out
/// negative (the target lies outside the emitters' triangle) is held at 0.
fn mix_to(emitters: [&Spectrum; 3], target: &Spectrum) -> Spectrum {
    let a: [[f64; 3]; 3] = emitters.map(|e| xyz(e).map(|v| v as f64));
    let t = xyz(target).map(|v| v as f64);
    let t = [t[0] / t[1], 1.0, t[2] / t[1]];
    // Columns of the system are the emitters' XYZ; Cramer's rule.
    let det3 = |c0: [f64; 3], c1: [f64; 3], c2: [f64; 3]| {
        c0[0] * (c1[1] * c2[2] - c2[1] * c1[2]) - c1[0] * (c0[1] * c2[2] - c2[1] * c0[2]) + c2[0] * (c0[1] * c1[2] - c1[1] * c0[2])
    };
    let det = det3(a[0], a[1], a[2]);
    let w = if det.abs() > 1.0e-30 {
        [det3(t, a[1], a[2]) / det, det3(a[0], t, a[2]) / det, det3(a[0], a[1], t) / det]
    } else {
        [1.0, 1.0, 1.0]
    };
    let w = w.map(|v| if v.is_finite() { v.max(0.0) as f32 } else { 0.0 });
    std::array::from_fn(|i| w[0] * emitters[0][i] + w[1] * emitters[1][i] + w[2] * emitters[2][i])
}

/// A phosphor-converted white LED at colour temperature `cct` (held to
/// 2000..=10000 K), sitting exactly on the white it is built to match
/// ([`lamp_white`]).
///
/// `quality` (0..=1) is the one knob. It moves the green phosphor from 560
/// to 520 nm and the red one from 600 to 630 nm and widens both, which is
/// what separates a cheap LED from a full-spectrum one: the gap at 480 nm
/// and the missing deep red fill in. CRI Ra runs from about 53 at 0 through
/// 80 near 0.5 to about 95 at 1, whatever the colour temperature.
pub fn led(cct: f32, quality: f32) -> Spectrum {
    let q = if quality.is_finite() { quality.clamp(0.0, 1.0) } else { 0.5 };
    let cct = if cct.is_finite() { cct.clamp(2000.0, 10_000.0) } else { 4000.0 };
    let mut pump = [0.0; SAMPLES];
    add_band(&mut pump, 450.0, 9.0 + 3.0 * q, 1.0);
    let mut green = [0.0; SAMPLES];
    add_band(&mut green, 560.0 - 40.0 * q, 32.0 + 12.0 * q, 1.0);
    let mut red = [0.0; SAMPLES];
    add_band(&mut red, 600.0 + 30.0 * q, 30.0 + 26.0 * q, 1.0);
    mix_to([&pump, &green, &red], &lamp_white(cct))
}

/// A three-band fluorescent tube at colour temperature `cct` (held to
/// 2500..=8000 K): narrow red (611 nm) and green (544 nm) rare-earth
/// phosphors with their side lines, a broad blue phosphor, and the mercury
/// lines at 405, 436 and 578 nm. CRI Ra is about 85.
pub fn fluorescent_triband(cct: f32) -> Spectrum {
    let cct = if cct.is_finite() { cct.clamp(2500.0, 8000.0) } else { 4000.0 };
    let mut blue = [0.0; SAMPLES];
    add_band(&mut blue, 455.0, 24.0, 1.0);
    add_band(&mut blue, 436.0, 3.5, 0.5);
    add_band(&mut blue, 405.0, 3.5, 0.1);
    let mut green = [0.0; SAMPLES];
    add_band(&mut green, 544.0, 6.0, 1.0);
    add_band(&mut green, 489.0, 7.0, 0.4);
    add_band(&mut green, 586.0, 6.0, 0.3);
    add_band(&mut green, 578.0, 3.5, 0.15);
    add_band(&mut green, 622.0, 6.0, 0.1);
    let mut red = [0.0; SAMPLES];
    add_band(&mut red, 611.0, 6.0, 1.0);
    add_band(&mut red, 593.0, 6.0, 0.3);
    add_band(&mut red, 628.0, 7.0, 0.3);
    add_band(&mut red, 650.0, 6.0, 0.1);
    add_band(&mut red, 705.0, 6.0, 0.1);
    mix_to([&blue, &green, &red], &lamp_white(cct))
}

/// A low-pressure sodium lamp: the D line at 589.3 nm and nothing else.
/// Colours cannot be told apart under it (CRI Ra about -45).
pub fn sodium_low_pressure() -> Spectrum {
    let mut s = [0.0; SAMPLES];
    add_band(&mut s, 589.3, 2.5, 1.0);
    s
}

/// A high-pressure sodium lamp: the D line broadened by the pressure and
/// self-absorbed at its centre, with the side lines at 467, 498, 569 and
/// 616 nm and a weak continuum. About 2000 K, CRI Ra in the twenties.
pub fn sodium_high_pressure() -> Spectrum {
    let mut wings = [0.0; SAMPLES];
    add_band(&mut wings, 589.0, 18.0, 1.0);
    add_band(&mut wings, 605.0, 18.0, 0.4);
    let mut s: Spectrum = std::array::from_fn(|i| {
        // The cool sodium near the wall takes the line's own centre back out.
        let d = (wavelength_nm(i) - 589.0) / 4.5;
        wings[i] * (1.0 - 0.95 * (-0.5 * d * d).exp()) + 0.02
    });
    add_band(&mut s, 569.0, 4.0, 0.45);
    add_band(&mut s, 616.0, 5.0, 0.2);
    add_band(&mut s, 498.0, 4.0, 0.10);
    add_band(&mut s, 467.0, 4.0, 0.03);
    s
}

/// One named source of the table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Source {
    /// The name stored in `LightParams::source`.
    pub name: &'static str,
    /// The name shown in the panel.
    pub label: &'static str,
    kind: SourceKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SourceKind {
    Planck(f32),
    Daylight(f32),
    /// (colour temperature, quality)
    Led(f32, f32),
    /// The parametric LED: the light's own Kelvin and quality knob.
    LedKnob,
    Fluorescent(f32),
    SodiumLow,
    SodiumHigh,
}

const fn source_row(name: &'static str, label: &'static str, kind: SourceKind) -> Source {
    Source { name, label, kind }
}

/// The parametric LED's name: its colour temperature is the light's Kelvin
/// and its quality the light's `source_quality`.
pub const LED_KNOB: &str = "led";
/// The knob LED's quality when the light sets none: a CRI-80 LED, as the
/// named `*_cri80` ones.
pub const LED_QUALITY_DEFAULT: f32 = 0.52;

/// Every named source, in panel order.
pub const SOURCES: [Source; 18] = [
    source_row("tungsten", "Tungsten bulb, 2700 K", SourceKind::Planck(2700.0)),
    source_row("halogen", "Halogen, 3200 K", SourceKind::Planck(3200.0)),
    source_row("daylight_d55", "Daylight D55", SourceKind::Daylight(5503.0)),
    source_row("daylight_d65", "Daylight D65", SourceKind::Daylight(6504.0)),
    source_row("daylight_d75", "North sky D75", SourceKind::Daylight(7504.0)),
    source_row("led_warm_cri80", "LED warm 3000 K, CRI 80", SourceKind::Led(3000.0, 0.52)),
    source_row("led_warm_cri95", "LED warm 3000 K, CRI 95", SourceKind::Led(3000.0, 1.0)),
    source_row("led_neutral_cri80", "LED neutral 4000 K, CRI 80", SourceKind::Led(4000.0, 0.52)),
    source_row("led_neutral_cri95", "LED neutral 4000 K, CRI 95", SourceKind::Led(4000.0, 1.0)),
    source_row("led_daylight_cri70", "LED daylight 5600 K, CRI 70", SourceKind::Led(5600.0, 0.32)),
    source_row("led_daylight_cri80", "LED daylight 5600 K, CRI 80", SourceKind::Led(5600.0, 0.52)),
    source_row("led_daylight_cri95", "LED daylight 5600 K, CRI 95", SourceKind::Led(5600.0, 1.0)),
    source_row(LED_KNOB, "LED at the Kelvin, quality knob", SourceKind::LedKnob),
    source_row("fluorescent_warm", "Fluorescent warm 3000 K", SourceKind::Fluorescent(3000.0)),
    source_row("fluorescent_cool", "Fluorescent cool 4000 K", SourceKind::Fluorescent(4000.0)),
    source_row("fluorescent_daylight", "Fluorescent daylight 6500 K", SourceKind::Fluorescent(6500.0)),
    source_row("sodium_low_pressure", "Sodium, low pressure", SourceKind::SodiumLow),
    source_row("sodium_high_pressure", "Sodium, high pressure", SourceKind::SodiumHigh),
];

/// The source names, in table order: what `LightParams::source` may hold.
pub const SOURCE_NAMES: [&str; 18] = {
    let mut names = [""; 18];
    let mut i = 0;
    while i < SOURCES.len() {
        names[i] = SOURCES[i].name;
        i += 1;
    }
    names
};

/// The table row for a name; spelling is forgiven (case, outer spaces).
pub fn source(name: &str) -> Option<&'static Source> {
    let name = name.trim();
    SOURCES.iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

impl Source {
    /// The source on its own. The knob LED, which needs a light, is 4000 K
    /// at the default quality here.
    pub fn spectrum(&self) -> Spectrum {
        self.spectrum_at(4000.0, LED_QUALITY_DEFAULT)
    }

    /// The source on a light of `kelvin` with the quality knob at `quality`.
    /// Only the knob LED reads the two; every other source is fixed.
    pub fn spectrum_at(&self, kelvin: f32, quality: f32) -> Spectrum {
        match self.kind {
            SourceKind::Planck(kelvin) => planck(kelvin),
            SourceKind::Daylight(kelvin) => daylight(kelvin),
            SourceKind::Led(cct, quality) => led(cct, quality),
            // led() holds both to its ranges and replaces a NaN.
            SourceKind::LedKnob => led(kelvin, quality),
            SourceKind::Fluorescent(cct) => fluorescent_triband(cct),
            SourceKind::SodiumLow => sodium_low_pressure(),
            SourceKind::SodiumHigh => sodium_high_pressure(),
        }
    }
}

/// The spectrum of a named source; None for an unknown name.
pub fn source_spectrum(name: &str) -> Option<Spectrum> {
    source(name).map(|s| s.spectrum())
}

/// The colour of a named source: linear Rec.709 at luminance 1, as
/// `kelvin_to_rgb` gives for a Kelvin. None for an unknown name.
pub fn source_rgb(name: &str) -> Option<Vec3f> {
    source_spectrum(name).map(|s| spectrum_rgb(&s))
}

/// The spectrum of a light's named source, at the light's Kelvin and
/// quality knob (the knob LED reads them; the other sources ignore them).
/// None when the light names no source or an unknown one. An explicit `rgb`
/// is the caller's business: this only looks at the source. The light is
/// read as it is given: its readers, the colour (`studio::light_white`) and
/// the CRI readout (`cri::light_spectrum`), hand it the light as clamp()
/// holds it (D17), and the knob LED holds the numbers it is built at to its
/// own domain (`led`).
pub fn light_source_spectrum(light: &LightParams) -> Option<Spectrum> {
    let row = source(light.source.as_deref()?)?;
    Some(row.spectrum_at(light.kelvin, light.source_quality.unwrap_or(LED_QUALITY_DEFAULT)))
}

/// The colour of a light's named source, as `light_source_spectrum` gives it:
/// linear Rec.709 at luminance 1.
pub fn light_source_rgb(light: &LightParams) -> Option<Vec3f> {
    light_source_spectrum(light).map(|s| spectrum_rgb(&s))
}

/// The named lamp a light's colour comes from when the lamp has a colour
/// temperature of its own: every source but the knob LED, which is built at
/// the light's Kelvin. None for a Kelvin light, the knob LED and an unknown
/// name. A stroke's Kelvin ramp on such a lamp has no temperature to slide
/// from (D10, L:99). An explicit `rgb` is the caller's business.
pub fn fixed_source(light: &LightParams) -> Option<&'static Source> {
    source(light.source.as_deref()?).filter(|row| !matches!(row.kind, SourceKind::LedKnob))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdri::studio::{kelvin_to_rgb, light_color};

    /// CIE 1931 chromaticity of a spectrum.
    fn xy(s: &Spectrum) -> (f32, f32) {
        let [x, y, z] = xyz(s);
        (x / (x + y + z), y / (x + y + z))
    }

    #[test]
    fn the_observer_table_integrates_to_the_equal_energy_white() {
        // The three functions have equal areas by construction (21.3714 at 5 nm).
        let sums = xyz(&[1.0; SAMPLES]);
        for v in sums {
            assert!((v - 21.3714).abs() < 2.0e-3, "{sums:?}");
        }
        assert_eq!(CMF_1931[35], [0.512050, 1.0, 0.005750], "y bar peaks at 555 nm");
        assert_eq!(wavelength_nm(0), 380.0);
        assert_eq!(wavelength_nm(35), 555.0);
        assert_eq!(wavelength_nm(SAMPLES - 1), 780.0);
    }

    #[test]
    fn d65_is_white() {
        let d65 = daylight(6504.0);
        let (x, y) = xy(&d65);
        assert!((x - 0.3127).abs() < 3.0e-4 && (y - 0.3290).abs() < 3.0e-4, "D65 is at ({x}, {y})");
        // The CIE's D65 table: 100 at 560 nm, 117.0 at 450 nm, 90.0 at 600 nm.
        assert!((d65[36] - 1.0).abs() < 1.0e-6);
        assert!((d65[14] - 1.170).abs() < 2.0e-3, "{}", d65[14]);
        assert!((d65[44] - 0.900).abs() < 2.0e-3, "{}", d65[44]);
        // Rec.709's white point is D65: the source is white to a tenth of a percent.
        let rgb = spectrum_rgb(&d65);
        assert!((rgb - Vec3f::all(1.0)).abs().max_elem() < 2.0e-3, "{rgb:?}");
        assert!((luminance(rgb) - 1.0).abs() < 1.0e-5);
        // Out of range and NaN are held, not extrapolated.
        assert_eq!(daylight(1000.0), daylight(4000.0));
        assert_eq!(daylight(f32::NAN), daylight(6504.0));
    }

    #[test]
    fn a_blackbody_matches_the_kelvin_colours() {
        // Illuminant A is a 2856 K blackbody at x, y = 0.4476, 0.4074.
        let (x, y) = xy(&planck(2856.0));
        assert!((x - 0.4476).abs() < 3.0e-4 && (y - 0.4074).abs() < 3.0e-4, "A is at ({x}, {y})");
        // The integrated spectrum and studio.rs's fitted locus agree.
        for kelvin in [2000.0f32, 2700.0, 3200.0, 5600.0, 6500.0, 10000.0] {
            let (spectral, fitted) = (spectrum_rgb(&planck(kelvin)), kelvin_to_rgb(kelvin));
            assert!((spectral - fitted).abs().max_elem() < 5.0e-3, "{kelvin} K: {spectral:?} vs {fitted:?}");
        }
        let hot = planck(6500.0);
        assert!((hot[36] - 1.0).abs() < 1.0e-6, "scaled to 1 at 560 nm");
        assert!(planck(1000.0).iter().all(|v| v.is_finite() && *v >= 0.0));
        assert_eq!(planck(f32::NAN), planck(6500.0));
    }

    #[test]
    fn leds_and_tubes_sit_on_the_white_locus() {
        for (cct, quality) in [(2700.0f32, 0.0f32), (3000.0, 0.52), (4000.0, 1.0), (5000.0, 0.52), (5600.0, 0.32), (6500.0, 1.0)] {
            let got = spectrum_rgb(&led(cct, quality));
            let want = spectrum_rgb(&lamp_white(cct));
            assert!((got - want).abs().max_elem() < 2.0e-3, "LED {cct} K q {quality}: {got:?} vs {want:?}");
            assert!(led(cct, quality).iter().all(|v| v.is_finite() && *v >= 0.0));
        }
        for cct in [3000.0f32, 4000.0, 6500.0] {
            let got = spectrum_rgb(&fluorescent_triband(cct));
            let want = spectrum_rgb(&lamp_white(cct));
            assert!((got - want).abs().max_elem() < 2.0e-3, "tube {cct} K: {got:?} vs {want:?}");
        }
        // The quality knob changes the spectrum, not the colour: more red past 630 nm.
        let deep_red = |s: &Spectrum| s[50..].iter().sum::<f32>() / s.iter().sum::<f32>();
        assert!(deep_red(&led(3000.0, 1.0)) > 1.5 * deep_red(&led(3000.0, 0.0)));
        // A tube is spiky: its green line stands far above the band beside it.
        let tube = fluorescent_triband(4000.0);
        assert!(tube[33] > 3.0 * tube[30], "545 nm vs 530 nm: {} vs {}", tube[33], tube[30]);
    }

    /// D10 (L:88): the knob LED's colour does not jump at 5000 K, where the
    /// CIE's reference white switches from a blackbody to daylight. Dragged a
    /// kelvin at a time from 4000 to 6000 K it moves by at most 1e-3 per
    /// channel per step (the blackbody colours alone move about 2e-4).
    #[test]
    fn the_knob_led_has_no_step_at_5000_k() {
        let colour = |kelvin: f32| {
            let light = LightParams { source: Some(LED_KNOB.to_string()), kelvin, ..LightParams::default() };
            light_source_rgb(&light).expect("the knob LED has a colour")
        };
        let (mut worst, mut at) = (0.0f32, 0.0f32);
        let mut last = colour(4000.0);
        for k in 4001..=6000 {
            let c = colour(k as f32);
            let step = (c - last).abs().max_elem();
            if step > worst {
                (worst, at) = (step, k as f32);
            }
            last = c;
        }
        assert!(worst < 1.0e-3, "the LED's colour steps by {worst} at {at} K");
        // Outside the slide a lamp's white is the CIE's reference, bit for
        // bit, so every named LED and tube keeps its spectrum; inside it the
        // white lies between the blackbody and daylight.
        for kelvin in [1000.0f32, 2000.0, 3000.0, 4000.0, 4500.0, 5500.0, 5600.0, 6500.0, 10000.0] {
            assert_eq!(lamp_white(kelvin), reference_white(kelvin), "{kelvin} K");
        }
        assert_eq!(lamp_white(f32::NAN), reference_white(f32::NAN));
        let (warm, mid, cool) = (spectrum_rgb(&planck(5000.0)), spectrum_rgb(&lamp_white(5000.0)), spectrum_rgb(&daylight(5000.0)));
        for (w, m, c) in [(warm.x, mid.x, cool.x), (warm.y, mid.y, cool.y), (warm.z, mid.z, cool.z)] {
            assert!(m > w.min(c) && m < w.max(c), "5000 K: {mid:?} between {warm:?} and {cool:?}");
        }
    }

    #[test]
    fn sodium_is_orange_and_out_of_gamut() {
        let low = sodium_low_pressure();
        let (x, y) = xy(&low);
        // The 589 nm point of the spectral locus.
        assert!((x - 0.570).abs() < 0.01 && (y - 0.429).abs() < 0.01, "({x}, {y})");
        let rgb = spectrum_rgb(&low);
        assert!(rgb.x > 2.0 && rgb.y > 0.4 && rgb.z == 0.0, "{rgb:?}");
        assert!((luminance(rgb) - 1.0).abs() < 1.0e-5);
        let high = spectrum_rgb(&sodium_high_pressure());
        assert!(high.x > high.y && high.y > high.z, "{high:?}");
        assert!(high.x < rgb.x, "the high-pressure lamp is less saturated: {high:?}");
        // A dark spectrum is white, never NaN.
        assert_eq!(spectrum_rgb(&[0.0; SAMPLES]), Vec3f::all(1.0));
    }

    #[test]
    fn the_source_table_is_complete() {
        assert_eq!(SOURCE_NAMES.len(), SOURCES.len());
        let mut seen = std::collections::HashSet::new();
        for (name, row) in SOURCE_NAMES.iter().zip(SOURCES.iter()) {
            assert_eq!(*name, row.name);
            assert!(seen.insert(*name), "{name} is listed twice");
            assert!(name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'), "{name}");
            assert!(!row.label.is_empty());
            let rgb = source_rgb(name).expect("every name has a colour");
            assert!(rgb.is_finite() && rgb.min_elem() >= 0.0, "{name}: {rgb:?}");
            assert!((luminance(rgb) - 1.0).abs() < 1.0e-4, "{name}: luminance {}", luminance(rgb));
        }
        for family in ["tungsten", "daylight_", "led_", "fluorescent_", "sodium_"] {
            assert!(SOURCE_NAMES.iter().any(|n| n.starts_with(family)), "no {family} source");
        }
        assert_eq!(source(" LED_Warm_CRI80 ").map(|s| s.name), Some("led_warm_cri80"));
        assert!(source("candle").is_none() && source_rgb("candle").is_none() && source_spectrum("").is_none());
        // The parametric LED is in the table; alone (no light) it is 4000 K at the default knob.
        assert_eq!(source(" LED ").map(|s| s.name), Some(LED_KNOB));
        assert_eq!(source_spectrum(LED_KNOB), Some(led(4000.0, LED_QUALITY_DEFAULT)));
    }

    #[test]
    fn a_source_replaces_the_kelvin_of_a_light() {
        let sodium = LightParams { source: Some("sodium_low_pressure".to_string()), kelvin: 9000.0, intensity_ev: 1.0, ..LightParams::default() };
        let c = light_color(&sodium);
        let want = source_rgb("sodium_low_pressure").unwrap() * 2.0;
        assert!((c - want).length() < 1.0e-5, "{c:?} vs {want:?}");
        // The Kelvin is ignored while a source is set...
        assert_eq!(c, light_color(&LightParams { kelvin: 2000.0, ..sodium.clone() }));
        // ...the tint still applies, at the same luminance...
        let tinted = light_color(&LightParams { tint: 0.5, ..sodium.clone() });
        assert!(tinted.y / tinted.x < c.y / c.x);
        assert!((luminance(tinted) - 2.0).abs() < 1.0e-4);
        // ...an explicit colour still wins...
        let rgb = light_color(&LightParams { rgb: Some([0.1, 0.2, 0.3]), ..sodium.clone() });
        assert!((rgb - vec3f(0.2, 0.4, 0.6)).length() < 1.0e-6);
        // ...and an unknown name falls back to the Kelvin.
        let unknown = LightParams { source: Some("candle".to_string()), ..LightParams::default() };
        assert_eq!(light_color(&unknown), light_color(&LightParams::default()));
        // A daylight source is white where 6500 K is slightly pink.
        let d65 = light_color(&LightParams { source: Some("daylight_d65".to_string()), intensity_ev: 0.0, ..LightParams::default() });
        assert!((d65 - Vec3f::all(1.0)).abs().max_elem() < 2.0e-3, "{d65:?}");
        // The parametric LED is the one source that reads the Kelvin, and its
        // quality knob (the spec's "LED with a quality knob").
        let knob = LightParams {
            source: Some(LED_KNOB.to_string()),
            kelvin: 3000.0,
            source_quality: Some(1.0),
            intensity_ev: 0.0,
            ..LightParams::default()
        };
        assert_eq!(light_source_spectrum(&knob), Some(led(3000.0, 1.0)));
        assert_eq!(
            light_source_spectrum(&LightParams { source_quality: None, ..knob.clone() }),
            Some(led(3000.0, LED_QUALITY_DEFAULT)),
            "no knob is the default quality"
        );
        let warm = light_color(&knob);
        assert!((warm - spectrum_rgb(&led(3000.0, 1.0))).length() < 1.0e-4, "{warm:?}");
        assert!(warm.x > warm.z, "3000 K is warm: {warm:?}");
        assert_eq!(light_source_rgb(&knob), Some(spectrum_rgb(&led(3000.0, 1.0))));
        // A named source ignores the knob; no source is no source spectrum.
        assert_eq!(light_source_spectrum(&LightParams { source_quality: Some(0.0), ..sodium.clone() }), source_spectrum("sodium_low_pressure"));
        assert_eq!(light_source_spectrum(&LightParams::default()), None);
    }

    #[test]
    fn gels_filter_a_source() {
        let bare = LightParams { source: Some("led_neutral_cri80".to_string()), intensity_ev: 0.0, ..LightParams::default() };
        let gelled = LightParams { gels: Some(vec!["cto_half".to_string(), "nd_0.3".to_string()]), ..bare.clone() };
        let (a, b) = (light_color(&bare), light_color(&gelled));
        assert!(b.x / b.z > a.x / a.z, "a CTO warms the source");
        assert!(b.x <= a.x && b.y <= a.y && b.z <= a.z, "and only takes light away: {b:?} vs {a:?}");
        assert!(luminance(b) < 0.5 * luminance(a), "the ND alone halves it");
    }

    /// Plan hdri-7: the colour-matching functions between the table's nodes, and an emission line's light per rayleigh
    /// (refs §4.3: cd/m² per R from 683 lm/W, over the engine's about 6300 cd/m² per unit).
    #[test]
    fn a_line_per_rayleigh_is_its_photometric_light() {
        for i in 0..SAMPLES {
            assert_eq!(cmf_at(wavelength_nm(i)), CMF_1931[i]);
        }
        let mid = cmf_at(557.5);
        for c in 0..3 {
            assert!((mid[c] - 0.5 * (CMF_1931[35][c] + CMF_1931[36][c])).abs() < 1.0e-6);
        }
        assert_eq!((cmf_at(379.0), cmf_at(781.0)), ([0.0; 3], [0.0; 3]));
        for (nm, want) in [(557.7f32, 3.063e-11f32), (630.0, 7.205e-12), (427.8, 3.887e-13), (656.3, 1.990e-12)] {
            let y = line_xyz_per_rayleigh(nm)[1];
            assert!((y / want - 1.0).abs() < 0.02, "{nm} nm: {y:e} per rayleigh, want {want:e}");
        }
    }

    /// A line's colour keeps its luminance however Rec.709 clips it (a plain clip loses 14x at 427.8 nm, engine §12.2).
    #[test]
    fn a_clipped_colour_keeps_its_luminance() {
        for nm in [427.8f32, 486.1, 500.7, 557.7, 630.0, 656.3] {
            let xyz = line_xyz_per_rayleigh(nm).map(|c| c * 1.0e12);
            let rgb = xyz_to_rgb_keep_luminance(xyz);
            assert!(rgb.x >= 0.0 && rgb.y >= 0.0 && rgb.z >= 0.0, "{nm}: {rgb:?}");
            assert!((luminance(rgb) / xyz[1] - 1.0).abs() < 1.0e-4, "{nm}: {} vs {}", luminance(rgb), xyz[1]);
        }
        let violet = xyz_to_rgb_keep_luminance(line_xyz_per_rayleigh(427.8));
        assert!(violet.z > violet.x && violet.y == 0.0, "{violet:?}");
        let green = xyz_to_rgb_keep_luminance(line_xyz_per_rayleigh(557.7));
        assert!(green.y > 4.0 * green.x && green.z == 0.0, "557.7 nm is lime in Rec.709: {green:?}");
        assert_eq!(xyz_to_rgb_keep_luminance([0.0; 3]), Vec3f::default());
    }
}
