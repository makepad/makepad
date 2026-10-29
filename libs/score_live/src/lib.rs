//! Scores: live-coded music. One Splash document is at once a flow graph
//! (instruments, loops, FX, buses and the master are nodes and wires),
//! loop code (each loop's body runs once per cycle on a virtual clock) and
//! an arrangement (sections in time). Every view edits the same source.
//!
//! - [`source`]: the lexer, the outline and source-preserving patches.
//! - [`theory`]: notes, chords, scales, patterns.
//! - [`vm`]: `mod.score` and the functions loop code calls.
//! - [`model`]: the typed score, loaded with diagnostics.
//! - [`compose`]: runs loop code ahead of the clock into events.
//! - [`player`]: the score built as the audio engine's nodes (data) and
//!   prepared on its own clock or its host's.
//! - [`node`]: `Score`, a score as one node of any audio flow graph, fed
//!   from a slot.
//! - [`render`]: offline render on the audio engine.
//! - [`held`]: settings an editor holds live, landed without a revision.
//! - [`generate`]: code generators (arpeggios, progressions, rhythms, rewrites).
//! - [`vary`]: melody variants of a loop's part (seeded), applied or merged by bar.
//! - [`live`]: a live score: the composer thread feeding the graph's loops.
//! - [`midi`]: a MIDI performance written as score code, and checked.
//! - [`tools`]: the Score tools every app's AI registry offers.

pub mod assist;
pub mod audition;
pub mod build;
pub mod compose;
pub mod game;
pub mod generate;
pub mod gm_font;
pub mod graph;
pub mod guide;
pub mod held;
pub mod instruments;
pub mod listen;
pub mod live;
pub mod osc;
pub mod palette;
pub mod node;
pub mod player;
pub mod midi;
pub mod model;
pub mod render;
pub mod source;
pub mod tandem_library;
pub mod templates;
pub mod theory;
pub mod tools;
pub mod vary;
pub mod vm;

/// The MIDI the live score's MIDI loops send (the host puts it on its outputs).
pub use makepad_audio_engine::feed::midi_out;
/// Live audio in: the host pushes the input device's frames here.
pub use makepad_audio_engine::feed::audio_in;
/// An instrument's settings held live (an editor's control under a hand).
pub use makepad_audio_engine::feed::{live_params, LiveParams};
/// Seconds at a beat under a tempo map (as renders and the transport time it).
pub use makepad_audio_engine::feed::tempo_seconds;
pub use build::{NoSamples, Samples};
pub use live::{ClockSource, LiveConfig, LiveScore, Status};
pub use model::{load, AssetRef, Diag, ScoreDoc};
pub use render::{render, RenderOpts, Rendered};

/// The stable key of a node name: what the player, the meters and the
/// graph call a node.
pub fn key(name: &str) -> u64 {
    // FNV-1a, never zero (zero is "no node").
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h.max(1)
}

#[cfg(test)]
mod tests;
