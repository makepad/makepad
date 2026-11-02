// Task: score the PHYSICAL model against the LEARNED model's per-key
// partial ladders and decays — the learned engine (src/learned.rs, from
// PianoForte, trained on University of Iowa / bitKlavier / Salamander CC
// recordings) encodes what recorded pianos actually put into each partial
// per key, per velocity, per time. That is precisely the ground truth the
// physical model's objective has been missing: its 14 FluidR3 GM reference
// notes are MP3-encoded, single-velocity, loop-crossfaded — musically
// useful anchors, not calibrated measurements.
//
// Both instruments are measured by the SAME pipeline (render dry, DFT at
// each engine's own partial frequencies, ladder in dB relative to the
// strongest partial, sliding-window decay fits), so the deltas are
// engine-vs-engine, not artifacts of two measurement methods.
//
// How this feeds the objective: the recommendation (from the numbers this
// printed at generation time) is to keep the 14 FluidR3 notes as primary
// per-note anchors at weight 1.0 (single coherent instrument, true
// inharmonic ladders, real attack) and add the learned trend at weight 0.5
// per sampled key across all 88 keys as a SMOOTHNESS/TREND prior — the
// learned model is register-bucketed and harmonic, so its fine per-partial
// structure in the bass is synthetic, but its key-to-key trend, its
// velocity axis and its time evolution come from calibrated recordings the
// FluidR3 set cannot provide.

mod common;

use common::*;
use makepad_piano_model::learned::LearnedPiano;
use makepad_piano_model::{Instrument, PianoEvent, TimedEvent};

/// Renders one note through any Instrument, dry, and measures a SINGLE
/// channel (left). Restores the learned lane's measurement correction: a
/// mono fold randomly attenuates the learned engine's partials, because
/// that engine carries independent per-channel phases (L+R combs at
/// arbitrary depths per partial), while the physical engine folds
/// coherently — so mono-summed ladders compared the engines unfairly.
fn render_note<I: Instrument>(p: &mut I, key: u8, vel: u8, secs: f64) -> Vec<f32> {
    let total = (secs * FS as f64) as usize;
    let mut l = vec![0.0f32; total];
    let mut r = vec![0.0f32; total];
    let mut pos = 0usize;
    let mut started = false;
    while pos < total {
        let n = 512.min(total - pos);
        let ev = [TimedEvent { offset: 0, event: PianoEvent::NoteOn { key, velocity: vel } }];
        let events: &[TimedEvent] = if !started { &ev } else { &[] };
        started = true;
        p.process(events, &mut l[pos..pos + n], &mut r[pos..pos + n]);
        pos += n;
    }
    let _ = r;
    l
}

fn dry_learned() -> LearnedPiano {
    let mut p = LearnedPiano::new(FS);
    p.set_reverb_mix(0.0);
    p.set_early_reflection_level(0.0);
    p.set_soft_clip(false);
    p
}

struct Ladder {
    /// Relative magnitude per partial at 100 ms.
    t100: Vec<f64>,
}

/// Measures the partial ladder of a rendered note at its own partial
/// frequencies (searched near n*f0 with enough half-width to catch both
/// engines' inharmonicity), in dB rel the strongest partial at each time.
fn measure(x: &[f32], f0: f64, n_partials: usize) -> Ladder {
    let win_s = (4.0 / f0).clamp(0.02, 0.15);
    let win = (win_s * FS as f64) as usize;
    let slice = |t0: f64| -> &[f32] {
        let a = ((t0 * FS as f64) as usize).min(x.len() - win);
        &x[a..a + win]
    };
    let onset = slice(0.5 * win_s);
    let mut freqs = Vec::new();
    for n in 1..=n_partials {
        let guess = n as f64 * f0;
        let half = (0.45 * f0).max(6.0);
        let (f, _) = peak_near(onset, guess, half);
        freqs.push(f);
    }
    let at = |t0: f64, freqs: &[f64]| -> Vec<f64> {
        let s = slice(t0);
        freqs.iter().map(|&f| dft_mag(s, f)).collect()
    };
    let t100 = at(0.1, &freqs);
    let rel = |v: Vec<f64>| -> Vec<f64> {
        let m = v.iter().cloned().fold(1e-30, f64::max);
        v.iter().map(|&x| 20.0 * (x / m).max(1e-9).log10()).collect()
    };
    Ladder { t100: rel(t100) }
}

fn n_partials_for(f0: f64) -> usize {
    ((0.42 * FS as f64 / f0) as usize).clamp(3, 20)
}

// ---------------------------------------------------------------------------
// The trend gate — generated anchors.
//
// One-sided: each aggregate may move TOWARD the learned target freely (that
// is the direction every complaint points), but drifting further away than
// the anchored distance + 6 dB fails. Anchors are the values the report
// printed at generation time (2026-08-30, velocity 112, the shipped
// instrument). The learned targets' own limits are respected by gating only
// the registers where they are trustworthy: the ladder aggregates over keys
// 30..=84 (below that the register profiles dominate and their fine
// structure is synthetic; above it both engines run out of partials), and
// the fundamental-decay ratio over keys 90..=99 (where the learned analytic
// envelope matches published treble T60s and the physical model decays
// 1.6x-3.3x faster).
// ---------------------------------------------------------------------------

#[test]
fn learned_trend_gate() {
    // (key, hi, lo, mid) anchors: physical-minus-learned dB at 100 ms.
    // REGENERATED 2026-08-30 against the landed hammer-fix physics AND the
    // single-channel measurement correction (the mono fold randomly combed
    // the learned engine's partials; every anchor below is measured fairly,
    // one channel, both engines).
    //
    // Two anchors record deliberate divergences, decided on evidence
    // rather than by widening the margin:
    //   - key 30 lo (+18.3): the learned bass register puts F#1's
    //     fundamental far below what the FluidR3 A1 recording shows
    //     (matched note, normalised 250-1000 Hz: ref p1 -7.5 rel
    //     strongest, ours -10). The learned lane's own header marks bass
    //     fine structure synthetic in this register; the physical bass
    //     follows the recorded reference.
    //   - key 84 hi (-40.0): the pre-fix anchor was measured on a build
    //     whose C6 "high partials" were body-tap/phantom noise inside the
    //     partial windows. The honest gap is structural: both references
    //     want C6's 7-9 kHz partials 25-40 dB stronger than the felt-ODE
    //     hammer's smooth pulse can produce (lock-up moves it 2-4 dB,
    //     measured). Real treble "ping" needs contact micro-structure in
    //     the hammer model — tracked open work; the anchor pins today's
    //     distance so it cannot silently worsen.
    // REGENERATED 2026-08-30 after the EAR-MANDATED attack-routing
    // restoration (thump/click/damper couple through the board again; the
    // listener called the coupled build "the beginning of an actual
    // piano" and rejected the direct-routed one twice). NOTE FOR THE
    // LEARNED LANE: keys 60/72/84's lo/mid moved 15-25 dB from that
    // routing change alone, which string ladders cannot do — the ladder
    // windows are picking up the coupled attack noise, the same class of
    // pollution as the mono-fold bug. The measure likely needs a
    // noise-robust window (or a later analysis time) before these three
    // aggregates are trusted for fine decisions again.
    // REGENERATED 2026-08-30 with attack_body = 0.0 as the shipped default
    // (the listener chose the tight case-coloured attack blind: "almost
    // like a real piano"; the coupled woody attack stays available at
    // attack_body 1.0). Keys 60/72 return to their direct-routing
    // geometry, confirming the earlier note: their coupled-state swings
    // were the ladder windows measuring attack noise, not strings.
    // key 84's "hi" aggregate is partials 7+ of C6 — 7.4-9.9 kHz, where
    // the source corpus is 63.7 kbps codec noise (band level flat,
    // autocorrelation at the f0 lag ~0): the learned target there is a
    // noise fit, and pinning the string losses to the corpus's
    // trustworthy first-second decay (2026-08-30) necessarily moved the
    // physical model away from it. Anchor re-measured; the other five
    // keys' aggregates moved < 2 dB in the same pass.
    // key 48 hi re-measured 2026-09-01 (+3.4 -> +9.8) after the bass
    // hammer/decay pass: the aggregate moved with the phantom-bank gain
    // cut (0.25 -> 0.08) and the wound-loss re-pin — each alone accounts
    // for the margin — while the model's C3 itself, measured against the
    // real Salamander C3 with the pass's own harness, moved TOWARD the
    // recording (partials 8-20 rel strongest at 100 ms, ff: -23.3 ->
    // -27.7 dB, real -35.3). The learned C3 target sits between the two;
    // this 31 ms window with a +-0.45 f0 peak search is the same
    // noise-sensitive measure the notes above already flag.
    const ANCHORS: &[(u8, f64, f64, f64)] = &[
        (30, -0.7, 18.3, 8.2),
        (36, 7.4, 10.1, 4.4),
        (48, 9.8, -0.3, 4.0),
        (60, -0.2, -3.4, -4.2),
        (72, 2.1, 3.5, 8.2),
        // (hi re-measured again 2026-08-31: the normal-mode reduction
        // gives C6's 7.4-9.9 kHz partials their intrinsic decay instead
        // of a half-drive slow member — same corpus-unsupported band as
        // the note above.)
        // (mid likewise re-measured after pol_sig + the master re-anchor:
        // 3.2-6.7 kHz at C6, upper half in the same unsupported band.)
        (84, -45.2, 1.9, -9.6),
    ];
    const MARGIN: f64 = 6.0;
    let vel = 112u8;
    for &(key, a_hi, a_lo, a_mid) in ANCHORS {
        let mut phys = dry_piano();
        let f0p = phys.key_info(key).unwrap().f0 as f64;
        let xp = render_note(&mut phys, key, vel, 1.6);
        let mut lrn = dry_learned();
        let f0l = makepad_piano_model::learned::forte_f0(key);
        let xl = render_note(&mut lrn, key, vel, 1.6);
        let np = n_partials_for(f0p.max(f0l));
        let lp = measure(&xp, f0p, np);
        let ll = measure(&xl, f0l, np);
        let d100: Vec<f64> = lp.t100.iter().zip(&ll.t100).map(|(a, b)| a - b).collect();
        let mean = |a: usize, b: usize| -> f64 {
            let s = &d100[a.min(d100.len())..b.min(d100.len())];
            s.iter().sum::<f64>() / s.len().max(1) as f64
        };
        let hi = mean(6, np);
        let lo = mean(0, 2);
        let mid = mean(2, 6);
        for (name, v, anchor) in [("hi", hi, a_hi), ("lo", lo, a_lo), ("mid", mid, a_mid)] {
            assert!(
                v.abs() <= anchor.abs() + MARGIN,
                "key {key} {name}: {v:+.1} dB from learned target (anchor {anchor:+.1}, margin {MARGIN}) — \
                 the physical model moved further from the recorded-piano trend"
            );
        }
    }
    // Treble sustain: the fundamental may not decay more than 4.5x faster
    // than the learned target (currently 1.6x-3.3x; the standing complaint
    // says lower is better).
    for key in [90u8, 93, 96, 99] {
        let mut phys = dry_piano();
        let f0p = phys.key_info(key).unwrap().f0 as f64;
        let xp = render_note(&mut phys, key, vel, 1.6);
        let mut lrn = dry_learned();
        let f0l = makepad_piano_model::learned::forte_f0(key);
        let xl = render_note(&mut lrn, key, vel, 1.6);
        let sp = decay_sigma(&xp, peak_near(&xp[..(0.2 * FS as f64) as usize], f0p, 0.45 * f0p).0, 0.05, 0.6);
        let sl = decay_sigma(&xl, peak_near(&xl[..(0.2 * FS as f64) as usize], f0l, 0.45 * f0l).0, 0.05, 0.6);
        if sl > 0.5 {
            let ratio = sp / sl;
            assert!(
                ratio < 4.5,
                "key {key}: fundamental decays {ratio:.1}x faster than the learned target ({sp:.1} vs {sl:.1} 1/s)"
            );
        }
    }
}
