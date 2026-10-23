//! The node DSP of a score's graph: what the audio engine's `Loop` and FX
//! nodes (makepad-audio-engine) and audio shaders run.
//!
//! - [`loop_engine`]: a loop's instrument and voice pool, notes started and
//!   stopped on their exact frame.
//! - [`fx`]: the FX and buses (filters, delay, reverb, drive, crush, chorus,
//!   compressor, duck, bus, limiter).
//! - [`presets`]: drum slots and GM drum notes, Ironfish programs, recipe text.
//! - [`shader`]: Splash audio shaders as instruments and FX.
//! - [`meters`]: what live displays read from whatever plays the graph.
//!
//! Nothing here allocates, locks or frees while rendering, except that a
//! neural singer's loop takes a short lock to look for a phrase that has not
//! arrived yet (never a wait on its render).

pub mod event;
pub mod fx;
pub mod loop_engine;
pub mod presets;
pub mod shader;
pub mod meters;
mod voice;

pub use event::{Env, Event, EventKind, NoteOpts, ARP_STEPS, ENV_POINTS};
pub use fx::FxKind;
pub use loop_engine::{Inst, KitSlot, LoopEngine, Sample, Tempo, MAX_LOOP_VOICES};
pub use makepad_score_voice as sung;
pub use meters::Meters;
pub use voice::pan_gains;

/// The most frames one render call takes (the engine's pass).
pub const CHUNK: usize = 256;
