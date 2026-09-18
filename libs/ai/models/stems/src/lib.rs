//! BS-RoFormer 4-stem music source separation on the makepad AI stack.
//!
//! Model: `model_bs_roformer_ep_17_sdr_9.6568.ckpt` from
//! ZFTurbo/Music-Source-Separation-Training. The CODE is MIT; the checkpoint
//! is published as a release asset of that repo without a licence of its
//! own, and it was trained on MUSDB18-HQ, whose terms allow academic use
//! only. So the weights are for development: nothing built for sale ships
//! them (see [`MODEL_COMMERCIAL_USE`]). 131.7 M parameters; MUSDB test SDR
//! 9.65 / multisong 9.38 — the best 4-stem entry in the repo's own table.
//!
//! **Placement-neutral by construction.** This crate is `demix(track) -> 4
//! stems` and nothing else: no app types, no service types, no file formats
//! beyond the checkpoint. It builds one ggml graph (`graph.rs`) and hands it to
//! whichever device store the build targets, so the same code is the Metal
//! client path and the CUDA fleet path.
//!
//! ```no_run
//! use makepad_ai_stems::{demix_all, StemsModel, StereoBuf};
//! let mut model = StemsModel::load("model_bs_roformer_ep_17_sdr_9.6568.ckpt")?;
//! let track = StereoBuf { left: vec![0.0; 44100], right: vec![0.0; 44100] };
//! let stems = demix_all(&mut model, &track, |done, total| {
//!     eprintln!("{done}/{total}");
//! })?;
//! # Ok::<(), makepad_ai_common::DiffusionError>(())
//! ```
//!
//! For playback that must start before the whole track is separated, drive
//! [`Demixer`] instead: it yields one finished 5.5-second span per model
//! forward, supports `seek`, and running it to the end reproduces
//! [`demix_all`] exactly. [`DemixCursor`] is the same stream holding no
//! model, for a thread that keeps one model and several tracks in flight.
//!
//! The chunk length is a runtime value, [`ChunkGeometry`]: the same weights
//! run at the trained 11-second chunk and at a 2.76-second one that has
//! stems for a playhead within a second, at a measured quality cost.

pub mod cache;
pub mod config;
pub mod demix;
pub mod graph;
pub mod model;
pub mod stft;
pub mod weights;

pub use config::{
    ChunkGeometry, Stem, AUDIO_CHANNELS, CHUNK_SAMPLES, CHUNK_STEP, NUM_STEMS, SAMPLE_RATE,
    STEM_NAMES,
};
pub use cache::{
    is_complete_on_disk as cache_is_complete_on_disk, prune as prune_cache, CacheError,
    CacheHeader, PruneReport, StemCache, DEFAULT_BUDGET_BYTES as CACHE_BUDGET_BYTES,
};
pub use demix::{chunk_count, demix_all, DemixCursor, Demixer, StemSpan};
pub use model::{StemSet, StemsModel, StereoBuf};
pub use weights::StemsWeights;

/// Weight-provenance record, carried into whatever metadata a caller writes
/// (the fleet lane puts it in the derivation's json; the client lane puts it in
/// the cache header). Provenance-only: nothing here enforces anything.
pub const MODEL_ID: &str = "bs-roformer-4stem";
pub const MODEL_CHECKPOINT: &str = "model_bs_roformer_ep_17_sdr_9.6568.ckpt";
pub const MODEL_SOURCE: &str = "https://github.com/ZFTurbo/Music-Source-Separation-Training";
pub const MODEL_LICENSE: &str = "code MIT (ZFTurbo/Music-Source-Separation-Training, (c) 2024 Roman Solovyev); checkpoint released without a weights licence, trained on MUSDB18-HQ (academic use only): development use only";
/// The licence line as the span cache's header carries it, frozen.
///
/// Builds from before the header learned to tell a separation from the words
/// about it compare the whole header byte for byte and DELETE an entry that
/// differs. They are still about, and they share cache roots with this one.
/// So the header goes on carrying exactly the line it always has, and an
/// entry written by either build reads as its own to the other; the
/// statement of record about these weights is [`MODEL_LICENSE`], which a
/// person reads, and not this, which only a comparison does.
pub const CACHE_HEADER_LICENSE: &str =
    "MIT (ZFTurbo/Music-Source-Separation-Training, (c) 2024 Roman Solovyev)";
/// Whether these weights may go into something sold. They may not: the
/// checkpoint carries no licence and its training set is academic-only. A
/// host that ships to customers must refuse to bundle or install them.
pub const MODEL_COMMERCIAL_USE: bool = false;
/// SHA-256 of the published checkpoint.
pub const MODEL_SHA256: &str =
    "3e9daecd70aaed5b5a0d1f861cc4d77eaa45afb3fc6301b1cf32c1be0f5868fb";
pub const MODEL_BYTES: u64 = 527_385_512;
