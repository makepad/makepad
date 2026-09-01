use crate::*;

const COMPLEX_PARTWISE: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE score-partwise PUBLIC "-//Recordare//DTD MusicXML 4.0 Partwise//EN" "http://www.musicxml.org/dtds/partwise.dtd">
<score-partwise version="4.0" id="score-anchor" xmlns:ext="https://example.test/ext">
  <work id="work-1"><work-number>Op. 1</work-number><work-title>Piano &amp; Winds — Étude</work-title></work>
  <identification id="identity"><creator type="composer">Zoë Example</creator><encoding><software>Makepad</software></encoding></identification>
  <defaults id="defaults"><scaling><millimeters>7</millimeters><tenths>40</tenths></scaling><page-layout><page-height>1683</page-height><page-width>1190</page-width></page-layout></defaults>
  <credit page="1" id="credit-1"><credit-type>title</credit-type><credit-words>Étude &lt;No. 1&gt;</credit-words></credit>
  <part-list id="part-list-1">
    <part-group type="start" number="1" id="group-1"><group-name>Ensemble</group-name><group-symbol>brace</group-symbol></part-group>
    <score-part id="P1"><part-name>Piano</part-name><score-instrument id="P1-I1"><instrument-name>Piano</instrument-name></score-instrument><midi-instrument id="P1-I1"><midi-channel>1</midi-channel><midi-program>1</midi-program></midi-instrument></score-part>
    <score-part id="P2"><part-name>Clarinet in B♭</part-name><score-instrument id="P2-I1"><instrument-name>Clarinet</instrument-name></score-instrument><midi-instrument id="P2-I1"><midi-channel>2</midi-channel><midi-program>72</midi-program></midi-instrument></score-part>
    <part-group type="stop" number="1"/>
  </part-list>
  <part id="P1">
    <measure number="1" id="measure-p1-1" width="300">
      <print new-system="yes" id="print-1"><system-layout><system-distance>120</system-distance></system-layout><staff-layout number="2"><staff-distance>80</staff-distance></staff-layout></print>
      <attributes id="attrs-1">
        <divisions>12</divisions><key number="1"><fifths>2</fifths><mode>major</mode></key>
        <time symbol="normal"><beats>3</beats><beat-type>8</beat-type><beats>2</beats><beat-type>8</beat-type></time>
        <staves>2</staves><clef number="1"><sign>G</sign><line>2</line><clef-octave-change>1</clef-octave-change></clef><clef number="2"><sign>F</sign><line>4</line></clef>
        <staff-details number="2"><staff-type>regular</staff-type><staff-lines>5</staff-lines></staff-details>
        <measure-style number="1"><multiple-rest>2</multiple-rest></measure-style>
      </attributes>
      <note id="note-chord-a"><pitch><step>C</step><alter>1</alter><octave>5</octave></pitch><duration>6</duration><tie type="start"/><voice>1</voice><type>eighth</type><dot/><accidental cautionary="yes" editorial="yes">sharp</accidental><stem>up</stem><notehead filled="yes">normal</notehead><staff>1</staff><beam number="1">begin</beam><instrument id="P1-I1"/><notations id="notations-a"><tied type="start" number="1"/><slur type="start" number="2"/><articulations><staccato/></articulations><ornaments><trill-mark/></ornaments><technical><fingering>1</fingering></technical><dynamics><mf/></dynamics><fermata>upright</fermata><arpeggiate number="1"/></notations><lyric number="1" name="English"><syllabic>begin</syllabic><text>hel</text><elision>‿</elision><text>lo</text><extend type="start"/></lyric><lyric number="2"><syllabic>single</syllabic><text>salut</text></lyric></note>
      <note id="note-chord-b"><chord/><pitch><step>E</step><octave>5</octave></pitch><duration>6</duration><voice>1</voice><type>eighth</type><staff>1</staff><beam number="1">continue</beam></note>
      <note id="tuplet-note"><pitch><step>G</step><octave>5</octave></pitch><duration>4</duration><tie type="stop"/><voice>1</voice><type>eighth</type><time-modification><actual-notes>3</actual-notes><normal-notes>2</normal-notes><normal-type>eighth</normal-type><normal-dot/></time-modification><staff>1</staff><beam number="1">end</beam><notations><tied type="stop" number="1"/><slur type="stop" number="2"/><tuplet type="start" number="1"/><glissando type="start" number="1">gliss.</glissando><slide type="start" number="1">slide</slide><non-arpeggiate type="top"/></notations></note>
      <backup id="backup-1"><duration>16</duration></backup>
      <note id="grace-note"><grace slash="yes" steal-time-following="12.5"/><cue/><unpitched><display-step>D</display-step><display-octave>4</display-octave></unpitched><voice>2</voice><type>16th</type><staff>2</staff></note>
      <forward id="forward-1"><duration>4</duration><voice>2</voice><staff>2</staff></forward>
      <direction placement="above" id="direction-1"><direction-type><words>Allegro</words><dynamics><f/></dynamics><wedge type="crescendo"/><metronome><beat-unit>quarter</beat-unit><per-minute>120</per-minute></metronome><octave-shift type="up" size="8"/><pedal type="start"/><dashes type="start"/><bracket type="start"/><rehearsal>A</rehearsal><segno/><coda/><harp-pedals><pedal-tuning><pedal-step>D</pedal-step><pedal-alter>-1</pedal-alter></pedal-tuning></harp-pedals><symbol>noteheadDiamondBlack</symbol></direction-type><offset>0</offset><voice>1</voice><staff>1</staff><sound tempo="120" dynamics="80"/></direction>
      <harmony id="harmony-1"><root><root-step>C</root-step><root-alter>1</root-alter></root><kind text="maj7">major-seventh</kind><bass><bass-step>G</bass-step></bass><degree><degree-value>9</degree-value><degree-alter>0</degree-alter><degree-type>add</degree-type></degree></harmony>
      <figured-bass id="fb-1"><figure><prefix>♯</prefix><figure-number>6</figure-number><suffix>+</suffix></figure><duration>12</duration></figured-bass>
      <sound id="sound-1" tempo="120" dacapo="no"><midi-instrument id="P1-I1"><midi-channel>1</midi-channel></midi-instrument></sound>
      <barline location="left" id="bar-left"><bar-style>heavy-light</bar-style><repeat direction="forward"/></barline>
      <barline location="right" id="bar-right"><bar-style>light-heavy</bar-style><ending number="1, 2" type="start">1–2.</ending><repeat direction="backward" times="2"/></barline>
      <ext:analysis id="extension-1" ext:quality="kept"><![CDATA[x < y && z]]><!--inside extension--><ext:item/></ext:analysis>
    </measure>
  </part>
  <part id="P2">
    <measure number="1" id="measure-p2-1" width="300"><attributes><divisions>12</divisions><key><fifths>2</fifths></key><time><beats>5</beats><beat-type>8</beat-type></time><transpose id="transpose-1"><diatonic>-1</diatonic><chromatic>-2</chromatic><octave-change>0</octave-change><double/></transpose><clef><sign>G</sign><line>2</line></clef></attributes><note id="clarinet-note"><pitch><step>D</step><octave>5</octave></pitch><duration>12</duration><voice>1</voice><type>quarter</type><instrument id="P2-I1"/></note></measure>
  </part>
</score-partwise>"##;

const SIMPLE_TIMEWISE: &str = r#"<score-timewise version="4.0" id="tw-score">
  <work id="tw-work"><work-title>Timewise</work-title></work>
  <part-list><score-part id="P1"><part-name>One</part-name></score-part><score-part id="P2"><part-name>Two</part-name></score-part></part-list>
  <measure number="1" id="tw-m1"><part id="P1"><attributes><divisions>4</divisions><time><beats>4</beats><beat-type>4</beat-type></time></attributes><note id="tw-n1"><pitch><step>C</step><octave>4</octave></pitch><duration>4</duration><voice>1</voice><type>quarter</type></note></part><part id="P2"><note id="tw-n2"><rest/><duration>4</duration><voice>1</voice><type>quarter</type></note></part></measure>
  <measure number="2" id="tw-m2"><part id="P1"><note><pitch><step>D</step><octave>4</octave></pitch><duration>4</duration></note></part><part id="P2"><note><pitch><step>F</step><octave>3</octave></pitch><duration>4</duration></note></part></measure>
</score-timewise>"#;

fn partwise(document: &MusicXmlDocument) -> &ScorePartwise {
    match &document.score {
        Score::Partwise(score) => score,
        Score::Timewise(_) => panic!("expected partwise score"),
    }
}

fn assert_round_trip(source: &str) -> MusicXmlDocument {
    let first = parse_musicxml(source).unwrap();
    let written = write_musicxml(&first).unwrap();
    let second = parse_musicxml(&written).unwrap();
    assert_eq!(first, second);
    first
}

#[test]
fn complex_partwise_document_is_typed_and_round_trips() {
    let document = assert_round_trip(COMPLEX_PARTWISE);
    assert_eq!(document.format(), ScoreFormat::Partwise);
    assert_eq!(document.version(), Some("4.0"));
    assert_eq!(document.root().id(), Some("score-anchor"));

    let score = partwise(&document);
    let part_list = document.score.part_list().unwrap();
    assert_eq!(part_list.score_parts().count(), 2);
    assert_eq!(part_list.score_parts().next().unwrap().name().as_deref(), Some("Piano"));
    assert_eq!(score.parts().count(), 2);

    let measure = score.part("P1").unwrap().measures().next().unwrap();
    assert_eq!(measure.number(), Some("1"));
    assert_eq!(measure.id(), Some("measure-p1-1"));
    let attributes = measure.attributes().next().unwrap();
    assert_eq!(attributes.divisions(), Some(12));
    assert_eq!(attributes.staves(), Some(2));
    assert_eq!(attributes.keys().next().unwrap().fifths(), Some(2));
    assert_eq!(
        attributes.times().next().unwrap().components(),
        vec![
            TimeComponent { beats: "3".into(), beat_type: "8".into() },
            TimeComponent { beats: "2".into(), beat_type: "8".into() },
        ]
    );
    let clefs: Vec<_> = attributes.clefs().collect();
    assert_eq!(clefs.len(), 2);
    assert_eq!(clefs[0].number(), Some(1));
    assert_eq!(clefs[0].octave_change(), Some(1));
    assert_eq!(attributes.staff_details().next().unwrap().staff_lines(), Some(5));
    assert_eq!(attributes.measure_styles().count(), 1);

    let items: Vec<_> = measure.items().collect();
    assert!(items.iter().any(|item| matches!(item, MeasureItemRef::Print(_))));
    assert!(items.iter().any(|item| matches!(item, MeasureItemRef::Backup(_))));
    assert!(items.iter().any(|item| matches!(item, MeasureItemRef::Forward(_))));
    assert!(items.iter().any(|item| matches!(item, MeasureItemRef::Harmony(_))));
    assert!(items.iter().any(|item| matches!(item, MeasureItemRef::FiguredBass(_))));
    assert!(items.iter().any(|item| matches!(item, MeasureItemRef::Sound(_))));
}

#[test]
fn notes_keep_chords_beams_tuplets_ties_slurs_grace_and_lyrics_distinct() {
    let document = parse_musicxml(COMPLEX_PARTWISE).unwrap();
    let measure = partwise(&document)
        .part("P1")
        .unwrap()
        .measures()
        .next()
        .unwrap();
    let notes: Vec<_> = measure.notes().collect();
    assert_eq!(notes.len(), 4);
    match notes[0].kind().unwrap() {
        NoteKind::Pitch(pitch) => {
            assert_eq!(pitch.step, Step::C);
            assert_eq!(pitch.alter, Some(1.0));
            assert_eq!(pitch.octave, 5);
        }
        other => panic!("wrong note kind: {other:?}"),
    }
    assert_eq!(notes[0].duration(), Some(6));
    assert_eq!(notes[0].voice().as_deref(), Some("1"));
    assert_eq!(notes[0].dots(), 1);
    assert_eq!(notes[0].staff(), Some(1));
    assert_eq!(notes[0].instrument_id(), Some("P1-I1"));
    assert_eq!(notes[0].beams().next().unwrap().value, "begin");
    let accidental = notes[0].accidental().unwrap();
    assert!(accidental.cautionary && accidental.editorial);
    assert!(notes[1].is_chord());

    let tuplet = notes[2].time_modification().unwrap();
    assert_eq!(tuplet.actual_notes, 3);
    assert_eq!(tuplet.normal_notes, 2);
    assert_eq!(tuplet.normal_type.as_deref(), Some("eighth"));
    assert_eq!(tuplet.normal_dots, 1);

    let sounding_ties: Vec<_> = notes[0].ties().collect();
    assert_eq!(sounding_ties.len(), 1);
    assert_eq!(sounding_ties[0].kind, StartStopContinue::Start);
    let notation_items: Vec<_> = notes[0].notations().next().unwrap().items().collect();
    assert!(notation_items.iter().any(|item| matches!(
        item,
        NotationItemRef::Tied(tie) if tie.kind == StartStopContinue::Start
    )));
    assert!(notation_items
        .iter()
        .any(|item| matches!(item, NotationItemRef::Slur(element) if element.attr("number") == Some("2"))));

    assert!(notes[3].is_grace());
    assert!(notes[3].is_cue());
    assert_eq!(notes[3].duration(), None);
    assert_eq!(notes[3].grace().unwrap().attr("slash"), Some("yes"));

    let lyrics: Vec<_> = notes[0].lyrics().collect();
    assert_eq!(lyrics.len(), 2);
    assert_eq!(lyrics[0].number(), Some("1"));
    assert_eq!(lyrics[0].texts().collect::<Vec<_>>(), ["hel", "lo"]);
    assert_eq!(lyrics[0].elisions().collect::<Vec<_>>(), ["‿"]);
    assert!(lyrics[0].has_extend());
    assert_eq!(lyrics[1].texts().collect::<Vec<_>>(), ["salut"]);
}

#[test]
fn directions_barlines_and_transposition_have_typed_views() {
    let document = parse_musicxml(COMPLEX_PARTWISE).unwrap();
    let first_measure = partwise(&document)
        .part("P1")
        .unwrap()
        .measures()
        .next()
        .unwrap();
    let direction = first_measure
        .items()
        .find_map(|item| match item {
            MeasureItemRef::Direction(value) => Some(value),
            _ => None,
        })
        .unwrap();
    let types: Vec<_> = direction.types().collect();
    assert_eq!(types.len(), 13);
    assert!(types.iter().any(|item| matches!(item, DirectionTypeItemRef::Words(_))));
    assert!(types.iter().any(|item| matches!(item, DirectionTypeItemRef::Metronome(_))));
    assert!(types.iter().any(|item| matches!(item, DirectionTypeItemRef::HarpPedals(_))));
    assert!(types.iter().any(|item| matches!(item, DirectionTypeItemRef::Symbol(_))));

    let right_barline = first_measure
        .items()
        .filter_map(|item| match item {
            MeasureItemRef::Barline(value) => Some(value),
            _ => None,
        })
        .find(|barline| barline.location() == Some("right"))
        .unwrap();
    assert_eq!(right_barline.bar_style().as_deref(), Some("light-heavy"));
    assert_eq!(right_barline.repeats().next().unwrap().attr("times"), Some("2"));
    assert_eq!(right_barline.endings().next().unwrap().attr("number"), Some("1, 2"));

    let second_measure = partwise(&document)
        .part("P2")
        .unwrap()
        .measures()
        .next()
        .unwrap();
    let transpose = second_measure
        .attributes()
        .next()
        .unwrap()
        .transposes()
        .next()
        .unwrap();
    assert_eq!(transpose.diatonic(), Some(-1));
    assert_eq!(transpose.chromatic(), Some(-2.0));
    assert_eq!(transpose.octave_change(), Some(0));
    assert!(transpose.doubled());
    assert_eq!(transpose.0.id(), Some("transpose-1"));
}

#[test]
fn timewise_round_trips_and_converts_both_directions() {
    let timewise = assert_round_trip(SIMPLE_TIMEWISE);
    let timewise_score = match &timewise.score {
        Score::Timewise(score) => score,
        _ => panic!("expected timewise"),
    };
    assert_eq!(timewise_score.measures().count(), 2);
    assert_eq!(timewise_score.measures().next().unwrap().parts().count(), 2);

    let converted = timewise.to_partwise().unwrap();
    assert_eq!(converted.format(), ScoreFormat::Partwise);
    assert_eq!(partwise(&converted).parts().count(), 2);
    assert_eq!(partwise(&converted).part("P1").unwrap().measures().count(), 2);
    assert_eq!(partwise(&converted).part("P2").unwrap().measures().count(), 2);
    assert_eq!(
        partwise(&converted)
            .parts()
            .flat_map(PartRef::measures)
            .filter(|measure| measure.id() == Some("tw-m1"))
            .count(),
        1
    );
    let reparsed = parse_musicxml(&converted.to_xml_string().unwrap()).unwrap();
    assert_eq!(converted, reparsed);

    let back = converted.to_timewise().unwrap();
    assert_eq!(back.format(), ScoreFormat::Timewise);
    let back_score = match &back.score {
        Score::Timewise(score) => score,
        _ => unreachable!(),
    };
    assert_eq!(back_score.measures().count(), 2);
    assert_eq!(back_score.measures().next().unwrap().parts().count(), 2);
}

#[test]
fn unknown_elements_ids_entities_unicode_and_xml_nodes_survive() {
    let source = r#"<?xml version="1.0"?><?before data?><score-partwise version="4.0" xmlns:x="urn:x" id="s"><part-list><score-part id="P1"><part-name>A &amp; B &#x1D11E; ♭</part-name></score-part></part-list><part id="P1"><measure number="1" id="m"><x:future id="stable" answer="&quot;42&quot;"><![CDATA[a < b & c]]><!--keep me--><?inside value?><x:empty/></x:future></measure></part></score-partwise><!--after--><!---->"#;
    let document = assert_round_trip(source);
    assert_eq!(document.before_score.len(), 1);
    assert_eq!(document.after_score.len(), 2);
    let unknown = document.unmodelled_elements();
    assert_eq!(unknown.iter().map(|element| element.name.as_str()).collect::<Vec<_>>(), ["x:future", "x:empty"]);
    assert_eq!(unknown[0].id(), Some("stable"));
    assert_eq!(unknown[0].attr("answer"), Some("\"42\""));
    let part_name = document
        .score
        .part_list()
        .unwrap()
        .score_parts()
        .next()
        .unwrap()
        .name()
        .unwrap();
    assert_eq!(part_name, "A & B 𝄞 ♭");
    assert!(matches!(unknown[0].children[0], XmlNode::CData(_)));
    assert!(matches!(unknown[0].children[1], XmlNode::Comment(_)));
    assert!(matches!(unknown[0].children[2], XmlNode::ProcessingInstruction { .. }));
}

#[test]
fn senza_misura_and_nontraditional_keys_parse() {
    let source = r#"<score-partwise version="4.0"><part-list><score-part id="P"><part-name>P</part-name></score-part></part-list><part id="P"><measure number="1"><attributes><key number="2"><key-step>F</key-step><key-alter>0.5</key-alter><key-accidental>quarter-sharp</key-accidental></key><time number="2"><senza-misura>free</senza-misura></time></attributes></measure></part></score-partwise>"#;
    let document = assert_round_trip(source);
    let attributes = partwise(&document).parts().next().unwrap().measures().next().unwrap().attributes().next().unwrap();
    let key = attributes.keys().next().unwrap();
    assert_eq!(key.number(), Some(2));
    assert_eq!(key.non_traditional_steps(), vec![NonTraditionalKeyStep {
        step: "F".into(), alter: 0.5, accidental: Some("quarter-sharp".into())
    }]);
    assert_eq!(attributes.times().next().unwrap().senza_misura().as_deref(), Some("free"));
}

#[test]
fn mxl_uses_container_rootfile_and_round_trips() {
    let document = parse_musicxml(COMPLEX_PARTWISE).unwrap();
    let bytes = write_mxl_with_rootfile(&document, "scores/nested/main.xml").unwrap();
    assert!(bytes.starts_with(b"PK\x03\x04"));
    let parsed = parse_mxl(&bytes).unwrap();
    assert_eq!(document, parsed);

    let default_bytes = document.to_mxl_bytes().unwrap();
    assert_eq!(MusicXmlDocument::from_mxl_bytes(&default_bytes).unwrap(), document);
}

#[test]
fn mxl_reader_accepts_a_zip_comment() {
    let document = parse_musicxml(SIMPLE_TIMEWISE).unwrap();
    let mut bytes = write_mxl(&document).unwrap();
    let old_len = bytes.len();
    bytes[old_len - 2..].copy_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(b"note");
    assert_eq!(parse_mxl(&bytes).unwrap(), document);
}

#[test]
fn malformed_inputs_return_typed_errors_without_panics() {
    let cases = [
        ("<score-partwise><part-list></score-timewise>", XmlErrorKind::MismatchedClosingTag),
        ("<score-partwise a=\"1\" a=\"2\"/>", XmlErrorKind::DuplicateAttribute),
        ("<score-partwise>&bogus;</score-partwise>", XmlErrorKind::InvalidEntity),
        ("<score-partwise><!-- bad -- comment --></score-partwise>", XmlErrorKind::InvalidComment),
        ("<score-partwise>", XmlErrorKind::UnexpectedEnd),
    ];
    for (source, expected) in cases {
        match parse_musicxml(source) {
            Err(MusicXmlError::Xml(error)) => assert_eq!(error.kind, expected, "{source}"),
            other => panic!("expected typed XML error for {source:?}, got {other:?}"),
        }
    }
    assert!(matches!(
        parse_musicxml("<not-a-score/>"),
        Err(MusicXmlError::InvalidRoot(_))
    ));
    assert!(matches!(
        parse_mxl(b"not a zip"),
        Err(MusicXmlError::InvalidMxl(_))
    ));
    assert!(matches!(
        parse_musicxml("<score-timewise><measure number=\"1\"/></score-timewise>")
            .unwrap()
            .into_partwise(),
        Err(ConversionError::UnsupportedStructure(_))
    ));
}

#[test]
fn attribute_escaping_and_numeric_entities_are_stable() {
    let source = "<score-partwise version=\"4.0\" id=\"a&amp;b&#10;c&#9;d\"><part-list/></score-partwise>";
    let document = assert_round_trip(source);
    assert_eq!(document.root().id(), Some("a&b\nc\td"));
    let written = document.to_xml_string().unwrap();
    assert!(written.contains("id=\"a&amp;b&#10;c&#9;d\""));
}

#[test]
fn plain_utf16_musicxml_bytes_are_supported() {
    let source = "<?xml version=\"1.0\" encoding=\"UTF-16\"?><score-partwise version=\"4.0\"><part-list/></score-partwise>";
    let mut bytes = vec![0xff, 0xfe];
    for unit in source.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let document = parse_musicxml_bytes(&bytes).unwrap();
    assert_eq!(document.format(), ScoreFormat::Partwise);
    assert_eq!(document.declaration.encoding.as_deref(), Some("UTF-8"));
}
