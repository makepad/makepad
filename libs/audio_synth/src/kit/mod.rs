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
//! - [`core`]: the hybrid family's shared conventions (blocks, pitch/dB
//!   maths, amplitude and pan laws, slot values and definitions, the
//!   smoothed biquad, the half-band 2x sampler).
//! - [`flt`], [`shaper`]: the filter family (36 types with subtypes) and the
//!   waveshaper family (43 types).
//! - [`oscs`]: the hybrid oscillators (classic, sine, wavetable, window,
//!   S&H noise, audio in, FM2, FM3, modern, string, twist, alias).
//! - [`mods`]: envelopes (digital/analog), LFOs with step sequencer, MSEG
//!   and envelope shapes, controller smoothing.
//! - [`fxs`]: the effects family (one module per effect).
//! - [`formula`]: Formula LFO scripts compiled to AIR (no interpreter).
//!
//! Nothing in a render path allocates, locks or does I/O.

pub mod arp;
pub mod core;
pub mod flt;
pub mod formula;
pub mod fxs;
pub mod mods;
pub mod oscs;
pub mod shaper;
pub mod env;
pub mod filter;
pub mod fx;
pub mod lfo;
pub mod matrix;
pub mod osc;
pub mod schema;
pub mod table;
pub mod tuning;

/// tanh by a rational fit (within 0.3% for |x| < 3, saturating beyond).
#[inline]
pub fn fast_tanh(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    let x2 = x * x;
    x * (27.0 + x2) / (27.0 + 9.0 * x2)
}
