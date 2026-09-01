//! Immediate hover/click audition and deliberate drag-speed scrubbing policy.

use crate::event::{EventSource, NoteId, PartId, SynthEvent, SynthEventKind, Velocity};
use crate::realtime::{AudioMessage, AudioMessageKind};

const MAX_BATCH_EVENTS: usize = 64;

/// Fixed message batch created on the control/UI thread and pushed into an SPSC ring.
#[derive(Clone, Copy, Debug)]
pub struct EventBatch {
    events: [AudioMessage; MAX_BATCH_EVENTS],
    len: usize,
}

impl Default for EventBatch {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBatch {
    pub const fn new() -> Self {
        Self {
            events: [AudioMessage::EMPTY; MAX_BATCH_EVENTS],
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_slice(&self) -> &[AudioMessage] {
        &self.events[..self.len]
    }

    fn push(&mut self, event: AudioMessage) -> bool {
        if let Some(slot) = self.events.get_mut(self.len) {
            *slot = event;
            self.len += 1;
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditionError {
    TooManyNotes,
    InvalidPitch,
}

#[derive(Clone, Copy, Debug, Default)]
struct ActiveVoice {
    key: u8,
    note_id: NoteId,
    part: PartId,
}

/// Tracks audition-owned voices so crossing notes rapidly always releases the old chord.
pub struct AuditionController<const MAX_NOTES: usize> {
    active: [ActiveVoice; MAX_NOTES],
    active_len: usize,
    next_note_id: u64,
    sequence: u32,
}

impl<const MAX_NOTES: usize> Default for AuditionController<MAX_NOTES> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const MAX_NOTES: usize> AuditionController<MAX_NOTES> {
    pub const fn new() -> Self {
        Self {
            active: [ActiveVoice {
                key: 0,
                note_id: 0,
                part: 0,
            }; MAX_NOTES],
            active_len: 0,
            next_note_id: 1,
            sequence: 0,
        }
    }

    pub fn active_voice_count(&self) -> usize {
        self.active_len
    }

    /// Auditions with a musically useful mezzo-forte default velocity.
    pub fn audition_default(
        &mut self,
        at_device_sample: u64,
        part: PartId,
        pitches: &[u8],
    ) -> Result<EventBatch, AuditionError> {
        self.audition(at_device_sample, part, pitches, 40_000, 240)
    }

    /// Replaces the currently auditioned chord at the exact supplied audio-device sample.
    /// Identical pitch sets are de-duplicated, avoiding hover churn and envelope clicks.
    pub fn audition(
        &mut self,
        at_device_sample: u64,
        part: PartId,
        pitches: &[u8],
        velocity: Velocity,
        release_frames: u32,
    ) -> Result<EventBatch, AuditionError> {
        if pitches.len() > MAX_NOTES || pitches.len().saturating_mul(2) > MAX_BATCH_EVENTS {
            return Err(AuditionError::TooManyNotes);
        }
        if pitches.iter().any(|pitch| *pitch > 127) {
            return Err(AuditionError::InvalidPitch);
        }
        let mut sorted = [0u8; MAX_NOTES];
        sorted[..pitches.len()].copy_from_slice(pitches);
        sorted[..pitches.len()].sort_unstable();
        let unchanged = pitches.len() == self.active_len
            && self.active[..self.active_len]
                .iter()
                .map(|voice| voice.key)
                .eq(sorted[..pitches.len()].iter().copied());
        if unchanged {
            return Ok(EventBatch::new());
        }

        let mut batch = self.release(at_device_sample, release_frames);
        for key in sorted[..pitches.len()].iter().copied() {
            let note_id = (1u64 << 63) | self.next_note_id;
            self.next_note_id = self.next_note_id.wrapping_add(1).max(1);
            let message = self.message(
                at_device_sample,
                SynthEvent {
                    source: EventSource::Audition,
                    kind: SynthEventKind::NoteOn {
                        part,
                        note_id,
                        key,
                        velocity: velocity.max(1),
                        attack: 32_768,
                    },
                },
            );
            let _ = batch.push(message);
            self.active[self.active_len] = ActiveVoice { key, note_id, part };
            self.active_len += 1;
        }
        Ok(batch)
    }

    pub fn release(&mut self, at_device_sample: u64, release_frames: u32) -> EventBatch {
        let mut batch = EventBatch::new();
        for index in 0..self.active_len {
            let voice = self.active[index];
            let message = self.message(
                at_device_sample,
                SynthEvent {
                    source: EventSource::Audition,
                    kind: SynthEventKind::NoteOff {
                        part: voice.part,
                        note_id: voice.note_id,
                        release: release_frames.min(u32::from(u16::MAX)) as u16,
                    },
                },
            );
            let _ = batch.push(message);
        }
        self.active_len = 0;
        batch
    }

    fn message(&mut self, at_device_sample: u64, event: SynthEvent) -> AudioMessage {
        let result = AudioMessage {
            at_device_sample,
            sequence: self.sequence,
            kind: AudioMessageKind::Synth(event),
        };
        self.sequence = self.sequence.wrapping_add(1);
        result
    }
}

/// Granular scrub policy. Trigger timing follows cursor samples, never score tempo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrubConfig {
    /// Hard trigger-rate ceiling; 35 ms is about 28 grains/second.
    pub min_interval_samples: u32,
    /// Same score hit cannot retrigger within this window.
    pub dedup_samples: u32,
    /// Fixed short grain, independent of notated duration.
    pub gate_samples: u32,
    /// Release applied when a newer grain supersedes the prior one.
    pub release_samples: u32,
    pub minimum_velocity: Velocity,
    pub maximum_velocity: Velocity,
    /// Cursor speed at which the velocity curve reaches half its range.
    pub speed_reference: f32,
}

impl ScrubConfig {
    pub fn for_sample_rate(sample_rate: u32) -> Self {
        Self {
            min_interval_samples: sample_rate.saturating_mul(35) / 1_000,
            dedup_samples: sample_rate.saturating_mul(120) / 1_000,
            gate_samples: sample_rate.saturating_mul(90) / 1_000,
            release_samples: sample_rate.saturating_mul(5) / 1_000,
            minimum_velocity: 14_000,
            maximum_velocity: 48_000,
            speed_reference: 8.0,
        }
    }
}

pub struct ScrubHit<'a> {
    /// Stable notation hit identity, e.g. chord/rest-cell id.
    pub token: u64,
    pub part: PartId,
    pub pitches: &'a [u8],
    pub cursor_units_per_second: f32,
}

#[derive(Clone, Copy, Debug)]
pub enum ScrubOutcome {
    Triggered(EventBatch),
    RateLimited,
    Duplicate,
    Invalid,
}

/// Fixed-storage granular retrigger controller for scrub gestures.
pub struct ScrubController<const MAX_NOTES: usize> {
    config: ScrubConfig,
    active: [ActiveVoice; MAX_NOTES],
    active_len: usize,
    active_until: u64,
    last_trigger: Option<u64>,
    last_token: Option<u64>,
    next_note_id: u64,
    sequence: u32,
}

impl<const MAX_NOTES: usize> ScrubController<MAX_NOTES> {
    pub const fn new(config: ScrubConfig) -> Self {
        Self {
            config,
            active: [ActiveVoice {
                key: 0,
                note_id: 0,
                part: 0,
            }; MAX_NOTES],
            active_len: 0,
            active_until: 0,
            last_trigger: None,
            last_token: None,
            next_note_id: 1,
            sequence: 0,
        }
    }

    pub fn update(&mut self, at_device_sample: u64, hit: ScrubHit<'_>) -> ScrubOutcome {
        if hit.pitches.is_empty()
            || hit.pitches.len() > MAX_NOTES
            || hit.pitches.len().saturating_mul(3) > MAX_BATCH_EVENTS
            || hit.pitches.iter().any(|pitch| *pitch > 127)
        {
            return ScrubOutcome::Invalid;
        }
        if let Some(last) = self.last_trigger {
            if at_device_sample.saturating_sub(last) < u64::from(self.config.min_interval_samples) {
                return ScrubOutcome::RateLimited;
            }
            if self.last_token == Some(hit.token)
                && at_device_sample.saturating_sub(last) < u64::from(self.config.dedup_samples)
            {
                return ScrubOutcome::Duplicate;
            }
        }
        if at_device_sample >= self.active_until {
            self.active_len = 0;
        }

        let mut batch = EventBatch::new();
        for index in 0..self.active_len {
            let voice = self.active[index];
            let message = self.message(
                at_device_sample,
                SynthEvent {
                    source: EventSource::Scrub,
                    kind: SynthEventKind::NoteOff {
                        part: voice.part,
                        note_id: voice.note_id,
                        release: self.config.release_samples.min(u32::from(u16::MAX)) as u16,
                    },
                },
            );
            let _ = batch.push(message);
        }
        self.active_len = 0;

        let speed = hit.cursor_units_per_second.abs();
        let fraction = speed / (speed + self.config.speed_reference.max(0.001));
        let velocity = f32::from(self.config.minimum_velocity)
            + f32::from(
                self.config
                    .maximum_velocity
                    .saturating_sub(self.config.minimum_velocity),
            ) * fraction;
        let velocity = velocity.round().clamp(1.0, f32::from(u16::MAX)) as u16;
        let off_sample = at_device_sample.saturating_add(u64::from(self.config.gate_samples));
        for key in hit.pitches.iter().copied() {
            let note_id = (1u64 << 62) | self.next_note_id;
            self.next_note_id = self.next_note_id.wrapping_add(1).max(1);
            let on = self.message(
                at_device_sample,
                SynthEvent {
                    source: EventSource::Scrub,
                    kind: SynthEventKind::NoteOn {
                        part: hit.part,
                        note_id,
                        key,
                        velocity,
                        attack: 32_768,
                    },
                },
            );
            let off = self.message(
                off_sample,
                SynthEvent {
                    source: EventSource::Scrub,
                    kind: SynthEventKind::NoteOff {
                        part: hit.part,
                        note_id,
                        release: self.config.release_samples.min(u32::from(u16::MAX)) as u16,
                    },
                },
            );
            let _ = batch.push(on);
            let _ = batch.push(off);
            self.active[self.active_len] = ActiveVoice {
                key,
                note_id,
                part: hit.part,
            };
            self.active_len += 1;
        }
        self.active_until = off_sample;
        self.last_trigger = Some(at_device_sample);
        self.last_token = Some(hit.token);
        ScrubOutcome::Triggered(batch)
    }

    fn message(&mut self, at_device_sample: u64, event: SynthEvent) -> AudioMessage {
        let result = AudioMessage {
            at_device_sample,
            sequence: self.sequence,
            kind: AudioMessageKind::Synth(event),
        };
        self.sequence = self.sequence.wrapping_add(1);
        result
    }
}
