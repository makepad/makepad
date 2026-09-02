//! Sample-based acoustic drum kit using the CC BY-SA 3.0 Salamander
//! Drumkit by Alexander Holm. Sample loading and preparation happen off the
//! audio thread; [`DrumKit::trigger`] and [`DrumKit::process`] use fixed
//! storage and perform no allocation, locking or I/O.

mod wav;

use std::array;
use std::fs;
use std::path::Path;
use std::sync::Arc;

const POLYPHONY: usize = 12;
const MAX_LAYERS: usize = 6;
const SILENCE_THRESHOLD: f32 = 0.001; // -60 dBFS
const NORMALISED_PEAK: f32 = 0.707_945_76; // -3 dBFS

/// The compact Salamander subset installed by the AI hub. Kick and snare
/// retain extra round-robins; the other layers use one take each.
const SAMPLE_FILES: [&str; 37] = [
    "kick_OH_P_1.wav", "kick_OH_P_2.wav",
    "kick_OH_F_1.wav", "kick_OH_F_2.wav",
    "kick_OH_FF_1.wav", "kick_OH_FF_2.wav",
    "snare_OH_Ghost_1.wav", "snare_OH_Ghost_2.wav", "snare_OH_Ghost_3.wav",
    "snare_OH_MP_1.wav", "snare_OH_MP_2.wav", "snare_OH_MP_3.wav",
    "snare_OH_F_1.wav", "snare_OH_F_2.wav", "snare_OH_F_3.wav",
    "snare_OH_FF_1.wav", "snare_OH_FF_2.wav", "snare_OH_FF_3.wav",
    "snareStick_OH_F_1.wav",
    "hihatClosed_OH_P_1.wav", "hihatClosed_OH_F_1.wav",
    "hihatOpen_OH_P_1.wav", "hihatOpen_OH_F_1.wav", "hihatOpen_OH_FF_1.wav",
    "hihatFoot_OH_MP_1.wav",
    "hiTom_OH_P_1.wav", "hiTom_OH_F_1.wav", "hiTom_OH_FF_1.wav",
    "loTom_OH_PP_1.wav", "loTom_OH_MP_1.wav", "loTom_OH_FF_1.wav",
    "crash1_OH_P_1.wav", "crash1_OH_FF_1.wav", "crash2_OH_FF_1.wav",
    "ride1_OH_MP_1.wav", "ride1_OH_FF_1.wav", "ride1Bell_OH_F_1.wav",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DrumVoice {
    Kick,
    Snare,
    SideStick,
    HiHatClosed,
    HiHatOpen,
    HiHatPedal,
    TomHigh,
    TomMid,
    TomLow,
    TomFloor,
    Ride,
    RideBell,
    Crash,
    Clap,
}

impl DrumVoice {
    pub const ALL: [Self; 14] = [
        Self::Kick,
        Self::Snare,
        Self::SideStick,
        Self::HiHatClosed,
        Self::HiHatOpen,
        Self::HiHatPedal,
        Self::TomHigh,
        Self::TomMid,
        Self::TomLow,
        Self::TomFloor,
        Self::Ride,
        Self::RideBell,
        Self::Crash,
        Self::Clap,
    ];

    pub const fn gm_note(self) -> u8 {
        match self {
            Self::Kick => 36,
            Self::Snare => 38,
            Self::SideStick => 37,
            Self::HiHatClosed => 42,
            Self::HiHatOpen => 46,
            Self::HiHatPedal => 44,
            Self::TomHigh => 50,
            Self::TomMid => 48,
            Self::TomLow => 45,
            Self::TomFloor => 41,
            Self::Ride => 51,
            Self::RideBell => 53,
            Self::Crash => 49,
            Self::Clap => 39,
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Kick => 0,
            Self::Snare => 1,
            Self::SideStick => 2,
            Self::HiHatClosed => 3,
            Self::HiHatOpen => 4,
            Self::HiHatPedal => 5,
            Self::TomHigh => 6,
            Self::TomMid => 7,
            Self::TomLow => 8,
            Self::TomFloor => 9,
            Self::Ride => 10,
            Self::RideBell => 11,
            Self::Crash => 12,
            Self::Clap => 13,
        }
    }
}

impl From<DrumVoice> for u8 {
    fn from(voice: DrumVoice) -> Self {
        voice.gm_note()
    }
}

impl TryFrom<u8> for DrumVoice {
    type Error = ();

    fn try_from(note: u8) -> Result<Self, Self::Error> {
        match note {
            36 => Ok(Self::Kick),
            38 => Ok(Self::Snare),
            37 => Ok(Self::SideStick),
            42 => Ok(Self::HiHatClosed),
            46 => Ok(Self::HiHatOpen),
            44 => Ok(Self::HiHatPedal),
            50 => Ok(Self::TomHigh),
            48 => Ok(Self::TomMid),
            45 => Ok(Self::TomLow),
            41 => Ok(Self::TomFloor),
            51 => Ok(Self::Ride),
            53 => Ok(Self::RideBell),
            49 => Ok(Self::Crash),
            39 => Ok(Self::Clap),
            _ => Err(()),
        }
    }
}

struct Sample {
    frames: Box<[[f32; 2]]>,
    sample_rate: u32,
}

struct Layer {
    rank: u8,
    samples: Vec<Sample>,
}

#[derive(Default)]
struct VoiceBank {
    layers: Vec<Layer>,
}

/// Immutable, fully decoded sample storage. Construct this on a worker
/// thread, then hand it to [`DrumKit::set_bank`] as an [`Arc`].
pub struct SampleBank {
    voices: [VoiceBank; 14],
}

impl SampleBank {
    /// Load the hub-installed `OH` directory. Every file is decoded to f32
    /// stereo, leading samples below -60 dBFS are removed, then one shared
    /// gain per voice normalises its loudest layer to -3 dBFS.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let mut voices: [VoiceBank; 14] = array::from_fn(|_| VoiceBank::default());
        for name in SAMPLE_FILES {
            let (voice, rank) = classify(name)
                .ok_or_else(|| format!("internal drum sample name is not classified: {name}"))?;
            let path = dir.join(name);
            let bytes = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
            let decoded = wav::decode(&bytes).map_err(|error| format!("decode {}: {error}", path.display()))?;
            let first = decoded
                .frames
                .iter()
                .position(|frame| frame[0].abs().max(frame[1].abs()) >= SILENCE_THRESHOLD)
                .unwrap_or(decoded.frames.len());
            let frames = decoded.frames[first..].to_vec();
            if frames.is_empty() {
                return Err(format!("{} is silent after trimming", path.display()));
            }
            let bank = &mut voices[voice.index()];
            let layer = if let Some(index) = bank.layers.iter().position(|layer| layer.rank == rank) {
                &mut bank.layers[index]
            } else {
                bank.layers.push(Layer { rank, samples: Vec::new() });
                bank.layers.last_mut().unwrap()
            };
            layer.samples.push(Sample {
                frames: frames.into_boxed_slice(),
                sample_rate: decoded.sample_rate,
            });
        }

        // TomMid/TomFloor alias existing sample storage at playback time.
        // Clap is deliberately the only synthesised voice: three 8 ms noise
        // bursts 10 ms apart, followed by a 90 ms 1-2.5 kHz band-passed tail.
        voices[DrumVoice::Clap.index()].layers.push(Layer {
            rank: 4,
            samples: vec![synthesise_clap()],
        });

        for (index, bank) in voices.iter_mut().enumerate() {
            if matches!(index, 7 | 9) || bank.layers.is_empty() {
                continue;
            }
            bank.layers.sort_by_key(|layer| layer.rank);
            let peak = bank
                .layers
                .iter()
                .flat_map(|layer| &layer.samples)
                .flat_map(|sample| sample.frames.iter())
                .fold(0.0f32, |peak, frame| peak.max(frame[0].abs()).max(frame[1].abs()));
            if peak <= 0.0 || !peak.is_finite() {
                return Err(format!("{:?} sample set has no finite signal", DrumVoice::ALL[index]));
            }
            let gain = NORMALISED_PEAK / peak;
            for sample in bank.layers.iter_mut().flat_map(|layer| &mut layer.samples) {
                for frame in sample.frames.iter_mut() {
                    frame[0] *= gain;
                    frame[1] *= gain;
                }
            }
        }

        let bank = Self { voices };
        for voice in DrumVoice::ALL {
            let (source, _) = bank.source(voice);
            if source.layers.is_empty() {
                return Err(format!("no samples loaded for {voice:?}"));
            }
        }
        Ok(bank)
    }

    /// Human-readable velocity-layer and round-robin counts for logging.
    pub fn summary(&self) -> String {
        let mut parts = Vec::with_capacity(DrumVoice::ALL.len());
        for voice in DrumVoice::ALL {
            let (bank, rate) = self.source(voice);
            let counts = bank
                .layers
                .iter()
                .map(|layer| layer.samples.len().to_string())
                .collect::<Vec<_>>()
                .join("/");
            if rate == 1.0 {
                parts.push(format!("{voice:?}:{}[{counts}]", bank.layers.len()));
            } else {
                parts.push(format!("{voice:?}:{}[{counts}]@{rate:.3}", bank.layers.len()));
            }
        }
        parts.join(", ")
    }

    fn source(&self, voice: DrumVoice) -> (&VoiceBank, f64) {
        match voice {
            DrumVoice::TomMid => (&self.voices[DrumVoice::TomHigh.index()], 0.891),
            DrumVoice::TomFloor => (&self.voices[DrumVoice::TomLow.index()], 0.841),
            _ => (&self.voices[voice.index()], 1.0),
        }
    }
}

fn classify(name: &str) -> Option<(DrumVoice, u8)> {
    let voice = if name.starts_with("kick_OH_") {
        DrumVoice::Kick
    } else if name.starts_with("snare_OH_") {
        DrumVoice::Snare
    } else if name.starts_with("snareStick_OH_") {
        DrumVoice::SideStick
    } else if name.starts_with("hihatClosed_OH_") {
        DrumVoice::HiHatClosed
    } else if name.starts_with("hihatOpen_OH_") {
        DrumVoice::HiHatOpen
    } else if name.starts_with("hihatFoot_OH_") {
        DrumVoice::HiHatPedal
    } else if name.starts_with("hiTom_OH_") {
        DrumVoice::TomHigh
    } else if name.starts_with("loTom_OH_") {
        DrumVoice::TomLow
    } else if name.starts_with("ride1Bell_OH_") {
        DrumVoice::RideBell
    } else if name.starts_with("ride1_OH_") {
        DrumVoice::Ride
    } else if name.starts_with("crash1_OH_") || name.starts_with("crash2_OH_") {
        DrumVoice::Crash
    } else {
        return None;
    };
    let rank = [
        ("_PP_", 0),
        ("_P_", 1),
        ("_MP_", 2),
        ("_Ghost_", 3),
        ("_F_", 4),
        ("_FF_", 5),
    ]
    .iter()
    .find_map(|(needle, rank)| name.contains(needle).then_some(*rank))?;
    Some((voice, rank))
}

fn synthesise_clap() -> Sample {
    const RATE: u32 = 48_000;
    let frames = (0.12 * RATE as f32) as usize;
    let mut output = vec![[0.0f32; 2]; frames];
    let mut seed = 0x5a17_93d1u32;
    let mut noise = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as i32 as f32) / i32::MAX as f32
    };
    let burst_len = (0.008 * RATE as f32) as usize;
    let gap = (0.010 * RATE as f32) as usize;
    for burst in 0..3 {
        let start = burst * gap;
        for offset in 0..burst_len {
            let envelope = 1.0 - offset as f32 / burst_len as f32;
            let value = noise() * envelope * (0.9 - burst as f32 * 0.12);
            output[start + offset] = [value, value * 0.92];
        }
    }
    let tail_start = 2 * gap + burst_len;
    let tail_len = (0.090 * RATE as f32) as usize;
    let low_alpha = 1.0 - (-2.0 * std::f32::consts::PI * 2_500.0 / RATE as f32).exp();
    let high_alpha = 1.0 - (-2.0 * std::f32::consts::PI * 1_000.0 / RATE as f32).exp();
    let (mut low, mut slow) = (0.0f32, 0.0f32);
    for offset in 0..tail_len.min(frames - tail_start) {
        let white = noise();
        low += low_alpha * (white - low);
        slow += high_alpha * (white - slow);
        let envelope = (-5.0 * offset as f32 / tail_len as f32).exp();
        let value = (low - slow) * envelope * 1.7;
        output[tail_start + offset][0] += value;
        output[tail_start + offset][1] += value * 0.9;
    }
    Sample { frames: output.into_boxed_slice(), sample_rate: RATE }
}

#[derive(Clone, Copy)]
struct PlayingVoice {
    active: bool,
    source_voice: usize,
    layer: usize,
    sample: usize,
    position: f64,
    step: f64,
    gain: f32,
    age: u32,
    fade_in: u32,
    fade_remaining: u32,
    fade_total: u32,
    serial: u64,
    kind: DrumVoice,
}

impl PlayingVoice {
    const fn idle() -> Self {
        Self {
            active: false,
            source_voice: 0,
            layer: 0,
            sample: 0,
            position: 0.0,
            step: 1.0,
            gain: 0.0,
            age: 0,
            fade_in: 1,
            fade_remaining: 0,
            fade_total: 0,
            serial: 0,
            kind: DrumVoice::Kick,
        }
    }

    fn fade_out(&mut self, frames: u32) {
        if self.active && (self.fade_remaining == 0 || frames < self.fade_remaining) {
            self.fade_remaining = frames.max(1);
            self.fade_total = frames.max(1);
        }
    }
}

pub struct DrumKit {
    bank: Option<Arc<SampleBank>>,
    voices: [PlayingVoice; POLYPHONY],
    round_robin: [[usize; MAX_LAYERS]; 14],
    sample_rate: f32,
    serial: u64,
}

impl DrumKit {
    /// Construct an empty, silent player. Load a [`SampleBank`] off-thread
    /// and install it with [`Self::set_bank`].
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = if sample_rate.is_finite() {
            sample_rate.clamp(8_000.0, 384_000.0)
        } else {
            48_000.0
        };
        Self {
            bank: None,
            voices: [PlayingVoice::idle(); POLYPHONY],
            round_robin: [[0; MAX_LAYERS]; 14],
            sample_rate,
            serial: 0,
        }
    }

    /// Swap in a fully prepared bank. This is only an `Arc` pointer move and
    /// fixed-state reset; it performs no allocation or locking.
    pub fn set_bank(&mut self, bank: Arc<SampleBank>) {
        self.voices.fill(PlayingVoice::idle());
        self.round_robin = [[0; MAX_LAYERS]; 14];
        self.bank = Some(bank);
    }

    /// Start a hit, stealing the oldest of 12 voices when necessary.
    pub fn trigger(&mut self, voice: DrumVoice, velocity: f32) {
        let velocity = if velocity.is_finite() { velocity.clamp(0.0, 1.0) } else { 0.0 };
        if velocity <= 0.0 {
            return;
        }
        let Some(bank) = self.bank.as_ref() else { return };
        if matches!(voice, DrumVoice::HiHatClosed | DrumVoice::HiHatPedal) {
            let choke = (self.sample_rate * 0.030).round() as u32;
            for playing in &mut self.voices {
                if playing.kind == DrumVoice::HiHatOpen {
                    playing.fade_out(choke);
                }
            }
        }
        let (source, rate_multiplier) = bank.source(voice);
        let layer_index = velocity_layer(velocity, source.layers.len());
        let layer = &source.layers[layer_index];
        let rr = &mut self.round_robin[voice.index()][layer_index];
        let sample_index = *rr % layer.samples.len();
        *rr = rr.wrapping_add(1);
        let sample = &layer.samples[sample_index];
        self.serial = self.serial.wrapping_add(1);
        let slot = self.voices.iter().position(|playing| !playing.active).unwrap_or_else(|| {
            let mut oldest = 0;
            for index in 1..POLYPHONY {
                if self.voices[index].serial < self.voices[oldest].serial {
                    oldest = index;
                }
            }
            oldest
        });
        self.voices[slot] = PlayingVoice {
            active: true,
            source_voice: match voice {
                DrumVoice::TomMid => DrumVoice::TomHigh.index(),
                DrumVoice::TomFloor => DrumVoice::TomLow.index(),
                _ => voice.index(),
            },
            layer: layer_index,
            sample: sample_index,
            position: 0.0,
            step: sample.sample_rate as f64 / self.sample_rate as f64 * rate_multiplier,
            gain: velocity.powf(1.6),
            age: 0,
            fade_in: (self.sample_rate * 0.005).round().max(1.0) as u32,
            fade_remaining: 0,
            fade_total: 0,
            serial: self.serial,
            kind: voice,
        };
    }

    /// Add this kit into stereo frames. No allocation, locking, blocking or
    /// I/O occurs here, and the result is independent of block boundaries.
    pub fn process(&mut self, out: &mut [[f32; 2]]) {
        let Some(bank) = self.bank.as_ref() else { return };
        for playing in &mut self.voices {
            if !playing.active {
                continue;
            }
            let sample = &bank.voices[playing.source_voice].layers[playing.layer].samples[playing.sample];
            for output in out.iter_mut() {
                let index = playing.position as usize;
                if index >= sample.frames.len() {
                    playing.active = false;
                    break;
                }
                let next = (index + 1).min(sample.frames.len() - 1);
                let fraction = (playing.position - index as f64) as f32;
                let a = sample.frames[index];
                let b = sample.frames[next];
                let mut envelope = (playing.age as f32 / playing.fade_in as f32).min(1.0);
                if playing.fade_remaining > 0 {
                    envelope *= playing.fade_remaining as f32 / playing.fade_total as f32;
                }
                output[0] += (a[0] + (b[0] - a[0]) * fraction) * playing.gain * envelope;
                output[1] += (a[1] + (b[1] - a[1]) * fraction) * playing.gain * envelope;
                playing.position += playing.step;
                playing.age = playing.age.saturating_add(1);
                if playing.fade_remaining > 0 {
                    playing.fade_remaining -= 1;
                    if playing.fade_remaining == 0 {
                        playing.active = false;
                        break;
                    }
                }
            }
        }
    }

    /// Fade every active hit to silence over 20 ms.
    pub fn all_off(&mut self) {
        let frames = (self.sample_rate * 0.020).round() as u32;
        for voice in &mut self.voices {
            voice.fade_out(frames);
        }
    }

    pub fn active(&self) -> bool {
        self.voices.iter().any(|voice| voice.active)
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }
}

fn velocity_layer(velocity: f32, layers: usize) -> usize {
    match layers {
        0 | 1 => 0,
        2 => usize::from(velocity >= 0.5),
        3 => if velocity < 0.3 { 0 } else if velocity < 0.75 { 1 } else { 2 },
        4 => if velocity < 0.2 { 0 } else if velocity < 0.45 { 1 } else if velocity < 0.75 { 2 } else { 3 },
        count => ((velocity * count as f32) as usize).min(count - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone_bank(frequency: f32, rate: u32) -> Arc<SampleBank> {
        let len = rate as usize;
        let frames = (0..len)
            .map(|index| {
                let value = (2.0 * std::f32::consts::PI * frequency * index as f32 / rate as f32).sin() * 0.5;
                [value, value]
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let mut voices: [VoiceBank; 14] = array::from_fn(|_| VoiceBank::default());
        voices[DrumVoice::Kick.index()].layers.push(Layer {
            rank: 4,
            samples: vec![Sample { frames, sample_rate: rate }],
        });
        Arc::new(SampleBank { voices })
    }

    #[test]
    fn gm_notes_round_trip() {
        for voice in DrumVoice::ALL {
            assert_eq!(DrumVoice::try_from(u8::from(voice)), Ok(voice));
        }
        assert!(DrumVoice::try_from(35).is_err());
    }

    #[test]
    fn empty_kit_is_silent() {
        let mut kit = DrumKit::new(48_000.0);
        kit.trigger(DrumVoice::Kick, 1.0);
        let mut output = [[0.0; 2]; 32];
        kit.process(&mut output);
        assert!(!kit.active());
        assert_eq!(output, [[0.0; 2]; 32]);
    }

    #[test]
    fn resampler_keeps_one_khz_within_half_percent_at_44k1() {
        let mut kit = DrumKit::new(44_100.0);
        kit.set_bank(tone_bank(1_000.0, 48_000));
        kit.trigger(DrumVoice::Kick, 1.0);
        let mut output = vec![[0.0; 2]; 22_050];
        kit.process(&mut output);
        let crossings = output[441..]
            .windows(2)
            .filter(|pair| pair[0][0] <= 0.0 && pair[1][0] > 0.0)
            .count();
        let seconds = (output.len() - 441) as f32 / 44_100.0;
        let measured = crossings as f32 / seconds;
        assert!((measured - 1_000.0).abs() / 1_000.0 < 0.005, "measured {measured} Hz");
    }
}
