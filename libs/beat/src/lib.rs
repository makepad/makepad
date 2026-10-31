//! Beat and tempo detection: one detector, two modes.
//!
//! * [`onset`] — the one front end. Four filter lanes, log-compressed flux
//!   at a 10 ms hop, adaptive novelty. Both modes read it, sample for sample.
//! * [`tempo`] — the metrical decisions both modes share: the 60–200 BPM
//!   range, the musical preference, a known tempo's say, and the octave rule.
//! * [`realtime`] — the streaming mode: [`BeatSyncAnalyzer`] (allocation-free,
//!   for a worker or audio-adjacent thread), the published [`BeatClock`]
//!   (a disciplined oscillator whose position never jumps) and [`TapTempo`].
//! * [`live`] — [`LiveTempo`], the streaming tracker steering a clock, for a
//!   caller that just wants the beat now.
//! * [`offline`] — the whole-file mode: the same front end over a whole
//!   record, then comb refinement, a least-squares refit, a dynamic-program
//!   tempo map, downbeats and a confidence. For DJ decks and bakers, where
//!   the whole record is available and a stable grid beats a quick one.
//!
//! Both modes accept a PRIOR — a known tempo from a tag, a library, or (for
//! the realtime mode) a whole-file pass over the same record — which decides
//! the octave and speeds up the lock, and is never published unmeasured.
//!
//! No dependencies. An external beat tracker (a neural model) can verify the
//! offline result through [`offline::BeatEventSource`], implemented by the
//! caller.

pub mod live;
pub mod offline;
pub mod onset;
pub mod realtime;
pub mod tempo;

#[cfg(any(test, feature = "eval"))]
pub mod eval;

pub use live::LiveTempo;
pub use offline::{
    analyze, analyze_with, BeatAnalysis, BeatEventSource, BeatEvents, OfflineOptions, TempoMap,
    TempoSegment, TrackGrid,
};
pub use realtime::{
    BeatClock, BeatLockState, BeatSnapshot, BeatSyncAnalyzer, BeatTarget, ClockSink, ClockSource,
    Published, SnapshotCell, TapClock, TapTempo,
};
pub use tempo::{MAX_BPM, MIN_BPM};
