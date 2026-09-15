//! Native Rust inference for CPJKU's Beat This! beat/downbeat tracker.
//!
//! Audio preprocessing is CPU-side; the complete neural forward pass is one
//! placement-neutral ggml graph compiled by `GraphDevice::{Metal,Cuda}`.

pub mod config;
pub mod graph;
pub mod mel;
pub mod model;
pub mod weights;

pub use config::{FRAME_RATE, SAMPLE_RATE};
pub use model::{BeatAnalysis, BeatsModel};
pub use weights::{checkpoint_census, BeatsWeights, CheckpointCensus};

pub const MODEL_ID: &str = "beat-this";
pub const MODEL_CHECKPOINT: &str = "final0.ckpt";
pub const MODEL_SOURCE: &str = "https://github.com/CPJKU/beat_this";
pub const MODEL_LICENSE: &str = "MIT";
pub const MODEL_SHA256: &str =
    "8c328b45f59d8dd3dff219253ff6a8d6482be57d0133a29140e2febbf8eb8331";
pub const MODEL_BYTES: u64 = 81_058_141;

pub const SMALL_MODEL_CHECKPOINT: &str = "small0.ckpt";
pub const SMALL_MODEL_SHA256: &str =
    "6074be2c4d490c5f6101fcc374a1ec72ae93456e23bb6019783b849f5dc7d47b";
pub const SMALL_MODEL_BYTES: u64 = 8_451_101;
