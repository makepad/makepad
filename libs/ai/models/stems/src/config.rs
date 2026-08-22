//! BS-RoFormer 4-stem geometry — the pinned `config_bs_roformer_384_8_2_485100`
//! shape that `model_bs_roformer_ep_17_sdr_9.6568.ckpt` was trained with.
//!
//! Nothing here is configurable at runtime on purpose: the checkpoint and the
//! band table are one artifact. A future tier (a smaller RoFormer) gets its own
//! constant table rather than a knob that can silently mismatch weights.

/// Feature dimension of the transformer trunk.
pub const DIM: usize = 384;
/// Number of (time transformer, freq transformer) blocks.
pub const DEPTH: usize = 8;
pub const HEADS: usize = 8;
pub const DIM_HEAD: usize = 64;
/// heads * dim_head — the qkv inner width.
pub const DIM_INNER: usize = HEADS * DIM_HEAD;
/// FeedForward hidden width (ff_mult 4).
pub const FF_INNER: usize = DIM * 4;
/// MaskEstimator hidden width (mlp_expansion_factor 2).
pub const MASK_HIDDEN: usize = DIM * 2;
pub const NUM_STEMS: usize = 4;

/// RoPE base. `rotary_embedding_torch::RotaryEmbedding(dim=64)` stores
/// `freqs[i] = 1 / 10000^(2i/64)`; verified numerically against the checkpoint
/// buffer.
pub const ROPE_THETA: f32 = 10_000.0;

pub const SAMPLE_RATE: u32 = 44_100;
pub const AUDIO_CHANNELS: usize = 2;

/// STFT geometry (see `stft.rs`).
pub const N_FFT: usize = 2048;
pub const HOP: usize = 441;
pub const WIN: usize = 2048;
pub const FREQ_BINS: usize = N_FFT / 2 + 1; // 1025

/// Samples the model consumes in one forward pass.
pub const CHUNK_SAMPLES: usize = 485_100;
/// `config.inference.num_overlap`.
pub const NUM_OVERLAP: usize = 2;
/// Overlap-add hop between chunks.
pub const CHUNK_STEP: usize = CHUNK_SAMPLES / NUM_OVERLAP;
/// Linear fade length at each chunk edge (`chunk_size // 10`).
pub const FADE_SAMPLES: usize = CHUNK_SAMPLES / 10;
/// Reflect padding applied to the whole track before chunking.
pub const BORDER: usize = CHUNK_SAMPLES - CHUNK_STEP;
/// STFT frames in one chunk (1 + 485100/441).
pub const CHUNK_FRAMES: usize = 1 + CHUNK_SAMPLES / HOP; // 1101

/// The model's stem order == `config.training.instruments`.
pub const STEM_NAMES: [&str; NUM_STEMS] = ["drums", "bass", "other", "vocals"];

/// One separated source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stem {
    Drums,
    Bass,
    Other,
    Vocals,
}

impl Stem {
    /// Index into the model's mask-estimator list / the artifact order.
    pub const ALL: [Stem; NUM_STEMS] = [Stem::Drums, Stem::Bass, Stem::Other, Stem::Vocals];

    pub fn index(self) -> usize {
        match self {
            Stem::Drums => 0,
            Stem::Bass => 1,
            Stem::Other => 2,
            Stem::Vocals => 3,
        }
    }

    pub fn name(self) -> &'static str {
        STEM_NAMES[self.index()]
    }

    pub fn parse(name: &str) -> Option<Stem> {
        match name {
            "drums" => Some(Stem::Drums),
            "bass" => Some(Stem::Bass),
            "other" => Some(Stem::Other),
            "vocals" => Some(Stem::Vocals),
            _ => None,
        }
    }
}

/// `freqs_per_bands` from the config: 62 bands covering all 1025 bins.
pub const FREQS_PER_BAND: [usize; 62] = [
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, // 24 x 2
    4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, // 12 x 4
    12, 12, 12, 12, 12, 12, 12, 12, // 8 x 12
    24, 24, 24, 24, 24, 24, 24, 24, // 8 x 24
    48, 48, 48, 48, 48, 48, 48, 48, // 8 x 48
    128, 129,
];

pub const NUM_BANDS: usize = FREQS_PER_BAND.len();

/// Feature width of one band: `2 (complex) * freqs * 2 (stereo)`.
pub const fn band_width(band: usize) -> usize {
    2 * FREQS_PER_BAND[band] * AUDIO_CHANNELS
}

/// Total per-frame feature width == `2 * FREQ_BINS * AUDIO_CHANNELS`.
pub const FEATURES: usize = 2 * FREQ_BINS * AUDIO_CHANNELS; // 4100

/// A run of consecutive bands that share a feature width.
///
/// The band table is sorted by width, so every distinct width is one
/// contiguous run. That is what lets the whole per-band machinery (band split
/// and both mask-estimator layers) run as SEVEN batched `mul_mat`s per stage
/// instead of 62 — the single biggest graph-node saving in the port, and the
/// reason no zero-padding (and hence no wasted FLOPs) is needed anywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BandGroup {
    /// Index of the first band in the run.
    pub first_band: usize,
    /// How many bands share this width.
    pub count: usize,
    /// Feature width of each band in the run.
    pub width: usize,
    /// Offset of the run inside the 4100-wide feature vector.
    pub feature_offset: usize,
}

/// The 7 band groups, in feature order.
pub fn band_groups() -> Vec<BandGroup> {
    let mut groups: Vec<BandGroup> = Vec::new();
    let mut feature_offset = 0usize;
    for band in 0..NUM_BANDS {
        let width = band_width(band);
        match groups.last_mut() {
            Some(last) if last.width == width => {
                last.count += 1;
            }
            _ => groups.push(BandGroup {
                first_band: band,
                count: 1,
                width,
                feature_offset,
            }),
        }
        feature_offset += width;
    }
    groups
}

/// Feature offset of a band inside the 4100-wide vector.
pub fn band_feature_offset(band: usize) -> usize {
    (0..band).map(band_width).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_table_covers_every_stft_bin() {
        assert_eq!(NUM_BANDS, 62);
        assert_eq!(FREQS_PER_BAND.iter().sum::<usize>(), FREQ_BINS);
        assert_eq!(
            (0..NUM_BANDS).map(band_width).sum::<usize>(),
            FEATURES,
            "band widths must tile the 4100-wide feature vector"
        );
    }

    #[test]
    fn chunk_geometry_matches_the_reference_demix_loop() {
        assert_eq!(CHUNK_STEP, 242_550);
        assert_eq!(BORDER, 242_550);
        assert_eq!(FADE_SAMPLES, 48_510);
        assert_eq!(CHUNK_FRAMES, 1101);
    }

    #[test]
    fn band_groups_are_seven_contiguous_runs() {
        let groups = band_groups();
        assert_eq!(groups.len(), 7, "{groups:?}");
        let widths: Vec<usize> = groups.iter().map(|g| g.width).collect();
        assert_eq!(widths, vec![8, 16, 48, 96, 192, 512, 516]);
        let counts: Vec<usize> = groups.iter().map(|g| g.count).collect();
        assert_eq!(counts, vec![24, 12, 8, 8, 8, 1, 1]);
        // Runs tile the feature axis with no gap and no overlap.
        let mut expected_offset = 0usize;
        let mut expected_band = 0usize;
        for group in &groups {
            assert_eq!(group.feature_offset, expected_offset);
            assert_eq!(group.first_band, expected_band);
            expected_offset += group.width * group.count;
            expected_band += group.count;
        }
        assert_eq!(expected_offset, FEATURES);
        assert_eq!(expected_band, NUM_BANDS);
        // Widths are strictly increasing, so a width appears in exactly one run.
        assert!(widths.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn band_feature_offsets_agree_with_groups() {
        for group in band_groups() {
            for i in 0..group.count {
                let band = group.first_band + i;
                assert_eq!(
                    band_feature_offset(band),
                    group.feature_offset + i * group.width
                );
            }
        }
    }

    #[test]
    fn stem_order_matches_the_checkpoint() {
        assert_eq!(
            Stem::ALL.map(|s| s.name()),
            ["drums", "bass", "other", "vocals"]
        );
        for stem in Stem::ALL {
            assert_eq!(Stem::parse(stem.name()), Some(stem));
        }
        assert_eq!(Stem::parse("guitar"), None);
    }
}
