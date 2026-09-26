//! BS-RoFormer 4-stem geometry — the pinned `config_bs_roformer_384_8_2_485100`
//! shape that `model_bs_roformer_ep_17_sdr_9.6568.ckpt` was trained with.
//!
//! The band table and the trunk widths are the checkpoint's and are not
//! configurable: the checkpoint and the band table are one artifact. The
//! chunk length is the one thing that is NOT baked into the weights — no
//! weight depends on the frame count — so it is a runtime value,
//! [`ChunkGeometry`], and the same checkpoint runs at the long chunk it was
//! trained at and at a short one that answers faster.

use makepad_ai_common::{DiffusionError, Result};

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

/// `config.inference.num_overlap`.
pub const NUM_OVERLAP: usize = 2;

/// How the track is cut into model forwards: the chunk the model consumes
/// and everything the overlap-add derives from it.
///
/// Two geometries are named. [`FULL`](Self::FULL) is the 11-second chunk the
/// checkpoint was trained and measured at; it is the separator's full
/// quality and the grid the span cache is addressed in. [`BRIDGE`](Self::BRIDGE)
/// is a 2.76-second chunk that runs a forward in a fraction of the time and
/// so has stems for a playhead within a second of a load or a seek, at a
/// measured cost of about 11 dB against the full output on the weakest stem.
/// Any other size on the grain goes through [`new`](Self::new).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkGeometry {
    /// Samples the model consumes in one forward pass.
    pub samples: usize,
    /// Overlap-add hop between chunks.
    pub step: usize,
    /// Linear fade length at each chunk edge (`chunk_size // 10`).
    pub fade: usize,
    /// Reflect padding applied to the whole track before chunking.
    pub border: usize,
    /// STFT frames in one chunk (`1 + samples / HOP`).
    pub frames: usize,
}

impl ChunkGeometry {
    /// The trained chunk: `config_bs_roformer_384_8_2_485100`.
    pub const FULL: ChunkGeometry = ChunkGeometry::derive(485_100);
    /// The short chunk: 276 STFT hops, 2.76 s.
    pub const BRIDGE: ChunkGeometry = ChunkGeometry::derive(276 * HOP);
    /// A chunk is a whole number of STFT hops split into two equal steps,
    /// so its length is a multiple of this.
    pub const GRAIN: usize = NUM_OVERLAP * HOP;

    /// A geometry of `samples` per chunk, refused off the grain. A chunk
    /// that is not a whole number of hops does not come back out of the
    /// inverse transform at its own length, and one whose step is not its
    /// border breaks the span arithmetic the stream is built on; either
    /// demixed to silence rather than failing.
    pub fn new(samples: usize) -> Result<ChunkGeometry> {
        if samples == 0 || samples % Self::GRAIN != 0 {
            return Err(DiffusionError::model(format!(
                "stems: a chunk of {samples} samples is not a multiple of {}",
                Self::GRAIN
            )));
        }
        Ok(Self::derive(samples))
    }

    const fn derive(samples: usize) -> ChunkGeometry {
        let step = samples / NUM_OVERLAP;
        ChunkGeometry {
            samples,
            step,
            fade: samples / 10,
            border: samples - step,
            frames: 1 + samples / HOP,
        }
    }

    /// Seconds of audio one chunk holds.
    pub fn secs(&self) -> f64 {
        self.samples as f64 / SAMPLE_RATE as f64
    }

    /// Seconds of audio one forward finalizes.
    pub fn step_secs(&self) -> f64 {
        self.step as f64 / SAMPLE_RATE as f64
    }

    /// The reference only pads the track when it is longer than `2 * border`.
    pub fn track_padding(&self, len: usize) -> usize {
        if len > 2 * self.border {
            self.border
        } else {
            0
        }
    }

    /// Number of chunks the reference loop runs for a track of `len` samples.
    pub fn chunk_count(&self, len: usize) -> usize {
        let padded = len + 2 * self.track_padding(len);
        if padded == 0 {
            return 0;
        }
        padded.div_ceil(self.step)
    }
}

/// Samples the model consumes in one forward pass, at the full geometry.
pub const CHUNK_SAMPLES: usize = ChunkGeometry::FULL.samples;
/// Overlap-add hop between chunks, at the full geometry. The span cache and
/// every consumer of a 5.5-second span are on this grid.
pub const CHUNK_STEP: usize = ChunkGeometry::FULL.step;
/// Linear fade length at each chunk edge, at the full geometry.
pub const FADE_SAMPLES: usize = ChunkGeometry::FULL.fade;
/// Reflect padding applied to the whole track before chunking, at the full
/// geometry.
pub const BORDER: usize = ChunkGeometry::FULL.border;
/// STFT frames in one chunk at the full geometry (1 + 485100/441).
pub const CHUNK_FRAMES: usize = ChunkGeometry::FULL.frames; // 1101

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
        assert_eq!(CHUNK_SAMPLES, 485_100);
        assert_eq!(CHUNK_STEP, 242_550);
        assert_eq!(BORDER, 242_550);
        assert_eq!(FADE_SAMPLES, 48_510);
        assert_eq!(CHUNK_FRAMES, 1101);
        assert_eq!(ChunkGeometry::new(485_100).unwrap(), ChunkGeometry::FULL);
    }

    #[test]
    fn the_bridge_geometry_is_a_whole_number_of_hops() {
        let bridge = ChunkGeometry::BRIDGE;
        assert_eq!(bridge.samples, 121_716);
        assert_eq!(bridge.step, 60_858);
        assert_eq!(bridge.border, bridge.step);
        assert_eq!(bridge.fade, 12_171);
        assert_eq!(bridge.frames, 277);
        assert_eq!(ChunkGeometry::new(121_716).unwrap(), bridge);
        assert!((bridge.secs() - 2.76).abs() < 1e-9);
        assert!((bridge.step_secs() - 1.38).abs() < 1e-9);
    }

    /// 121 275 samples (275 hops) is not a multiple of the grain: its step is
    /// not a whole number of hops and its border is one sample longer than
    /// its step. Run, it demixed every span to silence; it must be refused.
    #[test]
    fn a_chunk_off_the_grain_is_refused() {
        assert!(ChunkGeometry::new(121_275).is_err());
        assert!(ChunkGeometry::new(0).is_err());
        assert!(ChunkGeometry::new(441).is_err());
        assert!(ChunkGeometry::new(882).is_ok());
    }

    #[test]
    fn chunk_counts_cover_the_padded_track_at_both_geometries() {
        let four_minutes = 240 * SAMPLE_RATE as usize;
        for geometry in [ChunkGeometry::FULL, ChunkGeometry::BRIDGE] {
            let padded = four_minutes + 2 * geometry.border;
            assert_eq!(geometry.chunk_count(four_minutes), padded.div_ceil(geometry.step));
            // A track no longer than twice the border is not padded at all,
            // and one sample more is.
            assert_eq!(geometry.track_padding(2 * geometry.border), 0);
            assert_eq!(geometry.track_padding(2 * geometry.border + 1), geometry.border);
            assert_eq!(geometry.chunk_count(1000), 1);
            assert_eq!(geometry.chunk_count(0), 0);
        }
        assert_eq!(ChunkGeometry::FULL.chunk_count(four_minutes), 46);
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
