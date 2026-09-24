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
#![forbid(unsafe_code)]

pub mod easing;
pub mod event;
pub mod ids;
pub mod quick;
pub mod spec;
pub mod stagger;
pub mod ticker;
pub mod value;

pub use easing::*;
pub use event::*;
pub use ids::*;
pub use quick::*;
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
        let f = y.floor();
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
    let w = q.floor();
    let w = if q != 0.0 && w == q { w - 1.0 } else { w };
    // `as` saturates for floats: negative and NaN become 0, huge becomes u32::MAX.
    w as u32
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
