// Iron fish is MIT licensed, (C) Stijn Kuipers
// Super saw oscillator implementation is MIT licensed (C) Niels J. de Wit

// TO-DO:
// - Create 'Phase' object to keep track of, tick and sample phases
// - Fancy XY Effects
// - Actually optimize
// - regular wavetable oscs
// - mod matrix
// - reverb
// - chorus
// - predelay for mod envelope?
// - longer sequencer
// - record to sequencer
// - omnichord mode playing
// - better arp
// - fv1 emu?

#![allow(dead_code)]

use crate::delay_toys::DelayToy;
use crate::waveguide::Waveguide;

use {
    crate::{makepad_audio_graph::*, makepad_platform::live_atomic::*, makepad_platform::*},
    std::sync::Arc,
};

#[derive(Live, LiveHook, PartialEq, LiveAtomic, Debug, LiveRead)]
pub enum OscType {
    #[pick]
    DPWSawPulse,
    BlampTri,
    Pure,
    SuperSaw,
    HyperSaw,
    HarmonicSeries,
}

#[derive(Live, LiveHook, PartialEq, LiveAtomic, Debug, LiveRead)]
pub enum LFOWave {
    #[pick]
    Saw,
    Sine,
    Pulse,
    Triangle,
}

#[derive(Copy, Clone, Live, LiveHook, PartialEq, LiveAtomic, Debug, LiveRead)]
pub enum RootNote {
    A,
    Asharp,
    B,
    #[pick]
    C,
    Csharp,
    D,
    Dsharp,
    E,
    F,
    Fsharp,
    G,
    Gsharp,
}

#[derive(Copy, Clone, Live, LiveHook, PartialEq, LiveAtomic, Debug, LiveRead)]
pub enum MusicalScale {
    #[pick]
    Minor,
    Major,
    Dorian,
    Pentatonic,
}

#[derive(Live, LiveHook, PartialEq, LiveAtomic, Debug, LiveRead)]
pub enum FilterType {
    #[pick]
    LowPass,
    HighPass,
    BandPass,
    BandReject,
}
/*
struct LaddFilterCoefficients {
    k: f64,
    alpha: f64,
    alpha0: f64,
    sigma: f64,
    bass_comp: f64,
    g: f64,
    subfilter_beta: [f64; 4]
}

impl Default for LaddFilterCoefficients {
    fn default() -> Self {
        Self {
            k: 1.0,
            alpha: 0.0,
            alpha0: 1.0,
            sigma: 1.0,
            bass_comp: 1.0,
            g: 1.0,

            subfilter_beta: [0.0, 0.0, 0.0, 0.0]
        }
    }
}*/

#[derive(Live, LiveHook, LiveRegister, LiveAtomic, Debug, LiveRead, Clone)]
pub struct OscSettings {
    #[live]
    osc_type: U32A<OscType>,
    #[live(0)]
    transpose: i64a,
    #[live(0.0)]
    detune: f32a,
    #[live(0.0)]
    harmonic: f32a,
    #[live(0.0)]
    harmonicenv: f32a,
    #[live(0.0)]
    harmoniclfo: f32a,
}

#[derive(Live, LiveHook, LiveRegister, LiveAtomic, Debug, LiveRead, Clone)]
pub struct SupersawSettings {
    #[live(0.0)]
    spread: f32a,
    #[live(0.0)]
    diffuse: f32a,
}

#[derive(Live, LiveHook, LiveRegister, LiveAtomic, Debug, LiveRead)]
pub struct EnvelopeSettings {
    #[live(0.0)]
    predelay: f32a,
    #[live(0.05)]
    a: f32a,
    #[live(0.0)]
    h: f32a,
    #[live(0.2)]
    d: f32a,
    #[live(0.5)]
    s: f32a,
    #[live(0.2)]
    r: f32a,
}

#[derive(Live, LiveHook, LiveRegister, LiveAtomic, Debug, LiveRead)]
pub struct LFOSettings {
    #[live(0.2)]
    rate: f32a,
    #[live(0)]
    keysync: u32a,
    #[live]
    waveform: U32A<LFOWave>,
}

#[derive(Copy, Clone)]
pub struct LFOState {
    phase: f32,
}

#[derive(Live, LiveHook, LiveRegister, LiveAtomic, Debug, LiveRead)]
pub struct FilterSettings {
    #[live]
    filter_type: U32A<FilterType>,
    #[live(0.5)]
    cutoff: f32a,
    #[live(0.05)]
    resonance: f32a,
    #[live(0.1)]
    envelope_amount: f32a,
    #[live(0.1)]
    lfo_amount: f32a,
    #[live(0.1)]
    touch_amount: f32a,
    #[live(0.0)]
    envelope_curvature: f32a,
}

#[derive(Live, LiveHook, LiveRegister, LiveAtomic, Debug, LiveRead)]
pub struct TouchSettings {
    #[live(0.5)]
    offset: f32a,
    #[live(1.0)]
    scale: f32a,
    #[live(0.5)]
    curve: f32a,
}

#[derive(Live, LiveHook, LiveRegister, LiveAtomic, Debug, LiveRead)]
pub struct BitCrushSettings {
    #[live(false)]
    enable: boola,

    #[live(0.4)]
    amount: f32a,
}

#[derive(Live, LiveHook, LiveRegister, LiveAtomic, Debug, LiveRead)]
pub struct BlurSettings {
    #[live(0.4)]
    size: f32a,
    #[live(0.4)]
    std: f32a,
}


#[derive(Live, LiveHook, LiveRegister, LiveAtomic, Debug, LiveRead)]
pub struct ShadowSettings {
    #[live(0.4)]
    opacity: f32a,
    #[live(2.0)]
    x: f32a,
    #[live(2.0)]
    y: f32a
}

#[derive(Live, LiveHook, LiveRegister, LiveAtomic, Debug, LiveRead)]
pub struct DelaySettings {
    #[live(0.15)]
    delaysend: f32a,
    #[live(0.8)]
    delayfeedback: f32a,
    #[live(0.9)]
    cross: f32a,
    #[live(0.1)]
    difference: f32a,
    #[live(0.7)]
    length: f32a,

}

#[derive(Live, LiveHook, LiveRegister, LiveAtomic, Debug, LiveRead)]
pub struct ArpSettings {
    #[live(true)]
    enabled: boola,
    #[live(0)]
    octaves: i32a,
}

#[derive(Live, LiveHook, LiveRegister, LiveAtomic, Debug, LiveRead)]
pub struct SequencerSettings {
    #[live]
    pub steps: [u32a; 16],

    #[live]
    scale: U32A<MusicalScale>,
    #[live]
    rootnote: U32A<RootNote>,

    #[live(125.0)]
    bpm: f32a,
    #[live(false)]
    playing: boola,

    #[live(0)]
    oneshot: u32a,
    #[live(1)]
    transposewithmidi: u32a,
    #[live(0)]
    polyphoniconeshot: u32a,
}

impl SequencerSettings {
    pub fn get_step(&self, step: usize) -> u32 {
        self.steps[step].get()
    }

    pub fn set_step(&self, step: usize, value: u32) {
        self.steps[step].set(value)
    }
}

#[derive(Live, LiveHook, LiveRegister, LiveAtomic, Debug, LiveRead)]
#[live_ignore]
pub struct IronFishSettings {
    #[live]
    supersaw1: SupersawSettings,
    #[live]
    supersaw2: SupersawSettings,
    #[live]
    osc1: OscSettings,
    #[live]
    osc2: OscSettings,
    #[live]
    subosc: OscSettings,
    #[live]
    lfo: LFOSettings,
    #[live]
    filter1: FilterSettings,
    #[live]
    volume_envelope: EnvelopeSettings,
    #[live]
    mod_envelope: EnvelopeSettings,
    #[live]
    touch: TouchSettings,
    #[live]
    delay: DelaySettings,
    #[live]
    bitcrush: BitCrushSettings,
    #[live]
    pub sequencer: SequencerSettings,
    #[live]
    pub arp: ArpSettings,
    #[live]
    chorus: ChorusSettings,
    #[live]
    reverb: ReverbSettings,
    #[live(48000.0)]
    sample_rate: f32a,
    #[live(0.5)]
    osc_balance: f32a,
    #[live(0.1)]
    sub_osc: f32a,
    #[live(0.0)]
    noise: f32a,
    #[live(0.0)]
    portamento: f32a,

//    #[live]
//  blur: BlurSettings,
//    #[live]
//  shadow: ShadowSettings,
}

#[derive(Copy, Clone)]
pub struct SequencerState {
    currentstep: usize,
    samplesleftinstep: usize,
    currentrootnote: RootNote,
    currentscale: MusicalScale,
}

#[derive(Copy, Clone)]
pub struct ArpState {
    step: usize,
    lastarpnote: u32,
    melody: [u32; 128],
    melodylength: usize,
}

#[derive(Copy, Clone)]
pub struct SuperSawOscillatorState {
    phase: [f32; 7],
    delta_phase: [f32; 7],
    detune: f32,
    mix_main: f32,
    mix_side_bands: f32,
}

#[derive(Copy, Clone)]
pub struct HyperSawOscillatorState {
    phase: [f32; 7],
    delta_phase: [f32; 7],
    dpw: [DPWState; 7],
}

impl HyperSawOscillatorState {
    pub fn get(&mut self, state: HyperSawGlobalState) -> f32 {
        let mut res = 0.0;
        for i in 0..state.new_n_saws {
            res += OscillatorState::poly_saw(self.phase[i], self.delta_phase[i])
                * state.volume_level[i];
            // res +=  self.dpw[i].get3rdorder(self.phase[i], self.delta_phase[i]) * state.volume_level[i];
        }

        return res;
    }

    pub fn tick(&mut self, state: &HyperSawGlobalState) {
        for i in 0..state.new_n_saws {
            self.phase[i] += self.delta_phase[i];
            while self.phase[i] > 1.0 {
                self.phase[i] -= 1.0;
            }
        }
    }

    pub fn set_freq(
        &mut self,
        freq: f32,
        samplerate: f32,
        delta_phase: f32,
        state: &HyperSawGlobalState,
        _update: bool,
    ) {
        for i in 0..7 {
            self.delta_phase[i] = delta_phase * state.freq_multiplier[i];
            let prep = samplerate / (freq * state.freq_multiplier[i]); // / samplerate;
            if !_update {
                self.dpw[i] = DPWState::default();
            }
            self.dpw[i].set_gain1(
                (1.0 / 24.0 * (3.1415 / (2.0 * (3.1415 / prep).sin())).powf(3.0)).powf(1.0 / 3.0),
            );
        }
    }
}
#[derive(Copy, Clone)]
pub struct HyperSawGlobalState {
    volume_level: [f32; 7],
    freq_multiplier: [f32; 7],
    last_spread: f32,
    last_diffuse: f32,
    orig_level: f32,
    f_extra_saws: f32,
    n_extra_saws: usize,
    f_frac_saw: f32,
    f_saws: f32,
    new_n_saws: usize,
}

impl HyperSawGlobalState {
    pub fn recalcsaws(&mut self) {
        self.f_extra_saws = self.last_diffuse * 6.0;
        self.n_extra_saws = (self.f_extra_saws.floor() + 1.0).min(6.0) as usize;
        if self.f_extra_saws <= 0.000001 {
            self.n_extra_saws = 0;
        }
        self.f_frac_saw = self.f_extra_saws - self.f_extra_saws.floor();
        self.f_saws = 1.0 + self.f_extra_saws;
        self.new_n_saws = self.n_extra_saws + 1;
    }

    pub fn downrampfrom1(&mut self, factor: f32, input: f32) -> f32 {
        return 1.0 + ((input - 1.0) * factor);
    }

    pub fn recalclevels(&mut self) {
        let new_level =
            (self.orig_level * self.orig_level) / (self.downrampfrom1(-0.1, self.f_saws));

        for i in 0..self.new_n_saws {
            self.volume_level[i] = 1.0;
        }

        if self.f_frac_saw.abs() > 0.00001 {
            let fract_level = self.f_frac_saw * self.f_frac_saw;
            self.volume_level[self.new_n_saws - 1] = fract_level;
        }

        let mut total = 0.0;

        for i in 0..self.new_n_saws {
            total += self.volume_level[i];
        }

        total = 1.0 / total;

        for i in 0..self.new_n_saws {
            self.volume_level[i] = (self.volume_level[i]) * (total * new_level);
        }

        //   log!("{:?} {} {:?}", std::time::Instant::now(), self.new_n_saws, self.volume_level);
    }

    pub fn recalcfreqs(&mut self) {
        self.freq_multiplier[0] = 1.0;

        for i in 1..self.new_n_saws {
            let detune =
                ((i as f32) / 2.0) * (self.last_spread * (1.0 / 6.0)) * (0.5 / (self.f_saws * 0.5));
            if i & 2 == 0 {
                self.freq_multiplier[i] = 2.0f32.powf(detune);
            } else {
                self.freq_multiplier[i] = 2.0f32.powf(-detune);
            };
        }

        //log!("{:?} {} {:?}", std::time::Instant::now(), self.new_n_saws, self.freq_multiplier);
    }
}

impl Default for HyperSawGlobalState {
    fn default() -> Self {
        Self {
            volume_level: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            freq_multiplier: [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
            last_spread: -1.0,
            last_diffuse: -1.0,
            orig_level: 1.0,
            f_extra_saws: 0.0,
            n_extra_saws: 0,
            f_frac_saw: 0.0,
            f_saws: 0.0,
            new_n_saws: 0,
        }
    }
}

impl Default for HyperSawOscillatorState {
    fn default() -> Self {
        Self {
            phase: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            delta_phase: [1.414, 1.732, 2.236, 2.646, 3.317, 3.606, 4.123], // square root of primes: 2, 3, 5, 7, 11, 13, 17 (FIXME: initialize in code)
            dpw: [Default::default(); 7],
        }
    }
}

#[derive(Copy, Clone)]
pub struct DPWState {
    x1: f32,
    x2: f32,
}

impl Default for DPWState {
    fn default() -> Self {
        Self { x2: 0.0, x1: 0.0 }
    }
}

impl DPWState {
    pub fn get2ndorder(&mut self, t: f32, dt: f32) -> f32 {
        let x = 2. * t - 1.;
        let x0 = x * x;
        let y = (x0 - self.x1) / (4. * dt);
        self.x1 = x0;
        return y;
    }
    pub fn get3rdorder(&mut self, t: f32, dt: f32) -> f32 {
        let x = 2. * t - 1.;
        let x0 = x * x * x - x;
        let y = ((x0 - self.x1) - (self.x1 - self.x2)) / (24. * dt * dt);
        self.x2 = self.x1;
        self.x1 = x0;
        return y;
    }

    pub fn set_gain1(&mut self, _newgain: f32) {}
}

#[derive(Copy, Clone)]
pub struct DPWStateOld {
    dpw_gain1: f32,
    dpw_gain2: f32,
    dpw_diff_b: [f32; 4],
    dpw_diff_b_write_index: i8, // diffB write index
    dpw_init_countdown: i8,
}

impl DPWStateOld {
    pub fn set_gain1(&mut self, newgain: f32) {
        self.dpw_gain1 = newgain;
    }

    pub fn get(&mut self, phase: f32) -> f32 {
        let triv = 2.0 * phase - 1.0;
        let sqr = triv * triv;

        let poly = sqr * sqr - 2.0 * sqr;

        self.dpw_diff_b[self.dpw_diff_b_write_index as usize] = poly;
        self.dpw_diff_b_write_index = self.dpw_diff_b_write_index + 1;

        if self.dpw_diff_b_write_index == 4 {
            self.dpw_diff_b_write_index = 0
        };

        if self.dpw_init_countdown > 0 {
            self.dpw_init_countdown = self.dpw_init_countdown - 1;

            return poly;
        }

        let mut tmp_a = [0.0, 0.0, 0.0, 0.0];
        let mut dbr = self.dpw_diff_b_write_index - 1;
        if dbr < 0 {
            dbr = 3
        }

        for i in 0..4 {
            tmp_a[i] = self.dpw_diff_b[dbr as usize];
            dbr = dbr - 1;
            if dbr < 0 {
                dbr = 3;
            }
        }

        let gain = self.dpw_gain1;
        tmp_a[0] = gain * (tmp_a[0] - tmp_a[1]);
        tmp_a[1] = gain * (tmp_a[1] - tmp_a[2]);
        tmp_a[2] = gain * (tmp_a[2] - tmp_a[3]);

        tmp_a[0] = gain * (tmp_a[0] - tmp_a[1]);
        tmp_a[1] = gain * (tmp_a[1] - tmp_a[2]);

        tmp_a[0] = gain * (tmp_a[0] - tmp_a[1]);

        return tmp_a[0] * self.dpw_gain2 * 0.005; //* self.dpw_gain;
    }
}

impl Default for DPWStateOld {
    fn default() -> Self {
        Self {
            dpw_gain1: 1.0,
            dpw_gain2: 1.0,
            dpw_init_countdown: 4,
            dpw_diff_b: [3.0, 3.0, 3.0, 3.0],
            dpw_diff_b_write_index: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct OscillatorState {
    phase: f32,
    delta_phase: f32,
    supersaw: SuperSawOscillatorState,
    hypersaw: HyperSawOscillatorState,
    dpw: DPWState,
}

#[derive(Copy, Clone)]
pub struct SubOscillatorState {
    phase: f32,
    delta_phase: f32,
}

impl SubOscillatorState {
    fn get(self) -> f32 {
        return (self.phase * 6.28318530718).sin();
    }

    fn tick(&mut self) {
        self.phase += self.delta_phase;
        while self.phase > 1.0 {
            self.phase -= 1.0
        }
    }

    pub fn set_note(&mut self, note: f32, samplerate: f32) {
        let freq = 440.0 * f32::powf(2.0, ((note as f32) - 69.0 - 12.0) / 12.0);
        self.delta_phase = (freq * 0.5) / samplerate;
    }
}

impl OscillatorState {
    fn blamp(&mut self, t: f32, dt: f32) -> f32 {
        let mut y = 0.0;
        if 0.0 <= t && t < 2.0 * dt {
            let x = t / dt;
            let u = 2.0 - x;
            let u2 = u * u;
            let u4 = u2 * u2;
            y -= u4;
            if t < dt {
                let v = 1.0 - x;
                let v2 = v * v;
                let v5 = v * v2 * v2;
                y += 4.0 * v5;
            }
        }
        return y * dt / 15.0;
    }

    fn blamp_triangle(&mut self) -> f32 {
        let mut tri = 2.0 * (2.0 * self.phase - 1.0).abs() - 1.0;

        tri += self.blamp(self.phase, self.delta_phase);
        tri += self.blamp(1.0 - self.phase, self.delta_phase);
        let mut t2 = self.phase + 0.5;
        t2 -= t2.floor();
        tri -= self.blamp(t2, self.delta_phase);
        tri -= self.blamp(1.0 - t2, self.delta_phase);
        return tri;
    }

    fn pure(&mut self) -> f32 {
        return (self.phase * 6.28318530718).sin();
    }

    fn harmonic(&mut self, parameter: f32) -> f32 {
        let h = parameter * 16.0;
        let f_h = h.floor();
        let n_h = f_h + 1.0;
        let n_f = h - f_h;

        let phasepi = self.phase * 6.28318530718;
        let p1 = phasepi * n_h;
        let p2 = phasepi * (n_h + 1.0);

        return (p1).sin() * (1.0 - n_f) + (p2).sin() * n_f;
    }

    /* PolyBLEP saw impl. */

    fn poly_saw_bitwise_or_zero(value: f32) -> u32 {
        let result = (value as u32) | 0;
        return result;
    }

    // http://www.acoustics.hut.fi/publications/papers/smc2010-phaseshaping/
    fn poly_saw_blep(point: f32, dt: f32) -> f32 {
        if point < dt {
            let mut squared = (point / dt) - 1.0;
            squared *= squared;
            return -squared;
        } else if point > (1.0 - dt) {
            let mut squared = ((point - 1.0) / dt) + 1.0;
            squared *= squared;
            return squared;
        } else {
            return 0.0;
        }
    }

    pub fn poly_saw(phase: f32, pitch /* delta_phase */: f32) -> f32 {
        let mut p1 = phase + 0.5;
        p1 -= Self::poly_saw_bitwise_or_zero(p1) as f32;

        let mut saw = 2.0 * p1 - 1.0;
        saw -= Self::poly_saw_blep(p1, pitch);

        return saw;
    }

    /* begin of supersaw specific impl. */

    // values adapted from measurements done with actual synthesizer
    fn sps_calc_mix(&mut self, mix: f32) {
        // FIXME: here I would assert that mix is [0..1]
        self.supersaw.mix_main = -0.55366 * mix + 0.99785;
        self.supersaw.mix_side_bands = -0.73764 * powf(mix, 2.0) + 1.2841 * mix + 0.044372;
    }

    fn sps_pure(&mut self, phase_idx: usize) -> f32 {
        return (self.supersaw.phase[phase_idx] * 6.28318530718).cos();
    }

    fn sps_tick(&mut self) {
        for n in 0..7 {
            self.supersaw.phase[n] += self.supersaw.delta_phase[n];
            while self.supersaw.phase[n] > 1.0 {
                self.supersaw.phase[n] -= 1.0;
            }
        }
    }

    fn supersaw(&mut self) -> f32 {
        let main_band = Self::poly_saw(self.supersaw.phase[0], self.supersaw.delta_phase[0]);
        // main_band -= self.sps_pure(0);

        let mut side_bands = 0.0;
        for n in 1..7 {
            side_bands += Self::poly_saw(self.supersaw.phase[n], self.supersaw.delta_phase[n]);
            if n < 6 {
                side_bands -= self.sps_pure(n); // subtract sin. from any but the highest freq. band
            }
        }

        return main_band * self.supersaw.mix_main + side_bands * self.supersaw.mix_side_bands;
    }

    /* end of supersaw specific impl. */

    fn tick(&mut self, hyper_global_state: &HyperSawGlobalState) {
        // internal single phase
        self.phase += self.delta_phase;
        while self.phase > 1.0 {
            self.phase -= 1.0;
        }

        // complex osc. phases
        self.sps_tick();
        self.hypersaw.tick(&hyper_global_state);
    }

    fn get(
        &mut self,
        settings: &OscSettings,
        _samplerate: f32,
        hyper: HyperSawGlobalState,
        env: f32,
        lfo: f32,
    ) -> f32 {
        match settings.osc_type.get() {
            OscType::Pure => self.pure(),
            OscType::DPWSawPulse => Self::poly_saw(self.phase, self.delta_phase), // FIXME: reinstate DPW osc.
            //OscType::TrivialSaw => self.trivialsaw(),
            OscType::BlampTri => self.blamp_triangle(),
            OscType::SuperSaw => self.supersaw(),
            OscType::HyperSaw => self.hypersaw.get(hyper),
            OscType::HarmonicSeries => self.harmonic(
                (settings.harmonic.get()
                    + env * settings.harmonicenv.get()
                    + lfo * settings.harmoniclfo.get())
                .clamp(0.0, 1.0),
            ),
        }
    }

    fn set_note(
        &mut self,
        note: f32,
        samplerate: f32,
        settings: &OscSettings,
        supersaw: &SupersawSettings,
        hypersaw: &HyperSawGlobalState,
        sps_detune_tab: &[f32; 1024],
        _update: bool,
    ) {
        let freq = (440.0 / 6.28318530718)
            * f32::powf(
                2.0,
                ((note as f32) - 69.0 + settings.transpose.get() as f32 + settings.detune.get())
                    / 12.0,
            );
        self.delta_phase = (6.28318530718 * freq) / samplerate;

        match settings.osc_type.get() {
            OscType::Pure | OscType::BlampTri => {}
            OscType::DPWSawPulse => {
                if !_update {
                    self.dpw = DPWState::default();
                }

                let prep = samplerate / freq;
                self.dpw.set_gain1(
                    (1.0 / 24.0 * (3.1415 / (2.0 * (3.1415 / prep).sin())).powf(3.0))
                        .powf(1.0 / 3.0),
                );
            }
            OscType::HyperSaw => {
                self.hypersaw
                    .set_freq(freq, samplerate, self.delta_phase, &hypersaw, _update);
            }
            OscType::HarmonicSeries => {}
            OscType::SuperSaw => {
                // look up detune base (interpolated)
                let detune = supersaw.spread.get();
                let detune_idx_lo = (detune * (1023.0 - 1.0)) as usize;
                let detune_lo = sps_detune_tab[detune_idx_lo];
                let detune_hi = sps_detune_tab[detune_idx_lo + 1];
                self.supersaw.detune = detune_lo + (detune_hi - detune_lo) * detune;

                // set main & side band gains
                self.sps_calc_mix(supersaw.diffuse.get());

                // lazily initialiizing here (constants courtesy of Alex Shore, the better sounding set of the 2 I have in FM. BISON)
                // reference: https://github.com/bipolaraudio/FM-BISON/blob/master/literature/Supersaw%20thesis.pdf
                let sps_coeffs: [f32; 6] = [
                    -0.11002313,
                    -0.06288439,
                    -0.03024148,
                    0.02953130,
                    0.06216538,
                    0.10745242,
                ];

                self.supersaw.delta_phase[0] = self.delta_phase;

                for n in 1..7 {
                    // calculate & set sideband phase deltas
                    let offs = self.supersaw.detune * sps_coeffs[n - 1];
                    let freq_offs = freq * offs;
                    let detuned_freq = freq + freq_offs;
                    self.supersaw.delta_phase[n] = (6.28318530718 * detuned_freq) / samplerate;
                }
            }
        }
    }
}

impl Default for SubOscillatorState {
    fn default() -> Self {
        Self {
            phase: 0.0,
            delta_phase: 0.0001,
        }
    }
}

impl Default for ArpState {
    fn default() -> Self {
        Self {
            step: 0,
            lastarpnote: 0,
            melody: [0u32; 128],
            melodylength: 0,
        }
    }
}

impl Default for LFOState {
    fn default() -> Self {
        Self { phase: 0.0 }
    }
}

impl Default for SuperSawOscillatorState {
    fn default() -> Self {
        Self {
            phase: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            delta_phase: [1.414, 1.732, 2.236, 2.646, 3.317, 3.606, 4.123], // square root of primes: 2, 3, 5, 7, 11, 13, 17 (FIXME: initialize in code)
            detune: 0.0,
            mix_main: 1.0,
            mix_side_bands: 0.0,
        }
    }
}

impl Default for OscillatorState {
    fn default() -> Self {
        Self {
            phase: 0.0,
            delta_phase: 0.0,
            supersaw: Default::default(),
            hypersaw: Default::default(),
            dpw: Default::default(),
        }
    }
}

impl Default for FilterState {
    fn default() -> Self {
        Self {
            hp: 0.0,
            bp: 0.0,
            lp: 0.0,
            gamma: 0.0,
            fc: 0.0,
            phi: 0.0,
            damp: 0.0,
        }
    }
}

#[derive(PartialEq, Copy, Clone)]
enum EnvelopePhase {
    Idle,
    Predelay,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
    FastRelease,
}

#[derive(Live, LiveHook, LiveRegister, LiveAtomic, Debug, LiveRead)]
pub struct ChorusSettings {
    #[live(0.1)]
    mindelay: f32a,
    #[live(0.4)]
    moddepth: f32a,
    #[live(0.3)]
    rate: f32a,
    #[live(0.4)]
    phasediff: f32a,
    #[live(0.5)]
    mix: f32a,
    #[live(0.0)]
    feedback: f32a,
}

#[derive(Live, LiveHook, LiveRegister, LiveAtomic, Debug, LiveRead)]
pub struct ReverbSettings {
    #[live(0.00)]
    mix: f32a,
    #[live(0.04)]
    feedback: f32a,
}

#[derive(Copy, Clone)]
pub struct SmoothVal {
    current: f32,
    target: f32,
    rate: f32,
}

impl SmoothVal {
    pub fn get(&mut self, target: f32) -> f32 {
        self.target = target;
        self.current += (self.target - self.current) * self.rate;
        return self.current;
    }

    pub fn ratebased(rate: f32) -> Self {
        Self {
            current: 0.0,
            target: 0.0,
            rate: rate,
        }
    }
}

impl Default for SmoothVal {
    fn default() -> Self {
        Self {
            current: 0.0,
            target: 0.0,
            rate: 0.1,
        }
    }
}

#[derive(Clone)]
pub struct ChorusState {
    lines: [Waveguide; 6],
    phase: f32,
    linephase: [f32; 6],
    dphase: f32,
    feedbacksmooth: SmoothVal,
    mixsmooth: SmoothVal,
    phasediffsmooth: SmoothVal,
    mindepthsmooth: SmoothVal,
    moddepthsmooth: SmoothVal,
}

impl Default for ChorusState {
    fn default() -> Self {
        Self {
            lines: std::array::from_fn(|_| Waveguide::default()),
            phase: 0.0,
            linephase: [0.0; 6],
            dphase: 0.0,
            feedbacksmooth: Default::default(),
            mixsmooth: Default::default(),
            phasediffsmooth: Default::default(),
            mindepthsmooth: SmoothVal::ratebased(0.01),
            moddepthsmooth: SmoothVal::ratebased(0.01),
        }
    }
}

impl ChorusState {
    pub fn apply_chorus(
        &mut self,
        buffer: &mut AudioBuffer,
        settings: &ChorusSettings,
        sample_rate: f32,
    ) {
        if settings.mix.get() == 0.0 {
            return;
        }

        let frame_count = buffer.frame_count();
        let (left, right) = buffer.stereo_mut();

        let lfofreq = 0.5 * (2.0).powf(settings.rate.get() * 8.0 - 4.0);
        let lfodphase = 1.0 / (sample_rate / lfofreq);

        self.phase += lfodphase * frame_count as f32;

        if self.phase > 6.283 {
            self.phase = self.phase - 6.283;
        };
        let mindelay = self
            .mindepthsmooth
            .get(sample_rate * 0.030 * settings.mindelay.get())
            + 1.0;
        let moddepth = self
            .moddepthsmooth
            .get(settings.moddepth.get() * sample_rate * 0.050);
        let phasediff = self.phasediffsmooth.get(settings.phasediff.get());
        for i in 0..6 {
            self.linephase[i] =
                mindelay + ((self.phase + (i as f32) * phasediff).sin() + 1.0) * moddepth;
        }
        let fb = self.feedbacksmooth.get(settings.feedback.get()) * 0.86;
        let mix = self.mixsmooth.get(settings.mix.get());
        let imix = 1.0 - mix;
        for i in 0..frame_count {
            let l1 = self.lines[0].feed(left[i], fb, self.linephase[0]);
            let l2 = self.lines[1].feed(right[i], fb, self.linephase[1]);
            let l3 = self.lines[2].feed(left[i], fb, self.linephase[2]);
            let l4 = self.lines[3].feed(right[i], fb, self.linephase[3]);
            let l5 = self.lines[4].feed(left[i], fb, self.linephase[4]);
            let l6 = self.lines[5].feed(right[i], fb, self.linephase[5]);

            left[i] = mix * (l1 + l3 + l5) + left[i] * imix;
            right[i] = mix * (l2 + l4 + l6) + right[i] * imix;
        }
    }
}

#[derive(Clone)]
pub struct ReverbState {
    toy: DelayToy,
}

impl Default for ReverbState {
    fn default() -> Self {
        Self {
            toy: Default::default(),
        }
    }
}

#[derive(Copy, Clone)]
struct EnvelopeState {
    phase: EnvelopePhase,
    delta_value: f32,
    current_value: f32,
    target_value: f32,
    state_time: i32,
}

impl Default for EnvelopeState {
    fn default() -> Self {
        Self {
            phase: EnvelopePhase::Idle,
            delta_value: 0.0,
            current_value: 0.0,
            target_value: 0.0,
            state_time: 0,
        }
    }
}

impl EnvelopeState {
    fn get_n(&mut self, settings: &EnvelopeSettings, samplerate: f32, total: usize) -> f32 {
        let mut res = 0.0;
        for _ in 0..total {
            res = self.get(settings, samplerate);
        }
        return res;
    }
    fn get(&mut self, settings: &EnvelopeSettings, samplerate: f32) -> f32 {
        self.current_value = self.current_value + self.delta_value;
        self.state_time = self.state_time - 1;
        if self.state_time < -1 {
            self.state_time = -1;
        }
        if self.state_time == 0 {
            match self.phase {
                EnvelopePhase::Attack => {
                    if settings.h.get() != 0.0 {
                        self.phase = EnvelopePhase::Hold;
                        self.delta_value = 0.0;
                        self.current_value = 1.0;
                        self.target_value = 1.0;
                        self.state_time =
                            EnvelopeState::nicerange(settings.h.get(), samplerate) as i32;
                    } else {
                        self.phase = EnvelopePhase::Decay;
                        let sustainlevel = settings.s.get() * settings.s.get();
                        self.delta_value = -(1.0 - sustainlevel)
                            / EnvelopeState::nicerange(settings.d.get(), samplerate);
                        self.current_value = 1.0;
                        self.target_value = sustainlevel;
                        self.state_time =
                            EnvelopeState::nicerange(settings.d.get(), samplerate) as i32;
                    }
                }

                EnvelopePhase::Hold => {
                    let sustainlevel = settings.s.get() * settings.s.get();
                    self.phase = EnvelopePhase::Decay;
                    self.delta_value = -(1.0 - settings.s.get())
                        / EnvelopeState::nicerange(settings.d.get(), samplerate);
                    self.current_value = 1.0;
                    self.target_value = sustainlevel;
                    self.state_time = EnvelopeState::nicerange(settings.d.get(), samplerate) as i32;
                }

                EnvelopePhase::Decay => {
                    self.phase = EnvelopePhase::Sustain;
                    self.delta_value = 0.0;
                    let sustainlevel = settings.s.get() * settings.s.get();
                    self.current_value = sustainlevel;
                    self.target_value = sustainlevel;
                    self.state_time = -1;
                }

                EnvelopePhase::FastRelease | EnvelopePhase::Release => {
                    self.phase = EnvelopePhase::Idle;
                    self.delta_value = 0.0;
                    self.current_value = 0.0;
                    self.target_value = 0.0;
                    self.state_time = -1;
                }
                EnvelopePhase::Predelay => {
                    self.phase = EnvelopePhase::Attack;
                    self.delta_value = (1.0 - self.current_value)
                        / EnvelopeState::nicerange(settings.a.get(), samplerate);
                    self.state_time = EnvelopeState::nicerange(settings.a.get(), samplerate) as i32;
                    self.target_value = 1.0;
                }

                _ => {}
            }
        } else {
            if self.phase == EnvelopePhase::Sustain {
                let sustainlevel = settings.s.get() * settings.s.get();
                self.current_value = sustainlevel;
                self.target_value = sustainlevel;
            }
        }
        return self.current_value;
    }

    fn trigger_on(&mut self, _velocity: f32, settings: &EnvelopeSettings, samplerate: f32) {
        if settings.predelay.get() != 0.0 {
            if self.phase != EnvelopePhase::Idle {
                self.phase = EnvelopePhase::FastRelease;
                self.target_value = 0.0;
                self.delta_value = -self.current_value / 100.0;
                self.state_time = 100;
                return;
            }
            self.delta_value = 0.0;
            self.current_value = 0.0;
            self.target_value = 0.0;
            self.phase = EnvelopePhase::Predelay;
            self.state_time = EnvelopeState::nicerange(settings.predelay.get(), samplerate) as i32;
            return;
        };

        self.phase = EnvelopePhase::Attack;
        self.delta_value =
            (1.0 - self.current_value) / (EnvelopeState::nicerange(settings.a.get(), samplerate));
        self.state_time = EnvelopeState::nicerange(settings.a.get(), samplerate) as i32;
        self.target_value = 1.0;
    }

    fn nicerange(input: f32, samplerate: f32) -> f32 {
        let inputexp = input.powf(0.54);
        let result = 64.0 * ((samplerate * 6.0) / 64.0).powf(inputexp);
        return result;
    }

    fn trigger_off(&mut self, _velocity: f32, settings: &EnvelopeSettings, samplerate: f32) {
        match self.phase {
            EnvelopePhase::Attack
            | EnvelopePhase::Decay
            | EnvelopePhase::Hold
            | EnvelopePhase::Sustain => {
                self.phase = EnvelopePhase::Release;
                self.target_value = 0.0;
                self.delta_value =
                    -self.current_value / EnvelopeState::nicerange(settings.r.get(), samplerate);
                self.state_time = EnvelopeState::nicerange(settings.r.get(), samplerate) as i32;
            }
            _ => {}
        }
    }
}

#[derive(Copy, Clone)]
struct FilterState {
    hp: f32,
    bp: f32,
    lp: f32,
    phi: f32,
    gamma: f32,
    fc: f32,
    damp: f32,
}

impl FilterState {
    fn pump(&mut self, input: f32) {
        self.bp = self.phi * self.hp + self.bp;
        self.lp = self.phi * self.bp + self.lp;
        self.hp = input - self.lp - self.gamma * self.bp;
    }

    fn get_lp(&mut self, input: f32) -> f32 {
        self.pump(input);
        return self.lp;
    }

    fn get_bp(&mut self, input: f32) -> f32 {
        self.pump(input);
        return self.bp;
    }

    fn get_br(&mut self, input: f32) -> f32 {
        self.pump(input);
        return input - self.bp;
    }

    fn get_hp(&mut self, input: f32) -> f32 {
        self.pump(input);
        return self.hp;
    }

    fn get(&mut self, input: f32, settings: &FilterSettings) -> f32 {
        match settings.filter_type.get() {
            FilterType::LowPass => self.get_lp(input),
            FilterType::HighPass => self.get_hp(input),
            FilterType::BandPass => self.get_bp(input),
            FilterType::BandReject => self.get_br(input),
        }
    }

    fn set_cutoff(
        &mut self,
        settings: &FilterSettings,
        envelope: f32,
        _sample_rate: f32,
        touch: f32,
        lfo: f32,
    ) {
        self.fc = (settings.cutoff.get()
            + touch * settings.touch_amount.get()
            + lfo * settings.lfo_amount.get() * 0.35
            + envelope * settings.envelope_amount.get() * 0.5)
            .clamp(0.0, 1.0);
        self.fc *= self.fc * 0.5;
        self.damp = 1.0 - settings.resonance.get();
        let preclamp = 2.0 * ((3.1415 * self.fc).sin());
        self.phi = (preclamp).clamp(0.0, 1.0);
        self.gamma = (2.0 * self.damp).clamp(0.0, 1.0);
    }
}
/*
struct GriesingerReverb {
    lp_: f32,
    diffusion_: f32,
    dphase1: f32,
    dphase2: f32,
    writepos: i32,
    buffer: [f32; 96000]
}*/
/*
impl GriesingerReverb {
    pub fn init(&mut self, sample_rate: f32) {
        self.dphase1 = 0.5 / sample_rate;
        self.dphase2 = 0.3 / sample_rate;
        self.lp_ = 0.7;
        self.diffusion_ = 0.625;
    }

    pub fn fill_buffer(&mut self, buffer: &mut AudioBuffer) {

        for i in 0..buffer.frame_count() {
            let output = 0.0;
            buffer.left[i] += output as f32;
            buffer.right[i] += output as f32;
        }
    }
}
*/

#[derive(Copy, Clone)]
pub struct IronFishVoice {
    osc1: OscillatorState,
    osc2: OscillatorState,
    subosc: SubOscillatorState,
    filter1: FilterState,
    volume_envelope: EnvelopeState,
    mod_envelope: EnvelopeState,
    current_note: i16,
    current_notefreq: f32,
    seed: u32,
    fromnote: f32,
    tonote: f32,
    notetime: f32,
    notetimetotal: f32,
    //sequencer: SequencerState
}

fn random_bit(seed: &mut u32) -> u32 {
    *seed = seed.overflowing_add((seed.overflowing_mul(*seed)).0 | 5).0;
    return *seed >> 31;
}

fn random_f32(seed: &mut u32) -> f32 {
    let mut out = 0;
    for _ in 0..32 {
        out |= random_bit(seed);
        out <<= 1;
    }
    out as f32 / std::u32::MAX as f32
}

impl IronFishVoice {
    pub fn active(&mut self) -> i16 {
        if self.volume_envelope.phase == EnvelopePhase::Idle {
            return -1;
        }

        return self.current_note;
    }

    pub fn note_off(&mut self, _b1: u8, b2: u8, settings: &IronFishSettings) {
        let velocity = (b2 as f32) / 127.0;
        self.volume_envelope.trigger_off(
            velocity,
            &settings.volume_envelope,
            settings.sample_rate.get(),
        );
        self.mod_envelope
            .trigger_off(velocity, &settings.mod_envelope, settings.sample_rate.get());
    }

    pub fn update_note(
        &mut self,
        settings: &IronFishSettings,
        h: &IronFishGlobalVoiceState,
        sps_detune_tab: &[f32; 1024],
        update: bool,
    ) {
        self.osc1.set_note(
            self.current_notefreq,
            settings.sample_rate.get(),
            &settings.osc1,
            &settings.supersaw1,
            &h.hypersaw1,
            sps_detune_tab,
            update,
        );
        self.osc2.set_note(
            self.current_notefreq,
            settings.sample_rate.get(),
            &settings.osc2,
            &settings.supersaw2,
            &h.hypersaw2,
            sps_detune_tab,
            update,
        );
        self.subosc
            .set_note(self.current_notefreq, settings.sample_rate.get());
    }

    pub fn note_on(
        &mut self,
        b1: u8,
        prev: u8,
        b2: u8,
        settings: &IronFishSettings,
        h: &IronFishGlobalVoiceState,
        sps_detune_tab: &[f32; 1024],
    ) {
        let velocity = (b2 as f32) / 127.0;

        if settings.portamento.get() > 0.0
        //&& prev < 128
        {
            self.notetimetotal =
                EnvelopeState::nicerange(settings.portamento.get(), settings.sample_rate.get());
            self.notetime = self.notetimetotal;
            self.fromnote = prev as f32;
            self.tonote = b1 as f32;
        } else {
            self.fromnote = b1 as f32;
            self.tonote = b1 as f32;
            self.notetime = 0.0;
            self.notetimetotal = 0.0;
        }

        self.volume_envelope.trigger_on(
            velocity,
            &settings.volume_envelope,
            settings.sample_rate.get(),
        );
        self.mod_envelope
            .trigger_on(velocity, &settings.mod_envelope, settings.sample_rate.get());
        self.current_notefreq = self.fromnote;
        self.current_note = b1 as i16;
        self.update_note(settings, h, sps_detune_tab, false);
    }

    pub fn compute_one(
        &mut self,
        state: &IronFishGlobalVoiceState,
        settings: &IronFishSettings,
        _touch: f32,
        lfo: f32,
        osc1_gain: f32,
        osc2_gain: f32,
        mod_envelope: f32,
    ) -> f32 {
        // update phases (all are free running, more or lesss)
        self.subosc.tick();
        self.osc1.tick(&state.hypersaw1);
        self.osc2.tick(&state.hypersaw2);

        let volume_envelope = self
            .volume_envelope
            .get(&settings.volume_envelope, settings.sample_rate.get());

        // sample
        let sub = self.subosc.get();
        let osc1 = self.osc1.get(
            &settings.osc1,
            settings.sample_rate.get(),
            state.hypersaw1,
            mod_envelope,
            lfo,
        );
        let osc2 = self.osc2.get(
            &settings.osc2,
            settings.sample_rate.get(),
            state.hypersaw2,
            mod_envelope,
            lfo,
        );
        let noise = random_f32(&mut self.seed) * 2.0 - 1.0;

        // mix signal
        let oscinput = osc1 * osc1_gain
            + osc2 * osc2_gain
            + settings.sub_osc.get() * sub
            + noise * settings.noise.get();

        // apply filter
        let filter = self.filter1.get(oscinput, &settings.filter1);

        // apply envelope
        let output = volume_envelope * filter;

        return output;
    }

    pub fn updatenote(
        &mut self,
        frame_count: usize,
        settings: &IronFishSettings,
        h: &IronFishGlobalVoiceState,
        sps_detune_tab: &[f32; 1024],
    ) {
        if self.notetime > 0.0 {
            self.notetime -= frame_count as f32;
            if self.notetime < 0.0 {
                self.notetime = 0.0
            };
            let d = self.notetime / self.notetimetotal;
            self.current_notefreq = self.tonote + (self.fromnote - self.tonote) * d;
            //log!("up - {}", self.current_notefreq);
            self.update_note(settings, h, sps_detune_tab, true);
        }
    }

    pub fn fill_buffer(
        &mut self,
        mix_buffer: &mut AudioBuffer,
        startidx: usize,
        frame_count: usize,
        display_buffer: Option<&mut AudioBuffer>,
        settings: &IronFishSettings,
        state: &IronFishGlobalVoiceState,
        touch: f32,
        lfo: f32,
        sps_detune_tab: &[f32; 1024],
    ) {
        let (left, right) = mix_buffer.stereo_mut();

        // FIXME: like many parameters this one is also not interpolated per sample; I have an idea for a proper interpolation object
        //        and have used it before but right now I'm just going to avoid those sqrt() calls per sample and pray nobody touches that slider mid-note
        let balance = settings.osc_balance.get();
        let osc1_gain = (1.0 - balance).sqrt();
        let osc2_gain = balance.sqrt();

        if let Some(display_buffer) = display_buffer {
            let (left_disp, right_disp) = display_buffer.stereo_mut();
            let mut remaining = frame_count;
            let mut startidxmut = startidx;
            while remaining > 0 {
                let proc = remaining.min(8);
                let mod_envelope = self.mod_envelope.get_n(
                    &settings.mod_envelope,
                    settings.sample_rate.get(),
                    proc,
                );

                // set up filter
                self.filter1.set_cutoff(
                    &settings.filter1,
                    mod_envelope,
                    settings.sample_rate.get(),
                    touch,
                    lfo,
                );
                self.updatenote(proc, settings, state, sps_detune_tab);
                for i in startidxmut..proc + startidxmut {
                    let output = self.compute_one(
                        state,
                        &settings,
                        touch,
                        lfo,
                        osc1_gain,
                        osc2_gain,
                        mod_envelope,
                    ) * (6.28 * 0.02);
                    left_disp[i] = output as f32;
                    right_disp[i] = output as f32;
                    left[i] += output as f32;
                    right[i] += output as f32;
                }
                startidxmut += proc;
                remaining -= proc;
            }
        } else {
            let mut remaining = frame_count;
            let mut startidxmut = startidx;
            while remaining > 0 {
                let proc = remaining.min(8);
                let mod_envelope = self.mod_envelope.get_n(
                    &settings.mod_envelope,
                    settings.sample_rate.get(),
                    proc,
                );

                // set up filter
                self.filter1.set_cutoff(
                    &settings.filter1,
                    mod_envelope,
                    settings.sample_rate.get(),
                    touch,
                    lfo,
                );
                self.updatenote(proc, settings, state, sps_detune_tab);
                for i in startidxmut..proc + startidxmut {
                    let output = self.compute_one(
                        state,
                        &settings,
                        touch,
                        lfo,
                        osc1_gain,
                        osc2_gain,
                        mod_envelope,
                    ) * (6.28 * 0.02);
                    left[i] += output as f32;
                    right[i] += output as f32;
                }
                startidxmut += proc;
                remaining -= proc;
            }
        }
    }
}

#[derive(Default)]
pub struct IronFishGlobalVoiceState {
    hypersaw1: HyperSawGlobalState,
    hypersaw2: HyperSawGlobalState,
}

pub struct IronFishState {
    //from_ui: FromUIReceiver<FromUI>,
    //to_ui: ToUISender<ToUI>,
    display_buffers: Vec<Option<AudioBuffer>>,
    pub settings: Arc<IronFishSettings>,
    voices: [IronFishVoice; 16],
    activemidinotes: [bool; 256],
    activeinternalnotes: [bool; 256],
    activeinternalnotecount: usize,
    activemidinotecount: usize,
    osc1cache: OscSettings,
    osc2cache: OscSettings,
    touch: f32,
    delaylineleft: Vec<f32>,
    delaylineright: Vec<f32>,
    //delayreadpos: usize,
    delaywritepos: usize,
    pub sequencer: SequencerState,
    arp: ArpState,
    lastplaying: bool,
    old_step: u32,
    lfo: LFOState,
    lfovalue: f32,
    lastnote: u8,
    sps_detune_tab: [f32; 1024], // FIXME: move to IronFishGlobalVoiceState
    g: IronFishGlobalVoiceState,
    chorus: ChorusState,
    reverb: ReverbState,
    actual_delay_length: f32
}

impl IronFishState {
    pub fn note_off(&mut self, b1: u8, b2: u8) {
        self.activemidinotes[b1 as usize] = false;
        if self.activemidinotecount > 0 {
            self.activemidinotecount = self.activemidinotecount - 1;
        }

        self.rebuildarp();

        if self.settings.arp.enabled.get() {
        } else {
            self.internal_note_off(b1, b2);
        }
    }

    pub fn internal_note_off(&mut self, b1: u8, b2: u8) {
        if self.activeinternalnotes[b1 as usize] == true {
            for i in 0..self.voices.len() {
                if self.voices[i].active() == b1 as i16 {
                    self.voices[i].note_off(b1, b2, &self.settings)
                }
            }
            self.activeinternalnotes[b1 as usize] = false;

            if self.activeinternalnotecount <= 0 {
                //self.lastnote = 255;
                //    log!("last note - set to 255");
                self.activeinternalnotecount = 0;
            } else {
                self.activeinternalnotecount = self.activeinternalnotecount - 1;
                //      log!("{}", self.activeinternalnotecount);
            }
        }
    }

    pub fn note_on(&mut self, b1: u8, b2: u8) {
        if b1 > 127 {
            return;
        };
        self.activemidinotes[b1 as usize] = true;
        self.activemidinotecount = self.activemidinotecount + 1;
        self.rebuildarp();

        //log!("note! {} {}", b1,b2);
        if self.settings.arp.enabled.get() {
        } else {
            self.internal_note_on(b1, b2);
        }
    }

    pub fn internal_note_on(&mut self, b1: u8, b2: u8) {
        for i in 0..self.voices.len() {
            if self.voices[i].active() == -1 {
                self.voices[i].note_on(
                    b1,
                    self.lastnote,
                    b2,
                    &self.settings,
                    &self.g,
                    &self.sps_detune_tab,
                );
                self.lastnote = b1;
                self.activeinternalnotes[b1 as usize] = true;
                self.activeinternalnotecount = self.activeinternalnotecount + 1;
                return;
            }
        }
    }

    pub fn rebuildarp(&mut self) {
        let mut current = 0;
        for i in 0..128 {
            if self.activemidinotes[i] {
                self.arp.melody[current] = i as u32;
                current += 1;
            }
        }
        let mut n = 1;
        let notesinoriginalmelody = current;
        if self.settings.arp.octaves.get() != 0 {
            if self.settings.arp.octaves.get() < 0 {
                n = -1;
            }

            for i in 0..self.settings.arp.octaves.get().abs() {
                for q in 0..notesinoriginalmelody {
                    self.arp.melody[current] =
                        ((self.arp.melody[q] as i32) + ((i + 1) as i32) * 12 * n) as u32;
                    current += 1;
                }
            }
        }
        self.arp.melodylength = current;
    }

    /*
    pub fn one(&mut self) -> f32 {
        let mut output: f32 = 0.0;
        for i in 0..self.voices.len() {
            output += self.voices[i].one(&self.settings, self.touch, self.lfovalue);
        }
        return output; // * 1000.0;
    }
    */
    pub fn apply_bitcrush(&mut self, buffer: &mut AudioBuffer) {
        if !self.settings.bitcrush.enable.get() {
            return;
        };

        let amount = self.settings.bitcrush.amount.get();
        let crushbits = (amount * 22.0) as i32;
        let precrushmult = 65536.0 * 8.0;
        let postcrushmult = (1 << crushbits) as f32 / precrushmult;
        let frame_count = buffer.frame_count();
        let (left, right) = buffer.stereo_mut();
        for i in 0..frame_count {
            let intermediate_left = (left[i] * precrushmult) as i32;
            let crushed_left = intermediate_left >> crushbits;

            left[i] = (crushed_left as f32) * postcrushmult;
            let intermediate_right = (right[i] * precrushmult) as i32;
            let crushed_right = intermediate_right >> crushbits;

            right[i] = (crushed_right as f32) * postcrushmult;
        }
    }
    pub fn apply_reverb(&mut self, buffer: &mut AudioBuffer) {
        if self.settings.reverb.mix.get() == 0.0 {
            return;
        };
        let frame_count = buffer.frame_count();
        let (left, right) = buffer.stereo_mut();
        for i in 0..frame_count {
            // let verb = self.reverb.toy.test_delay(left[i], right[i], self.settings.reverb.mix.get(), self.settings.reverb.feedback.get());
            let verb = self.reverb.toy.griesinger_reverb(
                left[i],
                right[i],
                self.settings.reverb.mix.get(),
                self.settings.reverb.feedback.get(),
            );
            left[i] = verb.0;
            right[i] = verb.1;
        }
    }

    pub fn apply_delay(&mut self, buffer: &mut AudioBuffer) {
        let frame_count = buffer.frame_count();
        let (left, right) = buffer.stereo_mut();

        let cross = self.settings.delay.cross.get();
        let icross = 1.0 - cross;

        let mut l = self.settings.delay.length.get();
        let leftoffs = (self.settings.delay.difference.get()).powf(2.0);
        
        l = (l * l) *  (47000.0 - 15000.0) + 1000.0;
        self.actual_delay_length += (l - self.actual_delay_length) * 0.3;
        let delaylen: f32 = (self.actual_delay_length) as f32;  
        
        let mut delayreadposl:i32 = self.delaywritepos as i32 - (delaylen - (leftoffs * (48000.0 - delaylen))).max(1.0).min(47500.0) as i32;
        let mut delayreadposr:i32 = self.delaywritepos as i32 - (delaylen + (leftoffs * (48000.0 - delaylen))).max(1.0).min(47500.0) as i32; 

        while delayreadposl >= 48000 {
            delayreadposl -= 48000;
        }
        while delayreadposr >= 48000 {
            delayreadposr -= 48000;
        }
        while delayreadposl < 0 {
            delayreadposl += 48000;
        }
        while delayreadposr < 0 {
            delayreadposr += 48000;
        }

        let fb = self.settings.delay.delayfeedback.get() * 0.98;
        let send = self.settings.delay.delaysend.get();

        for i in 0..frame_count {
            let rr = self.delaylineright[delayreadposr as  usize];
            let ll = self.delaylineleft[delayreadposl as usize];

            let mut r = ll * cross + rr * icross;
            let mut l = rr * cross + ll * icross;

            r *= fb;
            r += send * (right[i]);

            l *= fb;
            l += send * (left[i]);

            self.delaylineright[self.delaywritepos] = r;
            self.delaylineleft[self.delaywritepos] = l;

            left[i] += l;
            right[i] += r;
            self.delaywritepos += 1;
            if self.delaywritepos >= 48000 {
                self.delaywritepos = 0;
            }
            delayreadposl += 1;
            if delayreadposl >= 48000 {
                delayreadposl = 0;
            }
            delayreadposr += 1;
            if delayreadposr >= 48000 {
                delayreadposr = 0;
            }
        }
    }

    pub fn get_sequencer_step(&mut self, step: usize) -> u32 {
        return self.settings.sequencer.steps[step].get();
    }

    pub fn fill_buffer(&mut self, buffer: &mut AudioBuffer, display: &mut DisplayAudioGraph) {
        for i in 0..self.voices.len() {
            let mut dp = display.pop_buffer_resize(buffer.frame_count(), buffer.channel_count());
            if let Some(dp) = &mut dp {
                dp.zero();
            }
            self.display_buffers[i] = dp;
        }

        buffer.zero();

        let mut pitchdirty: bool = false;
        if self.osc1cache.transpose.get() != self.settings.osc1.transpose.get() {
            pitchdirty = true;
        }
        if self.osc1cache.detune.get() != self.settings.osc1.detune.get() {
            pitchdirty = true;
        }
        if self.osc2cache.transpose.get() != self.settings.osc2.transpose.get() {
            pitchdirty = true;
        }
        if self.osc2cache.detune.get() != self.settings.osc2.detune.get() {
            pitchdirty = true;
        }

        // FIXME: if oscillator is dirty always re-evaluate/calculate hypersaw settings (situation
        //        would occur where settings were not picked up when switching from supersaw to hypersaw,
        //        that said I'd like those to be separate parameters, and shouldn't this logic just not live here
        //        or even better: not exist in this capacity?)
        let osc_dirty = self.osc1cache.osc_type.get() != self.settings.osc1.osc_type.get()
            || self.osc2cache.osc_type.get() != self.settings.osc2.osc_type.get();

        // FIXME: maybe we should rename 'supersaw*' to something generic shared by both types?
        let mut diffuse1dirty: bool = false;
        if osc_dirty || self.g.hypersaw1.last_diffuse != self.settings.supersaw1.diffuse.get() {
            diffuse1dirty = true;
            self.g.hypersaw1.last_diffuse = self.settings.supersaw1.diffuse.get();
            self.g.hypersaw1.recalcsaws();
        }

        let mut diffuse2dirty: bool = false;
        if osc_dirty || self.g.hypersaw2.last_diffuse != self.settings.supersaw2.diffuse.get() {
            diffuse2dirty = true;
            self.g.hypersaw2.last_diffuse = self.settings.supersaw2.diffuse.get();
            self.g.hypersaw2.recalcsaws();
        }

        let mut spread1dirty: bool = false;
        if osc_dirty || self.g.hypersaw1.last_spread != self.settings.supersaw1.spread.get() {
            spread1dirty = true;
            self.g.hypersaw1.last_spread = self.settings.supersaw1.spread.get();
        }

        let mut spread2dirty: bool = false;
        if osc_dirty || self.g.hypersaw2.last_spread != self.settings.supersaw2.spread.get() {
            spread2dirty = true;
            self.g.hypersaw2.last_spread = self.settings.supersaw2.spread.get();
        }

        let recalchyperlevels1 = diffuse1dirty;
        let recalchyperlevels2 = diffuse2dirty;

        if recalchyperlevels1 {
            self.g.hypersaw1.recalclevels();
            spread1dirty = true;
        }

        if recalchyperlevels2 {
            self.g.hypersaw2.recalclevels();
            spread2dirty = true;
        }

        if spread1dirty {
            self.g.hypersaw1.recalcfreqs();
        }

        if spread2dirty {
            self.g.hypersaw2.recalcfreqs();
        }

        if osc_dirty {
            self.osc1cache
                .osc_type
                .set(self.settings.osc1.osc_type.get());
            self.osc2cache
                .osc_type
                .set(self.settings.osc2.osc_type.get());
        }

        if pitchdirty {
            self.osc1cache
                .transpose
                .set(self.settings.osc1.transpose.get());
            self.osc1cache.detune.set(self.settings.osc1.detune.get());
            self.osc2cache
                .transpose
                .set(self.settings.osc2.transpose.get());
            self.osc2cache.detune.set(self.settings.osc2.detune.get());

            for i in 0..self.voices.len() {
                if self.voices[i].active() > -1 {
                    self.voices[i].update_note(&self.settings, &self.g, &self.sps_detune_tab, true);
                }
            }
        }

        let mut remaining = buffer.frame_count();
        let mut bufferidx = 0;

        let lfofreq = 0.5 * (2.0).powf(self.settings.lfo.rate.get() * 8.0 - 4.0);
        let lfodphase = 1.0 / (self.settings.sample_rate.get() / lfofreq);

        self.lfo.phase += remaining as f32 * lfodphase;

        while self.lfo.phase > 1.0 {
            self.lfo.phase -= 1.0;
        }
        self.lfovalue = (self.lfo.phase * 6.283).sin();
        //   log!("s{}", remaining);

        while remaining > 0 {
            //log!("b{}", remaining);
            let mut toprocess = remaining;
            if self.sequencer.samplesleftinstep == 0 {
                if self.settings.sequencer.playing.get() {
                    if self.lastplaying == false {
                        self.lastplaying = true;
                        self.sequencer.currentstep = 15;
                        self.old_step = 0;
                    }

                    if self.sequencer.currentscale != self.settings.sequencer.scale.get() {
                        self.sequencer.currentscale = self.settings.sequencer.scale.get();
                        self.all_notes_off();
                    }
                    if self.sequencer.currentrootnote != self.settings.sequencer.rootnote.get() {
                        self.all_notes_off();
                        self.sequencer.currentrootnote = self.settings.sequencer.rootnote.get();
                    }

                    // process notes!
                    let newstepidx = (self.sequencer.currentstep + 1) % 16;
                    let new_step = self.get_sequencer_step(newstepidx);

                    //log!("tick! {:?} {:?}",newstepidx, new_step);
                    // minor scale..
                    let scalecount = [7, 7, 7, 5];
                    let scale = [
                        [0, 2, 3, 5, 7, 8, 11],
                        [0, 2, 4, 5, 7, 9, 11],
                        [0, 2, 3, 5, 7, 9, 10],
                        [0, 2, 5, 7, 9, 12, 14],
                    ];

                    let mut scaleidx = 0;
                    let rootnoteenum = self.settings.sequencer.rootnote.get();

                    let mut rootnote = 12;

                    if rootnoteenum == RootNote::A {
                        rootnote = 12 - 3;
                    };
                    if rootnoteenum == RootNote::Asharp {
                        rootnote = 12 - 2;
                    };
                    if rootnoteenum == RootNote::B {
                        rootnote = 12 - 1;
                    };
                    if rootnoteenum == RootNote::C {
                        rootnote = 12 - 0;
                    };
                    if rootnoteenum == RootNote::Csharp {
                        rootnote = 12 + 1;
                    };
                    if rootnoteenum == RootNote::D {
                        rootnote = 12 + 2;
                    };
                    if rootnoteenum == RootNote::Dsharp {
                        rootnote = 12 + 3;
                    };
                    if rootnoteenum == RootNote::E {
                        rootnote = 12 + 4;
                    };
                    if rootnoteenum == RootNote::F {
                        rootnote = 12 + 5;
                    };
                    if rootnoteenum == RootNote::Fsharp {
                        rootnote = 12 + 6;
                    };
                    if rootnoteenum == RootNote::G {
                        rootnote = 12 + 7;
                    };
                    if rootnoteenum == RootNote::Gsharp {
                        rootnote = 12 + 8;
                    };

                    if self.settings.sequencer.scale.get() == MusicalScale::Major {
                        scaleidx = 1;
                    };
                    if self.settings.sequencer.scale.get() == MusicalScale::Dorian {
                        scaleidx = 2;
                    };
                    if self.settings.sequencer.scale.get() == MusicalScale::Pentatonic {
                        scaleidx = 3;
                    };

                    for i in 0..32 {
                        if self.old_step & (1 << (31 - i)) != 0 {
                            if (new_step & (1 << (31 - i))) == 0 {
                                //  log!("note off {:?}",scale[i]);
                                self.internal_note_off(
                                    rootnote
                                        + scale[scaleidx][(i % scalecount[scaleidx]) as usize]
                                        + (i / scalecount[scaleidx]) * 12,
                                    127,
                                );
                            }
                        } else {
                            if new_step & (1 << (31 - i)) != 0 {
                                // log!("note on {:?}",scale[i]);
                                self.internal_note_on(
                                    rootnote
                                        + scale[scaleidx][(i % scalecount[scaleidx]) as usize]
                                        + (i / scalecount[scaleidx]) * 12,
                                    127,
                                );
                            }
                        }
                    }
                    self.old_step = new_step;

                    self.sequencer.currentstep = newstepidx;
                }

                if self.settings.arp.enabled.get() {
                    self.internal_note_off(self.arp.lastarpnote as u8, 127);

                    if self.activemidinotecount > 0 {
                        self.arp.lastarpnote = self.arp.melody[self.arp.step];
                        self.internal_note_on(self.arp.lastarpnote as u8, 127);
                        self.arp.step = (self.arp.step + 1) % self.arp.melodylength.max(1);
                    }
                }
                self.sequencer.samplesleftinstep = ((self.settings.sample_rate.get() * 60.0)
                    / (self.settings.sequencer.bpm.get() * 4.0))
                    as usize;
            }

            toprocess = toprocess.min(self.sequencer.samplesleftinstep);
            self.sequencer.samplesleftinstep -= toprocess;

            if self.lastplaying && self.settings.sequencer.playing.get() == false {
                self.all_notes_off();
                self.lastplaying = false;
            }

            for i in 0..self.voices.len() {
                if self.voices[i].active() > -1 {
                    self.voices[i].fill_buffer(
                        buffer,
                        bufferidx,
                        toprocess,
                        self.display_buffers[i].as_mut(),
                        &self.settings,
                        &self.g,
                        self.touch,
                        self.lfovalue,
                        &self.sps_detune_tab,
                    );
                }
            }
            bufferidx += toprocess;
            remaining -= toprocess;
        }

        for (i, dp) in self.display_buffers.iter_mut().enumerate() {
            if let Some(dp) = dp.take() {
                display.send_buffer(self.voices[i].active() > -1, i, dp);
            }
        }
        self.apply_bitcrush(buffer);
        self.chorus.apply_chorus(
            buffer,
            &self.settings.chorus,
            self.settings.sample_rate.get(),
        );
        self.apply_delay(buffer);
        self.apply_reverb(buffer);
    }
}

impl Default for SequencerState {
    fn default() -> Self {
        Self {
            samplesleftinstep: 10,
            currentstep: 0,
            currentscale: MusicalScale::Minor,
            currentrootnote: RootNote::C,
        }
    }
}

impl Default for IronFishVoice {
    fn default() -> Self {
        Self {
            osc1: OscillatorState::default(),
            osc2: OscillatorState::default(),
            subosc: SubOscillatorState::default(),
            filter1: FilterState::default(),
            volume_envelope: EnvelopeState::default(),
            mod_envelope: EnvelopeState::default(),
            //sequencer: SequencerState::default(),
            current_note: -1,
            current_notefreq: 69.0,
            fromnote: 69.0,
            tonote: 69.0,
            notetime: 0.0,
            notetimetotal: 0.0,
            seed: 1234,
        }
    }
}

live_design! {
    IronFish = {{IronFish}} {
        settings: {}
    }
}

#[derive(Live)]
pub struct IronFish {
    #[live]
    pub settings: Arc<IronFishSettings>,
    //#[rust] to_ui: ToUIReceiver<ToUI>,
    //#[rust] from_ui: FromUISender<FromUI>,
}

impl LiveRegister for IronFish {
    fn live_register(cx: &mut Cx) {
        register_audio_component!(cx, IronFish)
    }
}

impl LiveHook for IronFish {
    fn skip_apply(
        &mut self,
        _cx: &mut Cx,
        apply: &mut Apply,
        index: usize,
        nodes: &[LiveNode],
    ) -> Option<usize> {
        if let ApplyFrom::UpdateFromDoc { .. } = apply.from {
            log!("NOT HERE");
            return Some(nodes.skip_node(index));
        }
        None
    }
}

impl AudioGraphNode for IronFishState {
    fn all_notes_off(&mut self) {
        for i in 0..self.voices.len() {
            self.voices[i].volume_envelope.phase = EnvelopePhase::Idle;
            self.activemidinotes[i] = false;
            self.activeinternalnotes[i] = false;
        }
        self.activemidinotecount = 0;
        self.lastnote = 69;
        self.activeinternalnotecount = 0;
        self.rebuildarp();
    }

    fn handle_midi_data(&mut self, data: MidiData) {
        match data.decode() {
            MidiEvent::Note(note) => {
                if note.is_on {
                    self.note_on(note.note_number, note.velocity);
                } else {
                    self.note_off(note.note_number, note.velocity);
                }
            }
            _ => (),
        }

        if data.data[0] == 0xb0 && data.data[1] == 1 {
            self.touch = (data.data[2] as f32 - 40.0) / (127.0 - 40.0);
            self.touch += self.settings.touch.offset.get();
            self.touch *= self.settings.touch.scale.get();
            self.touch = self
                .touch
                .powf(self.settings.touch.curve.get() * 3.0)
                .min(1.0)
                .max(-1.0);
        }
    }

    fn render_to_audio_buffer(
        &mut self,
        _info: AudioInfo,
        outputs: &mut [&mut AudioBuffer],
        _inputs: &[&AudioBuffer],
        display: &mut DisplayAudioGraph,
    ) {
        self.fill_buffer(outputs[0], display)
    }
}

impl AudioComponent for IronFish {
    fn get_graph_node(&mut self, _cx: &mut Cx) -> Box<dyn AudioGraphNode + Send> {
        // self.from_ui.new_channel();
        let mut buffers = Vec::new();
        buffers.resize(16, None);

        // precalculate supersaw detune table (heavy polnynomial, based on data sampled from actual synth.)
        // reference: https://github.com/bipolaraudio/FM-BISON/blob/master/literature/Supersaw%20thesis.pdf
        // FIXME: move to it's own function/object to keep things tidy
        let mut sps_detune_tab = [0f32; 1024];
        for i in 0..1024 {
            let detune = (1.0 / 1024.0) * i as f32;
            sps_detune_tab[i] = (10028.7312891634 * pow(detune, 11.0))
                - (50818.8652045924 * pow(detune, 10.0))
                + (111363.4808729368 * pow(detune, 9.0))
                - (138150.6761080548 * pow(detune, 8.0))
                + (106649.6679158292 * pow(detune, 7.0))
                - (53046.9642751875 * pow(detune, 6.0))
                + (17019.9518580080 * pow(detune, 5.0))
                - (3425.0836591318 * pow(detune, 4.0))
                + (404.2703938388 * pow(detune, 3.0))
                - (24.1878824391 * pow(detune, 2.0))
                + (0.6717417634 * detune)
                + 0.0030115596;
        }

        Box::new(IronFishState {
            display_buffers: buffers,
            settings: self.settings.clone(),
            voices: Default::default(),
            activemidinotes: [false; 256],
            activeinternalnotes: [false; 256],
            //to_ui: self.to_ui.sender(),
            //from_ui: self.from_ui.receiver(),
            osc1cache: self.settings.osc1.clone(),
            osc2cache: self.settings.osc2.clone(),
            touch: 0.0,
            delaylineleft: vec![0.0f32; 48000],
            delaylineright: vec![0.0f32; 48000],
            delaywritepos: 15000,
            //delayreadpos: 0,
            sequencer: SequencerState::default(),
            lastplaying: false,
            old_step: 0,
            arp: Default::default(),
            activemidinotecount: 0,
            activeinternalnotecount: 0,
            lfo: Default::default(),
            lfovalue: 0.0,
            lastnote: 69,
            sps_detune_tab,
            g: Default::default(),
            chorus: Default::default(),
            reverb: Default::default(),
            actual_delay_length: 1.0
        })
    }

    fn handle_event_with(
        &mut self,
        _cx: &mut Cx,
        _event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, AudioComponentAction),
    ) {
    }
    // we dont have inputs
    fn audio_query(
        &mut self,
        _query: &AudioQuery,
        _callback: &mut Option<AudioQueryCb>,
    ) -> AudioResult {
        AudioResult::not_found()
    }
}
