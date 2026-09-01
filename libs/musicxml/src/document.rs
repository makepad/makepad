use crate::{
    parse_xml, write_xml, ConversionError, MusicXmlError, MusicXmlResult, XmlDeclaration,
    XmlDocument, XmlElement, XmlNode,
};
use std::collections::{BTreeMap, BTreeSet};

pub const PARTWISE_DOCTYPE: &str = "score-partwise PUBLIC \"-//Recordare//DTD MusicXML 4.0 Partwise//EN\" \"http://www.musicxml.org/dtds/partwise.dtd\"";
pub const TIMEWISE_DOCTYPE: &str = "score-timewise PUBLIC \"-//Recordare//DTD MusicXML 4.0 Timewise//EN\" \"http://www.musicxml.org/dtds/timewise.dtd\"";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MusicXmlDocument {
    pub declaration: XmlDeclaration,
    pub doctype: String,
    pub before_score: Vec<XmlNode>,
    pub score: Score,
    pub after_score: Vec<XmlNode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Score {
    Partwise(ScorePartwise),
    Timewise(ScoreTimewise),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScorePartwise {
    pub element: XmlElement,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreTimewise {
    pub element: XmlElement,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScoreFormat {
    Partwise,
    Timewise,
}

impl MusicXmlDocument {
    pub fn parse(source: &str) -> MusicXmlResult<Self> {
        let xml = parse_xml(source)?;
        Self::from_xml_document(xml)
    }

    pub fn from_xml_document(mut xml: XmlDocument) -> MusicXmlResult<Self> {
        let format = match xml.root.name.as_str() {
            "score-partwise" => ScoreFormat::Partwise,
            "score-timewise" => ScoreFormat::Timewise,
            name => return Err(MusicXmlError::InvalidRoot(name.to_string())),
        };
        // Output from this crate is UTF-8. Encoding is a transport detail, so
        // normalize it while retaining XML version and standalone semantics.
        xml.declaration.encoding = Some("UTF-8".to_string());
        let doctype = xml.doctype.unwrap_or_else(|| match format {
            ScoreFormat::Partwise => PARTWISE_DOCTYPE.to_string(),
            ScoreFormat::Timewise => TIMEWISE_DOCTYPE.to_string(),
        });
        let score = match format {
            ScoreFormat::Partwise => Score::Partwise(ScorePartwise { element: xml.root }),
            ScoreFormat::Timewise => Score::Timewise(ScoreTimewise { element: xml.root }),
        };
        Ok(Self {
            declaration: xml.declaration,
            doctype,
            before_score: xml.before_root,
            score,
            after_score: xml.after_root,
        })
    }

    pub fn to_xml_document(&self) -> XmlDocument {
        XmlDocument {
            declaration: self.declaration.clone(),
            doctype: Some(self.doctype.clone()),
            before_root: self.before_score.clone(),
            root: self.score.element().clone(),
            after_root: self.after_score.clone(),
        }
    }

    pub fn into_xml_document(self) -> XmlDocument {
        XmlDocument {
            declaration: self.declaration,
            doctype: Some(self.doctype),
            before_root: self.before_score,
            root: self.score.into_element(),
            after_root: self.after_score,
        }
    }

    pub fn to_xml_string(&self) -> MusicXmlResult<String> {
        Ok(write_xml(&self.to_xml_document())?)
    }

    pub fn format(&self) -> ScoreFormat {
        self.score.format()
    }

    pub fn root(&self) -> &XmlElement {
        self.score.element()
    }

    pub fn root_mut(&mut self) -> &mut XmlElement {
        self.score.element_mut()
    }

    pub fn version(&self) -> Option<&str> {
        self.root().attr("version")
    }

    pub fn to_partwise(&self) -> Result<Self, ConversionError> {
        self.clone().into_partwise()
    }

    pub fn into_partwise(mut self) -> Result<Self, ConversionError> {
        if let Score::Timewise(score) = self.score {
            self.score = Score::Partwise(timewise_to_partwise(score)?);
            self.doctype = PARTWISE_DOCTYPE.to_string();
        }
        Ok(self)
    }

    pub fn to_timewise(&self) -> Result<Self, ConversionError> {
        self.clone().into_timewise()
    }

    pub fn into_timewise(mut self) -> Result<Self, ConversionError> {
        if let Score::Partwise(score) = self.score {
            self.score = Score::Timewise(partwise_to_timewise(score)?);
            self.doctype = TIMEWISE_DOCTYPE.to_string();
        }
        Ok(self)
    }

    /// Returns every element whose name is outside the MusicXML 4.0 names
    /// recognized by this document tier. Namespaced extension elements are
    /// intentionally reported here while still being retained in the tree.
    pub fn unmodelled_elements(&self) -> Vec<&XmlElement> {
        fn visit<'a>(element: &'a XmlElement, out: &mut Vec<&'a XmlElement>) {
            if !is_known_musicxml_element(&element.name) {
                out.push(element);
            }
            for child in element.child_elements() {
                visit(child, out);
            }
        }
        let mut out = Vec::new();
        visit(self.root(), &mut out);
        out
    }
}

impl Score {
    pub fn format(&self) -> ScoreFormat {
        match self {
            Self::Partwise(_) => ScoreFormat::Partwise,
            Self::Timewise(_) => ScoreFormat::Timewise,
        }
    }

    pub fn element(&self) -> &XmlElement {
        match self {
            Self::Partwise(score) => &score.element,
            Self::Timewise(score) => &score.element,
        }
    }

    pub fn element_mut(&mut self) -> &mut XmlElement {
        match self {
            Self::Partwise(score) => &mut score.element,
            Self::Timewise(score) => &mut score.element,
        }
    }

    pub fn into_element(self) -> XmlElement {
        match self {
            Self::Partwise(score) => score.element,
            Self::Timewise(score) => score.element,
        }
    }

    pub fn part_list(&self) -> Option<PartListRef<'_>> {
        self.element().first_child("part-list").map(PartListRef)
    }

    pub fn header_items(&self) -> impl Iterator<Item = HeaderItemRef<'_>> {
        self.element().child_elements().filter_map(header_item)
    }
}

impl ScorePartwise {
    pub fn parts(&self) -> impl Iterator<Item = PartRef<'_>> {
        self.element.children_named("part").map(PartRef)
    }

    pub fn part(&self, id: &str) -> Option<PartRef<'_>> {
        self.parts().find(|part| part.id() == Some(id))
    }
}

impl ScoreTimewise {
    pub fn measures(&self) -> impl Iterator<Item = TimewiseMeasureRef<'_>> {
        self.element
            .children_named("measure")
            .map(TimewiseMeasureRef)
    }
}

macro_rules! element_ref {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name<'a>(pub &'a XmlElement);

        impl<'a> $name<'a> {
            pub fn element(self) -> &'a XmlElement {
                self.0
            }

            pub fn id(self) -> Option<&'a str> {
                self.0.id()
            }
        }
    };
}

element_ref!(PartListRef);
element_ref!(ScorePartRef);
element_ref!(PartGroupRef);
element_ref!(PartRef);
element_ref!(MeasureRef);
element_ref!(TimewiseMeasureRef);
element_ref!(TimewisePartRef);
element_ref!(AttributesRef);
element_ref!(NotationsRef);
element_ref!(LyricRef);
element_ref!(DirectionRef);
element_ref!(HarmonyRef);
element_ref!(FiguredBassRef);
element_ref!(BarlineRef);
element_ref!(PrintRef);
element_ref!(SoundRef);
element_ref!(StaffDetailsRef);
element_ref!(MeasureStyleRef);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderItemRef<'a> {
    Work(&'a XmlElement),
    MovementNumber(&'a XmlElement),
    MovementTitle(&'a XmlElement),
    Identification(&'a XmlElement),
    Defaults(&'a XmlElement),
    Credit(&'a XmlElement),
    PartList(PartListRef<'a>),
}

fn header_item(element: &XmlElement) -> Option<HeaderItemRef<'_>> {
    Some(match element.name.as_str() {
        "work" => HeaderItemRef::Work(element),
        "movement-number" => HeaderItemRef::MovementNumber(element),
        "movement-title" => HeaderItemRef::MovementTitle(element),
        "identification" => HeaderItemRef::Identification(element),
        "defaults" => HeaderItemRef::Defaults(element),
        "credit" => HeaderItemRef::Credit(element),
        "part-list" => HeaderItemRef::PartList(PartListRef(element)),
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartListItemRef<'a> {
    ScorePart(ScorePartRef<'a>),
    PartGroup(PartGroupRef<'a>),
    Unknown(&'a XmlElement),
}

impl<'a> PartListRef<'a> {
    pub fn items(self) -> impl Iterator<Item = PartListItemRef<'a>> {
        self.0.child_elements().map(|element| match element.name.as_str() {
            "score-part" => PartListItemRef::ScorePart(ScorePartRef(element)),
            "part-group" => PartListItemRef::PartGroup(PartGroupRef(element)),
            _ => PartListItemRef::Unknown(element),
        })
    }

    pub fn score_parts(self) -> impl Iterator<Item = ScorePartRef<'a>> {
        self.0.children_named("score-part").map(ScorePartRef)
    }
}

impl<'a> ScorePartRef<'a> {
    pub fn name(self) -> Option<String> {
        self.0.first_child("part-name").map(XmlElement::direct_text)
    }

    pub fn score_instruments(self) -> impl Iterator<Item = &'a XmlElement> {
        self.0.children_named("score-instrument")
    }

    pub fn midi_instruments(self) -> impl Iterator<Item = &'a XmlElement> {
        self.0.children_named("midi-instrument")
    }
}

impl<'a> PartRef<'a> {
    pub fn measures(self) -> impl Iterator<Item = MeasureRef<'a>> {
        self.0.children_named("measure").map(MeasureRef)
    }
}

impl<'a> TimewiseMeasureRef<'a> {
    pub fn number(self) -> Option<&'a str> {
        self.0.attr("number")
    }

    pub fn parts(self) -> impl Iterator<Item = TimewisePartRef<'a>> {
        self.0.children_named("part").map(TimewisePartRef)
    }
}

impl<'a> TimewisePartRef<'a> {
    pub fn items(self) -> impl Iterator<Item = MeasureItemRef<'a>> {
        self.0.child_elements().map(measure_item)
    }
}

impl<'a> MeasureRef<'a> {
    pub fn number(self) -> Option<&'a str> {
        self.0.attr("number")
    }

    pub fn items(self) -> impl Iterator<Item = MeasureItemRef<'a>> {
        self.0.child_elements().map(measure_item)
    }

    pub fn notes(self) -> impl Iterator<Item = NoteRef<'a>> {
        self.0.children_named("note").map(NoteRef)
    }

    pub fn attributes(self) -> impl Iterator<Item = AttributesRef<'a>> {
        self.0.children_named("attributes").map(AttributesRef)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasureItemRef<'a> {
    Attributes(AttributesRef<'a>),
    Note(NoteRef<'a>),
    Backup(&'a XmlElement),
    Forward(&'a XmlElement),
    Direction(DirectionRef<'a>),
    Harmony(HarmonyRef<'a>),
    FiguredBass(FiguredBassRef<'a>),
    Print(PrintRef<'a>),
    Sound(SoundRef<'a>),
    Barline(BarlineRef<'a>),
    Grouping(&'a XmlElement),
    Link(&'a XmlElement),
    Bookmark(&'a XmlElement),
    Unknown(&'a XmlElement),
}

fn measure_item(element: &XmlElement) -> MeasureItemRef<'_> {
    match element.name.as_str() {
        "attributes" => MeasureItemRef::Attributes(AttributesRef(element)),
        "note" => MeasureItemRef::Note(NoteRef(element)),
        "backup" => MeasureItemRef::Backup(element),
        "forward" => MeasureItemRef::Forward(element),
        "direction" => MeasureItemRef::Direction(DirectionRef(element)),
        "harmony" => MeasureItemRef::Harmony(HarmonyRef(element)),
        "figured-bass" => MeasureItemRef::FiguredBass(FiguredBassRef(element)),
        "print" => MeasureItemRef::Print(PrintRef(element)),
        "sound" => MeasureItemRef::Sound(SoundRef(element)),
        "barline" => MeasureItemRef::Barline(BarlineRef(element)),
        "grouping" => MeasureItemRef::Grouping(element),
        "link" => MeasureItemRef::Link(element),
        "bookmark" => MeasureItemRef::Bookmark(element),
        _ => MeasureItemRef::Unknown(element),
    }
}

impl<'a> AttributesRef<'a> {
    pub fn divisions(self) -> Option<u32> {
        child_parse(self.0, "divisions")
    }

    pub fn keys(self) -> impl Iterator<Item = KeySignatureRef<'a>> {
        self.0.children_named("key").map(KeySignatureRef)
    }

    pub fn times(self) -> impl Iterator<Item = TimeSignatureRef<'a>> {
        self.0.children_named("time").map(TimeSignatureRef)
    }

    pub fn clefs(self) -> impl Iterator<Item = ClefRef<'a>> {
        self.0.children_named("clef").map(ClefRef)
    }

    pub fn staff_details(self) -> impl Iterator<Item = StaffDetailsRef<'a>> {
        self.0.children_named("staff-details").map(StaffDetailsRef)
    }

    pub fn transposes(self) -> impl Iterator<Item = TransposeRef<'a>> {
        self.0.children_named("transpose").map(TransposeRef)
    }

    pub fn measure_styles(self) -> impl Iterator<Item = MeasureStyleRef<'a>> {
        self.0.children_named("measure-style").map(MeasureStyleRef)
    }

    pub fn staves(self) -> Option<u32> {
        child_parse(self.0, "staves")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeySignatureRef<'a>(pub &'a XmlElement);

#[derive(Clone, Debug, PartialEq)]
pub struct NonTraditionalKeyStep {
    pub step: String,
    pub alter: f64,
    pub accidental: Option<String>,
}

impl KeySignatureRef<'_> {
    pub fn fifths(self) -> Option<i16> {
        child_parse(self.0, "fifths")
    }

    pub fn mode(self) -> Option<String> {
        child_text(self.0, "mode")
    }

    pub fn cancel(self) -> Option<i16> {
        child_parse(self.0, "cancel")
    }

    pub fn number(self) -> Option<u32> {
        attr_parse(self.0, "number")
    }

    pub fn non_traditional_steps(self) -> Vec<NonTraditionalKeyStep> {
        let children: Vec<_> = self.0.child_elements().collect();
        let mut result = Vec::new();
        let mut index = 0;
        while index < children.len() {
            if children[index].name == "key-step" {
                if let Some(alter) = children.get(index + 1).filter(|e| e.name == "key-alter") {
                    if let Ok(alter) = alter.direct_text().trim().parse() {
                        let accidental = children
                            .get(index + 2)
                            .filter(|e| e.name == "key-accidental")
                            .map(|e| e.direct_text());
                        result.push(NonTraditionalKeyStep {
                            step: children[index].direct_text(),
                            alter,
                            accidental,
                        });
                    }
                }
            }
            index += 1;
        }
        result
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeSignatureRef<'a>(pub &'a XmlElement);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeComponent {
    pub beats: String,
    pub beat_type: String,
}

impl<'a> TimeSignatureRef<'a> {
    pub fn components(self) -> Vec<TimeComponent> {
        let elements: Vec<_> = self.0.child_elements().collect();
        let mut result = Vec::new();
        let mut index = 0;
        while index < elements.len() {
            if elements[index].name == "beats" {
                if let Some(beat_type) = elements
                    .get(index + 1)
                    .filter(|element| element.name == "beat-type")
                {
                    result.push(TimeComponent {
                        beats: elements[index].direct_text(),
                        beat_type: beat_type.direct_text(),
                    });
                    index += 1;
                }
            }
            index += 1;
        }
        result
    }

    pub fn senza_misura(self) -> Option<String> {
        child_text(self.0, "senza-misura")
    }

    pub fn symbol(self) -> Option<&'a str> {
        self.0.attr("symbol")
    }

    pub fn number(self) -> Option<u32> {
        attr_parse(self.0, "number")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClefRef<'a>(pub &'a XmlElement);

impl ClefRef<'_> {
    pub fn sign(self) -> Option<String> {
        child_text(self.0, "sign")
    }

    pub fn line(self) -> Option<u8> {
        child_parse(self.0, "line")
    }

    pub fn octave_change(self) -> Option<i8> {
        child_parse(self.0, "clef-octave-change")
    }

    pub fn number(self) -> Option<u32> {
        attr_parse(self.0, "number")
    }

    pub fn additional(self) -> bool {
        self.0.attr("additional") == Some("yes")
    }
}

impl StaffDetailsRef<'_> {
    pub fn number(self) -> Option<u32> {
        attr_parse(self.0, "number")
    }

    pub fn staff_type(self) -> Option<String> {
        child_text(self.0, "staff-type")
    }

    pub fn staff_lines(self) -> Option<u32> {
        child_parse(self.0, "staff-lines")
    }

    pub fn capo(self) -> Option<u32> {
        child_parse(self.0, "capo")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransposeRef<'a>(pub &'a XmlElement);

impl TransposeRef<'_> {
    pub fn number(self) -> Option<u32> {
        attr_parse(self.0, "number")
    }

    pub fn diatonic(self) -> Option<i16> {
        child_parse(self.0, "diatonic")
    }

    pub fn chromatic(self) -> Option<f64> {
        child_parse(self.0, "chromatic")
    }

    pub fn octave_change(self) -> Option<i8> {
        child_parse(self.0, "octave-change")
    }

    pub fn doubled(self) -> bool {
        self.0.first_child("double").is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteRef<'a>(pub &'a XmlElement);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pitch {
    pub step: Step,
    pub alter: Option<f64>,
    pub octave: i8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rest {
    pub measure: bool,
    pub display_step: Option<Step>,
    pub display_octave: Option<i8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unpitched {
    pub display_step: Option<Step>,
    pub display_octave: Option<i8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NoteKind {
    Pitch(Pitch),
    Rest(Rest),
    Unpitched(Unpitched),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartStopContinue {
    Start,
    Stop,
    Continue,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TieRef<'a> {
    pub kind: StartStopContinue,
    pub element: &'a XmlElement,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accidental {
    pub value: String,
    pub cautionary: bool,
    pub editorial: bool,
    pub parentheses: bool,
    pub bracket: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeModification {
    pub actual_notes: u32,
    pub normal_notes: u32,
    pub normal_type: Option<String>,
    pub normal_dots: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Beam<'a> {
    pub number: Option<u8>,
    pub value: String,
    pub element: &'a XmlElement,
}

impl<'a> NoteRef<'a> {
    pub fn element(self) -> &'a XmlElement {
        self.0
    }

    pub fn id(self) -> Option<&'a str> {
        self.0.id()
    }

    pub fn kind(self) -> Option<NoteKind> {
        if let Some(pitch) = self.0.first_child("pitch") {
            let step = child_text(pitch, "step").and_then(|value| parse_step(&value))?;
            let octave = child_parse(pitch, "octave")?;
            return Some(NoteKind::Pitch(Pitch {
                step,
                alter: child_parse(pitch, "alter"),
                octave,
            }));
        }
        if let Some(rest) = self.0.first_child("rest") {
            return Some(NoteKind::Rest(Rest {
                measure: rest.attr("measure") == Some("yes"),
                display_step: child_text(rest, "display-step")
                    .and_then(|value| parse_step(&value)),
                display_octave: child_parse(rest, "display-octave"),
            }));
        }
        self.0.first_child("unpitched").map(|unpitched| {
            NoteKind::Unpitched(Unpitched {
                display_step: child_text(unpitched, "display-step")
                    .and_then(|value| parse_step(&value)),
                display_octave: child_parse(unpitched, "display-octave"),
            })
        })
    }

    pub fn duration(self) -> Option<u32> {
        child_parse(self.0, "duration")
    }

    /// Sounding ties (`note/tie`), deliberately separate from notation ties.
    pub fn ties(self) -> impl Iterator<Item = TieRef<'a>> {
        self.0.children_named("tie").map(|element| TieRef {
            kind: start_stop_continue(element.attr("type")),
            element,
        })
    }

    pub fn voice(self) -> Option<String> {
        child_text(self.0, "voice")
    }

    pub fn note_type(self) -> Option<String> {
        child_text(self.0, "type")
    }

    pub fn dots(self) -> usize {
        self.0.children_named("dot").count()
    }

    pub fn accidental(self) -> Option<Accidental> {
        let accidental = self.0.first_child("accidental")?;
        Some(Accidental {
            value: accidental.direct_text(),
            cautionary: yes(accidental.attr("cautionary")),
            editorial: yes(accidental.attr("editorial")),
            parentheses: yes(accidental.attr("parentheses")),
            bracket: yes(accidental.attr("bracket")),
        })
    }

    pub fn time_modification(self) -> Option<TimeModification> {
        let value = self.0.first_child("time-modification")?;
        Some(TimeModification {
            actual_notes: child_parse(value, "actual-notes")?,
            normal_notes: child_parse(value, "normal-notes")?,
            normal_type: child_text(value, "normal-type"),
            normal_dots: value.children_named("normal-dot").count(),
        })
    }

    pub fn stem(self) -> Option<String> {
        child_text(self.0, "stem")
    }

    pub fn notehead(self) -> Option<&'a XmlElement> {
        self.0.first_child("notehead")
    }

    pub fn staff(self) -> Option<u32> {
        child_parse(self.0, "staff")
    }

    pub fn beams(self) -> impl Iterator<Item = Beam<'a>> {
        self.0.children_named("beam").map(|element| Beam {
            number: attr_parse(element, "number"),
            value: element.direct_text(),
            element,
        })
    }

    pub fn is_chord(self) -> bool {
        self.0.first_child("chord").is_some()
    }

    pub fn is_grace(self) -> bool {
        self.0.first_child("grace").is_some()
    }

    pub fn grace(self) -> Option<&'a XmlElement> {
        self.0.first_child("grace")
    }

    pub fn is_cue(self) -> bool {
        self.0.first_child("cue").is_some()
    }

    pub fn instrument_id(self) -> Option<&'a str> {
        self.0.first_child("instrument").and_then(|e| e.attr("id"))
    }

    pub fn notations(self) -> impl Iterator<Item = NotationsRef<'a>> {
        self.0.children_named("notations").map(NotationsRef)
    }

    pub fn lyrics(self) -> impl Iterator<Item = LyricRef<'a>> {
        self.0.children_named("lyric").map(LyricRef)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotationItemRef<'a> {
    Tied(TieRef<'a>),
    Slur(&'a XmlElement),
    Tuplet(&'a XmlElement),
    Glissando(&'a XmlElement),
    Slide(&'a XmlElement),
    Ornaments(&'a XmlElement),
    Technical(&'a XmlElement),
    Articulations(&'a XmlElement),
    Dynamics(&'a XmlElement),
    Fermata(&'a XmlElement),
    Arpeggiate(&'a XmlElement),
    NonArpeggiate(&'a XmlElement),
    AccidentalMark(&'a XmlElement),
    OtherNotation(&'a XmlElement),
    Unknown(&'a XmlElement),
}

impl<'a> NotationsRef<'a> {
    pub fn items(self) -> impl Iterator<Item = NotationItemRef<'a>> {
        self.0.child_elements().map(|element| match element.name.as_str() {
            "tied" => NotationItemRef::Tied(TieRef {
                kind: start_stop_continue(element.attr("type")),
                element,
            }),
            "slur" => NotationItemRef::Slur(element),
            "tuplet" => NotationItemRef::Tuplet(element),
            "glissando" => NotationItemRef::Glissando(element),
            "slide" => NotationItemRef::Slide(element),
            "ornaments" => NotationItemRef::Ornaments(element),
            "technical" => NotationItemRef::Technical(element),
            "articulations" => NotationItemRef::Articulations(element),
            "dynamics" => NotationItemRef::Dynamics(element),
            "fermata" => NotationItemRef::Fermata(element),
            "arpeggiate" => NotationItemRef::Arpeggiate(element),
            "non-arpeggiate" => NotationItemRef::NonArpeggiate(element),
            "accidental-mark" => NotationItemRef::AccidentalMark(element),
            "other-notation" => NotationItemRef::OtherNotation(element),
            _ => NotationItemRef::Unknown(element),
        })
    }
}

impl<'a> LyricRef<'a> {
    pub fn number(self) -> Option<&'a str> {
        self.0.attr("number")
    }

    pub fn name(self) -> Option<&'a str> {
        self.0.attr("name")
    }

    pub fn syllabic(self) -> Option<String> {
        child_text(self.0, "syllabic")
    }

    pub fn texts(self) -> impl Iterator<Item = String> + 'a {
        self.0.children_named("text").map(XmlElement::direct_text)
    }

    pub fn elisions(self) -> impl Iterator<Item = String> + 'a {
        self.0
            .children_named("elision")
            .map(XmlElement::direct_text)
    }

    pub fn has_extend(self) -> bool {
        self.0.first_child("extend").is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectionTypeItemRef<'a> {
    Words(&'a XmlElement),
    Dynamics(&'a XmlElement),
    Wedge(&'a XmlElement),
    Metronome(&'a XmlElement),
    OctaveShift(&'a XmlElement),
    Pedal(&'a XmlElement),
    Dashes(&'a XmlElement),
    Bracket(&'a XmlElement),
    Rehearsal(&'a XmlElement),
    Segno(&'a XmlElement),
    Coda(&'a XmlElement),
    HarpPedals(&'a XmlElement),
    Symbol(&'a XmlElement),
    Image(&'a XmlElement),
    PrincipalVoice(&'a XmlElement),
    AccordionRegistration(&'a XmlElement),
    Percussion(&'a XmlElement),
    OtherDirection(&'a XmlElement),
    Unknown(&'a XmlElement),
}

impl<'a> DirectionRef<'a> {
    pub fn placement(self) -> Option<&'a str> {
        self.0.attr("placement")
    }

    pub fn staff(self) -> Option<u32> {
        child_parse(self.0, "staff")
    }

    pub fn voice(self) -> Option<String> {
        child_text(self.0, "voice")
    }

    pub fn offset(self) -> Option<f64> {
        child_parse(self.0, "offset")
    }

    pub fn types(self) -> impl Iterator<Item = DirectionTypeItemRef<'a>> {
        self.0
            .children_named("direction-type")
            .flat_map(XmlElement::child_elements)
            .map(|element| match element.name.as_str() {
                "words" => DirectionTypeItemRef::Words(element),
                "dynamics" => DirectionTypeItemRef::Dynamics(element),
                "wedge" => DirectionTypeItemRef::Wedge(element),
                "metronome" => DirectionTypeItemRef::Metronome(element),
                "octave-shift" => DirectionTypeItemRef::OctaveShift(element),
                "pedal" => DirectionTypeItemRef::Pedal(element),
                "dashes" => DirectionTypeItemRef::Dashes(element),
                "bracket" => DirectionTypeItemRef::Bracket(element),
                "rehearsal" => DirectionTypeItemRef::Rehearsal(element),
                "segno" => DirectionTypeItemRef::Segno(element),
                "coda" => DirectionTypeItemRef::Coda(element),
                "harp-pedals" => DirectionTypeItemRef::HarpPedals(element),
                "symbol" => DirectionTypeItemRef::Symbol(element),
                "image" => DirectionTypeItemRef::Image(element),
                "principal-voice" => DirectionTypeItemRef::PrincipalVoice(element),
                "accordion-registration" => {
                    DirectionTypeItemRef::AccordionRegistration(element)
                }
                "percussion" => DirectionTypeItemRef::Percussion(element),
                "other-direction" => DirectionTypeItemRef::OtherDirection(element),
                _ => DirectionTypeItemRef::Unknown(element),
            })
    }
}

impl<'a> BarlineRef<'a> {
    pub fn location(self) -> Option<&'a str> {
        self.0.attr("location")
    }

    pub fn bar_style(self) -> Option<String> {
        child_text(self.0, "bar-style")
    }

    pub fn repeats(self) -> impl Iterator<Item = &'a XmlElement> {
        self.0.children_named("repeat")
    }

    pub fn endings(self) -> impl Iterator<Item = &'a XmlElement> {
        self.0.children_named("ending")
    }
}

fn timewise_to_partwise(score: ScoreTimewise) -> Result<ScorePartwise, ConversionError> {
    let mut root = score.element;
    let mut part_ids = Vec::new();
    if let Some(part_list) = root.first_child("part-list") {
        for score_part in part_list.children_named("score-part") {
            push_unique_part_id(&mut part_ids, required_id(score_part)?)?;
        }
    }
    for measure in root.children_named("measure") {
        for part in measure.children_named("part") {
            push_unique_part_id(&mut part_ids, required_id(part)?)?;
        }
        reject_significant_non_elements(measure, "timewise measure")?;
        if measure
            .child_elements()
            .any(|element| element.name != "part")
        {
            return Err(ConversionError::UnsupportedStructure(
                "a timewise measure contains a non-part element".to_string(),
            ));
        }
    }
    if part_ids.is_empty() && root.children_named("measure").next().is_some() {
        return Err(ConversionError::UnsupportedStructure(
            "a timewise score with measures but no parts cannot be converted losslessly"
                .to_string(),
        ));
    }

    let mut parts: BTreeMap<String, XmlElement> = part_ids
        .iter()
        .map(|id| {
            (
                id.clone(),
                XmlElement::new("part").with_attribute("id", id.clone()),
            )
        })
        .collect();

    for measure in root.children_named("measure") {
        let mut seen = BTreeSet::new();
        for (part_index, time_part) in measure.children_named("part").enumerate() {
            let id = required_id(time_part)?.to_string();
            if !seen.insert(id.clone()) {
                return Err(ConversionError::DuplicatePartId(id));
            }
            if time_part.attributes.len() != 1 {
                return Err(ConversionError::UnsupportedStructure(
                    "timewise part attributes other than id cannot be mapped losslessly"
                        .to_string(),
                ));
            }
            let mut converted_measure = XmlElement::new("measure");
            converted_measure.attributes = measure
                .attributes
                .iter()
                .filter(|attribute| part_index == 0 || attribute.name != "id")
                .cloned()
                .collect();
            converted_measure.children = time_part.children.clone();
            parts
                .get_mut(&id)
                .ok_or_else(|| {
                    ConversionError::UnsupportedStructure(
                        "internal part index became inconsistent during conversion".to_string(),
                    )
                })?
                .push_element(converted_measure);
        }
        if seen.len() != part_ids.len() {
            return Err(ConversionError::UnsupportedStructure(
                "each timewise measure must contain every declared part for lossless conversion"
                    .to_string(),
            ));
        }
    }

    let mut inserted = false;
    let mut new_children = Vec::new();
    for node in root.children {
        if matches!(&node, XmlNode::Element(element) if element.name == "measure") {
            if !inserted {
                for id in &part_ids {
                    new_children.push(XmlNode::Element(parts.remove(id).ok_or_else(|| {
                        ConversionError::UnsupportedStructure(
                            "internal part index became inconsistent during conversion".to_string(),
                        )
                    })?));
                }
                inserted = true;
            }
        } else {
            new_children.push(node);
        }
    }
    root.name = "score-partwise".to_string();
    root.children = new_children;
    coalesce_adjacent_text(&mut root);
    Ok(ScorePartwise { element: root })
}

fn partwise_to_timewise(score: ScorePartwise) -> Result<ScoreTimewise, ConversionError> {
    let mut root = score.element;
    let part_elements: Vec<_> = root.children_named("part").cloned().collect();
    let mut part_ids = Vec::new();
    let mut part_measures = Vec::new();
    for part in &part_elements {
        let id = required_id(part)?.to_string();
        push_unique_part_id(&mut part_ids, &id)?;
        if part.attributes.len() != 1 {
            return Err(ConversionError::UnsupportedStructure(
                "partwise part attributes other than id cannot be mapped losslessly".to_string(),
            ));
        }
        reject_significant_non_elements(part, "partwise part")?;
        if part
            .child_elements()
            .any(|element| element.name != "measure")
        {
            return Err(ConversionError::UnsupportedStructure(
                "a partwise part contains a non-measure element".to_string(),
            ));
        }
        part_measures.push(part.children_named("measure").cloned().collect::<Vec<_>>());
    }
    let count = part_measures.first().map_or(0, Vec::len);
    if part_measures.iter().any(|measures| measures.len() != count) {
        return Err(ConversionError::UnequalMeasureCounts);
    }

    let mut measures = Vec::new();
    for measure_index in 0..count {
        let attributes = merge_partwise_measure_attributes(&part_measures, measure_index)?;
        let mut measure = XmlElement::new("measure");
        measure.attributes = attributes;
        for (part_index, id) in part_ids.iter().enumerate() {
            let mut time_part = XmlElement::new("part").with_attribute("id", id);
            time_part.children = part_measures[part_index][measure_index].children.clone();
            measure.push_element(time_part);
        }
        measures.push(measure);
    }

    let mut inserted = false;
    let mut new_children = Vec::new();
    for node in root.children {
        if matches!(&node, XmlNode::Element(element) if element.name == "part") {
            if !inserted {
                new_children.extend(measures.drain(..).map(XmlNode::Element));
                inserted = true;
            }
        } else {
            new_children.push(node);
        }
    }
    root.name = "score-timewise".to_string();
    root.children = new_children;
    coalesce_adjacent_text(&mut root);
    Ok(ScoreTimewise { element: root })
}

fn merge_partwise_measure_attributes(
    part_measures: &[Vec<XmlElement>],
    measure_index: usize,
) -> Result<Vec<crate::XmlAttribute>, ConversionError> {
    let without_id = |element: &XmlElement| {
        element
            .attributes
            .iter()
            .filter(|attribute| attribute.name != "id")
            .cloned()
            .collect::<Vec<_>>()
    };
    let expected = without_id(&part_measures[0][measure_index]);
    if part_measures
        .iter()
        .any(|part| without_id(&part[measure_index]) != expected)
    {
        return Err(ConversionError::UnequalMeasureAttributes { measure_index });
    }
    let with_ids: Vec<_> = part_measures
        .iter()
        .filter(|part| part[measure_index].attr("id").is_some())
        .collect();
    if with_ids.len() > 1 {
        return Err(ConversionError::UnsupportedStructure(format!(
            "multiple document-unique measure ids occur at partwise measure index {measure_index}"
        )));
    }
    Ok(with_ids
        .first()
        .map_or_else(|| part_measures[0][measure_index].attributes.clone(), |part| {
            part[measure_index].attributes.clone()
        }))
}

fn coalesce_adjacent_text(element: &mut XmlElement) {
    for child in element.child_elements_mut() {
        coalesce_adjacent_text(child);
    }
    let mut merged = Vec::with_capacity(element.children.len());
    for node in std::mem::take(&mut element.children) {
        if let XmlNode::Text(text) = node {
            if let Some(XmlNode::Text(previous)) = merged.last_mut() {
                previous.push_str(&text);
            } else {
                merged.push(XmlNode::Text(text));
            }
        } else {
            merged.push(node);
        }
    }
    element.children = merged;
}

fn reject_significant_non_elements(
    element: &XmlElement,
    context: &str,
) -> Result<(), ConversionError> {
    if element.children.iter().any(|node| match node {
        XmlNode::Text(text) => !text.chars().all(char::is_whitespace),
        XmlNode::Element(_) => false,
        XmlNode::CData(_) | XmlNode::Comment(_) | XmlNode::ProcessingInstruction { .. } => true,
    }) {
        return Err(ConversionError::UnsupportedStructure(format!(
            "{context} has non-element content that cannot be mapped losslessly"
        )));
    }
    Ok(())
}

fn required_id(element: &XmlElement) -> Result<&str, ConversionError> {
    element.attr("id").ok_or(ConversionError::MissingPartId)
}

fn push_unique_part_id(ids: &mut Vec<String>, id: &str) -> Result<(), ConversionError> {
    if ids.iter().any(|existing| existing == id) {
        return Ok(());
    }
    ids.push(id.to_string());
    Ok(())
}

fn child_text(element: &XmlElement, name: &str) -> Option<String> {
    element.first_child(name).map(XmlElement::direct_text)
}

fn child_parse<T: std::str::FromStr>(element: &XmlElement, name: &str) -> Option<T> {
    child_text(element, name)?.trim().parse().ok()
}

fn attr_parse<T: std::str::FromStr>(element: &XmlElement, name: &str) -> Option<T> {
    element.attr(name)?.trim().parse().ok()
}

fn parse_step(value: &str) -> Option<Step> {
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

fn yes(value: Option<&str>) -> bool {
    value == Some("yes")
}

fn start_stop_continue(value: Option<&str>) -> StartStopContinue {
    match value {
        Some("start") => StartStopContinue::Start,
        Some("stop") => StartStopContinue::Stop,
        Some("continue") => StartStopContinue::Continue,
        _ => StartStopContinue::Other,
    }
}

/// Whether a name belongs to the MusicXML 4.0 vocabulary understood by this
/// crate. The complete XML node remains available even for recognized names.
pub fn is_known_musicxml_element(name: &str) -> bool {
    matches!(name,
        "score-partwise" | "score-timewise" | "work" | "work-number" | "work-title" |
        "opus" | "movement-number" | "movement-title" | "identification" | "creator" |
        "rights" | "encoding" | "encoding-date" | "encoder" | "software" |
        "encoding-description" | "supports" | "source" | "relation" | "miscellaneous" |
        "miscellaneous-field" | "defaults" | "scaling" | "millimeters" | "tenths" |
        "page-layout" | "page-height" | "page-width" | "page-margins" | "left-margin" |
        "right-margin" | "top-margin" | "bottom-margin" | "system-layout" | "system-margins" |
        "system-distance" | "top-system-distance" | "system-dividers" | "left-divider" |
        "right-divider" | "staff-layout" | "staff-distance" | "appearance" | "line-width" |
        "note-size" | "distance" | "glyph" | "other-appearance" | "music-font" |
        "word-font" | "lyric-font" | "lyric-language" | "credit" | "credit-type" |
        "credit-words" | "credit-symbol" | "credit-image" | "link" | "bookmark" |
        "part-list" | "part-group" | "group-name" | "group-name-display" |
        "group-abbreviation" | "group-abbreviation-display" | "group-symbol" |
        "group-barline" | "group-time" | "score-part" | "part-name" |
        "part-name-display" | "part-abbreviation" | "part-abbreviation-display" |
        "group" | "score-instrument" | "instrument-name" | "instrument-abbreviation" |
        "instrument-sound" | "solo" | "ensemble" | "virtual-instrument" |
        "virtual-library" | "virtual-name" | "midi-device" | "midi-instrument" |
        "midi-channel" | "midi-name" | "midi-bank" | "midi-program" | "midi-unpitched" |
        "volume" | "pan" | "elevation" | "part" | "measure" | "attributes" |
        "footnote" | "level" | "divisions" | "key" | "cancel" | "fifths" | "mode" |
        "key-step" | "key-alter" | "key-accidental" | "time" | "beats" | "beat-type" |
        "senza-misura" | "staves" | "part-symbol" | "instruments" | "clef" | "sign" |
        "line" | "clef-octave-change" | "staff-details" | "staff-type" | "staff-lines" |
        "staff-tuning" | "tuning-step" | "tuning-alter" | "tuning-octave" | "capo" |
        "staff-size" | "transpose" | "diatonic" | "chromatic" | "octave-change" |
        "double" | "for-part" | "measure-style" | "multiple-rest" | "measure-repeat" |
        "beat-repeat" | "slash" | "slash-type" | "slash-dot" | "note" | "grace" |
        "cue" | "chord" | "pitch" | "step" | "alter" | "octave" | "unpitched" |
        "display-step" | "display-octave" | "rest" | "duration" | "tie" | "instrument" |
        "voice" | "type" | "dot" | "accidental" | "time-modification" | "actual-notes" |
        "normal-notes" | "normal-type" | "normal-dot" | "stem" | "notehead" |
        "notehead-text" | "staff" | "beam" | "notations" | "tied" | "slur" | "tuplet" |
        "tuplet-actual" | "tuplet-normal" | "tuplet-number" | "tuplet-type" |
        "tuplet-dot" | "glissando" | "slide" | "ornaments" | "trill-mark" | "turn" |
        "delayed-turn" | "inverted-turn" | "delayed-inverted-turn" | "vertical-turn" |
        "shake" | "wavy-line" | "mordent" | "inverted-mordent" | "schleifer" | "tremolo" |
        "haydn" | "other-ornament" | "accidental-mark" | "technical" | "up-bow" |
        "down-bow" | "harmonic" | "open-string" | "thumb-position" | "fingering" |
        "pluck" | "double-tongue" | "triple-tongue" | "stopped" | "snap-pizzicato" |
        "fret" | "string" | "hammer-on" | "pull-off" | "bend" | "bend-alter" |
        "pre-bend" | "release" | "with-bar" | "tap" | "heel" | "toe" | "fingernails" |
        "hole" | "arrow" | "handbell" | "brass-bend" | "flip" | "smear" | "open" |
        "half-muted" | "harmon-mute" | "golpe" | "other-technical" | "articulations" |
        "accent" | "strong-accent" | "staccato" | "tenuto" | "detached-legato" |
        "staccatissimo" | "spiccato" | "scoop" | "plop" | "doit" | "falloff" |
        "breath-mark" | "caesura" | "stress" | "unstress" | "soft-accent" |
        "other-articulation" | "dynamics" | "p" | "pp" | "ppp" | "pppp" | "ppppp" |
        "pppppp" | "f" | "ff" | "fff" | "ffff" | "fffff" | "ffffff" | "mp" | "mf" |
        "sf" | "sfp" | "sfpp" | "fp" | "rf" | "rfz" | "sfz" | "sffz" | "fz" |
        "n" | "pf" | "sfzp" | "other-dynamics" | "fermata" | "arpeggiate" |
        "non-arpeggiate" | "other-notation" | "lyric" | "syllabic" | "text" |
        "elision" | "extend" | "humming" | "laughing" | "end-line" | "end-paragraph" |
        "editorial" | "play" | "ipa" | "mute" | "semi-pitched" | "other-play" |
        "backup" | "forward" | "direction" | "direction-type" | "rehearsal" | "segno" |
        "coda" | "words" | "symbol" | "wedge" | "dashes" | "bracket" |
        "pedal" | "metronome" | "beat-unit" | "beat-unit-dot" | "per-minute" |
        "metronome-note" | "metronome-relation" | "metronome-tuplet" | "octave-shift" |
        "harp-pedals" | "pedal-tuning" | "pedal-step" | "pedal-alter" | "damp" |
        "damp-all" | "eyeglasses" | "string-mute" | "scordatura" | "accord" |
        "accordion-registration" | "accordion-high" | "accordion-middle" | "accordion-low" |
        "percussion" | "glass" | "metal" | "wood" | "pitched" | "membrane" |
        "effect" | "timpani" | "beater" | "stick" | "stick-location" | "other-percussion" |
        "staff-divide" | "principal-voice" | "image" | "other-direction" | "offset" |
        "harmony" | "root" | "root-step" | "root-alter" | "numeral" | "numeral-root" |
        "numeral-alter" | "function" | "kind" | "inversion" | "bass" | "bass-step" |
        "bass-alter" | "degree" | "degree-value" | "degree-alter" | "degree-type" |
        "frame" | "frame-strings" | "frame-frets" | "first-fret" | "frame-note" |
        "barre" | "figured-bass" | "figure" |
        "prefix" | "figure-number" | "suffix" | "print" | "measure-layout" |
        "measure-distance" | "sound" |
        "listen" | "listening" | "sync" | "other-listen" | "barline" | "bar-style" |
        "ending" | "repeat" |
        "grouping" | "feature"
    )
}
