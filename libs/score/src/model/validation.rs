use super::graph::{EventKind, Score, SpannerEndpoint, SpannerKind};
use super::id::{EventId, MeasureId, NoteId, SpannerId, StaffId, VoiceId};
use super::pitch::Pitch;
use super::time::{Duration, ScoreTime};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValidationProblem {
    EntityIdMismatch { entity: &'static str },
    MissingPartForStaff { staff: StaffId },
    MissingStaffForVoice { voice: VoiceId, staff: StaffId },
    VoiceNotListedByStaff { voice: VoiceId, staff: StaffId },
    DuplicateEventId { event: EventId },
    DuplicateNoteId { note: NoteId },
    EventOrder { voice: VoiceId, event: EventId },
    VoiceOverlap {
        voice: VoiceId,
        first: EventId,
        second: EventId,
    },
    VoiceGap {
        voice: VoiceId,
        measure: MeasureId,
        start: ScoreTime,
        end: ScoreTime,
    },
    EventOutsideMeasure { event: EventId, measure: MeasureId },
    MeasureDurationMismatch {
        measure: MeasureId,
        expected: Duration,
        actual: Duration,
    },
    InvalidMeter { measure: MeasureId },
    MissingDisplayStaff { note: NoteId, staff: StaffId },
    DanglingTie { note: NoteId, target: NoteId },
    NonReciprocalTie { note: NoteId, target: NoteId },
    TiePitchMismatch { note: NoteId, target: NoteId },
    DanglingSpannerEndpoint { spanner: SpannerId },
    InvalidSlurEndpoints { spanner: SpannerId },
    DanglingLyricNote { note: NoteId },
    DanglingMelismaEnd { note: NoteId },
    FlowReferencesMissingMeasure { measure: MeasureId },
    FlowOmitsMeasure { measure: MeasureId },
    DuplicateFlowMeasure { measure: MeasureId },
}

impl Score {
    /// Checks meter, voice, tie, lyric, and spanner consistency without panicking.
    pub fn validate(&self) -> Vec<ValidationProblem> {
        let mut problems = validate_references(self);
        problems.extend(validate_measures_and_voices(self));
        problems.extend(validate_ties(self));
        problems.extend(validate_spanners(self));
        problems.extend(validate_lyrics(self));
        problems
    }
}

pub(crate) fn validate_edit_invariants(score: &Score) -> Vec<ValidationProblem> {
    let mut problems = validate_references(score);
    problems.extend(
        validate_measures_and_voices(score)
            .into_iter()
            .filter(|problem| {
                !matches!(
                    problem,
                    ValidationProblem::VoiceGap { .. }
                        | ValidationProblem::EventOutsideMeasure { .. }
                        | ValidationProblem::MeasureDurationMismatch { .. }
                        | ValidationProblem::InvalidMeter { .. }
                )
            }),
    );
    problems.extend(validate_ties(score));
    problems.extend(validate_spanners(score));
    problems.extend(validate_lyrics(score));
    problems
}

fn validate_references(score: &Score) -> Vec<ValidationProblem> {
    let mut problems = Vec::new();
    let mut events = BTreeSet::new();
    let mut notes = BTreeSet::new();

    for (id, part) in score.parts.iter() {
        if *id != part.id {
            problems.push(ValidationProblem::EntityIdMismatch { entity: "part" });
        }
    }
    for (id, staff) in score.staves.iter() {
        if *id != staff.id {
            problems.push(ValidationProblem::EntityIdMismatch { entity: "staff" });
        }
        if !score.parts.contains_key(&staff.part) {
            problems.push(ValidationProblem::MissingPartForStaff { staff: staff.id });
        }
    }
    for (id, voice) in score.voices.iter() {
        if *id != voice.id {
            problems.push(ValidationProblem::EntityIdMismatch { entity: "voice" });
        }
        match score.staves.get(&voice.staff) {
            Some(staff) if !staff.voices.contains(id) => {
                problems.push(ValidationProblem::VoiceNotListedByStaff {
                    voice: voice.id,
                    staff: voice.staff,
                });
            }
            None => problems.push(ValidationProblem::MissingStaffForVoice {
                voice: voice.id,
                staff: voice.staff,
            }),
            _ => {}
        }
        for event in &voice.events {
            if !events.insert(event.id) {
                problems.push(ValidationProblem::DuplicateEventId { event: event.id });
            }
            if let EventKind::Chord(chord) = &event.kind {
                for note in chord {
                    if !notes.insert(note.id) {
                        problems.push(ValidationProblem::DuplicateNoteId { note: note.id });
                    }
                    if !score.staves.contains_key(&note.display_staff) {
                        problems.push(ValidationProblem::MissingDisplayStaff {
                            note: note.id,
                            staff: note.display_staff,
                        });
                    }
                }
            }
        }
    }
    for (id, measure) in score.measures.iter() {
        if *id != measure.id {
            problems.push(ValidationProblem::EntityIdMismatch { entity: "measure" });
        }
    }
    for (id, spanner) in score.spanners.iter() {
        if *id != spanner.id {
            problems.push(ValidationProblem::EntityIdMismatch { entity: "spanner" });
        }
    }
    for (id, annotation) in score.annotations.iter() {
        if *id != annotation.id {
            problems.push(ValidationProblem::EntityIdMismatch {
                entity: "annotation",
            });
        }
    }
    let mut flow_measures = BTreeSet::new();
    for node in &score.flow.nodes {
        if !score.measures.contains_key(&node.measure) {
            problems.push(ValidationProblem::FlowReferencesMissingMeasure {
                measure: node.measure,
            });
        }
        if !flow_measures.insert(node.measure) {
            problems.push(ValidationProblem::DuplicateFlowMeasure {
                measure: node.measure,
            });
        }
    }
    for measure in score.measures.keys() {
        if !flow_measures.contains(measure) {
            problems.push(ValidationProblem::FlowOmitsMeasure { measure: *measure });
        }
    }
    problems
}

fn validate_measures_and_voices(score: &Score) -> Vec<ValidationProblem> {
    let mut problems = Vec::new();
    let mut measures: Vec<_> = score.measures.values().collect();
    measures.sort_by_key(|measure| (measure.start, measure.ordinal, measure.id));

    for measure in &measures {
        if let Some(meter) = score.maps.meter_at(measure.start, None, None) {
            match meter.duration() {
                Ok(Some(expected)) if expected != measure.extent => {
                    problems.push(ValidationProblem::MeasureDurationMismatch {
                        measure: measure.id,
                        expected,
                        actual: measure.extent,
                    });
                }
                Err(_) => problems.push(ValidationProblem::InvalidMeter {
                    measure: measure.id,
                }),
                _ => {}
            }
        }
    }

    for voice in score.voices.values() {
        let mut previous = None;
        for event in &voice.events {
            if let Some((previous_key, previous_id, previous_end)) = previous {
                if (event.onset, event.id) < previous_key {
                    problems.push(ValidationProblem::EventOrder {
                        voice: voice.id,
                        event: event.id,
                    });
                }
                if event.grace.is_none()
                    && event.duration.is_some()
                    && event.onset < previous_end
                {
                    problems.push(ValidationProblem::VoiceOverlap {
                        voice: voice.id,
                        first: previous_id,
                        second: event.id,
                    });
                }
            }
            if let Ok(end) = event.end() {
                if event.grace.is_none() && event.duration.is_some() {
                    previous = Some(((event.onset, event.id), event.id, end));
                }
            }
        }

        for measure in &measures {
            let Ok(measure_end) = measure.start.checked_add(measure.extent) else {
                continue;
            };
            let sounding_events: Vec<_> = voice
                .events
                .iter()
                .filter(|event| {
                    event.grace.is_none()
                        && event.duration.is_some()
                        && event.onset >= measure.start
                        && event.onset < measure_end
                })
                .collect();
            if sounding_events.is_empty() {
                continue;
            }
            let mut cursor = measure.start;
            for event in sounding_events {
                if event.onset > cursor {
                    problems.push(ValidationProblem::VoiceGap {
                        voice: voice.id,
                        measure: measure.id,
                        start: cursor,
                        end: event.onset,
                    });
                }
                if let Ok(end) = event.end() {
                    if end > measure_end {
                        problems.push(ValidationProblem::EventOutsideMeasure {
                            event: event.id,
                            measure: measure.id,
                        });
                    }
                    if end > cursor {
                        cursor = end;
                    }
                }
            }
            if cursor < measure_end {
                problems.push(ValidationProblem::VoiceGap {
                    voice: voice.id,
                    measure: measure.id,
                    start: cursor,
                    end: measure_end,
                });
            }
        }
    }
    problems
}

fn validate_ties(score: &Score) -> Vec<ValidationProblem> {
    let mut problems = Vec::new();
    let notes: BTreeMap<_, _> = score
        .voices
        .values()
        .flat_map(|voice| voice.events.iter())
        .flat_map(|event| event.chord_notes().iter())
        .map(|note| (note.id, note))
        .collect();
    for note in notes.values() {
        if let Some(target_id) = note.tie_to {
            match notes.get(&target_id) {
                None => problems.push(ValidationProblem::DanglingTie {
                    note: note.id,
                    target: target_id,
                }),
                Some(target) => {
                    if target.tie_from != Some(note.id) {
                        problems.push(ValidationProblem::NonReciprocalTie {
                            note: note.id,
                            target: target_id,
                        });
                    }
                    if !same_tie_pitch(note.written_pitch, target.written_pitch) {
                        problems.push(ValidationProblem::TiePitchMismatch {
                            note: note.id,
                            target: target_id,
                        });
                    }
                }
            }
        }
        if let Some(target_id) = note.tie_from {
            match notes.get(&target_id) {
                None => problems.push(ValidationProblem::DanglingTie {
                    note: note.id,
                    target: target_id,
                }),
                Some(target) if target.tie_to != Some(note.id) => {
                    problems.push(ValidationProblem::NonReciprocalTie {
                        note: note.id,
                        target: target_id,
                    });
                }
                _ => {}
            }
        }
    }
    problems
}

fn same_tie_pitch(left: Option<Pitch>, right: Option<Pitch>) -> bool {
    left == right && left.is_some()
}

fn validate_spanners(score: &Score) -> Vec<ValidationProblem> {
    let mut problems = Vec::new();
    for spanner in score.spanners.values() {
        if !endpoint_exists(score, spanner.start) || !endpoint_exists(score, spanner.end) {
            problems.push(ValidationProblem::DanglingSpannerEndpoint {
                spanner: spanner.id,
            });
        }
        if matches!(spanner.kind, SpannerKind::Slur { .. }) && spanner.start == spanner.end {
            problems.push(ValidationProblem::InvalidSlurEndpoints {
                spanner: spanner.id,
            });
        }
    }
    problems
}

fn endpoint_exists(score: &Score, endpoint: SpannerEndpoint) -> bool {
    match endpoint {
        SpannerEndpoint::Note(id) => score.note(id).is_some(),
        SpannerEndpoint::Event(id) => score.event(id).is_some(),
        SpannerEndpoint::StaffTime { staff, .. } => score.staves.contains_key(&staff),
    }
}

fn validate_lyrics(score: &Score) -> Vec<ValidationProblem> {
    let mut problems = Vec::new();
    for lyric in &score.lyrics {
        if score.note(lyric.note).is_none() {
            problems.push(ValidationProblem::DanglingLyricNote { note: lyric.note });
        }
        if let Some(note) = lyric.melisma_to {
            if score.note(note).is_none() {
                problems.push(ValidationProblem::DanglingMelismaEnd { note });
            }
        }
    }
    problems
}
