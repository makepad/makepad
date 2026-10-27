//! The audio engine (`stage.audio`'s own node types, without decks).
//!
//! - [`nodes`] registers the connection types and node descriptors.
//! - [`schedule`] prepares an owner slice into a [`schedule::Schedule`]: ops
//!   in order, every buffer and DSP state allocated, off the audio thread.
//!   An owner's own node types (Stage's decks) come in through
//!   [`schedule::Ext`]; a whole player inside a schedule (a score) comes
//!   in through [`nested`].
//! - [`run`] is one render pass over a schedule.
//!
//! `makepad-stage-live-audio` is Stage's owner on top: the decks, the
//! engine that mixes every live instance and the host runtime.

pub mod clock;
pub mod feed;
pub mod nested;
pub mod nodes;
pub mod pads;
pub mod rest;
pub mod run;
pub mod schedule;

pub use nodes::{OWNER, AUDIO, CLOCK, LEVEL};

/// Frames one pass over a schedule renders. A device buffer of any size is
/// rendered as that many passes; nothing is truncated.
pub const CHUNK: usize = 256;
