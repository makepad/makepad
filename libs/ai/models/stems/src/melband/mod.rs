//! Mel-Band RoFormer vocal separation, beside the four-stem model.
//!
//! Model: `MelBandRoformer.ckpt`, a vocals-only Mel-Band RoFormer (228.2 M
//! parameters). The architecture code it was ported from is the MIT
//! reference the four-stem model also came from; the weights' standing is
//! recorded on [`crate::VOCALS_MODEL_LICENSE`] and
//! [`crate::VOCALS_MODEL_COMMERCIAL_USE`].
//!
//! It shares everything it can with the four-stem path: the STFT, the
//! spectrum packing and the complex mask (`stft.rs`, `model.rs`), the
//! transformer builder (`graph.rs`), the checkpoint loader (`weights.rs`),
//! and — through [`ChunkSeparator`](crate::ChunkSeparator) at one target —
//! the overlap-add stream and the span cache. What is its own is here: the
//! band table and chunk (`config`), the weight plan (`weights`), the graph
//! around the trunk (`graph`), and the model type (`model`).
//!
//! ```no_run
//! use makepad_ai_stems::melband::{instrumental, VocalsModel};
//! use makepad_ai_stems::{demix_all_lanes, StereoBuf};
//! let mut model = VocalsModel::load("MelBandRoformer.ckpt")?;
//! let track = StereoBuf { left: vec![0.0; 44100], right: vec![0.0; 44100] };
//! let vocals = demix_all_lanes(&mut model, &track, |_, _| {})?.remove(0);
//! let rest = instrumental(&track, &vocals);
//! # let _ = rest;
//! # Ok::<(), makepad_ai_common::DiffusionError>(())
//! ```

pub mod config;
pub mod graph;
pub mod model;
pub mod weights;

pub use config::{CHUNK, LANES, NUM_TARGETS};
pub use graph::Stage;
pub use model::{instrumental, StageProbe, VocalsModel};

use crate::cache::{CacheHeader, ModelIdentity};

impl ModelIdentity {
    /// The vocals-only Mel-Band RoFormer.
    pub const MEL_BAND_VOCALS: ModelIdentity = ModelIdentity {
        model_id: crate::VOCALS_MODEL_ID,
        checkpoint: crate::VOCALS_MODEL_CHECKPOINT,
        checkpoint_sha256: crate::VOCALS_MODEL_SHA256,
        license: crate::VOCALS_MODEL_LICENSE,
        source: crate::VOCALS_MODEL_SOURCE,
    };
}

/// The cache header of a track of `frames` separated by this model: one
/// `vocals` lane in 4-second spans, the step of [`CHUNK`].
///
/// An entry under this header must live under a root of its own. Opening it
/// on a directory that holds a four-stem entry of the same track is refused
/// and leaves that entry alone, but builds that know nothing of lanes share
/// the four-stem root, and they delete what they do not recognise.
pub fn cache_header(frames: u64) -> CacheHeader {
    CacheHeader::for_model(
        &ModelIdentity::MEL_BAND_VOCALS,
        CHUNK.step as u64,
        frames,
        &LANES,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_header_names_one_vocals_lane_on_the_four_second_grid() {
        let frames = 8_653_008u64;
        let header = cache_header(frames);
        assert_eq!(header.model_id, "mel-band-roformer-vocals");
        assert_eq!(header.checkpoint, "MelBandRoformer.ckpt");
        assert_eq!(header.checkpoint_sha256, crate::VOCALS_MODEL_SHA256);
        assert_eq!(header.stems, vec!["vocals".to_string()]);
        assert_eq!(header.span_samples, 176_400);
        assert_eq!(header.span_count, 50);
        assert_eq!(header.sample_rate, crate::SAMPLE_RATE);
    }

    #[test]
    fn a_vocals_entry_is_never_taken_for_a_four_stem_entry_of_the_same_track() {
        let frames = 8_653_008u64;
        let vocals = cache_header(frames);
        let four = CacheHeader::for_track(frames);
        assert!(!vocals.same_separation(&four));
        assert!(!four.same_separation(&vocals));
        assert!(vocals.same_separation(&cache_header(frames)));
    }

    #[test]
    fn the_checkpoint_hash_is_a_sha256() {
        assert_eq!(crate::VOCALS_MODEL_SHA256.len(), 64);
        assert!(crate::VOCALS_MODEL_SHA256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert!(!crate::VOCALS_MODEL_COMMERCIAL_USE);
    }
}
