use crate::{pitch_label, InstrumentSpec, ScoreSpecification};
use makepad_score::model::{
    Alter, Duration, EventId, EventKind, Measure, Meter, NoteId, Part, Pitch, Rational, Score,
    ScoreTime, SpannerEndpoint, SpannerId, SpannerKind, StaffId, Step, TimedEvent, Voice,
    VoiceId,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MusicalProblem {
    UnexpectedBarCount {
        expected: u32,
        actual: usize,
    },
    MissingInstrumentPart {
        part: String,
    },
    UnexpectedInstrumentPart {
        part: String,
    },
    DeclaredMeterMismatch {
        bar: u32,
        expected: String,
        actual: String,
    },
    MeasureExtentMismatch {
        bar: u32,
        expected: Rational,
        actual: Rational,
    },
    BarDurationMismatch {
        part: String,
        bar: u32,
        voice: u16,
        expected: Rational,
        actual: Rational,
    },
    EventOutsideBar {
        part: String,
        bar: u32,
        voice: u16,
        event: EventId,
    },
    VoiceCollision {
        part: String,
        bar: u32,
        voice: u16,
        first: EventId,
        second: EventId,
    },
    DuplicateVoiceNumber {
        part: String,
        staff: StaffId,
        number: u16,
    },
    TieTargetMissing {
        note: NoteId,
        target: NoteId,
    },
    TieNotReciprocal {
        note: NoteId,
        target: NoteId,
    },
    TiePitchMismatch {
        note: NoteId,
        target: NoteId,
    },
    TieTargetNotFollowing {
        note: NoteId,
        target: NoteId,
    },
    SlurEndpointMissing {
        spanner: SpannerId,
        endpoint: &'static str,
    },
    SlurEndpointOrder {
        spanner: SpannerId,
    },
    PitchOutOfRange {
        part: String,
        bar: u32,
        pitch: Pitch,
        low: Pitch,
        high: Pitch,
    },
    InvalidKeySignature {
        bar: Option<u32>,
        detail: String,
    },
    DeclaredKeyMismatch {
        bar: u32,
        expected_fifths: i8,
        actual: String,
    },
    ImplausibleAccidental {
        part: String,
        bar: u32,
        pitch: Pitch,
    },
    KeyboardHandSpan {
        part: String,
        bar: u32,
        staff: StaffId,
        at: ScoreTime,
        span: Rational,
        limit: u8,
    },
    Arithmetic {
        detail: String,
    },
}

impl fmt::Display for MusicalProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedBarCount { expected, actual } => {
                write!(f, "score has {actual} bars; exactly {expected} required")
            }
            Self::MissingInstrumentPart { part } => {
                write!(f, "required instrument part \"{part}\" is missing")
            }
            Self::UnexpectedInstrumentPart { part } => {
                write!(f, "unexpected instrument part \"{part}\"")
            }
            Self::DeclaredMeterMismatch {
                bar,
                expected,
                actual,
            } => write!(f, "bar {bar} declares {actual}; {expected} required"),
            Self::MeasureExtentMismatch {
                bar,
                expected,
                actual,
            } => write!(
                f,
                "bar {bar} extent is {actual}; meter requires {expected}"
            ),
            Self::BarDurationMismatch {
                part,
                bar,
                voice,
                expected,
                actual,
            } => write!(
                f,
                "bar {bar} part \"{part}\" voice {voice} sums to {actual} under meter duration {expected}"
            ),
            Self::EventOutsideBar {
                part,
                bar,
                voice,
                event,
            } => write!(
                f,
                "bar {bar} part \"{part}\" voice {voice} event {event:?} crosses the barline"
            ),
            Self::VoiceCollision {
                part,
                bar,
                voice,
                first,
                second,
            } => write!(
                f,
                "bar {bar} part \"{part}\" voice {voice} has colliding events {first:?} and {second:?}"
            ),
            Self::DuplicateVoiceNumber {
                part,
                staff,
                number,
            } => write!(
                f,
                "part \"{part}\" staff {staff:?} has duplicate voice number {number}"
            ),
            Self::TieTargetMissing { note, target } => {
                write!(f, "note {note:?} ties to missing note {target:?}")
            }
            Self::TieNotReciprocal { note, target } => write!(
                f,
                "tie between note {note:?} and {target:?} is not reciprocal"
            ),
            Self::TiePitchMismatch { note, target } => write!(
                f,
                "tie from note {note:?} resolves to a different pitch at {target:?}"
            ),
            Self::TieTargetNotFollowing { note, target } => write!(
                f,
                "tie from note {note:?} does not resolve to a following note {target:?} in the same voice"
            ),
            Self::SlurEndpointMissing { spanner, endpoint } => {
                write!(f, "slur {spanner:?} has a missing {endpoint} endpoint")
            }
            Self::SlurEndpointOrder { spanner } => {
                write!(f, "slur {spanner:?} does not end after it starts")
            }
            Self::PitchOutOfRange {
                part,
                bar,
                pitch,
                low,
                high,
            } => write!(
                f,
                "bar {bar} part \"{part}\" pitch {} is outside written range {}..{}",
                pitch_label(*pitch),
                pitch_label(*low),
                pitch_label(*high)
            ),
            Self::InvalidKeySignature { bar, detail } => match bar {
                Some(bar) => write!(f, "bar {bar} has an invalid key signature: {detail}"),
                None => write!(f, "invalid key signature: {detail}"),
            },
            Self::DeclaredKeyMismatch {
                bar,
                expected_fifths,
                actual,
            } => write!(
                f,
                "bar {bar} key is {actual}; {expected_fifths} fifths required"
            ),
            Self::ImplausibleAccidental { part, bar, pitch } => write!(
                f,
                "bar {bar} part \"{part}\" uses implausible accidental spelling {}",
                pitch_label(*pitch)
            ),
            Self::KeyboardHandSpan {
                part,
                bar,
                staff,
                at,
                span,
                limit,
            } => write!(
                f,
                "bar {bar} part \"{part}\" staff {staff:?} at {} spans {span} semitones; limit is {limit}",
                at.0
            ),
            Self::Arithmetic { detail } => write!(f, "score arithmetic failed: {detail}"),
        }
    }
}

/// Validates generated notation against both semantic invariants and the
/// exact generation contract.
pub fn validate_score(score: &Score, specification: &ScoreSpecification) -> Vec<MusicalProblem> {
    let mut problems = Vec::new();
    let measures = sorted_measures(score);
    if measures.len() != specification.bar_count as usize {
        problems.push(MusicalProblem::UnexpectedBarCount {
            expected: specification.bar_count,
            actual: measures.len(),
        });
    }
    validate_instrumentation(score, specification, &measures, &mut problems);
    validate_meter_and_keys(score, specification, &measures, &mut problems);
    validate_voices(score, specification, &measures, &mut problems);
    validate_ties(score, &mut problems);
    validate_slurs(score, &mut problems);
    validate_keyboard_spans(score, specification, &measures, &mut problems);
    problems
}

fn sorted_measures(score: &Score) -> Vec<&Measure> {
    let mut measures: Vec<_> = score.measures.values().collect();
    measures.sort_by_key(|measure| (measure.start, measure.ordinal, measure.id));
    measures
}

fn validate_instrumentation(
    score: &Score,
    specification: &ScoreSpecification,
    measures: &[&Measure],
    problems: &mut Vec<MusicalProblem>,
) {
    for instrument in &specification.instruments {
        if find_part(score, &instrument.part_name).is_none() {
            problems.push(MusicalProblem::MissingInstrumentPart {
                part: instrument.part_name.clone(),
            });
        }
    }
    for part in score.parts.values() {
        let Some(instrument) = find_instrument(specification, &part.name) else {
            problems.push(MusicalProblem::UnexpectedInstrumentPart {
                part: part.name.clone(),
            });
            continue;
        };
        let Ok(low) = chromatic_pitch(instrument.written_low) else {
            problems.push(MusicalProblem::Arithmetic {
                detail: format!("invalid low range for {}", instrument.part_name),
            });
            continue;
        };
        let Ok(high) = chromatic_pitch(instrument.written_high) else {
            problems.push(MusicalProblem::Arithmetic {
                detail: format!("invalid high range for {}", instrument.part_name),
            });
            continue;
        };
        for voice in part_voices(score, part) {
            for event in &voice.events {
                let Some(measure) = measure_at(measures, event.onset) else {
                    continue;
                };
                for note in event.chord_notes() {
                    let Some(pitch) = note.written_pitch else {
                        continue;
                    };
                    match chromatic_pitch(pitch) {
                        Ok(value) if value < low || value > high => {
                            problems.push(MusicalProblem::PitchOutOfRange {
                                part: part.name.clone(),
                                bar: measure.ordinal,
                                pitch,
                                low: instrument.written_low,
                                high: instrument.written_high,
                            });
                        }
                        Err(_) => problems.push(MusicalProblem::Arithmetic {
                            detail: format!("invalid pitch {}", pitch_label(pitch)),
                        }),
                        _ => {}
                    }
                    if !sane_alter(pitch.alter) {
                        problems.push(MusicalProblem::ImplausibleAccidental {
                            part: part.name.clone(),
                            bar: measure.ordinal,
                            pitch,
                        });
                    }
                }
            }
        }
    }
}

fn validate_meter_and_keys(
    score: &Score,
    specification: &ScoreSpecification,
    measures: &[&Measure],
    problems: &mut Vec<MusicalProblem>,
) {
    let expected_meter = Meter::Measured {
        groups: specification.meter.groups.clone(),
        unit: specification.meter.unit,
    };
    let expected_duration = match expected_meter.duration() {
        Ok(Some(duration)) => duration,
        _ => {
            problems.push(MusicalProblem::Arithmetic {
                detail: "requested meter is invalid".to_string(),
            });
            return;
        }
    };
    for measure in measures {
        let actual_meter = score.maps.meter_at(measure.start, None, None);
        if actual_meter != Some(&expected_meter) {
            problems.push(MusicalProblem::DeclaredMeterMismatch {
                bar: measure.ordinal,
                expected: specification.meter.label(),
                actual: actual_meter
                    .map(meter_label)
                    .unwrap_or_else(|| "no meter".to_string()),
            });
        }
        if measure.extent != expected_duration {
            problems.push(MusicalProblem::MeasureExtentMismatch {
                bar: measure.ordinal,
                expected: expected_duration.0,
                actual: measure.extent.0,
            });
        }
        match score.maps.key_at(measure.start, None, None) {
            Some(key) => {
                if key.fifths != specification.key.fifths {
                    problems.push(MusicalProblem::DeclaredKeyMismatch {
                        bar: measure.ordinal,
                        expected_fifths: specification.key.fifths,
                        actual: format!("{} fifths", key.fifths),
                    });
                }
                if !(-7..=7).contains(&key.fifths) {
                    problems.push(MusicalProblem::InvalidKeySignature {
                        bar: Some(measure.ordinal),
                        detail: format!("{} fifths is outside -7..7", key.fifths),
                    });
                }
                let mut steps = BTreeSet::new();
                for (step, alter) in &key.custom {
                    if !steps.insert(*step) {
                        problems.push(MusicalProblem::InvalidKeySignature {
                            bar: Some(measure.ordinal),
                            detail: format!("duplicate custom step {step:?}"),
                        });
                    }
                    if !sane_alter(*alter) {
                        problems.push(MusicalProblem::InvalidKeySignature {
                            bar: Some(measure.ordinal),
                            detail: format!("implausible custom alteration on {step:?}"),
                        });
                    }
                }
            }
            None => problems.push(MusicalProblem::DeclaredKeyMismatch {
                bar: measure.ordinal,
                expected_fifths: specification.key.fifths,
                actual: "no key signature".to_string(),
            }),
        }
    }
}

fn validate_voices(
    score: &Score,
    specification: &ScoreSpecification,
    measures: &[&Measure],
    problems: &mut Vec<MusicalProblem>,
) {
    let expected = match (Meter::Measured {
        groups: specification.meter.groups.clone(),
        unit: specification.meter.unit,
    })
    .duration()
    {
        Ok(Some(value)) => value,
        _ => return,
    };
    for part in score.parts.values() {
        for staff_id in &part.staves {
            let Some(staff) = score.staves.get(staff_id) else {
                continue;
            };
            let mut numbers = BTreeSet::new();
            for voice_id in &staff.voices {
                let Some(voice) = score.voices.get(voice_id) else {
                    continue;
                };
                if !numbers.insert(voice.number) {
                    problems.push(MusicalProblem::DuplicateVoiceNumber {
                        part: part.name.clone(),
                        staff: staff.id,
                        number: voice.number,
                    });
                }
                validate_voice_measures(part, voice, measures, expected, problems);
            }
        }
    }
}

fn validate_voice_measures(
    part: &Part,
    voice: &Voice,
    measures: &[&Measure],
    expected: Duration,
    problems: &mut Vec<MusicalProblem>,
) {
    for measure in measures {
        let Ok(measure_end) = measure.start.checked_add(measure.extent) else {
            problems.push(MusicalProblem::Arithmetic {
                detail: format!("bar {} end overflow", measure.ordinal),
            });
            continue;
        };
        let mut events: Vec<_> = voice
            .events
            .iter()
            .filter(|event| {
                event.grace.is_none()
                    && event.duration.is_some()
                    && matches!(event.kind, EventKind::Chord(_) | EventKind::Rest)
                    && event.onset >= measure.start
                    && event.onset < measure_end
            })
            .collect();
        events.sort_by_key(|event| (event.onset, event.id));
        let mut sum = Rational::ZERO;
        let mut previous: Option<(&TimedEvent, ScoreTime)> = None;
        for event in events {
            let duration = event.duration.expect("filtered duration");
            match sum.checked_add(duration.0) {
                Ok(value) => sum = value,
                Err(_) => problems.push(MusicalProblem::Arithmetic {
                    detail: format!("bar {} duration sum overflow", measure.ordinal),
                }),
            }
            match event.end() {
                Ok(end) => {
                    if end > measure_end {
                        problems.push(MusicalProblem::EventOutsideBar {
                            part: part.name.clone(),
                            bar: measure.ordinal,
                            voice: voice.number,
                            event: event.id,
                        });
                    }
                    if let Some((prior, prior_end)) = previous {
                        if event.onset < prior_end {
                            problems.push(MusicalProblem::VoiceCollision {
                                part: part.name.clone(),
                                bar: measure.ordinal,
                                voice: voice.number,
                                first: prior.id,
                                second: event.id,
                            });
                        }
                    }
                    if previous.is_none_or(|(_, prior_end)| end > prior_end) {
                        previous = Some((event, end));
                    }
                }
                Err(_) => problems.push(MusicalProblem::Arithmetic {
                    detail: format!("bar {} event end overflow", measure.ordinal),
                }),
            }
        }
        if sum != expected.0 {
            problems.push(MusicalProblem::BarDurationMismatch {
                part: part.name.clone(),
                bar: measure.ordinal,
                voice: voice.number,
                expected: expected.0,
                actual: sum,
            });
        }
    }
}

#[derive(Clone, Copy)]
struct NoteLocation<'a> {
    note: &'a makepad_score::model::Note,
    voice: VoiceId,
    onset: ScoreTime,
    end: ScoreTime,
}

fn validate_ties(score: &Score, problems: &mut Vec<MusicalProblem>) {
    let mut notes = BTreeMap::new();
    for (voice_id, voice) in score.voices.iter() {
        for event in &voice.events {
            let Ok(end) = event.end() else {
                continue;
            };
            for note in event.chord_notes() {
                notes.insert(
                    note.id,
                    NoteLocation {
                        note,
                        voice: *voice_id,
                        onset: event.onset,
                        end,
                    },
                );
            }
        }
    }
    for location in notes.values() {
        let note = location.note;
        if let Some(target_id) = note.tie_to {
            let Some(target) = notes.get(&target_id) else {
                problems.push(MusicalProblem::TieTargetMissing {
                    note: note.id,
                    target: target_id,
                });
                continue;
            };
            if target.note.tie_from != Some(note.id) {
                problems.push(MusicalProblem::TieNotReciprocal {
                    note: note.id,
                    target: target_id,
                });
            }
            if note.written_pitch.is_none() || note.written_pitch != target.note.written_pitch {
                problems.push(MusicalProblem::TiePitchMismatch {
                    note: note.id,
                    target: target_id,
                });
            }
            if target.voice != location.voice
                || target.onset < location.end
                || target.onset <= location.onset
            {
                problems.push(MusicalProblem::TieTargetNotFollowing {
                    note: note.id,
                    target: target_id,
                });
            }
        }
        if let Some(source_id) = note.tie_from {
            match notes.get(&source_id) {
                Some(source) if source.note.tie_to == Some(note.id) => {}
                Some(_) => problems.push(MusicalProblem::TieNotReciprocal {
                    note: note.id,
                    target: source_id,
                }),
                None => problems.push(MusicalProblem::TieTargetMissing {
                    note: note.id,
                    target: source_id,
                }),
            }
        }
    }
}

fn validate_slurs(score: &Score, problems: &mut Vec<MusicalProblem>) {
    for spanner in score.spanners.values() {
        if !matches!(spanner.kind, SpannerKind::Slur { .. }) {
            continue;
        }
        let start = endpoint_time(score, spanner.start);
        let end = endpoint_time(score, spanner.end);
        if start.is_none() {
            problems.push(MusicalProblem::SlurEndpointMissing {
                spanner: spanner.id,
                endpoint: "start",
            });
        }
        if end.is_none() {
            problems.push(MusicalProblem::SlurEndpointMissing {
                spanner: spanner.id,
                endpoint: "end",
            });
        }
        if let (Some(start), Some(end)) = (start, end) {
            if end <= start {
                problems.push(MusicalProblem::SlurEndpointOrder {
                    spanner: spanner.id,
                });
            }
        }
    }
}

fn endpoint_time(score: &Score, endpoint: SpannerEndpoint) -> Option<ScoreTime> {
    match endpoint {
        SpannerEndpoint::Note(note) => score.note_context(note).map(|(_, event, _)| event.onset),
        SpannerEndpoint::Event(event) => score.event(event).map(|event| event.onset),
        SpannerEndpoint::StaffTime { staff, at } => {
            score.staves.contains_key(&staff).then_some(at)
        }
    }
}

#[derive(Clone)]
struct KeyboardEvent {
    onset: ScoreTime,
    end: ScoreTime,
    pitches: Vec<Rational>,
}

fn validate_keyboard_spans(
    score: &Score,
    specification: &ScoreSpecification,
    measures: &[&Measure],
    problems: &mut Vec<MusicalProblem>,
) {
    for part in score.parts.values() {
        let Some(instrument) = find_instrument(specification, &part.name) else {
            continue;
        };
        if !instrument.keyboard {
            continue;
        }
        for staff_id in &part.staves {
            let Some(staff) = score.staves.get(staff_id) else {
                continue;
            };
            let mut events = Vec::new();
            for voice_id in &staff.voices {
                let Some(voice) = score.voices.get(voice_id) else {
                    continue;
                };
                for event in &voice.events {
                    if event.grace.is_some() || event.duration.is_none() {
                        continue;
                    }
                    let pitches: Vec<_> = event
                        .chord_notes()
                        .iter()
                        .filter_map(|note| note.written_pitch)
                        .filter_map(|pitch| chromatic_pitch(pitch).ok())
                        .collect();
                    if pitches.len() < 2 && event.chord_notes().len() < 2 {
                        if pitches.is_empty() {
                            continue;
                        }
                    }
                    if let Ok(end) = event.end() {
                        events.push(KeyboardEvent {
                            onset: event.onset,
                            end,
                            pitches,
                        });
                    }
                }
            }
            let attacks: BTreeSet<_> = events.iter().map(|event| event.onset).collect();
            for attack in attacks {
                let sounding: Vec<_> = events
                    .iter()
                    .filter(|event| event.onset <= attack && event.end > attack)
                    .flat_map(|event| event.pitches.iter().copied())
                    .collect();
                let (Some(low), Some(high)) = (sounding.iter().min(), sounding.iter().max()) else {
                    continue;
                };
                let Ok(span) = high.checked_sub(*low) else {
                    continue;
                };
                let limit = Rational::new(i64::from(instrument.max_hand_span_semitones), 1)
                    .expect("u8 denominator is valid");
                if span > limit {
                    let bar = measure_at(measures, attack)
                        .map(|measure| measure.ordinal)
                        .unwrap_or_default();
                    problems.push(MusicalProblem::KeyboardHandSpan {
                        part: part.name.clone(),
                        bar,
                        staff: staff.id,
                        at: attack,
                        span,
                        limit: instrument.max_hand_span_semitones,
                    });
                }
            }
        }
    }
}

fn part_voices<'a>(score: &'a Score, part: &'a Part) -> Vec<&'a Voice> {
    part.staves
        .iter()
        .filter_map(|staff| score.staves.get(staff))
        .flat_map(|staff| staff.voices.iter())
        .filter_map(|voice| score.voices.get(voice))
        .collect()
}

fn find_part<'a>(score: &'a Score, name: &str) -> Option<&'a Part> {
    score
        .parts
        .values()
        .find(|part| part.name.eq_ignore_ascii_case(name))
}

fn find_instrument<'a>(
    specification: &'a ScoreSpecification,
    part_name: &str,
) -> Option<&'a InstrumentSpec> {
    specification
        .instruments
        .iter()
        .find(|instrument| instrument.part_name.eq_ignore_ascii_case(part_name))
}

fn measure_at<'a>(measures: &'a [&Measure], at: ScoreTime) -> Option<&'a Measure> {
    measures.iter().copied().find(|measure| {
        measure
            .start
            .checked_add(measure.extent)
            .is_ok_and(|end| at >= measure.start && at < end)
    })
}

fn meter_label(meter: &Meter) -> String {
    match meter {
        Meter::Free => "free meter".to_string(),
        Meter::Measured { groups, unit } => format!(
            "{}/{}",
            groups
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join("+"),
            unit
        ),
    }
}

fn sane_alter(alter: Alter) -> bool {
    alter.0.denominator() == 1 && alter.0.numerator().unsigned_abs() <= 2
}

fn chromatic_pitch(pitch: Pitch) -> Result<Rational, makepad_score::model::RationalError> {
    let natural = match pitch.step {
        Step::C => 0,
        Step::D => 2,
        Step::E => 4,
        Step::F => 5,
        Step::G => 7,
        Step::A => 9,
        Step::B => 11,
    };
    Rational::new(i64::from(pitch.octave) * 12 + natural, 1)?.checked_add(pitch.alter.0)
}
