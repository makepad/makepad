// Rust port of the twist engine's synthesis voice: dsp/voice.h + voice.cc,
// dsp/envelope.h and dsp/fx/low_pass_gate.h of the MIT eurorack
// macro-oscillator. Only the sixteen original models are registered, in
// their original order. Original notices:
//
// Copyright 2016 Emilie Gillet. (dsp/voice.h, dsp/voice.cc, dsp/envelope.h)
// Copyright 2014 Emilie Gillet. (dsp/fx/low_pass_gate.h)
//
// Author: Emilie Gillet (emilie.o.gillet@gmail.com)
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.
//
// See http://creativecommons.org/licenses/MIT/ for more information.
//
// -----------------------------------------------------------------------------

use super::engine::*;
use super::stm::*;

use super::additive::AdditiveEngine;
use super::bass_drum::BassDrumEngine;
use super::chord::ChordEngine;
use super::fm::FmEngine;
use super::grain::GrainEngine;
use super::hi_hat::HiHatEngine;
use super::modal::ModalEngine;
use super::noise::NoiseEngine;
use super::particle::ParticleEngine;
use super::snare_drum::SnareDrumEngine;
use super::speech::SpeechEngine;
use super::string::StringEngine;
use super::swarm::SwarmEngine;
use super::va::VirtualAnalogEngine;
use super::waveshaping::WaveshapingEngine;
use super::wavetable::WavetableEngine;

pub const NUM_ENGINES: usize = 16;
pub const ENGINE_SPEECH: usize = 7;
const MAX_TRIGGER_DELAY: usize = 8;
const TRIGGER_DELAY: usize = 5;

/// The voice's envelopes, LPG and trigger delay step once per render call
/// with rates made for the native 12-sample block, also when the host calls
/// with 4- or 1-sample sub-blocks (the reference calls the voice that way
/// unchanged, so its envelopes run 3x / 12x faster than the module's).
#[allow(dead_code)]
const QUIRK_VOICE_RATES_PER_CALL: bool = true;

// ------------------------------------------------------------- envelope.h

#[derive(Clone, Copy, Debug, Default)]
pub struct LpgEnvelope {
    vactrol_state: f32,
    gain: f32,
    frequency: f32,
    hf_bleed: f32,
    ramp_up: bool,
}

impl LpgEnvelope {
    pub fn init(&mut self) {
        self.vactrol_state = 0.0;
        self.gain = 1.0;
        self.frequency = 0.5;
        self.hf_bleed = 0.0;
        self.ramp_up = false;
    }
    pub fn trigger(&mut self) {
        self.ramp_up = true;
    }
    pub fn process_ping(&mut self, attack: f32, short_decay: f32, decay_tail: f32, hf: f32) {
        if self.ramp_up {
            self.vactrol_state += attack;
            if self.vactrol_state >= 1.0 {
                self.vactrol_state = 1.0;
                self.ramp_up = false;
            }
        }
        let level = if self.ramp_up { self.vactrol_state } else { 0.0 };
        self.process_lp(level, short_decay, decay_tail, hf);
    }
    pub fn process_lp(&mut self, level: f32, short_decay: f32, decay_tail: f32, hf: f32) {
        let vactrol_error = level - self.vactrol_state;
        let s2 = self.vactrol_state * self.vactrol_state;
        let s4 = s2 * s2;
        let tail = 1.0 - self.vactrol_state;
        let tail_2 = tail * tail;
        let coefficient = if vactrol_error > 0.0 { 0.6 } else { short_decay + (1.0 - s4) * decay_tail };
        self.vactrol_state += coefficient * vactrol_error;
        self.gain = self.vactrol_state;
        self.frequency = 0.003 + 0.3 * s4 + hf * 0.04;
        self.hf_bleed = (tail_2 + (1.0 - tail_2) * hf) * hf * hf;
    }
    pub fn gain(&self) -> f32 {
        self.gain
    }
    pub fn frequency(&self) -> f32 {
        self.frequency
    }
    pub fn hf_bleed(&self) -> f32 {
        self.hf_bleed
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DecayEnvelope {
    value: f32,
}

impl DecayEnvelope {
    pub fn init(&mut self) {
        self.value = 0.0;
    }
    pub fn trigger(&mut self) {
        self.value = 1.0;
    }
    pub fn process(&mut self, decay: f32) {
        self.value *= 1.0 - decay;
    }
    pub fn value(&self) -> f32 {
        self.value
    }
}

// ------------------------------------------------------- low_pass_gate.h

#[derive(Clone, Copy, Debug, Default)]
pub struct LowPassGate {
    previous_gain: f32,
    filter: Svf,
}

impl LowPassGate {
    pub fn init(&mut self) {
        self.previous_gain = 0.0;
        self.filter.init();
    }
    /// The 16-bit output variant: `out` receives the clipped integer values.
    pub fn process_to_i16(&mut self, gain: f32, frequency: f32, hf_bleed: f32, input: &[f32], out: &mut [f32]) {
        let size = input.len();
        self.filter.set_f_q::<FREQUENCY_DIRTY>(frequency, 0.4);
        let mut g = ParameterInterpolator::new(&mut self.previous_gain, gain, size);
        for (o, &x) in out.iter_mut().zip(input.iter()) {
            let s = x * g.next();
            let lp = self.filter.process::<FILTER_MODE_LOW_PASS>(s);
            *o = clip16(1 + (lp + (s - lp) * hf_bleed) as i32) as f32;
        }
    }
}

// ------------------------------------------------------------- voice.h/cc

#[derive(Clone, Copy, Debug, Default)]
pub struct ChannelPostProcessor {
    limiter: Limiter,
    lpg: LowPassGate,
}

impl ChannelPostProcessor {
    pub fn init(&mut self) {
        self.lpg.init();
        self.reset();
    }
    pub fn reset(&mut self) {
        self.limiter.init();
    }
    /// Writes 16-bit-range values (as f32) into `out`.
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        gain: f32,
        bypass_lpg: bool,
        lpg_gain: f32,
        lpg_frequency: f32,
        lpg_hf_bleed: f32,
        input: &mut [f32],
        out: &mut [f32],
    ) {
        if gain < 0.0 {
            self.limiter.process(-gain, input);
        }
        let post_gain = (if gain < 0.0 { 1.0 } else { gain }) * -32767.0;
        if !bypass_lpg {
            self.lpg.process_to_i16(post_gain * lpg_gain, lpg_frequency, lpg_hf_bleed, input, out);
        } else {
            for (o, &x) in out.iter_mut().zip(input.iter()) {
                *o = clip16(1 + (x * post_gain) as i32) as f32;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Patch {
    pub note: f32,
    pub harmonics: f32,
    pub timbre: f32,
    pub morph: f32,
    pub frequency_modulation_amount: f32,
    pub timbre_modulation_amount: f32,
    pub morph_modulation_amount: f32,
    pub engine: i32,
    pub decay: f32,
    pub lpg_colour: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Modulations {
    pub engine: f32,
    pub note: f32,
    pub frequency: f32,
    pub harmonics: f32,
    pub timbre: f32,
    pub morph: f32,
    pub trigger: f32,
    pub level: f32,
    pub frequency_patched: bool,
    pub timbre_patched: bool,
    pub morph_patched: bool,
    pub trigger_patched: bool,
    pub level_patched: bool,
}

#[derive(Clone, Copy, Debug)]
struct PostProcessingSettings {
    already_enveloped: bool,
    out_gain: f32,
    aux_gain: f32,
}

const fn pp(already_enveloped: bool, out_gain: f32, aux_gain: f32) -> PostProcessingSettings {
    PostProcessingSettings { already_enveloped, out_gain, aux_gain }
}

/// Registration table of the sixteen models (envelope flag, out/aux gains;
/// a negative gain means "limit with this pre-gain").
const POST_PROCESSING: [PostProcessingSettings; NUM_ENGINES] = [
    pp(false, 0.8, 0.8),  // virtual analog
    pp(false, 0.7, 0.6),  // waveshaping
    pp(false, 0.6, 0.6),  // 2-op FM
    pp(false, 0.7, 0.6),  // grain / formant
    pp(false, 0.8, 0.8),  // additive
    pp(false, 0.6, 0.6),  // wavetable
    pp(false, 0.8, 0.8),  // chords
    pp(false, -0.7, 0.8), // speech
    pp(false, -3.0, 1.0), // swarm
    pp(false, -1.0, -1.0), // filtered noise
    pp(false, -2.0, 1.0), // particle noise
    pp(true, -1.0, 0.8),  // inharmonic string
    pp(true, -1.0, 0.8),  // modal resonator
    pp(true, 0.8, 0.8),   // kick
    pp(true, 0.8, 0.8),   // snare
    pp(true, 0.8, 0.8),   // hi-hat
];

#[inline]
fn apply_modulations(
    base_value: f32,
    mut modulation_amount: f32,
    use_external_modulation: bool,
    external_modulation: f32,
    use_internal_envelope: bool,
    envelope: f32,
    default_internal_modulation: f32,
    minimum_value: f32,
    maximum_value: f32,
) -> f32 {
    let mut value = base_value;
    modulation_amount *= (modulation_amount.abs() - 0.05).max(0.05);
    modulation_amount *= 1.05;
    let modulation = if use_external_modulation {
        external_modulation
    } else if use_internal_envelope {
        envelope
    } else {
        default_internal_modulation
    };
    value += modulation_amount * modulation;
    constrain(value, minimum_value, maximum_value)
}

pub struct Voice {
    va: VirtualAnalogEngine,
    waveshaping: WaveshapingEngine,
    fm: FmEngine,
    grain: GrainEngine,
    additive: AdditiveEngine,
    wavetable: WavetableEngine,
    chord: ChordEngine,
    speech: SpeechEngine,
    swarm: SwarmEngine,
    noise: NoiseEngine,
    particle: ParticleEngine,
    string: StringEngine,
    modal: ModalEngine,
    bass_drum: BassDrumEngine,
    snare_drum: SnareDrumEngine,
    hi_hat: HiHatEngine,

    engine_quantizer: HysteresisQuantizer2,
    previous_engine_index: i32,
    engine_cv: f32,
    previous_note: f32,
    trigger_state: bool,
    decay_envelope: DecayEnvelope,
    lpg_envelope: LpgEnvelope,
    trigger_delay: DelayLine<MAX_TRIGGER_DELAY>,
    out_post_processor: ChannelPostProcessor,
    aux_post_processor: ChannelPostProcessor,
    rng: Random,
}

impl Voice {
    /// Allocates every model (they share nothing, unlike the original's
    /// shared RAM).
    pub fn new(seed: u32) -> Voice {
        super::stm::init_tables();
        let _ = super::tables::tables();
        let mut v = Voice {
            va: VirtualAnalogEngine::new(),
            waveshaping: WaveshapingEngine::new(),
            fm: FmEngine::new(),
            grain: GrainEngine::new(),
            additive: AdditiveEngine::new(),
            wavetable: WavetableEngine::new(),
            chord: ChordEngine::new(),
            speech: SpeechEngine::new(),
            swarm: SwarmEngine::new(),
            noise: NoiseEngine::new(),
            particle: ParticleEngine::new(),
            string: StringEngine::new(),
            modal: ModalEngine::new(),
            bass_drum: BassDrumEngine::new(),
            snare_drum: SnareDrumEngine::new(),
            hi_hat: HiHatEngine::new(),
            engine_quantizer: HysteresisQuantizer2::default(),
            previous_engine_index: -1,
            engine_cv: 0.0,
            previous_note: 0.0,
            trigger_state: false,
            decay_envelope: DecayEnvelope::default(),
            lpg_envelope: LpgEnvelope::default(),
            trigger_delay: DelayLine::default(),
            out_post_processor: ChannelPostProcessor::default(),
            aux_post_processor: ChannelPostProcessor::default(),
            rng: Random::new(seed),
        };
        v.init();
        v
    }

    /// `Init` minus the engine allocation: back to the power-on state.
    pub fn init(&mut self) {
        self.engine_quantizer.init(NUM_ENGINES as i32, 0.05, true);
        self.previous_engine_index = -1;
        self.engine_cv = 0.0;
        self.out_post_processor.init();
        self.aux_post_processor.init();
        self.decay_envelope.init();
        self.lpg_envelope.init();
        self.trigger_state = false;
        self.previous_note = 0.0;
        self.trigger_delay.init();
    }

    fn engine(&mut self, i: usize) -> &mut dyn Engine {
        match i {
            0 => &mut self.va,
            1 => &mut self.waveshaping,
            2 => &mut self.fm,
            3 => &mut self.grain,
            4 => &mut self.additive,
            5 => &mut self.wavetable,
            6 => &mut self.chord,
            7 => &mut self.speech,
            8 => &mut self.swarm,
            9 => &mut self.noise,
            10 => &mut self.particle,
            11 => &mut self.string,
            12 => &mut self.modal,
            13 => &mut self.bass_drum,
            14 => &mut self.snare_drum,
            _ => &mut self.hi_hat,
        }
    }

    /// Renders `out.len()` (<= 24) frames. Outputs are in 16-bit units, as
    /// the original's frames (divide by 32768 for full scale).
    pub fn render(&mut self, patch: &Patch, modulations: &Modulations, out: &mut [f32], aux: &mut [f32]) {
        let size = out.len().min(MAX_BLOCK_SIZE);
        // Trigger delayed by a few calls (sequencers whose CV lags the gate).
        self.trigger_delay.write(modulations.trigger);
        let trigger_value = self.trigger_delay.read_f(TRIGGER_DELAY as f32);

        let previous_trigger_state = self.trigger_state;
        if !previous_trigger_state {
            if trigger_value > 0.3 {
                self.trigger_state = true;
                if !modulations.level_patched {
                    self.lpg_envelope.trigger();
                }
                self.decay_envelope.trigger();
                self.engine_cv = modulations.engine;
            }
        } else if trigger_value < 0.1 {
            self.trigger_state = false;
        }
        if !modulations.trigger_patched {
            self.engine_cv = modulations.engine;
        }

        let engine_index = self.engine_quantizer.process_base(patch.engine, self.engine_cv);
        let ei = engine_index as usize;

        if engine_index != self.previous_engine_index {
            self.engine(ei).reset();
            self.out_post_processor.reset();
            self.previous_engine_index = engine_index;
        }

        let mut p = EngineParameters::default();
        let rising_edge = self.trigger_state && !previous_trigger_state;
        let note = (modulations.note + self.previous_note) * 0.5;
        self.previous_note = modulations.note;
        let pp_s = POST_PROCESSING[ei];

        p.trigger = if modulations.trigger_patched {
            (if rising_edge { TRIGGER_RISING_EDGE } else { TRIGGER_LOW })
                | (if self.trigger_state { TRIGGER_HIGH } else { TRIGGER_LOW })
        } else {
            TRIGGER_UNPATCHED
        };

        // QUIRK_VOICE_RATES_PER_CALL: native block length, whatever `size` is.
        let block = BLOCK_SIZE as f32;
        let short_decay = (200.0 * block) / SAMPLE_RATE * semitones_to_ratio(-96.0 * patch.decay);
        self.decay_envelope.process(short_decay * 2.0);

        let compressed_level = constrain(1.3 * modulations.level / (0.3 + modulations.level.abs()), 0.0, 1.0);
        p.accent = if modulations.level_patched { compressed_level } else { 0.8 };

        let use_internal_envelope = modulations.trigger_patched;

        p.harmonics = constrain(patch.harmonics + modulations.harmonics, 0.0, 1.0);

        let mut internal_envelope_amplitude = 1.0;
        if ei == ENGINE_SPEECH {
            internal_envelope_amplitude = constrain(2.0 - p.harmonics * 6.0, 0.0, 1.0);
            self.speech.set_prosody_amount(
                if !modulations.trigger_patched || modulations.frequency_patched {
                    0.0
                } else {
                    patch.frequency_modulation_amount
                },
            );
            self.speech.set_speed(
                if !modulations.trigger_patched || modulations.morph_patched { 0.0 } else { patch.morph_modulation_amount },
            );
        }

        let env = self.decay_envelope.value();
        p.note = apply_modulations(
            patch.note + note,
            patch.frequency_modulation_amount,
            modulations.frequency_patched,
            modulations.frequency,
            use_internal_envelope,
            internal_envelope_amplitude * env * env * 48.0,
            1.0,
            -119.0,
            120.0,
        );
        p.timbre = apply_modulations(
            patch.timbre,
            patch.timbre_modulation_amount,
            modulations.timbre_patched,
            modulations.timbre,
            use_internal_envelope,
            env,
            0.0,
            0.0,
            1.0,
        );
        p.morph = apply_modulations(
            patch.morph,
            patch.morph_modulation_amount,
            modulations.morph_patched,
            modulations.morph,
            use_internal_envelope,
            internal_envelope_amplitude * env,
            0.0,
            0.0,
            1.0,
        );

        let mut already_enveloped = pp_s.already_enveloped;
        let mut out_buffer = [0.0f32; MAX_BLOCK_SIZE];
        let mut aux_buffer = [0.0f32; MAX_BLOCK_SIZE];
        let mut rng = self.rng;
        self.engine(ei).render(&p, &mut out_buffer[..size], &mut aux_buffer[..size], &mut already_enveloped, &mut rng);
        self.rng = rng;

        let lpg_bypass = already_enveloped || (!modulations.level_patched && !modulations.trigger_patched);

        if !lpg_bypass {
            let hf = patch.lpg_colour;
            let decay_tail = (20.0 * block) / SAMPLE_RATE * semitones_to_ratio(-72.0 * patch.decay + 12.0 * hf) - short_decay;
            if modulations.level_patched {
                self.lpg_envelope.process_lp(compressed_level, short_decay, decay_tail, hf);
            } else {
                let attack = note_to_frequency(p.note) * block * 2.0;
                self.lpg_envelope.process_ping(attack, short_decay, decay_tail, hf);
            }
        } else {
            self.lpg_envelope.init();
        }

        let (g, f, h) = (self.lpg_envelope.gain(), self.lpg_envelope.frequency(), self.lpg_envelope.hf_bleed());
        self.out_post_processor.process(pp_s.out_gain, lpg_bypass, g, f, h, &mut out_buffer[..size], &mut out[..size]);
        self.aux_post_processor.process(pp_s.aux_gain, lpg_bypass, g, f, h, &mut aux_buffer[..size], &mut aux[..size]);
    }
}
