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
//! - [`stream`]: a keyboard's notes through a score's stream nodes (chord,
//!   transpose, velocity, arp) into a loop's instrument.
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
pub mod stream;
mod voice;

pub use event::{Env, Event, EventKind, NoteOpts, ARP_STEPS, ENV_POINTS};
pub use fx::FxKind;
pub use loop_engine::{Inst, KitSlot, LoopEngine, Sample, Tempo, MAX_LOOP_VOICES};
pub use makepad_score_voice as sung;

/// What a build without the neural singer says of a voice on it (`engine: @neural`): its model
/// runs in this process, so it comes only with an app's `localai` (AGENTS.md), and the hub has no
/// singer out of process to stand in for it.
pub const NEURAL_NEEDS_LOCALAI: &str = "@neural needs a `localai` build: the neural voice runs its model in this process; @dsp sings in every build";
pub use meters::Meters;
pub use voice::pan_gains;

/// The most frames one render call takes (the engine's pass).
pub const CHUNK: usize = 256;
