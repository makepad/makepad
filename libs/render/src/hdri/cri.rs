//! The CIE general colour rendering index, Ra (CIE 13.3-1995), of a light
//! source's spectrum: a read-only quality figure for the panel.
//!
//! The method, as the standard gives it:
//! 1. The source's correlated colour temperature is the blackbody nearest to
//!    it in the CIE 1960 (u, v) diagram.
//! 2. The reference illuminant is a blackbody of that temperature below
//!    5000 K, CIE daylight from there up.
//! 3. Eight test colour samples (TCS01-08, medium-saturated Munsell colours)
//!    are lit by both. Their colours under the source are adapted to the
//!    reference white with the standard's von Kries transform and placed in
//!    the CIE 1964 U*V*W* space.
//! 4. Each sample's index is 100 - 4.6 times its colour difference; Ra is
//!    the mean of the eight.
//!
//! Ra says how alike the eight samples look under the source and under its
//! reference, nothing more: 100 is "as daylight or a filament would", a
//! three-band tube scores about 85, low-pressure sodium is negative. It does
//! not change how the map renders (the map is RGB); it tells the person what
//! lamp they picked.
//!
//! Data: the TCS reflectances are the CIE 13.3 table, 380..=780 nm at 5 nm.

use super::spectrum::{light_source_spectrum, planck, reference_white, xyz, Spectrum, SAMPLES};
use super::LightParams;

/// Spectral reflectance of the eight CIE 13.3 test colour samples (TCS01 to
/// TCS08), 380..=780 nm in 5 nm steps.
pub const TCS: [[f32; SAMPLES]; 8] = [
    // TCS01, 7.5 R 6/4: light greyish red
    [
        0.219, 0.239, 0.252, 0.256, 0.256, 0.254, 0.252, 0.248, 0.244, 0.240, 0.237, 0.232, 0.230, 0.226,
        0.225, 0.222, 0.220, 0.218, 0.216, 0.214, 0.214, 0.214, 0.216, 0.218, 0.223, 0.225, 0.226, 0.226,
        0.225, 0.225, 0.227, 0.230, 0.236, 0.245, 0.253, 0.262, 0.272, 0.283, 0.298, 0.318, 0.341, 0.367,
        0.390, 0.409, 0.424, 0.435, 0.442, 0.448, 0.450, 0.451, 0.451, 0.451, 0.451, 0.451, 0.450, 0.450,
        0.451, 0.451, 0.453, 0.454, 0.455, 0.457, 0.458, 0.460, 0.462, 0.463, 0.464, 0.465, 0.466, 0.466,
        0.466, 0.466, 0.467, 0.467, 0.467, 0.467, 0.467, 0.467, 0.467, 0.467, 0.467,
    ],
    // TCS02, 5 Y 6/4: dark greyish yellow
    [
        0.070, 0.079, 0.089, 0.101, 0.111, 0.116, 0.118, 0.120, 0.121, 0.122, 0.122, 0.122, 0.123, 0.124,
        0.127, 0.128, 0.131, 0.134, 0.138, 0.143, 0.150, 0.159, 0.174, 0.190, 0.207, 0.225, 0.242, 0.253,
        0.260, 0.264, 0.267, 0.269, 0.272, 0.276, 0.282, 0.289, 0.299, 0.309, 0.322, 0.329, 0.335, 0.339,
        0.341, 0.341, 0.342, 0.342, 0.342, 0.341, 0.341, 0.339, 0.339, 0.338, 0.338, 0.337, 0.336, 0.335,
        0.334, 0.332, 0.332, 0.331, 0.331, 0.330, 0.329, 0.328, 0.328, 0.327, 0.326, 0.325, 0.324, 0.324,
        0.324, 0.323, 0.322, 0.321, 0.320, 0.318, 0.316, 0.315, 0.315, 0.314, 0.314,
    ],
    // TCS03, 5 GY 6/8: strong yellow green
    [
        0.065, 0.068, 0.070, 0.072, 0.073, 0.073, 0.074, 0.074, 0.074, 0.073, 0.073, 0.073, 0.073, 0.073,
        0.074, 0.075, 0.077, 0.080, 0.085, 0.094, 0.109, 0.126, 0.148, 0.172, 0.198, 0.221, 0.241, 0.260,
        0.278, 0.302, 0.339, 0.370, 0.392, 0.399, 0.400, 0.393, 0.380, 0.365, 0.349, 0.332, 0.315, 0.299,
        0.285, 0.272, 0.264, 0.257, 0.252, 0.247, 0.241, 0.235, 0.229, 0.224, 0.220, 0.217, 0.216, 0.216,
        0.219, 0.224, 0.230, 0.238, 0.251, 0.269, 0.288, 0.312, 0.340, 0.366, 0.390, 0.412, 0.431, 0.447,
        0.460, 0.472, 0.481, 0.488, 0.493, 0.497, 0.500, 0.502, 0.505, 0.510, 0.516,
    ],
    // TCS04, 2.5 G 6/6: moderate yellowish green
    [
        0.074, 0.083, 0.093, 0.105, 0.116, 0.121, 0.124, 0.126, 0.128, 0.131, 0.135, 0.139, 0.144, 0.151,
        0.161, 0.172, 0.186, 0.205, 0.229, 0.254, 0.281, 0.308, 0.332, 0.352, 0.370, 0.383, 0.390, 0.394,
        0.395, 0.392, 0.385, 0.377, 0.367, 0.354, 0.341, 0.327, 0.312, 0.296, 0.280, 0.263, 0.247, 0.229,
        0.214, 0.198, 0.185, 0.175, 0.169, 0.164, 0.160, 0.156, 0.154, 0.152, 0.151, 0.149, 0.148, 0.148,
        0.148, 0.149, 0.151, 0.154, 0.158, 0.162, 0.165, 0.168, 0.170, 0.171, 0.170, 0.168, 0.166, 0.164,
        0.164, 0.165, 0.168, 0.172, 0.177, 0.181, 0.185, 0.189, 0.192, 0.194, 0.197,
    ],
    // TCS05, 10 BG 6/4: light bluish green
    [
        0.295, 0.306, 0.310, 0.312, 0.313, 0.315, 0.319, 0.322, 0.326, 0.330, 0.334, 0.339, 0.346, 0.352,
        0.360, 0.369, 0.381, 0.394, 0.403, 0.410, 0.415, 0.418, 0.419, 0.417, 0.413, 0.409, 0.403, 0.396,
        0.389, 0.381, 0.372, 0.363, 0.353, 0.342, 0.331, 0.320, 0.308, 0.296, 0.284, 0.271, 0.260, 0.247,
        0.232, 0.220, 0.210, 0.200, 0.194, 0.189, 0.185, 0.183, 0.180, 0.177, 0.176, 0.175, 0.175, 0.175,
        0.175, 0.177, 0.180, 0.183, 0.186, 0.189, 0.192, 0.195, 0.199, 0.200, 0.199, 0.198, 0.196, 0.195,
        0.195, 0.196, 0.197, 0.200, 0.203, 0.205, 0.208, 0.212, 0.215, 0.217, 0.219,
    ],
    // TCS06, 5 PB 6/8: light blue
    [
        0.151, 0.203, 0.265, 0.339, 0.410, 0.464, 0.492, 0.508, 0.517, 0.524, 0.531, 0.538, 0.544, 0.551,
        0.556, 0.556, 0.554, 0.549, 0.541, 0.531, 0.519, 0.504, 0.488, 0.469, 0.450, 0.431, 0.414, 0.395,
        0.377, 0.358, 0.341, 0.325, 0.309, 0.293, 0.279, 0.265, 0.253, 0.241, 0.234, 0.227, 0.225, 0.222,
        0.221, 0.220, 0.220, 0.220, 0.220, 0.220, 0.223, 0.227, 0.233, 0.239, 0.244, 0.251, 0.258, 0.263,
        0.268, 0.273, 0.278, 0.281, 0.283, 0.286, 0.291, 0.296, 0.302, 0.313, 0.325, 0.338, 0.351, 0.364,
        0.376, 0.389, 0.401, 0.413, 0.425, 0.436, 0.447, 0.458, 0.469, 0.477, 0.485,
    ],
    // TCS07, 2.5 P 6/8: light violet
    [
        0.378, 0.459, 0.524, 0.546, 0.551, 0.555, 0.559, 0.560, 0.561, 0.558, 0.556, 0.551, 0.544, 0.535,
        0.522, 0.506, 0.488, 0.469, 0.448, 0.429, 0.408, 0.385, 0.363, 0.341, 0.324, 0.311, 0.301, 0.291,
        0.283, 0.273, 0.265, 0.260, 0.257, 0.257, 0.259, 0.260, 0.260, 0.258, 0.256, 0.254, 0.254, 0.259,
        0.270, 0.284, 0.302, 0.324, 0.344, 0.362, 0.377, 0.389, 0.400, 0.410, 0.420, 0.429, 0.438, 0.445,
        0.452, 0.457, 0.462, 0.466, 0.468, 0.470, 0.473, 0.477, 0.483, 0.489, 0.496, 0.503, 0.511, 0.518,
        0.525, 0.532, 0.539, 0.546, 0.553, 0.559, 0.565, 0.570, 0.575, 0.578, 0.581,
    ],
    // TCS08, 10 P 6/8: light reddish purple
    [
        0.104, 0.129, 0.170, 0.240, 0.319, 0.416, 0.462, 0.482, 0.490, 0.488, 0.482, 0.473, 0.462, 0.450,
        0.439, 0.426, 0.413, 0.397, 0.382, 0.366, 0.352, 0.337, 0.325, 0.310, 0.299, 0.289, 0.283, 0.276,
        0.270, 0.262, 0.256, 0.251, 0.250, 0.251, 0.254, 0.258, 0.264, 0.269, 0.272, 0.274, 0.278, 0.284,
        0.295, 0.316, 0.348, 0.384, 0.434, 0.482, 0.528, 0.568, 0.604, 0.629, 0.648, 0.663, 0.676, 0.685,
        0.693, 0.700, 0.705, 0.709, 0.712, 0.715, 0.717, 0.719, 0.721, 0.720, 0.719, 0.722, 0.725, 0.727,
        0.729, 0.730, 0.730, 0.730, 0.730, 0.730, 0.730, 0.730, 0.730, 0.730, 0.730,
    ],
];

/// A source farther than this from the blackbody locus in (u, v) is not
/// "white" in the standard's sense, and its Ra is only indicative (CIE 13.3
/// puts the limit at 5.4e-3).
pub const WHITE_DUV_LIMIT: f32 = 5.4e-3;

/// The colour rendering of one source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cri {
    /// The general index: the mean of `samples`. 100 is the best; strongly
    /// coloured sources go below 0.
    pub ra: f32,
    /// The special indices R1 to R8.
    pub samples: [f32; 8],
    /// The correlated colour temperature, kelvin.
    pub cct: f32,
    /// How far the source is from the blackbody locus in CIE 1960 (u, v).
    pub duv: f32,
}

/// CIE 1960 (u, v) of an XYZ colour; None for black.
fn uv_1960(xyz: [f64; 3]) -> Option<[f64; 2]> {
    let d = xyz[0] + 15.0 * xyz[1] + 3.0 * xyz[2];
    if !(d > 1.0e-30) {
        return None;
    }
    Some([4.0 * xyz[0] / d, 6.0 * xyz[1] / d])
}

fn xyz64(s: &Spectrum) -> [f64; 3] {
    xyz(s).map(|v| v as f64)
}

/// XYZ of a sample of reflectance `r` under `s`.
fn reflected_xyz(s: &Spectrum, r: &[f32; SAMPLES]) -> [f64; 3] {
    let lit: Spectrum = std::array::from_fn(|i| s[i] * r[i]);
    xyz64(&lit)
}

/// The correlated colour temperature of a spectrum and its distance from the
/// blackbody locus in (u, v): the blackbody nearest to it, searched from
/// 1000 K to 25000 K. None for a spectrum with no visible power.
pub fn cct(s: &Spectrum) -> Option<(f32, f32)> {
    let uv = uv_1960(xyz64(s))?;
    // The search runs in mireds (10^6 / K): the locus is close to evenly
    // spaced in them, so the distance has one clean minimum.
    let distance = |mired: f64| match uv_1960(xyz64(&planck((1.0e6 / mired) as f32))) {
        Some(p) => ((p[0] - uv[0]).powi(2) + (p[1] - uv[1]).powi(2)).sqrt(),
        None => f64::MAX,
    };
    // A coarse scan every 10 mired, then a golden-section search in the
    // bracket round the best one.
    let (mut best, mut best_d) = (40.0f64, f64::MAX);
    let mut mired = 40.0;
    while mired <= 1000.0 {
        let d = distance(mired);
        if d < best_d {
            best = mired;
            best_d = d;
        }
        mired += 10.0;
    }
    let (mut a, mut b) = ((best - 10.0).max(40.0), (best + 10.0).min(1000.0));
    let g = 0.618_033_988_75;
    let (mut c, mut d) = (b - g * (b - a), a + g * (b - a));
    let (mut fc, mut fd) = (distance(c), distance(d));
    for _ in 0..40 {
        if fc < fd {
            b = d;
            d = c;
            fd = fc;
            c = b - g * (b - a);
            fc = distance(c);
        } else {
            a = c;
            c = d;
            fc = fd;
            d = a + g * (b - a);
            fd = distance(d);
        }
    }
    let mired = 0.5 * (a + b);
    Some(((1.0e6 / mired) as f32, distance(mired) as f32))
}

/// CIE 13.3 general colour rendering index of a spectrum. None for a
/// spectrum with no visible power.
pub fn cri(s: &Spectrum) -> Option<Cri> {
    let (cct, duv) = cct(s)?;
    let reference = reference_white(cct);
    let (xyz_t, xyz_r) = (xyz64(s), xyz64(&reference));
    let (uv_t, uv_r) = (uv_1960(xyz_t)?, uv_1960(xyz_r)?);
    // Both whites are scaled to Y = 100, as the standard asks.
    let (scale_t, scale_r) = (100.0 / xyz_t[1], 100.0 / xyz_r[1]);
    // The standard's von Kries coefficients of a chromaticity.
    let cd = |uv: [f64; 2]| ((4.0 - uv[0] - 10.0 * uv[1]) / uv[1], (1.708 * uv[1] + 0.404 - 1.481 * uv[0]) / uv[1]);
    let (c_t, d_t) = cd(uv_t);
    let (c_r, d_r) = cd(uv_r);
    let mut samples = [0.0f32; 8];
    for (index, reflectance) in TCS.iter().enumerate() {
        let lit_t = reflected_xyz(s, reflectance).map(|v| v * scale_t);
        let lit_r = reflected_xyz(&reference, reflectance).map(|v| v * scale_r);
        let (uv_ti, uv_ri) = (uv_1960(lit_t)?, uv_1960(lit_r)?);
        // The sample under the source, adapted to the reference white.
        let (c_ti, d_ti) = cd(uv_ti);
        let (c, d) = (c_r / c_t * c_ti, d_r / d_t * d_ti);
        let denominator = 16.518 + 1.481 * c - d;
        let adapted = [(10.872 + 0.404 * c - 4.0 * d) / denominator, 5.520 / denominator];
        // CIE 1964 U*V*W*, both about the reference white.
        let uvw = |y: f64, uv: [f64; 2]| {
            let w = 25.0 * y.max(0.0).cbrt() - 17.0;
            [13.0 * w * (uv[0] - uv_r[0]), 13.0 * w * (uv[1] - uv_r[1]), w]
        };
        let (a, b) = (uvw(lit_t[1], adapted), uvw(lit_r[1], uv_ri));
        let delta = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
        samples[index] = (100.0 - 4.6 * delta) as f32;
    }
    let ra = samples.iter().sum::<f32>() / 8.0;
    if !ra.is_finite() {
        return None;
    }
    Some(Cri { ra, samples, cct, duv })
}

/// The spectrum a light's colour comes from: its named source (the
/// parametric LED at the light's Kelvin and quality knob), else a blackbody
/// at its Kelvin, of the light as clamp() holds it (`LightParams::clamped`),
/// as its colour reads it (`studio::light_white`), so the two read one
/// Kelvin and one knob (L:104). None for a light with an explicit `rgb`: an
/// RGB colour has no spectrum. Tint and gels are RGB corrections on top and
/// are not part of the reading.
pub fn light_spectrum(light: &LightParams) -> Option<Spectrum> {
    if light.rgb.is_some() {
        return None;
    }
    let light = &light.clamped();
    match light_source_spectrum(light) {
        Some(spectrum) => Some(spectrum),
        None => Some(planck(light.kelvin)),
    }
}

/// The colour rendering of a light's source; None for an RGB light.
pub fn light_cri(light: &LightParams) -> Option<Cri> {
    cri(&light_spectrum(light)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdri::spectrum::{
        daylight, fluorescent_triband, led, sodium_high_pressure, sodium_low_pressure, source_spectrum, SOURCE_NAMES,
    };

    fn ra(s: &Spectrum) -> f32 {
        cri(s).expect("a visible spectrum").ra
    }

    #[test]
    fn the_sample_table_is_the_cie_one() {
        // Spot values of the CIE 13.3 table: (sample, band, reflectance).
        for (sample, band, value) in [
            (0, 0, 0.219), (0, 44, 0.424), (1, 24, 0.207), (2, 34, 0.400), (3, 28, 0.395),
            (4, 22, 0.419), (5, 14, 0.556), (6, 8, 0.561), (7, 54, 0.676), (7, 80, 0.730),
        ] {
            assert_eq!(TCS[sample][band], value, "TCS{:02} at {} nm", sample + 1, 380 + 5 * band);
        }
        assert!(TCS.iter().all(|t| t.iter().all(|r| *r > 0.05 && *r < 0.75)));
    }

    /// CIE illuminant FL1, a daylight halophosphate tube, 380..=780 nm at
    /// 5 nm: the CIE's own table (CIE 015:2018 table 10.1; the CIE dataset
    /// "CIE_illum_FLs", DOI 10.25039/CIE.DS.ukaymjdn, its first column).
    const FL1: Spectrum = [
        1.87, 2.36, 2.94, 3.47, 5.17, 19.49, 6.13, 6.24, 7.01, 7.79, 8.56, 43.67, 16.94, 10.72, 11.35, 11.89, 12.37,
        12.75, 13.00, 13.15, 13.23, 13.17, 13.13, 12.85, 12.52, 12.20, 11.83, 11.50, 11.22, 11.05, 11.03, 11.18, 11.53,
        27.74, 17.05, 13.55, 14.33, 15.01, 15.52, 18.29, 19.55, 15.48, 14.91, 14.15, 13.22, 12.19, 11.12, 10.03, 8.95,
        7.96, 7.02, 6.20, 5.42, 4.73, 4.15, 3.64, 3.20, 2.81, 2.47, 2.18, 1.93, 1.72, 1.67, 1.43, 1.29, 1.19, 1.08,
        0.96, 0.88, 0.81, 0.77, 0.75, 0.73, 0.68, 0.69, 0.64, 0.68, 0.69, 0.61, 0.52, 0.43,
    ];

    /// D17 (L:103): the sample table is pinned against a published R1-R8
    /// set, which a typo in any of its 648 reflectances would move.
    ///
    /// The set is FL1's special indices as colour-science publishes them for
    /// CIE 13.3 (`colour/quality/tests/test_cri.py`, its FL1 specification,
    /// to three decimals here): an implementation of its own, on the CIE's
    /// FL1 table, whose Ra, 75.85, is the 76 the CIE gives FL1. This method
    /// differs from it only in the CCT search (6429 K here) and in reading
    /// the 5 nm daylight between the CIE's 10 nm rows, and lands within 0.06
    /// of every index: R_TOLERANCE allows that and little more.
    ///
    /// An index compares a sample under the source with the same sample
    /// under its reference, so a typo moves both and the index only by
    /// their difference: a reflectance off by 0.01 moves it by up to about
    /// 0.15, a swapped pair of digits by 0.15 to 0.4. The same specification
    /// also publishes each sample's colour under FL1 alone (XYZ with the
    /// source at Y = 100), which a typo moves directly: Y agrees here to
    /// 0.005 and X and Z to 0.07 (the observer's 5 nm table against
    /// colour-science's integration), so a reflectance 0.01 off where the
    /// eye sees well, which moves Y by 0.03 to 0.06, fails as well. One 0.01
    /// off in the deep blue or the deep red moves no reading that far.
    #[test]
    fn the_sample_table_renders_fl1_as_published() {
        const PUBLISHED_R: [f32; 8] = [69.197, 83.651, 92.136, 72.666, 73.891, 79.619, 82.273, 53.391];
        const PUBLISHED_XYZ: [[f64; 3]; 8] = [
            [31.19561134, 29.74560797, 23.44190201],
            [26.70829694, 29.25797165, 13.98804447],
            [24.20150315, 31.45341032, 9.19170689],
            [20.74577359, 29.44046997, 19.40749007],
            [25.09405566, 30.60912259, 37.93634878],
            [27.43598853, 28.84467787, 55.15209308],
            [31.30354023, 28.49931283, 51.55875721],
            [33.81156122, 29.92921717, 44.07459562],
        ];
        const R_TOLERANCE: f32 = 0.1;
        const Y_TOLERANCE: f64 = 0.01;
        const XZ_TOLERANCE: f64 = 0.1;
        let c = cri(&FL1).expect("FL1 has a reading");
        for (i, (got, want)) in c.samples.iter().zip(PUBLISHED_R).enumerate() {
            assert!((got - want).abs() < R_TOLERANCE, "R{} of FL1 is {got}, published {want}", i + 1);
        }
        assert_eq!(c.ra.round(), 76.0, "the CIE gives FL1 Ra 76: {}", c.ra);
        assert!((c.cct - 6430.0).abs() < 5.0, "the CIE gives FL1 6430 K: {}", c.cct);
        let to_y100 = 100.0 / xyz64(&FL1)[1];
        for (i, (reflectance, want)) in TCS.iter().zip(PUBLISHED_XYZ).enumerate() {
            let got = reflected_xyz(&FL1, reflectance).map(|v| v * to_y100);
            let off = [got[0] - want[0], got[1] - want[1], got[2] - want[2]];
            assert!(
                off[1].abs() < Y_TOLERANCE && off[0].abs() < XZ_TOLERANCE && off[2].abs() < XZ_TOLERANCE,
                "TCS{:02} under FL1 is {got:?}, published {want:?}",
                i + 1
            );
        }
    }

    #[test]
    fn the_references_score_a_hundred() {
        // Daylight is its own reference from 5000 K up, a blackbody below.
        let d65 = cri(&daylight(6504.0)).unwrap();
        assert!((d65.ra - 100.0).abs() < 0.5, "D65: {}", d65.ra);
        assert!((d65.cct - 6504.0).abs() < 15.0, "D65 is {} K", d65.cct);
        assert!(d65.duv < WHITE_DUV_LIMIT, "daylight lies just off the blackbody locus: {}", d65.duv);
        assert!((ra(&daylight(5503.0)) - 100.0).abs() < 0.5);
        let a = cri(&planck(2856.0)).unwrap();
        assert!((a.ra - 100.0).abs() < 0.1 && (a.cct - 2856.0).abs() < 2.0 && a.duv < 1.0e-4, "{a:?}");
        assert!(a.samples.iter().all(|r| (r - 100.0).abs() < 0.2));
        // Above 5000 K a blackbody is measured against daylight and loses a little.
        let hot = ra(&planck(6500.0));
        assert!((97.0..99.5).contains(&hot), "{hot}");
    }

    #[test]
    fn sodium_cannot_render_colours() {
        let low = cri(&sodium_low_pressure()).unwrap();
        assert!(low.ra < 0.0, "low-pressure sodium: {}", low.ra);
        assert!((1600.0..1900.0).contains(&low.cct), "{}", low.cct);
        assert!(low.duv > WHITE_DUV_LIMIT, "one line is not a white light: {}", low.duv);
        let high = cri(&sodium_high_pressure()).unwrap();
        assert!((10.0..45.0).contains(&high.ra), "high-pressure sodium: {}", high.ra);
        assert!((1800.0..2300.0).contains(&high.cct), "{}", high.cct);
        assert!(high.ra > low.ra);
    }

    #[test]
    fn the_led_quality_knob_is_the_cri() {
        // The named CRI-80 LEDs are 80, give or take 5, at every colour temperature.
        for cct in [3000.0f32, 4000.0, 5600.0] {
            let eighty = ra(&led(cct, 0.52));
            assert!((eighty - 80.0).abs() < 5.0, "{cct} K at quality 0.52: {eighty}");
            let full = ra(&led(cct, 1.0));
            assert!((full - 95.0).abs() < 5.0, "{cct} K at quality 1: {full}");
        }
        let seventy = ra(&led(5600.0, 0.32));
        assert!((seventy - 70.0).abs() < 5.0, "{seventy}");
        // More quality never renders worse.
        let mut last = f32::MIN;
        for step in 0..=10 {
            let r = ra(&led(4000.0, step as f32 / 10.0));
            assert!(r > last, "quality {}: {r} after {last}", step as f32 / 10.0);
            last = r;
        }
        // The colour temperature asked for is the one measured.
        let warm = cri(&led(3000.0, 0.52)).unwrap();
        assert!((warm.cct - 3000.0).abs() < 10.0 && warm.duv < 1.0e-3, "{warm:?}");
        // A three-band tube is in the eighties.
        for cct in [3000.0f32, 4000.0, 6500.0] {
            let tube = ra(&fluorescent_triband(cct));
            assert!((80.0..92.0).contains(&tube), "tube {cct} K: {tube}");
        }
    }

    #[test]
    fn every_named_source_has_a_reading() {
        for name in SOURCE_NAMES {
            let c = cri(&source_spectrum(name).unwrap()).unwrap_or_else(|| panic!("{name} has no CRI"));
            assert!(c.ra.is_finite() && c.ra <= 100.01 && c.cct >= 1000.0 && c.cct <= 25_000.0, "{name}: {c:?}");
            // A name that promises a CRI keeps it within 5.
            if let Some(promised) = name.rsplit("cri").next().and_then(|n| n.parse::<f32>().ok()) {
                assert!((c.ra - promised).abs() < 5.0, "{name}: {}", c.ra);
            }
        }
        assert!(cri(&[0.0; SAMPLES]).is_none(), "darkness has no colour rendering");
    }

    #[test]
    fn a_light_reads_its_source_or_its_kelvin() {
        let mut light = LightParams::default();
        // 6500 K: a blackbody against daylight.
        let kelvin = light_cri(&light).unwrap();
        assert!((kelvin.cct - 6500.0).abs() < 5.0 && (97.0..99.5).contains(&kelvin.ra), "{kelvin:?}");
        light.kelvin = 3200.0;
        assert!((light_cri(&light).unwrap().ra - 100.0).abs() < 0.1);
        light.source = Some("sodium_low_pressure".to_string());
        assert!(light_cri(&light).unwrap().ra < 0.0, "the source replaces the Kelvin");
        // Tint and gels are RGB corrections: the reading is the source's.
        let bare = light_cri(&light).unwrap();
        light.tint = 0.7;
        light.gels = Some(vec!["cto_full".to_string()]);
        assert_eq!(light_cri(&light), Some(bare));
        // An explicit colour has no spectrum.
        light.rgb = Some([1.0, 0.5, 0.2]);
        assert_eq!(light_cri(&light), None);
        assert!(light_spectrum(&light).is_none());
        // The parametric LED reads the light's Kelvin and its quality knob.
        let knob = LightParams {
            source: Some("led".to_string()),
            kelvin: 3000.0,
            source_quality: Some(1.0),
            ..LightParams::default()
        };
        let full = light_cri(&knob).unwrap();
        assert!((full.ra - 95.0).abs() < 5.0 && (full.cct - 3000.0).abs() < 15.0, "{full:?}");
        let cheap = light_cri(&LightParams { source_quality: Some(0.0), ..knob.clone() }).unwrap();
        assert!(cheap.ra < full.ra - 20.0, "the knob is the CRI: {cheap:?} vs {full:?}");
        assert_eq!(light_spectrum(&knob), Some(led(3000.0, 1.0)));
    }
}
