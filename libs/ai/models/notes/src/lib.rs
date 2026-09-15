//! Native Rust port of Spotify Basic Pitch (Bittner et al., ICASSP 2022).
//!
//! The model code and published `nmp.onnx` weights are Apache-2.0. This port
//! is a from-scratch implementation of the documented architecture and the
//! Apache-licensed reference algorithms; no Python runtime or ONNX runtime is
//! required.

pub mod config;
pub mod cqt;
pub mod graph;
pub mod model;
pub mod weights;

pub use config::{FRAME_RATE, SAMPLE_RATE};
pub use model::{create_notes, to_midi_bytes, NoteEvent, NoteTranscription, NotesModel};
pub use weights::{NotesWeights, WeightCensus};

pub const MODEL_ID: &str = "basic-pitch";
pub const MODEL_LICENSE: &str = "Apache-2.0";
pub const MODEL_SOURCE: &str = "https://github.com/spotify/basic-pitch";
pub const MODEL_FILE: &str = "basic_pitch_nmp.onnx";
pub const MODEL_BYTES: u64 = 230_444;
pub const MODEL_SHA256: &str =
    "2c3c1d144bfa61ad236e92e169c13535c880469a12a047d4e73451f2c059a0ec";
