//! Train it yourself: music training data for a language model, built on
//! the person's machine from the song libraries they chose to download and
//! their own files, through the importers Stage reads songs with.
//!
//! - [`source`]: the libraries (the Lakh MIDI clean subset; a folder of
//!   the person's MIDI, tracker and chip files).
//! - [`prep`]: song identity and the split by song, duplicates, windows of
//!   bars, transposition and tempo labels.
//! - [`tasks`]: the chat examples (describe -> write, continue, infill a
//!   part, roll -> Score, Score -> roll), every answer read back.
//! - [`tokens`]: counts with the Qwen tokenizer from a GGUF.
//! - [`corpus`]: the build, its shards, manifest and stats.
//! - [`eval`]: the metrics a model's answers are scored by, and baselines.
//!
//! The data it writes is the person's and stays on their machine: it is
//! never shipped and never committed.

pub mod corpus;
pub mod eval;
pub mod prep;
pub mod source;
pub mod tasks;
pub mod tokens;
