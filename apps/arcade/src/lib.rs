//! Arcade as a library, so tools can reuse the REAL authoring context.
//!
//! The eval harness (`tools/arcade_eval`) must send the same system prompt and
//! the same tool policy the app sends, or it measures a fiction. Exposing the
//! modules here is what keeps the two from drifting; the app binary keeps its
//! own `mod` declarations because `app_main!` owns the process entry point.
pub use makepad_widgets;

pub mod ai;
/// The Zelda-scale world build. Layout is pure and testable; see the module
/// docs for why it is split from realisation.
pub mod bigworld;
pub mod capability;
pub mod library;
pub mod pair_server;
pub mod pairing;
