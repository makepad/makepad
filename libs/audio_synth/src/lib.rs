//! Makepad's shared realtime synthesis crate.
//!
//! One DSP vocabulary for every app that makes sound from code: Stage's
//! Ironfish and its filters, the games' sound effects and vehicle engines,
//! and the instruments (piano, drums, SoundFont) a game or a score can play.
//!
//! Layers, bottom up:
//! - [`dsp`], [`delay`], [`svf`]: oscillators (PolyBLEP), ADSR, SVF/ladder
//!   filters, noise colours, delay lines, combs and all-passes.
//! - [`reverb`], [`spatial`], [`dynamics`]: rooms, 3D placement (distance,
//!   air absorption, ITD/ILD, doppler, occlusion) and the master bus.
//! - [`recipe`]: a small modular sound-recipe format (layers of oscillator/
//!   noise/FM/pluck through filter and envelope) with a built-in library,
//!   writable as text or built field by field from a script object.
//! - [`engine`], [`vehicle`]: firing-pulse engine synthesis (cylinders,
//!   firing order, exhaust/intake resonances, turbo, crackle, shifts,
//!   starter), electric/turbine/rotor models, tyres, road and wind.
//! - [`instrument`], [`poly`]: the [`instrument::Instrument`] trait and a
//!   recipe poly-synth (music is Stage's scores, `makepad-stage-score`).
//! - [`bus`]: the game mixer that owns all of the above on the audio
//!   thread with fixed voice pools and graceful stealing.
//!
//! Nothing in the render path allocates, locks or does I/O.

pub mod bus;
pub mod delay;
pub mod dsp;
pub mod dynamics;
pub mod engine;
pub mod instrument;
#[cfg(feature = "instruments")]
pub mod instruments;
pub mod ironfish;
pub mod poly;
pub mod recipe;
pub mod reverb;
pub mod spatial;
pub mod svf;
pub mod vehicle;
pub mod wav;

pub use instrument::{Control, Instrument};
