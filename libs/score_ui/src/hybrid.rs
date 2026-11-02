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
//! cargo run -p makepad-score-ui --release --example generate_hybrid_targets
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
#[path = "hybrid_derive.rs"]
mod derive;

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
             cargo run -p makepad-score-ui --release --example generate_hybrid_targets"
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
