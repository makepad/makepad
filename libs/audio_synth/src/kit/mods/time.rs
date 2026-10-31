//! The modulators' shared time base (spec section 3): the per-block rate of
//! a stage lasting 2^x s, its tempo-synced form, the analog EG coefficient,
//! the LFO phase increment, and the raw ranges of the time / rate controls.
//!
//! A tempo-synced value keeps its log2 scale (seconds or Hz at 120 BPM);
//! the engine only scales by the tempo ratio. Snapping a synced value to
//! straight / dotted / triplet happens when it is set
//! ([`temposync_quantize`](crate::kit::core::temposync_quantize)), not here.

use super::ModCtx;
use crate::kit::core::envelope_rate_linear;

/// Envelope / LFO-EG stage times (log2 s): 3.9 ms .. 32 s.
pub const ENV_TIME_MIN: f32 = -8.0;
pub const ENV_TIME_MAX: f32 = 5.0;
/// LFO release at its maximum means "Forever" (no release).
pub const LFO_RELEASE_FOREVER: f32 = 5.0;
/// LFO rate (log2 Hz): 0.0078 .. 512 Hz.
pub const LFO_RATE_MIN: f32 = -7.0;
pub const LFO_RATE_MAX: f32 = 9.0;
/// A stage time this close to its minimum counts as "at the minimum" (skipped).
pub const AT_MIN_EPS: f32 = 0.01;

/// Whether a stage time sits at its minimum (the stage is skipped).
#[inline]
pub fn at_min(x: f32) -> bool {
    x < ENV_TIME_MIN + AT_MIN_EPS
}

/// R(x) with the tempo ratio applied when synced: the per-block phase
/// increment of a linear stage lasting 2^x s (2^x / tsr s when synced).
#[inline]
pub fn stage_rate(x: f32, synced: bool, ctx: &ModCtx) -> f32 {
    let r = envelope_rate_linear(x, ctx.block_secs);
    if synced { r * ctx.tsr } else { r }
}

/// The variant of R whose table index wraps modulo 512 instead of clamping
/// (the portamento form; differs only far outside parameter ranges).
pub fn envelope_rate_linear_wrap(x: f32, block_secs: f32) -> f32 {
    let q = 16.0 * x + 256.0;
    let f = q.floor();
    let a = q - f;
    let e0 = (f as i64).rem_euclid(512);
    let e1 = (e0 + 1) % 512;
    let t = |e: i64| block_secs * (-((e as f32) - 256.0) / 16.0).exp2();
    (1.0 - a) * t(e0) + a * t(e1)
}

/// Seconds a stage of raw time x lasts (synced: scaled by the tempo).
#[inline]
pub fn stage_seconds(x: f32, synced: bool, tsr: f32) -> f32 {
    if synced { x.exp2() / tsr } else { x.exp2() }
}

/// Tempo sync of an analog EG stage multiplies the log-time by the tempo
/// ratio instead of the rate; mirrored.
const QUIRK_ANALOG_SYNC_SCALES_LOG_TIME: bool = true;

/// The analog EG's per-block coefficient for a stage time x:
/// 2^min(0, 2 - log2(1/Tb) - x m) with m = tsr when synced (the tempo ratio
/// scales the log-time, a quirk kept for parity).
#[inline]
pub fn analog_coef(x: f32, synced: bool, ctx: &ModCtx) -> f32 {
    let e = if synced && !QUIRK_ANALOG_SYNC_SCALES_LOG_TIME {
        2.0 - (1.0 / ctx.block_secs).log2() - x - ctx.tsr.log2()
    } else {
        2.0 - (1.0 / ctx.block_secs).log2() - x * if synced { ctx.tsr } else { 1.0 }
    };
    e.min(0.0).exp2()
}

/// The LFO's per-block phase increment: R(-rate) unsynced (table), exactly
/// Tb 2^rate tsr synced, 0 when the rate is deactivated.
#[inline]
pub fn lfo_phase_inc(rate: f32, synced: bool, deactivated: bool, ctx: &ModCtx) -> f32 {
    if deactivated {
        0.0
    } else if synced {
        ctx.block_secs * rate.exp2() * ctx.tsr
    } else {
        envelope_rate_linear(-rate, ctx.block_secs)
    }
}

/// The LFO rate in Hz (the free-run phase derivation uses the exact form).
#[inline]
pub fn lfo_hz(rate: f32, synced: bool, deactivated: bool, tsr: f32) -> f64 {
    if deactivated {
        0.0
    } else if synced {
        (rate as f64).exp2() * tsr as f64
    } else {
        (rate as f64).exp2()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates() {
        let ctx = ModCtx::new(48000.0);
        for x in [-8.0f32, -3.25, 0.0, 2.5, 5.0] {
            let r = stage_rate(x, false, &ctx);
            assert!((r * x.exp2() / ctx.block_secs - 1.0).abs() < 2e-3, "{x}");
            assert!((envelope_rate_linear_wrap(x, ctx.block_secs) - r).abs() < 1e-9);
        }
        let ctx2 = ctx.with_bpm(240.0);
        assert!((stage_rate(0.0, true, &ctx2) - 2.0 * stage_rate(0.0, false, &ctx2)).abs() < 1e-9);
        assert!((lfo_phase_inc(2.0, true, false, &ctx) - 4.0 * ctx.block_secs).abs() < 1e-9);
        assert!((lfo_phase_inc(2.0, false, false, &ctx) - 4.0 * ctx.block_secs).abs() < 1e-7);
        assert_eq!(lfo_phase_inc(2.0, false, true, &ctx), 0.0);
        assert!(analog_coef(-20.0, false, &ctx) == 1.0);
    }
}
