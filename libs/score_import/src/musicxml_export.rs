use crate::diagnostic::{ImportError, ImportReport, SourceLocation};
use crate::ids::exported_id;
use makepad_musicxml::{
    MusicXmlDocument, Score as XmlScore, ScorePartwise, XmlDeclaration, XmlElement, XmlNode,
    PARTWISE_DOCTYPE,
};
use makepad_score::model::*;
use makepad_score::symbol::{Articulation, Clef, DynamicMark, Ornament, Placement};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MusicXmlExportOptions {
    pub version: &'static str,
}

impl Default for MusicXmlExportOptions {
    fn default() -> Self {
        Self { version: "4.0" }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicXmlExportResult {
    pub document: MusicXmlDocument,
    pub report: ImportReport,
}

pub fn export_musicxml(
    score: &makepad_score::model::Score,
) -> Result<MusicXmlExportResult, ImportError> {
    export_musicxml_with_options(score, MusicXmlExportOptions::default())
}

pub fn export_musicxml_string(
    score: &makepad_score::model::Score,
) -> Result<(String, ImportReport), ImportError> {
    let result = export_musicxml(score)?;
    Ok((result.document.to_xml_string()?, result.report))
}

pub fn export_musicxml_with_options(
    score: &makepad_score::model::Score,
    options: MusicXmlExportOptions,
) -> Result<MusicXmlExportResult, ImportError> {
    let mut report = ImportReport::default();
    let mut root = XmlElement::new("score-partwise").with_attribute("version", options.version);
    if !score.title.is_empty() {
        root.push_element(text_element("movement-title", &score.title));
    }
    let mut part_list = XmlElement::new("part-list");
    for part in score.parts.values() {
        let mut score_part = XmlElement::new("score-part").with_attribute("id", exported_id(part.id));
        score_part.push_element(text_element("part-name", &part.name));
        part_list.push_element(score_part);
    }
    root.push_element(part_list);

    let measures = ordered_measures(score);
    let spanner_numbers: BTreeMap<_, _> = score
        .spanners
        .keys()
        .enumerate()
        .map(|(index, id)| (*id, u16::try_from(index + 1).unwrap_or(u16::MAX)))
        .collect();
    let note_spanners = note_spanner_marks(score, &spanner_numbers);
    let direction_spanners = direction_spanner_marks(score, &spanner_numbers);
    let tuplet_bounds = tuplet_bounds(score);

    for (part_index, part) in score.parts.values().enumerate() {
        let mut part_element = XmlElement::new("part").with_attribute("id", exported_id(part.id));
        for measure in &measures {
            part_element.push_element(export_measure(
                score,
                part,
                measure,
                &note_spanners,
                &direction_spanners,
                &tuplet_bounds,
                &mut report,
                part_index == 0,
            )?);
        }
        root.push_element(part_element);
    }

    if !score.annotations.is_empty()
        || !score.annotation_layers.is_empty()
        || !score.source_regions.is_empty()
    {
        report.ignored(
            "model.annotations",
            "practice annotations and source regions are native-model data and were not exported to MusicXML",
            SourceLocation::Model {
                entity: "annotations".to_string(),
            },
        );
    }
    if !score.part_views.is_empty() {
        report.ignored(
            "model.part-views",
            "part-view layout overrides were not exported",
            SourceLocation::Model {
                entity: "part_views".to_string(),
            },
        );
    }
    report_model_export_losses(score, &mut report);

    let document = MusicXmlDocument {
        declaration: XmlDeclaration::default(),
        doctype: PARTWISE_DOCTYPE.to_string(),
        before_score: Vec::new(),
        score: XmlScore::Partwise(ScorePartwise { element: root }),
        after_score: Vec::new(),
    };
    Ok(MusicXmlExportResult { document, report })
}

fn report_model_export_losses(score: &makepad_score::model::Score, report: &mut ImportReport) {
    if score.staves.values().any(|staff| {
        matches!(
            staff.kind,
            StaffKind::Tablature(_) | StaffKind::Percussion(_) | StaffKind::Unpitched
        )
    }) {
        report.approximated(
            "model.staff-instrument-detail",
            "tablature tuning and percussion/unpitched maps were reduced to staff and note pitch data",
            SourceLocation::Model {
                entity: "staves".to_string(),
            },
        );
    }
    if score.spanners.values().any(|spanner| {
        !matches!(
            spanner.kind,
            SpannerKind::Slur { .. }
                | SpannerKind::Hairpin { .. }
                | SpannerKind::Ottava { .. }
                | SpannerKind::Pedal
                | SpannerKind::Glissando { .. }
        )
    }) {
        report.ignored(
            "model.spanner",
            "native volta, lyric-extender, tie, or custom spanners without a direct MusicXML endpoint mapping were not exported",
            SourceLocation::Model {
                entity: "spanners".to_string(),
            },
        );
    }
    if score.voices.values().any(|voice| {
        voice.events.iter().any(|event| {
            event.articulations.iter().any(|articulation| {
                matches!(
                    articulation.kind,
                    Articulation::AccentStaccato
                        | Articulation::MarcatoStaccato
                        | Articulation::MarcatoTenuto
                )
            })
        })
    }) {
        report.approximated(
            "model.combined-articulation",
            "combined native articulations were exported using their closest MusicXML articulation",
            SourceLocation::Model {
                entity: "articulations".to_string(),
            },
        );
    }
}

#[derive(Clone)]
struct NoteSpannerMark {
    number: u16,
    family: &'static str,
    kind: SpannerKind,
    start: bool,
}

#[derive(Clone)]
struct DirectionSpannerMark {
    number: u16,
    kind: SpannerKind,
    start: bool,
    staff: StaffId,
    at: ScoreTime,
}

fn export_measure(
    score: &makepad_score::model::Score,
    part: &Part,
    measure: &Measure,
    note_spanners: &BTreeMap<NoteId, Vec<NoteSpannerMark>>,
    direction_spanners: &[DirectionSpannerMark],
    tuplet_bounds: &BTreeMap<SpannerId, (EventId, EventId)>,
    report: &mut ImportReport,
    include_measure_id: bool,
) -> Result<XmlElement, ImportError> {
    let divisions = measure_divisions(score, part, measure)?;
    let mut output = XmlElement::new("measure").with_attribute("number", measure.label.clone());
    if include_measure_id {
        output.set_attr("id", exported_id(measure.id));
    }
    output.push_element(export_start_attributes(score, part, measure, divisions, report)?);
    export_mid_measure_maps(score, part, measure, divisions, &mut output)?;
    export_map_directions(score, part, measure, divisions, &mut output, report)?;
    export_flow_directions(score, measure.ordinal, &mut output);
    let measure_end = measure.start.checked_add(measure.extent)?;
    let score_end = score
        .measures
        .values()
        .filter_map(|measure| measure.start.checked_add(measure.extent).ok())
        .max()
        .unwrap_or(measure_end);
    for mark in direction_spanners.iter().filter(|mark| {
        part.staves.contains(&mark.staff)
            && mark.at >= measure.start
            && (mark.at < measure_end || mark.at == measure_end && measure_end == score_end)
    }) {
        output.push_element(export_direction_spanner(mark, measure, divisions)?);
    }

    let voices = part
        .staves
        .iter()
        .filter_map(|staff| score.staves.get(staff))
        .flat_map(|staff| staff.voices.iter())
        .filter_map(|voice| score.voices.get(voice))
        .collect::<Vec<_>>();
    for (voice_index, voice) in voices.iter().enumerate() {
        if voice_index > 0 {
            output.push_element(backup_element(measure.extent, divisions)?);
        }
        let mut cursor = measure.start;
        let mut events = voice
            .events
            .iter()
            .filter(|event| {
                event.onset >= measure.start
                    && (event.onset < measure_end
                        || event.onset == measure_end
                            && matches!(event.kind, EventKind::Barline(_)))
            })
            .collect::<Vec<_>>();
        events.sort_by_key(|event| (event.onset, event.duration.is_some(), event.id));
        for event in events {
            if matches!(event.kind, EventKind::Barline(_)) {
                continue;
            }
            if event.onset > cursor {
                output.push_element(forward_element(event.onset.checked_sub(cursor)?, divisions)?);
                cursor = event.onset;
            }
            match &event.kind {
                EventKind::Chord(notes) => {
                    for (index, note) in notes.iter().enumerate() {
                        output.push_element(export_note(
                            score,
                            voice,
                            event,
                            note,
                            index > 0,
                            divisions,
                            note_spanners,
                            tuplet_bounds,
                            report,
                        )?);
                    }
                }
                EventKind::Rest => output.push_element(export_rest(voice, event, divisions)?),
                EventKind::Direction(direction) => {
                    output.push_element(export_direction(direction, voice))
                }
                EventKind::Clef(change) if event.onset != measure.start => output.push_element(
                    attributes_with_clef(change, staff_number(score, part, voice.staff)),
                ),
                EventKind::KeySignature(key) if event.onset != measure.start => {
                    output.push_element(attributes_with_key(key));
                }
                EventKind::TimeSignature(meter) if event.onset != measure.start => {
                    output.push_element(attributes_with_meter(meter));
                }
                EventKind::ChordSymbol(chord) => output.push_element(export_harmony(chord)),
                EventKind::FiguredBass(figured) => output.push_element(export_figured_bass(figured)),
                EventKind::Clef(_) | EventKind::KeySignature(_) | EventKind::TimeSignature(_) => {}
                EventKind::Barline(_) => {}
            }
            if event.grace.is_none() {
                if let Some(duration) = event.duration {
                    cursor = event.onset.checked_add(duration)?;
                }
            }
        }
        if voice_index + 1 < voices.len() && cursor < measure_end {
            output.push_element(forward_element(measure_end.checked_sub(cursor)?, divisions)?);
        }
    }
    for barline in export_barlines(score, part, measure) {
        output.push_element(barline);
    }
    report.imported("exported measures");
    Ok(output)
}

fn export_start_attributes(
    score: &makepad_score::model::Score,
    part: &Part,
    measure: &Measure,
    divisions: u32,
    report: &mut ImportReport,
) -> Result<XmlElement, ImportError> {
    let mut attributes = XmlElement::new("attributes");
    attributes.push_element(text_element("divisions", &divisions.to_string()));
    let first_measure = score
        .measures
        .values()
        .map(|measure| measure.start)
        .min()
        == Some(measure.start);
    let key_changes_here = score
        .maps
        .key
        .iter()
        .any(|change| change.at == measure.start && scope_applies(change.scope, part));
    if first_measure || key_changes_here {
        if let Some(key) = score.maps.key_at(measure.start, Some(part.id), None) {
            attributes.push_element(key_element(key));
        }
    }
    let meter_changes_here = score
        .maps
        .time_signature
        .iter()
        .any(|change| change.at == measure.start && scope_applies(change.scope, part));
    if first_measure || meter_changes_here {
        if let Some(meter) = score.maps.meter_at(measure.start, Some(part.id), None) {
            attributes.push_element(meter_element(meter));
        }
    }
    if part.staves.len() > 1 {
        attributes.push_element(text_element("staves", &part.staves.len().to_string()));
    }
    if part.transposition != Transposition::NONE {
        let mut transpose = XmlElement::new("transpose");
        transpose.push_element(text_element(
            "diatonic",
            &part.transposition.diatonic_steps.to_string(),
        ));
        transpose.push_element(text_element(
            "chromatic",
            &rational_decimal(part.transposition.chromatic_semitones.0, report, "transpose")?,
        ));
        if part.transposition.octave_shift != 0 {
            transpose.push_element(text_element(
                "octave-change",
                &part.transposition.octave_shift.to_string(),
            ));
        }
        attributes.push_element(transpose);
    }
    let mut seen = BTreeSet::new();
    for voice in part
        .staves
        .iter()
        .filter_map(|id| score.staves.get(id))
        .flat_map(|staff| staff.voices.iter())
        .filter_map(|id| score.voices.get(id))
    {
        for event in voice.events.iter().filter(|event| event.onset == measure.start) {
            if let EventKind::Clef(change) = event.kind {
                if seen.insert(voice.staff) {
                    attributes.push_element(clef_element(
                        &change,
                        staff_number(score, part, voice.staff),
                    ));
                }
            }
        }
    }
    Ok(attributes)
}

#[allow(clippy::too_many_arguments)]
fn export_note(
    score: &makepad_score::model::Score,
    voice: &Voice,
    event: &TimedEvent,
    note: &Note,
    chord: bool,
    divisions: u32,
    spanners: &BTreeMap<NoteId, Vec<NoteSpannerMark>>,
    tuplet_bounds: &BTreeMap<SpannerId, (EventId, EventId)>,
    report: &mut ImportReport,
) -> Result<XmlElement, ImportError> {
    let mut output = XmlElement::new("note").with_attribute("id", exported_id(note.id));
    if let Some(grace) = event.grace {
        let mut grace_element = XmlElement::new("grace");
        if grace.slash {
            grace_element.set_attr("slash", "yes");
        }
        if let Some(steal) = grace.steal {
            grace_element.set_attr(
                match grace.position {
                    GracePosition::BeforeBeat => "steal-time-previous",
                    GracePosition::AfterBeat => "steal-time-following",
                },
                rational_decimal(
                    steal.checked_mul(Rational::new(100, 1)?)?,
                    report,
                    "grace",
                )?,
            );
        }
        output.push_element(grace_element);
    }
    if chord {
        output.push_element(XmlElement::new("chord"));
    }
    if let Some(pitch) = note.written_pitch {
        let mut pitch_element = XmlElement::new("pitch");
        pitch_element.push_element(text_element("step", step_name(pitch.step)));
        if pitch.alter != Alter::NATURAL {
            pitch_element.push_element(text_element(
                "alter",
                &rational_decimal(pitch.alter.0, report, "pitch alter")?,
            ));
        }
        pitch_element.push_element(text_element("octave", &pitch.octave.to_string()));
        output.push_element(pitch_element);
    } else {
        output.push_element(XmlElement::new("unpitched"));
    }
    if event.grace.is_none() {
        if let Some(duration) = event.duration {
            output.push_element(text_element(
                "duration",
                &units_for_duration(duration, divisions)?.to_string(),
            ));
        }
    }
    if note.tie_from.is_some() {
        output.push_element(XmlElement::new("tie").with_attribute("type", "stop"));
    }
    if note.tie_to.is_some() {
        output.push_element(XmlElement::new("tie").with_attribute("type", "start"));
    }
    output.push_element(text_element("voice", &voice.number.to_string()));
    if let Some(duration) = event.duration {
        output.push_element(text_element("type", duration_type(duration)));
    }
    let staff_number = score
        .staves
        .get(&voice.staff)
        .and_then(|staff| score.parts.get(&staff.part))
        .map(|part| staff_number(score, part, voice.staff))
        .unwrap_or(1);
    output.push_element(text_element("staff", &staff_number.to_string()));
    if !chord {
        for beam in &event.beams {
            output.push_element(
                text_element("beam", beam_name(beam.state))
                    .with_attribute("number", beam.level.to_string()),
            );
        }
    }
    match &note.notehead {
        Notehead::Normal => {}
        Notehead::X => output.push_element(text_element("notehead", "x")),
        Notehead::Diamond => output.push_element(text_element("notehead", "diamond")),
        Notehead::Triangle => output.push_element(text_element("notehead", "triangle")),
        Notehead::Slash => output.push_element(text_element("notehead", "slash")),
        Notehead::Other(value) => output.push_element(text_element("notehead", value)),
    }
    if !chord {
        if let Some(tuplet) = event.tuplets.first() {
            let mut modification = XmlElement::new("time-modification");
            modification.push_element(text_element("actual-notes", &tuplet.actual.to_string()));
            modification.push_element(text_element("normal-notes", &tuplet.normal.to_string()));
            output.push_element(modification);
        }
    }
    let notations = export_notations(note, event, !chord, spanners, tuplet_bounds, report)?;
    if !notations.children.is_empty() {
        output.push_element(notations);
    }
    for lyric in score.lyrics.iter().filter(|lyric| lyric.note == note.id) {
        output.push_element(export_lyric(lyric));
    }
    Ok(output)
}

fn export_rest(
    voice: &Voice,
    event: &TimedEvent,
    divisions: u32,
) -> Result<XmlElement, ImportError> {
    let mut output = XmlElement::new("note").with_attribute("id", exported_id(event.id));
    output.push_element(XmlElement::new("rest"));
    if let Some(duration) = event.duration {
        output.push_element(text_element(
            "duration",
            &units_for_duration(duration, divisions)?.to_string(),
        ));
        output.push_element(text_element("type", duration_type(duration)));
    }
    output.push_element(text_element("voice", &voice.number.to_string()));
    Ok(output)
}

fn export_notations(
    note: &Note,
    event: &TimedEvent,
    include_event_notations: bool,
    spanners: &BTreeMap<NoteId, Vec<NoteSpannerMark>>,
    tuplet_bounds: &BTreeMap<SpannerId, (EventId, EventId)>,
    report: &mut ImportReport,
) -> Result<XmlElement, ImportError> {
    let mut notations = XmlElement::new("notations");
    if note.tie_from.is_some() {
        notations.push_element(XmlElement::new("tied").with_attribute("type", "stop"));
    }
    if note.tie_to.is_some() {
        notations.push_element(XmlElement::new("tied").with_attribute("type", "start"));
    }
    if let Some(marks) = spanners.get(&note.id) {
        for mark in marks {
            let mut element = XmlElement::new(mark.family)
                .with_attribute("number", mark.number.to_string())
                .with_attribute("type", if mark.start { "start" } else { "stop" });
            match &mark.kind {
                SpannerKind::Slur { placement: Some(Placement::Above) } => {
                    element.set_attr("placement", "above");
                }
                SpannerKind::Slur { placement: Some(Placement::Below) } => {
                    element.set_attr("placement", "below");
                }
                SpannerKind::Glissando { text: Some(text) } => {
                    element.children.push(XmlNode::Text(text.clone()));
                }
                _ => {}
            }
            notations.push_element(element);
        }
    }
    if include_event_notations {
        for tuplet in &event.tuplets {
            if let Some((first, last)) = tuplet_bounds.get(&tuplet.group) {
                if event.id == *first || event.id == *last {
                    let mut element = XmlElement::new("tuplet")
                        .with_attribute("number", tuplet.level.to_string())
                        .with_attribute("type", if event.id == *first { "start" } else { "stop" });
                    if tuplet.bracket {
                        element.set_attr("bracket", "yes");
                    }
                    notations.push_element(element);
                }
            }
        }
    }
    if include_event_notations && !event.articulations.is_empty() {
        let mut group = XmlElement::new("articulations");
        for articulation in &event.articulations {
            let mut element = XmlElement::new(articulation_name(articulation.kind));
            if let Some(placement) = articulation.placement {
                element.set_attr(
                    "placement",
                    if placement == Placement::Above { "above" } else { "below" },
                );
            }
            group.push_element(element);
        }
        notations.push_element(group);
    }
    if include_event_notations && !event.ornaments.is_empty() {
        let mut group = XmlElement::new("ornaments");
        for ornament in &event.ornaments {
            group.push_element(XmlElement::new(ornament_name(*ornament)));
        }
        notations.push_element(group);
    }
    if let Some(tab) = note.tab {
        let mut technical = XmlElement::new("technical");
        technical.push_element(text_element("string", &tab.string.to_string()));
        technical.push_element(text_element("fret", &tab.fret.to_string()));
        if tab.bend != Alter::NATURAL {
            let mut bend = XmlElement::new("bend");
            bend.push_element(text_element(
                "bend-alter",
                &rational_decimal(tab.bend.0, report, "tab bend")?,
            ));
            technical.push_element(bend);
        }
        notations.push_element(technical);
    }
    Ok(notations)
}

fn export_direction(direction: &DirectionEvent, voice: &Voice) -> XmlElement {
    let mut output = XmlElement::new("direction");
    if let Some(placement) = direction.placement {
        output.set_attr(
            "placement",
            if placement == Placement::Above { "above" } else { "below" },
        );
    }
    let mut kind = XmlElement::new("direction-type");
    match &direction.kind {
        DirectionKind::Words(text) | DirectionKind::TempoText(text) => {
            kind.push_element(text_element("words", text));
        }
        DirectionKind::Dynamic(dynamic) => {
            let mut dynamics = XmlElement::new("dynamics");
            dynamics.push_element(XmlElement::new(dynamic_name(*dynamic)));
            kind.push_element(dynamics);
        }
        DirectionKind::Rehearsal(text) => kind.push_element(text_element("rehearsal", text)),
        DirectionKind::Segno => kind.push_element(XmlElement::new("segno")),
        DirectionKind::Coda => kind.push_element(XmlElement::new("coda")),
        DirectionKind::Fine => kind.push_element(text_element("words", "Fine")),
        DirectionKind::DaCapo => kind.push_element(text_element("words", "D.C.")),
        DirectionKind::DalSegno => kind.push_element(text_element("words", "D.S.")),
        DirectionKind::ToCoda => kind.push_element(text_element("words", "To Coda")),
        DirectionKind::Breath => kind.push_element(text_element("words", "breath")),
    }
    output.push_element(kind);
    output.push_element(text_element("voice", &voice.number.to_string()));
    output
}

fn export_map_directions(
    score: &makepad_score::model::Score,
    part: &Part,
    measure: &Measure,
    divisions: u32,
    output: &mut XmlElement,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let end = measure.start.checked_add(measure.extent)?;
    for change in score.maps.tempo.iter().filter(|change| {
        change.at >= measure.start && change.at < end && scope_applies(change.scope, part)
    }) {
        let mut direction = XmlElement::new("direction");
        let mut direction_type = XmlElement::new("direction-type");
        let mut metronome = XmlElement::new("metronome");
        metronome.push_element(text_element("beat-unit", "quarter"));
        match &change.value {
            Tempo::Instant {
                quarters_per_minute,
            } => metronome.push_element(text_element(
                "per-minute",
                &rational_decimal(*quarters_per_minute, report, "tempo")?,
            )),
            Tempo::Ramp {
                from_quarters_per_minute,
                ..
            } => {
                metronome.push_element(text_element(
                    "per-minute",
                    &rational_decimal(*from_quarters_per_minute, report, "tempo ramp")?,
                ));
                report.approximated(
                    "model.tempo-ramp",
                    "MusicXML export wrote the ramp's initial tempo; continuous ramp playback is not portable",
                    SourceLocation::Model {
                        entity: "tempo ramp".to_string(),
                    },
                );
            }
        }
        direction_type.push_element(metronome);
        direction.push_element(direction_type);
        if change.at != measure.start {
            direction.push_element(text_element(
                "offset",
                &units_for_time(change.at.checked_sub(measure.start)?, divisions)?.to_string(),
            ));
        }
        output.push_element(direction);
    }
    Ok(())
}

fn export_mid_measure_maps(
    score: &makepad_score::model::Score,
    part: &Part,
    measure: &Measure,
    divisions: u32,
    output: &mut XmlElement,
) -> Result<(), ImportError> {
    let end = measure.start.checked_add(measure.extent)?;
    let mut changes = Vec::<(ScoreTime, XmlElement)>::new();
    for change in score.maps.key.iter().filter(|change| {
        change.at > measure.start && change.at < end && scope_applies(change.scope, part)
    }) {
        changes.push((change.at, attributes_with_key(&change.value)));
    }
    for change in score.maps.time_signature.iter().filter(|change| {
        change.at > measure.start && change.at < end && scope_applies(change.scope, part)
    }) {
        changes.push((change.at, attributes_with_meter(&change.value)));
    }
    changes.sort_by_key(|(at, _)| *at);
    for (at, attributes) in changes {
        let offset = at.checked_sub(measure.start)?;
        output.push_element(forward_element(offset, divisions)?);
        output.push_element(attributes);
        output.push_element(backup_time_element(offset, divisions)?);
    }
    Ok(())
}

fn export_flow_directions(score: &makepad_score::model::Score, ordinal: u32, output: &mut XmlElement) {
    for marker in score.flow.markers.iter().filter(|marker| marker.at == ordinal) {
        let mut direction = XmlElement::new("direction");
        let mut direction_type = XmlElement::new("direction-type");
        direction_type.push_element(XmlElement::new(match marker.kind {
            MarkerKind::Segno => "segno",
            MarkerKind::Coda => "coda",
        }));
        direction.push_element(direction_type);
        output.push_element(direction);
    }
    for jump in score.flow.jumps.iter().filter(|jump| jump.at == ordinal) {
        let mut sound = XmlElement::new("sound");
        match jump.kind {
            JumpKind::DaCapo => sound.set_attr("dacapo", "yes"),
            JumpKind::DalSegno => sound.set_attr("dalsegno", "segno"),
            JumpKind::ToCoda => sound.set_attr("tocoda", "coda"),
            JumpKind::Fine => sound.set_attr("fine", "yes"),
        }
        output.push_element(sound);
    }
}

fn export_direction_spanner(
    mark: &DirectionSpannerMark,
    measure: &Measure,
    divisions: u32,
) -> Result<XmlElement, ImportError> {
    let mut output = XmlElement::new("direction");
    let mut direction_type = XmlElement::new("direction-type");
    let mut element = match &mark.kind {
        SpannerKind::Hairpin { crescendo, niente } => {
            let mut element = XmlElement::new("wedge").with_attribute(
                "type",
                if mark.start {
                    if *crescendo { "crescendo" } else { "diminuendo" }
                } else {
                    "stop"
                },
            );
            if *niente {
                element.set_attr("niente", "yes");
            }
            element
        }
        SpannerKind::Ottava { octaves } => XmlElement::new("octave-shift")
            .with_attribute(
                "type",
                if mark.start {
                    if *octaves < 0 { "down" } else { "up" }
                } else {
                    "stop"
                },
            )
            .with_attribute("size", (u16::from(octaves.unsigned_abs()) * 7 + 1).to_string()),
        SpannerKind::Pedal => XmlElement::new("pedal")
            .with_attribute("type", if mark.start { "start" } else { "stop" }),
        _ => XmlElement::new("bracket")
            .with_attribute("type", if mark.start { "start" } else { "stop" }),
    };
    element.set_attr("number", mark.number.to_string());
    direction_type.push_element(element);
    output.push_element(direction_type);
    if mark.at != measure.start {
        output.push_element(text_element(
            "offset",
            &units_for_time(mark.at.checked_sub(measure.start)?, divisions)?.to_string(),
        ));
    }
    Ok(output)
}

fn export_barlines(
    score: &makepad_score::model::Score,
    part: &Part,
    measure: &Measure,
) -> Vec<XmlElement> {
    let mut output = Vec::new();
    let starts_repeat = score
        .flow
        .repeats
        .iter()
        .any(|repeat| repeat.start == measure.ordinal);
    let ending_starts = score
        .flow
        .voltas
        .iter()
        .filter(|volta| volta.start == measure.ordinal)
        .collect::<Vec<_>>();
    if starts_repeat || !ending_starts.is_empty() {
        let mut barline = XmlElement::new("barline").with_attribute("location", "left");
        for volta in ending_starts {
            let passes = passes_text(&volta.passes);
            barline.push_element(
                text_element("ending", &passes)
                    .with_attribute("number", passes)
                    .with_attribute("type", "start"),
            );
        }
        if starts_repeat {
            barline.push_element(
                XmlElement::new("repeat").with_attribute("direction", "forward"),
            );
        }
        output.push(barline);
    }
    let repeat = score
        .flow
        .repeats
        .iter()
        .find(|repeat| repeat.end == measure.ordinal);
    let ending_ends = score
        .flow
        .voltas
        .iter()
        .filter(|volta| volta.end == measure.ordinal)
        .collect::<Vec<_>>();
    let semantic = part
        .staves
        .iter()
        .filter_map(|staff| score.staves.get(staff))
        .flat_map(|staff| staff.voices.iter())
        .filter_map(|voice| score.voices.get(voice))
        .flat_map(|voice| voice.events.iter())
        .find_map(|event| match event.kind {
            EventKind::Barline(barline) if event.onset >= measure.start => Some(barline),
            _ => None,
        });
    if repeat.is_some() || !ending_ends.is_empty() || semantic.is_some() {
        let mut barline = XmlElement::new("barline").with_attribute("location", "right");
        if let Some(semantic) = semantic {
            barline.push_element(text_element("bar-style", barline_style_name(semantic.style)));
        }
        for volta in ending_ends {
            barline.push_element(
                XmlElement::new("ending")
                    .with_attribute("number", passes_text(&volta.passes))
                    .with_attribute("type", "stop"),
            );
        }
        if let Some(repeat) = repeat {
            barline.push_element(
                XmlElement::new("repeat")
                    .with_attribute("direction", "backward")
                    .with_attribute("times", repeat.times.to_string()),
            );
        }
        output.push(barline);
    }
    output
}

fn note_spanner_marks(
    score: &makepad_score::model::Score,
    numbers: &BTreeMap<SpannerId, u16>,
) -> BTreeMap<NoteId, Vec<NoteSpannerMark>> {
    let mut output = BTreeMap::<_, Vec<_>>::new();
    for spanner in score.spanners.values() {
        let family = match spanner.kind {
            SpannerKind::Slur { .. } => Some("slur"),
            SpannerKind::Glissando { .. } => Some("glissando"),
            _ => None,
        };
        let Some(family) = family else { continue };
        if let SpannerEndpoint::Note(note) = spanner.start {
            output.entry(note).or_default().push(NoteSpannerMark {
                number: numbers[&spanner.id],
                family,
                kind: spanner.kind.clone(),
                start: true,
            });
        }
        if let SpannerEndpoint::Note(note) = spanner.end {
            output.entry(note).or_default().push(NoteSpannerMark {
                number: numbers[&spanner.id],
                family,
                kind: spanner.kind.clone(),
                start: false,
            });
        }
    }
    output
}

fn direction_spanner_marks(
    score: &makepad_score::model::Score,
    numbers: &BTreeMap<SpannerId, u16>,
) -> Vec<DirectionSpannerMark> {
    let mut output = Vec::new();
    for spanner in score.spanners.values() {
        if !matches!(
            spanner.kind,
            SpannerKind::Hairpin { .. } | SpannerKind::Ottava { .. } | SpannerKind::Pedal
        ) {
            continue;
        }
        for (endpoint, start) in [(spanner.start, true), (spanner.end, false)] {
            if let SpannerEndpoint::StaffTime { staff, at } = endpoint {
                output.push(DirectionSpannerMark {
                    number: numbers[&spanner.id],
                    kind: spanner.kind.clone(),
                    start,
                    staff,
                    at,
                });
            }
        }
    }
    output
}

fn tuplet_bounds(score: &makepad_score::model::Score) -> BTreeMap<SpannerId, (EventId, EventId)> {
    let mut output = BTreeMap::new();
    for event in score.voices.values().flat_map(|voice| voice.events.iter()) {
        for tuplet in &event.tuplets {
            output
                .entry(tuplet.group)
                .and_modify(|bounds: &mut (EventId, EventId)| bounds.1 = event.id)
                .or_insert((event.id, event.id));
        }
    }
    output
}

fn export_harmony(chord: &ChordSymbol) -> XmlElement {
    let mut output = XmlElement::new("harmony");
    let mut root = XmlElement::new("root");
    root.push_element(text_element("root-step", step_name(chord.root.step)));
    if chord.root.alter != Alter::NATURAL {
        root.push_element(text_element(
            "root-alter",
            &simple_decimal(chord.root.alter.0),
        ));
    }
    output.push_element(root);
    output.push_element(
        text_element("kind", chord_quality_name(&chord.quality))
            .with_attribute("text", chord.original_text.clone()),
    );
    if let Some(bass) = chord.bass {
        let mut element = XmlElement::new("bass");
        element.push_element(text_element("bass-step", step_name(bass.step)));
        if bass.alter != Alter::NATURAL {
            element.push_element(text_element("bass-alter", &simple_decimal(bass.alter.0)));
        }
        output.push_element(element);
    }
    for degree in &chord.degrees {
        let mut element = XmlElement::new("degree");
        element.push_element(text_element("degree-value", &degree.value.to_string()));
        element.push_element(text_element("degree-alter", &simple_decimal(degree.alter.0)));
        element.push_element(text_element(
            "degree-type",
            match degree.operation {
                DegreeOperation::Add => "add",
                DegreeOperation::Alter => "alter",
                DegreeOperation::Subtract => "subtract",
            },
        ));
        output.push_element(element);
    }
    output
}

fn export_figured_bass(figured: &FiguredBass) -> XmlElement {
    let mut output = XmlElement::new("figured-bass");
    for figure in &figured.figures {
        let mut element = XmlElement::new("figure");
        if let Some(prefix) = &figure.prefix {
            element.push_element(text_element("prefix", prefix));
        }
        element.push_element(text_element("figure-number", &figure.interval.to_string()));
        if let Some(suffix) = &figure.suffix {
            element.push_element(text_element("suffix", suffix));
        }
        output.push_element(element);
    }
    output
}

fn export_lyric(lyric: &LyricSyllable) -> XmlElement {
    let mut output = XmlElement::new("lyric").with_attribute("number", lyric.verse.to_string());
    output.push_element(text_element(
        "syllabic",
        match lyric.role {
            SyllabicRole::Single => "single",
            SyllabicRole::Begin => "begin",
            SyllabicRole::Middle => "middle",
            SyllabicRole::End => "end",
        },
    ));
    output.push_element(text_element("text", &lyric.text));
    if let Some(elision) = &lyric.elision {
        output.push_element(text_element("elision", elision));
    }
    if lyric.melisma_to.is_some() {
        output.push_element(XmlElement::new("extend").with_attribute("type", "start"));
    }
    output
}

fn measure_divisions(
    score: &makepad_score::model::Score,
    part: &Part,
    measure: &Measure,
) -> Result<u32, ImportError> {
    let end = measure.start.checked_add(measure.extent)?;
    let mut divisions = quarter_denominator(measure.extent.0)?;
    for voice in part
        .staves
        .iter()
        .filter_map(|staff| score.staves.get(staff))
        .flat_map(|staff| staff.voices.iter())
        .filter_map(|voice| score.voices.get(voice))
    {
        for event in voice
            .events
            .iter()
            .filter(|event| event.onset >= measure.start && event.onset <= end)
        {
            divisions = checked_lcm_u32(
                divisions,
                quarter_denominator(event.onset.checked_sub(measure.start)?.0)?,
            )?;
            if let Some(duration) = event.duration {
                divisions = checked_lcm_u32(divisions, quarter_denominator(duration.0)?)?;
            }
        }
    }
    Ok(divisions.max(1))
}

fn quarter_denominator(value: Rational) -> Result<u32, ImportError> {
    let quarters = value.checked_mul(Rational::new(4, 1)?)?;
    u32::try_from(quarters.denominator())
        .map_err(|_| ImportError::Unsupported("MusicXML divisions exceed u32".to_string()))
}

fn checked_lcm_u32(left: u32, right: u32) -> Result<u32, ImportError> {
    let gcd = gcd_u64(u64::from(left), u64::from(right));
    let value = u64::from(left)
        .checked_div(gcd)
        .and_then(|value| value.checked_mul(u64::from(right)))
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| ImportError::Unsupported("MusicXML divisions overflow".to_string()))?;
    Ok(value)
}

fn units_for_duration(duration: Duration, divisions: u32) -> Result<u32, ImportError> {
    units_for_rational(duration.0, divisions)
}

fn units_for_time(time: ScoreTime, divisions: u32) -> Result<u32, ImportError> {
    units_for_rational(time.0, divisions)
}

fn units_for_rational(value: Rational, divisions: u32) -> Result<u32, ImportError> {
    let units = value.checked_mul(Rational::new(i64::from(divisions) * 4, 1)?)?;
    if units.denominator() != 1 || units.numerator() < 0 {
        return Err(ImportError::Unsupported(
            "selected MusicXML divisions do not exactly represent a score time".to_string(),
        ));
    }
    u32::try_from(units.numerator())
        .map_err(|_| ImportError::Unsupported("MusicXML duration exceeds u32".to_string()))
}

fn forward_element(duration: ScoreTime, divisions: u32) -> Result<XmlElement, ImportError> {
    let mut forward = XmlElement::new("forward");
    forward.push_element(text_element(
        "duration",
        &units_for_time(duration, divisions)?.to_string(),
    ));
    Ok(forward)
}

fn backup_element(duration: Duration, divisions: u32) -> Result<XmlElement, ImportError> {
    let mut backup = XmlElement::new("backup");
    backup.push_element(text_element(
        "duration",
        &units_for_duration(duration, divisions)?.to_string(),
    ));
    Ok(backup)
}

fn backup_time_element(duration: ScoreTime, divisions: u32) -> Result<XmlElement, ImportError> {
    let mut backup = XmlElement::new("backup");
    backup.push_element(text_element(
        "duration",
        &units_for_time(duration, divisions)?.to_string(),
    ));
    Ok(backup)
}

fn ordered_measures(score: &makepad_score::model::Score) -> Vec<&Measure> {
    let mut measures = score.measures.values().collect::<Vec<_>>();
    measures.sort_by_key(|measure| (measure.ordinal, measure.start, measure.id));
    measures
}

fn attributes_with_clef(change: &ClefChange, number: u32) -> XmlElement {
    let mut attributes = XmlElement::new("attributes");
    attributes.push_element(clef_element(change, number));
    attributes
}

fn attributes_with_key(key: &KeySignature) -> XmlElement {
    let mut attributes = XmlElement::new("attributes");
    attributes.push_element(key_element(key));
    attributes
}

fn attributes_with_meter(meter: &Meter) -> XmlElement {
    let mut attributes = XmlElement::new("attributes");
    attributes.push_element(meter_element(meter));
    attributes
}

fn key_element(key: &KeySignature) -> XmlElement {
    let mut output = XmlElement::new("key");
    output.push_element(text_element("fifths", &key.fifths.to_string()));
    for (step, alter) in &key.custom {
        output.push_element(text_element("key-step", step_name(*step)));
        output.push_element(text_element("key-alter", &simple_decimal(alter.0)));
    }
    output
}

fn meter_element(meter: &Meter) -> XmlElement {
    let mut output = XmlElement::new("time");
    match meter {
        Meter::Measured { groups, unit } => {
            output.push_element(text_element(
                "beats",
                &groups.iter().map(u16::to_string).collect::<Vec<_>>().join("+"),
            ));
            output.push_element(text_element("beat-type", &unit.to_string()));
        }
        Meter::Free => output.push_element(XmlElement::new("senza-misura")),
    }
    output
}

fn clef_element(change: &ClefChange, number: u32) -> XmlElement {
    let (sign, octave) = match change.clef {
        Clef::G => ("G", 0),
        Clef::G8va => ("G", 1),
        Clef::G8vb => ("G", -1),
        Clef::G15ma => ("G", 2),
        Clef::G15mb => ("G", -2),
        Clef::F => ("F", 0),
        Clef::F8va => ("F", 1),
        Clef::F8vb => ("F", -1),
        Clef::F15ma => ("F", 2),
        Clef::F15mb => ("F", -2),
        Clef::C => ("C", 0),
        Clef::Percussion | Clef::PercussionAlternate => ("percussion", 0),
        Clef::Tab4String | Clef::Tab6String => ("TAB", 0),
    };
    let mut output = XmlElement::new("clef").with_attribute("number", number.to_string());
    output.push_element(text_element("sign", sign));
    output.push_element(text_element("line", &change.line.to_string()));
    if octave != 0 {
        output.push_element(text_element("clef-octave-change", &octave.to_string()));
    }
    output
}

fn text_element(name: &str, text: &str) -> XmlElement {
    let mut element = XmlElement::new(name);
    element.children.push(XmlNode::Text(text.to_string()));
    element
}

fn rational_decimal(
    value: Rational,
    report: &mut ImportReport,
    feature: &str,
) -> Result<String, ImportError> {
    let mut denominator = value.denominator();
    let mut twos = 0_u32;
    let mut fives = 0_u32;
    while denominator % 2 == 0 {
        denominator /= 2;
        twos += 1;
    }
    while denominator % 5 == 0 {
        denominator /= 5;
        fives += 1;
    }
    if denominator != 1 {
        report.approximated(
            "musicxml.non-terminating-decimal",
            format!("{feature} value {value} was rounded to nine decimal places"),
            SourceLocation::Model {
                entity: feature.to_string(),
            },
        );
        let scaled = value.checked_mul(Rational::new(1_000_000_000, 1)?)?;
        return Ok(format_scaled(
            div_round(scaled.numerator(), scaled.denominator()),
            9,
        ));
    }
    let scale = twos.max(fives);
    let scaled = i128::from(value.numerator())
        * 5_i128.pow(scale - twos)
        * 2_i128.pow(scale - fives);
    Ok(format_scaled(scaled, scale))
}

fn simple_decimal(value: Rational) -> String {
    let mut denominator = value.denominator();
    let mut twos = 0_u32;
    let mut fives = 0_u32;
    while denominator % 2 == 0 {
        denominator /= 2;
        twos += 1;
    }
    while denominator % 5 == 0 {
        denominator /= 5;
        fives += 1;
    }
    if denominator == 1 {
        let scale = twos.max(fives);
        let scaled = i128::from(value.numerator())
            * 5_i128.pow(scale - twos)
            * 2_i128.pow(scale - fives);
        format_scaled(scaled, scale)
    } else {
        let scaled_numerator = value.numerator().saturating_mul(1_000_000);
        format_scaled(div_round(scaled_numerator, value.denominator()), 6)
    }
}

fn format_scaled(value: i128, scale: u32) -> String {
    if scale == 0 {
        return value.to_string();
    }
    let negative = value < 0;
    let mut digits = value.unsigned_abs().to_string();
    while digits.len() <= scale as usize {
        digits.insert(0, '0');
    }
    let split = digits.len() - scale as usize;
    digits.insert(split, '.');
    while digits.ends_with('0') {
        digits.pop();
    }
    if digits.ends_with('.') {
        digits.pop();
    }
    if negative { format!("-{digits}") } else { digits }
}

fn div_round(numerator: i64, denominator: u64) -> i128 {
    let numerator = i128::from(numerator);
    let denominator = i128::from(denominator);
    if numerator >= 0 {
        (numerator + denominator / 2) / denominator
    } else {
        (numerator - denominator / 2) / denominator
    }
}

fn gcd_u64(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left.max(1)
}

fn staff_number(score: &makepad_score::model::Score, part: &Part, staff: StaffId) -> u32 {
    part.staves
        .iter()
        .position(|id| *id == staff)
        .map(|index| index as u32 + 1)
        .or_else(|| score.staves.get(&staff).map(|_| 1))
        .unwrap_or(1)
}

fn scope_applies(scope: MapScope, part: &Part) -> bool {
    match scope {
        MapScope::Global => true,
        MapScope::Part(id) => id == part.id,
        MapScope::Staff(id) => part.staves.contains(&id),
    }
}

fn duration_type(duration: Duration) -> &'static str {
    let value = duration.0;
    if value >= Rational::new(1, 1).expect("constant") {
        "whole"
    } else if value >= Rational::new(1, 2).expect("constant") {
        "half"
    } else if value >= Rational::new(1, 4).expect("constant") {
        "quarter"
    } else if value >= Rational::new(1, 8).expect("constant") {
        "eighth"
    } else if value >= Rational::new(1, 16).expect("constant") {
        "16th"
    } else if value >= Rational::new(1, 32).expect("constant") {
        "32nd"
    } else {
        "64th"
    }
}

fn step_name(step: Step) -> &'static str {
    match step {
        Step::C => "C",
        Step::D => "D",
        Step::E => "E",
        Step::F => "F",
        Step::G => "G",
        Step::A => "A",
        Step::B => "B",
    }
}

fn beam_name(state: BeamState) -> &'static str {
    match state {
        BeamState::Begin => "begin",
        BeamState::Continue => "continue",
        BeamState::End => "end",
        BeamState::ForwardHook => "forward hook",
        BeamState::BackwardHook => "backward hook",
    }
}

fn articulation_name(value: Articulation) -> &'static str {
    match value {
        Articulation::Accent => "accent",
        Articulation::Staccato => "staccato",
        Articulation::Tenuto => "tenuto",
        Articulation::Staccatissimo => "staccatissimo",
        Articulation::Marcato => "strong-accent",
        Articulation::LaissezVibrer => "other-articulation",
        Articulation::Stress => "stress",
        Articulation::SoftAccent => "soft-accent",
        Articulation::AccentStaccato => "accent",
        Articulation::TenutoStaccato => "detached-legato",
        Articulation::MarcatoStaccato | Articulation::MarcatoTenuto => "strong-accent",
    }
}

fn ornament_name(value: Ornament) -> &'static str {
    match value {
        Ornament::Trill => "trill-mark",
        Ornament::Turn => "turn",
        Ornament::InvertedTurn => "inverted-turn",
        Ornament::TurnWithSlash => "turn",
        Ornament::Mordent => "mordent",
        Ornament::ShortTrill => "inverted-mordent",
        Ornament::Tremblement => "other-ornament",
        Ornament::Schleifer => "schleifer",
    }
}

fn dynamic_name(value: DynamicMark) -> &'static str {
    match value {
        DynamicMark::Piano => "p",
        DynamicMark::Pianissimo => "pp",
        DynamicMark::Pianississimo => "ppp",
        DynamicMark::Pianissississimo => "pppp",
        DynamicMark::MezzoPiano => "mp",
        DynamicMark::MezzoForte => "mf",
        DynamicMark::Forte => "f",
        DynamicMark::Fortissimo => "ff",
        DynamicMark::Fortississimo => "fff",
        DynamicMark::Fortissississimo => "ffff",
        DynamicMark::FortePiano => "fp",
        DynamicMark::Sforzando => "sfz",
        DynamicMark::SforzandoPiano => "sfp",
        DynamicMark::Sforzato => "fz",
        DynamicMark::Rinforzando => "rfz",
        DynamicMark::Niente => "n",
        DynamicMark::Mezzo | DynamicMark::Z => "other-dynamics",
    }
}

fn chord_quality_name(value: &ChordQuality) -> &str {
    match value {
        ChordQuality::Major => "major",
        ChordQuality::Minor => "minor",
        ChordQuality::Augmented => "augmented",
        ChordQuality::Diminished => "diminished",
        ChordQuality::Dominant => "dominant",
        ChordQuality::Suspended => "suspended-fourth",
        ChordQuality::Other(value) => value,
    }
}

fn barline_style_name(value: BarlineStyle) -> &'static str {
    match value {
        BarlineStyle::Regular => "regular",
        BarlineStyle::Dotted => "dotted",
        BarlineStyle::Dashed => "dashed",
        BarlineStyle::Heavy => "heavy",
        BarlineStyle::Double => "light-light",
        BarlineStyle::Final => "light-heavy",
        BarlineStyle::Invisible => "none",
    }
}

fn passes_text(passes: &[u16]) -> String {
    passes.iter().map(u16::to_string).collect::<Vec<_>>().join(",")
}
