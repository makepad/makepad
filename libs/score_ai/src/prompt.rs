use makepad_score::model::{EventKind, Measure, Meter, PartId, Pitch, Score, Step};
use std::collections::BTreeSet;
use std::fmt::Write;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MeterSpec {
    /// Additive groups, for example `[3, 2, 2]` for 7/8 grouped 3+2+2.
    pub groups: Vec<u16>,
    pub unit: u16,
}

impl MeterSpec {
    pub fn simple(beats: u16, unit: u16) -> Self {
        Self {
            groups: vec![beats],
            unit,
        }
    }

    pub fn label(&self) -> String {
        let groups = self
            .groups
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join("+");
        format!("{groups}/{}", self.unit)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeySpec {
    pub fifths: i8,
    pub mode: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstrumentSpec {
    /// Must match the imported semantic part name.
    pub part_name: String,
    pub instrument_name: String,
    pub written_low: Pitch,
    pub written_high: Pitch,
    pub keyboard: bool,
    /// Per-staff simultaneous span. Twelve semitones is a practical default.
    pub max_hand_span_semitones: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScoreSpecification {
    pub meter: MeterSpec,
    pub key: KeySpec,
    pub bar_count: u32,
    pub instruments: Vec<InstrumentSpec>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScoreSelection {
    /// Empty means every part.
    pub parts: Vec<PartId>,
    pub first_bar: u32,
    pub last_bar: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactScoreContext(pub String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextError {
    InvalidBarRange,
    EmptySelection,
    Arithmetic,
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBarRange => f.write_str("selection has an invalid bar range"),
            Self::EmptySelection => f.write_str("selection contains no score events"),
            Self::Arithmetic => f.write_str("score time arithmetic overflowed"),
        }
    }
}

impl std::error::Error for ContextError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScoreOperation {
    Generate {
        description: String,
    },
    Continue {
        description: String,
        context: CompactScoreContext,
    },
    HarmoniseMelody {
        description: String,
        melody: CompactScoreContext,
    },
    GenerateSecondPart {
        description: String,
        existing_part: CompactScoreContext,
    },
    RevoiceSelection {
        description: String,
        selection: CompactScoreContext,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationRequest {
    pub operation: ScoreOperation,
    pub specification: ScoreSpecification,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelPrompt {
    pub system: String,
    pub user: String,
}

impl ModelPrompt {
    pub fn provenance_text(&self) -> String {
        format!("SYSTEM\n{}\n\nUSER\n{}", self.system, self.user)
    }
}

/// Serializes a selection into a compact, deterministic notation shorthand.
/// The shorthand is input context only; model output is always MusicXML.
pub fn serialize_selection(
    score: &Score,
    selection: &ScoreSelection,
) -> Result<CompactScoreContext, ContextError> {
    if selection.first_bar > selection.last_bar {
        return Err(ContextError::InvalidBarRange);
    }
    let selected_parts: BTreeSet<_> = selection.parts.iter().copied().collect();
    let mut measures: Vec<_> = score
        .measures
        .values()
        .filter(|measure| {
            measure.ordinal >= selection.first_bar && measure.ordinal <= selection.last_bar
        })
        .collect();
    measures.sort_by_key(|measure| (measure.ordinal, measure.start, measure.id));
    if measures.is_empty() {
        return Err(ContextError::EmptySelection);
    }

    let mut output = String::new();
    writeln!(&mut output, "title={}", compact_text(&score.title)).expect("writing to String");
    let mut event_count = 0usize;
    for measure in measures {
        write_measure_header(&mut output, score, measure)?;
        let measure_end = measure
            .start
            .checked_add(measure.extent)
            .map_err(|_| ContextError::Arithmetic)?;
        for part in score.parts.values() {
            if !selected_parts.is_empty() && !selected_parts.contains(&part.id) {
                continue;
            }
            for staff_id in &part.staves {
                let Some(staff) = score.staves.get(staff_id) else {
                    continue;
                };
                for voice_id in &staff.voices {
                    let Some(voice) = score.voices.get(voice_id) else {
                        continue;
                    };
                    let events: Vec<_> = voice
                        .events
                        .iter()
                        .filter(|event| {
                            event.onset >= measure.start && event.onset < measure_end
                        })
                        .collect();
                    if events.is_empty() {
                        continue;
                    }
                    event_count += events.len();
                    write!(
                        &mut output,
                        " p={} s={} v{}:",
                        compact_text(&part.name),
                        staff.id.counter(),
                        voice.number
                    )
                    .expect("writing to String");
                    for event in events {
                        let relative = event
                            .onset
                            .checked_sub(measure.start)
                            .map_err(|_| ContextError::Arithmetic)?;
                        write!(&mut output, " @{}", relative.0).expect("writing to String");
                        if let Some(duration) = event.duration {
                            write!(&mut output, "+{}", duration.0).expect("writing to String");
                        }
                        output.push(':');
                        match &event.kind {
                            EventKind::Chord(notes) => {
                                for (index, note) in notes.iter().enumerate() {
                                    if index != 0 {
                                        output.push(',');
                                    }
                                    match note.written_pitch {
                                        Some(pitch) => output.push_str(&pitch_label(pitch)),
                                        None => output.push('u'),
                                    }
                                }
                            }
                            EventKind::Rest => output.push('r'),
                            _ => output.push('x'),
                        }
                        output.push(';');
                    }
                    output.push('\n');
                }
            }
        }
    }
    if event_count == 0 {
        return Err(ContextError::EmptySelection);
    }
    Ok(CompactScoreContext(output))
}

fn write_measure_header(
    output: &mut String,
    score: &Score,
    measure: &Measure,
) -> Result<(), ContextError> {
    write!(output, "m{}", measure.ordinal).expect("writing to String");
    if let Some(meter) = score.maps.meter_at(measure.start, None, None) {
        write!(output, " meter={}", meter_label(meter)).expect("writing to String");
    }
    if let Some(key) = score.maps.key_at(measure.start, None, None) {
        write!(output, " key={}f", key.fifths).expect("writing to String");
    }
    output.push('\n');
    Ok(())
}

fn meter_label(meter: &Meter) -> String {
    match meter {
        Meter::Free => "free".to_string(),
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

fn compact_text(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '\n' | '\r' | '\t' | '|' | ';' => ' ',
            other => other,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn pitch_label(pitch: Pitch) -> String {
    let step = match pitch.step {
        Step::C => 'C',
        Step::D => 'D',
        Step::E => 'E',
        Step::F => 'F',
        Step::G => 'G',
        Step::A => 'A',
        Step::B => 'B',
    };
    let alter = match (
        pitch.alter.0.numerator(),
        pitch.alter.0.denominator(),
    ) {
        (0, _) => String::new(),
        (1, 1) => "#".to_string(),
        (-1, 1) => "b".to_string(),
        (value, denominator) => format!("[{value}/{denominator}]"),
    };
    format!("{step}{alter}{}", pitch.octave)
}

pub fn build_initial_prompt(request: &GenerationRequest) -> ModelPrompt {
    let specification = &request.specification;
    let instrumentation = specification
        .instruments
        .iter()
        .map(|instrument| {
            format!(
                "{} as part \"{}\" (written range {}..{})",
                instrument.instrument_name,
                instrument.part_name,
                pitch_label(instrument.written_low),
                pitch_label(instrument.written_high)
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let system = format!(
        "You are a deterministic music-notation engine. Return exactly one complete MusicXML 4.0 score-partwise document in one ```musicxml fenced block and no prose. The score must have exactly {bars} measures in {meter}, key signature {fifths} fifths ({mode}), and exactly this instrumentation: {instrumentation}. Every non-grace voice in every measure must fill exactly the declared meter using explicit rests where needed. Use reciprocal same-pitch ties with real following notes, paired slur endpoints, valid voice numbers, conventional accidentals, playable written ranges, and human-playable keyboard hand spans. Include divisions, part-list, attributes, pitches, durations, voices, and staves explicitly. Do not abbreviate, omit repeated measures, use measure-repeat shorthand, or truncate the XML.",
        bars = specification.bar_count,
        meter = specification.meter.label(),
        fifths = specification.key.fifths,
        mode = specification.key.mode,
    );
    let (operation, description, context) = match &request.operation {
        ScoreOperation::Generate { description } => {
            ("Generate a new score", description.as_str(), None)
        }
        ScoreOperation::Continue {
            description,
            context,
        } => ("Continue the supplied passage", description.as_str(), Some(&context.0)),
        ScoreOperation::HarmoniseMelody {
            description,
            melody,
        } => ("Harmonise the supplied melody", description.as_str(), Some(&melody.0)),
        ScoreOperation::GenerateSecondPart {
            description,
            existing_part,
        } => (
            "Generate a second part against the supplied part",
            description.as_str(),
            Some(&existing_part.0),
        ),
        ScoreOperation::RevoiceSelection {
            description,
            selection,
        } => (
            "Re-voice or arrange the supplied selection",
            description.as_str(),
            Some(&selection.0),
        ),
    };
    let mut user = format!("TASK: {operation}.\nBRIEF: {description}");
    if let Some(context) = context {
        user.push_str("\nINPUT SCORE CONTEXT (compact exact notation):\n");
        user.push_str(context);
    }
    ModelPrompt { system, user }
}

pub fn build_repair_prompt(
    initial: &ModelPrompt,
    failures: &[String],
    previous_musicxml: Option<&str>,
) -> ModelPrompt {
    let mut user = String::from(
        "Repair the prior score. Return a complete replacement MusicXML document, not a patch. Correct every issue below without changing requirements that already pass:\n",
    );
    for failure in failures {
        writeln!(&mut user, "- {failure}").expect("writing to String");
    }
    if let Some(xml) = previous_musicxml {
        user.push_str("\nPRIOR MUSICXML TO REPAIR:\n");
        user.push_str(xml);
    }
    ModelPrompt {
        system: initial.system.clone(),
        user,
    }
}
