//! Envelopes, LFOs, MSEG, step sequencer, controller smoothing (owner:
//! SYNTHPORT-MOD), per the hybrid family's modulation spec sections 3-10.
//!
//! Everything here runs once per block of [`BLOCK`](super::core::BLOCK)
//! host samples (the modulation clock); consumers interpolate inside the
//! block. The routing arithmetic and the voice/scene ordering belong to the
//! layout that owns the sources (the Tandem layout).
//!
//! - [`time`]: the envelope-rate time base, tempo sync, the range constants.
//! - [`adsr`]: the amp / filter ADSR (digital and analog modes, gated and
//!   fast release).
//! - [`lfo`]: the LFO (ten shapes), its DAHDSR, trigger modes, outputs and
//!   EG retrigger requests.
//! - [`stepseq`], [`mseg`]: the step-sequencer and MSEG data models (plain
//!   data the patch loader fills) and the MSEG evaluator.
//! - [`formula`]: the formula modulator as stored, its compile check and
//!   the output combination rules (the scripts run in `kit::formula`).
//! - [`smooth`]: the controller smoother.
//!
//! Nothing in a render path allocates, locks or does I/O.

pub mod adsr;
pub mod formula;
pub mod lfo;
pub mod mseg;
pub mod smooth;
pub mod stepseq;
pub mod time;

pub use adsr::{Adsr, AdsrParams};
pub use formula::{Formula, FormulaIo};
pub use lfo::{Lfo, LfoParams, LfoShape};
pub use mseg::{Mseg, MsegSegment, SegmentType};
pub use smooth::{ControllerSmoother, SmoothMode};
pub use stepseq::StepSeq;

use super::core::BLOCK;

/// What every modulator reads about the host each block.
#[derive(Clone, Copy, Debug)]
pub struct ModCtx {
    /// Host sample rate.
    pub fs: f32,
    /// Seconds per modulation block (BLOCK / fs).
    pub block_secs: f32,
    /// Tempo ratio: BPM / 120 (1 without a host tempo).
    pub tsr: f32,
    /// Host song position in beats.
    pub song_pos_beats: f64,
}

impl ModCtx {
    /// A context at `fs` with the standard block, 120 BPM, song position 0.
    pub fn new(fs: f32) -> ModCtx {
        ModCtx { fs, block_secs: BLOCK as f32 / fs, tsr: 1.0, song_pos_beats: 0.0 }
    }

    pub fn with_bpm(mut self, bpm: f32) -> ModCtx {
        self.tsr = bpm / 120.0;
        self
    }
}
