use super::graph::{EventKind, Note, Score, TimedEvent};
use super::id::*;
use super::pitch::Pitch;
use super::time::{Rational, ScoreTime};
use makepad_micro_serde::{DeBin, DeBinErr, SerBin};

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct Annotation {
    pub id: AnnotationId,
    pub layer: LayerId,
    pub kind: AnnotationKind,
    pub anchor: SemanticAnchor,
    pub body: AnnotationBody,
    pub style: AnnotationStyle,
    pub action: Option<PracticeAction>,
    pub author: [u8; 16],
    pub created_lamport: u64,
    pub modified_lamport: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum AnnotationKind {
    Highlight,
    Circle,
    Box,
    Fingering,
    Bowing,
    Breath,
    Cue,
    Text,
    Loop,
    PracticeTempo,
    Ink,
    Analysis,
    Other(String),
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum AnnotationBody {
    None,
    Text(String),
    Fingering(String),
    Bowing(String),
    Ink(InkStroke),
    Opaque(Vec<u8>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct AnnotationStyle {
    pub color_rgba: [u8; 4],
    pub width_milli_staff_space: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum PracticeAction {
    Loop,
    SuggestedTempo {
        quarters_per_minute: Rational,
        ramp_increment: Option<Rational>,
    },
}

/// A semantic target with redundant exact-time recovery information.
#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct SemanticAnchor {
    pub primary: AnchorTarget,
    pub fallback: BeatRange,
    pub affinity: Affinity,
    pub context_fingerprint: ContextFingerprint,
    pub ink: Option<InkAnchor>,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum AnchorTarget {
    Note(NoteId),
    Event(EventId),
    Measure(MeasureId),
    ElementRange { first: EventId, last: EventId },
    MusicalRange(BeatRange),
    SourceRegion(SourceRegionId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct BeatRange {
    pub staff: StaffId,
    pub voice: Option<VoiceId>,
    pub start: ScoreTime,
    pub end: ScoreTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum Affinity {
    Before,
    On,
    After,
    Span,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct ContextFingerprint(pub [u8; 16]);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnchorResolution {
    Exact(ResolvedTarget),
    Fallback {
        target: ResolvedTarget,
        confidence_milli: u16,
    },
    Orphaned {
        last_known: BeatRange,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedTarget {
    Note(NoteId),
    Event(EventId),
    Measure(MeasureId),
    ElementRange { first: EventId, last: EventId },
    MusicalRange(BeatRange),
    SourceRegion(SourceRegionId),
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum InkAnchor {
    ElementLocal {
        target: ElementRef,
        points: Vec<LocalInkPoint>,
    },
    StaffStrip {
        fragments: Vec<StaffInkFragment>,
    },
    EndpointWarp {
        from: AnchorPoint,
        to: AnchorPoint,
        curve: LocalBezier,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum ElementRef {
    Note(NoteId),
    Event(EventId),
    Measure(MeasureId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct LocalInkPoint {
    pub u: Rational,
    pub v: Rational,
    pub pressure: u16,
    pub tilt: i16,
    pub azimuth: u16,
    pub elapsed_micros: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct StaffInkFragment {
    pub staff: StaffId,
    pub points: Vec<StaffStripPoint>,
    pub keep_page_bound: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct StaffStripPoint {
    pub segment: BeatSegment,
    pub u: Rational,
    pub v_staff_spaces: Rational,
    pub pressure: u16,
    pub elapsed_micros: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct BeatSegment {
    pub start: ScoreTime,
    pub end: ScoreTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct AnchorPoint {
    pub target: ElementRef,
    pub u: Rational,
    pub v: Rational,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct LocalBezier {
    pub from_dx: Rational,
    pub from_dy: Rational,
    pub to_dx: Rational,
    pub to_dy: Rational,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct InkStroke {
    pub anchor_kind: InkAnchor,
    pub original_polyline: Vec<SourcePoint>,
    pub color_rgba: [u8; 4],
    pub nominal_width_milli_staff_space: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct AnnotationLayer {
    pub id: LayerId,
    pub title: String,
    pub owner: [u8; 16],
    pub color_hint: [u8; 4],
    pub visible_by_default: bool,
    pub scope: LayerScope,
    pub permissions: LayerPermissions,
    pub export_policy: ExportPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum LayerScope {
    AllScore,
    Parts(Vec<PartId>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum LayerPermissions {
    Private,
    ViewOnly,
    Collaborative,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum ExportPolicy {
    Include,
    Exclude,
    Ask,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct SourceRegion {
    pub id: SourceRegionId,
    pub document_digest: [u8; 32],
    pub page: u32,
    pub normalized_polygon: Vec<SourcePoint>,
    pub source_to_page: [Rational; 6],
    pub classification: SourceClassification,
    pub candidates: Vec<RecognizedCandidate>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct SourcePoint {
    pub x: Rational,
    pub y: Rational,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, SerBin, DeBin)]
pub enum SourceClassification {
    Raster,
    Vector,
    Mixed,
}

#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct RecognizedCandidate {
    pub address: BeatRange,
    pub confidence_milli: u16,
    pub target: Option<ElementRef>,
}

impl Score {
    pub fn resolve_anchor(&self, anchor: &SemanticAnchor) -> AnchorResolution {
        if let Some(target) = self.exact_target(&anchor.primary) {
            return AnchorResolution::Exact(target);
        }
        if let Some(target) = self.fallback_target(anchor) {
            return AnchorResolution::Fallback {
                target,
                confidence_milli: 1000,
            };
        }
        AnchorResolution::Orphaned {
            last_known: anchor.fallback,
        }
    }

    pub fn note_fingerprint(&self, note_id: NoteId) -> Option<ContextFingerprint> {
        let (voice_id, event, note) = self.note_context(note_id)?;
        let voice = self.voices.get(&voice_id)?;
        Some(fingerprint_note(voice.staff, voice_id, event, note))
    }

    pub fn event_fingerprint(&self, event_id: EventId) -> Option<ContextFingerprint> {
        let voice_id = self.event_owner(event_id)?;
        let voice = self.voices.get(&voice_id)?;
        let event = self.event(event_id)?;
        Some(fingerprint_event(voice.staff, voice_id, event))
    }

    pub fn measure_fingerprint(&self, measure_id: MeasureId) -> Option<ContextFingerprint> {
        let measure = self.measures.get(&measure_id)?;
        let mut hash = FingerprintBuilder::new();
        hash.rational(measure.start.0);
        hash.rational(measure.extent.0);
        hash.bytes(&measure.ordinal.to_le_bytes());
        Some(hash.finish())
    }

    fn exact_target(&self, target: &AnchorTarget) -> Option<ResolvedTarget> {
        match *target {
            AnchorTarget::Note(id) => self.note(id).map(|_| ResolvedTarget::Note(id)),
            AnchorTarget::Event(id) => self.event(id).map(|_| ResolvedTarget::Event(id)),
            AnchorTarget::Measure(id) => self
                .measures
                .contains_key(&id)
                .then_some(ResolvedTarget::Measure(id)),
            AnchorTarget::ElementRange { first, last } => (self.event(first).is_some()
                && self.event(last).is_some())
            .then_some(ResolvedTarget::ElementRange { first, last }),
            AnchorTarget::MusicalRange(range) => (self.staves.contains_key(&range.staff)
                && range.start <= range.end)
                .then_some(ResolvedTarget::MusicalRange(range)),
            AnchorTarget::SourceRegion(id) => self
                .source_regions
                .contains_key(&id)
                .then_some(ResolvedTarget::SourceRegion(id)),
        }
    }

    fn fallback_target(&self, anchor: &SemanticAnchor) -> Option<ResolvedTarget> {
        let range = anchor.fallback;
        let voices = self.voices.iter().filter(|(voice_id, voice)| {
            voice.staff == range.staff && range.voice.map_or(true, |id| id == **voice_id)
        });
        match anchor.primary {
            AnchorTarget::Note(_) => voices
                .flat_map(|(voice_id, voice)| {
                    voice.events.iter().flat_map(move |event| {
                        event
                            .chord_notes()
                            .iter()
                            .map(move |note| (*voice_id, voice.staff, event, note))
                    })
                })
                .find_map(|(voice_id, staff, event, note)| {
                    (event.onset == range.start
                        && fingerprint_note(staff, voice_id, event, note)
                            == anchor.context_fingerprint)
                        .then_some(ResolvedTarget::Note(note.id))
                }),
            AnchorTarget::Event(_) => voices
                .flat_map(|(voice_id, voice)| {
                    voice
                        .events
                        .iter()
                        .map(move |event| (*voice_id, voice.staff, event))
                })
                .find_map(|(voice_id, staff, event)| {
                    (event.onset == range.start
                        && fingerprint_event(staff, voice_id, event)
                            == anchor.context_fingerprint)
                        .then_some(ResolvedTarget::Event(event.id))
                }),
            AnchorTarget::Measure(_) => self.measures.values().find_map(|measure| {
                let mut hash = FingerprintBuilder::new();
                hash.rational(measure.start.0);
                hash.rational(measure.extent.0);
                hash.bytes(&measure.ordinal.to_le_bytes());
                (measure.start == range.start && hash.finish() == anchor.context_fingerprint)
                    .then_some(ResolvedTarget::Measure(measure.id))
            }),
            AnchorTarget::ElementRange { .. } => {
                let mut events = voices
                    .flat_map(|(_, voice)| voice.events.iter())
                    .filter(|event| event.onset >= range.start && event.onset < range.end);
                let first = events.next()?.id;
                let last = events.last().map_or(first, |event| event.id);
                Some(ResolvedTarget::ElementRange { first, last })
            }
            AnchorTarget::MusicalRange(_) => self
                .staves
                .contains_key(&range.staff)
                .then_some(ResolvedTarget::MusicalRange(range)),
            AnchorTarget::SourceRegion(_) => None,
        }
    }
}

fn fingerprint_note(
    staff: StaffId,
    voice: VoiceId,
    event: &TimedEvent,
    note: &Note,
) -> ContextFingerprint {
    let mut hash = FingerprintBuilder::new();
    hash.id(staff.raw());
    hash.id(voice.raw());
    hash.rational(event.onset.0);
    hash.optional_rational(event.duration.map(|duration| duration.0));
    hash.pitch(note.written_pitch);
    hash.finish()
}

fn fingerprint_event(staff: StaffId, voice: VoiceId, event: &TimedEvent) -> ContextFingerprint {
    let mut hash = FingerprintBuilder::new();
    hash.id(staff.raw());
    hash.id(voice.raw());
    hash.rational(event.onset.0);
    hash.optional_rational(event.duration.map(|duration| duration.0));
    hash.byte(match &event.kind {
        EventKind::Chord(_) => 0,
        EventKind::Rest => 1,
        EventKind::Direction(_) => 2,
        EventKind::Clef(_) => 3,
        EventKind::KeySignature(_) => 4,
        EventKind::TimeSignature(_) => 5,
        EventKind::Barline(_) => 6,
        EventKind::ChordSymbol(_) => 7,
        EventKind::FiguredBass(_) => 8,
    });
    for note in event.chord_notes() {
        hash.pitch(note.written_pitch);
    }
    hash.finish()
}

struct FingerprintBuilder {
    left: u64,
    right: u64,
}

impl FingerprintBuilder {
    const fn new() -> Self {
        Self {
            left: 0xcbf29ce484222325,
            right: 0x84222325cbf29ce4,
        }
    }

    fn byte(&mut self, value: u8) {
        self.left ^= u64::from(value);
        self.left = self.left.wrapping_mul(0x100000001b3);
        self.right ^= u64::from(value.rotate_left(1));
        self.right = self.right.wrapping_mul(0x100000001b3);
    }

    fn bytes(&mut self, values: &[u8]) {
        for value in values {
            self.byte(*value);
        }
    }

    fn id(&mut self, (actor, counter): (u64, u64)) {
        self.bytes(&actor.to_le_bytes());
        self.bytes(&counter.to_le_bytes());
    }

    fn rational(&mut self, value: Rational) {
        self.bytes(&value.numerator().to_le_bytes());
        self.bytes(&value.denominator().to_le_bytes());
    }

    fn optional_rational(&mut self, value: Option<Rational>) {
        match value {
            Some(value) => {
                self.byte(1);
                self.rational(value);
            }
            None => self.byte(0),
        }
    }

    fn pitch(&mut self, pitch: Option<Pitch>) {
        match pitch {
            Some(pitch) => {
                self.byte(1);
                self.byte(pitch.step.index() as u8);
                self.rational(pitch.alter.0);
                self.byte(pitch.octave as u8);
            }
            None => self.byte(0),
        }
    }

    fn finish(self) -> ContextFingerprint {
        let mut output = [0_u8; 16];
        output[..8].copy_from_slice(&self.left.to_le_bytes());
        output[8..].copy_from_slice(&self.right.to_le_bytes());
        ContextFingerprint(output)
    }
}
