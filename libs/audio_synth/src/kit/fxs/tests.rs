//! Cross-type properties of the effects family: every type at its defaults
//! on a noise burst + sine gives finite, bounded output; a tail stops within
//! its ring-out length after the input goes silent (always-run types decay
//! on their own); slot definitions are self-consistent; migrations of every
//! revision run.

use super::*;
use crate::dsp::Rng;

const SR: f32 = 48000.0;

fn all_types() -> impl Iterator<Item = u8> {
    1u8..32
}

/// 0.25 s of noise at 0.5, then 0.5 s of a 440 Hz sine at 0.5.
fn excitation(rng: &mut Rng, n: usize) -> (f32, f32) {
    let burst = (0.25 * SR) as usize;
    let sine_end = (0.75 * SR) as usize;
    if n < burst {
        (0.5 * rng.bipolar(), 0.5 * rng.bipolar())
    } else if n < sine_end {
        let s = 0.5 * (std::f64::consts::TAU * 440.0 * n as f64 / SR as f64).sin() as f32;
        (s, s)
    } else {
        (0.0, 0.0)
    }
}

fn block_peak(l: &[f32; 32], r: &[f32; 32]) -> f32 {
    l.iter().chain(r.iter()).fold(0.0f32, |m, x| m.max(x.abs()))
}

#[test]
fn every_type_finite_and_rings_out() {
    let ctx = FxCtx::new(SR);
    let input_blocks = (0.75 * SR) as usize / 32 + 1;
    for ty in all_types() {
        let p = default_values(ty);
        let mut fx = Fx::new(ty, SR);
        assert_eq!(fx.ty(), ty);
        let mut rng = Rng::new(7);
        let mut peak = 0.0f32;
        let mut n = 0usize;
        for _ in 0..input_blocks {
            let mut l = [0f32; 32];
            let mut r = [0f32; 32];
            for k in 0..32 {
                let (a, b) = excitation(&mut rng, n);
                l[k] = a;
                r[k] = b;
                n += 1;
            }
            fx.process(&p, &ctx, &mut l, &mut r);
            assert!(l.iter().chain(r.iter()).all(|x| x.is_finite()), "{} not finite", TYPE_NAMES[ty as usize]);
            peak = peak.max(block_peak(&l, &r));
        }
        assert!(peak < 100.0, "{} peak {peak}", TYPE_NAMES[ty as usize]);
        let d = fx.ringout_blocks(&p, SR);
        // Ring-out types: the last blocks inside the ring-out are quiet; the
        // always-run types: 20 s of silence leave the last second quiet.
        let (run, tail) = if d >= 0 { (d as usize, (d as usize / 4).max(1)) } else { ((20.0 * SR) as usize / 32, (SR as usize) / 32) };
        let mut tail_peak = 0.0f32;
        for b in 0..run {
            let mut l = [0f32; 32];
            let mut r = [0f32; 32];
            fx.process(&p, &ctx, &mut l, &mut r);
            assert!(l.iter().chain(r.iter()).all(|x| x.is_finite()), "{} tail not finite", TYPE_NAMES[ty as usize]);
            if b + tail >= run {
                tail_peak = tail_peak.max(block_peak(&l, &r));
            }
        }
        let rel = if peak > 0.0 { tail_peak / peak } else { 0.0 };
        // The frequency shifter's band split sits at fs/4 where its steep
        // half-band rings well past the delay-only ring-out estimate the
        // reference uses (it cuts that ringing off).
        let limit = if ty == 7 { 0.15 } else { 1e-2 };
        assert!(rel < limit, "{}: tail {tail_peak} vs peak {peak} after {run} blocks (ringout {d})", TYPE_NAMES[ty as usize]);
        // Control-only blocks never panic and the effect resumes cleanly.
        for _ in 0..4 {
            fx.process_only_control(&p, &ctx);
        }
        let mut l = [0.1f32; 32];
        let mut r = [0.1f32; 32];
        fx.process(&p, &ctx, &mut l, &mut r);
        assert!(l.iter().chain(r.iter()).all(|x| x.is_finite()));
        fx.reset();
    }
}

#[test]
fn slot_defs_are_consistent() {
    for ty in 0u8..32 {
        let defs = slot_defs(ty);
        let p = default_values(ty);
        for (i, d) in defs.iter().enumerate() {
            if !d.used() {
                continue;
            }
            assert!(d.min <= d.max, "{} p{i}", TYPE_NAMES[ty as usize]);
            assert!(d.default >= d.min && d.default <= d.max, "{} p{i} default {} not in {}..{}", TYPE_NAMES[ty as usize], d.default, d.min, d.max);
            if !d.choices.is_empty() {
                assert!(d.int);
                assert_eq!(d.choices.len() as f32 - 1.0, d.max - d.min, "{} p{i}", TYPE_NAMES[ty as usize]);
            }
            if !d.deform.is_empty() {
                assert!(p[i].deform >= 0 && (p[i].deform as usize) < d.deform.len());
            }
            if p[i].deactivated {
                assert!(d.can_deactivate, "{} p{i}", TYPE_NAMES[ty as usize]);
            }
            assert_eq!(p[i].int, d.int);
        }
    }
}

#[test]
fn migrations_run_for_every_revision() {
    for ty in 0u8..32 {
        for rev in 0..=31 {
            let mut p = default_values(ty);
            migrate(ty, rev, &mut p);
            clamp_values(ty, &mut p);
            assert!(p.iter().all(|v| v.v.is_finite()));
        }
    }
}

