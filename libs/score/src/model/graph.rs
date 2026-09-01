use super::annotation::{Annotation, AnnotationLayer, SourceRegion};
use super::id::*;
use super::maps::{GlobalMaps, KeySignature, Meter};
use super::ordered::OrderedMap;
use super::pitch::{Pitch, PitchProjection, Step, Transposition};
use super::playback::FlowGraph;
use super::time::{Alter, Duration, Rational, RationalError, ScoreTime};
use crate::symbol::{Articulation, Clef, DynamicMark, Ornament, Placement};
use makepad_micro_serde::{DeBin, DeBinErr, SerBin};

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct Score {
    pub score_id: [u8; 16],
    pub title: String,
    pub parts: OrderedMap<PartId, Part>,
    pub staves: OrderedMap<StaffId, Staff>,
    pub voices: OrderedMap<VoiceId, Voice>,
    pub measures: OrderedMap<MeasureId, Measure>,
    pub spanners: OrderedMap<SpannerId, Spanner>,
    pub lyrics: Vec<LyricSyllable>,
    pub maps: GlobalMaps,
    pub flow: FlowGraph,
    pub annotations: OrderedMap<AnnotationId, Annotation>,
    pub annotation_layers: OrderedMap<LayerId, AnnotationLayer>,
    pub source_regions: OrderedMap<SourceRegionId, SourceRegion>,
    pub part_views: OrderedMap<PartViewId, PartView>,
}

impl Score {
    pub fn new(score_id: [u8; 16]) -> Self {
        Self {
            score_id,
            title: String::new(),
            parts: OrderedMap::new(),
            staves: OrderedMap::new(),
            voices: OrderedMap::new(),
            measures: OrderedMap::new(),
            spanners: OrderedMap::new(),
            lyrics: Vec::new(),
            maps: GlobalMaps::default(),
            flow: FlowGraph::default(),
            annotations: OrderedMap::new(),
            annotation_layers: OrderedMap::new(),
            source_regions: OrderedMap::new(),
            part_views: OrderedMap::new(),
        }
    }

    pub fn event(&self, id: EventId) -> Option<&TimedEvent> {
        self.voices
            .values()
            .find_map(|voice| voice.events.iter().find(|event| event.id == id))
    }

    pub fn event_mut(&mut self, id: EventId) -> Option<&mut TimedEvent> {
        self.voices
            .values_mut()
            .find_map(|voice| voice.events.iter_mut().find(|event| event.id == id))
    }

    pub fn event_owner(&self, id: EventId) -> Option<VoiceId> {
        self.voices
            .iter()
            .find_map(|(voice_id, voice)| voice.events.iter().any(|event| event.id == id).then_some(*voice_id))
    }

    pub fn note(&self, id: NoteId) -> Option<&Note> {
        self.voices.values().find_map(|voice| {
            voice.events.iter().find_map(|event| match &event.kind {
                EventKind::Chord(notes) => notes.iter().find(|note| note.id == id),
                _ => None,
            })
        })
    }

    pub fn note_mut(&mut self, id: NoteId) -> Option<&mut Note> {
        self.voices.values_mut().find_map(|voice| {
            voice.events.iter_mut().find_map(|event| match &mut event.kind {
                EventKind::Chord(notes) => notes.iter_mut().find(|note| note.id == id),
                _ => None,
            })
        })
    }

    pub fn note_context(&self, id: NoteId) -> Option<(VoiceId, &TimedEvent, &Note)> {
        self.voices.iter().find_map(|(voice_id, voice)| {
            voice.events.iter().find_map(|event| match &event.kind {
                EventKind::Chord(notes) => notes
                    .iter()
                    .find(|note| note.id == id)
                    .map(|note| (*voice_id, event, note)),
                _ => None,
            })
        })
    }

    pub fn pitch_projection(
        &self,
        part: PartId,
        note: NoteId,
        concert_pitch: bool,
    ) -> Result<Option<PitchProjection>, RationalError> {
        let part = match self.parts.get(&part) {
            Some(part) => part,
            None => return Ok(None),
        };
        let note = match self.note(note).and_then(|note| note.written_pitch) {
            Some(note) => note,
            None => return Ok(None),
        };
        Ok(Some(PitchProjection::new(
            note,
            part.transposition,
            concert_pitch,
        )?))
    }

    pub fn project<'a>(&'a self, view: &'a PartView) -> PartProjection<'a> {
        PartProjection { score: self, view }
    }

    pub(crate) fn sort_voice(voice: &mut Voice) {
        voice.events.sort_by_key(|event| (event.onset, event.id));
    }
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct Part {
    pub id: PartId,
    pub name: String,
    pub staves: Vec<StaffId>,
    pub transposition: Transposition,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct Staff {
    pub id: StaffId,
    pub part: PartId,
    pub parent: Option<StaffId>,
    pub kind: StaffKind,
    pub voices: Vec<VoiceId>,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum StaffKind {
    Standard,
    Ossia,
    Tablature(Tuning),
    Percussion(PercussionMap),
    Unpitched,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct Tuning {
    pub strings_low_to_high: Vec<Pitch>,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct PercussionMap {
    pub entries: OrderedMap<u16, PercussionSound>,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct PercussionSound {
    pub name: String,
    pub midi_note: u8,
    pub display: Pitch,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct Voice {
    pub id: VoiceId,
    pub staff: StaffId,
    pub number: u16,
    pub events: Vec<TimedEvent>,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct Measure {
    pub id: MeasureId,
    pub ordinal: u32,
    pub label: String,
    pub start: ScoreTime,
    pub extent: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct TimedEvent {
    pub id: EventId,
    pub onset: ScoreTime,
    pub duration: Option<Duration>,
    pub grace: Option<GraceTiming>,
    pub kind: EventKind,
    pub beams: Vec<BeamMembership>,
    pub tuplets: Vec<TupletNotation>,
    pub articulations: Vec<PlacedArticulation>,
    pub ornaments: Vec<Ornament>,
}

impl TimedEvent {
    pub fn end(&self) -> Result<ScoreTime, RationalError> {
        match self.duration {
            Some(duration) if self.grace.is_none() => self.onset.checked_add(duration),
            _ => Ok(self.onset),
        }
    }

    pub fn chord_notes(&self) -> &[Note] {
        match &self.kind {
            EventKind::Chord(notes) => notes,
            _ => &[],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum EventKind {
    Chord(Vec<Note>),
    Rest,
    Direction(DirectionEvent),
    Clef(ClefChange),
    KeySignature(KeySignature),
    TimeSignature(Meter),
    Barline(Barline),
    ChordSymbol(ChordSymbol),
    FiguredBass(FiguredBass),
}

/// How a note was PLAYED, kept beside how it is written.
///
/// A score imported from a performance knows things the engraving cannot say:
/// this note was struck at velocity 43 and that one at 96. Notation has no
/// place to put that — a dynamic mark covers a phrase, not a note — so it
/// rides here, hidden: nothing engraves it, nothing edits it, and a note that
/// was typed rather than played simply has none. Playback reads it and gets
/// the performance back; everything else ignores it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct NotePerformance {
    /// MIDI velocity as struck, 1..=127.
    pub velocity: u8,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct Note {
    pub id: NoteId,
    /// The performance this note came from, when it came from one. See
    /// [`NotePerformance`].
    pub performance: Option<NotePerformance>,
    pub written_pitch: Option<Pitch>,
    pub unpitched_sound: Option<u16>,
    pub display_staff: StaffId,
    pub tie_from: Option<NoteId>,
    pub tie_to: Option<NoteId>,
    pub tab: Option<TabPosition>,
    pub notehead: Notehead,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct TabPosition {
    pub string: u16,
    pub fret: u16,
    pub bend: Alter,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum Notehead {
    Normal,
    X,
    Diamond,
    Triangle,
    Slash,
    Other(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct GraceTiming {
    pub position: GracePosition,
    pub steal: Option<Rational>,
    pub slash: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum GracePosition {
    BeforeBeat,
    AfterBeat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct BeamMembership {
    pub level: u8,
    pub state: BeamState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum BeamState {
    Begin,
    Continue,
    End,
    ForwardHook,
    BackwardHook,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct TupletNotation {
    pub actual: u16,
    pub normal: u16,
    pub group: SpannerId,
    pub level: u8,
    pub bracket: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct PlacedArticulation {
    pub kind: Articulation,
    pub placement: Option<Placement>,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct DirectionEvent {
    pub kind: DirectionKind,
    pub placement: Option<Placement>,
    pub original_text: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum DirectionKind {
    Words(String),
    Dynamic(DynamicMark),
    Rehearsal(String),
    TempoText(String),
    Segno,
    Coda,
    Fine,
    DaCapo,
    DalSegno,
    ToCoda,
    Breath,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct ClefChange {
    pub clef: Clef,
    pub line: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct Barline {
    pub style: BarlineStyle,
    pub repeat: Option<RepeatDirection>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum BarlineStyle {
    Regular,
    Dotted,
    Dashed,
    Heavy,
    Double,
    Final,
    Invisible,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum RepeatDirection {
    Forward,
    Backward { times: u16 },
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct Spanner {
    pub id: SpannerId,
    pub kind: SpannerKind,
    pub start: SpannerEndpoint,
    pub end: SpannerEndpoint,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum SpannerKind {
    Slur { placement: Option<Placement> },
    Hairpin { crescendo: bool, niente: bool },
    Ottava { octaves: i8 },
    Pedal,
    Volta { passes: Vec<u16>, text: String },
    Glissando { text: Option<String> },
    LyricExtender,
    Tie,
    Other(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum SpannerEndpoint {
    Note(NoteId),
    Event(EventId),
    StaffTime { staff: StaffId, at: ScoreTime },
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct LyricSyllable {
    pub note: NoteId,
    pub verse: u16,
    pub text: String,
    pub role: SyllabicRole,
    pub elision: Option<String>,
    pub melisma_to: Option<NoteId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum SyllabicRole {
    Single,
    Begin,
    Middle,
    End,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct ChordSymbol {
    pub root: PitchClass,
    pub quality: ChordQuality,
    pub degrees: Vec<ChordDegree>,
    pub bass: Option<PitchClass>,
    pub original_text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct PitchClass {
    pub step: Step,
    pub alter: Alter,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum ChordQuality {
    Major,
    Minor,
    Augmented,
    Diminished,
    Dominant,
    Suspended,
    Other(String),
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct ChordDegree {
    pub value: u8,
    pub alter: Alter,
    pub operation: DegreeOperation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum DegreeOperation {
    Add,
    Alter,
    Subtract,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct FiguredBass {
    pub figures: Vec<Figure>,
    pub continuation: Option<SpannerId>,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct Figure {
    pub interval: u8,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, SerBin, DeBin)]
pub struct LayoutOverrides {
    pub page_turns: Vec<MeasureId>,
    pub hidden_staves: Vec<StaffId>,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct PartView {
    pub id: PartViewId,
    pub name: String,
    pub included_parts: Vec<PartId>,
    pub layout_overrides: LayoutOverrides,
}

/// A borrowed extracted-part/full-score projection over the one authoritative graph.
#[derive(Clone, Copy, Debug)]
pub struct PartProjection<'a> {
    score: &'a Score,
    view: &'a PartView,
}

impl<'a> PartProjection<'a> {
    pub const fn score(&self) -> &'a Score {
        self.score
    }

    pub const fn view(&self) -> &'a PartView {
        self.view
    }

    pub fn parts(&self) -> impl Iterator<Item = &'a Part> + 'a {
        self.view
            .included_parts
            .iter()
            .filter_map(|id| self.score.parts.get(id))
    }

    pub fn staves(&self) -> impl Iterator<Item = &'a Staff> + 'a {
        self.parts()
            .flat_map(|part| part.staves.iter())
            .filter_map(|id| self.score.staves.get(id))
            .filter(|staff| !self.view.layout_overrides.hidden_staves.contains(&staff.id))
    }

    pub fn voices(&self) -> impl Iterator<Item = &'a Voice> + 'a {
        self.staves()
            .flat_map(|staff| staff.voices.iter())
            .filter_map(|id| self.score.voices.get(id))
    }
}
