//! The drum slots and their names, General MIDI drum notes, Ironfish's
//! programs and the recipe text a patch travels as in a flow graph. The
//! synthesized kits and patches themselves are Splash documents
//! (makepad-score-live's `palette::sounds`).

/// Drum slots, in the order a kit lists them. `hit(@kick)` names one.
pub const DRUM_SLOTS: [&str; 16] = [
    "kick", "snare", "clap", "hat", "ohat", "rim", "tom_lo", "tom_mid", "tom_hi", "crash", "ride", "shaker", "cowbell", "conga", "snap", "zap",
];
pub const SLOTS: usize = DRUM_SLOTS.len();
/// The open hat is choked by the closed one.
pub const CHOKES: [(usize, usize); 1] = [(3, 4)];

pub fn slot_of(name: &str) -> Option<usize> {
    let name = match name {
        "bd" | "bass_drum" => "kick",
        "sd" => "snare",
        "cp" => "clap",
        "hh" | "hihat" | "ch" | "closed_hat" => "hat",
        "oh" | "open_hat" => "ohat",
        "rs" | "rimshot" | "side" => "rim",
        "lt" | "tom" | "tom1" => "tom_lo",
        "mt" | "tom2" => "tom_mid",
        "ht" | "tom3" => "tom_hi",
        "cy" | "cymbal" => "crash",
        "rd" => "ride",
        "sh" | "maraca" => "shaker",
        "cb" | "bell" => "cowbell",
        "perc" | "bongo" => "conga",
        "fingersnap" => "snap",
        other => other,
    };
    DRUM_SLOTS.iter().position(|slot| *slot == name)
}

/// A recipe as text that parses back to the same recipe, field for field:
/// how a patch travels as a node setting in a flow graph.
pub fn recipe_text(recipe: &makepad_audio_synth::recipe::Recipe) -> String {
    use makepad_audio_synth::dsp::{FilterMode, NoiseColor, Wave};
    use makepad_audio_synth::recipe::Source;
    let wave = |w: Wave| match w {
        Wave::Sine => "sine",
        Wave::Triangle => "tri",
        Wave::Saw => "saw",
        Wave::Square => "square",
        Wave::Pulse => "pulse",
        Wave::SoftSaw => "softsaw",
    };
    let mut parts = Vec::new();
    for (i, l) in recipe.layers().iter().enumerate() {
        let mut t = Vec::new();
        if i == 0 {
            t.push(format!("volume={} jitter={} reverb={} pitch={}", recipe.volume, recipe.jitter, recipe.reverb, recipe.pitch));
        }
        match l.source {
            Source::None | Source::Osc(_) => t.push(format!("osc={}", wave(match l.source { Source::Osc(w) => w, _ => Wave::Sine }))),
            Source::Noise(c) => t.push(format!("noise={}", match c {
                NoiseColor::White => "white",
                NoiseColor::Pink => "pink",
                NoiseColor::Brown => "brown",
                NoiseColor::Blue => "blue",
                NoiseColor::Velvet => "velvet",
            })),
            Source::Fm { carrier, ratio } => t.push(format!("osc={} fm={ratio}", wave(carrier))),
            Source::Pluck { damping } => t.push(format!("pluck={damping}")),
        }
        t.push(format!("freq={} glide={} vib={} vib_depth={} detune={} index={}", l.freq, l.glide, l.vib_hz, l.vib_depth, l.detune_cents, l.fm_index));
        if l.freq_to.is_finite() {
            t.push(format!("to={}", l.freq_to));
        }
        if l.fm_index_to.is_finite() {
            t.push(format!("index_to={}", l.fm_index_to));
        }
        // The cutoff first: it switches an unfiltered layer to low-pass, which
        // the filter word after it sets back.
        t.push(format!("cutoff={}", l.cutoff));
        if l.cutoff_to.is_finite() {
            t.push(format!("cutoff_to={}", l.cutoff_to));
        }
        t.push(format!("filter={}", match l.filter {
            FilterMode::Off => "off",
            FilterMode::Lowpass => "lowpass",
            FilterMode::Highpass => "highpass",
            FilterMode::Bandpass => "bandpass",
            FilterMode::Notch => "notch",
            FilterMode::Ladder => "ladder",
        }));
        t.push(format!("q={} drive={} a={} d={} s={} r={} gain={} delay={} pan={} width={} density={} repeat={} every={} falloff={}",
            l.q, l.drive, l.attack, l.decay, l.sustain, l.release, l.gain, l.delay, l.pan, l.width, l.density, l.repeat, l.every, l.repeat_decay));
        if l.pwm_depth > 0.0 {
            t.push(format!("pwm={} pwm_depth={}", l.pwm_hz, l.pwm_depth));
        }
        if l.hold >= 0.0 {
            t.push(format!("hold={}", l.hold));
        }
        parts.push(t.join(" "));
    }
    parts.join("; ")
}

/// Ironfish's built-in programs, by name (`Ironfish{program: "glass"}`).
pub const IRON_PROGRAMS: [&str; 8] = ["init", "glass", "acid", "sub", "formant", "chip", "supersaw", "pad"];

/// The Ironfish settings a score writes by name, each 0..1 over the
/// synth's own range; an unset one keeps the program's.
pub const IRON_PARAMS: [(&str, makepad_audio_synth::ironfish::IronfishParam); 15] = {
    use makepad_audio_synth::ironfish::IronfishParam as P;
    [
        ("cutoff", P::FilterCutoff),
        ("resonance", P::FilterResonance),
        ("env", P::FilterEnvAmount),
        ("attack", P::AmpAttack),
        ("decay", P::AmpDecay),
        ("sustain", P::AmpSustain),
        ("release", P::AmpRelease),
        ("sub", P::Sub),
        ("noise", P::Noise),
        ("glide", P::Portamento),
        ("crush", P::Bitcrush),
        ("delay", P::DelaySend),
        ("chorus", P::ChorusMix),
        ("reverb", P::ReverbMix),
        ("output", P::Output),
    ]
};

/// An Ironfish patch: `program` with `values` (one per [`IRON_PARAMS`],
/// NaN or out of 0..1 where the program's plays).
pub fn iron_patch(program: usize, values: &[f32]) -> makepad_audio_synth::ironfish::IronfishPatch {
    let mut patch = makepad_audio_synth::ironfish::IronfishPatch::preset(program);
    for ((name, param), value) in IRON_PARAMS.iter().zip(values) {
        if (0.0..=1.0).contains(value) {
            patch.set_normalised(*param, *value);
            if *name == "crush" {
                patch.bitcrush_enabled = *value > 0.0;
            }
        }
    }
    patch.sanitise()
}

/// The General MIDI percussion note of a drum by name: the kit slots, and
/// the GM kit's own pieces (`pedalhat`, `floortom`, `tom1`-`tom3` high to
/// low, `bell` the ride bell).
pub fn gm_drum(name: &str) -> Option<u8> {
    Some(match name {
        "kick" | "bd" | "bass_drum" => 36,
        "snare" | "sd" => 38,
        "rim" | "rs" | "rimshot" | "side" | "sidestick" => 37,
        "clap" | "cp" => 39,
        "hat" | "hihat" | "hh" | "ch" | "closed_hat" => 42,
        "ohat" | "openhat" | "open_hat" | "oh" => 46,
        "pedalhat" | "pedal_hat" => 44,
        "tom1" | "hitom" | "tom_hi" | "ht" => 50,
        "tom2" | "midtom" | "tom_mid" | "mt" => 47,
        "tom3" | "lowtom" | "tom_lo" | "lt" | "tom" => 45,
        "floortom" | "floor_tom" => 43,
        "crash" | "cy" | "cymbal" => 49,
        "ride" | "rd" => 51,
        "bell" | "ridebell" | "ride_bell" => 53,
        "cowbell" | "cb" => 56,
        "shaker" | "sh" | "maraca" => 70,
        "conga" | "perc" | "bongo" => 63,
        "snap" | "fingersnap" => 39,
        _ => return None,
    })
}

/// A drum by name: its kit slot (the nearest one for the GM pieces a kit
/// lacks) and its General MIDI note.
pub fn drum(name: &str) -> Option<(usize, u8)> {
    let slot = slot_of(name).or_else(|| slot_of(match name {
        "pedalhat" | "pedal_hat" => "hat",
        "floortom" | "floor_tom" => "tom_lo",
        "ridebell" | "ride_bell" => "ride",
        "hitom" => "tom_hi",
        "midtom" => "tom_mid",
        "lowtom" => "tom_lo",
        _ => return None,
    }))?;
    let gm = gm_drum(name).or_else(|| gm_drum(DRUM_SLOTS[slot])).unwrap_or(36);
    Some((slot, gm))
}
