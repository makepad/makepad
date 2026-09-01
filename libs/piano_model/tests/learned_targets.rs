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
// The report test prints, per key across the compass:
//   - ladder deltas (physical minus learned) at onset / 100 ms / 300 ms
//   - the three diagnostic aggregates behind the standing listener
//     complaints: high-partial excess (rasp), fundamental+octave balance
//     (thin low end / body), and mid-ladder tilt
//   - per-partial decay-rate comparisons over the first second
//
//   cargo test -p makepad-piano-model --release --test learned_targets \
//     -- --ignored report --nocapture
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
    /// (freq, mag) per partial 1..=n at each analysis time.
    on: Vec<f64>,
    t100: Vec<f64>,
    t300: Vec<f64>,
    /// decay sigma (1/s) per partial, fitted 0.1..1.0 s.
    sigma: Vec<f64>,
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
    let mut mags_on = Vec::new();
    for n in 1..=n_partials {
        let guess = n as f64 * f0;
        let half = (0.45 * f0).max(6.0);
        let (f, m) = peak_near(onset, guess, half);
        freqs.push(f);
        mags_on.push(m);
    }
    let at = |t0: f64, freqs: &[f64]| -> Vec<f64> {
        let s = slice(t0);
        freqs.iter().map(|&f| dft_mag(s, f)).collect()
    };
    let t100 = at(0.1, &freqs);
    let t300 = at(0.3, &freqs);
    let sigma: Vec<f64> = freqs.iter().map(|&f| decay_sigma(x, f, 0.1, 1.0)).collect();
    let rel = |v: Vec<f64>| -> Vec<f64> {
        let m = v.iter().cloned().fold(1e-30, f64::max);
        v.iter().map(|&x| 20.0 * (x / m).max(1e-9).log10()).collect()
    };
    Ladder { on: rel(mags_on), t100: rel(t100), t300: rel(t300), sigma }
}

fn n_partials_for(f0: f64) -> usize {
    ((0.42 * FS as f64 / f0) as usize).clamp(3, 20)
}

/// The full report across the compass. The most valuable output of this
/// file: read the aggregates, then the per-partial rows.
#[test]
#[ignore]
fn report_physical_vs_learned_targets() {
    let vel = 112u8;
    let keys: Vec<u8> = (21..=108).step_by(3).collect();
    println!();
    println!("physical vs learned targets, velocity {vel} (dB, physical minus learned; + = physical too strong)");
    println!("aggregates per key:  hi = mean delta partials>=7 at 100ms (rasp axis)");
    println!("                     lo = mean delta partials 1-2 at 100ms (body/low-end axis)");
    println!("                     mid = mean delta partials 3-6 at 100ms");
    println!("                     dsig = mean (phys_sigma - learned_sigma) partials 1..6, 1/s (decay axis)");
    println!();
    let mut agg_hi = Vec::new();
    let mut agg_lo = Vec::new();
    let mut agg_mid = Vec::new();
    for &key in &keys {
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
        let don: Vec<f64> = lp.on.iter().zip(&ll.on).map(|(a, b)| a - b).collect();
        let d300: Vec<f64> = lp.t300.iter().zip(&ll.t300).map(|(a, b)| a - b).collect();
        let mean = |v: &[f64], a: usize, b: usize| -> f64 {
            let s = &v[a.min(v.len())..b.min(v.len())];
            if s.is_empty() {
                return 0.0;
            }
            s.iter().sum::<f64>() / s.len() as f64
        };
        let hi = mean(&d100, 6, np);
        let lo = mean(&d100, 0, 2);
        let mid = mean(&d100, 2, 6);
        let ds: Vec<f64> = lp.sigma.iter().zip(&ll.sigma).take(6).map(|(a, b)| a - b).collect();
        let dsig = ds.iter().sum::<f64>() / ds.len().max(1) as f64;
        agg_hi.push(hi);
        agg_lo.push(lo);
        agg_mid.push(mid);
        println!("key {key:>3} (f0 {f0p:6.1}):  hi {hi:+6.1}  lo {lo:+6.1}  mid {mid:+6.1}  dsig {dsig:+7.2}");
        print!("    d_on  ");
        for d in don.iter().take(12) {
            print!("{d:+6.1}");
        }
        println!();
        print!("    d_100 ");
        for d in d100.iter().take(12) {
            print!("{d:+6.1}");
        }
        println!();
        print!("    d_300 ");
        for d in d300.iter().take(12) {
            print!("{d:+6.1}");
        }
        println!();
        print!("    sig_p ");
        for s in lp.sigma.iter().take(8) {
            print!("{s:+7.2}");
        }
        println!();
        print!("    sig_l ");
        for s in ll.sigma.iter().take(8) {
            print!("{s:+7.2}");
        }
        println!();
    }
    let m = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    println!();
    println!(
        "COMPASS MEANS: hi {:+.1} dB, lo {:+.1} dB, mid {:+.1} dB (physical minus learned, 100 ms)",
        m(&agg_hi),
        m(&agg_lo),
        m(&agg_mid)
    );
}

/// The velocity axis the FluidR3 set cannot provide at all: how the
/// learned ladder brightens from pp to ff per register, versus the
/// physical model's felt nonlinearity.
#[test]
#[ignore]
fn report_velocity_axis() {
    println!();
    println!("spectral tilt vs velocity (mean ladder level of partials 4..10 rel partial 1, dB, 100 ms)");
    for &key in &[36u8, 60, 84] {
        print!("key {key:>3}: ");
        for &vel in &[30u8, 60, 90, 120] {
            let mut phys = dry_piano();
            let f0p = phys.key_info(key).unwrap().f0 as f64;
            let xp = render_note(&mut phys, key, vel, 0.8);
            let mut lrn = dry_learned();
            let xl = render_note(&mut lrn, key, vel, 0.8);
            let np = n_partials_for(f0p);
            let lp = measure(&xp, f0p, np);
            let ll = measure(&xl, makepad_piano_model::learned::forte_f0(key), np);
            let tilt = |l: &Ladder| -> f64 {
                let hi: Vec<f64> = l.t100.iter().skip(3).take(7).cloned().collect();
                let base = l.t100[0];
                hi.iter().sum::<f64>() / hi.len().max(1) as f64 - base
            };
            print!(" v{vel}: phys {:+6.1} lrn {:+6.1} |", tilt(&lp), tilt(&ll));
        }
        println!();
    }
}

// ---------------------------------------------------------------------------
// The trend gate — GENERATED anchors. To regenerate after an intentional
// physics change: run
//
//   cargo test -p makepad-piano-model --release --test learned_targets \
//     -- --ignored report --nocapture
//
// and copy each gated key's printed (hi, lo, mid) into ANCHORS below,
// keeping (and updating) the evidence comments for any anchor that
// records a deliberate divergence from the learned trend.
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
    const ANCHORS: &[(u8, f64, f64, f64)] = &[
        (30, -0.7, 18.3, 8.2),
        (36, 7.4, 10.1, 4.4),
        (48, 3.4, -0.3, 4.0),
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
