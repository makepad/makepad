//! The song iteration loop's core (MUSIC-AGENTS.md is the context it
//! iterates): render a Score offline to a mix and stems by role
//! ([`render`]), read it with a repeatable scorecard ([`card`]), and judge a
//! new version against the last kept one ([`card::compare`]). The SCORE
//! page and the `music_iter` tool share it.
//!
//! The batch loop ([`batch`], [`gen`]): a request (prompt, style markers,
//! length) makes several songs at once, each written by the model as a
//! whole Score and rendered as it arrives; the person rates aspects of
//! them; [`batch::learn`] has the model work out why and rewrite
//! MUSIC-AGENTS.md's learned rules ([`context`]), which the next batch
//! reads.

pub mod batch;
pub mod card;
pub mod context;
pub mod gen;
pub mod render;

pub use card::{compare, score, Card, Metric, Verdict};
pub use render::{render_song, Role, Song};
