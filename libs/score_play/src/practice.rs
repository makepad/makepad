//! Practice mixer, protected click routing, loop-boundary tempo training, and audio seam.

use std::array;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartMix {
    pub gain: f32,
    /// Equal-power pan in `-1.0..=1.0`.
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
}

impl Default for PartMix {
    fn default() -> Self {
        Self {
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MixSnapshot {
    pub gain: f32,
    pub left: f32,
    pub right: f32,
    pub audible: bool,
}

struct AtomicPartMix {
    gain: AtomicU32,
    pan: AtomicU32,
    flags: AtomicU8,
}

impl AtomicPartMix {
    fn new() -> Self {
        Self {
            gain: AtomicU32::new(1.0f32.to_bits()),
            pan: AtomicU32::new(0.0f32.to_bits()),
            flags: AtomicU8::new(0),
        }
    }

    fn store(&self, mix: PartMix) {
        self.gain.store(mix.gain.max(0.0).to_bits(), Ordering::Release);
        self.pan
            .store(mix.pan.clamp(-1.0, 1.0).to_bits(), Ordering::Release);
        self.flags
            .store(u8::from(mix.mute) | (u8::from(mix.solo) << 1), Ordering::Release);
    }

    fn load(&self) -> PartMix {
        let flags = self.flags.load(Ordering::Acquire);
        PartMix {
            gain: f32::from_bits(self.gain.load(Ordering::Acquire)),
            pan: f32::from_bits(self.pan.load(Ordering::Acquire)),
            mute: flags & 1 != 0,
            solo: flags & 2 != 0,
        }
    }
}

/// Atomically configured per-part mixer. The audio thread snapshots it once per quantum.
pub struct PartMixer<const PARTS: usize> {
    parts: [AtomicPartMix; PARTS],
}

impl<const PARTS: usize> Default for PartMixer<PARTS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const PARTS: usize> PartMixer<PARTS> {
    pub fn new() -> Self {
        Self {
            parts: array::from_fn(|_| AtomicPartMix::new()),
        }
    }

    pub fn set(&self, part: usize, mix: PartMix) -> bool {
        if let Some(slot) = self.parts.get(part) {
            slot.store(mix);
            true
        } else {
            false
        }
    }

    /// Fills caller-owned fixed storage; no allocation or per-sample atomic read occurs.
    pub fn snapshot(&self, output: &mut [MixSnapshot; PARTS]) {
        let mut any_solo = false;
        for part in &self.parts {
            any_solo |= part.load().solo;
        }
        for (source, target) in self.parts.iter().zip(output.iter_mut()) {
            let mix = source.load();
            let audible = !mix.mute && (!any_solo || mix.solo);
            let angle = (mix.pan + 1.0) * std::f32::consts::FRAC_PI_4;
            let gain = if audible { mix.gain } else { 0.0 };
            *target = MixSnapshot {
                gain,
                left: gain * angle.cos(),
                right: gain * angle.sin(),
                audible,
            };
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProtectedMixConfig {
    /// 0.501187 is -6 dB, reserving space for clicks and coincident voices.
    pub music_gain: f32,
    pub click_gain: f32,
    pub duck_music_gain: f32,
    pub duck_frames: u32,
    pub ducking: bool,
    pub limiter_ceiling: f32,
}

impl Default for ProtectedMixConfig {
    fn default() -> Self {
        Self {
            music_gain: 0.501_187_2,
            click_gain: 0.9,
            duck_music_gain: 0.707_945_76,
            duck_frames: 960,
            ducking: true,
            limiter_ceiling: 0.98,
        }
    }
}

/// Tiny stateful post-mixer for a click bus routed after the music bus.
pub struct ProtectedMix {
    config: ProtectedMixConfig,
    duck_remaining: u32,
}

impl ProtectedMix {
    pub fn new(config: ProtectedMixConfig) -> Self {
        Self {
            config,
            duck_remaining: 0,
        }
    }

    pub fn click_attack(&mut self) {
        if self.config.ducking {
            self.duck_remaining = self.duck_remaining.max(self.config.duck_frames);
        }
    }

    pub fn mix_frame(
        &mut self,
        music_left: f32,
        music_right: f32,
        click_left: f32,
        click_right: f32,
    ) -> (f32, f32) {
        let duck = if self.duck_remaining > 0 {
            self.duck_remaining -= 1;
            self.config.duck_music_gain
        } else {
            1.0
        };
        let ceiling = self.config.limiter_ceiling.clamp(0.01, 1.0);
        let left = (music_left * self.config.music_gain * duck
            + click_left * self.config.click_gain)
            .clamp(-ceiling, ceiling);
        let right = (music_right * self.config.music_gain * duck
            + click_right * self.config.click_gain)
            .clamp(-ceiling, ceiling);
        (left, right)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TempoRampStep {
    Bpm(f64),
    Percent(f64),
}

/// Tempo changes only when the owner reports a completed loop boundary.
pub struct TempoTrainer {
    current_bpm: f64,
    target_bpm: f64,
    step: TempoRampStep,
}

impl TempoTrainer {
    pub fn new(start_bpm: f64, target_bpm: f64, step: TempoRampStep) -> Option<Self> {
        if !start_bpm.is_finite()
            || !target_bpm.is_finite()
            || start_bpm <= 0.0
            || target_bpm < start_bpm
        {
            return None;
        }
        match step {
            TempoRampStep::Bpm(value) if value > 0.0 && value.is_finite() => {}
            TempoRampStep::Percent(value) if value > 0.0 && value.is_finite() => {}
            _ => return None,
        }
        Some(Self {
            current_bpm: start_bpm,
            target_bpm,
            step,
        })
    }

    pub fn current_bpm(&self) -> f64 {
        self.current_bpm
    }

    pub fn on_loop_boundary(&mut self, plan_base_bpm: f64) -> f64 {
        let next = match self.step {
            TempoRampStep::Bpm(value) => self.current_bpm + value,
            TempoRampStep::Percent(value) => self.current_bpm * (1.0 + value / 100.0),
        };
        self.current_bpm = next.min(self.target_bpm);
        if plan_base_bpm > 0.0 {
            self.current_bpm / plan_base_bpm
        } else {
            1.0
        }
    }
}

/// Worker-side seam for imported backing audio.
///
/// Symbolic score playback changes event rate and therefore preserves pitch for free;
/// it does not use time stretching. An implementation of this trait may prepare
/// pitch-preserving imported audio ahead of the callback and feed a separate bounded
/// audio ring. Spectral processing must never run inside the device callback.
pub trait AudioStretchWorker {
    type Error;

    fn set_tempo_ratio(&mut self, ratio: f64) -> Result<(), Self::Error>;
    fn process_ahead(&mut self, input: &[f32], output: &mut [f32]) -> Result<usize, Self::Error>;
    fn input_latency_frames(&self) -> u32;
    fn output_latency_frames(&self) -> u32;
}
