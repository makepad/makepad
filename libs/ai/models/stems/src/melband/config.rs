//! Mel-Band RoFormer vocals geometry — the shape `MelBandRoformer.ckpt` was
//! trained with.
//!
//! The trunk is the four-stem model's trunk (same width, heads, feed-forward
//! and rotary convention, so [`crate::config`] supplies those) at six blocks
//! instead of eight. What differs is everything around it: the bands are 60
//! mel-spaced bin ranges that OVERLAP, every axial transformer ends in its own
//! norm, the mask head is one layer deeper and twice as wide, and there is a
//! single target.
//!
//! As with the four-stem model the band table and the checkpoint are one
//! artifact. The reference derives the table at construction time from a mel
//! filterbank and keeps only which bins each filter touches; the result is
//! baked in here as a constant, checked against the checkpoint's own band
//! widths, so that nothing at run time depends on reproducing a filterbank
//! to the last bit.

use crate::config::{ChunkGeometry, AUDIO_CHANNELS, DIM, FEATURES, FREQ_BINS};

/// Number of (time transformer, freq transformer) blocks.
pub const DEPTH: usize = 6;
/// MaskEstimator hidden width (mlp_expansion_factor 4).
pub const MASK_HIDDEN: usize = DIM * 4;
pub const NUM_BANDS: usize = 60;
/// The model estimates the vocal and nothing else; the accompaniment is what
/// is left of the mix (see [`super::instrumental`]).
pub const NUM_TARGETS: usize = 1;
/// The lane a cache entry of this model holds, in [`NUM_TARGETS`] order.
pub const LANES: [&str; NUM_TARGETS] = ["vocals"];

/// The trained chunk: 8 seconds, `config.inference.chunk_size` 352800, with
/// the same 2x overlap as the four-stem model. Spelled out rather than
/// derived because the deriving function is private to the four-stem
/// configuration; a test holds it to `ChunkGeometry::new(352_800)`.
pub const CHUNK: ChunkGeometry = ChunkGeometry {
    samples: 352_800,
    step: 176_400,
    fade: 35_280,
    border: 176_400,
    frames: 801,
};

/// The STFT bins of each band as a half-open range `(first, end)`.
///
/// These are the bins where the reference's 60-filter Slaney mel bank
/// (44.1 kHz, 2048-point transform) is positive, with its two forced corners
/// (bin 0 into band 0, bin 1024 into band 59). A triangular bank makes every
/// band one contiguous run, makes each band end where the band after next
/// begins, and so puts every bin in one band or in two.
pub const BAND_BINS: [(usize, usize); NUM_BANDS] = [
    (0, 7), (4, 10), (7, 13), (10, 16), (13, 19), (16, 22),
    (19, 25), (22, 28), (25, 31), (28, 34), (31, 37), (34, 40),
    (37, 43), (40, 46), (43, 49), (46, 53), (49, 56), (53, 60),
    (56, 65), (60, 69), (65, 74), (69, 79), (74, 84), (79, 90),
    (84, 97), (90, 103), (97, 110), (103, 118), (110, 126), (118, 135),
    (126, 145), (135, 155), (145, 165), (155, 177), (165, 189), (177, 203),
    (189, 217), (203, 232), (217, 248), (232, 265), (248, 284), (265, 304),
    (284, 325), (304, 348), (325, 372), (348, 398), (372, 426), (398, 455),
    (426, 487), (455, 521), (487, 558), (521, 597), (558, 638), (597, 683),
    (638, 731), (683, 782), (731, 836), (782, 895), (836, 958), (895, 1025),
];

/// Features one bin contributes: `2 (complex) * 2 (stereo)`.
const FEATURES_PER_BIN: usize = 2 * AUDIO_CHANNELS;

/// Feature width of one band: `2 (complex) * freqs * 2 (stereo)`.
pub const fn band_width(band: usize) -> usize {
    (BAND_BINS[band].1 - BAND_BINS[band].0) * FEATURES_PER_BIN
}

/// Where a band starts inside the 4100-wide spectrum vector.
///
/// Within a band the reference orders its input `((bin - first) * 2 +
/// channel) * 2 + re_im`, which is [`crate::model::feature_index`] shifted by
/// the band's first bin. A band's input is therefore a plain slice of the
/// spectrum the four-stem model already packs; the slices merely overlap.
pub const fn band_feature_offset(band: usize) -> usize {
    BAND_BINS[band].0 * FEATURES_PER_BIN
}

/// Where a band starts inside the band-concatenated mask the graph returns.
pub const fn band_mask_offset(band: usize) -> usize {
    let mut offset = 0;
    let mut b = 0;
    while b < band {
        offset += band_width(b);
        b += 1;
    }
    offset
}

/// Width of the 60 band masks laid end to end: 1979 bin memberships of four
/// features each, against the 4100 features of the spectrum itself.
pub const BAND_FEATURES: usize = band_mask_offset(NUM_BANDS);

/// For each of the 4100 spectrum features, one over the number of bands that
/// cover its bin. The reference averages the masks of overlapping bands; the
/// count is only ever one or two, so the factor is exactly 1.0 or 0.5 and the
/// average loses nothing to the division.
pub fn inverse_band_counts() -> Vec<f32> {
    let mut counts = vec![0u32; FREQ_BINS];
    for (first, end) in BAND_BINS {
        for count in &mut counts[first..end] {
            *count += 1;
        }
    }
    let mut out = Vec::with_capacity(FEATURES);
    for count in counts {
        for _ in 0..FEATURES_PER_BIN {
            out.push(1.0 / count as f32);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `band_split.to_features.{b}.0.gamma` widths of the published
    /// checkpoint divided by four, written out independently of the table.
    const CHECKPOINT_BINS_PER_BAND: [usize; NUM_BANDS] = [
        7, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 7, 7, 7, 9, 9, 9, 10, 10, 11, 13, 13, 13,
        15, 16, 17, 19, 20, 20, 22, 24, 26, 28, 29, 31, 33, 36, 39, 41, 44, 47, 50, 54, 57, 61,
        66, 71, 76, 80, 86, 93, 99, 105, 113, 122, 130,
    ];

    #[test]
    fn band_widths_are_the_checkpoints() {
        for band in 0..NUM_BANDS {
            let (first, end) = BAND_BINS[band];
            assert!(first < end && end <= FREQ_BINS, "band {band}");
            assert_eq!(end - first, CHECKPOINT_BINS_PER_BAND[band], "band {band}");
            assert_eq!(band_width(band), 4 * CHECKPOINT_BINS_PER_BAND[band]);
        }
    }

    #[test]
    fn the_bands_hold_1979_bin_memberships() {
        let memberships: usize = BAND_BINS.iter().map(|(first, end)| end - first).sum();
        assert_eq!(memberships, 1979);
        assert_eq!(BAND_FEATURES, 4 * 1979);
        assert_eq!(BAND_FEATURES, 7916);
        assert_eq!(band_mask_offset(0), 0);
        for band in 1..NUM_BANDS {
            assert_eq!(
                band_mask_offset(band),
                band_mask_offset(band - 1) + band_width(band - 1)
            );
        }
    }

    /// A triangular filter is positive strictly between the centres of its
    /// two neighbours, so every other band tiles the axis: the even bands
    /// cover bins `[0, 958)` end to end and the odd bands `[4, 1025)`.
    #[test]
    fn even_and_odd_bands_each_tile_the_bins_without_gap_or_overlap() {
        for (parity, from, to) in [(0usize, 0usize, 958usize), (1, 4, 1025)] {
            let mut at = from;
            for band in (parity..NUM_BANDS).step_by(2) {
                let (first, end) = BAND_BINS[band];
                assert_eq!(first, at, "band {band} does not continue its run");
                at = end;
            }
            assert_eq!(at, to);
        }
    }

    #[test]
    fn every_bin_is_in_one_band_or_two() {
        let inverse = inverse_band_counts();
        assert_eq!(inverse.len(), FEATURES);
        let mut single = 0usize;
        for bin in 0..FREQ_BINS {
            let covering = BAND_BINS
                .iter()
                .filter(|(first, end)| (*first..*end).contains(&bin))
                .count();
            assert!(covering == 1 || covering == 2, "bin {bin} is in {covering} bands");
            if covering == 1 {
                single += 1;
            }
            for feature in 0..FEATURES_PER_BIN {
                let want = if covering == 1 { 1.0 } else { 0.5 };
                assert_eq!(inverse[bin * FEATURES_PER_BIN + feature], want);
            }
        }
        // Bins 0..4 lie below the second band and 958..1025 above the last
        // but one.
        assert_eq!(single, 4 + 67);
    }

    #[test]
    fn a_band_reads_the_spectrum_in_the_four_stem_feature_order() {
        use crate::model::feature_index;
        for band in 0..NUM_BANDS {
            let (first, end) = BAND_BINS[band];
            assert_eq!(band_feature_offset(band), feature_index(first, 0, 0));
            assert_eq!(
                band_feature_offset(band) + band_width(band),
                feature_index(end - 1, AUDIO_CHANNELS - 1, 1) + 1
            );
        }
        assert_eq!(band_feature_offset(NUM_BANDS - 1) + band_width(NUM_BANDS - 1), FEATURES);
    }

    #[test]
    fn the_chunk_is_the_trained_eight_seconds_on_the_shared_grain() {
        assert_eq!(CHUNK, ChunkGeometry::new(352_800).unwrap());
        assert_eq!(CHUNK.step, 176_400);
        assert_eq!(CHUNK.border, CHUNK.step);
        assert_eq!(CHUNK.frames, 801);
        assert!((CHUNK.secs() - 8.0).abs() < 1e-9);
        assert!((CHUNK.step_secs() - 4.0).abs() < 1e-9);
    }
}
