use crate::diagnostic::{ImportError, ImportReport, SourceLocation};
use crate::ids::{score_id, stable_id};
use makepad_musicxml::{
    MeasureItemRef, MusicXmlDocument, NoteKind, Score as XmlScore, XmlElement,
};
use makepad_score::model::*;
use makepad_score::symbol::{Articulation, Clef, DynamicMark, Ornament, Placement};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MusicXmlImportOptions {
    /// If set, constructs that cannot be represented without loss are errors.
    /// The default records every such decision in the report and continues.
    pub reject_loss: bool,
}

impl Default for MusicXmlImportOptions {
    fn default() -> Self {
        Self { reject_loss: false }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicXmlImportResult {
    pub score: makepad_score::model::Score,
    pub report: ImportReport,
}

pub fn import_musicxml_str(source: &str) -> Result<MusicXmlImportResult, ImportError> {
    import_musicxml_str_with_options(source, MusicXmlImportOptions::default())
}

/// Imports either plain MusicXML bytes or a compressed `.mxl` container.
pub fn import_musicxml_bytes(bytes: &[u8]) -> Result<MusicXmlImportResult, ImportError> {
    let document = if bytes.starts_with(b"PK\x03\x04") {
        makepad_musicxml::parse_mxl(bytes)?
    } else {
        makepad_musicxml::parse_musicxml_bytes(bytes)?
    };
    import_musicxml(&document)
}

pub fn import_musicxml_str_with_options(
    source: &str,
    options: MusicXmlImportOptions,
) -> Result<MusicXmlImportResult, ImportError> {
    let document = makepad_musicxml::parse_musicxml(source)?;
    import_musicxml_with_options(&document, options)
}

pub fn import_musicxml(document: &MusicXmlDocument) -> Result<MusicXmlImportResult, ImportError> {
    import_musicxml_with_options(document, MusicXmlImportOptions::default())
}

pub fn import_musicxml_with_options(
    document: &MusicXmlDocument,
    options: MusicXmlImportOptions,
) -> Result<MusicXmlImportResult, ImportError> {
    let partwise = document.to_partwise()?;
    let XmlScore::Partwise(xml_score) = &partwise.score else {
        unreachable!("to_partwise returned a non-partwise score")
    };
    let mut report = ImportReport::default();
    for element in document.unmodelled_elements() {
        report.ignored(
            "musicxml.unknown-element",
            format!("extension element <{}> was retained by the document tier but has no score-model representation", element.name),
            SourceLocation::MusicXml {
                part: None,
                measure: None,
                element: element.name.clone(),
                occurrence: 0,
            },
        );
    }

    let title = xml_score
        .element
        .first_child("movement-title")
        .map(XmlElement::direct_text)
        .or_else(|| {
            xml_score
                .element
                .first_child("work")
                .and_then(|work| work.first_child("work-title"))
                .map(XmlElement::direct_text)
        })
        .unwrap_or_default();
    let identity = score_identity(&xml_score.element, &title);
    let mut score = makepad_score::model::Score::new(score_id(&identity));
    score.title = title;

    let part_names = part_names(&xml_score.element);
    let parts: Vec<_> = xml_score.parts().collect();
    if parts.is_empty() {
        return Err(ImportError::InvalidSource(
            "MusicXML score has no parts".to_string(),
        ));
    }

    let measure_extents = measure_extents(&parts, &mut report)?;
    let mut measure_starts = Vec::with_capacity(measure_extents.len());
    let mut at = ScoreTime::ZERO;
    for (index, extent) in measure_extents.iter().copied().enumerate() {
        measure_starts.push(at);
        let source_measure = parts
            .first()
            .and_then(|part| part.measures().nth(index));
        let label = source_measure
            .and_then(|measure| measure.number())
            .map(str::to_string)
            .unwrap_or_else(|| (index + 1).to_string());
        let path = format!("measure/{index}");
        let id = stable_id::<MeasureTag>(
            "measure",
            source_measure.and_then(|measure| measure.id()),
            &path,
        );
        score.measures.insert(
            id,
            Measure {
                id,
                ordinal: u32::try_from(index).map_err(|_| {
                    ImportError::InvalidSource("too many MusicXML measures".to_string())
                })?,
                label,
                start: at,
                extent,
            },
        );
        score.flow.nodes.push(FlowNode {
            measure: id,
            ordinal: index as u32,
        });
        report.imported("measures");
        at = at.checked_add(extent)?;
    }

    let mut part_contexts = Vec::new();
    for (part_index, xml_part) in parts.iter().enumerate() {
        let xml_id = xml_part.id().unwrap_or("");
        let path = format!("part/{part_index}");
        let part_id = stable_id::<PartTag>("part", xml_part.id(), &path);
        let staff_count = detect_staff_count(xml_part.element()).max(1);
        let transposition = detect_transposition(xml_part.element(), &mut report, xml_id)?;
        let mut staff_ids = Vec::new();
        for staff_number in 1..=staff_count {
            let staff_path = format!("{path}/staff/{staff_number}");
            let staff_id = stable_id::<StaffTag>("staff", None, &staff_path);
            let kind = detect_staff_kind(xml_part.element(), staff_number);
            score.staves.insert(
                staff_id,
                Staff {
                    id: staff_id,
                    part: part_id,
                    parent: None,
                    kind,
                    voices: Vec::new(),
                },
            );
            staff_ids.push(staff_id);
            report.imported("staves");
        }
        score.parts.insert(
            part_id,
            Part {
                id: part_id,
                name: part_names
                    .get(xml_id)
                    .cloned()
                    .unwrap_or_else(|| xml_id.to_string()),
                staves: staff_ids.clone(),
                transposition,
            },
        );
        part_contexts.push(PartContext {
            index: part_index,
            xml_id: xml_id.to_string(),
            id: part_id,
            staves: staff_ids,
            divisions: 1,
        });
        report.imported("parts");
    }

    let mut state = ConvertState::default();
    for (xml_part, context) in parts.iter().zip(part_contexts.iter_mut()) {
        convert_part(
            &mut score,
            &mut report,
            &mut state,
            xml_part.element(),
            context,
            &measure_starts,
            &measure_extents,
        )?;
    }
    finish_relations(&mut score, &mut report, state)?;
    score.maps.sort();

    if options.reject_loss && !report.diagnostics.is_empty() {
        return Err(ImportError::Unsupported(format!(
            "conversion produced {} loss diagnostics",
            report.diagnostics.len()
        )));
    }
    Ok(MusicXmlImportResult { score, report })
}

#[derive(Clone)]
struct PartContext {
    index: usize,
    xml_id: String,
    id: PartId,
    staves: Vec<StaffId>,
    divisions: u32,
}

#[derive(Default)]
struct ConvertState {
    voices: BTreeMap<(usize, u32, String), VoiceId>,
    voice_numbers: BTreeMap<(usize, u32), u16>,
    ties: BTreeMap<(VoiceId, Pitch), NoteId>,
    tie_edges: Vec<(NoteId, NoteId)>,
    open_spanners: BTreeMap<String, PendingSpanner>,
    implicit_tuplets: BTreeMap<VoiceId, ImplicitTuplet>,
    open_melismas: BTreeMap<(VoiceId, u16), usize>,
    last_lyric_note: BTreeMap<(VoiceId, u16), NoteId>,
    repeat_start: u32,
    last_repeat_start: u32,
    open_voltas: BTreeMap<String, (u32, u32, Vec<u16>)>,
    closed_voltas: Vec<VoltaEnding>,
}

struct PendingSpanner {
    id: SpannerId,
    kind: SpannerKind,
    start: SpannerEndpoint,
    source: SourceLocation,
}

#[derive(Clone, Copy)]
struct ImplicitTuplet {
    actual: u16,
    normal: u16,
    group: SpannerId,
    remaining: u16,
}

fn convert_part(
    score: &mut makepad_score::model::Score,
    report: &mut ImportReport,
    state: &mut ConvertState,
    part: &XmlElement,
    context: &mut PartContext,
    measure_starts: &[ScoreTime],
    measure_extents: &[Duration],
) -> Result<(), ImportError> {
    for (measure_index, measure) in part.children_named("measure").enumerate() {
        let Some(&measure_start) = measure_starts.get(measure_index) else {
            break;
        };
        let measure_number = measure
            .attr("number")
            .map(str::to_string)
            .unwrap_or_else(|| (measure_index + 1).to_string());
        let mut cursor = ScoreTime::ZERO;
        let mut previous_onset = ScoreTime::ZERO;
        let mut item_occurrence = BTreeMap::<String, usize>::new();
        for item in measure.child_elements() {
            let occurrence = item_occurrence.entry(item.name.clone()).or_default();
            let source = xml_location(
                &context.xml_id,
                &measure_number,
                &item.name,
                *occurrence,
            );
            *occurrence += 1;
            match MeasureItemRef::from_element_for_import(item) {
                MeasureItemRef::Attributes(attributes) => {
                    if let Some(divisions) = attributes.divisions() {
                        if divisions == 0 {
                            return Err(ImportError::InvalidSource(format!(
                                "zero divisions in part {} measure {}",
                                context.xml_id, measure_number
                            )));
                        }
                        context.divisions = divisions;
                    }
                    convert_attributes(
                        score,
                        report,
                        state,
                        context,
                        attributes.element(),
                        measure_start.checked_add_time(cursor)?,
                        &source,
                    )?;
                }
                MeasureItemRef::Note(note_ref) => {
                    let absolute = measure_start.checked_add_time(if note_ref.is_chord() {
                        previous_onset
                    } else {
                        cursor
                    })?;
                    let duration = note_ref
                        .duration()
                        .map(|value| duration_from_divisions(value, context.divisions))
                        .transpose()?;
                    let (event_id, note_id) = convert_note(
                        score,
                        report,
                        state,
                        context,
                        note_ref.element(),
                        measure_index,
                        absolute,
                        duration,
                        &source,
                    )?;
                    if !note_ref.is_chord() {
                        previous_onset = cursor;
                        if !note_ref.is_grace() {
                            if let Some(duration) = duration {
                                cursor = cursor.checked_add(duration)?;
                            }
                        }
                    }
                    let _ = (event_id, note_id);
                }
                MeasureItemRef::Backup(element) => {
                    let value = child_u32(element, "duration").ok_or_else(|| {
                        ImportError::InvalidSource("backup without duration".to_string())
                    })?;
                    let amount = score_time_from_divisions(value, context.divisions)?;
                    cursor = cursor.checked_sub(amount)?;
                    if cursor.0.numerator() < 0 {
                        report.repaired(
                            "musicxml.backup-before-measure",
                            "backup moved before the measure start; cursor was clamped",
                            source,
                        );
                        cursor = ScoreTime::ZERO;
                    }
                    report.imported("backup");
                }
                MeasureItemRef::Forward(element) => {
                    let value = child_u32(element, "duration").ok_or_else(|| {
                        ImportError::InvalidSource("forward without duration".to_string())
                    })?;
                    cursor = cursor.checked_add_time(score_time_from_divisions(
                        value,
                        context.divisions,
                    )?)?;
                    report.imported("forward");
                }
                MeasureItemRef::Direction(direction) => convert_direction(
                    score,
                    report,
                    state,
                    context,
                    direction.element(),
                    measure_start,
                    cursor,
                    measure_index as u32,
                    &source,
                )?,
                MeasureItemRef::Harmony(harmony) => convert_harmony(
                    score,
                    report,
                    state,
                    context,
                    harmony.element(),
                    measure_start.checked_add_time(cursor)?,
                    measure_index,
                    &source,
                )?,
                MeasureItemRef::FiguredBass(figured) => convert_figured_bass(
                    score,
                    report,
                    state,
                    context,
                    figured.element(),
                    measure_start.checked_add_time(cursor)?,
                    measure_index,
                    &source,
                )?,
                MeasureItemRef::Sound(sound) => convert_sound(
                    score,
                    report,
                    sound.element(),
                    measure_start.checked_add_time(cursor)?,
                    measure_index as u32,
                    &source,
                )?,
                MeasureItemRef::Barline(barline) => convert_barline(
                    score,
                    report,
                    state,
                    context,
                    barline.element(),
                    measure_start,
                    measure_extents[measure_index],
                    measure_index as u32,
                    &source,
                )?,
                MeasureItemRef::Print(_) => {
                    report.ignored(
                        "musicxml.layout",
                        "print/layout directives are outside the semantic score model",
                        source,
                    );
                }
                MeasureItemRef::Grouping(_) | MeasureItemRef::Link(_) | MeasureItemRef::Bookmark(_) => {
                    report.ignored(
                        "musicxml.non-musical-anchor",
                        format!("<{}> has no semantic score-model representation", item.name),
                        source,
                    );
                }
                MeasureItemRef::Unknown(_) => report.ignored(
                    "musicxml.measure-item",
                    format!("measure child <{}> was not mapped", item.name),
                    source,
                ),
            }
        }
    }
    Ok(())
}

trait MeasureItemImport<'a> {
    fn from_element_for_import(element: &'a XmlElement) -> MeasureItemRef<'a>;
}

impl<'a> MeasureItemImport<'a> for MeasureItemRef<'a> {
    fn from_element_for_import(element: &'a XmlElement) -> MeasureItemRef<'a> {
        match element.name.as_str() {
            "attributes" => MeasureItemRef::Attributes(makepad_musicxml::AttributesRef(element)),
            "note" => MeasureItemRef::Note(makepad_musicxml::NoteRef(element)),
            "backup" => MeasureItemRef::Backup(element),
            "forward" => MeasureItemRef::Forward(element),
            "direction" => MeasureItemRef::Direction(makepad_musicxml::DirectionRef(element)),
            "harmony" => MeasureItemRef::Harmony(makepad_musicxml::HarmonyRef(element)),
            "figured-bass" => MeasureItemRef::FiguredBass(makepad_musicxml::FiguredBassRef(element)),
            "print" => MeasureItemRef::Print(makepad_musicxml::PrintRef(element)),
            "sound" => MeasureItemRef::Sound(makepad_musicxml::SoundRef(element)),
            "barline" => MeasureItemRef::Barline(makepad_musicxml::BarlineRef(element)),
            "grouping" => MeasureItemRef::Grouping(element),
            "link" => MeasureItemRef::Link(element),
            "bookmark" => MeasureItemRef::Bookmark(element),
            _ => MeasureItemRef::Unknown(element),
        }
    }
}

fn convert_attributes(
    score: &mut makepad_score::model::Score,
    report: &mut ImportReport,
    state: &mut ConvertState,
    context: &PartContext,
    attributes: &XmlElement,
    at: ScoreTime,
    source: &SourceLocation,
) -> Result<(), ImportError> {
    for details in attributes.children_named("staff-details") {
        convert_staff_details(score, report, context, details, source)?;
    }
    for key in attributes.children_named("key") {
        let staff_number = attr_u32(key, "number");
        let custom = parse_custom_key(key)?;
        let fifths = child_i16(key, "fifths").unwrap_or(0);
        let value = KeySignature {
            fifths: i8::try_from(fifths).unwrap_or_else(|_| if fifths < 0 { -7 } else { 7 }),
            custom,
        };
        let scope = map_scope(context, staff_number);
        score.maps.key.push(Change {
            at,
            scope,
            value: value.clone(),
        });
        let staff = staff_for(context, staff_number.unwrap_or(1));
        let voice = ensure_voice(score, state, context, staff_number.unwrap_or(1), "1");
        push_event(
            score,
            voice,
            TimedEvent {
                id: stable_id::<EventTag>("event", key.id(), &format!("key/{:?}/{at:?}", context.id)),
                onset: at,
                duration: None,
                grace: None,
                kind: EventKind::KeySignature(value),
                beams: Vec::new(),
                tuplets: Vec::new(),
                articulations: Vec::new(),
                ornaments: Vec::new(),
            },
        );
        let _ = staff;
        report.imported("key signatures");
    }
    for time in attributes.children_named("time") {
        let staff_number = attr_u32(time, "number");
        let meter = parse_meter(time)?;
        score.maps.time_signature.push(Change {
            at,
            scope: map_scope(context, staff_number),
            value: meter.clone(),
        });
        let voice = ensure_voice(score, state, context, staff_number.unwrap_or(1), "1");
        push_event(
            score,
            voice,
            plain_event(
                stable_id::<EventTag>("event", time.id(), &format!("time/{:?}/{at:?}", context.id)),
                at,
                EventKind::TimeSignature(meter),
            ),
        );
        report.imported("time signatures");
    }
    for clef in attributes.children_named("clef") {
        let staff_number = attr_u32(clef, "number").unwrap_or(1);
        let sign = child_text(clef, "sign").unwrap_or_else(|| "G".to_string());
        let line = child_u8(clef, "line").unwrap_or_else(|| if sign == "F" { 4 } else { 2 });
        let octave = child_i8(clef, "clef-octave-change").unwrap_or(0);
        let clef_value = parse_clef(&sign, octave);
        let voice = ensure_voice(score, state, context, staff_number, "1");
        push_event(
            score,
            voice,
            plain_event(
                stable_id::<EventTag>("event", clef.id(), &format!("clef/{:?}/{staff_number}/{at:?}", context.id)),
                at,
                EventKind::Clef(ClefChange {
                    clef: clef_value,
                    line,
                }),
            ),
        );
        report.imported("clef changes");
    }
    for child in attributes.child_elements() {
        if !matches!(
            child.name.as_str(),
            "divisions" | "key" | "time" | "staves" | "clef" | "transpose" | "staff-details" | "instruments"
        ) {
            report.ignored(
                "musicxml.attributes",
                format!("attribute component <{}> was not mapped", child.name),
                source.clone(),
            );
        }
    }
    Ok(())
}

fn convert_staff_details(
    score: &mut makepad_score::model::Score,
    report: &mut ImportReport,
    context: &PartContext,
    details: &XmlElement,
    source: &SourceLocation,
) -> Result<(), ImportError> {
    let number = attr_u32(details, "number").unwrap_or(1);
    let staff_id = staff_for(context, number);
    let mut tunings = details
        .children_named("staff-tuning")
        .filter_map(|tuning| {
            let line = attr_u32(tuning, "line").unwrap_or(1);
            let step = child_text(tuning, "tuning-step")
                .as_deref()
                .and_then(parse_model_step)?;
            let octave = child_i8(tuning, "tuning-octave")?;
            let alter = tuning
                .first_child("tuning-alter")
                .map(XmlElement::direct_text)
                .and_then(|value| parse_decimal_rational(&value).ok())
                .unwrap_or(Rational::ZERO);
            Some((
                line,
                Pitch {
                    step,
                    alter: Alter(alter),
                    octave,
                },
            ))
        })
        .collect::<Vec<_>>();
    if !tunings.is_empty() {
        tunings.sort_by_key(|(line, _)| std::cmp::Reverse(*line));
        if let Some(staff) = score.staves.get_mut(&staff_id) {
            staff.kind = StaffKind::Tablature(Tuning {
                strings_low_to_high: tunings.into_iter().map(|(_, pitch)| pitch).collect(),
            });
        }
        report.imported("tablature tuning");
    }
    if details.first_child("capo").is_some() {
        report.ignored(
            "musicxml.capo",
            "capo position has no semantic score-model field",
            source.clone(),
        );
    }
    if details.first_child("staff-lines").is_some() {
        report.approximated(
            "musicxml.staff-lines",
            "non-default staff-line count is not stored separately from staff kind",
            source.clone(),
        );
    }
    Ok(())
}

fn convert_note(
    score: &mut makepad_score::model::Score,
    report: &mut ImportReport,
    state: &mut ConvertState,
    context: &PartContext,
    note: &XmlElement,
    measure_index: usize,
    onset: ScoreTime,
    duration: Option<Duration>,
    source: &SourceLocation,
) -> Result<(EventId, Option<NoteId>), ImportError> {
    let note_ref = makepad_musicxml::NoteRef(note);
    let staff_number = note_ref.staff().unwrap_or(1);
    let voice_name = note_ref.voice().unwrap_or_else(|| "1".to_string());
    let voice = ensure_voice(score, state, context, staff_number, &voice_name);
    let note_path = format!(
        "part/{}/measure/{measure_index}/note/{}",
        context.index,
        source_occurrence(source)
    );
    let note_id = stable_id::<NoteTag>("note", note_ref.id(), &note_path);
    let event_id = stable_id::<EventTag>("event", note_ref.id(), &format!("{note_path}/event"));
    let mut grace = None;
    if let Some(element) = note_ref.grace() {
        let steal = element
            .attr("steal-time-previous")
            .or_else(|| element.attr("steal-time-following"))
            .map(parse_decimal_rational)
            .transpose()?
            .map(|value| value.checked_div(Rational::new(100, 1).expect("constant")))
            .transpose()?;
        grace = Some(GraceTiming {
            position: if element.attr("steal-time-following").is_some() {
                GracePosition::AfterBeat
            } else {
                GracePosition::BeforeBeat
            },
            steal,
            slash: element.attr("slash") == Some("yes"),
        });
        report.imported("grace notes");
    }

    let mut model_note = None;
    let kind = match note_ref.kind() {
        Some(NoteKind::Pitch(pitch)) => {
            let written = Pitch {
                step: xml_step(pitch.step),
                alter: pitch_alter(note.first_child("pitch"))?,
                octave: pitch.octave,
            };
            let (tab, notehead) = parse_note_details(note)?;
            model_note = Some(Note {
                id: note_id,
                written_pitch: Some(written),
                unpitched_sound: None,
                display_staff: staff_for(context, staff_number),
                tie_from: None,
                tie_to: None,
                tab,
                notehead,
            });
            EventKind::Chord(Vec::new())
        }
        Some(NoteKind::Unpitched(_)) => {
            let (tab, notehead) = parse_note_details(note)?;
            model_note = Some(Note {
                id: note_id,
                written_pitch: None,
                unpitched_sound: note_ref
                    .instrument_id()
                    .map(|value| stable_u16(value)),
                display_staff: staff_for(context, staff_number),
                tie_from: None,
                tie_to: None,
                tab,
                notehead,
            });
            EventKind::Chord(Vec::new())
        }
        Some(NoteKind::Rest(_)) => EventKind::Rest,
        None => {
            report.ignored(
                "musicxml.note-kind",
                "note has no supported pitch, rest, or unpitched child",
                source.clone(),
            );
            EventKind::Rest
        }
    };

    let mut event = TimedEvent {
        id: event_id,
        onset,
        duration: if grace.is_some() { None } else { duration },
        grace,
        kind,
        beams: parse_beams(note),
        tuplets: Vec::new(),
        articulations: parse_articulations(note, report, source),
        ornaments: parse_ornaments(note, report, source),
    };
    add_tuplets(&mut event, note, voice, state, context, source, report)?;

    let actual_event_id = if note_ref.is_chord() {
        let existing = score
            .voices
            .get_mut(&voice)
            .and_then(|voice| {
                voice
                    .events
                    .iter_mut()
                    .rev()
                    .find(|event| event.onset == onset && matches!(event.kind, EventKind::Chord(_)))
            });
        if let Some(existing) = existing {
            if existing.duration != event.duration {
                report.approximated(
                    "musicxml.chord-duration",
                    "different durations inside one chord were represented by the first note's duration",
                    source.clone(),
                );
            }
            existing.beams.extend(event.beams);
            existing.articulations.extend(event.articulations);
            existing.ornaments.extend(event.ornaments);
            existing.tuplets.extend(event.tuplets);
            if let (EventKind::Chord(notes), Some(note)) = (&mut existing.kind, model_note.clone()) {
                notes.push(note);
            }
            existing.id
        } else {
            report.repaired(
                "musicxml.orphan-chord",
                "a chord note had no preceding note in its voice; it was made a new chord",
                source.clone(),
            );
            if let (EventKind::Chord(notes), Some(note)) = (&mut event.kind, model_note.clone()) {
                notes.push(note);
            }
            push_event(score, voice, event);
            event_id
        }
    } else {
        if let (EventKind::Chord(notes), Some(note)) = (&mut event.kind, model_note.clone()) {
            notes.push(note);
        }
        push_event(score, voice, event);
        event_id
    };

    if let Some(model_note) = model_note {
        convert_ties(state, voice, &model_note, note, report, source);
        convert_note_spanners(
            score,
            report,
            state,
            context,
            note,
            model_note.id,
            actual_event_id,
            source,
        )?;
        convert_note_dynamics(score, report, voice, note, onset, source);
        convert_lyrics(score, report, state, voice, model_note.id, note, source);
        report.imported("notes");
    } else {
        report.imported("rests");
    }
    if note_ref.is_cue() {
        report.approximated(
            "musicxml.cue",
            "cue-note sizing is not stored by the semantic model; musical content was imported",
            source.clone(),
        );
    }
    if note_ref.accidental().is_some() {
        report.approximated(
            "musicxml.accidental-display",
            "explicit/cautionary accidental display is not stored; the spelled pitch and key signature were imported",
            source.clone(),
        );
    }
    Ok((actual_event_id, Some(note_id)))
}

fn convert_ties(
    state: &mut ConvertState,
    voice: VoiceId,
    note: &Note,
    element: &XmlElement,
    report: &mut ImportReport,
    source: &SourceLocation,
) {
    let Some(pitch) = note.written_pitch else {
        return;
    };
    let mut stop = false;
    let mut start = false;
    for tie in element.children_named("tie") {
        match tie.attr("type") {
            Some("stop") => stop = true,
            Some("start") => start = true,
            _ => report.ignored(
                "musicxml.tie-type",
                "tie with unknown type was not mapped",
                source.clone(),
            ),
        }
    }
    let key = (voice, pitch);
    if stop {
        if let Some(previous) = state.ties.remove(&key) {
            state.tie_edges.push((previous, note.id));
            report.imported("ties");
        } else {
            report.repaired(
                "musicxml.unmatched-tie-stop",
                "tie stop had no matching start",
                source.clone(),
            );
        }
    }
    if start {
        if state.ties.insert(key, note.id).is_some() {
            report.repaired(
                "musicxml.overlapping-tie",
                "a newer tie start replaced an unmatched start on the same voice and pitch",
                source.clone(),
            );
        }
    }
}

fn convert_note_spanners(
    score: &mut makepad_score::model::Score,
    report: &mut ImportReport,
    state: &mut ConvertState,
    context: &PartContext,
    note: &XmlElement,
    note_id: NoteId,
    _event_id: EventId,
    source: &SourceLocation,
) -> Result<(), ImportError> {
    for notations in note.children_named("notations") {
        for notation in notations.child_elements() {
            match notation.name.as_str() {
                "slur" => {
                    let placement = placement(notation.attr("placement"));
                    handle_spanner_element(
                        score,
                        report,
                        state,
                        context,
                        "slur",
                        notation,
                        SpannerKind::Slur { placement },
                        SpannerEndpoint::Note(note_id),
                        source,
                    );
                }
                "glissando" | "slide" => handle_spanner_element(
                    score,
                    report,
                    state,
                    context,
                    notation.name.as_str(),
                    notation,
                    SpannerKind::Glissando {
                        text: nonempty(notation.direct_text()),
                    },
                    SpannerEndpoint::Note(note_id),
                    source,
                ),
                "ornaments" => {
                    for ornament in notation.children_named("wavy-line") {
                        handle_spanner_element(
                            score,
                            report,
                            state,
                            context,
                            "wavy-line",
                            ornament,
                            SpannerKind::Other("wavy-line".to_string()),
                            SpannerEndpoint::Note(note_id),
                            source,
                        );
                    }
                }
                "tied" | "tuplet" | "articulations" | "technical" | "dynamics" => {}
                "fermata" | "arpeggiate" | "non-arpeggiate" | "accidental-mark" | "other-notation" => report.ignored(
                    "musicxml.notation",
                    format!("notation <{}> has no semantic-model field", notation.name),
                    source.clone(),
                ),
                _ => report.ignored(
                    "musicxml.notation",
                    format!("notation <{}> was not mapped", notation.name),
                    source.clone(),
                ),
            }
        }
    }
    Ok(())
}

fn handle_spanner_element(
    score: &mut makepad_score::model::Score,
    report: &mut ImportReport,
    state: &mut ConvertState,
    context: &PartContext,
    family: &str,
    element: &XmlElement,
    kind: SpannerKind,
    endpoint: SpannerEndpoint,
    source: &SourceLocation,
) {
    let number = element.attr("number").unwrap_or("1");
    let key = format!("{}:{family}:{number}", context.index);
    match element.attr("type") {
        Some("start") => {
            let id = stable_id::<SpannerTag>("spanner", element.id(), &format!("{key}:{source:?}"));
            if state
                .open_spanners
                .insert(
                    key,
                    PendingSpanner {
                        id,
                        kind,
                        start: endpoint,
                        source: source.clone(),
                    },
                )
                .is_some()
            {
                report.repaired(
                    "musicxml.overlapping-spanner",
                    format!("overlapping {family} number {number}; newer start won"),
                    source.clone(),
                );
            }
        }
        Some("stop") => {
            if let Some(pending) = state.open_spanners.remove(&key) {
                score.spanners.insert(
                    pending.id,
                    Spanner {
                        id: pending.id,
                        kind: pending.kind,
                        start: pending.start,
                        end: endpoint,
                    },
                );
                report.imported("spanners");
            } else {
                report.repaired(
                    "musicxml.unmatched-spanner-stop",
                    format!("{family} stop number {number} had no matching start"),
                    source.clone(),
                );
            }
        }
        Some("continue") => {
            if !state.open_spanners.contains_key(&key) {
                report.repaired(
                    "musicxml.unmatched-spanner-continue",
                    format!("{family} continue number {number} had no matching start"),
                    source.clone(),
                );
            }
        }
        _ => report.ignored(
            "musicxml.spanner-type",
            format!("{family} with unknown type was not mapped"),
            source.clone(),
        ),
    }
}

fn convert_note_dynamics(
    score: &mut makepad_score::model::Score,
    report: &mut ImportReport,
    voice: VoiceId,
    note: &XmlElement,
    onset: ScoreTime,
    source: &SourceLocation,
) {
    for dynamics in note
        .children_named("notations")
        .flat_map(|notations| notations.children_named("dynamics"))
    {
        let Some(dynamic) = parse_dynamic_element(dynamics) else {
            report.ignored(
                "musicxml.dynamic",
                "notation dynamic was not in the score model's dynamic vocabulary",
                source.clone(),
            );
            continue;
        };
        let id = stable_id::<EventTag>(
            "event",
            dynamics.id(),
            &format!("note-dynamic/{voice:?}/{onset:?}/{source:?}"),
        );
        push_event(
            score,
            voice,
            plain_event(
                id,
                onset,
                EventKind::Direction(DirectionEvent {
                    kind: DirectionKind::Dynamic(dynamic),
                    placement: placement(dynamics.attr("placement")),
                    original_text: None,
                }),
            ),
        );
        report.imported("dynamics");
    }
}

fn convert_lyrics(
    score: &mut makepad_score::model::Score,
    report: &mut ImportReport,
    state: &mut ConvertState,
    voice: VoiceId,
    note_id: NoteId,
    note: &XmlElement,
    source: &SourceLocation,
) {
    for lyric in note.children_named("lyric") {
        let verse = lyric
            .attr("number")
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(1);
        let text = lyric
            .children_named("text")
            .map(XmlElement::direct_text)
            .collect::<Vec<_>>()
            .join("");
        let role = match child_text(lyric, "syllabic").as_deref() {
            Some("begin") => SyllabicRole::Begin,
            Some("middle") => SyllabicRole::Middle,
            Some("end") => SyllabicRole::End,
            _ => SyllabicRole::Single,
        };
        let elision = lyric
            .children_named("elision")
            .next()
            .map(XmlElement::direct_text);
        let index = score.lyrics.len();
        score.lyrics.push(LyricSyllable {
            note: note_id,
            verse,
            text,
            role,
            elision,
            melisma_to: None,
        });
        let key = (voice, verse);
        if let Some(extend) = lyric.first_child("extend") {
            match extend.attr("type").unwrap_or("start") {
                "start" => {
                    state.open_melismas.insert(key, index);
                }
                "continue" => {}
                "stop" => {
                    if let Some(open) = state.open_melismas.remove(&key) {
                        score.lyrics[open].melisma_to = Some(note_id);
                    } else {
                        report.repaired(
                            "musicxml.unmatched-lyric-extend",
                            "lyric extender stop had no start",
                            source.clone(),
                        );
                    }
                }
                _ => {}
            }
        } else if let Some(open) = state.open_melismas.remove(&key) {
            score.lyrics[open].melisma_to = Some(note_id);
        }
        state.last_lyric_note.insert(key, note_id);
        report.imported("lyrics");
    }
}

fn convert_direction(
    score: &mut makepad_score::model::Score,
    report: &mut ImportReport,
    state: &mut ConvertState,
    context: &PartContext,
    direction: &XmlElement,
    measure_start: ScoreTime,
    cursor: ScoreTime,
    measure_ordinal: u32,
    source: &SourceLocation,
) -> Result<(), ImportError> {
    let offset = direction
        .first_child("offset")
        .map(XmlElement::direct_text)
        .map(|value| parse_decimal_score_time(&value, context.divisions))
        .transpose()?
        .unwrap_or(ScoreTime::ZERO);
    let at = measure_start
        .checked_add_time(cursor)?
        .checked_add_time(offset)?;
    let staff_number = child_u32(direction, "staff").unwrap_or(1);
    let voice_name = child_text(direction, "voice").unwrap_or_else(|| "1".to_string());
    let voice = ensure_voice(score, state, context, staff_number, &voice_name);
    let endpoint = SpannerEndpoint::StaffTime {
        staff: staff_for(context, staff_number),
        at,
    };
    for direction_type in direction.children_named("direction-type") {
        for element in direction_type.child_elements() {
            let event_kind = match element.name.as_str() {
                "words" => {
                    let text = element.direct_text();
                    Some(parse_words_direction(&text, &mut score.flow, measure_ordinal))
                }
                "rehearsal" => Some(DirectionKind::Rehearsal(element.direct_text())),
                "segno" => {
                    add_marker(&mut score.flow, measure_ordinal, MarkerKind::Segno);
                    Some(DirectionKind::Segno)
                }
                "coda" => {
                    add_marker(&mut score.flow, measure_ordinal, MarkerKind::Coda);
                    Some(DirectionKind::Coda)
                }
                "dynamics" => parse_dynamic_element(element).map(DirectionKind::Dynamic),
                "metronome" => {
                    if let Some(tempo) = parse_metronome(element)? {
                        score.maps.tempo.push(Change {
                            at,
                            scope: MapScope::Global,
                            value: Tempo::Instant {
                                quarters_per_minute: tempo,
                            },
                        });
                        report.imported("tempo");
                    }
                    None
                }
                "wedge" => {
                    let wedge_type = element.attr("type").unwrap_or("");
                    let crescendo = wedge_type != "diminuendo";
                    let mut proxy = element.clone();
                    if matches!(wedge_type, "crescendo" | "diminuendo") {
                        proxy.set_attr("type", "start");
                    }
                    handle_spanner_element(
                        score,
                        report,
                        state,
                        context,
                        "wedge",
                        &proxy,
                        SpannerKind::Hairpin {
                            crescendo,
                            niente: element.attr("niente") == Some("yes"),
                        },
                        endpoint,
                        source,
                    );
                    None
                }
                "octave-shift" => {
                    let size = attr_u32(element, "size").unwrap_or(8);
                    let octaves = match size {
                        15 => 2,
                        22 => 3,
                        _ => 1,
                    } * if element.attr("type") == Some("down") { -1 } else { 1 };
                    let mut proxy = element.clone();
                    if matches!(element.attr("type"), Some("up" | "down")) {
                        proxy.set_attr("type", "start");
                    }
                    handle_spanner_element(
                        score,
                        report,
                        state,
                        context,
                        "octave-shift",
                        &proxy,
                        SpannerKind::Ottava { octaves },
                        endpoint,
                        source,
                    );
                    None
                }
                "pedal" => {
                    handle_spanner_element(
                        score,
                        report,
                        state,
                        context,
                        "pedal",
                        element,
                        SpannerKind::Pedal,
                        endpoint,
                        source,
                    );
                    None
                }
                "dashes" | "bracket" => {
                    handle_spanner_element(
                        score,
                        report,
                        state,
                        context,
                        element.name.as_str(),
                        element,
                        SpannerKind::Other(element.name.clone()),
                        endpoint,
                        source,
                    );
                    None
                }
                _ => {
                    report.ignored(
                        "musicxml.direction",
                        format!("direction <{}> was not mapped", element.name),
                        source.clone(),
                    );
                    None
                }
            };
            if let Some(kind) = event_kind {
                let id = stable_id::<EventTag>(
                    "event",
                    element.id(),
                    &format!("direction/{:?}/{at:?}/{}", context.id, element.name),
                );
                push_event(
                    score,
                    voice,
                    plain_event(
                        id,
                        at,
                        EventKind::Direction(DirectionEvent {
                            kind,
                            placement: placement(direction.attr("placement")),
                            original_text: None,
                        }),
                    ),
                );
                report.imported("directions");
            }
        }
    }
    if let Some(sound) = direction.first_child("sound") {
        convert_sound(score, report, sound, at, measure_ordinal, source)?;
    }
    Ok(())
}

fn convert_sound(
    score: &mut makepad_score::model::Score,
    report: &mut ImportReport,
    sound: &XmlElement,
    at: ScoreTime,
    ordinal: u32,
    source: &SourceLocation,
) -> Result<(), ImportError> {
    if let Some(value) = sound.attr("tempo") {
        score.maps.tempo.push(Change {
            at,
            scope: MapScope::Global,
            value: Tempo::Instant {
                quarters_per_minute: parse_decimal_rational(value)?,
            },
        });
        report.imported("tempo");
    }
    if sound.attr("segno").is_some() {
        add_marker(&mut score.flow, ordinal, MarkerKind::Segno);
    }
    if sound.attr("coda").is_some() {
        add_marker(&mut score.flow, ordinal, MarkerKind::Coda);
    }
    if yes_or_value(sound.attr("dacapo")) {
        add_jump(&mut score.flow, ordinal, JumpKind::DaCapo);
    }
    if sound.attr("dalsegno").is_some() {
        add_jump(&mut score.flow, ordinal, JumpKind::DalSegno);
    }
    if sound.attr("tocoda").is_some() {
        add_jump(&mut score.flow, ordinal, JumpKind::ToCoda);
    }
    if yes_or_value(sound.attr("fine")) {
        add_jump(&mut score.flow, ordinal, JumpKind::Fine);
    }
    let known = ["tempo", "segno", "coda", "dacapo", "dalsegno", "tocoda", "fine", "dynamics"];
    for attribute in &sound.attributes {
        if !known.contains(&attribute.name.as_str()) {
            report.ignored(
                "musicxml.sound",
                format!("sound attribute {} was not mapped", attribute.name),
                source.clone(),
            );
        }
    }
    Ok(())
}

fn parse_words_direction(text: &str, flow: &mut FlowGraph, ordinal: u32) -> DirectionKind {
    let normalized = text
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '.')
        .flat_map(char::to_lowercase)
        .collect::<String>();
    match normalized.as_str() {
        "dc" | "dacapo" => {
            add_jump(flow, ordinal, JumpKind::DaCapo);
            DirectionKind::DaCapo
        }
        "ds" | "dalsegno" => {
            add_jump(flow, ordinal, JumpKind::DalSegno);
            DirectionKind::DalSegno
        }
        "tocoda" => {
            add_jump(flow, ordinal, JumpKind::ToCoda);
            DirectionKind::ToCoda
        }
        "fine" => {
            add_jump(flow, ordinal, JumpKind::Fine);
            DirectionKind::Fine
        }
        _ => DirectionKind::Words(text.to_string()),
    }
}

fn convert_harmony(
    score: &mut makepad_score::model::Score,
    report: &mut ImportReport,
    state: &mut ConvertState,
    context: &PartContext,
    harmony: &XmlElement,
    at: ScoreTime,
    measure_index: usize,
    source: &SourceLocation,
) -> Result<(), ImportError> {
    let Some(root) = harmony.first_child("root") else {
        report.ignored(
            "musicxml.harmony-root",
            "harmony without a root was not mapped",
            source.clone(),
        );
        return Ok(());
    };
    let root_step = child_text(root, "root-step")
        .as_deref()
        .and_then(parse_model_step)
        .ok_or_else(|| ImportError::InvalidSource("invalid harmony root-step".to_string()))?;
    let root_alter = optional_decimal_alter(root.first_child("root-alter"))?;
    let kind_element = harmony.first_child("kind");
    let kind_text = kind_element
        .map(XmlElement::direct_text)
        .unwrap_or_else(|| "major".to_string());
    let quality = match kind_text.trim() {
        "major" => ChordQuality::Major,
        "minor" => ChordQuality::Minor,
        "augmented" => ChordQuality::Augmented,
        "diminished" | "half-diminished" => ChordQuality::Diminished,
        "dominant" => ChordQuality::Dominant,
        "suspended-second" | "suspended-fourth" => ChordQuality::Suspended,
        other => ChordQuality::Other(other.to_string()),
    };
    let mut degrees = Vec::new();
    for degree in harmony.children_named("degree") {
        let value = child_u8(degree, "degree-value").unwrap_or(0);
        let alter = optional_decimal_alter(degree.first_child("degree-alter"))?;
        let operation = match child_text(degree, "degree-type").as_deref() {
            Some("alter") => DegreeOperation::Alter,
            Some("subtract") => DegreeOperation::Subtract,
            _ => DegreeOperation::Add,
        };
        degrees.push(ChordDegree {
            value,
            alter,
            operation,
        });
    }
    let bass = harmony.first_child("bass").and_then(|bass| {
        Some(PitchClass {
            step: child_text(bass, "bass-step")
                .as_deref()
                .and_then(parse_model_step)?,
            alter: optional_decimal_alter(bass.first_child("bass-alter")).ok()?,
        })
    });
    let chord = ChordSymbol {
        root: PitchClass {
            step: root_step,
            alter: root_alter,
        },
        quality,
        degrees,
        bass,
        original_text: kind_element
            .and_then(|element| element.attr("text"))
            .unwrap_or(&kind_text)
            .to_string(),
    };
    let staff_number = child_u32(harmony, "staff").unwrap_or(1);
    let voice = ensure_voice(score, state, context, staff_number, "1");
    push_event(
        score,
        voice,
        plain_event(
            stable_id::<EventTag>("event", harmony.id(), &format!("harmony/{}/{measure_index}/{at:?}", context.index)),
            at,
            EventKind::ChordSymbol(chord),
        ),
    );
    report.imported("chord symbols");
    Ok(())
}

fn convert_figured_bass(
    score: &mut makepad_score::model::Score,
    report: &mut ImportReport,
    state: &mut ConvertState,
    context: &PartContext,
    figured: &XmlElement,
    at: ScoreTime,
    measure_index: usize,
    source: &SourceLocation,
) -> Result<(), ImportError> {
    let figures = figured
        .children_named("figure")
        .filter_map(|figure| {
            Some(Figure {
                interval: child_u8(figure, "figure-number")?,
                prefix: child_text(figure, "prefix"),
                suffix: child_text(figure, "suffix"),
            })
        })
        .collect::<Vec<_>>();
    if figures.is_empty() {
        report.ignored(
            "musicxml.figured-bass",
            "empty figured-bass element was not mapped",
            source.clone(),
        );
        return Ok(());
    }
    let voice = ensure_voice(score, state, context, 1, "1");
    push_event(
        score,
        voice,
        plain_event(
            stable_id::<EventTag>("event", figured.id(), &format!("figured/{}/{measure_index}/{at:?}", context.index)),
            at,
            EventKind::FiguredBass(FiguredBass {
                figures,
                continuation: None,
            }),
        ),
    );
    report.imported("figured bass");
    Ok(())
}

fn convert_barline(
    score: &mut makepad_score::model::Score,
    report: &mut ImportReport,
    state: &mut ConvertState,
    context: &PartContext,
    barline: &XmlElement,
    measure_start: ScoreTime,
    extent: Duration,
    ordinal: u32,
    source: &SourceLocation,
) -> Result<(), ImportError> {
    let is_left = barline.attr("location") == Some("left");
    let at = if is_left {
        measure_start
    } else {
        measure_start.checked_add(extent)?
    };
    let mut repeat_value = None;
    for repeat in barline.children_named("repeat") {
        match repeat.attr("direction") {
            Some("forward") => {
                if context.index == 0 {
                    state.repeat_start = ordinal;
                    state.last_repeat_start = ordinal;
                }
                repeat_value = Some(RepeatDirection::Forward);
            }
            Some("backward") => {
                let times = attr_u16(repeat, "times").unwrap_or(2).max(1);
                if context.index == 0 {
                    score.flow.repeats.push(RepeatSection {
                        start: state.repeat_start,
                        end: ordinal,
                        times,
                    });
                    state.last_repeat_start = state.repeat_start;
                }
                repeat_value = Some(RepeatDirection::Backward { times });
                report.imported("repeats");
            }
            _ => report.ignored(
                "musicxml.repeat-direction",
                "repeat with unknown direction was not mapped",
                source.clone(),
            ),
        }
    }
    for ending in barline
        .children_named("ending")
        .filter(|_| context.index == 0)
    {
        let number = ending.attr("number").unwrap_or("1").to_string();
        let passes = parse_passes(&number);
        match ending.attr("type") {
            Some("start") => {
                state
                    .open_voltas
                    .insert(number.clone(), (ordinal, state.repeat_start, passes));
            }
            Some("stop") | Some("discontinue") => {
                if let Some((start, repeat_start, passes)) = state.open_voltas.remove(&number) {
                    state.closed_voltas.push(VoltaEnding {
                        start,
                        end: ordinal,
                        repeat_start,
                        passes,
                    });
                    report.imported("voltas");
                } else {
                    report.repaired(
                        "musicxml.unmatched-ending-stop",
                        format!("ending {number} stop had no matching start"),
                        source.clone(),
                    );
                }
            }
            _ => report.ignored(
                "musicxml.ending-type",
                "ending with unknown type was not mapped",
                source.clone(),
            ),
        }
    }
    let style = match child_text(barline, "bar-style").as_deref() {
        Some("dotted") => BarlineStyle::Dotted,
        Some("dashed") => BarlineStyle::Dashed,
        Some("heavy") | Some("heavy-heavy") => BarlineStyle::Heavy,
        Some("light-light") => BarlineStyle::Double,
        Some("light-heavy") => BarlineStyle::Final,
        Some("none") => BarlineStyle::Invisible,
        _ => BarlineStyle::Regular,
    };
    let voice = ensure_voice(score, state, context, 1, "1");
    push_event(
        score,
        voice,
        plain_event(
            stable_id::<EventTag>("event", barline.id(), &format!("barline/{}/{ordinal}/{is_left}", context.index)),
            at,
            EventKind::Barline(Barline {
                style,
                repeat: repeat_value,
            }),
        ),
    );
    report.imported("barlines");
    Ok(())
}

fn finish_relations(
    score: &mut makepad_score::model::Score,
    report: &mut ImportReport,
    state: ConvertState,
) -> Result<(), ImportError> {
    for (from, to) in state.tie_edges {
        if let Some(note) = score.note_mut(from) {
            note.tie_to = Some(to);
        }
        if let Some(note) = score.note_mut(to) {
            note.tie_from = Some(from);
        }
    }
    for (_, pending) in state.open_spanners {
        report.ignored(
            "musicxml.unclosed-spanner",
            format!("unclosed {:?} was not inserted", pending.kind),
            pending.source,
        );
    }
    for ((_, _), note) in state.ties {
        report.ignored(
            "musicxml.unclosed-tie",
            format!("tie starting at note {:?} has no stop", note),
            SourceLocation::Document,
        );
    }
    for (_, (start, repeat_start, passes)) in state.open_voltas {
        let end = score.flow.nodes.len().saturating_sub(1) as u32;
        report.repaired(
            "musicxml.unclosed-ending",
            "unclosed volta was extended to the final measure",
            SourceLocation::Document,
        );
        score.flow.voltas.push(VoltaEnding {
            start,
            end,
            repeat_start,
            passes,
        });
    }
    score.flow.voltas.extend(state.closed_voltas);
    for repeat in &mut score.flow.repeats {
        if let Some(last_ending) = score
            .flow
            .voltas
            .iter()
            .filter(|volta| volta.repeat_start == repeat.start)
            .map(|volta| volta.end)
            .max()
        {
            repeat.end = repeat.end.max(last_ending);
        }
    }
    for voice in score.voices.values_mut() {
        voice.events.sort_by_key(|event| (event.onset, event.id));
    }
    Ok(())
}

fn add_tuplets(
    event: &mut TimedEvent,
    note: &XmlElement,
    voice: VoiceId,
    state: &mut ConvertState,
    context: &PartContext,
    source: &SourceLocation,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let Some(modification) = note.first_child("time-modification") else {
        state.implicit_tuplets.remove(&voice);
        return Ok(());
    };
    let actual = child_u16(modification, "actual-notes").unwrap_or(0);
    let normal = child_u16(modification, "normal-notes").unwrap_or(0);
    if actual == 0 || normal == 0 {
        report.ignored(
            "musicxml.invalid-tuplet",
            "time-modification with zero ratio was not mapped",
            source.clone(),
        );
        return Ok(());
    }
    let mut explicit_number = None;
    let mut explicit_type = None;
    let mut bracket = false;
    for notations in note.children_named("notations") {
        for tuplet in notations.children_named("tuplet") {
            explicit_number = Some(tuplet.attr("number").unwrap_or("1").to_string());
            explicit_type = tuplet.attr("type").map(str::to_string);
            bracket = tuplet.attr("bracket") == Some("yes");
        }
    }
    let group = if let Some(number) = explicit_number {
        let key = format!("{}:tuplet:{voice:?}:{number}", context.index);
        match explicit_type.as_deref() {
            Some("start") => {
                let group = stable_id::<SpannerTag>("tuplet", None, &format!("{key}:{source:?}"));
                state.implicit_tuplets.insert(
                    voice,
                    ImplicitTuplet {
                        actual,
                        normal,
                        group,
                        remaining: actual,
                    },
                );
                group
            }
            _ => state
                .implicit_tuplets
                .get(&voice)
                .map(|active| active.group)
                .unwrap_or_else(|| stable_id::<SpannerTag>("tuplet", None, &key)),
        }
    } else {
        let needs_new = state
            .implicit_tuplets
            .get(&voice)
            .is_none_or(|active| active.actual != actual || active.normal != normal || active.remaining == 0);
        if needs_new {
            let group = stable_id::<SpannerTag>(
                "tuplet",
                None,
                &format!("{}:{voice:?}:{source:?}", context.index),
            );
            state.implicit_tuplets.insert(
                voice,
                ImplicitTuplet {
                    actual,
                    normal,
                    group,
                    remaining: actual,
                },
            );
        }
        state.implicit_tuplets.get(&voice).expect("inserted").group
    };
    if let Some(active) = state.implicit_tuplets.get_mut(&voice) {
        active.remaining = active.remaining.saturating_sub(1);
        if explicit_type.as_deref() == Some("stop") {
            active.remaining = 0;
        }
    }
    event.tuplets.push(TupletNotation {
        actual,
        normal,
        group,
        level: 1,
        bracket,
    });
    report.imported("tuplets");
    Ok(())
}

fn parse_articulations(
    note: &XmlElement,
    report: &mut ImportReport,
    source: &SourceLocation,
) -> Vec<PlacedArticulation> {
    let mut output = Vec::new();
    for notations in note.children_named("notations") {
        for group in notations.children_named("articulations") {
            for element in group.child_elements() {
                let kind = match element.name.as_str() {
                    "accent" => Some(Articulation::Accent),
                    "staccato" => Some(Articulation::Staccato),
                    "tenuto" => Some(Articulation::Tenuto),
                    "staccatissimo" => Some(Articulation::Staccatissimo),
                    "strong-accent" => Some(Articulation::Marcato),
                    "stress" => Some(Articulation::Stress),
                    "soft-accent" => Some(Articulation::SoftAccent),
                    "detached-legato" => Some(Articulation::TenutoStaccato),
                    _ => None,
                };
                if let Some(kind) = kind {
                    output.push(PlacedArticulation {
                        kind,
                        placement: placement(element.attr("placement")),
                    });
                    report.imported("articulations");
                } else {
                    report.ignored(
                        "musicxml.articulation",
                        format!("articulation <{}> was not mapped", element.name),
                        source.clone(),
                    );
                }
            }
        }
    }
    output
}

fn parse_ornaments(
    note: &XmlElement,
    report: &mut ImportReport,
    source: &SourceLocation,
) -> Vec<Ornament> {
    let mut output = Vec::new();
    for notations in note.children_named("notations") {
        for group in notations.children_named("ornaments") {
            for element in group.child_elements() {
                let ornament = match element.name.as_str() {
                    "trill-mark" => Some(Ornament::Trill),
                    "turn" | "delayed-turn" => Some(Ornament::Turn),
                    "inverted-turn" | "delayed-inverted-turn" => Some(Ornament::InvertedTurn),
                    "mordent" => Some(Ornament::Mordent),
                    "inverted-mordent" => Some(Ornament::ShortTrill),
                    "schleifer" => Some(Ornament::Schleifer),
                    _ => None,
                };
                if let Some(ornament) = ornament {
                    output.push(ornament);
                    report.imported("ornaments");
                } else if !matches!(element.name.as_str(), "accidental-mark" | "wavy-line") {
                    report.ignored(
                        "musicxml.ornament",
                        format!("ornament <{}> was not mapped", element.name),
                        source.clone(),
                    );
                }
            }
        }
    }
    output
}

fn parse_beams(note: &XmlElement) -> Vec<BeamMembership> {
    note.children_named("beam")
        .filter_map(|beam| {
            let level = attr_u8(beam, "number").unwrap_or(1);
            let state = match beam.direct_text().trim() {
                "begin" => BeamState::Begin,
                "continue" => BeamState::Continue,
                "end" => BeamState::End,
                "forward hook" => BeamState::ForwardHook,
                "backward hook" => BeamState::BackwardHook,
                _ => return None,
            };
            Some(BeamMembership { level, state })
        })
        .collect()
}

fn measure_extents(
    parts: &[makepad_musicxml::PartRef<'_>],
    report: &mut ImportReport,
) -> Result<Vec<Duration>, ImportError> {
    let count = parts.iter().map(|part| part.measures().count()).max().unwrap_or(0);
    let mut extents = vec![Duration::new(1, 1)?; count];
    let mut has_extent = vec![false; count];
    for part in parts {
        let mut divisions = 1_u32;
        let mut meter = Meter::Measured {
            groups: vec![4],
            unit: 4,
        };
        for (index, measure) in part.measures().enumerate() {
            let mut cursor = ScoreTime::ZERO;
            let mut max = ScoreTime::ZERO;
            for item in measure.element().child_elements() {
                match item.name.as_str() {
                    "attributes" => {
                        if let Some(value) = child_u32(item, "divisions") {
                            if value > 0 {
                                divisions = value;
                            }
                        }
                        if let Some(time) = item.first_child("time") {
                            meter = parse_meter(time)?;
                        }
                    }
                    "note" if item.first_child("chord").is_none() && item.first_child("grace").is_none() => {
                        if let Some(value) = child_u32(item, "duration") {
                            cursor = cursor.checked_add(duration_from_divisions(value, divisions)?)?;
                            max = max.max(cursor);
                        }
                    }
                    "backup" => {
                        if let Some(value) = child_u32(item, "duration") {
                            let amount = score_time_from_divisions(value, divisions)?;
                            cursor = cursor.checked_sub(amount)?.max(ScoreTime::ZERO);
                        }
                    }
                    "forward" => {
                        if let Some(value) = child_u32(item, "duration") {
                            cursor = cursor.checked_add_time(score_time_from_divisions(value, divisions)?)?;
                            max = max.max(cursor);
                        }
                    }
                    _ => {}
                }
            }
            let candidate = if max > ScoreTime::ZERO {
                Duration::from_rational(max.0)?
            } else {
                meter.duration()?.unwrap_or(Duration::new(1, 1)?)
            };
            if !has_extent[index] || candidate > extents[index] {
                extents[index] = candidate;
                has_extent[index] = true;
            }
        }
    }
    for (index, present) in has_extent.into_iter().enumerate() {
        if !present {
            report.repaired(
                "musicxml.empty-measure",
                "measure with no duration-bearing content used a whole-note extent",
                SourceLocation::MusicXml {
                    part: None,
                    measure: Some((index + 1).to_string()),
                    element: "measure".to_string(),
                    occurrence: index,
                },
            );
        }
    }
    Ok(extents)
}

fn ensure_voice(
    score: &mut makepad_score::model::Score,
    state: &mut ConvertState,
    context: &PartContext,
    staff_number: u32,
    source_number: &str,
) -> VoiceId {
    let key = (context.index, staff_number, source_number.to_string());
    if let Some(id) = state.voices.get(&key) {
        return *id;
    }
    let staff = staff_for(context, staff_number);
    let id = stable_id::<VoiceTag>(
        "voice",
        None,
        &format!("part/{}/staff/{staff_number}/voice/{source_number}", context.index),
    );
    let number = source_number.parse::<u16>().ok().filter(|value| *value > 0).unwrap_or_else(|| {
        let next = state.voice_numbers.entry((context.index, staff_number)).or_insert(0);
        *next = next.saturating_add(1);
        *next
    });
    score.voices.insert(
        id,
        Voice {
            id,
            staff,
            number,
            events: Vec::new(),
        },
    );
    if let Some(staff) = score.staves.get_mut(&staff) {
        staff.voices.push(id);
    }
    state.voices.insert(key, id);
    id
}

fn push_event(score: &mut makepad_score::model::Score, voice: VoiceId, event: TimedEvent) {
    if let Some(voice) = score.voices.get_mut(&voice) {
        voice.events.push(event);
    }
}

fn plain_event(id: EventId, onset: ScoreTime, kind: EventKind) -> TimedEvent {
    TimedEvent {
        id,
        onset,
        duration: None,
        grace: None,
        kind,
        beams: Vec::new(),
        tuplets: Vec::new(),
        articulations: Vec::new(),
        ornaments: Vec::new(),
    }
}

fn score_identity(root: &XmlElement, title: &str) -> String {
    if let Some(id) = root.id() {
        return format!("id:{id}");
    }
    let parts = root
        .first_child("part-list")
        .into_iter()
        .flat_map(|list| list.children_named("score-part"))
        .filter_map(XmlElement::id)
        .collect::<Vec<_>>()
        .join("|");
    format!("title:{title}|parts:{parts}")
}

fn part_names(root: &XmlElement) -> BTreeMap<String, String> {
    root.first_child("part-list")
        .into_iter()
        .flat_map(|list| list.children_named("score-part"))
        .filter_map(|part| {
            Some((
                part.id()?.to_string(),
                child_text(part, "part-name").unwrap_or_default(),
            ))
        })
        .collect()
}

fn detect_staff_count(part: &XmlElement) -> u32 {
    let from_attributes = part
        .children_named("measure")
        .flat_map(|measure| measure.children_named("attributes"))
        .filter_map(|attributes| child_u32(attributes, "staves"))
        .max()
        .unwrap_or(1);
    let from_notes = part
        .children_named("measure")
        .flat_map(|measure| measure.children_named("note"))
        .filter_map(|note| child_u32(note, "staff"))
        .max()
        .unwrap_or(1);
    from_attributes.max(from_notes)
}

fn detect_staff_kind(part: &XmlElement, staff_number: u32) -> StaffKind {
    for details in part
        .children_named("measure")
        .flat_map(|measure| measure.children_named("attributes"))
        .flat_map(|attributes| attributes.children_named("staff-details"))
    {
        if attr_u32(details, "number").unwrap_or(1) != staff_number {
            continue;
        }
        return match child_text(details, "staff-type").as_deref() {
            Some("tablature") => StaffKind::Tablature(Tuning {
                strings_low_to_high: Vec::new(),
            }),
            Some("percussion") => StaffKind::Percussion(PercussionMap {
                entries: OrderedMap::new(),
            }),
            Some("ossia") => StaffKind::Ossia,
            Some("cue") => StaffKind::Ossia,
            _ => StaffKind::Standard,
        };
    }
    StaffKind::Standard
}

fn detect_transposition(
    part: &XmlElement,
    report: &mut ImportReport,
    part_name: &str,
) -> Result<Transposition, ImportError> {
    let transposes = part
        .children_named("measure")
        .flat_map(|measure| measure.children_named("attributes"))
        .flat_map(|attributes| attributes.children_named("transpose"))
        .collect::<Vec<_>>();
    let Some(first) = transposes.first() else {
        return Ok(Transposition::NONE);
    };
    if transposes.iter().skip(1).any(|value| *value != *first) {
        report.approximated(
            "musicxml.changing-transposition",
            "the score model stores transposition per part; the first transpose value was used",
            SourceLocation::MusicXml {
                part: Some(part_name.to_string()),
                measure: None,
                element: "transpose".to_string(),
                occurrence: 0,
            },
        );
    }
    let chromatic = first
        .first_child("chromatic")
        .map(XmlElement::direct_text)
        .map(|value| parse_decimal_rational(&value))
        .transpose()?
        .unwrap_or(Rational::ZERO);
    Ok(Transposition {
        diatonic_steps: child_i16(first, "diatonic").unwrap_or(0),
        chromatic_semitones: Alter(chromatic),
        octave_shift: child_i8(first, "octave-change").unwrap_or(0),
    })
}

fn parse_note_details(note: &XmlElement) -> Result<(Option<TabPosition>, Notehead), ImportError> {
    let notehead = match note.first_child("notehead").map(XmlElement::direct_text) {
        Some(value) => match value.trim() {
            "normal" => Notehead::Normal,
            "x" => Notehead::X,
            "diamond" => Notehead::Diamond,
            "triangle" => Notehead::Triangle,
            "slash" => Notehead::Slash,
            other => Notehead::Other(other.to_string()),
        },
        None => Notehead::Normal,
    };
    let technical = note
        .children_named("notations")
        .flat_map(|notations| notations.children_named("technical"))
        .next();
    let tab = technical.and_then(|technical| {
        let string = child_u16(technical, "string")?;
        let fret = child_u16(technical, "fret")?;
        let bend = technical
            .first_child("bend")
            .and_then(|bend| bend.first_child("bend-alter"))
            .map(XmlElement::direct_text)
            .and_then(|value| parse_decimal_rational(&value).ok())
            .unwrap_or(Rational::ZERO);
        Some(TabPosition {
            string,
            fret,
            bend: Alter(bend),
        })
    });
    Ok((tab, notehead))
}

fn parse_custom_key(key: &XmlElement) -> Result<Vec<(Step, Alter)>, ImportError> {
    let children = key.child_elements().collect::<Vec<_>>();
    let mut output = Vec::new();
    for window in children.windows(2) {
        if window[0].name == "key-step" && window[1].name == "key-alter" {
            if let Some(step) = parse_model_step(window[0].direct_text().trim()) {
                output.push((
                    step,
                    Alter(parse_decimal_rational(&window[1].direct_text())?),
                ));
            }
        }
    }
    Ok(output)
}

fn parse_meter(time: &XmlElement) -> Result<Meter, ImportError> {
    if time.first_child("senza-misura").is_some() {
        return Ok(Meter::Free);
    }
    let children = time.child_elements().collect::<Vec<_>>();
    let mut components = Vec::<(Vec<u16>, u16)>::new();
    let mut index = 0;
    while index + 1 < children.len() {
        if children[index].name == "beats" && children[index + 1].name == "beat-type" {
            let current_unit = children[index + 1]
                .direct_text()
                .trim()
                .parse::<u16>()
                .map_err(|_| ImportError::InvalidSource("invalid beat-type".to_string()))?;
            let mut groups = Vec::new();
            for group in children[index].direct_text().split('+') {
                groups.push(group.trim().parse::<u16>().map_err(|_| {
                    ImportError::InvalidSource("invalid beats value".to_string())
                })?);
            }
            components.push((groups, current_unit));
            index += 2;
        } else {
            index += 1;
        }
    }
    if components.is_empty() {
        return Ok(Meter::Measured {
            groups: vec![4],
            unit: 4,
        });
    }
    let common_unit = components.iter().try_fold(1_u64, |common, (_, unit)| {
        let divisor = meter_gcd(common, u64::from(*unit));
        common
            .checked_div(divisor)
            .and_then(|value| value.checked_mul(u64::from(*unit)))
            .ok_or_else(|| ImportError::InvalidSource("meter denominator overflow".to_string()))
    })?;
    let common_unit = u16::try_from(common_unit)
        .map_err(|_| ImportError::InvalidSource("meter denominator exceeds u16".to_string()))?;
    let mut groups = Vec::new();
    for (component_groups, unit) in components {
        let scale = common_unit / unit;
        for group in component_groups {
            groups.push(group.checked_mul(scale).ok_or_else(|| {
                ImportError::InvalidSource("meter group overflow".to_string())
            })?);
        }
    }
    Ok(Meter::Measured {
        groups,
        unit: common_unit,
    })
}

fn meter_gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left.max(1)
}

fn parse_metronome(element: &XmlElement) -> Result<Option<Rational>, ImportError> {
    let Some(per_minute) = child_text(element, "per-minute") else {
        return Ok(None);
    };
    let beat_unit = child_text(element, "beat-unit").unwrap_or_else(|| "quarter".to_string());
    let base = match beat_unit.trim() {
        "maxima" => Rational::new(32, 1)?,
        "long" => Rational::new(16, 1)?,
        "breve" => Rational::new(8, 1)?,
        "whole" => Rational::new(4, 1)?,
        "half" => Rational::new(2, 1)?,
        "quarter" => Rational::ONE,
        "eighth" => Rational::new(1, 2)?,
        "16th" => Rational::new(1, 4)?,
        "32nd" => Rational::new(1, 8)?,
        "64th" => Rational::new(1, 16)?,
        _ => Rational::ONE,
    };
    let mut duration = base;
    let mut addition = base;
    for _ in element.children_named("beat-unit-dot") {
        addition = addition.checked_div(Rational::new(2, 1)?)?;
        duration = duration.checked_add(addition)?;
    }
    Ok(Some(parse_decimal_rational(&per_minute)?.checked_mul(duration)?))
}

fn parse_dynamic_element(element: &XmlElement) -> Option<DynamicMark> {
    let name = element.child_elements().next()?.name.as_str();
    Some(match name {
        "p" => DynamicMark::Piano,
        "pp" => DynamicMark::Pianissimo,
        "ppp" => DynamicMark::Pianississimo,
        "pppp" => DynamicMark::Pianissississimo,
        "mp" => DynamicMark::MezzoPiano,
        "mf" => DynamicMark::MezzoForte,
        "f" => DynamicMark::Forte,
        "ff" => DynamicMark::Fortissimo,
        "fff" => DynamicMark::Fortississimo,
        "ffff" => DynamicMark::Fortissississimo,
        "fp" => DynamicMark::FortePiano,
        "sf" | "sfz" => DynamicMark::Sforzando,
        "sfp" => DynamicMark::SforzandoPiano,
        "fz" => DynamicMark::Sforzato,
        "rf" | "rfz" => DynamicMark::Rinforzando,
        "n" => DynamicMark::Niente,
        _ => return None,
    })
}

fn parse_clef(sign: &str, octave: i8) -> Clef {
    match (sign.trim(), octave) {
        ("G", 1) => Clef::G8va,
        ("G", -1) => Clef::G8vb,
        ("G", 2) => Clef::G15ma,
        ("G", -2) => Clef::G15mb,
        ("G", _) => Clef::G,
        ("F", 1) => Clef::F8va,
        ("F", -1) => Clef::F8vb,
        ("F", 2) => Clef::F15ma,
        ("F", -2) => Clef::F15mb,
        ("F", _) => Clef::F,
        ("C", _) => Clef::C,
        ("percussion", _) => Clef::Percussion,
        ("TAB", _) => Clef::Tab6String,
        _ => Clef::G,
    }
}

fn parse_passes(value: &str) -> Vec<u16> {
    let mut passes = BTreeSet::new();
    for component in value.split(',') {
        let component = component.trim();
        if let Some((start, end)) = component.split_once('-') {
            if let (Ok(start), Ok(end)) = (
                start.trim().parse::<u16>(),
                end.trim().parse::<u16>(),
            ) {
                passes.extend(start..=end);
            }
        } else if let Ok(pass) = component.parse::<u16>() {
            passes.insert(pass);
        }
    }
    if passes.is_empty() {
        passes.insert(1);
    }
    passes.into_iter().collect()
}

fn parse_decimal_rational(value: &str) -> Result<Rational, ImportError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ImportError::InvalidSource("empty decimal".to_string()));
    }
    let (negative, unsigned) = value
        .strip_prefix('-')
        .map_or((false, value), |value| (true, value));
    let unsigned = unsigned.strip_prefix('+').unwrap_or(unsigned);
    let (mantissa, exponent) = unsigned
        .split_once(['e', 'E'])
        .map_or((unsigned, 0_i32), |(mantissa, exponent)| {
            (mantissa, exponent.parse::<i32>().unwrap_or(i32::MIN))
        });
    if exponent == i32::MIN {
        return Err(ImportError::InvalidSource(format!("invalid decimal {value:?}")));
    }
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty() && fraction.is_empty()
        || !whole.chars().chain(fraction.chars()).all(|character| character.is_ascii_digit())
    {
        return Err(ImportError::InvalidSource(format!("invalid decimal {value:?}")));
    }
    let digits = format!("{}{}", if whole.is_empty() { "0" } else { whole }, fraction);
    let mut numerator = digits
        .parse::<i128>()
        .map_err(|_| ImportError::InvalidSource(format!("decimal is too large: {value}")))?;
    if negative {
        numerator = -numerator;
    }
    let scale = i32::try_from(fraction.len()).unwrap_or(i32::MAX) - exponent;
    if scale >= 0 {
        let denominator = 10_u128
            .checked_pow(scale as u32)
            .ok_or_else(|| ImportError::InvalidSource(format!("decimal is too precise: {value}")))?;
        Rational::new(
            i64::try_from(numerator).map_err(|_| ImportError::InvalidSource("decimal overflow".to_string()))?,
            u64::try_from(denominator).map_err(|_| ImportError::InvalidSource("decimal overflow".to_string()))?,
        )
        .map_err(ImportError::from)
    } else {
        let multiplier = 10_i128
            .checked_pow((-scale) as u32)
            .ok_or_else(|| ImportError::InvalidSource(format!("decimal is too large: {value}")))?;
        Rational::new(
            i64::try_from(numerator.checked_mul(multiplier).ok_or_else(|| ImportError::InvalidSource("decimal overflow".to_string()))?)
                .map_err(|_| ImportError::InvalidSource("decimal overflow".to_string()))?,
            1,
        )
        .map_err(ImportError::from)
    }
}

fn duration_from_divisions(value: u32, divisions: u32) -> Result<Duration, ImportError> {
    if divisions == 0 || value == 0 {
        return Err(ImportError::InvalidSource(
            "duration and divisions must be positive".to_string(),
        ));
    }
    Ok(Duration::new(i64::from(value), u64::from(divisions) * 4)?)
}

fn score_time_from_divisions(value: u32, divisions: u32) -> Result<ScoreTime, ImportError> {
    if divisions == 0 {
        return Err(ImportError::InvalidSource("divisions must be positive".to_string()));
    }
    Ok(ScoreTime::new(i64::from(value), u64::from(divisions) * 4)?)
}

fn parse_decimal_score_time(value: &str, divisions: u32) -> Result<ScoreTime, ImportError> {
    let units = parse_decimal_rational(value)?;
    Ok(ScoreTime(units.checked_div(Rational::new(i64::from(divisions) * 4, 1)?)?))
}

fn optional_decimal_alter(element: Option<&XmlElement>) -> Result<Alter, ImportError> {
    Ok(Alter(match element {
        Some(element) => parse_decimal_rational(&element.direct_text())?,
        None => Rational::ZERO,
    }))
}

fn pitch_alter(pitch: Option<&XmlElement>) -> Result<Alter, ImportError> {
    optional_decimal_alter(pitch.and_then(|pitch| pitch.first_child("alter")))
}

fn xml_step(step: makepad_musicxml::Step) -> Step {
    match step {
        makepad_musicxml::Step::A => Step::A,
        makepad_musicxml::Step::B => Step::B,
        makepad_musicxml::Step::C => Step::C,
        makepad_musicxml::Step::D => Step::D,
        makepad_musicxml::Step::E => Step::E,
        makepad_musicxml::Step::F => Step::F,
        makepad_musicxml::Step::G => Step::G,
    }
}

fn parse_model_step(value: &str) -> Option<Step> {
    Some(match value.trim() {
        "A" => Step::A,
        "B" => Step::B,
        "C" => Step::C,
        "D" => Step::D,
        "E" => Step::E,
        "F" => Step::F,
        "G" => Step::G,
        _ => return None,
    })
}

fn map_scope(context: &PartContext, staff: Option<u32>) -> MapScope {
    match staff {
        Some(staff) => MapScope::Staff(staff_for(context, staff)),
        None if context.index == 0 => MapScope::Global,
        None => MapScope::Part(context.id),
    }
}

fn staff_for(context: &PartContext, number: u32) -> StaffId {
    let index = usize::try_from(number.saturating_sub(1)).unwrap_or(0);
    context
        .staves
        .get(index)
        .copied()
        .unwrap_or(context.staves[0])
}

fn placement(value: Option<&str>) -> Option<Placement> {
    match value {
        Some("above") => Some(Placement::Above),
        Some("below") => Some(Placement::Below),
        _ => None,
    }
}

fn add_marker(flow: &mut FlowGraph, at: u32, kind: MarkerKind) {
    if !flow.markers.iter().any(|marker| marker.at == at && marker.kind == kind) {
        flow.markers.push(FlowMarker { at, kind });
    }
}

fn add_jump(flow: &mut FlowGraph, at: u32, kind: JumpKind) {
    if !flow.jumps.iter().any(|jump| jump.at == at && jump.kind == kind) {
        flow.jumps.push(JumpInstruction { at, kind });
    }
}

fn xml_location(part: &str, measure: &str, element: &str, occurrence: usize) -> SourceLocation {
    SourceLocation::MusicXml {
        part: Some(part.to_string()),
        measure: Some(measure.to_string()),
        element: element.to_string(),
        occurrence,
    }
}

fn source_occurrence(source: &SourceLocation) -> usize {
    match source {
        SourceLocation::MusicXml { occurrence, .. } => *occurrence,
        _ => 0,
    }
}

fn child_text(element: &XmlElement, name: &str) -> Option<String> {
    element.first_child(name).map(XmlElement::direct_text)
}

fn child_u32(element: &XmlElement, name: &str) -> Option<u32> {
    child_text(element, name)?.trim().parse().ok()
}

fn child_u16(element: &XmlElement, name: &str) -> Option<u16> {
    child_text(element, name)?.trim().parse().ok()
}

fn child_u8(element: &XmlElement, name: &str) -> Option<u8> {
    child_text(element, name)?.trim().parse().ok()
}

fn child_i16(element: &XmlElement, name: &str) -> Option<i16> {
    child_text(element, name)?.trim().parse().ok()
}

fn child_i8(element: &XmlElement, name: &str) -> Option<i8> {
    child_text(element, name)?.trim().parse().ok()
}

fn attr_u32(element: &XmlElement, name: &str) -> Option<u32> {
    element.attr(name)?.trim().parse().ok()
}

fn attr_u16(element: &XmlElement, name: &str) -> Option<u16> {
    element.attr(name)?.trim().parse().ok()
}

fn attr_u8(element: &XmlElement, name: &str) -> Option<u8> {
    element.attr(name)?.trim().parse().ok()
}

fn yes_or_value(value: Option<&str>) -> bool {
    value.is_some_and(|value| !matches!(value, "no" | "false" | "0"))
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn stable_u16(value: &str) -> u16 {
    value
        .bytes()
        .fold(0x811c_u16, |hash, byte| hash.wrapping_mul(251) ^ u16::from(byte))
}
