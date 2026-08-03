//! The `game.*` script binding layer for Makepad Arcade (game.md §game_script).
//!
//! Table-driven dispatch over the sim + blocks, plus a `ScriptHost` that owns
//! an isolate and evaluates a `game.splash` with last-good rollback. Arcade
//! loads games through this; the sim itself never sees the script VM.

mod build;
pub mod callbacks;
pub mod audio3d;
pub mod dispatch;
pub mod host;
pub mod input;
pub mod sandbox;
pub mod value;

pub use dispatch::{AudioRequest, Ctx, ToneWave, VerbFn, VERBS};
pub use host::{EvalReport, ScriptHost};
pub use sandbox::{strip_capabilities, Trust};

/// `game.api()` text — also what splashgame.md documents.
pub fn api_text() -> String {
    let mut out = String::new();
    for (name, _, doc) in VERBS {
        out.push_str(&format!("game.{name}{doc}\n"));
    }
    out
}
