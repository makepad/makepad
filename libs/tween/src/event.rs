//! Callback events. GSAP runs `onStart`, `onComplete`, ... synchronously
//! inside its render; here the engine appends [`TweenEvent`]s to an ordered
//! queue that the host drains after each advance, so the core holds no
//! closures.

use crate::ids::{Tag, TweenId};

/// Which callback fired.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EventKind {
    /// GSAP `onStart`: the animation left its start moving forward.
    Start,
    /// GSAP `onUpdate`: the animation rendered.
    Update,
    /// GSAP `onRepeat`: an iteration boundary was crossed.
    Repeat,
    /// GSAP `onComplete`: the end was reached moving forward.
    Complete,
    /// GSAP `onReverseComplete`: the start was reached moving backward.
    ReverseComplete,
    /// GSAP `onInterrupt`: killed before completing.
    Interrupt,
    /// A timeline `call()` was crossed, in the given direction.
    Call {
        /// Whether the playhead was moving forward.
        forward: bool,
    },
    /// A timeline `addPause()` stopped the playhead.
    Pause,
    /// A watched label was crossed (not GSAP; see `EventMask::LABELS`).
    Label(Tag),
}

/// One fired callback, in firing order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TweenEvent {
    /// The animation it belongs to.
    pub id: TweenId,
    /// That animation's tag (GSAP `id`), or the call / label tag.
    pub tag: Tag,
    /// What happened.
    pub kind: EventKind,
    /// The animation's total time when it fired (for exact corrective seeks).
    pub total_time: f64,
    /// The animation's iteration when it fired, 1-based like GSAP `iteration()`.
    pub iteration: u32,
}

/// Engine counters, for diagnostics and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    /// Allocated animation nodes (live and free).
    pub nodes: u32,
    /// Live animation nodes.
    pub live_nodes: u32,
    /// Allocated property tracks.
    pub tracks: u32,
    /// Tracks of killed tweens awaiting compaction.
    pub dead_tracks: u32,
    /// Value slots.
    pub slots: u32,
    /// Tracks that started on a slot nobody seeded (they do not move).
    pub unseeded_starts: u64,
    /// Events dropped because the queue was full.
    pub event_overflows: u64,
    /// Track compactions run.
    pub compactions: u64,
    /// Live motion paths ([`crate::TweenEngine::add_path`]).
    pub paths: u32,
    /// Live path binds: one per (tween, target) that follows a path.
    pub path_binds: u32,
    /// `PropTo::path` props skipped because their path handle was stale.
    pub bad_paths: u64,
}
