//! Dependency-free Standard MIDI File (SMF) parsing, derived musical views,
//! and serialization.

mod error;
mod model;
mod parse;
mod timing;
mod write;

pub use error::{MidiError, MidiErrorKind, MidiResult, WriteError};
pub use model::*;
pub use parse::{parse, parse_with_options, ParseOptions};
pub use timing::*;
pub use write::{write, write_with_options, WriteOptions};

#[cfg(test)]
mod tests;
