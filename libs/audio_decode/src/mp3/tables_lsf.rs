//! MPEG-2 / MPEG-2.5 (low sampling frequency) Layer III tables, plus the
//! rate-independent constants the standard defines by formula.
//!
//! ISO/IEC 13818-3 reuses MPEG-1's Huffman code tables verbatim but replaces
//! the scalefactor band layout and the whole scalefactor-compress scheme, so
//! only those pieces live here. The band boundaries were cross-checked against
//! the CC0 minimp3 distribution; every row is re-verified at test time to sum
//! to 576 lines (long) and 192 lines per window (short).

/// Long-block scalefactor band boundaries for the six LSF rates, in the order
/// [22050, 24000, 16000, 11025, 12000, 8000].
pub static SFB_LONG_LSF: [[u16; 23]; 6] = [
    // 22050
    [0, 6, 12, 18, 24, 30, 36, 44, 54, 66, 80, 96, 116, 140, 168, 200, 238, 284, 336, 396, 464, 522, 576],
    // 24000
    [0, 6, 12, 18, 24, 30, 36, 44, 54, 66, 80, 96, 114, 136, 162, 194, 232, 278, 332, 394, 464, 540, 576],
    // 16000
    [0, 6, 12, 18, 24, 30, 36, 44, 54, 66, 80, 96, 116, 140, 168, 200, 238, 284, 336, 396, 464, 522, 576],
    // 11025
    [0, 6, 12, 18, 24, 30, 36, 44, 54, 66, 80, 96, 116, 140, 168, 200, 238, 284, 336, 396, 464, 522, 576],
    // 12000
    [0, 6, 12, 18, 24, 30, 36, 44, 54, 66, 80, 96, 116, 140, 168, 200, 238, 284, 336, 396, 464, 522, 576],
    // 8000
    [0, 12, 24, 36, 48, 60, 72, 88, 108, 132, 160, 192, 232, 280, 336, 400, 476, 566, 568, 570, 572, 574, 576],
];

/// Short-block scalefactor band boundaries for the six LSF rates, in lines
/// *per window*; multiply by three for the interleaved spectral position.
pub static SFB_SHORT_LSF: [[u16; 14]; 6] = [
    // 22050
    [0, 4, 8, 12, 18, 24, 32, 42, 56, 74, 100, 132, 174, 192],
    // 24000
    [0, 4, 8, 12, 18, 26, 36, 48, 62, 80, 104, 136, 180, 192],
    // 16000
    [0, 4, 8, 12, 18, 26, 36, 48, 62, 80, 104, 134, 174, 192],
    // 11025
    [0, 4, 8, 12, 18, 26, 36, 48, 62, 80, 104, 134, 174, 192],
    // 12000
    [0, 4, 8, 12, 18, 26, 36, 48, 62, 80, 104, 134, 174, 192],
    // 8000
    [0, 8, 16, 24, 36, 52, 72, 96, 124, 160, 162, 164, 166, 192],
];

/// Scalefactor partition sizes for LSF, indexed
/// `[slen-selector 0..6][block kind 0=long, 1=short, 2=mixed][partition 0..4]`
/// (ISO/IEC 13818-3 Table B.1). Selectors 0..2 are the normal path, 3..5 the
/// intensity-stereo right channel. Long rows total 21 bands, short rows 36
/// (12 bands x 3 windows) and mixed rows 33 (6 long bands + 9 short x 3).
pub static LSF_NSFB: [[[u8; 4]; 3]; 6] = [
    [[6, 5, 5, 5], [9, 9, 9, 9], [6, 9, 9, 9]],
    [[6, 5, 7, 3], [9, 9, 12, 6], [6, 9, 12, 6]],
    [[11, 10, 0, 0], [18, 18, 0, 0], [15, 18, 0, 0]],
    [[7, 7, 7, 0], [12, 12, 12, 0], [6, 15, 12, 0]],
    [[6, 6, 6, 3], [12, 9, 9, 6], [6, 12, 9, 6]],
    [[8, 8, 5, 0], [15, 12, 9, 0], [6, 18, 9, 0]],
];

/// Extra scalefactor weighting for long blocks when `preflag` is set
/// (Table 3-B.6).
pub static PRETAB: [u8; 22] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 3, 3, 3, 2, 0,
];

/// Alias-reduction butterfly coefficients, `cs = 1/sqrt(1+c^2)` and
/// `ca = c/sqrt(1+c^2)` over the eight `c` of Table 3-B.9.
pub fn alias_coefficients() -> ([f32; 8], [f32; 8]) {
    const C: [f64; 8] = [-0.6, -0.535, -0.33, -0.185, -0.095, -0.041, -0.0142, -0.0037];
    let mut cs = [0.0f32; 8];
    let mut ca = [0.0f32; 8];
    for i in 0..8 {
        let norm = (1.0 + C[i] * C[i]).sqrt();
        cs[i] = (1.0 / norm) as f32;
        ca[i] = (C[i] / norm) as f32;
    }
    (cs, ca)
}

/// Intensity-stereo pan ratios for MPEG-1: `tan(is_pos * pi/12)` folded into
/// the pair `(k_left, k_right)`. Position 6 is the pure-left end (the ratio is
/// infinite) and position 7 is the "not intensity coded" escape, which callers
/// must handle before indexing.
pub fn intensity_pan_mpeg1() -> [(f32, f32); 7] {
    let mut out = [(0.0f32, 0.0f32); 7];
    for (i, slot) in out.iter_mut().enumerate() {
        if i == 6 {
            *slot = (1.0, 0.0);
        } else {
            let ratio = (std::f64::consts::PI / 12.0 * i as f64).tan();
            *slot = ((ratio / (1.0 + ratio)) as f32, (1.0 / (1.0 + ratio)) as f32);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsf_band_tables_cover_the_spectrum() {
        for (i, row) in SFB_LONG_LSF.iter().enumerate() {
            assert_eq!(row[0], 0, "row {i}");
            assert_eq!(row[22], 576, "row {i}");
            assert!(row.windows(2).all(|w| w[1] >= w[0]), "row {i} is not monotone");
        }
        for (i, row) in SFB_SHORT_LSF.iter().enumerate() {
            assert_eq!(row[0], 0, "row {i}");
            assert_eq!(row[13], 192, "row {i}");
            assert!(row.windows(2).all(|w| w[1] >= w[0]), "row {i} is not monotone");
        }
    }

    #[test]
    fn lsf_partition_rows_total_their_band_counts() {
        for (i, kinds) in LSF_NSFB.iter().enumerate() {
            let total = |k: usize| kinds[k].iter().map(|&v| v as u32).sum::<u32>();
            assert_eq!(total(0), 21, "selector {i} long");
            assert_eq!(total(1), 36, "selector {i} short");
            assert_eq!(total(2), 33, "selector {i} mixed");
        }
    }

    #[test]
    fn alias_coefficients_are_normalised() {
        let (cs, ca) = alias_coefficients();
        for i in 0..8 {
            assert!((cs[i] * cs[i] + ca[i] * ca[i] - 1.0).abs() < 1e-6, "i={i}");
            assert!(ca[i] < 0.0);
        }
        assert!((cs[0] - 0.857_492_9).abs() < 1e-5);
        assert!((ca[0] + 0.514_495_8).abs() < 1e-5);
    }

    #[test]
    fn intensity_pan_matches_the_standard_ratios() {
        let pan = intensity_pan_mpeg1();
        assert_eq!(pan[0], (0.0, 1.0));
        assert!((pan[3].0 - 0.5).abs() < 1e-6 && (pan[3].1 - 0.5).abs() < 1e-6);
        assert_eq!(pan[6], (1.0, 0.0));
        for (kl, kr) in pan {
            assert!((0.0..=1.0).contains(&kl) && (0.0..=1.0).contains(&kr));
        }
    }
}
