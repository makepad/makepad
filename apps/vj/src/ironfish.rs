//! Faithful, realtime-safe modernization of Makepad's final Ironfish synth.
//!
//! The DSP vocabulary and algorithms come from the last Ironfish tree before
//! it was removed (`62f2cf7a5a1ed66b74542c2957b0bf91ea260538`). Ownership is
//! deliberately modern: this value belongs to the audio callback, its patch is
//! copied in through the bounded mixer command queue, and every delay line is
//! allocated when the engine is built rather than while it is rendering.
//!
//! Ironfish is MIT licensed, (C) Stijn Kuipers.
//! The SuperSaw oscillator implementation is MIT licensed, (C) Niels J. de Wit.

const TAU: f32 = core::f32::consts::TAU;

fn finite(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum OscillatorKind {
    #[default]
    DpwSawPulse,
    BlampTriangle,
    Pure,
    SuperSaw,
    HyperSaw,
    HarmonicSeries,
}

impl OscillatorKind {
    pub const LABELS: [&'static str; 6] =
        ["DPW SAW", "BLAMP TRI", "PURE SINE", "SUPERSAW", "HYPERSAW", "HARMONIC"];

    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn from_index(index: usize) -> Self {
        match index {
            1 => Self::BlampTriangle,
            2 => Self::Pure,
            3 => Self::SuperSaw,
            4 => Self::HyperSaw,
            5 => Self::HarmonicSeries,
            _ => Self::DpwSawPulse,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum LfoWave {
    #[default]
    Saw,
    Sine,
    Pulse,
    Triangle,
}

impl LfoWave {
    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Sine,
            2 => Self::Pulse,
            3 => Self::Triangle,
            _ => Self::Saw,
        }
    }

    fn sample(self, phase: f32) -> f32 {
        match self {
            Self::Saw => phase * 2.0 - 1.0,
            Self::Sine => (phase * TAU).sin(),
            Self::Pulse => if phase < 0.5 { 1.0 } else { -1.0 },
            Self::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum FilterKind {
    #[default]
    LowPass,
    HighPass,
    BandPass,
    BandReject,
}

impl FilterKind {
    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn from_index(index: usize) -> Self {
        match index {
            1 => Self::HighPass,
            2 => Self::BandPass,
            3 => Self::BandReject,
            _ => Self::LowPass,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum RootNote {
    A,
    ASharp,
    B,
    #[default]
    C,
    CSharp,
    D,
    DSharp,
    E,
    F,
    FSharp,
    G,
    GSharp,
}

impl RootNote {
    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn from_index(index: usize) -> Self {
        match index {
            0 => Self::A,
            1 => Self::ASharp,
            2 => Self::B,
            4 => Self::CSharp,
            5 => Self::D,
            6 => Self::DSharp,
            7 => Self::E,
            8 => Self::F,
            9 => Self::FSharp,
            10 => Self::G,
            11 => Self::GSharp,
            _ => Self::C,
        }
    }

    const fn semitone(self) -> u8 {
        match self {
            Self::C => 0,
            Self::CSharp => 1,
            Self::D => 2,
            Self::DSharp => 3,
            Self::E => 4,
            Self::F => 5,
            Self::FSharp => 6,
            Self::G => 7,
            Self::GSharp => 8,
            Self::A => 9,
            Self::ASharp => 10,
            Self::B => 11,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum ScaleKind {
    #[default]
    Minor,
    Major,
    Dorian,
    Pentatonic,
}

impl ScaleKind {
    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Major,
            2 => Self::Dorian,
            3 => Self::Pentatonic,
            _ => Self::Minor,
        }
    }

    fn degree(self, row: usize) -> u8 {
        const MINOR: [u8; 7] = [0, 2, 3, 5, 7, 8, 11];
        const MAJOR: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
        const DORIAN: [u8; 7] = [0, 2, 3, 5, 7, 9, 10];
        const PENTATONIC: [u8; 5] = [0, 2, 5, 7, 9];
        let scale: &[u8] = match self {
            Self::Minor => &MINOR,
            Self::Major => &MAJOR,
            Self::Dorian => &DORIAN,
            Self::Pentatonic => &PENTATONIC,
        };
        scale[row % scale.len()] + 12 * (row / scale.len()) as u8
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OscillatorPatch {
    pub kind: OscillatorKind,
    pub transpose: i8,
    pub detune: f32,
    pub spread: f32,
    pub diffuse: f32,
    pub harmonic: f32,
    pub harmonic_env: f32,
    pub harmonic_lfo: f32,
}

impl Default for OscillatorPatch {
    fn default() -> Self {
        Self {
            kind: OscillatorKind::DpwSawPulse,
            transpose: 0,
            detune: 0.0,
            spread: 0.28,
            diffuse: 0.45,
            harmonic: 0.0,
            harmonic_env: 0.0,
            harmonic_lfo: 0.0,
        }
    }
}

impl OscillatorPatch {
    fn sanitise(mut self) -> Self {
        self.transpose = self.transpose.clamp(-24, 24);
        self.detune = finite(self.detune, 0.0).clamp(-1.0, 1.0);
        self.spread = finite(self.spread, 0.0).clamp(0.0, 0.999_999);
        self.diffuse = finite(self.diffuse, 0.0).clamp(0.0, 1.0);
        self.harmonic = finite(self.harmonic, 0.0).clamp(0.0, 1.0);
        self.harmonic_env = finite(self.harmonic_env, 0.0).clamp(-1.0, 1.0);
        self.harmonic_lfo = finite(self.harmonic_lfo, 0.0).clamp(-1.0, 1.0);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvelopePatch {
    pub predelay: f32,
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl EnvelopePatch {
    fn amp_default() -> Self {
        Self { predelay: 0.0, attack: 0.05, hold: 0.0, decay: 0.2, sustain: 0.5, release: 0.2 }
    }

    fn mod_default() -> Self {
        Self { predelay: 0.0, attack: 0.02, hold: 0.0, decay: 0.25, sustain: 0.0, release: 0.2 }
    }

    fn sanitise(mut self) -> Self {
        self.predelay = finite(self.predelay, 0.0).clamp(0.0, 1.0);
        self.attack = finite(self.attack, 0.05).clamp(0.0, 1.0);
        self.hold = finite(self.hold, 0.0).clamp(0.0, 1.0);
        self.decay = finite(self.decay, 0.2).clamp(0.0, 1.0);
        self.sustain = finite(self.sustain, 0.5).clamp(0.0, 1.0);
        self.release = finite(self.release, 0.2).clamp(0.0, 1.0);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilterPatch {
    pub kind: FilterKind,
    pub cutoff: f32,
    pub resonance: f32,
    pub envelope_amount: f32,
    pub lfo_amount: f32,
    pub touch_amount: f32,
}

impl Default for FilterPatch {
    fn default() -> Self {
        Self {
            kind: FilterKind::LowPass,
            cutoff: 0.5,
            resonance: 0.05,
            envelope_amount: 0.1,
            lfo_amount: 0.1,
            touch_amount: 0.1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LfoPatch {
    pub rate: f32,
    pub key_sync: bool,
    pub wave: LfoWave,
}

impl Default for LfoPatch {
    fn default() -> Self {
        Self { rate: 0.2, key_sync: false, wave: LfoWave::Sine }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DelayPatch {
    pub send: f32,
    pub feedback: f32,
    pub cross: f32,
    pub difference: f32,
    pub length: f32,
}

impl Default for DelayPatch {
    fn default() -> Self {
        Self { send: 0.15, feedback: 0.8, cross: 0.9, difference: 0.1, length: 0.7 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChorusPatch {
    pub min_delay: f32,
    pub mod_depth: f32,
    pub rate: f32,
    pub phase_diff: f32,
    pub mix: f32,
    pub feedback: f32,
}

impl Default for ChorusPatch {
    fn default() -> Self {
        Self { min_delay: 0.1, mod_depth: 0.4, rate: 0.3, phase_diff: 0.4, mix: 0.5, feedback: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReverbPatch {
    pub mix: f32,
    pub feedback: f32,
}

impl Default for ReverbPatch {
    fn default() -> Self {
        Self { mix: 0.0, feedback: 0.04 }
    }
}

/// Complete plain-data Ironfish program. Discrete selectors are sent with the
/// same patch command as the continuous values, so the callback observes one
/// coherent program and never follows UI atomics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IronfishPatch {
    pub osc1: OscillatorPatch,
    pub osc2: OscillatorPatch,
    pub osc_balance: f32,
    pub sub_level: f32,
    pub noise_level: f32,
    pub portamento: f32,
    pub amp: EnvelopePatch,
    pub modulation: EnvelopePatch,
    pub filter: FilterPatch,
    pub lfo: LfoPatch,
    pub touch: f32,
    pub bitcrush_enabled: bool,
    pub bitcrush: f32,
    pub delay: DelayPatch,
    pub chorus: ChorusPatch,
    pub reverb: ReverbPatch,
    pub arp_enabled: bool,
    pub arp_octaves: i8,
    pub root: RootNote,
    pub scale: ScaleKind,
    pub output: f32,
}

impl Default for IronfishPatch {
    fn default() -> Self {
        let mut osc2 = OscillatorPatch::default();
        osc2.kind = OscillatorKind::SuperSaw;
        osc2.detune = 0.07;
        Self {
            osc1: OscillatorPatch::default(),
            osc2,
            osc_balance: 0.42,
            sub_level: 0.1,
            noise_level: 0.0,
            portamento: 0.0,
            amp: EnvelopePatch::amp_default(),
            modulation: EnvelopePatch::mod_default(),
            filter: FilterPatch::default(),
            lfo: LfoPatch::default(),
            touch: 0.5,
            bitcrush_enabled: false,
            bitcrush: 0.4,
            delay: DelayPatch::default(),
            chorus: ChorusPatch::default(),
            reverb: ReverbPatch::default(),
            arp_enabled: false,
            arp_octaves: 0,
            root: RootNote::C,
            scale: ScaleKind::Minor,
            // The historical voice already carries its TAU * 0.02 gain;
            // unity here reproduces that level before the VJ channel strip.
            output: 1.0,
        }
    }
}

impl IronfishPatch {
    pub fn sanitise(mut self) -> Self {
        self.osc1 = self.osc1.sanitise();
        self.osc2 = self.osc2.sanitise();
        self.osc_balance = finite(self.osc_balance, 0.5).clamp(0.0, 1.0);
        self.sub_level = finite(self.sub_level, 0.0).clamp(0.0, 1.0);
        self.noise_level = finite(self.noise_level, 0.0).clamp(0.0, 1.0);
        self.portamento = finite(self.portamento, 0.0).clamp(0.0, 1.0);
        self.amp = self.amp.sanitise();
        self.modulation = self.modulation.sanitise();
        self.filter.cutoff = finite(self.filter.cutoff, 0.5).clamp(0.0, 1.0);
        self.filter.resonance = finite(self.filter.resonance, 0.05).clamp(0.0, 0.995);
        self.filter.envelope_amount = finite(self.filter.envelope_amount, 0.0).clamp(-1.0, 1.0);
        self.filter.lfo_amount = finite(self.filter.lfo_amount, 0.0).clamp(-1.0, 1.0);
        self.filter.touch_amount = finite(self.filter.touch_amount, 0.0).clamp(-1.0, 1.0);
        self.lfo.rate = finite(self.lfo.rate, 0.2).clamp(0.0, 1.0);
        self.touch = finite(self.touch, 0.5).clamp(0.0, 1.0);
        self.bitcrush = finite(self.bitcrush, 0.4).clamp(0.0, 1.0);
        self.delay.send = finite(self.delay.send, 0.0).clamp(0.0, 1.0);
        self.delay.feedback = finite(self.delay.feedback, 0.0).clamp(0.0, 0.98);
        self.delay.cross = finite(self.delay.cross, 0.0).clamp(0.0, 1.0);
        self.delay.difference = finite(self.delay.difference, 0.0).clamp(0.0, 1.0);
        self.delay.length = finite(self.delay.length, 0.5).clamp(0.0, 1.0);
        self.chorus.min_delay = finite(self.chorus.min_delay, 0.1).clamp(0.0, 1.0);
        self.chorus.mod_depth = finite(self.chorus.mod_depth, 0.4).clamp(0.0, 1.0);
        self.chorus.rate = finite(self.chorus.rate, 0.3).clamp(0.0, 1.0);
        self.chorus.phase_diff = finite(self.chorus.phase_diff, 0.4).clamp(0.0, 1.0);
        self.chorus.mix = finite(self.chorus.mix, 0.0).clamp(0.0, 1.0);
        self.chorus.feedback = finite(self.chorus.feedback, 0.0).clamp(0.0, 1.0);
        self.reverb.mix = finite(self.reverb.mix, 0.0).clamp(0.0, 1.0);
        self.reverb.feedback = finite(self.reverb.feedback, 0.04).clamp(0.0, 0.98);
        self.arp_octaves = self.arp_octaves.clamp(-3, 3);
        self.output = finite(self.output, 1.0).clamp(0.0, 1.25);
        self
    }

    pub fn preset(index: usize) -> Self {
        let mut patch = Self::default();
        match index % 8 {
            1 => {
                patch.osc1.kind = OscillatorKind::Pure;
                patch.osc2.kind = OscillatorKind::HarmonicSeries;
                patch.osc2.harmonic = 0.18;
                patch.amp.attack = 0.22;
                patch.amp.release = 0.62;
                patch.filter.cutoff = 0.72;
                patch.chorus.mix = 0.66;
                patch.reverb.mix = 0.3;
            }
            2 => {
                patch.osc1.kind = OscillatorKind::HyperSaw;
                patch.osc2.kind = OscillatorKind::SuperSaw;
                patch.osc1.diffuse = 0.82;
                patch.osc1.spread = 0.35;
                patch.filter.cutoff = 0.34;
                patch.filter.resonance = 0.45;
                patch.modulation.decay = 0.42;
                patch.filter.envelope_amount = 0.72;
            }
            3 => {
                patch.osc1.kind = OscillatorKind::BlampTriangle;
                patch.osc2.kind = OscillatorKind::Pure;
                patch.osc2.transpose = -12;
                patch.sub_level = 0.42;
                patch.filter.cutoff = 0.28;
                patch.portamento = 0.22;
            }
            4 => {
                patch.osc1.kind = OscillatorKind::HarmonicSeries;
                patch.osc2.kind = OscillatorKind::HarmonicSeries;
                patch.osc1.harmonic_env = 0.8;
                patch.osc2.harmonic_lfo = 0.55;
                patch.lfo.wave = LfoWave::Triangle;
                patch.lfo.rate = 0.35;
                patch.filter.kind = FilterKind::BandPass;
            }
            5 => {
                patch.osc1.kind = OscillatorKind::DpwSawPulse;
                patch.osc2.kind = OscillatorKind::DpwSawPulse;
                patch.osc2.transpose = 12;
                patch.filter.kind = FilterKind::HighPass;
                patch.filter.cutoff = 0.56;
                patch.bitcrush_enabled = true;
                patch.bitcrush = 0.34;
                patch.delay.send = 0.38;
            }
            6 => {
                patch.osc1.kind = OscillatorKind::SuperSaw;
                patch.osc2.kind = OscillatorKind::HyperSaw;
                patch.osc1.spread = 0.72;
                patch.osc1.diffuse = 0.76;
                patch.osc2.spread = 0.28;
                patch.osc2.diffuse = 1.0;
                patch.amp.attack = 0.0;
                patch.amp.release = 0.38;
                patch.chorus.mix = 0.72;
            }
            7 => {
                patch.osc1.kind = OscillatorKind::Pure;
                patch.osc2.kind = OscillatorKind::BlampTriangle;
                patch.amp.predelay = 0.08;
                patch.amp.attack = 0.52;
                patch.amp.hold = 0.12;
                patch.amp.release = 0.82;
                patch.filter.kind = FilterKind::BandReject;
                patch.reverb.mix = 0.52;
            }
            _ => {}
        }
        patch.sanitise()
    }

    pub fn grid_note(self, row: usize) -> u8 {
        (48u8 + self.root.semitone()).saturating_add(self.scale.degree(row)).min(127)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IronfishParam {
    OscBalance,
    Osc1Transpose,
    Osc1Detune,
    Osc1Spread,
    Osc1Diffuse,
    Osc1Harmonic,
    Osc1HarmonicEnv,
    Osc1HarmonicLfo,
    Osc2Transpose,
    Osc2Detune,
    Osc2Spread,
    Osc2Diffuse,
    Osc2Harmonic,
    Osc2HarmonicEnv,
    Osc2HarmonicLfo,
    Sub,
    Noise,
    Portamento,
    AmpPredelay,
    AmpAttack,
    AmpHold,
    AmpDecay,
    AmpSustain,
    AmpRelease,
    ModPredelay,
    ModAttack,
    ModHold,
    ModDecay,
    ModSustain,
    ModRelease,
    FilterCutoff,
    FilterResonance,
    FilterEnvAmount,
    FilterLfoAmount,
    FilterTouchAmount,
    LfoRate,
    Touch,
    Bitcrush,
    DelaySend,
    DelayFeedback,
    DelayCross,
    DelayDifference,
    DelayLength,
    ChorusMinDelay,
    ChorusModDepth,
    ChorusRate,
    ChorusPhaseDiff,
    ChorusMix,
    ChorusFeedback,
    ReverbMix,
    ReverbFeedback,
    ArpOctaves,
    Output,
}

impl IronfishPatch {
    pub fn set_normalised(&mut self, param: IronfishParam, value: f32) {
        let v = finite(value, 0.0).clamp(0.0, 1.0);
        match param {
            IronfishParam::OscBalance => self.osc_balance = v,
            IronfishParam::Osc1Transpose => self.osc1.transpose = (v * 48.0).round() as i8 - 24,
            IronfishParam::Osc1Detune => self.osc1.detune = v * 2.0 - 1.0,
            IronfishParam::Osc1Spread => self.osc1.spread = v,
            IronfishParam::Osc1Diffuse => self.osc1.diffuse = v,
            IronfishParam::Osc1Harmonic => self.osc1.harmonic = v,
            IronfishParam::Osc1HarmonicEnv => self.osc1.harmonic_env = v * 2.0 - 1.0,
            IronfishParam::Osc1HarmonicLfo => self.osc1.harmonic_lfo = v * 2.0 - 1.0,
            IronfishParam::Osc2Transpose => self.osc2.transpose = (v * 48.0).round() as i8 - 24,
            IronfishParam::Osc2Detune => self.osc2.detune = v * 2.0 - 1.0,
            IronfishParam::Osc2Spread => self.osc2.spread = v,
            IronfishParam::Osc2Diffuse => self.osc2.diffuse = v,
            IronfishParam::Osc2Harmonic => self.osc2.harmonic = v,
            IronfishParam::Osc2HarmonicEnv => self.osc2.harmonic_env = v * 2.0 - 1.0,
            IronfishParam::Osc2HarmonicLfo => self.osc2.harmonic_lfo = v * 2.0 - 1.0,
            IronfishParam::Sub => self.sub_level = v,
            IronfishParam::Noise => self.noise_level = v,
            IronfishParam::Portamento => self.portamento = v,
            IronfishParam::AmpPredelay => self.amp.predelay = v,
            IronfishParam::AmpAttack => self.amp.attack = v,
            IronfishParam::AmpHold => self.amp.hold = v,
            IronfishParam::AmpDecay => self.amp.decay = v,
            IronfishParam::AmpSustain => self.amp.sustain = v,
            IronfishParam::AmpRelease => self.amp.release = v,
            IronfishParam::ModPredelay => self.modulation.predelay = v,
            IronfishParam::ModAttack => self.modulation.attack = v,
            IronfishParam::ModHold => self.modulation.hold = v,
            IronfishParam::ModDecay => self.modulation.decay = v,
            IronfishParam::ModSustain => self.modulation.sustain = v,
            IronfishParam::ModRelease => self.modulation.release = v,
            IronfishParam::FilterCutoff => self.filter.cutoff = v,
            IronfishParam::FilterResonance => self.filter.resonance = v * 0.995,
            IronfishParam::FilterEnvAmount => self.filter.envelope_amount = v * 2.0 - 1.0,
            IronfishParam::FilterLfoAmount => self.filter.lfo_amount = v * 2.0 - 1.0,
            IronfishParam::FilterTouchAmount => self.filter.touch_amount = v * 2.0 - 1.0,
            IronfishParam::LfoRate => self.lfo.rate = v,
            IronfishParam::Touch => self.touch = v,
            IronfishParam::Bitcrush => self.bitcrush = v,
            IronfishParam::DelaySend => self.delay.send = v,
            IronfishParam::DelayFeedback => self.delay.feedback = v * 0.98,
            IronfishParam::DelayCross => self.delay.cross = v,
            IronfishParam::DelayDifference => self.delay.difference = v,
            IronfishParam::DelayLength => self.delay.length = v,
            IronfishParam::ChorusMinDelay => self.chorus.min_delay = v,
            IronfishParam::ChorusModDepth => self.chorus.mod_depth = v,
            IronfishParam::ChorusRate => self.chorus.rate = v,
            IronfishParam::ChorusPhaseDiff => self.chorus.phase_diff = v,
            IronfishParam::ChorusMix => self.chorus.mix = v,
            IronfishParam::ChorusFeedback => self.chorus.feedback = v,
            IronfishParam::ReverbMix => self.reverb.mix = v,
            IronfishParam::ReverbFeedback => self.reverb.feedback = v * 0.98,
            IronfishParam::ArpOctaves => self.arp_octaves = (v * 6.0).round() as i8 - 3,
            IronfishParam::Output => self.output = v * 1.25,
        }
        *self = self.sanitise();
    }

    pub fn normalised(self, param: IronfishParam) -> f32 {
        (match param {
            IronfishParam::OscBalance => self.osc_balance,
            IronfishParam::Osc1Transpose => (self.osc1.transpose as f32 + 24.0) / 48.0,
            IronfishParam::Osc1Detune => self.osc1.detune * 0.5 + 0.5,
            IronfishParam::Osc1Spread => self.osc1.spread,
            IronfishParam::Osc1Diffuse => self.osc1.diffuse,
            IronfishParam::Osc1Harmonic => self.osc1.harmonic,
            IronfishParam::Osc1HarmonicEnv => self.osc1.harmonic_env * 0.5 + 0.5,
            IronfishParam::Osc1HarmonicLfo => self.osc1.harmonic_lfo * 0.5 + 0.5,
            IronfishParam::Osc2Transpose => (self.osc2.transpose as f32 + 24.0) / 48.0,
            IronfishParam::Osc2Detune => self.osc2.detune * 0.5 + 0.5,
            IronfishParam::Osc2Spread => self.osc2.spread,
            IronfishParam::Osc2Diffuse => self.osc2.diffuse,
            IronfishParam::Osc2Harmonic => self.osc2.harmonic,
            IronfishParam::Osc2HarmonicEnv => self.osc2.harmonic_env * 0.5 + 0.5,
            IronfishParam::Osc2HarmonicLfo => self.osc2.harmonic_lfo * 0.5 + 0.5,
            IronfishParam::Sub => self.sub_level,
            IronfishParam::Noise => self.noise_level,
            IronfishParam::Portamento => self.portamento,
            IronfishParam::AmpPredelay => self.amp.predelay,
            IronfishParam::AmpAttack => self.amp.attack,
            IronfishParam::AmpHold => self.amp.hold,
            IronfishParam::AmpDecay => self.amp.decay,
            IronfishParam::AmpSustain => self.amp.sustain,
            IronfishParam::AmpRelease => self.amp.release,
            IronfishParam::ModPredelay => self.modulation.predelay,
            IronfishParam::ModAttack => self.modulation.attack,
            IronfishParam::ModHold => self.modulation.hold,
            IronfishParam::ModDecay => self.modulation.decay,
            IronfishParam::ModSustain => self.modulation.sustain,
            IronfishParam::ModRelease => self.modulation.release,
            IronfishParam::FilterCutoff => self.filter.cutoff,
            IronfishParam::FilterResonance => self.filter.resonance / 0.995,
            IronfishParam::FilterEnvAmount => self.filter.envelope_amount * 0.5 + 0.5,
            IronfishParam::FilterLfoAmount => self.filter.lfo_amount * 0.5 + 0.5,
            IronfishParam::FilterTouchAmount => self.filter.touch_amount * 0.5 + 0.5,
            IronfishParam::LfoRate => self.lfo.rate,
            IronfishParam::Touch => self.touch,
            IronfishParam::Bitcrush => self.bitcrush,
            IronfishParam::DelaySend => self.delay.send,
            IronfishParam::DelayFeedback => self.delay.feedback / 0.98,
            IronfishParam::DelayCross => self.delay.cross,
            IronfishParam::DelayDifference => self.delay.difference,
            IronfishParam::DelayLength => self.delay.length,
            IronfishParam::ChorusMinDelay => self.chorus.min_delay,
            IronfishParam::ChorusModDepth => self.chorus.mod_depth,
            IronfishParam::ChorusRate => self.chorus.rate,
            IronfishParam::ChorusPhaseDiff => self.chorus.phase_diff,
            IronfishParam::ChorusMix => self.chorus.mix,
            IronfishParam::ChorusFeedback => self.chorus.feedback,
            IronfishParam::ReverbMix => self.reverb.mix,
            IronfishParam::ReverbFeedback => self.reverb.feedback / 0.98,
            IronfishParam::ArpOctaves => (self.arp_octaves as f32 + 3.0) / 6.0,
            IronfishParam::Output => self.output / 1.25,
        })
        .clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EnvelopeStage {
    Idle,
    Predelay,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
}

#[derive(Clone, Copy)]
struct Envelope {
    stage: EnvelopeStage,
    level: f32,
    frames_left: f32,
    delta: f32,
}

impl Default for Envelope {
    fn default() -> Self {
        Self { stage: EnvelopeStage::Idle, level: 0.0, frames_left: 0.0, delta: 0.0 }
    }
}

impl Envelope {
    fn frames(value: f32, rate: f32) -> f32 {
        64.0 * ((rate * 6.0) / 64.0).powf(value.clamp(0.0, 1.0).powf(0.54))
    }

    fn enter(&mut self, stage: EnvelopeStage, patch: &EnvelopePatch, rate: f32) {
        self.stage = stage;
        match stage {
            EnvelopeStage::Idle => {
                self.level = 0.0;
                self.frames_left = -1.0;
                self.delta = 0.0;
            }
            EnvelopeStage::Predelay => {
                self.frames_left = Self::frames(patch.predelay, rate);
                self.delta = 0.0;
            }
            EnvelopeStage::Attack => {
                self.frames_left = Self::frames(patch.attack, rate);
                self.delta = (1.0 - self.level) / self.frames_left.max(1.0);
            }
            EnvelopeStage::Hold => {
                self.level = 1.0;
                self.frames_left = Self::frames(patch.hold, rate);
                self.delta = 0.0;
            }
            EnvelopeStage::Decay => {
                let target = patch.sustain * patch.sustain;
                self.level = self.level.max(target);
                self.frames_left = Self::frames(patch.decay, rate);
                self.delta = (target - self.level) / self.frames_left.max(1.0);
            }
            EnvelopeStage::Sustain => {
                self.level = patch.sustain * patch.sustain;
                self.frames_left = -1.0;
                self.delta = 0.0;
            }
            EnvelopeStage::Release => {
                self.frames_left = Self::frames(patch.release, rate);
                self.delta = -self.level / self.frames_left.max(1.0);
            }
        }
    }

    fn note_on(&mut self, patch: &EnvelopePatch, rate: f32) {
        if patch.predelay > 0.0 {
            self.level = 0.0;
            self.enter(EnvelopeStage::Predelay, patch, rate);
        } else {
            self.enter(EnvelopeStage::Attack, patch, rate);
        }
    }

    fn note_off(&mut self, patch: &EnvelopePatch, rate: f32) {
        if self.stage != EnvelopeStage::Idle {
            self.enter(EnvelopeStage::Release, patch, rate);
        }
    }

    fn next(&mut self, patch: &EnvelopePatch, rate: f32) -> f32 {
        if matches!(self.stage, EnvelopeStage::Idle | EnvelopeStage::Sustain) {
            if self.stage == EnvelopeStage::Sustain {
                self.level = patch.sustain * patch.sustain;
            }
            return self.level;
        }
        self.level = (self.level + self.delta).clamp(0.0, 1.0);
        self.frames_left -= 1.0;
        if self.frames_left <= 0.0 {
            match self.stage {
                EnvelopeStage::Predelay => self.enter(EnvelopeStage::Attack, patch, rate),
                EnvelopeStage::Attack if patch.hold > 0.0 => self.enter(EnvelopeStage::Hold, patch, rate),
                EnvelopeStage::Attack | EnvelopeStage::Hold => self.enter(EnvelopeStage::Decay, patch, rate),
                EnvelopeStage::Decay => self.enter(EnvelopeStage::Sustain, patch, rate),
                EnvelopeStage::Release => self.enter(EnvelopeStage::Idle, patch, rate),
                EnvelopeStage::Idle | EnvelopeStage::Sustain => {}
            }
        }
        self.level
    }

    fn active(self) -> bool {
        self.stage != EnvelopeStage::Idle
    }
}

#[derive(Clone, Copy, Default)]
struct FilterState {
    high: f32,
    band: f32,
    low: f32,
}

impl FilterState {
    fn process(&mut self, input: f32, patch: &FilterPatch, envelope: f32, touch: f32, lfo: f32) -> f32 {
        let mut cutoff = patch.cutoff
            + touch * patch.touch_amount
            + lfo * patch.lfo_amount * 0.35
            + envelope * patch.envelope_amount * 0.5;
        cutoff = cutoff.clamp(0.0, 1.0);
        cutoff *= cutoff * 0.5;
        let phi = (2.0 * (core::f32::consts::PI * cutoff).sin()).clamp(0.0, 1.0);
        let gamma = (2.0 * (1.0 - patch.resonance)).clamp(0.01, 1.0);
        self.band = phi * self.high + self.band;
        self.low = phi * self.band + self.low;
        self.high = input - self.low - gamma * self.band;
        let output = match patch.kind {
            FilterKind::LowPass => self.low,
            FilterKind::HighPass => self.high,
            FilterKind::BandPass => self.band,
            FilterKind::BandReject => input - self.band,
        };
        if output.is_finite() {
            output
        } else {
            *self = Self::default();
            0.0
        }
    }
}

fn advance(phase: &mut f32, delta: f32) -> f32 {
    let current = *phase;
    *phase = (*phase + delta).fract();
    current
}

fn poly_saw(phase: f32, delta: f32) -> f32 {
    let mut p = (phase + 0.5).fract();
    let mut saw = p * 2.0 - 1.0;
    if delta > 0.0 && p < delta {
        let x = p / delta - 1.0;
        saw += x * x;
    } else if delta > 0.0 && p > 1.0 - delta {
        p = (p - 1.0) / delta + 1.0;
        saw -= p * p;
    }
    saw
}

fn blamp(phase: f32, delta: f32) -> f32 {
    let mut y = 0.0;
    if phase < 2.0 * delta && delta > 0.0 {
        let x = phase / delta;
        let u = 2.0 - x;
        y -= u.powi(4);
        if phase < delta {
            let v = 1.0 - x;
            y += 4.0 * v.powi(5);
        }
    }
    y * delta / 15.0
}

fn blamp_triangle(phase: f32, delta: f32) -> f32 {
    let mut tri = 2.0 * (2.0 * phase - 1.0).abs() - 1.0;
    tri += blamp(phase, delta) + blamp(1.0 - phase, delta);
    let p2 = (phase + 0.5).fract();
    tri -= blamp(p2, delta) + blamp(1.0 - p2, delta);
    tri
}

fn supersaw_detune(value: f32) -> f32 {
    let x = value.clamp(0.0, 0.999_999);
    10028.731 * x.powi(11) - 50818.867 * x.powi(10) + 111363.48 * x.powi(9)
        - 138150.67 * x.powi(8)
        + 106649.66 * x.powi(7)
        - 53046.965 * x.powi(6)
        + 17019.951 * x.powi(5)
        - 3425.0837 * x.powi(4)
        + 404.2704 * x.powi(3)
        - 24.187883 * x.powi(2)
        + 0.6717418 * x
}

#[derive(Clone, Copy)]
struct OscillatorState {
    phase: f32,
    supersaw: [f32; 7],
    hypersaw: [f32; 7],
}

impl Default for OscillatorState {
    fn default() -> Self {
        Self {
            phase: 0.0,
            supersaw: [0.0; 7],
            hypersaw: [0.0, 0.414, 0.732, 0.236, 0.646, 0.317, 0.606],
        }
    }
}

impl OscillatorState {
    fn render(&mut self, patch: &OscillatorPatch, note: f32, env: f32, lfo: f32, rate: f32) -> f32 {
        let freq = 440.0 * 2.0f32.powf(
            (note - 69.0 + patch.transpose as f32 + patch.detune) / 12.0,
        );
        let delta = (freq / rate).clamp(0.0, 0.49);
        match patch.kind {
            OscillatorKind::Pure => (advance(&mut self.phase, delta) * TAU).sin(),
            OscillatorKind::DpwSawPulse => poly_saw(advance(&mut self.phase, delta), delta),
            OscillatorKind::BlampTriangle => {
                blamp_triangle(advance(&mut self.phase, delta), delta)
            }
            OscillatorKind::HarmonicSeries => {
                let phase = advance(&mut self.phase, delta) * TAU;
                let h = (patch.harmonic + env * patch.harmonic_env + lfo * patch.harmonic_lfo)
                    .clamp(0.0, 1.0)
                    * 16.0;
                let base = h.floor() + 1.0;
                let blend = h.fract();
                (phase * base).sin() * (1.0 - blend) + (phase * (base + 1.0)).sin() * blend
            }
            OscillatorKind::SuperSaw => {
                const COEFF: [f32; 6] =
                    [-0.11002313, -0.06288439, -0.03024148, 0.02953130, 0.06216538, 0.10745242];
                let detune = supersaw_detune(patch.spread);
                let main_gain = -0.55366 * patch.diffuse + 0.99785;
                let side_gain = -0.73764 * patch.diffuse.powi(2) + 1.2841 * patch.diffuse + 0.044372;
                let p = advance(&mut self.supersaw[0], delta);
                let main = poly_saw(p, delta);
                let mut sides = 0.0;
                for index in 1..7 {
                    let side_delta = (delta * (1.0 + detune * COEFF[index - 1])).clamp(0.0, 0.49);
                    let phase = advance(&mut self.supersaw[index], side_delta);
                    sides += poly_saw(phase, side_delta);
                    if index < 6 {
                        sides -= (phase * TAU).cos();
                    }
                }
                main * main_gain + sides * side_gain
            }
            OscillatorKind::HyperSaw => {
                let extra = patch.diffuse * 6.0;
                let whole = extra.floor() as usize;
                let count = if extra <= 0.000_001 { 1 } else { (whole + 2).min(7) };
                let fractional = extra.fract();
                let saws = 1.0 + extra;
                let base_level = 1.0 / (1.0 - 0.1 * (saws - 1.0));
                let mut weights = [0.0f32; 7];
                for weight in weights.iter_mut().take(count) {
                    *weight = 1.0;
                }
                if fractional > 0.000_01 {
                    weights[count - 1] = fractional * fractional;
                }
                let total = weights.iter().sum::<f32>().max(1.0);
                let mut output = 0.0;
                for index in 0..count {
                    if weights[index] == 0.0 {
                        continue;
                    }
                    let multiplier = if index == 0 {
                        1.0
                    } else {
                        let amount = index as f32 / 2.0 * (patch.spread / 6.0) * (0.5 / (saws * 0.5));
                        2.0f32.powf(if index & 2 == 0 { amount } else { -amount })
                    };
                    let side_delta = (delta * multiplier).clamp(0.0, 0.49);
                    output += poly_saw(advance(&mut self.hypersaw[index], side_delta), side_delta)
                        * weights[index]
                        * base_level
                        / total;
                }
                output
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Voice {
    note: u8,
    pitch: f32,
    from_pitch: f32,
    glide_left: f32,
    glide_total: f32,
    velocity: f32,
    osc1: OscillatorState,
    osc2: OscillatorState,
    sub_phase: f32,
    amp: Envelope,
    modulation: Envelope,
    filter: FilterState,
    noise: u32,
    serial: u64,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            note: 60,
            pitch: 60.0,
            from_pitch: 60.0,
            glide_left: 0.0,
            glide_total: 0.0,
            velocity: 0.0,
            osc1: OscillatorState::default(),
            osc2: OscillatorState::default(),
            sub_phase: 0.0,
            amp: Envelope::default(),
            modulation: Envelope::default(),
            filter: FilterState::default(),
            noise: 0x6d2b_79f5,
            serial: 0,
        }
    }
}

impl Voice {
    fn note_on(&mut self, note: u8, previous: u8, velocity: u8, serial: u64, patch: &IronfishPatch, rate: f32) {
        self.note = note;
        self.serial = serial;
        self.velocity = velocity as f32 / 127.0;
        if patch.portamento > 0.0 {
            self.from_pitch = previous as f32;
            self.pitch = self.from_pitch;
            self.glide_total = Envelope::frames(patch.portamento, rate);
            self.glide_left = self.glide_total;
        } else {
            self.from_pitch = note as f32;
            self.pitch = note as f32;
            self.glide_total = 0.0;
            self.glide_left = 0.0;
        }
        self.amp.note_on(&patch.amp, rate);
        self.modulation.note_on(&patch.modulation, rate);
    }

    fn note_off(&mut self, patch: &IronfishPatch, rate: f32) {
        self.amp.note_off(&patch.amp, rate);
        self.modulation.note_off(&patch.modulation, rate);
    }

    fn next(&mut self, patch: &IronfishPatch, lfo: f32, rate: f32) -> f32 {
        let amp = self.amp.next(&patch.amp, rate);
        if !self.amp.active() {
            return 0.0;
        }
        let modulation = self.modulation.next(&patch.modulation, rate);
        if self.glide_left > 0.0 {
            self.glide_left -= 1.0;
            let remaining = (self.glide_left / self.glide_total.max(1.0)).clamp(0.0, 1.0);
            self.pitch = self.note as f32 + (self.from_pitch - self.note as f32) * remaining;
        } else {
            self.pitch = self.note as f32;
        }
        let osc1_gain = (1.0 - patch.osc_balance).sqrt();
        let osc2_gain = patch.osc_balance.sqrt();
        let osc1 = self.osc1.render(&patch.osc1, self.pitch, modulation, lfo, rate);
        let osc2 = self.osc2.render(&patch.osc2, self.pitch, modulation, lfo, rate);
        // The final Ironfish sub oscillator computes a note one octave down
        // and then halves its phase rate: two octaves below the played note.
        let sub_delta = (440.0 * 2.0f32.powf((self.pitch - 69.0 - 24.0) / 12.0) / rate)
            .clamp(0.0, 0.49);
        let sub = (advance(&mut self.sub_phase, sub_delta) * TAU).sin();
        self.noise ^= self.noise << 13;
        self.noise ^= self.noise >> 17;
        self.noise ^= self.noise << 5;
        let noise = self.noise as i32 as f32 / i32::MAX as f32;
        let input = osc1 * osc1_gain
            + osc2 * osc2_gain
            + sub * patch.sub_level
            + noise * patch.noise_level;
        self.filter.process(input, &patch.filter, modulation, patch.touch, lfo)
            * amp
            * self.velocity
            * (TAU * 0.02)
    }
}

#[derive(Clone, Copy)]
struct SmoothValue {
    current: f32,
    rate: f32,
}

impl SmoothValue {
    fn new(rate: f32) -> Self {
        Self { current: 0.0, rate }
    }

    fn get(&mut self, target: f32) -> f32 {
        self.current += (target - self.current) * self.rate;
        self.current
    }
}

struct Waveguide {
    head: usize,
    buffer: Vec<f32>,
}

impl Waveguide {
    fn new(rate: f32) -> Self {
        Self { head: 0, buffer: vec![0.0; (rate * 0.11).ceil() as usize + 8] }
    }

    fn feed(&mut self, input: f32, feedback: f32, delay: f32) -> f32 {
        let len = self.buffer.len();
        let delay = delay.clamp(1.0, (len - 4) as f32);
        let back = (self.head as f32 - delay).rem_euclid(len as f32);
        // `rem_euclid(len as f32)` can round to exactly `len` for a value
        // infinitesimally below it (notably with the 44.1 kHz chorus sizes).
        // Integer ring reduction keeps every cubic tap in bounds.
        let i0 = back.floor() as usize % len;
        let im1 = (i0 + len - 1) % len;
        let i1 = (i0 + 1) % len;
        let i2 = (i0 + 2) % len;
        let x = back.fract();
        let ym1 = self.buffer[im1];
        let y0 = self.buffer[i0];
        let y1 = self.buffer[i1];
        let y2 = self.buffer[i2];
        let c0 = y0;
        let c1 = 0.5 * (y1 - ym1);
        let c2 = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
        let c3 = 0.5 * (y2 - ym1) + 1.5 * (y0 - y1);
        let output = ((c3 * x + c2) * x + c1) * x + c0;
        self.buffer[self.head] = input + output * feedback;
        self.head = (self.head + 1) % len;
        output
    }
}

struct ChorusState {
    lines: [Waveguide; 6],
    phase: f32,
    feedback: SmoothValue,
    mix: SmoothValue,
    phase_diff: SmoothValue,
    min_delay: SmoothValue,
    mod_depth: SmoothValue,
}

impl ChorusState {
    fn new(rate: f32) -> Self {
        Self {
            lines: std::array::from_fn(|_| Waveguide::new(rate)),
            phase: 0.0,
            feedback: SmoothValue::new(0.1),
            mix: SmoothValue::new(0.1),
            phase_diff: SmoothValue::new(0.1),
            min_delay: SmoothValue::new(0.01),
            mod_depth: SmoothValue::new(0.01),
        }
    }

    fn process(&mut self, input: [f32; 2], patch: &ChorusPatch, rate: f32) -> [f32; 2] {
        let lfo_hz = 0.5 * 2.0f32.powf(patch.rate * 8.0 - 4.0);
        self.phase = (self.phase + lfo_hz * TAU / rate).rem_euclid(TAU);
        let min_delay = self.min_delay.get(rate * 0.030 * patch.min_delay) + 1.0;
        let depth = self.mod_depth.get(patch.mod_depth * rate * 0.050);
        let phase_diff = self.phase_diff.get(patch.phase_diff);
        let feedback = self.feedback.get(patch.feedback) * 0.86;
        let mix = self.mix.get(patch.mix);
        let mut wet = [0.0; 2];
        for index in 0..6 {
            let delay = min_delay + ((self.phase + index as f32 * phase_diff).sin() + 1.0) * depth;
            wet[index & 1] += self.lines[index].feed(input[index & 1], feedback, delay);
        }
        [input[0] * (1.0 - mix) + wet[0] * mix, input[1] * (1.0 - mix) + wet[1] * mix]
    }
}

const REVERB_LEN: usize = 1 << 16;
const REVERB_MASK: usize = REVERB_LEN - 1;

struct ResonatorLfo {
    cosine: f32,
    sine: f32,
    amplitude: f32,
}

impl ResonatorLfo {
    fn new(frequency: f32) -> Self {
        Self { cosine: 0.0, sine: 0.0, amplitude: frequency * 2.0 }
    }

    fn next(&mut self) -> f32 {
        self.cosine -= self.amplitude * self.sine;
        self.sine += self.amplitude * self.cosine;
        self.sine
    }
}

struct DelayToy {
    write: usize,
    local: usize,
    accumulator: f32,
    feedback: f32,
    feedback_filter: f32,
    buffer: Vec<f32>,
    lfo1: ResonatorLfo,
    lfo2: ResonatorLfo,
}

impl DelayToy {
    fn new(rate: f32) -> Self {
        let scale = rate / 32_777.0;
        Self {
            write: 0,
            local: 0,
            accumulator: 0.0,
            feedback: 0.0,
            feedback_filter: 0.0,
            buffer: vec![0.0; REVERB_LEN],
            lfo1: ResonatorLfo::new(9.4 / 32_777.0 / scale.max(0.01)),
            lfo2: ResonatorLfo::new(1.3 * 3.15971 / 32_777.0 / scale.max(0.01)),
        }
    }

    fn start(&mut self) {
        self.local = self.write;
    }

    fn end(&mut self) {
        self.write = self.write.wrapping_sub(1) & REVERB_MASK;
    }

    fn all_pass(&mut self, length: usize, coefficient: f32) {
        let index = (self.local + length) & REVERB_MASK;
        let delayed = self.buffer[index];
        self.accumulator -= delayed * coefficient;
        self.buffer[self.local] = self.accumulator.clamp(-1.0, 1.0);
        self.accumulator = self.accumulator * coefficient + delayed;
        self.local = index;
    }

    fn interpolate(&self, index: usize, offset: f32) -> f32 {
        let adjusted = (index as isize - offset.floor() as isize)
            .rem_euclid(REVERB_LEN as isize) as usize;
        let fraction = offset.fract();
        self.buffer[adjusted] * (1.0 - fraction)
            + self.buffer[(adjusted + 1) & REVERB_MASK] * fraction
    }

    fn all_pass_wobble(&mut self, length: usize, coefficient: f32, offset: f32) {
        let index = (self.local + length) & REVERB_MASK;
        let delayed = self.interpolate(index, offset);
        self.accumulator -= delayed * coefficient;
        self.buffer[self.local] = self.accumulator.clamp(-1.0, 1.0);
        self.accumulator = self.accumulator * coefficient + delayed;
        self.local = index;
    }

    fn delay(&mut self, length: usize) {
        let index = (self.local + length) & REVERB_MASK;
        self.buffer[self.local] = self.accumulator.clamp(-1.0, 1.0);
        self.accumulator = self.buffer[index];
        self.local = index;
    }

    fn griesinger(&mut self, input: [f32; 2], mix: f32, feedback: f32, rate: f32) -> [f32; 2] {
        if mix <= 0.000_001 {
            return input;
        }
        let scale = rate / 32_777.0;
        let length = |samples: usize| ((samples as f32 * scale).round() as usize).max(1);
        self.start();
        self.accumulator = (input[0] + input[1]) * mix;
        self.all_pass(length(142), 0.5);
        self.all_pass(length(379), 0.5);
        self.accumulator += (input[0] + input[1]) * mix;
        self.all_pass(length(107), 0.5);
        self.all_pass(length(277), 0.5);
        let reinject = self.accumulator;
        let wobble1 = self.lfo1.next();
        let wobble2 = self.lfo2.next();
        self.accumulator += self.feedback;
        self.all_pass_wobble(length(672), 0.5, wobble1);
        self.all_pass(length(1800), 0.5);
        self.delay(length(4453));
        let left = self.accumulator;
        self.accumulator += reinject;
        self.all_pass_wobble(length(908), 0.5, wobble2);
        self.all_pass(length(2656), 0.5);
        self.delay(length(3163));
        let right = self.accumulator;
        self.feedback_filter += (self.accumulator - self.feedback_filter) * 0.95;
        self.feedback = self.feedback_filter * feedback;
        self.end();
        [left * mix + input[0] * (1.0 - mix), right * mix + input[1] * (1.0 - mix)]
    }
}

struct Effects {
    chorus: ChorusState,
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    delay_write: usize,
    delay_length: f32,
    reverb: DelayToy,
}

impl Effects {
    fn new(rate: f32) -> Self {
        let delay_len = rate.ceil() as usize + 8;
        Self {
            chorus: ChorusState::new(rate),
            delay_l: vec![0.0; delay_len],
            delay_r: vec![0.0; delay_len],
            delay_write: 0,
            delay_length: 1.0,
            reverb: DelayToy::new(rate),
        }
    }

    fn bitcrush(mut frame: [f32; 2], patch: &IronfishPatch) -> [f32; 2] {
        if !patch.bitcrush_enabled {
            return frame;
        }
        let bits = (patch.bitcrush * 22.0) as u32;
        let pre = 524_288.0;
        let post = (1u32 << bits.min(22)) as f32 / pre;
        for sample in &mut frame {
            *sample = (((*sample * pre) as i32) >> bits.min(22)) as f32 * post;
        }
        frame
    }

    fn delay(&mut self, mut frame: [f32; 2], patch: &DelayPatch, rate: f32) -> [f32; 2] {
        let target = patch.length.powi(2) * (rate * (32_000.0 / 48_000.0)) + rate / 48.0;
        self.delay_length += (target - self.delay_length) * 0.000_8;
        let offset = patch.difference.powi(2) * (rate - self.delay_length);
        let left_frames = (self.delay_length - offset).clamp(1.0, rate - 2.0) as usize;
        let right_frames = (self.delay_length + offset).clamp(1.0, rate - 2.0) as usize;
        let len = self.delay_l.len();
        let read_l = (self.delay_write + len - left_frames.min(len - 1)) % len;
        let read_r = (self.delay_write + len - right_frames.min(len - 1)) % len;
        let delayed_l = self.delay_l[read_l];
        let delayed_r = self.delay_r[read_r];
        let cross = patch.cross;
        let feedback = patch.feedback;
        self.delay_l[self.delay_write] =
            (delayed_r * cross + delayed_l * (1.0 - cross)) * feedback + frame[0] * patch.send;
        self.delay_r[self.delay_write] =
            (delayed_l * cross + delayed_r * (1.0 - cross)) * feedback + frame[1] * patch.send;
        frame[0] += self.delay_l[self.delay_write];
        frame[1] += self.delay_r[self.delay_write];
        self.delay_write = (self.delay_write + 1) % len;
        frame
    }

    fn process(&mut self, frame: [f32; 2], patch: &IronfishPatch, rate: f32) -> [f32; 2] {
        let frame = Self::bitcrush(frame, patch);
        let frame = self.chorus.process(frame, &patch.chorus, rate);
        let frame = self.delay(frame, &patch.delay, rate);
        self.reverb.griesinger(frame, patch.reverb.mix, patch.reverb.feedback, rate)
    }
}

/// Audio-owned Ironfish voice/effect engine.
pub struct Ironfish {
    patch: IronfishPatch,
    voices: [Voice; 16],
    serial: u64,
    last_note: u8,
    rate: f32,
    lfo_phase: f32,
    held: [bool; 128],
    held_velocity: [u8; 128],
    arp_step: usize,
    arp_note: Option<u8>,
    effects: Effects,
}

impl Ironfish {
    pub fn new(rate: f32, patch: IronfishPatch) -> Self {
        let rate = finite(rate, 48_000.0).clamp(8_000.0, 384_000.0);
        Self {
            patch: patch.sanitise(),
            voices: [Voice::default(); 16],
            serial: 0,
            last_note: 60,
            rate,
            lfo_phase: 0.0,
            held: [false; 128],
            held_velocity: [0; 128],
            arp_step: 0,
            arp_note: None,
            effects: Effects::new(rate),
        }
    }

    pub fn patch(&self) -> IronfishPatch {
        self.patch
    }

    pub fn set_patch(&mut self, patch: IronfishPatch) {
        let patch = patch.sanitise();
        if patch.arp_enabled != self.patch.arp_enabled {
            self.silence_voices();
            self.arp_step = 0;
            self.arp_note = None;
            if !patch.arp_enabled {
                self.patch = patch;
                for note in 0..128 {
                    if self.held[note] {
                        self.start_voice(note as u8, self.held_velocity[note]);
                    }
                }
                return;
            }
        }
        self.patch = patch;
    }

    pub fn set_param(&mut self, param: IronfishParam, value: f32) {
        let mut patch = self.patch;
        patch.set_normalised(param, value);
        self.set_patch(patch);
    }

    fn start_voice(&mut self, note: u8, velocity: u8) {
        self.serial = self.serial.wrapping_add(1);
        let slot = self
            .voices
            .iter()
            .position(|voice| !voice.amp.active())
            .unwrap_or_else(|| {
                self.voices
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, voice)| voice.serial)
                    .map(|(index, _)| index)
                    .unwrap_or(0)
            });
        self.voices[slot].note_on(note, self.last_note, velocity, self.serial, &self.patch, self.rate);
        self.last_note = note;
        if self.patch.lfo.key_sync {
            self.lfo_phase = 0.0;
        }
    }

    fn release_voice(&mut self, note: u8) {
        for voice in &mut self.voices {
            if voice.note == note && voice.amp.active() {
                voice.note_off(&self.patch, self.rate);
            }
        }
    }

    fn silence_voices(&mut self) {
        for voice in &mut self.voices {
            voice.note_off(&self.patch, self.rate);
        }
    }

    pub fn note_on(&mut self, note: u8, velocity: u8) {
        if note > 127 {
            return;
        }
        self.held[note as usize] = true;
        self.held_velocity[note as usize] = velocity;
        if !self.patch.arp_enabled {
            self.start_voice(note, velocity);
        }
    }

    pub fn note_off(&mut self, note: u8) {
        if note > 127 {
            return;
        }
        self.held[note as usize] = false;
        if self.patch.arp_enabled {
            if self.arp_note == Some(note) {
                self.release_voice(note);
                self.arp_note = None;
            }
        } else {
            self.release_voice(note);
        }
    }

    /// Advance the historical arpeggiator from the rack's shared sixteenth
    /// clock. The old private BPM counter is intentionally not restored.
    pub fn clock_step(&mut self) {
        if !self.patch.arp_enabled {
            return;
        }
        if let Some(note) = self.arp_note.take() {
            self.release_voice(note);
        }
        let held_count = self.held.iter().filter(|&&held| held).count();
        if held_count == 0 {
            self.arp_step = 0;
            return;
        }
        let octave_count = self.patch.arp_octaves.unsigned_abs() as usize + 1;
        let total = held_count * octave_count;
        let wanted = self.arp_step % total;
        let base_index = wanted % held_count;
        let octave_index = wanted / held_count;
        let mut seen = 0;
        let mut base_note = 60u8;
        for note in 0..128 {
            if self.held[note] {
                if seen == base_index {
                    base_note = note as u8;
                    break;
                }
                seen += 1;
            }
        }
        let direction = self.patch.arp_octaves.signum() as i16;
        let note = (base_note as i16 + direction * octave_index as i16 * 12).clamp(0, 127) as u8;
        self.start_voice(note, self.held_velocity[base_note as usize]);
        self.arp_note = Some(note);
        self.arp_step = (self.arp_step + 1) % total;
    }

    pub fn all_notes_off(&mut self) {
        self.held.fill(false);
        self.held_velocity.fill(0);
        self.arp_step = 0;
        self.arp_note = None;
        self.silence_voices();
    }

    pub fn grid_note(&self, row: usize) -> u8 {
        self.patch.grid_note(row)
    }

    pub fn next_frame(&mut self) -> [f32; 2] {
        let lfo_hz = 0.5 * 2.0f32.powf(self.patch.lfo.rate * 8.0 - 4.0);
        self.lfo_phase = (self.lfo_phase + lfo_hz / self.rate).fract();
        let lfo = self.patch.lfo.wave.sample(self.lfo_phase);
        let mut mono = 0.0;
        for voice in &mut self.voices {
            mono += voice.next(&self.patch, lfo, self.rate);
        }
        mono *= self.patch.output;
        let frame = self.effects.process([mono, mono], &self.patch, self.rate);
        if frame[0].is_finite() && frame[1].is_finite() {
            frame
        } else {
            [0.0; 2]
        }
    }

    pub fn active_voices(&self) -> u8 {
        self.voices.iter().filter(|voice| voice.amp.active()).count() as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_historical_oscillator_renders_finite_audio() {
        for index in 0..OscillatorKind::LABELS.len() {
            let mut patch = IronfishPatch::default();
            patch.osc1.kind = OscillatorKind::from_index(index);
            patch.osc2.kind = OscillatorKind::from_index(index);
            patch.osc1.spread = 0.999;
            patch.osc1.diffuse = 1.0;
            let mut synth = Ironfish::new(48_000.0, patch);
            synth.note_on(60, 127);
            for _ in 0..16_384 {
                let frame = synth.next_frame();
                assert!(frame[0].is_finite() && frame[1].is_finite());
            }
        }
    }

    #[test]
    fn all_filter_and_lfo_modes_are_finite() {
        for filter in 0..4 {
            for wave in 0..4 {
                let mut patch = IronfishPatch::default();
                patch.filter.kind = FilterKind::from_index(filter);
                patch.filter.resonance = 0.995;
                patch.lfo.wave = LfoWave::from_index(wave);
                patch.filter.lfo_amount = 1.0;
                let mut synth = Ironfish::new(44_100.0, patch);
                synth.note_on(84, 127);
                for _ in 0..8_192 {
                    let frame = synth.next_frame();
                    assert!(frame[0].is_finite() && frame[1].is_finite());
                }
            }
        }
    }

    #[test]
    fn root_and_scale_map_the_grid_like_the_historical_sequencer() {
        let mut patch = IronfishPatch::default();
        patch.root = RootNote::D;
        patch.scale = ScaleKind::Minor;
        assert_eq!(patch.grid_note(0), 50);
        assert_eq!(patch.grid_note(1), 52);
        assert_eq!(patch.grid_note(7), 62);
    }

    #[test]
    fn every_program_survives_long_44100_render_with_effects() {
        for index in 0..8 {
            let mut patch = IronfishPatch::preset(index);
            patch.delay.send = 0.45;
            patch.delay.feedback = 0.9;
            patch.chorus.mix = 0.8;
            patch.chorus.feedback = 0.7;
            patch.reverb.mix = 0.55;
            patch.reverb.feedback = 0.8;
            let mut synth = Ironfish::new(44_100.0, patch);
            synth.note_on(72, 120);
            for frame in 0..132_300 {
                if frame == 44_100 {
                    synth.note_off(72);
                }
                let sample = synth.next_frame();
                assert!(sample[0].is_finite() && sample[1].is_finite());
            }
        }
    }

    #[test]
    fn arpeggiator_advances_only_from_clock_steps() {
        let mut patch = IronfishPatch::default();
        patch.arp_enabled = true;
        patch.arp_octaves = 1;
        let mut synth = Ironfish::new(48_000.0, patch);
        synth.note_on(60, 100);
        synth.note_on(64, 110);
        assert_eq!(synth.arp_note, None);
        synth.clock_step();
        assert_eq!(synth.arp_note, Some(60));
        synth.clock_step();
        assert_eq!(synth.arp_note, Some(64));
        synth.clock_step();
        assert_eq!(synth.arp_note, Some(72));
    }
}
