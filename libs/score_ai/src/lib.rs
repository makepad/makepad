//! AI-assisted score generation through the asset server chat broker.
//!
//! The crate is headless: callers provide a broker and a MusicXML-to-score
//! importer, then receive an ordinary editable [`makepad_score::model::Score`]
//! after extraction, musical validation, and a bounded repair loop.

mod broker;
pub mod local_broker;
mod engine;
mod extract;
mod prompt;
mod provenance;
mod validate;

pub use broker::*;
pub use engine::*;
pub use extract::*;
pub use prompt::*;
pub use provenance::*;
pub use validate::*;

#[cfg(test)]
mod tests;
