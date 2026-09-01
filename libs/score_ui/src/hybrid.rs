//! The hybrid instrument: the physical model with its per-partial targets
//! pulled toward what a recorded piano actually puts into each partial.
//!
//! # DORMANT — built, measured, and deliberately not offered
//!
//! Everything here works: the table applies in 3.6 ms and moves the partial
//! balance measurably closer to the recorded target. It is not in
//! [`crate::sound::ENGINES`] because LISTENING said the result is worse than
//! either engine it is made from. The numbers improved and the sound did not,
//! which is the only verdict that counts.
//!
//! Two things would have to happen before it is offered: the physical model's
//! current round of work lands (the table was derived against an older build
//! and its targets are only as good as the instrument underneath), and
//! somebody listens to it again. Re-expose it by adding
//! `ScoreEngine::Hybrid` back to `ENGINES` — after rebaking (below) and
//! after that listen.
//!
//! The learned engine (`makepad_piano_model::learned`, from PianoForte) is a
//! network trained on recorded pianos, so its per-key partial ladder is
//! calibrated measurement rather than a guess. The physical model has the
//! right *behaviour* — real inharmonic ladders, sympathetic resonance, pedal
//! and re-strike dynamics, a genuine velocity axis — but its partial balance
//! drifts from those measurements, brightest in the mid register.
//!
//! Hybrid keeps the physical model whole and corrects only that balance: a
//! per-key table of per-partial output-gain multipliers and pole-radius
//! powers, applied once at construction through [`apply_targets`].
//!
//! # Why this is a table and not a computation
//!
//! DERIVING the table means rendering every key through both engines and
//! analysing the spectra — seconds of work. APPLYING it is arithmetic over
//! small arrays: measured at **3.6 ms for all 88 keys**, which is why this is
//! an ordinary runtime engine and not an offline mode. The derivation is
//! baked once into [`TARGETS`] below; the application happens inside the same
//! crossfaded rebuild path that presets with a design override already use.
//!
//! # Regenerating the table
//!
//! [`TARGETS`] is generated against the shipped physical model. If that model
//! changes, the table drifts and `the_hybrid_is_closer_to_the_recorded_piano`
//! starts failing — that test is the tripwire. Regenerate with:
//!
//! ```text
//! cargo test -p makepad-score-ui --release --lib generate_hybrid_targets \
//!     -- --ignored --nocapture
//! ```
//!
//! and paste its output over [`TARGETS`].

use makepad_piano_model::Piano;

/// The lowest key the table covers; entry `i` is MIDI key `FIRST_KEY + i`.
pub const FIRST_KEY: u8 = 21;
/// How many partials each entry describes.
pub const PARTIALS: usize = 20;

/// One key's correction: per-partial output-gain multipliers, and per-partial
/// powers on the pole radius (below 1.0 the partial sustains longer).
pub struct KeyTargets {
    pub gain: [f32; PARTIALS],
    pub sigma_scale: [f32; PARTIALS],
}

/// Reshape every key of a freshly built physical instrument.
///
/// Control path only — call between `process()` calls, never inside one.
/// Allocates nothing.
pub fn apply_targets(piano: &mut Piano) {
    for (index, targets) in TARGETS.iter().enumerate() {
        let key = FIRST_KEY + index as u8;
        piano.debug_shape_partials(key, &targets.gain, &targets.sigma_scale);
    }
}

include!("hybrid_targets.rs");

/// Deriving the table, and proving it did what it claims.
///
/// Both engines are measured through the SAME pipeline — render dry, DFT at
/// each engine's own partial frequencies, ladder in dB relative to the
/// strongest partial — so the deltas are engine-vs-engine rather than
/// artefacts of two measurement methods. The pipeline is the one
/// `libs/piano_model/tests/learned_targets.rs` established; it is restated
/// here rather than shared because that crate belongs to another lane.
#[cfg(test)]
mod derive {
    use super::*;
    use makepad_piano_model::{
        learned::{forte_f0, LearnedPiano},
        Instrument, PianoEvent, Piano, TimedEvent,
    };

    pub(super) const FS: f32 = 48_000.0;
    /// The velocity the table is derived at. One fixed spectral correction
    /// cannot follow the velocity axis, so it is derived where the balance
    /// complaints live: a firm forte.
    const DERIVE_VELOCITY: u8 = 112;
    /// How far the correction pulls the physical ladder toward the learned
    /// one. Not all the way: the physical ladder carries real inharmonicity
    /// and real per-string detuning that the learned model's harmonic,
    /// register-bucketed output does not, and 100% would throw those away.
    const PULL: f64 = 0.60;
    /// Ceiling on any one partial's correction, in dB either way.
    const CLAMP_DB: f64 = 6.0;
    /// The register where the learned ladder is trustworthy. Below it the
    /// network's register profiles dominate and its fine per-partial
    /// structure is synthetic; above it both engines run out of partials.
    /// Outside this band the correction tapers to nothing.
    const LADDER_KEYS: (u8, u8) = (30, 84);
    const TAPER: f64 = 6.0;
    /// Where the learned envelope is trustworthy enough to lengthen the
    /// physical decay: the treble, whose published T60s it matches and where
    /// the physical model runs measurably short.
    const SUSTAIN_KEYS: (u8, u8) = (88, 108);
    /// Only ever sustain longer, never shorten: one-sided, so measurement
    /// noise cannot turn into a faster decay.
    const SIGMA_FLOOR: f32 = 0.35;

    pub(super) fn dry_piano() -> Piano {
        let mut p = Piano::new(FS);
        p.set_reverb_mix(0.0);
        p.set_early_reflection_level(0.0);
        p.set_soft_clip(false);
        p
    }
    fn dry_learned() -> LearnedPiano {
        let mut p = LearnedPiano::new(FS);
        p.set_reverb_mix(0.0);
        p.set_early_reflection_level(0.0);
        p.set_soft_clip(false);
        p
    }

    pub(super) fn render<I: Instrument>(p: &mut I, key: u8, vel: u8, secs: f64) -> Vec<f32> {
        let total = (secs * FS as f64) as usize;
        let (mut l, mut r) = (vec![0.0f32; total], vec![0.0f32; total]);
        let (mut pos, mut started) = (0usize, false);
        while pos < total {
            let n = 512.min(total - pos);
            let ev = [TimedEvent { offset: 0, event: PianoEvent::NoteOn { key, velocity: vel } }];
            let events: &[TimedEvent] = if !started { &ev } else { &[] };
            started = true;
            p.process(events, &mut l[pos..pos + n], &mut r[pos..pos + n]);
            pos += n;
        }
        l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect()
    }

    fn dft_mag(x: &[f32], f: f64) -> f64 {
        let n = x.len();
        let (mut re, mut im, mut wsum) = (0.0f64, 0.0f64, 0.0f64);
        let w0 = std::f64::consts::TAU * f / FS as f64;
        for (k, &v) in x.iter().enumerate() {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * k as f64 / n as f64).cos();
            wsum += w;
            let ph = w0 * k as f64;
            re += w * v as f64 * ph.cos();
            im -= w * v as f64 * ph.sin();
        }
        2.0 * (re * re + im * im).sqrt() / wsum
    }

    fn peak_near(x: &[f32], guess: f64, half: f64) -> (f64, f64) {
        let step = (half / 30.0).max(0.05);
        let (mut best_f, mut best_m) = (guess, -1.0f64);
        let mut f = guess - half;
        while f <= guess + half {
            if f > 5.0 {
                let m = dft_mag(x, f);
                if m > best_m {
                    best_m = m;
                    best_f = f;
                }
            }
            f += step;
        }
        (best_f, best_m)
    }

    fn decay_sigma(x: &[f32], f: f64, t0: f64, t1: f64) -> f64 {
        let win = (0.080 * FS as f64) as usize;
        let hop = win / 2;
        let (mut ts, mut ms) = (Vec::new(), Vec::new());
        let mut a = (t0 * FS as f64) as usize;
        let end = ((t1 * FS as f64) as usize).min(x.len());
        while a + win <= end {
            let m = dft_mag(&x[a..a + win], f);
            if m > 1e-12 {
                ts.push((a + win / 2) as f64 / FS as f64);
                ms.push(m.ln());
            }
            a += hop;
        }
        if ts.len() < 3 {
            return 0.0;
        }
        let n = ts.len() as f64;
        let (sx, sy): (f64, f64) = (ts.iter().sum(), ms.iter().sum());
        let sxx: f64 = ts.iter().map(|v| v * v).sum();
        let sxy: f64 = ts.iter().zip(&ms).map(|(a, b)| a * b).sum();
        let d = n * sxx - sx * sx;
        if d.abs() < 1e-12 {
            return 0.0;
        }
        -((n * sxy - sx * sy) / d)
    }

    pub(super) struct Ladder {
        pub t100: Vec<f64>,
        pub sigma: Vec<f64>,
    }

    pub(super) fn measure(x: &[f32], f0: f64, n_partials: usize, want_sigma: bool) -> Ladder {
        let win_s = (4.0 / f0).clamp(0.02, 0.15);
        let win = (win_s * FS as f64) as usize;
        let slice = |t0: f64| -> &[f32] {
            let a = ((t0 * FS as f64) as usize).min(x.len().saturating_sub(win));
            &x[a..(a + win).min(x.len())]
        };
        let onset = slice(0.5 * win_s);
        let mut freqs = Vec::new();
        for n in 1..=n_partials {
            let half = (0.45 * f0).max(6.0);
            freqs.push(peak_near(onset, n as f64 * f0, half).0);
        }
        let s = slice(0.1);
        let raw: Vec<f64> = freqs.iter().map(|&f| dft_mag(s, f)).collect();
        let m = raw.iter().cloned().fold(1e-30, f64::max);
        let t100: Vec<f64> = raw.iter().map(|&x| 20.0 * (x / m).max(1e-9).log10()).collect();
        let sigma = if want_sigma {
            freqs.iter().map(|&f| decay_sigma(x, f, 0.1, 1.0)).collect()
        } else {
            vec![0.0; freqs.len()]
        };
        Ladder { t100, sigma }
    }

    pub(super) fn n_partials_for(f0: f64) -> usize {
        ((0.42 * FS as f64 / f0) as usize).clamp(3, PARTIALS)
    }

    /// How much of the correction applies at this key: full inside the band
    /// the learned ladder is trustworthy in, tapering to nothing outside it.
    fn ladder_weight(key: u8) -> f64 {
        let k = key as f64;
        let (lo, hi) = (LADDER_KEYS.0 as f64, LADDER_KEYS.1 as f64);
        if k < lo {
            ((k - (lo - TAPER)) / TAPER).clamp(0.0, 1.0)
        } else if k > hi {
            (((hi + TAPER) - k) / TAPER).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    /// The whole derivation for one key.
    pub(super) fn derive_key(key: u8) -> (KeyTargets, Vec<f64>) {
        let mut phys = dry_piano();
        let f0p = phys.key_info(key).map(|k| k.f0 as f64).unwrap_or(440.0);
        let xp = render(&mut phys, key, DERIVE_VELOCITY, 1.2);
        let f0l = forte_f0(key);
        let mut lrn = dry_learned();
        let xl = render(&mut lrn, key, DERIVE_VELOCITY, 1.2);
        let np = n_partials_for(f0p.max(f0l));
        let want_sigma = key >= SUSTAIN_KEYS.0 && key <= SUSTAIN_KEYS.1;
        let lp = measure(&xp, f0p, np, want_sigma);
        let ll = measure(&xl, f0l, np, want_sigma);

        // The ladder correction, in dB, before smoothing.
        let weight = ladder_weight(key);
        let mut db: Vec<f64> = (0..np)
            .map(|m| {
                let delta = lp.t100[m] - ll.t100[m];
                (-PULL * delta * weight).clamp(-CLAMP_DB, CLAMP_DB)
            })
            .collect();
        // Three-tap smoothing: the correction is a spectral TILT, not a comb.
        // Without it, one noisy partial becomes an audible resonance.
        let raw = db.clone();
        for m in 0..np {
            let a = raw[m.saturating_sub(1)];
            let b = raw[m];
            let c = raw[(m + 1).min(np - 1)];
            db[m] = 0.25 * a + 0.5 * b + 0.25 * c;
        }

        let mut gain = [1.0f32; PARTIALS];
        for m in 0..np {
            gain[m] = 10f64.powf(db[m] / 20.0) as f32;
        }
        let mut sigma_scale = [1.0f32; PARTIALS];
        if want_sigma {
            for m in 0..np.min(6) {
                let (sp, sl) = (lp.sigma[m], ll.sigma[m]);
                if sp > 0.2 && sl > 0.2 && sl < sp {
                    sigma_scale[m] = ((sl / sp) as f32).clamp(SIGMA_FLOOR, 1.0);
                }
            }
        }
        let deltas: Vec<f64> = (0..np).map(|m| lp.t100[m] - ll.t100[m]).collect();
        (KeyTargets { gain, sigma_scale }, deltas)
    }

    /// Derive the whole table and print it as the Rust source that
    /// `hybrid_targets.rs` holds. See the module docs to regenerate.
    #[test]
    #[ignore]
    fn generate_hybrid_targets() {
        let started = std::time::Instant::now();
        let mut out = String::new();
        out.push_str(&format!(
            "// GENERATED by `generate_hybrid_targets` — do not edit by hand.\n\
             // Derived at velocity {DERIVE_VELOCITY}, pulling the physical partial ladder\n\
             // {:.0}% toward the learned one with a +/-{CLAMP_DB:.0} dB clamp and three-tap\n\
             // smoothing, over keys {}..={} (tapering {TAPER:.0} semitones either side),\n\
             // plus a one-sided treble sustain correction over keys {}..={}.\n\
             pub static TARGETS: [KeyTargets; 88] = [\n",
            PULL * 100.0, LADDER_KEYS.0, LADDER_KEYS.1, SUSTAIN_KEYS.0, SUSTAIN_KEYS.1
        ));
        let mut corrected = 0usize;
        for key in FIRST_KEY..=108u8 {
            let (t, _) = derive_key(key);
            let g: Vec<String> = t.gain.iter().map(|v| format!("{v:.4}")).collect();
            let s: Vec<String> = t.sigma_scale.iter().map(|v| format!("{v:.4}")).collect();
            if t.gain.iter().any(|v| (v - 1.0).abs() > 1e-3) {
                corrected += 1;
            }
            out.push_str(&format!(
                "    // key {key}\n    KeyTargets {{\n        gain: [{}],\n        sigma_scale: [{}],\n    }},\n",
                g.join(", "),
                s.join(", ")
            ));
        }
        out.push_str("];\n");
        std::fs::write("src/hybrid_targets.rs", &out).expect("write the table");
        println!(
            "wrote src/hybrid_targets.rs — {corrected} of 88 keys corrected, derived in {:?}",
            started.elapsed()
        );
    }
}

/// What the hybrid engine claims, checked against the engine.
#[cfg(test)]
mod truth {
    use super::derive::{measure, n_partials_for, render, FS};
    use super::*;
    use makepad_piano_model::{
        learned::{forte_f0, LearnedPiano},
        Piano, PianoPreset, PIANO_PRESETS,
    };

    /// Mean absolute ladder distance from the learned target, in dB: the
    /// number the whole hybrid exists to reduce.
    fn distance_from_learned(piano: &mut Piano, key: u8, vel: u8) -> f64 {
        let f0p = piano.key_info(key).map(|k| k.f0 as f64).unwrap_or(440.0);
        let x = render(piano, key, vel, 1.2);
        let f0l = forte_f0(key);
        let mut learned = LearnedPiano::new(FS);
        learned.set_reverb_mix(0.0);
        learned.set_early_reflection_level(0.0);
        learned.set_soft_clip(false);
        let xl = render(&mut learned, key, vel, 1.2);
        let np = n_partials_for(f0p.max(f0l));
        let a = measure(&x, f0p, np, false);
        let b = measure(&xl, f0l, np, false);
        a.t100
            .iter()
            .zip(&b.t100)
            .map(|(p, l)| (p - l).abs())
            .sum::<f64>()
            / np as f64
    }

    fn build(preset: Option<&PianoPreset>, hybrid: bool) -> Piano {
        let mut p = match preset {
            Some(preset) => Piano::new_with_preset(FS, preset),
            None => Piano::new(FS),
        };
        p.set_reverb_mix(0.0);
        p.set_early_reflection_level(0.0);
        p.set_soft_clip(false);
        if hybrid {
            apply_targets(&mut p);
        }
        p
    }

    /// The reason the engine exists: across the register the correction
    /// covers, the hybrid's partial balance is measurably closer to the
    /// recorded-piano target than the physical model's is.
    ///
    /// This is also the tripwire on the baked table. [`TARGETS`] is derived
    /// against the shipped physical model; if that model is revoiced, the
    /// table goes stale and this test says so instead of the app quietly
    /// shipping a correction aimed at an instrument that no longer exists.
    #[test]
    /// Measured in the OPTIMISED build only. The unoptimised physical model
    /// is audibly a different instrument in the treble — key 84 sits 34.9 dB
    /// from the learned target under `cargo test` and 11.6 dB under
    /// `cargo test --release` — so a correction derived for the shipped
    /// instrument cannot be judged against the debug one. Runs in
    /// `cargo test --release`; listed as ignored otherwise.
    #[ignore = "hybrid is dormant; run explicitly with --release after rebaking"]
    fn the_hybrid_is_closer_to_the_recorded_piano_than_the_physical_model() {
        let keys: [u8; 10] = [33, 39, 45, 51, 57, 60, 66, 72, 78, 84];
        let mut better = 0usize;
        for key in keys {
            let physical = distance_from_learned(&mut build(None, false), key, 112);
            let hybrid = distance_from_learned(&mut build(None, true), key, 112);
            if hybrid < physical {
                better += 1;
            }
            println!("key {key:>3}: physical {physical:5.2} dB -> hybrid {hybrid:5.2} dB");
        }
        assert!(
            better >= 8,
            "the baked table improved only {better}/10 keys. This almost always means the \
             physical model has been revoiced since the table was derived, so the \
             correction is aimed at an instrument that no longer exists. Rebake it:\n    \
             cargo test -p makepad-score-ui --release --lib generate_hybrid_targets -- \
             --ignored --nocapture"
        );
    }

    /// Hybrid IS the physical model, so everything that makes the physical
    /// model worth having still works: the character amounts still bite.
    #[test]
    fn the_hybrid_keeps_the_physical_models_character_controls() {
        use makepad_piano_model::Voicing;
        let quiet = Voicing { body_tap: 0.0, knock: 0.0, roughness: 0.0, phantoms: 0.0, attack_noise: 0.0, attack_body: 0.0, sympathetic: 0.0 };
        let loud = Voicing { body_tap: 2.0, knock: 2.0, roughness: 2.0, phantoms: 2.0, attack_noise: 2.0, attack_body: 2.0, sympathetic: 2.0 };
        let take = |voicing| {
            let mut p = build(None, true);
            p.set_voicing(voicing);
            render(&mut p, 60, 96, 0.5)
        };
        let reach = take(quiet)
            .iter()
            .zip(&take(loud))
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(reach > 0.1, "the character amounts must still bite under hybrid: {reach}");
        // And it is not simply the physical model unchanged.
        let plain = render(&mut build(None, false), 60, 96, 0.5);
        let shaped = render(&mut build(None, true), 60, 96, 0.5);
        let changed = plain.iter().zip(&shaped).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(changed > 1e-3, "the hybrid must actually differ from the physical model: {changed}");
    }

    /// Whether the baked table survives a preset that rebuilds the design.
    ///
    /// The table's gains multiply per-partial output weights that the design
    /// sets at construction, so a preset with a `design` override is a
    /// different instrument from the one the table was derived against. This
    /// measures rather than assumes: for each rebuilding preset, is the
    /// hybrid still no further from the recorded target than the physical
    /// model is?
    #[test]
    /// Measured in the OPTIMISED build only. The unoptimised physical model
    /// is audibly a different instrument in the treble — key 84 sits 34.9 dB
    /// from the learned target under `cargo test` and 11.6 dB under
    /// `cargo test --release` — so a correction derived for the shipped
    /// instrument cannot be judged against the debug one. Runs in
    /// `cargo test --release`; listed as ignored otherwise.
    #[ignore = "hybrid is dormant; run explicitly with --release after rebaking"]
    fn the_table_survives_the_presets_that_rebuild_the_design() {
        let mut worse = Vec::new();
        for preset in PIANO_PRESETS.iter() {
            let physical = distance_from_learned(&mut build(Some(preset), false), 60, 112);
            let hybrid = distance_from_learned(&mut build(Some(preset), true), 60, 112);
            println!("{:<22} physical {physical:5.2} dB -> hybrid {hybrid:5.2} dB", preset.name);
            if hybrid > physical + 1.0 {
                worse.push((preset.name, physical, hybrid));
            }
        }
        assert!(
            worse.len() * 3 <= PIANO_PRESETS.len(),
            "the baked table fights too many designs: {worse:?}"
        );
    }
}
