//! Allocation-free audio-thread render kernel.

use crate::clock::{
    AtomicAudioClock, AudioClockSnapshot, ClockQuality, PlaybackState,
};
use crate::event::{EventSource, SynthEvent, SynthEventKind};
use crate::practice::{MixSnapshot, PartMixer};
use crate::ring::SpscRing;
use crate::scheduler::PerformancePlan;
use std::ops::Range;

const PHASE_ONE: u128 = 1u128 << 32;

/// Event position supplied to a synth. `block_offset` is exact within the current callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SynthEventTiming {
    pub block_offset: usize,
    pub device_sample: u64,
    pub timeline_sample: Option<u64>,
    /// Nominal timeline samples advanced per device frame, in unsigned Q32.
    pub timeline_rate_q32: u64,
}

/// Plug-in seam shared by sampler, modelled piano, audition, scrub, and playback.
///
/// The engine renders spans up to an event boundary, then calls `dispatch` with the exact
/// callback offset. Implementations must use fixed voice/storage capacity and obey the same
/// no-allocation, no-locking, no-I/O, no-logging, no-panic callback contract as the engine.
pub trait SynthBackend {
    /// Called once per callback with a mixer snapshot read once for that quantum.
    fn begin_block(&mut self, _parts: &[MixSnapshot]) {}

    fn dispatch(&mut self, event: SynthEvent, timing: SynthEventTiming);

    /// Render/add the half-open subrange. The slice lengths were validated by the engine.
    fn render_range(&mut self, channels: &mut [&mut [f32]], range: Range<usize>);

    fn voice_count(&self) -> usize;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TransportLoop {
    pub start: u64,
    pub end: u64,
}

impl TransportLoop {
    pub fn new(start: u64, end: u64) -> Option<Self> {
        (start < end).then_some(Self { start, end })
    }

    /// Converts semantic score/bar endpoints through the plan's tempo map.
    pub fn from_score_range(
        plan: &PerformancePlan,
        start_quarter: f64,
        end_quarter: f64,
    ) -> Option<Self> {
        if !start_quarter.is_finite()
            || !end_quarter.is_finite()
            || start_quarter < 0.0
            || start_quarter >= end_quarter
        {
            return None;
        }
        Self::new(
            plan.score_quarter_to_sample(start_quarter),
            plan.score_quarter_to_sample(end_quarter),
        )
    }
}

/// A control or interactive message timestamped in the monotonic device-sample domain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AudioMessageKind {
    Play,
    Pause,
    Stop,
    Seek { timeline_sample: u64 },
    SetLoop { range: TransportLoop, enabled: bool },
    SetTempoScale { scale: f64 },
    Synth(SynthEvent),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioMessage {
    pub at_device_sample: u64,
    pub sequence: u32,
    pub kind: AudioMessageKind,
}

impl AudioMessage {
    pub const EMPTY: Self = Self {
        at_device_sample: 0,
        sequence: 0,
        kind: AudioMessageKind::Pause,
    };

    fn key(self) -> (u64, u32) {
        (self.at_device_sample, self.sequence)
    }
}

/// Timing supplied by the platform at the start of an audio callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderContext {
    pub first_device_sample: u64,
    /// The output device's sample rate. The plan is compiled at its own rate,
    /// which need not match the device (48 kHz plan on a 44.1 kHz output is
    /// normal), so the engine converts device frames to plan samples with this.
    /// Zero means "assume the plan's rate", preserving the old behaviour.
    pub device_sample_rate: u32,
    /// Host time at which the callback's first frame is expected to reach the DAC.
    pub first_presentation_host_ns: u64,
    pub frames: usize,
    pub output_latency_frames: u32,
    pub stream_generation: u32,
    pub clock_quality: ClockQuality,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderStatus {
    Rendered,
    InvalidBuffer,
    PendingOverflow,
}

#[derive(Clone, Copy)]
enum LocalDiscontinuity {
    None,
    Release,
    Restore(u64),
}

/// Audio-owned transport and dispatcher.
///
/// Nominal timeline position is Q32 fixed point. Tempo scaling changes its rate against
/// device frames, so symbolic notes retain pitch and timing is invariant to callback size.
pub struct PlaybackEngine<B, const PENDING: usize, const PARTS: usize> {
    backend: B,
    state: PlaybackState,
    phase_q32: u128,
    rate_q32: u64,
    loop_range: TransportLoop,
    loop_enabled: bool,
    event_cursor: usize,
    pending: [AudioMessage; PENDING],
    pending_len: usize,
    pending_overflows: u64,
    loop_count: u64,
    mix_snapshot: [MixSnapshot; PARTS],
    local_discontinuity: LocalDiscontinuity,
}

impl<B: SynthBackend, const PENDING: usize, const PARTS: usize>
    PlaybackEngine<B, PENDING, PARTS>
{
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            state: PlaybackState::Stopped,
            phase_q32: 0,
            rate_q32: PHASE_ONE as u64,
            loop_range: TransportLoop { start: 0, end: 0 },
            loop_enabled: false,
            event_cursor: 0,
            pending: [AudioMessage::EMPTY; PENDING],
            pending_len: 0,
            pending_overflows: 0,
            loop_count: 0,
            mix_snapshot: [MixSnapshot {
                gain: 0.0,
                left: 0.0,
                right: 0.0,
                audible: false,
            }; PARTS],
            local_discontinuity: LocalDiscontinuity::None,
        }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn state(&self) -> PlaybackState {
        self.state
    }

    pub fn timeline_sample(&self) -> u64 {
        (self.phase_q32 >> 32).min(u128::from(u64::MAX)) as u64
    }

    pub fn tempo_scale(&self) -> f64 {
        self.rate_q32 as f64 / PHASE_ONE as f64
    }

    pub fn loop_count(&self) -> u64 {
        self.loop_count
    }

    pub fn pending_overflow_count(&self) -> u64 {
        self.pending_overflows
    }

    /// Audio-owner convenience. Cross-thread callers should enqueue an `AudioMessage`.
    pub fn play(&mut self, plan: &PerformancePlan) {
        self.begin_playing(plan);
    }

    pub fn pause(&mut self) {
        self.state = PlaybackState::Paused;
        self.local_discontinuity = LocalDiscontinuity::Release;
    }

    pub fn stop(&mut self) {
        self.state = PlaybackState::Stopped;
        self.phase_q32 = 0;
        self.event_cursor = 0;
        self.local_discontinuity = LocalDiscontinuity::Release;
    }

    pub fn seek(&mut self, plan: &PerformancePlan, timeline_sample: u64) {
        let target = timeline_sample.min(plan.end_sample());
        self.phase_q32 = u128::from(target) << 32;
        self.event_cursor = plan.lower_bound(target);
        self.local_discontinuity = LocalDiscontinuity::Restore(target);
    }

    pub fn set_loop(&mut self, range: Option<TransportLoop>) {
        if let Some(range) = range {
            self.loop_range = range;
            self.loop_enabled = range.start < range.end;
        } else {
            self.loop_enabled = false;
        }
    }

    pub fn set_tempo_scale(&mut self, scale: f64) -> bool {
        if !scale.is_finite() || !(0.05..=8.0).contains(&scale) {
            return false;
        }
        self.rate_q32 = (scale * PHASE_ONE as f64).round() as u64;
        true
    }

    // BEGIN REALTIME_RENDER_PATH
    /// Enters `Playing` from the current position. A transport parked at the end of a
    /// non-looping plan rewinds first: otherwise `stop_if_due` would stop it again in the
    /// same block and the press would produce nothing.
    fn begin_playing(&mut self, plan: &PerformancePlan) {
        if !self.loop_enabled && self.timeline_sample() >= plan.end_sample() {
            self.phase_q32 = 0;
        }
        self.state = PlaybackState::Playing;
        self.event_cursor = plan.lower_bound(self.timeline_sample());
    }

    pub fn render<const RING: usize>(
        &mut self,
        context: RenderContext,
        channels: &mut [&mut [f32]],
        plan: &PerformancePlan,
        messages: &SpscRing<AudioMessage, RING>,
        mixer: &PartMixer<PARTS>,
        clock: &AtomicAudioClock,
    ) -> RenderStatus {
        if channels.iter().any(|channel| channel.len() < context.frames)
            || plan.tempo_map().sample_rate() == 0
        {
            return RenderStatus::InvalidBuffer;
        }
        // One device frame is not one plan sample unless the rates agree.
        let plan_rate = plan.tempo_map().sample_rate();
        let device_rate = if context.device_sample_rate == 0 { plan_rate } else { context.device_sample_rate };
        let rate_ratio = f64::from(plan_rate) / f64::from(device_rate.max(1));
        let effective_rate_q32 =
            ((self.rate_q32 as f64) * rate_ratio).round().clamp(0.0, u64::MAX as f64) as u64;
        mixer.snapshot(&mut self.mix_snapshot);
        self.backend.begin_block(&self.mix_snapshot);
        self.apply_local_discontinuity(context.first_device_sample, plan);
        let overflow_before = self.pending_overflows;
        while let Some(message) = messages.pop() {
            self.insert_pending(message);
        }

        let mut offset = 0usize;
        while offset < context.frames {
            let device_sample = context.first_device_sample.saturating_add(offset as u64);
            self.apply_messages_due(device_sample, offset, plan);
            if self.state == PlaybackState::Playing {
                self.wrap_if_due(device_sample, offset, plan);
                self.apply_plan_due(device_sample, offset, plan);
                self.stop_if_due(device_sample, offset, plan);
            }

            let remaining = context.frames - offset;
            let mut advance = remaining;
            if let Some(message) = self.pending.first().filter(|_| self.pending_len > 0) {
                let frames = message.at_device_sample.saturating_sub(device_sample) as usize;
                if frames > 0 {
                    advance = advance.min(frames);
                }
            }
            if self.state == PlaybackState::Playing {
                if let Some(event) = plan.events().get(self.event_cursor) {
                    advance = advance.min(self.frames_until(event.at));
                }
                if self.loop_enabled {
                    advance = advance.min(self.frames_until(self.loop_range.end));
                } else {
                    advance = advance.min(self.frames_until(plan.end_sample()));
                }
            }
            if advance == 0 {
                advance = 1.min(remaining);
            }
            let end = offset + advance;
            self.backend.render_range(channels, offset..end);
            if self.state == PlaybackState::Playing {
                self.phase_q32 = self
                    .phase_q32
                    .saturating_add(u128::from(effective_rate_q32) * advance as u128);
            }
            offset = end;
        }

        let anchor_device = context
            .first_device_sample
            .saturating_add(context.frames as u64);
        self.apply_messages_due(anchor_device, context.frames, plan);
        if self.state == PlaybackState::Playing {
            self.wrap_if_due(anchor_device, context.frames, plan);
            self.apply_plan_due(anchor_device, context.frames, plan);
            self.stop_if_due(anchor_device, context.frames, plan);
        }
        let host_advance = if plan.tempo_map().sample_rate() == 0 {
            0
        } else {
            (context.frames as u128 * 1_000_000_000u128 / u128::from(device_rate.max(1))) as u64
        };
        let presentation_sample = self.phase_q32 as f64 / PHASE_ONE as f64;
        let score_quarter = score_quarter_at(plan, presentation_sample);
        clock.publish(AudioClockSnapshot {
            stream_generation: context.stream_generation,
            device_sample: anchor_device,
            presentation_sample,
            presentation_host_ns: context
                .first_presentation_host_ns
                .saturating_add(host_advance),
            score_quarter,
            sample_rate: plan.tempo_map().sample_rate(),
            output_latency_frames: context.output_latency_frames,
            state: self.state,
            tempo_scale: effective_rate_q32 as f64 / PHASE_ONE as f64,
            loop_start: self.loop_range.start,
            loop_end: self.loop_range.end,
            loop_enabled: self.loop_enabled,
            quality: context.clock_quality,
        });
        if self.pending_overflows != overflow_before {
            RenderStatus::PendingOverflow
        } else {
            RenderStatus::Rendered
        }
    }

    fn insert_pending(&mut self, message: AudioMessage) {
        if self.pending_len >= PENDING {
            self.pending_overflows = self.pending_overflows.saturating_add(1);
            return;
        }
        let mut index = self.pending_len;
        while index > 0 && self.pending[index - 1].key() > message.key() {
            self.pending[index] = self.pending[index - 1];
            index -= 1;
        }
        self.pending[index] = message;
        self.pending_len += 1;
    }

    fn apply_local_discontinuity(&mut self, device_sample: u64, plan: &PerformancePlan) {
        let discontinuity = self.local_discontinuity;
        self.local_discontinuity = LocalDiscontinuity::None;
        match discontinuity {
            LocalDiscontinuity::None => {}
            LocalDiscontinuity::Release => self.release_playback(device_sample, 0, 128),
            LocalDiscontinuity::Restore(timeline_sample) => {
                self.restore_at(device_sample, 0, timeline_sample, plan)
            }
        }
    }

    fn apply_messages_due(
        &mut self,
        device_sample: u64,
        block_offset: usize,
        plan: &PerformancePlan,
    ) {
        while self.pending_len > 0 && self.pending[0].at_device_sample <= device_sample {
            let message = self.pending[0];
            let mut index = 1;
            while index < self.pending_len {
                self.pending[index - 1] = self.pending[index];
                index += 1;
            }
            self.pending_len -= 1;
            self.apply_message(message, device_sample, block_offset, plan);
        }
    }

    fn apply_message(
        &mut self,
        message: AudioMessage,
        device_sample: u64,
        block_offset: usize,
        plan: &PerformancePlan,
    ) {
        match message.kind {
            AudioMessageKind::Play => self.begin_playing(plan),
            AudioMessageKind::Pause => {
                self.state = PlaybackState::Paused;
                self.release_playback(device_sample, block_offset, 128);
            }
            AudioMessageKind::Stop => {
                self.state = PlaybackState::Stopped;
                self.phase_q32 = 0;
                self.event_cursor = 0;
                self.release_playback(device_sample, block_offset, 128);
            }
            AudioMessageKind::Seek { timeline_sample } => {
                let target = timeline_sample.min(plan.end_sample());
                self.phase_q32 = u128::from(target) << 32;
                self.event_cursor = plan.lower_bound(target);
                self.restore_at(device_sample, block_offset, target, plan);
            }
            AudioMessageKind::SetLoop { range, enabled } => {
                self.loop_range = range;
                self.loop_enabled = enabled && range.start < range.end;
            }
            AudioMessageKind::SetTempoScale { scale } => {
                let _ = self.set_tempo_scale(scale);
            }
            AudioMessageKind::Synth(event) => self.backend.dispatch(
                event,
                SynthEventTiming {
                    block_offset,
                    device_sample,
                    timeline_sample: None,
                    timeline_rate_q32: self.rate_q32,
                },
            ),
        }
    }

    fn wrap_if_due(
        &mut self,
        device_sample: u64,
        block_offset: usize,
        plan: &PerformancePlan,
    ) {
        if !self.loop_enabled || self.loop_range.start >= self.loop_range.end {
            return;
        }
        let end = u128::from(self.loop_range.end) << 32;
        if self.phase_q32 < end {
            return;
        }
        let start = u128::from(self.loop_range.start) << 32;
        let length = end - start;
        let beyond = self.phase_q32 - end;
        let crossings = 1u128 + beyond / length;
        self.phase_q32 = start + beyond % length;
        self.loop_count = self
            .loop_count
            .saturating_add(crossings.min(u128::from(u64::MAX)) as u64);
        self.event_cursor = plan.lower_bound(self.loop_range.start);
        self.restore_at(device_sample, block_offset, self.loop_range.start, plan);
    }

    fn apply_plan_due(
        &mut self,
        device_sample: u64,
        block_offset: usize,
        plan: &PerformancePlan,
    ) {
        let timeline = self.timeline_sample();
        while let Some(scheduled) = plan.events().get(self.event_cursor).copied() {
            if scheduled.at > timeline
                || (self.loop_enabled && scheduled.at >= self.loop_range.end)
            {
                break;
            }
            self.backend.dispatch(
                scheduled.event,
                SynthEventTiming {
                    block_offset,
                    device_sample,
                    timeline_sample: Some(scheduled.at),
                    timeline_rate_q32: self.rate_q32,
                },
            );
            self.event_cursor += 1;
        }
    }

    fn stop_if_due(
        &mut self,
        device_sample: u64,
        block_offset: usize,
        plan: &PerformancePlan,
    ) {
        if !self.loop_enabled && self.timeline_sample() >= plan.end_sample() {
            self.state = PlaybackState::Stopped;
            self.release_playback(device_sample, block_offset, 128);
        }
    }

    fn restore_at(
        &mut self,
        device_sample: u64,
        block_offset: usize,
        timeline_sample: u64,
        plan: &PerformancePlan,
    ) {
        let crossfade_frames = plan.tempo_map().sample_rate().saturating_mul(7) / 1_000;
        self.release_playback(device_sample, block_offset, crossfade_frames);
        self.backend.dispatch(
            SynthEvent {
                source: EventSource::Playback,
                kind: SynthEventKind::TransportReset {
                    crossfade_frames,
                },
            },
            SynthEventTiming {
                block_offset,
                device_sample,
                timeline_sample: Some(timeline_sample),
                timeline_rate_q32: self.rate_q32,
            },
        );
        if let Some(checkpoint) = plan.checkpoint_at(timeline_sample) {
            for event in checkpoint.restore.iter().copied() {
                self.backend.dispatch(
                    event,
                    SynthEventTiming {
                        block_offset,
                        device_sample,
                        timeline_sample: Some(timeline_sample),
                        timeline_rate_q32: self.rate_q32,
                    },
                );
            }
        }
    }

    fn release_playback(&mut self, device_sample: u64, block_offset: usize, release_frames: u32) {
        self.backend.dispatch(
            SynthEvent {
                source: EventSource::Playback,
                kind: SynthEventKind::AllNotesOff {
                    source: EventSource::Playback,
                    release_frames,
                },
            },
            SynthEventTiming {
                block_offset,
                device_sample,
                timeline_sample: Some(self.timeline_sample()),
                timeline_rate_q32: self.rate_q32,
            },
        );
    }

    fn frames_until(&self, target_sample: u64) -> usize {
        let target = u128::from(target_sample) << 32;
        if target <= self.phase_q32 || self.rate_q32 == 0 {
            return 0;
        }
        let delta = target - self.phase_q32;
        let rate = u128::from(self.rate_q32);
        let frames = (delta + rate - 1) / rate;
        frames.min(usize::MAX as u128) as usize
    }
    // END REALTIME_RENDER_PATH
}

fn score_quarter_at(plan: &PerformancePlan, timeline_sample: f64) -> f64 {
    let origin = plan.score_origin_sample() as f64;
    if timeline_sample >= origin {
        let seconds = (timeline_sample - origin) / f64::from(plan.tempo_map().sample_rate());
        plan.tempo_map().seconds_to_quarter(seconds)
    } else {
        -(origin - timeline_sample) * plan.tempo_map().initial_bpm()
            / (60.0 * f64::from(plan.tempo_map().sample_rate()))
    }
}
