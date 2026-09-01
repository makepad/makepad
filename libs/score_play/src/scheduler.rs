//! Control-thread compiler from an already-unfolded score plan to sample events.

use crate::event::{
    EventSource, ExpressionValue, NoteId, PartId, ScheduledEvent, SynthEvent, SynthEventKind,
    Velocity,
};
use crate::metronome::{Meter, Metronome};
use crate::tempo::TempoMap;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Articulation {
    Normal,
    Staccato,
    Tenuto,
    Accent,
    Marcato,
    /// Explicit gate and attack multipliers from a playback style.
    Custom { gate: f32, attack: f32 },
}

impl Articulation {
    fn gate_and_attack(self) -> (f64, f32) {
        match self {
            Self::Normal => (0.90, 1.0),
            Self::Staccato => (0.50, 1.0),
            Self::Tenuto => (0.95, 1.0),
            Self::Accent => (0.90, 1.18),
            Self::Marcato => (0.62, 1.30),
            Self::Custom { gate, attack } => {
                (f64::from(gate.clamp(0.01, 1.5)), attack.clamp(0.0, 4.0))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DynamicCurve {
    pub quiet_velocity: Velocity,
    pub loud_velocity: Velocity,
    pub gamma: f32,
}

impl Default for DynamicCurve {
    fn default() -> Self {
        Self {
            quiet_velocity: 6_000,
            loud_velocity: 58_000,
            gamma: 0.72,
        }
    }
}

impl DynamicCurve {
    pub fn velocity(self, dynamic: f32, attack: f32) -> Velocity {
        let shaped = dynamic.clamp(0.0, 1.0).powf(self.gamma.max(0.01));
        let span = f32::from(self.loud_velocity.saturating_sub(self.quiet_velocity));
        let velocity = (f32::from(self.quiet_velocity) + span * shaped) * attack;
        velocity.clamp(1.0, f32::from(u16::MAX)).round() as u16
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Swing {
    /// The eligible subdivision, in quarter notes (0.5 means written eighths).
    pub unit_quarters: f64,
    /// Fraction of each two-unit pair occupied by the first unit (2/3 is triplet swing).
    pub first_fraction: f64,
}

impl Swing {
    pub fn transform(self, quarter: f64) -> f64 {
        if self.unit_quarters <= 0.0 || !(0.5..1.0).contains(&self.first_fraction) {
            return quarter;
        }
        let pair = self.unit_quarters * 2.0;
        let pair_start = (quarter / pair).floor() * pair;
        let local = quarter - pair_start;
        if (local - self.unit_quarters).abs() <= 1.0e-9 {
            pair_start + pair * self.first_fraction
        } else {
            quarter
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteInput {
    pub at_quarter: f64,
    pub duration_quarters: f64,
    pub part: PartId,
    pub note_id: NoteId,
    pub key: u8,
    /// Normalized notation dynamic after any instrument-specific interpretation.
    pub dynamic: f32,
    pub articulation: Articulation,
    /// False for tuplets, grace notes, or intentionally unequal rhythm.
    pub swing_eligible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PedalInput {
    pub at_quarter: f64,
    pub part: PartId,
    pub value: ExpressionValue,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScoreControlInput {
    pub at_quarter: f64,
    pub part: PartId,
    pub control: u16,
    pub value: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HairpinInput {
    pub at_quarter: f64,
    pub duration_quarters: f64,
    pub part: PartId,
    pub from: ExpressionValue,
    pub to: ExpressionValue,
}

/// Inputs have already had repeats, voltas, D.S./D.C., Fine, and Coda unfolded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlanInput {
    pub notes: Vec<NoteInput>,
    pub pedals: Vec<PedalInput>,
    pub controls: Vec<ScoreControlInput>,
    pub hairpins: Vec<HairpinInput>,
    pub end_quarter: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CountInSpec {
    pub meter: Meter,
    pub bars: u8,
    /// Zero gives primary pulses only; one adds denominator-unit subdivisions.
    pub subdivisions_per_unit: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScheduleOptions {
    pub swing: Option<Swing>,
    pub count_in: Option<CountInSpec>,
    pub dynamic_curve: DynamicCurve,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleError {
    InvalidEnd,
    InvalidNote,
    InvalidControlTime,
    InvalidHairpin,
    EventAfterEnd,
    CountInOverflow,
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot compile playback plan: {self:?}")
    }
}

impl std::error::Error for ScheduleError {}

/// Prepared controller and sounding-note state at a seek or loop point.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackCheckpoint {
    pub at: u64,
    pub restore: Vec<SynthEvent>,
}

/// Immutable data read by the audio thread. All allocation happens while compiling it.
#[derive(Clone, Debug, PartialEq)]
pub struct PerformancePlan {
    tempo_map: TempoMap,
    events: Vec<ScheduledEvent>,
    checkpoints: Vec<PlaybackCheckpoint>,
    score_origin_sample: u64,
    end_sample: u64,
}

impl PerformancePlan {
    pub fn tempo_map(&self) -> &TempoMap {
        &self.tempo_map
    }

    pub fn events(&self) -> &[ScheduledEvent] {
        &self.events
    }

    pub fn checkpoints(&self) -> &[PlaybackCheckpoint] {
        &self.checkpoints
    }

    pub fn score_origin_sample(&self) -> u64 {
        self.score_origin_sample
    }

    pub fn end_sample(&self) -> u64 {
        self.end_sample
    }

    pub fn score_quarter_to_sample(&self, quarter: f64) -> u64 {
        self.score_origin_sample
            .saturating_add(self.tempo_map.quarter_to_sample(quarter))
    }

    pub fn sample_to_score_quarter(&self, sample: u64) -> f64 {
        if sample >= self.score_origin_sample {
            return self
                .tempo_map
                .sample_to_quarter(sample - self.score_origin_sample);
        }
        let before = (self.score_origin_sample - sample) as f64;
        -before * self.tempo_map.initial_bpm()
            / (60.0 * f64::from(self.tempo_map.sample_rate()))
    }

    pub fn checkpoint_at(&self, at: u64) -> Option<&PlaybackCheckpoint> {
        self.checkpoints.iter().find(|checkpoint| checkpoint.at == at)
    }

    /// Prepares controller, pedal, expression, and sounding-note state for a discontinuity.
    pub fn prepare_checkpoint(&mut self, at: u64) {
        let mut active: Vec<SynthEvent> = Vec::new();
        let mut state: Vec<SynthEvent> = Vec::new();
        for scheduled in self.events.iter().copied().take_while(|event| event.at < at) {
            match scheduled.event.kind {
                SynthEventKind::NoteOn { .. } => active.push(scheduled.event),
                SynthEventKind::NoteOff { note_id, .. } => {
                    if let Some(index) = active.iter().position(|event| {
                        matches!(event.kind, SynthEventKind::NoteOn { note_id: id, .. } if id == note_id)
                    }) {
                        active.remove(index);
                    }
                }
                SynthEventKind::Pedal { part, .. } => replace_part_state(&mut state, part, true, scheduled.event),
                SynthEventKind::Control { part, control, .. } => {
                    replace_control_state(&mut state, part, control, scheduled.event)
                }
                SynthEventKind::ExpressionRamp {
                    part,
                    from,
                    to,
                    end_sample,
                } => {
                    let restored = if end_sample <= at {
                        SynthEvent {
                            source: EventSource::Playback,
                            kind: SynthEventKind::ExpressionRamp {
                                part,
                                from: to,
                                to,
                                end_sample: at,
                            },
                        }
                    } else {
                        let span = end_sample.saturating_sub(scheduled.at).max(1);
                        let elapsed = at.saturating_sub(scheduled.at).min(span);
                        let value = f64::from(from)
                            + (f64::from(to) - f64::from(from)) * elapsed as f64 / span as f64;
                        SynthEvent {
                            source: EventSource::Playback,
                            kind: SynthEventKind::ExpressionRamp {
                                part,
                                from: value.round() as u16,
                                to,
                                end_sample,
                            },
                        }
                    };
                    replace_expression_state(&mut state, part, restored);
                }
                SynthEventKind::Click { .. }
                | SynthEventKind::AllNotesOff { .. }
                | SynthEventKind::TransportReset { .. } => {}
            }
        }
        state.extend(active);
        self.checkpoints.retain(|checkpoint| checkpoint.at != at);
        self.checkpoints.push(PlaybackCheckpoint { at, restore: state });
        self.checkpoints.sort_by_key(|checkpoint| checkpoint.at);
    }

    pub(crate) fn lower_bound(&self, sample: u64) -> usize {
        self.events.partition_point(|event| event.at < sample)
    }
}

pub struct Scheduler;

impl Scheduler {
    pub fn compile(
        input: PlanInput,
        tempo_map: TempoMap,
        options: ScheduleOptions,
    ) -> Result<PerformancePlan, ScheduleError> {
        if !input.end_quarter.is_finite() || input.end_quarter < 0.0 {
            return Err(ScheduleError::InvalidEnd);
        }
        let sample_rate = tempo_map.sample_rate();
        let initial_bpm = tempo_map.initial_bpm();
        let (score_origin_sample, mut events) = if let Some(count_in) = options.count_in {
            Metronome::count_in(
                sample_rate,
                initial_bpm,
                count_in.meter,
                count_in.bars,
                count_in.subdivisions_per_unit,
            )
            .ok_or(ScheduleError::CountInOverflow)?
        } else {
            (0, Vec::new())
        };

        let mut sequence = events.len() as u32;
        for note in input.notes {
            if !valid_time(note.at_quarter)
                || !note.duration_quarters.is_finite()
                || note.duration_quarters <= 0.0
                || note.key > 127
                || !note.dynamic.is_finite()
            {
                return Err(ScheduleError::InvalidNote);
            }
            if note.at_quarter + note.duration_quarters > input.end_quarter + 1.0e-9 {
                return Err(ScheduleError::EventAfterEnd);
            }
            let (gate, attack) = note.articulation.gate_and_attack();
            let mut onset = note.at_quarter;
            let mut end = note.at_quarter + note.duration_quarters * gate;
            if note.swing_eligible {
                if let Some(swing) = options.swing {
                    onset = swing.transform(onset);
                    end = swing.transform(end);
                }
            }
            let on_sample = score_origin_sample.saturating_add(tempo_map.quarter_to_sample(onset));
            let off_sample = score_origin_sample
                .saturating_add(tempo_map.quarter_to_sample(end))
                .max(on_sample.saturating_add(1));
            let velocity = options.dynamic_curve.velocity(note.dynamic, attack);
            events.push(ScheduledEvent {
                at: on_sample,
                sequence,
                event: SynthEvent {
                    source: EventSource::Playback,
                    kind: SynthEventKind::NoteOn {
                        part: note.part,
                        note_id: note.note_id,
                        key: note.key,
                        velocity,
                        attack: (attack * 32_768.0).clamp(0.0, 65_535.0).round() as u16,
                    },
                },
            });
            sequence = sequence.wrapping_add(1);
            events.push(ScheduledEvent {
                at: off_sample,
                sequence,
                event: SynthEvent {
                    source: EventSource::Playback,
                    kind: SynthEventKind::NoteOff {
                        part: note.part,
                        note_id: note.note_id,
                        release: 32_768,
                    },
                },
            });
            sequence = sequence.wrapping_add(1);
        }

        for pedal in input.pedals {
            if !valid_time(pedal.at_quarter) || pedal.at_quarter > input.end_quarter {
                return Err(ScheduleError::InvalidControlTime);
            }
            events.push(ScheduledEvent {
                at: score_origin_sample
                    .saturating_add(tempo_map.quarter_to_sample(pedal.at_quarter)),
                sequence,
                event: SynthEvent {
                    source: EventSource::Playback,
                    kind: SynthEventKind::Pedal {
                        part: pedal.part,
                        value: pedal.value,
                    },
                },
            });
            sequence = sequence.wrapping_add(1);
        }
        for control in input.controls {
            if !valid_time(control.at_quarter) || control.at_quarter > input.end_quarter {
                return Err(ScheduleError::InvalidControlTime);
            }
            events.push(ScheduledEvent {
                at: score_origin_sample
                    .saturating_add(tempo_map.quarter_to_sample(control.at_quarter)),
                sequence,
                event: SynthEvent {
                    source: EventSource::Playback,
                    kind: SynthEventKind::Control {
                        part: control.part,
                        control: control.control,
                        value: control.value,
                    },
                },
            });
            sequence = sequence.wrapping_add(1);
        }
        for hairpin in input.hairpins {
            if !valid_time(hairpin.at_quarter)
                || !hairpin.duration_quarters.is_finite()
                || hairpin.duration_quarters <= 0.0
                || hairpin.at_quarter + hairpin.duration_quarters > input.end_quarter + 1.0e-9
            {
                return Err(ScheduleError::InvalidHairpin);
            }
            let at = score_origin_sample
                .saturating_add(tempo_map.quarter_to_sample(hairpin.at_quarter));
            let end_sample = score_origin_sample.saturating_add(
                tempo_map.quarter_to_sample(hairpin.at_quarter + hairpin.duration_quarters),
            );
            events.push(ScheduledEvent {
                at,
                sequence,
                event: SynthEvent {
                    source: EventSource::Playback,
                    kind: SynthEventKind::ExpressionRamp {
                        part: hairpin.part,
                        from: hairpin.from,
                        to: hairpin.to,
                        end_sample,
                    },
                },
            });
            sequence = sequence.wrapping_add(1);
        }

        events.sort_by_key(|event| event.key());
        let end_sample = score_origin_sample
            .saturating_add(tempo_map.quarter_to_sample(input.end_quarter))
            .max(events.last().map_or(0, |event| event.at));
        Ok(PerformancePlan {
            tempo_map,
            events,
            checkpoints: Vec::new(),
            score_origin_sample,
            end_sample,
        })
    }
}

fn valid_time(time: f64) -> bool {
    time.is_finite() && time >= 0.0
}

fn replace_part_state(state: &mut Vec<SynthEvent>, part: PartId, pedal: bool, event: SynthEvent) {
    if let Some(index) = state.iter().position(|existing| match existing.kind {
        SynthEventKind::Pedal { part: existing, .. } => pedal && existing == part,
        _ => false,
    }) {
        state[index] = event;
    } else {
        state.push(event);
    }
}

fn replace_control_state(
    state: &mut Vec<SynthEvent>,
    part: PartId,
    control: u16,
    event: SynthEvent,
) {
    if let Some(index) = state.iter().position(|existing| {
        matches!(existing.kind, SynthEventKind::Control { part: p, control: c, .. } if p == part && c == control)
    }) {
        state[index] = event;
    } else {
        state.push(event);
    }
}

fn replace_expression_state(state: &mut Vec<SynthEvent>, part: PartId, event: SynthEvent) {
    if let Some(index) = state.iter().position(|existing| {
        matches!(existing.kind, SynthEventKind::ExpressionRamp { part: p, .. } if p == part)
    }) {
        state[index] = event;
    } else {
        state.push(event);
    }
}
