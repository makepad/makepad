//! A GSAP-style tween and timeline engine, rebuilt for Rust.
//!
//! The mental model is GSAP 3's: tweens move properties of targets over time
//! along an ease, timelines place tweens (and other timelines) at positions,
//! staggers spread one tween over many targets, and playback is controlled with
//! play / pause / reverse / seek / time scale. The data is plain `Copy` values
//! and the per-frame path never allocates.
//!
//! This crate has no dependencies. Values are written into a slot store that a
//! host (for example the Makepad widgets adapter) reads and pushes to its own
//! objects.
//!
//! The leaf modules re-exported here:
//! - [`ids`]: handles and keys (targets, properties, tags, tween handles).
//! - [`value`]: animatable values and colour-space maths.
//! - [`easing`]: every ease (GSAP named eases, CSS cubic-bezier and steps,
//!   and the Makepad `Ease` parity family).
//! - [`spec`]: the plain data a tween or timeline is built from (GSAP vars).
//! - [`stagger`]: GSAP `utils.distribute` / `stagger`.
//! - [`event`]: callback events (GSAP onStart, onComplete, ...).
//! - [`quick`]: `QuickTo`, the engine-free retargetable tween (GSAP `quickTo`).
//! - [`ticker`]: the app-wide clock policy (GSAP `ticker`, lag smoothing).
//! - [`sequencer_model`]: the data behind a timeline editor: a timeline as
//!   tracks of bars ([`SequencerModel::from_engine`]), its edits mapped back
//!   onto the timeline ([`apply_sequencer_edit`]) and the drags' snapping.
//! - [`path`]: motion paths (GSAP `MotionPathPlugin`): SVG path data, curves
//!   through points, an arc-length table, allocation-free sampling. A tween
//!   follows one with [`PropTo::path`] after [`TweenEngine::add_path`].
//!
//! The engine, [`TweenEngine`], is GSAP's global timeline: build tweens and
//! timelines on it, control them through [`AnimMut`] / [`TimelineMut`],
//! inspect them through [`AnimRef`] (whose [`AnimRef::kind`],
//! [`AnimRef::children`] and [`AnimRef::labels`] walk a timeline's tree, as
//! a timeline editor does), step it once per frame with
//! [`TweenEngine::advance`] and drain callbacks with
//! [`TweenEngine::swap_events`].
//!
//! # Example
//!
//! A widget-style entrance and exit: five items fade and slide in one after
//! another, then fade out from a label half a second after the entrance.
//! The host seeds the current values, builds once, then per frame calls
//! `advance`, drains the events and pushes the changed slots to its objects.
//!
//! ```
//! use makepad_tween::*;
//!
//! const OPACITY: PropKey = PropKey(1);
//! const SHIFT: PropKey = PropKey(2);
//! const INTRO: Tag = Tag(1);
//! const OUTRO: Tag = Tag(2);
//! const ITEMS: Targets<'static> = Targets::Range { first: 0, count: 5 };
//!
//! let mut e = TweenEngine::new();
//! for i in 0..5 {
//!     // GSAP reads a target's current value when a tween starts; the host
//!     // seeds it.
//!     e.seed(TargetId(i), OPACITY, 0.0.into());
//!     e.seed(TargetId(i), SHIFT, 20.0.into());
//! }
//! // gsap.timeline() with label events and onComplete.
//! let tl = e.timeline(TimelineOpts::new().watch_labels().on_complete());
//! e.tl(tl)
//!     .add_label(INTRO, 0.0)
//!     .to(
//!         ITEMS,
//!         &[PropTo::to_f64(OPACITY, 1.0), PropTo::to_f64(SHIFT, 0.0)],
//!         TweenOpts::new().duration(0.4).stagger(Stagger::each(0.1)),
//!         INTRO,
//!     )
//!     .add_label(OUTRO, Position::rel(0.5)) // "+=0.5"
//!     .to(
//!         ITEMS,
//!         &[PropTo::to_f64(OPACITY, 0.0)],
//!         TweenOpts::new().duration(0.3).ease(Easing::InQuad),
//!         OUTRO,
//!     );
//! assert_eq!(e.anim_ref(tl).label_time(OUTRO), Some(1.3));
//!
//! // The frame loop (a host drives this from its frame event).
//! let mut events = Vec::new();
//! let (mut saw_outro, mut done) = (false, false);
//! while e.is_active() {
//!     e.advance(1.0 / 64.0);
//!     e.swap_events(&mut events);
//!     for ev in &events {
//!         match ev.kind {
//!             EventKind::Label(OUTRO) => saw_outro = true,
//!             EventKind::Complete if ev.id == tl => done = true,
//!             _ => {}
//!         }
//!     }
//!     for &slot in e.changes() {
//!         let (_target, _prop) = e.slot_key(slot);
//!         let _value = e.value(slot); // push to the widget here
//!     }
//!     e.clear_changes();
//! }
//! assert!(saw_outro && done);
//! assert_eq!(e.get_f64(TargetId(4), OPACITY), Some(0.0));
//! assert_eq!(e.get_f64(TargetId(4), SHIFT), Some(0.0));
//!
//! // Timelines are kept: replay it.
//! e.anim(tl).restart(false, Emit::Suppress);
//! assert!(e.is_active());
//! ```
//!
//! # Allocation
//!
//! Only building calls allocate (`to`, `timeline`, `tl(..).to(..)`, `seed`,
//! `add_label`, ...). `advance`, every control, every getter, kills,
//! overwrites, reclamation and track compaction are allocation-free, provided
//! the event queue is drained with [`TweenEngine::swap_events`] (or
//! [`TweenEngine::clear_events`]) between frames. Motion paths follow the
//! same rule: [`TweenEngine::add_path`] and [`TweenEngine::release_path`]
//! are building calls, and a path freed inside a frame keeps its geometry
//! until the next building call drops it (no deallocation per frame either).
//!
//! # Deviations from GSAP 3.15
//!
//! Callbacks are queued events drained after the frame; `time(t)` on a
//! repeating animation stays in the current iteration; there is no lazy
//! first render; a paused timeline whose child starts before 0 keeps a
//! finite start; `yoyo_ease` is a pure function of the local time; values
//! are not rounded to 1e-6; zero-duration nodes ignore `repeat`; a killed
//! or completed (not kept, not paused) animation's handle goes stale; the
//! root clock rests at 0 while nothing is on it; non-finite numbers
//! count as unset (options), 0 (the root time scale) or are ignored
//! (controls).
#![forbid(unsafe_code)]

mod control;
pub mod easing;
mod engine;
pub mod event;
pub mod ids;
mod inspect;
mod overwrite;
pub mod path;
pub mod quick;
pub mod sequencer_model;
mod render;
pub mod spec;
pub mod stagger;
pub mod ticker;
pub mod value;

pub use control::{AnimKind, AnimMut, AnimRef, ChildIter, LabelIter, TimelineMut};
pub use easing::*;
pub use engine::TweenEngine;
pub use event::*;
pub use ids::*;
pub use inspect::{InspectKind, InspectNode};
pub use path::*;
pub use quick::*;
pub use sequencer_model::*;
pub use spec::*;
pub use stagger::*;
pub use ticker::*;
pub use value::*;

/// Total duration of an animation that repeats forever (GSAP `repeat: -1`
/// reports `totalDuration() == 1e10`, not infinity).
pub const INFINITE: f64 = 1e10;

/// GSAP `_bigNum`: a duration at or above this counts as effectively infinite
/// (a timeline holding a `repeat: -1` child clips default positions to its
/// most recent child). [`round7`] leaves values this large untouched.
pub const BIG: f64 = 1e8;

/// GSAP `_tinyNum`: the "just before / just after" distance used for end
/// snapping and zero-duration direction bookkeeping.
pub const TINY: f64 = 1e-8;

/// GSAP `_roundPrecise`: rounds to 1e-7, the precision GSAP keeps for start
/// times, durations, repeating local time and stagger delays.
///
/// Ties round like JavaScript's `Math.round` (toward +infinity, so
/// `round7(-2.5e-7) == -2e-7`, where `f64::round` would give `-3e-7`) and a
/// zero result is `+0.0` (GSAP's `|| 0`). Identity for `|x| >= BIG`, so
/// [`INFINITE`] stays exact.
#[inline]
pub fn round7(x: f64) -> f64 {
    if x.abs() < BIG {
        let y = x * 1e7;
        let f = floor_small(y);
        let r = (if y - f >= 0.5 { f + 1.0 } else { f }) / 1e7;
        if r == 0.0 {
            0.0
        } else {
            r
        }
    } else {
        x
    }
}

/// GSAP `_animationCycle`: the 0-based iteration that total time `tt` falls in
/// for an iteration cycle of `cycle` seconds (duration plus repeat delay).
///
/// An exact cycle boundary belongs to the iteration that is ending, so
/// `animation_cycle(1.0, 1.0) == 0` and `animation_cycle(1.25, 1.0) == 1`.
/// Negative and NaN results saturate to 0.
#[inline]
pub fn animation_cycle(tt: f64, cycle: f64) -> u32 {
    let q = round7(tt / cycle);
    let w = floor_small(q);
    let w = if q != 0.0 && w == q { w - 1.0 } else { w };
    // `as` saturates for floats: negative and NaN become 0, huge becomes u32::MAX.
    w as u32
}

/// `x` when finite, else `d`: non-finite numeric inputs never reach the
/// clock (a NaN start or time would stall or poison every render).
#[inline]
pub(crate) fn finite_or(x: f64, d: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        d
    }
}

/// `x.floor()` for `|x| < 2^62` (exact there) without a libm call: the
/// integer conversion truncates toward zero, one compare fixes negatives.
/// NaN and larger values fall back to `f64::floor`.
#[inline]
pub(crate) fn floor_small(x: f64) -> f64 {
    if x.abs() < 4.611_686_018_427_388e18 {
        let t = x as i64 as f64;
        if t > x {
            t - 1.0
        } else {
            t
        }
    } else {
        x.floor()
    }
}

/// The SplitMix64 mixing function: a fast, well-distributed 64-bit hash.
///
/// Used to fold multi-part property paths into one [`PropKey`] and to seed the
/// deterministic `from: "random"` stagger shuffle (GSAP uses `Math.random`).
pub const fn splitmix64(x: u64) -> u64 {
    let z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
