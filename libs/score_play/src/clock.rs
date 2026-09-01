//! Seqlock-published presentation anchor for UI-only interpolation.

use crate::scheduler::PerformancePlan;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PlaybackState {
    #[default]
    Stopped,
    Paused,
    Playing,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ClockQuality {
    /// Backend supplied a calibrated DAC presentation time and device latency.
    Exact,
    /// Host time or latency is estimated; audio event placement remains exact.
    #[default]
    Estimated,
}

/// One coherent audio presentation anchor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioClockSnapshot {
    pub stream_generation: u32,
    /// Monotonic audio-device frame at the anchor.
    pub device_sample: u64,
    /// Nominal performance-plan sample that reaches the DAC at `presentation_host_ns`.
    pub presentation_sample: f64,
    pub presentation_host_ns: u64,
    pub score_quarter: f64,
    pub sample_rate: u32,
    pub output_latency_frames: u32,
    pub state: PlaybackState,
    pub tempo_scale: f64,
    pub loop_start: u64,
    pub loop_end: u64,
    pub loop_enabled: bool,
    pub quality: ClockQuality,
}

impl Default for AudioClockSnapshot {
    fn default() -> Self {
        Self {
            stream_generation: 0,
            device_sample: 0,
            presentation_sample: 0.0,
            presentation_host_ns: 0,
            score_quarter: 0.0,
            sample_rate: 48_000,
            output_latency_frames: 0,
            state: PlaybackState::Stopped,
            tempo_scale: 1.0,
            loop_start: 0,
            loop_end: 0,
            loop_enabled: false,
            quality: ClockQuality::Estimated,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayPosition {
    pub device_sample: f64,
    pub timeline_sample: f64,
    pub score_quarter: f64,
    pub in_count_in: bool,
}

impl AudioClockSnapshot {
    /// Interpolates for one UI frame. This method is read-only and has no path back to
    /// transport state. Cursor and falling-note code should share this one result.
    pub fn estimate(self, plan: &PerformancePlan, host_time_ns: u64) -> DisplayPosition {
        let running = self.state == PlaybackState::Playing && self.sample_rate != 0;
        let host_delta_ns = host_time_ns as i128 - self.presentation_host_ns as i128;
        let device_delta = if running {
            host_delta_ns as f64 * f64::from(self.sample_rate) / 1_000_000_000.0
        } else {
            0.0
        };
        let device_sample = (self.device_sample as f64 + device_delta).max(0.0);
        let mut timeline_sample = self.presentation_sample + device_delta * self.tempo_scale;
        if self.loop_enabled && self.loop_end > self.loop_start {
            let start = self.loop_start as f64;
            let length = (self.loop_end - self.loop_start) as f64;
            if timeline_sample >= self.loop_end as f64 {
                timeline_sample = start + (timeline_sample - start).rem_euclid(length);
            }
        }
        timeline_sample = timeline_sample.max(0.0);
        let origin = plan.score_origin_sample() as f64;
        let score_quarter = if timeline_sample >= origin {
            let seconds = (timeline_sample - origin) / f64::from(plan.tempo_map().sample_rate());
            plan.tempo_map().seconds_to_quarter(seconds)
        } else {
            -(origin - timeline_sample) * plan.tempo_map().initial_bpm()
                / (60.0 * f64::from(plan.tempo_map().sample_rate()))
        };
        DisplayPosition {
            device_sample,
            timeline_sample,
            score_quarter,
            in_count_in: timeline_sample < origin,
        }
    }
}

/// Single-writer, multi-reader atomic publication using a sequence lock.
///
/// The audio thread is the sole writer. Readers retry if they overlap a publication;
/// no mutex, allocation, or callback into UI code exists on either side.
pub struct AtomicAudioClock {
    sequence: AtomicU64,
    stream_generation: AtomicU64,
    device_sample: AtomicU64,
    presentation_sample: AtomicU64,
    presentation_host_ns: AtomicU64,
    score_quarter: AtomicU64,
    sample_rate: AtomicU64,
    output_latency_frames: AtomicU64,
    state: AtomicU64,
    tempo_scale: AtomicU64,
    loop_start: AtomicU64,
    loop_end: AtomicU64,
    loop_enabled: AtomicU64,
    quality: AtomicU64,
}

impl Default for AtomicAudioClock {
    fn default() -> Self {
        Self::new()
    }
}

impl AtomicAudioClock {
    pub const fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            stream_generation: AtomicU64::new(0),
            device_sample: AtomicU64::new(0),
            presentation_sample: AtomicU64::new(0.0f64.to_bits()),
            presentation_host_ns: AtomicU64::new(0),
            score_quarter: AtomicU64::new(0.0f64.to_bits()),
            sample_rate: AtomicU64::new(48_000),
            output_latency_frames: AtomicU64::new(0),
            state: AtomicU64::new(0),
            tempo_scale: AtomicU64::new(1.0f64.to_bits()),
            loop_start: AtomicU64::new(0),
            loop_end: AtomicU64::new(0),
            loop_enabled: AtomicU64::new(0),
            quality: AtomicU64::new(1),
        }
    }

    /// Publishes one snapshot. Only the audio thread may call this concurrently.
    pub fn publish(&self, snapshot: AudioClockSnapshot) {
        self.sequence.fetch_add(1, Ordering::AcqRel);
        self.stream_generation
            .store(u64::from(snapshot.stream_generation), Ordering::Relaxed);
        self.device_sample.store(snapshot.device_sample, Ordering::Relaxed);
        self.presentation_sample
            .store(snapshot.presentation_sample.to_bits(), Ordering::Relaxed);
        self.presentation_host_ns
            .store(snapshot.presentation_host_ns, Ordering::Relaxed);
        self.score_quarter
            .store(snapshot.score_quarter.to_bits(), Ordering::Relaxed);
        self.sample_rate
            .store(u64::from(snapshot.sample_rate), Ordering::Relaxed);
        self.output_latency_frames
            .store(u64::from(snapshot.output_latency_frames), Ordering::Relaxed);
        self.state.store(encode_state(snapshot.state), Ordering::Relaxed);
        self.tempo_scale
            .store(snapshot.tempo_scale.to_bits(), Ordering::Relaxed);
        self.loop_start.store(snapshot.loop_start, Ordering::Relaxed);
        self.loop_end.store(snapshot.loop_end, Ordering::Relaxed);
        self.loop_enabled
            .store(u64::from(snapshot.loop_enabled), Ordering::Relaxed);
        self.quality.store(encode_quality(snapshot.quality), Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);
    }

    pub fn read(&self) -> AudioClockSnapshot {
        loop {
            let before = self.sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let snapshot = AudioClockSnapshot {
                stream_generation: self.stream_generation.load(Ordering::Relaxed) as u32,
                device_sample: self.device_sample.load(Ordering::Relaxed),
                presentation_sample: f64::from_bits(self.presentation_sample.load(Ordering::Relaxed)),
                presentation_host_ns: self.presentation_host_ns.load(Ordering::Relaxed),
                score_quarter: f64::from_bits(self.score_quarter.load(Ordering::Relaxed)),
                sample_rate: self.sample_rate.load(Ordering::Relaxed) as u32,
                output_latency_frames: self.output_latency_frames.load(Ordering::Relaxed) as u32,
                state: decode_state(self.state.load(Ordering::Relaxed)),
                tempo_scale: f64::from_bits(self.tempo_scale.load(Ordering::Relaxed)),
                loop_start: self.loop_start.load(Ordering::Relaxed),
                loop_end: self.loop_end.load(Ordering::Relaxed),
                loop_enabled: self.loop_enabled.load(Ordering::Relaxed) != 0,
                quality: decode_quality(self.quality.load(Ordering::Relaxed)),
            };
            let after = self.sequence.load(Ordering::Acquire);
            if before == after {
                return snapshot;
            }
        }
    }
}

fn encode_state(state: PlaybackState) -> u64 {
    match state {
        PlaybackState::Stopped => 0,
        PlaybackState::Paused => 1,
        PlaybackState::Playing => 2,
    }
}

fn decode_state(value: u64) -> PlaybackState {
    match value {
        1 => PlaybackState::Paused,
        2 => PlaybackState::Playing,
        _ => PlaybackState::Stopped,
    }
}

fn encode_quality(quality: ClockQuality) -> u64 {
    match quality {
        ClockQuality::Exact => 0,
        ClockQuality::Estimated => 1,
    }
}

fn decode_quality(value: u64) -> ClockQuality {
    if value == 0 {
        ClockQuality::Exact
    } else {
        ClockQuality::Estimated
    }
}
