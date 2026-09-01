/// Inclusive MIDI key or velocity range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub low: u8,
    pub high: u8,
}

impl Range {
    pub const ALL: Self = Self { low: 0, high: 127 };

    pub const fn contains(self, value: u8) -> bool {
        value >= self.low && value <= self.high
    }

    pub const fn intersection(self, other: Self) -> Option<Self> {
        let low = if self.low > other.low { self.low } else { other.low };
        let high = if self.high < other.high { self.high } else { other.high };
        if low <= high {
            Some(Self { low, high })
        } else {
            None
        }
    }
}

impl Default for Range {
    fn default() -> Self {
        Self::ALL
    }
}

/// Whether and for how long the sample loop remains active.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoopMode {
    #[default]
    NoLoop,
    Continuous,
    /// Loop while the key is held, then play from the current position to end.
    UntilRelease,
}

/// Linear amplitude-envelope times in seconds and a normalized sustain level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Envelope {
    pub delay: f32,
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl Default for Envelope {
    fn default() -> Self {
        Self {
            delay: 0.0,
            attack: 0.001,
            hold: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.001,
        }
    }
}

/// Render-time sound source. All variants are `Copy` and own no heap data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceSource {
    Sample { sample_id: u32 },
    ProceduralPiano,
    Metronome { accent: bool },
}

/// Fully resolved, render-ready parameters for one selected zone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceParameters {
    pub source: VoiceSource,
    pub key: u8,
    pub velocity: u8,
    pub root_key: f32,
    pub tune_cents: f32,
    /// Cents of pitch change per MIDI key (normally 100).
    pub scale_tuning: f32,
    pub sample_rate: u32,
    /// Sample-source-relative frame bounds; `end_frame` is exclusive.
    pub start_frame: i64,
    pub end_frame: i64,
    /// Loop end is exclusive.
    pub loop_start: i64,
    pub loop_end: i64,
    pub loop_mode: LoopMode,
    /// False for SFZ `one_shot` regions and self-terminating transients.
    pub release_on_note_off: bool,
    pub envelope: Envelope,
    /// Constant gain after generator/SFZ volume and velocity response.
    pub gain: f32,
    /// Normalized -1 (left) through +1 (right).
    pub pan: f32,
    pub filter_cutoff_hz: f32,
    pub filter_resonance_db: f32,
    pub exclusive_class: u16,
}

impl VoiceParameters {
    /// Playback frames advanced per output frame.
    pub fn pitch_ratio(self, output_sample_rate: f32) -> f64 {
        if output_sample_rate <= 0.0 || self.sample_rate == 0 {
            return 0.0;
        }
        let cents = (self.key as f32 - self.root_key) * self.scale_tuning + self.tune_cents;
        (self.sample_rate as f64 / output_sample_rate as f64)
            * 2.0_f64.powf(cents as f64 / 1200.0)
    }
}

/// A selected zone before key-dependent envelope scaling is applied.
#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub program: u16,
    pub bank: u16,
    pub key_range: Range,
    pub velocity_range: Range,
    pub parameters: VoiceParameters,
    pub fixed_key: Option<u8>,
    pub fixed_velocity: Option<u8>,
    /// SoundFont key tracking, in timecents/key around MIDI key 60.
    pub key_to_hold: i16,
    pub key_to_decay: i16,
}

impl Zone {
    pub fn matches(&self, program: u16, bank: u16, key: u8, velocity: u8) -> bool {
        self.program == program
            && self.bank == bank
            && self.key_range.contains(key)
            && self.velocity_range.contains(velocity)
    }

    pub fn voice_parameters(&self, key: u8, velocity: u8) -> VoiceParameters {
        let mut result = self.parameters;
        result.key = self.fixed_key.unwrap_or(key);
        result.velocity = self.fixed_velocity.unwrap_or(velocity);
        let hold_scale = self.key_to_hold as f32 * (60.0 - result.key as f32);
        let decay_scale = self.key_to_decay as f32 * (60.0 - result.key as f32);
        if hold_scale != 0.0 && result.envelope.hold > 0.0 {
            result.envelope.hold *= 2.0_f32.powf(hold_scale / 1200.0);
        }
        if decay_scale != 0.0 && result.envelope.decay > 0.0 {
            result.envelope.decay *= 2.0_f32.powf(decay_scale / 1200.0);
        }
        // A smooth velocity curve stands in for the SF2 default velocity-to-
        // attenuation modulator. It is deterministic and never reaches exact
        // silence for a legal note-on velocity.
        let normalized = (result.velocity as f32 / 127.0).sqrt();
        result.gain *= normalized;
        result
    }
}

/// Pure selection over already-resolved zones.
pub fn select_zones(
    zones: &[Zone],
    program: u16,
    bank: u16,
    key: u8,
    velocity: u8,
) -> Vec<VoiceParameters> {
    zones
        .iter()
        .filter(|zone| zone.matches(program, bank, key, velocity))
        .map(|zone| zone.voice_parameters(key, velocity))
        .collect()
}

/// One stereo frame returned without allocation by a sample store.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SampleRead {
    Resident { left: f32, right: f32 },
    /// The frame/page is unavailable (including an external SoundFont ROM).
    Missing,
}

/// Real-time sample-access seam. Implementations must not block or allocate.
/// A paged store reports a cache miss with [`SampleRead::Missing`]; the
/// sampler writes silence for that voice and increments its miss report.
pub trait SampleSource {
    fn read_frame(&self, sample_id: u32, frame: i64) -> SampleRead;
}

/// Source useful for procedural-only playback.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoSamples;

impl SampleSource for NoSamples {
    fn read_frame(&self, _sample_id: u32, _frame: i64) -> SampleRead {
        SampleRead::Missing
    }
}

pub(crate) fn timecents_to_seconds(value: i32) -> f32 {
    if value <= -32_768 {
        0.0
    } else {
        2.0_f32.powf(value as f32 / 1200.0)
    }
}

pub(crate) fn cents_to_hz(value: i32) -> f32 {
    8.176_f32 * 2.0_f32.powf(value as f32 / 1200.0)
}
