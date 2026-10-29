//! Twist (oscillator type 10): the macro engine — sixteen synthesis models
//! behind harmonics / timbre / morph — and its adapter (OSCILLATORS.md
//! section 13).
//!
//! The models are a Rust port of the MIT-licensed eurorack macro-oscillator
//! by Emilie Gillet (each ported file keeps its MIT notice; credit in
//! CREDITS.md). The engine runs at its native 48 kHz in sub-blocks of 4
//! samples (1 while the LPG or FM is active) and is resampled to the
//! oscillator rate with a Lanczos (a = 4) interpolator.
//!
//! The adapter (this file) is our own code.

mod engine;
pub(crate) mod lanczos;
mod osc_base;
pub(crate) mod stm;
mod tables;
mod voice;
mod wave_data;

mod chord_bank;
mod osc_string_synth;
mod osc_variable_saw;
mod osc_variable_shape;
mod osc_wavetable;
#[cfg(test)]
mod tests_a;
mod fx_diffuser;
mod noise_dust;
#[cfg(test)]
mod tests_c;

mod additive;
mod bass_drum;
mod chord;
mod fm;
mod grain;
mod hi_hat;
mod modal;
mod noise;
mod particle;
mod snare_drum;
mod speech;
mod speech_phonemes;
mod string;
mod swarm;
mod va;
mod waveshaping;
mod wavetable;

use super::OscBlock;
use crate::kit::core::{Lag, SlotDef, SlotUnit, SlotValue};
use lanczos::{lanczos, LANCZOS_A};
use stm::Random;
use voice::{Modulations, Patch, Voice, NUM_ENGINES};

/// The engine's native rate (its frequency scale uses this, see engine.rs).
pub const ENGINE_RATE: f32 = 48000.0;

pub const ENGINE_NAMES: [&str; NUM_ENGINES] = [
    "Waveforms",
    "Waveshaper",
    "2-Operator FM",
    "Formant/PD",
    "Harmonic",
    "Wavetable",
    "Chords",
    "Vowels/Speech",
    "Granular Cloud",
    "Filtered Noise",
    "Particle Noise",
    "Inharmonic String",
    "Modal Resonator",
    "Analog Kick",
    "Analog Snare",
    "Analog Hi-Hat",
];

/// Per-engine display labels of slots 1..4 (harmonics, timbre, morph, aux).
const ENGINE_LABELS: [[&str; 4]; NUM_ENGINES] = [
    ["Detune", "Square Shape", "Saw Shape", "Sync"],
    ["Waveshaper", "Fold", "Asymmetry", "Variation"],
    ["Ratio", "Amount", "Feedback", "Sub"],
    ["Ratio/Type", "Formant", "Shape", "PD"],
    ["Bump", "Peak", "Shape", "Organ"],
    ["Bank", "Morph X", "Morph Y", "Lo-Fi"],
    ["Type", "Inversion", "Shape", "Root"],
    ["Speak", "Species", "Segment", "Raw"],
    ["Pitch Random", "Grain Density", "Grain Duration", "Sine"],
    ["Type", "Clock Frequency", "Resonance", "Dual Peak"],
    ["Freq Random", "Density", "Filter Type", "Raw"],
    ["Inharmonicity", "Brightness", "Decay Time", "Exciter"],
    ["Material", "Brightness", "Decay Time", "Exciter"],
    ["Sharpness", "Brightness", "Decay Time", "Variation"],
    ["Tone<>Noise", "Model", "Decay Time", "Variation"],
    ["Tone<>Noise", "Low Cut", "Decay Time", "Variation"],
];

/// Display label of slot 1..3 for an engine (UI only).
pub fn slot_label(engine: i32, slot: usize) -> &'static str {
    let e = engine.clamp(0, NUM_ENGINES as i32 - 1) as usize;
    match slot {
        1..=3 => ENGINE_LABELS[e][slot - 1],
        _ => "",
    }
}

/// Display label of the aux slot (4): "<X> Mix", or "Main<>X Pan" when extended.
pub fn aux_label(engine: i32, extended: bool) -> String {
    let e = engine.clamp(0, NUM_ENGINES as i32 - 1) as usize;
    let x = ENGINE_LABELS[e][3];
    if extended { format!("Main<>{x} Pan") } else { format!("{x} Mix") }
}

pub fn slot_defs() -> [SlotDef; 7] {
    [
        SlotDef { about: "synthesis model (Vowels/Speech: vowel and formant voices; its word range speaks phonemes, no recorded words ship)", ..SlotDef::choice("Engine", &ENGINE_NAMES, 0.0) },
        SlotDef { about: "harmonics (label depends on the engine)", ..SlotDef::float("Harmonics", -1.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef { about: "timbre (label depends on the engine)", ..SlotDef::float("Timbre", -1.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef { about: "morph (label depends on the engine)", ..SlotDef::float("Morph", -1.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef {
            can_extend: true,
            about: "main/aux output mix; extended = main to one side, aux to the other",
            ..SlotDef::float("Aux Mix", -1.0, 1.0, -1.0, SlotUnit::Percent)
        },
        SlotDef {
            can_deactivate: true,
            about: "low-pass gate colour; deactivated = no gate (free running)",
            ..SlotDef::float("LPG Response", 0.0, 1.0, 0.0, SlotUnit::Percent)
        },
        SlotDef { about: "low-pass gate decay (inactive when the gate is off)", ..SlotDef::float("LPG Decay", 0.0, 1.0, 0.0, SlotUnit::Percent) },
    ]
}

/// Default slot values: the defs' defaults with LPG Response deactivated.
pub fn default_slots() -> [SlotValue; 7] {
    let d = slot_defs();
    let mut s = [SlotValue::default(); 7];
    for i in 0..7 {
        s[i] = d[i].default_value();
    }
    s[5].deactivated = true;
    s
}

/// No per-type stream migration (the rev <= 27 pitch fix is patch-level:
/// [`legacy_pitch_fix`]).
pub fn migrate(_rev: i32, _slots: &mut [SlotValue; 7]) {}

/// The patch-level fix for revision <= 27 (the engine's frequency scale moved
/// from the hardware's corrected rate to 48 kHz): returns the new osc pitch
/// parameter value for a Twist oscillator.
pub fn legacy_pitch_fix(rev: i32, pitch: f32, extended: bool, absolute: bool) -> f32 {
    if rev > 27 {
        return pitch;
    }
    if absolute {
        pitch * if extended { 1.0002222 } else { 1.0026667 }
    } else {
        pitch + if extended { 0.0038420667 } else { 0.0461048 }
    }
}

const RING: usize = 64;
const FM_RING: usize = 256;

/// Drift LFO (OSCILLATORS.md 1.3), stepped once per block.
#[derive(Clone, Copy, Debug, Default)]
struct DriftLfo {
    d2: f32,
    d: f32,
}

impl DriftLfo {
    fn init(&mut self, nonzero: bool, rng: &mut Random) {
        self.d2 = if nonzero { 0.0005 * rng.get_float() } else { 0.0 };
        self.d = 0.0;
    }
    fn next(&mut self, rng: &mut Random) -> f32 {
        let r = rng.get_float() * 2.0 - 1.0;
        self.d2 = self.d2 * (1.0 - 1e-5) + r * 1e-5;
        self.d = self.d2 * 316.227766;
        self.d
    }
}

/// Character filter post pass (OSCILLATORS.md 1.5), one channel.
#[derive(Clone, Copy, Debug, Default)]
struct Character {
    x1: f32,
    y1: f32,
    seeded: bool,
}

impl Character {
    fn process(&mut self, mode: u8, fs: f32, buf: &mut [f32; 64]) {
        if mode == 1 {
            return;
        }
        if !self.seeded {
            self.x1 = buf[0];
            self.y1 = buf[0];
            self.seeded = true;
        }
        let k = (1.0 - 2.0 * 5000.0 / fs).powi(2);
        for x in buf.iter_mut() {
            if mode == 0 {
                self.y1 = k * self.y1 + (1.0 - k) * *x;
                *x = self.y1;
            } else {
                let y = (*x - k * self.x1) / (1.0 - k);
                self.x1 = *x;
                *x = y;
            }
        }
    }
}

/// One Twist oscillator voice.
pub struct Twist {
    voice: Box<Voice>,
    patch: Patch,
    mods: Modulations,
    rng: Random,
    drift: DriftLfo,
    harmonics: Lag,
    timbre: Lag,
    morph: Lag,
    decay: Lag,
    lpg_colour: Lag,
    aux_mix: Lag,
    // 48 kHz engine output rings and the resampler's read position (in
    // engine samples; `written` = engine samples produced so far).
    main: [f32; RING],
    aux: [f32; RING],
    written: i64,
    pos: f64,
    // FM modulator at the oscillator rate, read at 48 kHz.
    fm_ring: [f32; FM_RING],
    fm_written: i64,
    fm_pos: f64,
    character: [Character; 2],
}

impl Twist {
    pub fn new(seed: u32) -> Twist {
        Twist {
            voice: Box::new(Voice::new(seed ^ 0x5eed_1234)),
            patch: Patch::default(),
            mods: Modulations::default(),
            rng: Random::new(seed),
            drift: DriftLfo::default(),
            harmonics: Lag::default(),
            timbre: Lag::default(),
            morph: Lag::default(),
            decay: Lag::default(),
            lpg_colour: Lag::default(),
            aux_mix: Lag::default(),
            main: [0.0; RING],
            aux: [0.0; RING],
            written: 0,
            pos: 0.0,
            fm_ring: [0.0; FM_RING],
            fm_written: 0,
            fm_pos: 0.0,
            character: [Character::default(); 2],
        }
    }

    /// Note start / type change: reset the engine voice and the adapter
    /// state, then pre-run and discard about a cycle of output.
    pub fn init(&mut self, b: &OscBlock, nonzero_drift: bool) {
        self.voice.init();
        self.patch = Patch::default();
        self.mods = Modulations::default();
        self.drift.init(nonzero_drift, &mut self.rng);
        for l in [&mut self.harmonics, &mut self.timbre, &mut self.morph, &mut self.decay, &mut self.lpg_colour, &mut self.aux_mix] {
            *l = Lag::default();
        }
        self.main = [0.0; RING];
        self.aux = [0.0; RING];
        self.written = 0;
        self.pos = 0.0;
        self.fm_ring = [0.0; FM_RING];
        self.fm_written = 0;
        self.fm_pos = 0.0;
        self.character = [Character::default(); 2];

        let freq = 440.0 * 2f32.powf((b.pitch - 69.0) / 12.0);
        let dphase = freq / b.fs_os;
        let mut cycle = (1.0 / dphase).max(1.0);
        while cycle < 10.0 {
            cycle *= 2.0;
        }
        if !b.retrigger {
            cycle *= 1.0 + self.rng.get_float();
        }
        let mut n = cycle.ceil() as usize;
        self.set_block(b);
        let mut ml = [0.0f32; 64];
        let mut mr = [0.0f32; 64];
        while n > 0 {
            let k = n.min(64);
            self.run(b, &mut ml[..k], &mut mr[..k]);
            n -= k;
        }
    }

    fn lpg_on(b: &OscBlock) -> bool {
        !b.slots[5].deactivated
    }

    /// Per-block patch fields (lag targets, engine, trigger, FM amount).
    fn set_block(&mut self, b: &OscBlock) {
        let s = b.slots;
        let drift = b.drift * self.drift.next(&mut self.rng);
        self.patch.note = b.pitch + drift;
        self.patch.engine = s[0].i().clamp(0, NUM_ENGINES as i32 - 1);
        self.harmonics.set((s[1].v + 1.0) * 0.5);
        self.timbre.set((s[2].v + 1.0) * 0.5);
        self.morph.set((s[3].v + 1.0) * 0.5);
        self.aux_mix.set((s[4].v + 1.0) * 0.5);
        self.lpg_colour.set(s[5].v.clamp(0.0, 1.0));
        self.decay.set(s[6].v.clamp(0.0, 1.0));
        if Self::lpg_on(b) {
            self.mods.trigger_patched = true;
            self.mods.trigger = if b.gate { 1.0 } else { 0.0 };
        } else {
            self.mods.trigger_patched = false;
            self.mods.trigger = 0.0;
        }
        if b.fm {
            let x = (18.0 * b.fmdepth.max(1e-30).log2()).clamp(-192.0, 96.0).clamp(-143.5, 71.7);
            self.patch.frequency_modulation_amount = ((x + 143.5) / 215.2).clamp(0.0, 1.0);
            self.mods.frequency_patched = true;
        } else {
            self.patch.frequency_modulation_amount = 0.0;
            self.mods.frequency_patched = false;
            self.mods.frequency = 0.0;
        }
    }

    /// Renders one engine sub-block of `n` samples into the rings.
    fn render_sub(&mut self, n: usize, fm: bool, fm_step: f64) {
        self.patch.harmonics = self.harmonics.process();
        self.patch.timbre = self.timbre.process();
        self.patch.morph = self.morph.process();
        self.patch.decay = self.decay.process();
        self.patch.lpg_colour = self.lpg_colour.process();
        if fm {
            // Lanczos read of the modulator at the engine rate; never past
            // what has been written.
            let limit = (self.fm_written - 1 - LANCZOS_A) as f64;
            let p = self.fm_pos.min(limit.max(0.0));
            self.mods.frequency = 137.0 * lanczos(&self.fm_ring, p);
            self.fm_pos += fm_step * n as f64;
        }
        let mut out = [0.0f32; 4];
        let mut aux = [0.0f32; 4];
        self.voice.render(&self.patch, &self.mods, &mut out[..n], &mut aux[..n]);
        for i in 0..n {
            let w = (self.written as usize) % RING;
            self.main[w] = out[i] / 32768.0;
            self.aux[w] = aux[i] / 32768.0;
            self.written += 1;
        }
    }

    /// Resamples engine output into `l` / `r` (no character filter).
    fn run(&mut self, b: &OscBlock, l: &mut [f32], r: &mut [f32]) {
        let ratio = (ENGINE_RATE / b.fs_os) as f64;
        let fm_step = (b.fs_os / ENGINE_RATE) as f64;
        let sub = if Self::lpg_on(b) || b.fm { 1 } else { 4 };
        let extended = b.slots[4].extend;
        for i in 0..l.len() {
            while self.written <= self.pos.floor() as i64 + LANCZOS_A {
                self.render_sub(sub, b.fm, fm_step);
            }
            let main = lanczos(&self.main, self.pos);
            let aux = lanczos(&self.aux, self.pos);
            self.pos += ratio;
            let m = self.aux_mix.process();
            if extended {
                l[i] = m * main + (1.0 - m) * aux;
                r[i] = m * aux + (1.0 - m) * main;
            } else {
                let v = aux * m + main * (1.0 - m);
                l[i] = v;
                r[i] = v;
            }
        }
        // Keep the positions small (exact: both are shifted by a multiple
        // of the ring sizes).
        if self.written > (1 << 40) {
            let shift = (self.written / (RING as i64 * FM_RING as i64)) * (RING as i64 * FM_RING as i64);
            self.written -= shift;
            self.pos -= shift as f64;
        }
    }

    pub fn process(&mut self, b: &OscBlock, l: &mut [f32; 64], r: &mut [f32; 64]) {
        self.set_block(b);
        if b.fm {
            for &x in b.fm_buf.iter() {
                self.fm_ring[(self.fm_written as usize) % FM_RING] = x;
                self.fm_written += 1;
            }
            // Trail the writer by a few samples so reads stay inside the ring.
            let lead = (self.fm_written - 64 - 2 * LANCZOS_A) as f64;
            if self.fm_pos < lead - 64.0 || self.fm_pos > self.fm_written as f64 {
                self.fm_pos = lead.max(0.0);
            }
        }
        let mut rr = [0.0f32; 64];
        self.run(b, l, &mut rr);
        let fs = b.fs_os * 0.5;
        self.character[0].process(b.character, fs, l);
        if b.stereo {
            self.character[1].process(b.character, fs, &mut rr);
            *r = rr;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS_OS: f32 = 96000.0;

    fn block<'a>(slots: &'a [SlotValue; 7], fm_buf: &'a [f32; 64], pitch: f32, gate: bool) -> OscBlock<'a> {
        OscBlock {
            pitch,
            drift: 0.0,
            stereo: true,
            fm: false,
            fmdepth: 0.0,
            fm_buf,
            slots,
            retrigger: true,
            character: 1,
            gate,
            table: None,
            extra: &[],
            audio_in: None,
            fs_os: FS_OS,
            tsr: 1.0,
        }
    }

    fn render(engine: i32, seed: u32, blocks: usize, lpg: bool) -> Vec<f32> {
        render_with(engine, seed, blocks, lpg, &[])
    }

    /// `overrides`: (slot, value) pairs on top of the defaults.
    fn render_with(engine: i32, seed: u32, blocks: usize, lpg: bool, overrides: &[(usize, f32)]) -> Vec<f32> {
        let mut slots = default_slots();
        slots[0] = SlotValue::int(engine);
        for &(i, v) in overrides {
            slots[i] = SlotValue::float(v);
        }
        if lpg {
            slots[5].deactivated = false;
            slots[6] = SlotValue::float(0.8);
        }
        let fm = [0.0f32; 64];
        let mut t = Twist::new(seed);
        let b = block(&slots, &fm, 69.0, true);
        t.init(&b, false);
        let mut out = Vec::with_capacity(blocks * 64);
        let mut l = [0.0; 64];
        let mut r = [0.0; 64];
        for _ in 0..blocks {
            t.process(&b, &mut l, &mut r);
            out.extend_from_slice(&l);
        }
        out
    }

    /// Frequency of the strongest spectral peak (Hann window, parabolic
    /// interpolation of the log magnitude), searched in [lo, hi] Hz.
    fn peak_hz(x: &[f32], fs: f32, lo: f32, hi: f32) -> f32 {
        let n = x.len();
        let mag = |f: f64| -> f64 {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, &v) in x.iter().enumerate() {
                let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos();
                let a = 2.0 * std::f64::consts::PI * f * i as f64 / fs as f64;
                re += v as f64 * w * a.cos();
                im += v as f64 * w * a.sin();
            }
            (re * re + im * im).sqrt()
        };
        let df = fs as f64 / n as f64;
        let mut best = (0.0, lo as f64);
        let mut f = lo as f64;
        while f <= hi as f64 {
            let m = mag(f);
            if m > best.0 {
                best = (m, f);
            }
            f += df / 4.0;
        }
        // Refine around the coarse peak.
        let mut step = df / 8.0;
        let mut c = best.1;
        for _ in 0..30 {
            let (a, m0, b) = (mag(c - step), mag(c), mag(c + step));
            if a > m0 {
                c -= step;
            } else if b > m0 {
                c += step;
            } else {
                step *= 0.5;
            }
        }
        c as f32
    }

    fn cents(a: f32, b: f32) -> f32 {
        1200.0 * (a / b).log2()
    }

    #[test]
    fn every_engine_renders_finite_sane_levels() {
        for e in 0..NUM_ENGINES as i32 {
            // Particle noise at the default density fires ~12 impulses/s:
            // raise the density to measure a level in a short window.
            let ov: &[(usize, f32)] = if e == 10 { &[(2, 0.6)] } else { &[] };
            let x = render_with(e, 7, 200, false, ov);
            assert!(x.iter().all(|v| v.is_finite()), "engine {e} NaN/inf");
            let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            // 16-bit full scale plus the Lanczos overshoot on clipped peaks.
            assert!(peak <= 1.05, "engine {e} peak {peak}");
            let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
            if e < 13 {
                assert!(rms > 1e-3, "engine {e} silent (rms {rms})");
            }
        }
    }

    #[test]
    fn drums_and_resonators_ring_on_gate() {
        for e in 11..NUM_ENGINES as i32 {
            let x = render(e, 3, 200, true);
            assert!(x.iter().all(|v| v.is_finite()), "engine {e}");
            let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
            assert!(rms > 1e-4, "engine {e} silent with gate (rms {rms})");
        }
    }

    #[test]
    fn pitched_engines_fundamental_within_2_cents() {
        // (engine, slot overrides, LPG on, cents bound). Overrides pick a
        // setting whose strongest partial in 300..600 Hz is the fundamental:
        // FM at ratio exactly 2 (harmonics 0.47), additive with the spectral
        // centroid on the fundamental (timbre 0), string / modal without
        // partial stretch (harmonics 0.25). Chords are left out: the model
        // detunes its voices by x0.998 / x1.004 by design (checked at model
        // level in tests_a). The string runs a few cents sharp by design
        // (DC blocker phase lead, approximate damping-delay compensation).
        let cases: [(i32, &[(usize, f32)], bool, f32); 10] = [
            (0, &[], false, 2.0),
            (1, &[], false, 2.0),
            (2, &[(1, -0.06)], false, 2.0),
            (3, &[], false, 2.0),
            (4, &[(2, -1.0)], false, 2.0),
            (5, &[], false, 2.0),
            (7, &[], false, 2.0),
            (11, &[(1, -0.5)], true, 8.0),
            (12, &[(1, -0.5)], true, 2.0),
            (12, &[(1, -0.5), (2, 0.5)], true, 2.0),
        ];
        for (e, ov, lpg, bound) in cases {
            let x = render_with(e, 11, 400, lpg, ov);
            let tail = &x[x.len() - 8192..];
            let f = peak_hz(tail, FS_OS, 300.0, 600.0);
            let c = cents(f, 440.0);
            assert!(c.abs() < bound, "engine {e}: {f} Hz ({c} cents)");
        }
    }

    #[test]
    fn fm_gate_and_pan_paths_stay_finite() {
        let mut fm = [0.0f32; 64];
        for (i, v) in fm.iter_mut().enumerate() {
            *v = (i as f32 * 0.07).sin();
        }
        for e in 0..NUM_ENGINES as i32 {
            let mut slots = default_slots();
            slots[0] = SlotValue::int(e);
            slots[4] = SlotValue { extend: true, ..SlotValue::float(0.3) };
            slots[5] = SlotValue::float(0.5);
            slots[6] = SlotValue::float(0.5);
            let mut t = Twist::new(5);
            let mut b = block(&slots, &fm, 57.0, true);
            b.fm = true;
            b.fmdepth = 0.5;
            b.character = 0;
            t.init(&b, true);
            let (mut l, mut r) = ([0.0; 64], [0.0; 64]);
            let mut energy = 0.0;
            for k in 0..150 {
                b.gate = k < 60;
                t.process(&b, &mut l, &mut r);
                assert!(l.iter().chain(r.iter()).all(|v| v.is_finite() && v.abs() < 1.1), "engine {e} block {k}");
                if k < 60 {
                    energy += l.iter().map(|v| v * v).sum::<f32>();
                }
            }
            assert!(energy > 0.0, "engine {e} silent under FM + gate");
        }
    }

    #[test]
    fn slot_defs_match_the_spec_table() {
        let d = slot_defs();
        assert_eq!(d[0].choices.len(), 16);
        assert_eq!(d[4].default, -1.0);
        assert!(d[4].can_extend && d[5].can_deactivate);
        assert!(default_slots()[5].deactivated);
        assert_eq!(aux_label(0, false), "Sync Mix");
        assert_eq!(aux_label(13, true), "Main<>Variation Pan");
        assert_eq!(slot_label(5, 2), "Morph X");
    }

    #[test]
    fn deterministic_for_seed() {
        for e in [8, 9, 10, 15] {
            assert_eq!(render(e, 42, 50, false), render(e, 42, 50, false), "engine {e}");
        }
    }

    #[test]
    fn lanczos_reproduces_integer_positions_and_a_sine() {
        let mut ring = [0.0f32; 64];
        for (i, v) in ring.iter_mut().enumerate() {
            *v = (i as f32 * 0.1).sin();
        }
        assert!((lanczos(&ring, 20.0) - ring[20]).abs() < 1e-6);
        let v = lanczos(&ring, 20.5);
        assert!((v - (2.05f32).sin()).abs() < 5e-3, "{v}");
    }

    #[test]
    fn legacy_pitch_fix_values() {
        assert!((legacy_pitch_fix(27, 0.0, false, false) - 0.0461048).abs() < 1e-7);
        assert_eq!(legacy_pitch_fix(28, 1.0, false, false), 1.0);
        assert!((legacy_pitch_fix(20, 10.0, true, true) - 10.002222).abs() < 1e-5);
    }
}
