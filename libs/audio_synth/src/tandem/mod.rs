//! Tandem: the two-scene hybrid synth layout over the kit. Each scene has
//! three oscillators of twelve types (FM between them, ring combinators,
//! noise), a mixer, a filter block of two filters from the shared family
//! with a waveshaper in one of eight configurations, two envelopes, six
//! voice and six scene LFOs; voices run 2x oversampled in 32-sample
//! blocks; four insert, four send and four global effect slots follow.
//! It loads the hybrid synth's own patch library (`.fxp`, revisions
//! 0..31) losslessly ([`patch`]).

pub mod binn;
pub mod catalog;
pub mod convert;
pub mod engine;
pub mod formula;
pub mod library;
pub mod patch;
pub mod program;
pub mod scene;
pub mod schema;
pub mod slots;
pub mod snapshots;
pub mod voice;
pub mod xml;

pub use engine::Tandem;
pub use patch::{LoadMode, Patch};
