//! The synth kit: one library of synthesis modules behind one parameter
//! schema, for every engine layout to compose (a layout is the list of
//! modules a patch has and how a voice wires them; [`crate::morph`] is the
//! first).
//!
//! - [`schema`]: parameters (key, label, group, range, curve, unit,
//!   choices, mod target, one line for the AI) and the builder modules
//!   declare into.
//! - [`table`]: band-limited wavetables, built-in tables, tables from
//!   partial lists and from samples.
//! - [`osc`]: wavetable / granular / spectral / sample oscillators with
//!   unison and two warps each; sub and noise oscillators.
//! - [`env`], [`lfo`]: envelopes with curves; LFOs with drawn shapes, chaos
//!   attractors and 2D paths.
//! - [`matrix`]: the mod matrix (curve, polarity, aux, output).
//! - [`arp`]: the arpeggiator.
//! - [`filter`], [`fx`]: the filter and effects SLOTS (the shared families
//!   arrive with the clean-room specs; for now this crate's own DSP).
//!
//! Nothing in a render path allocates, locks or does I/O.

pub mod arp;
pub mod env;
pub mod filter;
pub mod fx;
pub mod lfo;
pub mod matrix;
pub mod osc;
pub mod schema;
pub mod table;

/// tanh by a rational fit (within 0.3% for |x| < 3, saturating beyond).
#[inline]
pub fn fast_tanh(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    let x2 = x * x;
    x * (27.0 + x2) / (27.0 + 9.0 * x2)
}
