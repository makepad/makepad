use super::*;
use makepad_midi_file::{
    ChannelEvent, ChannelMessage, Division as MidiDivision, EventKind as MidiEventKind, Format,
    Header, KeySignature as MidiKeySignature, MetaEvent, MidiFile, TimeSignature, Track,
    TrackEvent,
};
use makepad_score::model::*;
use std::collections::BTreeSet;

const COMPREHENSIVE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0" id="score-one">
  <movement-title>Importer Fixture</movement-title>
  <part-list><score-part id="P1"><part-name>B-flat Clarinet</part-name></score-part></part-list>
  <part id="P1">
    <measure number="1" id="m1">
      <attributes>
        <divisions>3</divisions><key><fifths>1</fifths></key>
        <time><beats>4</beats><beat-type>4</beat-type></time><staves>2</staves>
        <clef number="1"><sign>G</sign><line>2</line></clef>
        <clef number="2"><sign>F</sign><line>4</line></clef>
        <transpose><diatonic>-1</diatonic><chromatic>-2</chromatic></transpose>
      </attributes>
      <barline location="left"><repeat direction="forward"/></barline>
      <note id="n1"><pitch><step>C</step><octave>4</octave></pitch><duration>3</duration>
        <tie type="start"/><voice>1</voice><type>quarter</type><staff>1</staff>
        <notations><tied type="start"/><slur type="start" number="2"/>
          <articulations><accent/></articulations><ornaments><trill-mark/></ornaments></notations>
        <lyric number="1"><syllabic>begin</syllabic><text>sing</text><extend type="start"/></lyric>
      </note>
      <note id="n2"><chord/><pitch><step>E</step><octave>4</octave></pitch><duration>3</duration><voice>1</voice><staff>1</staff></note>
      <attributes><clef number="1"><sign>C</sign><line>3</line></clef></attributes>
      <direction placement="below"><direction-type><dynamics><mf/></dynamics></direction-type><staff>1</staff></direction>
      <direction><direction-type><wedge type="crescendo" number="4"/></direction-type><staff>1</staff></direction>
      <harmony><root><root-step>G</root-step></root><kind text="G7">dominant</kind></harmony>
      <note id="t1"><pitch><step>D</step><octave>4</octave></pitch><duration>1</duration><voice>1</voice><staff>1</staff>
        <time-modification><actual-notes>3</actual-notes><normal-notes>2</normal-notes></time-modification>
        <notations><tuplet type="start" number="1"/></notations></note>
      <note id="t2"><pitch><step>E</step><octave>4</octave></pitch><duration>1</duration><voice>1</voice><staff>1</staff>
        <time-modification><actual-notes>3</actual-notes><normal-notes>2</normal-notes></time-modification></note>
      <note id="t3"><pitch><step>F</step><alter>1</alter><octave>4</octave></pitch><duration>1</duration><voice>1</voice><staff>1</staff>
        <time-modification><actual-notes>3</actual-notes><normal-notes>2</normal-notes></time-modification>
        <notations><tuplet type="stop" number="1"/></notations></note>
      <backup><duration>6</duration></backup>
      <note id="r1"><rest/><duration>6</duration><voice>2</voice><staff>2</staff></note>
    </measure>
    <measure number="2" id="m2">
      <attributes><divisions>10</divisions></attributes>
      <barline location="left"><ending number="1" type="start">1.</ending></barline>
      <direction><direction-type><wedge type="stop" number="4"/></direction-type><staff>1</staff></direction>
      <note id="n3"><pitch><step>C</step><octave>4</octave></pitch><duration>10</duration>
        <tie type="stop"/><voice>1</voice><staff>1</staff>
        <notations><tied type="stop"/><slur type="stop" number="2"/></notations>
        <lyric number="1"><syllabic>end</syllabic><text>ing</text><extend type="stop"/></lyric>
      </note>
      <note><rest/><duration>30</duration><voice>1</voice><staff>1</staff></note>
      <barline location="right"><ending number="1" type="stop"/><repeat direction="backward" times="2"/></barline>
    </measure>
    <measure number="3" id="m3">
      <barline location="left"><ending number="2" type="start">2.</ending></barline>
      <note id="n4"><pitch><step>G</step><octave>4</octave></pitch><duration>40</duration><voice>1</voice><staff>1</staff></note>
      <barline location="right"><ending number="2" type="stop"/></barline>
      <future-musical-extension>retained</future-musical-extension>
    </measure>
  </part>
</score-partwise>"#;

fn pitched_events(score: &makepad_score::model::Score) -> Vec<&TimedEvent> {
    score
        .voices
        .values()
        .flat_map(|voice| voice.events.iter())
        .filter(|event| matches!(event.kind, EventKind::Chord(_)))
        .collect()
}

#[test]
fn musicxml_maps_exact_time_chords_voices_tuplets_and_clef_changes() {
    let imported = import_musicxml_str(COMPREHENSIVE_XML).unwrap();
    let score = &imported.score;
    assert_eq!(score.parts.len(), 1);
    assert_eq!(score.staves.len(), 2);
    assert!(score.voices.len() >= 2);

    let n1_event = pitched_events(score)
        .into_iter()
        .find(|event| event.chord_notes().iter().any(|note| {
            note.written_pitch.is_some_and(|pitch| pitch.step == Step::C)
                && event.onset == ScoreTime::ZERO
        }))
        .unwrap();
    assert_eq!(n1_event.duration, Some(Duration::new(1, 4).unwrap()));
    assert_eq!(n1_event.chord_notes().len(), 2);

    let triplets = score
        .voices
        .values()
        .flat_map(|voice| voice.events.iter())
        .filter(|event| !event.tuplets.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(triplets.len(), 3);
    assert!(triplets
        .iter()
        .all(|event| event.duration == Some(Duration::new(1, 12).unwrap())));
    assert!(triplets
        .iter()
        .all(|event| event.tuplets[0].group == triplets[0].tuplets[0].group));

    let clefs = score
        .voices
        .values()
        .flat_map(|voice| voice.events.iter())
        .filter(|event| matches!(event.kind, EventKind::Clef(_)))
        .collect::<Vec<_>>();
    assert!(clefs.iter().any(|event| {
        event.onset == ScoreTime::new(1, 4).unwrap()
            && matches!(event.kind, EventKind::Clef(ClefChange { clef: makepad_score::symbol::Clef::C, .. }))
    }));
    assert_eq!(score.measures.values().nth(1).unwrap().extent, Duration::new(1, 1).unwrap());
}

#[test]
fn musicxml_resolves_ties_slurs_hairpins_transposition_lyrics_and_flow() {
    let imported = import_musicxml_str(COMPREHENSIVE_XML).unwrap();
    let score = &imported.score;
    let part = score.parts.values().next().unwrap();
    assert_eq!(part.transposition.diatonic_steps, -1);
    assert_eq!(part.transposition.chromatic_semitones, Alter::new(-2, 1).unwrap());
    let tied = score
        .voices
        .values()
        .flat_map(|voice| voice.events.iter())
        .flat_map(TimedEvent::chord_notes)
        .find(|note| note.tie_to.is_some())
        .unwrap();
    let projection = score.pitch_projection(part.id, tied.id, true).unwrap().unwrap();
    assert_eq!(projection.written.step, Step::C);
    assert_eq!(projection.displayed.step, Step::B);
    assert_eq!(projection.displayed.alter, Alter::new(-1, 1).unwrap());
    assert!(score
        .spanners
        .values()
        .any(|spanner| matches!(spanner.kind, SpannerKind::Slur { .. })));
    assert!(score
        .spanners
        .values()
        .any(|spanner| matches!(spanner.kind, SpannerKind::Hairpin { crescendo: true, .. })));
    assert_eq!(score.lyrics.len(), 2);
    assert!(score.lyrics[0].melisma_to.is_some());
    let visits = score.flow.unfold(score, 20).unwrap();
    let ordinals = visits
        .iter()
        .map(|visit| score.measures[&visit.source_measure].ordinal)
        .collect::<Vec<_>>();
    assert_eq!(ordinals, vec![0, 1, 0, 2]);
}

#[test]
fn musicxml_reports_every_known_loss_and_unknown_content() {
    let xml = COMPREHENSIVE_XML.replace(
        "<note id=\"n4\">",
        "<note id=\"n4\"><cue/><accidental cautionary=\"yes\">natural</accidental>",
    );
    let imported = import_musicxml_str(&xml).unwrap();
    let codes = imported
        .report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<BTreeSet<_>>();
    assert!(codes.contains("musicxml.cue"));
    assert!(codes.contains("musicxml.accidental-display"));
    assert!(codes.contains("musicxml.unknown-element"));
    assert!(codes.contains("musicxml.measure-item"));
    assert!(imported.report.stats.approximated >= 2);
    assert!(imported.report.stats.ignored >= 2);
}

#[derive(Debug, Eq, PartialEq)]
struct MusicalSnapshot {
    measures: Vec<(u32, Duration)>,
    notes: Vec<(ScoreTime, Option<Duration>, Vec<Option<Pitch>>)>,
    tuplets: usize,
    ties: usize,
    slurs: usize,
    lyrics: Vec<(u16, String, SyllabicRole, bool)>,
    repeats: Vec<RepeatSection>,
    voltas: Vec<VoltaEnding>,
    transpositions: Vec<Transposition>,
    maps: GlobalMaps,
    articulations: usize,
    ornaments: usize,
    hairpins: usize,
    directions: Vec<String>,
    harmonies: Vec<String>,
}

fn musical_snapshot(score: &makepad_score::model::Score) -> MusicalSnapshot {
    let mut measures = score
        .measures
        .values()
        .map(|measure| (measure.ordinal, measure.extent))
        .collect::<Vec<_>>();
    measures.sort_by_key(|item| item.0);
    let mut events = score
        .voices
        .values()
        .flat_map(|voice| voice.events.iter())
        .filter_map(|event| match &event.kind {
            EventKind::Chord(notes) => Some((
                event.onset,
                event.duration,
                notes.iter().map(|note| note.written_pitch).collect::<Vec<_>>(),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    events.sort();
    let mut directions = score
        .voices
        .values()
        .flat_map(|voice| voice.events.iter())
        .filter_map(|event| match &event.kind {
            EventKind::Direction(direction) => {
                Some(format!("{}:{:?}", event.onset.0, direction.kind))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    directions.sort();
    let mut harmonies = score
        .voices
        .values()
        .flat_map(|voice| voice.events.iter())
        .filter_map(|event| match &event.kind {
            EventKind::ChordSymbol(chord) => Some(format!("{}:{chord:?}", event.onset.0)),
            _ => None,
        })
        .collect::<Vec<_>>();
    harmonies.sort();
    MusicalSnapshot {
        measures,
        notes: events,
        tuplets: score
            .voices
            .values()
            .flat_map(|voice| voice.events.iter())
            .filter(|event| !event.tuplets.is_empty())
            .count(),
        ties: score
            .voices
            .values()
            .flat_map(|voice| voice.events.iter())
            .flat_map(TimedEvent::chord_notes)
            .filter(|note| note.tie_to.is_some())
            .count(),
        slurs: score
            .spanners
            .values()
            .filter(|spanner| matches!(spanner.kind, SpannerKind::Slur { .. }))
            .count(),
        lyrics: score
            .lyrics
            .iter()
            .map(|lyric| (lyric.verse, lyric.text.clone(), lyric.role, lyric.melisma_to.is_some()))
            .collect(),
        repeats: score.flow.repeats.clone(),
        voltas: score.flow.voltas.clone(),
        transpositions: score.parts.values().map(|part| part.transposition).collect(),
        maps: score.maps.clone(),
        articulations: score
            .voices
            .values()
            .flat_map(|voice| voice.events.iter())
            .map(|event| event.articulations.len())
            .sum(),
        ornaments: score
            .voices
            .values()
            .flat_map(|voice| voice.events.iter())
            .map(|event| event.ornaments.len())
            .sum(),
        hairpins: score
            .spanners
            .values()
            .filter(|spanner| matches!(spanner.kind, SpannerKind::Hairpin { .. }))
            .count(),
        directions,
        harmonies,
    }
}

#[test]
fn model_musicxml_model_round_trip_preserves_musical_content() {
    let first = import_musicxml_str(COMPREHENSIVE_XML).unwrap().score;
    let (xml, export_report) = export_musicxml_string(&first).unwrap();
    assert!(export_report
        .diagnostics
        .iter()
        .all(|diagnostic| diagnostic.code != "musicxml.non-terminating-decimal"));
    let second = import_musicxml_str(&xml).unwrap().score;
    assert_eq!(musical_snapshot(&first), musical_snapshot(&second));
}

fn midi_fixture(tuplets: bool) -> MidiFile {
    let mut events = vec![
        TrackEvent {
            tick: 0,
            kind: MidiEventKind::Meta(MetaEvent::SequenceOrTrackName(b"MIDI Fixture".to_vec())),
        },
        TrackEvent {
            tick: 0,
            kind: MidiEventKind::Meta(MetaEvent::SetTempo(500_000)),
        },
        TrackEvent {
            tick: 0,
            kind: MidiEventKind::Meta(MetaEvent::TimeSignature(TimeSignature::default())),
        },
        TrackEvent {
            tick: 0,
            kind: MidiEventKind::Meta(MetaEvent::KeySignature(MidiKeySignature {
                sharps_flats: 1,
                is_minor: false,
            })),
        },
        TrackEvent {
            tick: 0,
            kind: MidiEventKind::Channel(ChannelEvent {
                channel: 0,
                message: ChannelMessage::ControlChange {
                    controller: 64,
                    value: 127,
                },
            }),
        },
    ];
    let notes = if tuplets {
        vec![(0, 160, 66), (160, 320, 68), (320, 480, 69)]
    } else {
        vec![(13, 470, 66), (0, 480, 48)]
    };
    for (on, off, key) in notes {
        events.push(TrackEvent {
            tick: on,
            kind: MidiEventKind::Channel(ChannelEvent {
                channel: 0,
                message: ChannelMessage::NoteOn { key, velocity: 96 },
            }),
        });
        events.push(TrackEvent {
            tick: off,
            kind: MidiEventKind::Channel(ChannelEvent {
                channel: 0,
                message: ChannelMessage::NoteOff { key, velocity: 32 },
            }),
        });
    }
    events.sort_by_key(|event| event.tick);
    events.push(TrackEvent {
        tick: 480,
        kind: MidiEventKind::Meta(MetaEvent::EndOfTrack),
    });
    MidiFile {
        header: Header {
            format: Format::SingleTrack,
            track_count: 1,
            division: MidiDivision::TicksPerQuarter(480),
            extra_data: Vec::new(),
        },
        tracks: vec![Track {
            events,
            trailing_data: Vec::new(),
        }],
        unknown_chunks: Vec::new(),
    }
}

#[test]
fn midi_quantizes_splits_hands_spells_pitch_and_retains_raw_take() {
    let fixture = midi_fixture(false);
    let bytes = fixture.to_bytes().unwrap();
    let imported = import_midi_bytes(&bytes).unwrap();
    assert_eq!(imported.performance.file, MidiFile::parse(&bytes).unwrap());
    assert_eq!(imported.score.staves.len(), 2);
    let pitched = imported
        .score
        .voices
        .values()
        .flat_map(|voice| voice.events.iter())
        .filter(|event| matches!(event.kind, EventKind::Chord(_)))
        .collect::<Vec<_>>();
    assert!(pitched.iter().any(|event| {
        event.onset == ScoreTime::ZERO
            && event.duration == Some(Duration::new(1, 4).unwrap())
            && event.chord_notes().iter().any(|note| {
                note.written_pitch.is_some_and(|pitch| {
                    pitch.step == Step::F && pitch.alter == Alter::new(1, 1).unwrap()
                })
            })
    }));
    assert!(imported
        .report
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "midi.control-change"));
    assert!(imported
        .report
        .inferences
        .iter()
        .any(|inference| inference.kind == InferenceKind::Quantization));
}

#[test]
fn midi_pitch_spelling_is_contextual_and_tuplets_are_detected() {
    let sharp = spell_midi_pitches(&[66], 1)[0];
    let flat = spell_midi_pitches(&[66], -6)[0];
    assert_eq!((sharp.step, sharp.alter), (Step::F, Alter::new(1, 1).unwrap()));
    assert_eq!((flat.step, flat.alter), (Step::G, Alter::new(-1, 1).unwrap()));

    let imported = import_midi(&midi_fixture(true), MidiImportOptions::default()).unwrap();
    let tuplets = imported
        .score
        .voices
        .values()
        .flat_map(|voice| voice.events.iter())
        .filter(|event| !event.tuplets.is_empty())
        .count();
    assert_eq!(tuplets, 3);
    assert!(imported.score.validate().iter().all(|problem| {
        !matches!(
            problem,
            ValidationProblem::DanglingTie { .. }
                | ValidationProblem::NonReciprocalTie { .. }
                | ValidationProblem::TiePitchMismatch { .. }
        )
    }));
}

/// One note: `(track, channel, key, tick_on, tick_off)`.
type FixtureNote = (usize, u8, u8, u64, u64);

/// Builds a piano MIDI from note tuples, one track per named part. Tracks with
/// an empty name carry no hand information at all.
fn piano_fixture(track_names: &[&str], notes: &[FixtureNote]) -> MidiFile {
    let mut tracks = Vec::new();
    for (index, name) in track_names.iter().enumerate() {
        let mut events = Vec::new();
        if !name.is_empty() {
            events.push(TrackEvent {
                tick: 0,
                kind: MidiEventKind::Meta(MetaEvent::SequenceOrTrackName(name.as_bytes().to_vec())),
            });
        }
        if index == 0 {
            events.push(TrackEvent {
                tick: 0,
                kind: MidiEventKind::Meta(MetaEvent::SetTempo(500_000)),
            });
            events.push(TrackEvent {
                tick: 0,
                kind: MidiEventKind::Meta(MetaEvent::TimeSignature(TimeSignature::default())),
            });
        }
        for (track, channel, key, on, off) in notes.iter().copied() {
            if track != index {
                continue;
            }
            events.push(TrackEvent {
                tick: on,
                kind: MidiEventKind::Channel(ChannelEvent {
                    channel,
                    message: ChannelMessage::NoteOn { key, velocity: 88 },
                }),
            });
            events.push(TrackEvent {
                tick: off,
                kind: MidiEventKind::Channel(ChannelEvent {
                    channel,
                    message: ChannelMessage::NoteOff { key, velocity: 0 },
                }),
            });
        }
        events.sort_by_key(|event| event.tick);
        let last = events.last().map_or(0, |event| event.tick);
        events.push(TrackEvent {
            tick: last,
            kind: MidiEventKind::Meta(MetaEvent::EndOfTrack),
        });
        tracks.push(Track {
            events,
            trailing_data: Vec::new(),
        });
    }
    MidiFile {
        header: Header {
            format: if tracks.len() == 1 {
                Format::SingleTrack
            } else {
                Format::Parallel
            },
            track_count: tracks.len() as u16,
            division: MidiDivision::TicksPerQuarter(480),
            extra_data: Vec::new(),
        },
        tracks,
        unknown_chunks: Vec::new(),
    }
}

fn staff_keys(imported: &MidiImportResult) -> (Vec<u8>, Vec<u8>) {
    let mut upper = Vec::new();
    let mut lower = Vec::new();
    let staves = imported
        .score
        .parts
        .values()
        .next()
        .expect("piano part")
        .staves
        .clone();
    for note in &imported.performed_notes {
        match note.hand {
            Hand::Right => upper.push(note.key),
            Hand::Left => lower.push(note.key),
        }
    }
    // The hand and the staff a note is drawn on must agree.
    for voice in imported.score.voices.values() {
        let expected = if voice.staff == staves[0] {
            Hand::Right
        } else {
            Hand::Left
        };
        for event in &voice.events {
            for note in event.chord_notes() {
                assert_eq!(note.display_staff, voice.staff);
                let performed = imported
                    .performed_notes
                    .iter()
                    .find(|performed| performed.note == note.id);
                if let Some(performed) = performed {
                    assert_eq!(performed.hand, expected);
                }
            }
        }
    }
    upper.sort_unstable();
    lower.sort_unstable();
    (upper, lower)
}

/// The opening of the C major prelude in miniature: the left hand plays above
/// middle C, so any fixed split key puts the whole texture on one staff.
#[test]
fn midi_named_tracks_place_the_left_hand_above_middle_c() {
    let mut notes: Vec<FixtureNote> = Vec::new();
    for repeat in 0..4_u64 {
        let bar = repeat * 1920;
        // Left hand: C4 and E4, held.
        notes.push((1, 0, 60, bar, bar + 950));
        notes.push((1, 0, 64, bar + 120, bar + 950));
        // Right hand: G4 C5 E5, sixteenths.
        for (step, key) in [67_u8, 72, 76].into_iter().enumerate() {
            let on = bar + 240 + step as u64 * 120;
            notes.push((0, 0, key, on, on + 110));
        }
    }
    let file = piano_fixture(&["Piano right", "Piano left"], &notes);
    let imported = import_midi(&file, MidiImportOptions::default()).unwrap();
    let (upper, lower) = staff_keys(&imported);
    assert_eq!(upper, vec![67, 67, 67, 67, 72, 72, 72, 72, 76, 76, 76, 76]);
    assert_eq!(lower, vec![60, 60, 60, 60, 64, 64, 64, 64]);
    assert!(imported
        .performed_notes
        .iter()
        .all(|note| note.hand_source == HandSource::TrackName));
    // The bass staff must carry notes in every bar.
    let lower_staff = imported.score.parts.values().next().unwrap().staves[1];
    for bar in 0..4 {
        let start = ScoreTime::new(bar, 1).unwrap();
        let end = ScoreTime::new(bar + 1, 1).unwrap();
        assert!(
            imported
                .score
                .voices
                .values()
                .filter(|voice| voice.staff == lower_staff)
                .flat_map(|voice| voice.events.iter())
                .any(|event| {
                    !event.chord_notes().is_empty() && event.onset >= start && event.onset < end
                }),
            "bass staff is empty in bar {}",
            bar + 1
        );
    }
}

/// Two note streams and no names at all: the lower stream is the left hand.
#[test]
fn midi_two_streams_without_names_are_read_as_the_two_hands() {
    let mut notes: Vec<FixtureNote> = Vec::new();
    for step in 0..8_u64 {
        let on = step * 240;
        notes.push((0, 0, 76, on, on + 220));
        notes.push((0, 1, 55, on, on + 220));
    }
    let file = piano_fixture(&[""], &notes);
    let imported = import_midi(&file, MidiImportOptions::default()).unwrap();
    let (upper, lower) = staff_keys(&imported);
    assert!(upper.iter().all(|key| *key == 76));
    assert!(lower.iter().all(|key| *key == 55));
    assert!(imported
        .performed_notes
        .iter()
        .all(|note| note.hand_source == HandSource::StreamLayout));
}

/// A bass line that walks up past middle C while the melody stays high. A fixed
/// split key would hand the top of the walk to the treble staff; continuity
/// keeps the line whole, so the same pitch region ends up on both staves.
#[test]
fn midi_hand_assignment_follows_the_line_not_the_key_number() {
    let mut notes: Vec<FixtureNote> = Vec::new();
    for step in 0..12_u64 {
        let on = step * 240;
        notes.push((0, 0, 48 + step as u8 * 2, on, on + 230));
        notes.push((0, 0, 74, on, on + 230));
    }
    let file = piano_fixture(&[""], &notes);
    let imported = import_midi(&file, MidiImportOptions::default()).unwrap();
    let (upper, lower) = staff_keys(&imported);
    assert!(upper.iter().all(|key| *key == 74), "melody stays right handed");
    assert_eq!(lower, (0..12).map(|step| 48 + step * 2).collect::<Vec<u8>>());
    // The walk crosses middle C, so no single key number reproduces this split.
    assert!(lower.iter().any(|key| *key >= 60));
}

/// The left hand reaches over a right-hand chord it could not also hold. The
/// bass staff has to end up carrying the highest sounding note.
#[test]
fn midi_hand_assignment_lets_the_hands_cross() {
    let mut notes: Vec<FixtureNote> = Vec::new();
    for bar in 0..8_u64 {
        let start = bar * 1920;
        for beat in 0..4_u64 {
            let on = start + beat * 480;
            for key in [60_u8, 64, 67] {
                notes.push((0, 0, key, on, on + 460));
            }
            let reach = if beat % 2 == 0 { 36 } else { 84 };
            notes.push((0, 0, reach, on, on + 460));
        }
    }
    let file = piano_fixture(&[""], &notes);
    let imported = import_midi(&file, MidiImportOptions::default()).unwrap();
    let (upper, lower) = staff_keys(&imported);
    assert!(upper.iter().all(|key| matches!(key, 60 | 64 | 67)));
    assert!(lower.iter().all(|key| matches!(key, 36 | 84)));
    assert!(
        lower.contains(&84),
        "the reaching-over note stayed with the hand that plays the bass"
    );
    // Which is a crossing: the bass staff sounds above the treble staff.
    let staves = imported.score.parts.values().next().unwrap().staves.clone();
    let highest = |staff: StaffId| {
        imported
            .score
            .voices
            .values()
            .filter(|voice| voice.staff == staff)
            .flat_map(|voice| voice.events.iter())
            .filter(|event| event.onset == ScoreTime::new(1, 4).unwrap())
            .flat_map(TimedEvent::chord_notes)
            .filter_map(|note| note.written_pitch)
            .map(|pitch| i32::from(pitch.octave) * 12 + i32::from(pitch.step.index()))
            .max()
    };
    assert!(highest(staves[1]) > highest(staves[0]));
}

/// A simultaneity no hand could take alone is divided, and neither hand is left
/// holding more than the configured reach.
#[test]
fn midi_wide_simultaneity_is_divided_between_the_hands() {
    let notes: Vec<FixtureNote> = [36_u8, 43, 48, 72, 79, 84]
        .into_iter()
        .map(|key| (0_usize, 0_u8, key, 0_u64, 1900_u64))
        .collect();
    let file = piano_fixture(&[""], &notes);
    let options = MidiImportOptions::default();
    let imported = import_midi(&file, options).unwrap();
    let (upper, lower) = staff_keys(&imported);
    assert!(!upper.is_empty() && !lower.is_empty(), "the chord was divided");
    let span = |keys: &[u8]| i32::from(*keys.last().unwrap()) - i32::from(*keys.first().unwrap());
    assert!(span(&upper) <= i32::from(options.max_hand_span_semitones));
    assert!(span(&lower) <= i32::from(options.max_hand_span_semitones));
    assert_eq!(upper, vec![72, 79, 84]);
    assert_eq!(lower, vec![36, 43, 48]);
}

/// Detached playing is articulation, not rhythm: sixteenths released after a
/// thirty-second are still sixteenths, and the performed release survives beside
/// the written value rather than replacing it.
#[test]
fn midi_written_values_come_from_onset_spacing_not_release() {
    // Sixteenths, each released after a thirty-second. On a thirty-second grid
    // the release is exactly what the old rule would have written down.
    let notes: Vec<FixtureNote> = (0..16_u64)
        .map(|step| {
            let on = step * 120;
            (0_usize, 0_u8, 67_u8, on, on + 60)
        })
        .collect();
    let file = piano_fixture(&[""], &notes);
    let options = MidiImportOptions {
        quantize_grid: Duration::new(1, 32).unwrap(),
        ..MidiImportOptions::default()
    };
    let imported = import_midi(&file, options).unwrap();
    let sixteenth = Duration::new(1, 16).unwrap();
    let thirty_second = Duration::new(1, 32).unwrap();
    let written = imported
        .score
        .voices
        .values()
        .flat_map(|voice| voice.events.iter())
        .filter(|event| !event.chord_notes().is_empty())
        .map(|event| event.duration)
        .collect::<Vec<_>>();
    assert_eq!(written.len(), 16);
    assert!(
        written.iter().all(|value| *value == Some(sixteenth)),
        "played staccato sixteenths must notate as sixteenths, not {written:?}"
    );
    // No rest was written between them: the silence is articulation.
    assert!(imported
        .score
        .voices
        .values()
        .flat_map(|voice| voice.events.iter())
        .filter(|event| matches!(event.kind, EventKind::Rest))
        .all(|event| event.onset >= ScoreTime::new(1, 1).unwrap()));
    // The performance is not rewritten: the raw take still says a thirty-second.
    assert!(imported.performed_notes.iter().all(|note| {
        note.written_duration == Some(sixteenth)
            && note.sounding_end.checked_sub(note.sounding_onset).unwrap().0 == thirty_second.0
            && note.played_end.checked_sub(note.played_onset).unwrap().0 == thirty_second.0
    }));
    assert_eq!(imported.performance.sequence.notes.len(), 16);

    // The same shape one value coarser: eighths released after a sixteenth on
    // the default grid must not become sixteenths followed by rests.
    let notes: Vec<FixtureNote> = (0..8_u64)
        .map(|step| {
            let on = step * 240;
            (0_usize, 0_u8, 67_u8, on, on + 120)
        })
        .collect();
    let file = piano_fixture(&[""], &notes);
    let imported = import_midi(&file, MidiImportOptions::default()).unwrap();
    let eighth = Duration::new(1, 8).unwrap();
    assert!(imported
        .performed_notes
        .iter()
        .all(|note| note.written_duration == Some(eighth)
            && note.sounding_end.checked_sub(note.sounding_onset).unwrap().0 == sixteenth.0));
}

/// A note followed by a silence of a beat or more keeps its own value and the
/// rest is written out; only shorter silences are absorbed as articulation.
#[test]
fn midi_a_full_beat_of_silence_is_written_as_a_rest() {
    let notes: Vec<FixtureNote> = (0..4_u64)
        .map(|step| {
            let on = step * 960;
            (0_usize, 0_u8, 67_u8, on, on + 460)
        })
        .collect();
    let file = piano_fixture(&[""], &notes);
    let imported = import_midi(&file, MidiImportOptions::default()).unwrap();
    let quarter = Duration::new(1, 4).unwrap();
    let voice = imported
        .score
        .voices
        .values()
        .find(|voice| voice.events.iter().any(|event| !event.chord_notes().is_empty()))
        .unwrap();
    assert!(voice
        .events
        .iter()
        .filter(|event| !event.chord_notes().is_empty())
        .all(|event| event.duration == Some(quarter)));
    assert!(voice
        .events
        .iter()
        .any(|event| matches!(event.kind, EventKind::Rest) && event.duration == Some(quarter)));
}

/// Notes too short for the grid are never dropped: a slot with two real attacks
/// is quantized finer, and what is left over becomes a grace note.
#[test]
fn midi_notes_shorter_than_the_grid_survive_quantization() {
    let mut notes: Vec<FixtureNote> = Vec::new();
    // Eight thirty-seconds inside four sixteenth-note slots.
    for step in 0..8_u64 {
        let on = step * 60;
        notes.push((0, 0, 72 + step as u8, on, on + 50));
    }
    // A twelve-tick flick immediately before a sixteenth on the same pitch.
    notes.push((0, 0, 65, 960, 972));
    notes.push((0, 0, 65, 975, 1080));
    let file = piano_fixture(&[""], &notes);
    let imported = import_midi(&file, MidiImportOptions::default()).unwrap();
    assert_eq!(imported.performed_notes.len(), notes.len());
    let heads = imported
        .score
        .voices
        .values()
        .flat_map(|voice| voice.events.iter())
        .flat_map(TimedEvent::chord_notes)
        .count();
    assert_eq!(heads, notes.len(), "every performed note reached the page");
    let thirty_second = Duration::new(1, 32).unwrap();
    let sixteenth = Duration::new(1, 16).unwrap();
    // The run is quantized on a finer grid than the sixteenth-note default,
    // so it reads as thirty-seconds rather than as one blurred sixteenth.
    assert!(imported.performed_notes[..8]
        .iter()
        .all(|note| note.written_duration.is_some_and(|value| value <= sixteenth)));
    assert!(
        imported.performed_notes[..8]
            .iter()
            .filter(|note| note.written_duration == Some(thirty_second))
            .count()
            >= 6
    );
    let onsets = imported.performed_notes[..8]
        .iter()
        .map(|note| note.written_onset)
        .collect::<BTreeSet<_>>();
    assert_eq!(onsets.len(), 8, "every attack kept a place of its own");
    // Nothing collapsed to nothing, and the report says what it did.
    assert!(imported
        .report
        .diagnostics
        .iter()
        .all(|diagnostic| diagnostic.code != "midi.zero-quantized-duration"));
    assert!(imported.report.diagnostics.iter().any(|diagnostic| {
        matches!(
            diagnostic.code,
            "midi.subdivided-slot" | "midi.grace-note" | "midi.repeated-note-collision"
        )
    }));
    assert!(imported.score.validate().is_empty());
}

/// Written values are ordinary note values: an odd length is tied, not drawn as
/// a duration no notehead exists for.
#[test]
fn midi_written_values_are_ordinary_note_values() {
    let notes: Vec<FixtureNote> = vec![
        (0, 0, 67, 0, 1180),      // five sixteenths on the beat
        (0, 0, 67, 1200, 1900),
        (0, 0, 67, 1920, 2100),
    ];
    let file = piano_fixture(&[""], &notes);
    let imported = import_midi(&file, MidiImportOptions::default()).unwrap();
    let ordinary = |value: Duration| {
        for (numerator, denominator) in [(4, 1_u64), (2, 1), (1, 1), (1, 2), (1, 4), (1, 8), (1, 16), (1, 32), (1, 64)] {
            let base = Rational::new(numerator, denominator).unwrap();
            for (dot_numerator, dot_denominator) in [(1, 1_u64), (3, 2), (7, 4)] {
                let dotted = base
                    .checked_mul(Rational::new(dot_numerator, dot_denominator).unwrap())
                    .unwrap();
                for (scale_numerator, scale_denominator) in [(1, 1_u64), (2, 3)] {
                    let candidate = dotted
                        .checked_mul(Rational::new(scale_numerator, scale_denominator).unwrap())
                        .unwrap();
                    if candidate == value.0 {
                        return true;
                    }
                }
            }
        }
        false
    };
    let events = imported
        .score
        .voices
        .values()
        .flat_map(|voice| voice.events.iter())
        .filter(|event| event.duration.is_some())
        .collect::<Vec<_>>();
    assert!(events.iter().all(|event| ordinary(event.duration.unwrap())));
    // Five sixteenths on the beat is a quarter tied to a sixteenth.
    let tied = imported
        .score
        .voices
        .values()
        .flat_map(|voice| voice.events.iter())
        .flat_map(TimedEvent::chord_notes)
        .filter(|note| note.tie_to.is_some())
        .count();
    assert_eq!(tied, 1);
    assert!(imported.score.validate().is_empty());
}
