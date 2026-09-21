//! Playback-free score document and retained engraving pages.

use crate::spacing::ScoreSpacing;
use makepad_score::model::*;
use makepad_score_render::{PageCache, PaintList, Point, Rect, SemanticId};
use std::{collections::BTreeMap, sync::Arc};

pub const PAGE_WIDTH_SP: f64 = 168.0;
pub const PAGE_HEIGHT_SP: f64 = 238.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DocumentOptions {
    pub hide_labels: bool,
    pub drum_key: bool,
    pub page_size: Point,
}

impl Default for DocumentOptions {
    fn default() -> Self {
        Self {
            hide_labels: false,
            drum_key: false,
            page_size: Point::new(PAGE_WIDTH_SP, PAGE_HEIGHT_SP),
        }
    }
}

impl DocumentOptions {
    pub fn content(score: &Score, hide_labels: bool) -> Self {
        let measure_ratio = score.measures.len() as f64 / 8.0;
        Self {
            hide_labels,
            drum_key: true,
            page_size: Point::new(
                PAGE_WIDTH_SP * measure_ratio.clamp(0.5, 1.0),
                PAGE_HEIGHT_SP,
            ),
        }
    }
}

const ACTOR: u64 = 0x5c0e;
const NOTE_SEMANTIC_TAG: u64 = 0x1000_0000_0000_0000;
const MEASURE_SEMANTIC_TAG: u64 = 0x2000_0000_0000_0000;
pub(crate) const DECORATION_TAG: u64 = 0x8000_0000_0000_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticKind {
    Note,
    Measure,
}

#[derive(Clone, Debug)]
pub struct SemanticElement {
    pub semantic: SemanticId,
    pub kind: SemanticKind,
    pub note: Option<NoteId>,
    pub event: Option<EventId>,
    pub measure: MeasureId,
    pub staff: StaffId,
    pub voice: VoiceId,
    pub page: usize,
    pub bounds: Rect,
    pub midi: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentError {
    Native(String),
}

impl std::fmt::Display for DocumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Native(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for DocumentError {}

/// A score plus its retained, playback-free engraving pages.
pub struct ScoreDocument {
    score: Score,
    options: DocumentOptions,
    pages: Vec<Arc<PaintList>>,
    cache: PageCache,
    elements: BTreeMap<SemanticId, SemanticElement>,
    spacing: ScoreSpacing,
    frame: u64,
}

impl Default for ScoreDocument {
    fn default() -> Self {
        Self {
            score: Score::new([0; 16]),
            options: DocumentOptions::default(),
            pages: Vec::new(),
            cache: PageCache::new(32 * 1024 * 1024),
            elements: BTreeMap::new(),
            spacing: ScoreSpacing::new(),
            frame: 1,
        }
    }
}

impl ScoreDocument {
    pub fn demo() -> Result<Self, DocumentError> {
        Self::new(demo_score(8)?)
    }

    pub fn new(score: Score) -> Result<Self, DocumentError> {
        Self::with_options(score, DocumentOptions::default())
    }

    pub fn with_options(
        score: Score,
        options: DocumentOptions,
    ) -> Result<Self, DocumentError> {
        let mut document = Self::default();
        document.set_score_with_options(score, options)?;
        Ok(document)
    }

    pub fn set_score(&mut self, score: Score) -> Result<(), DocumentError> {
        self.set_score_with_options(score, self.options)
    }

    pub fn set_score_with_options(
        &mut self,
        score: Score,
        options: DocumentOptions,
    ) -> Result<(), DocumentError> {
        self.score = score;
        self.options = options;
        self.rebuild()
    }

    pub fn set_options(&mut self, options: DocumentOptions) -> Result<(), DocumentError> {
        if self.options == options {
            return Ok(());
        }
        self.options = options;
        self.rebuild()
    }

    pub fn clear(&mut self) {
        self.score = Score::new([0; 16]);
        self.pages.clear();
        self.cache = PageCache::new(32 * 1024 * 1024);
        self.elements.clear();
        self.spacing = ScoreSpacing::new();
        self.spacing.set_page_width(self.options.page_size.x);
    }

    pub fn score(&self) -> &Score {
        &self.score
    }

    pub fn pages(&self) -> &[Arc<PaintList>] {
        &self.pages
    }

    pub fn options(&self) -> DocumentOptions {
        self.options
    }

    pub fn content_bounds(&self, page: usize) -> Option<Rect> {
        let bounds = self.pages.get(page)?.items().iter().fold(Rect::EMPTY, |bounds, item| {
            bounds.union(item.bounds)
        });
        (!bounds.is_empty()).then_some(bounds)
    }

    pub fn first_system_bounds(&self, page: usize) -> Option<Rect> {
        let placement = self.spacing.pages().get(page)?;
        let system = placement.systems.first()?;
        let min_y = system.top - 14.0;
        let max_y = placement
            .systems
            .get(1)
            .map_or(system.bottom + 14.0, |next| (system.bottom + next.top) * 0.5);
        let bounds = self.pages.get(page)?.items().iter().fold(Rect::EMPTY, |bounds, item| {
            if item.bounds.max.y >= min_y && item.bounds.min.y <= max_y {
                bounds.union(item.bounds)
            } else {
                bounds
            }
        });
        (!bounds.is_empty()).then_some(bounds)
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn system_count(&self, page: usize) -> usize {
        self.spacing.pages().get(page).map_or(0, |page| page.systems.len())
    }

    pub fn spacing(&self) -> &ScoreSpacing {
        &self.spacing
    }

    pub fn element(&self, semantic: SemanticId) -> Option<&SemanticElement> {
        self.elements.get(&semantic)
    }

    pub fn rebuild(&mut self) -> Result<(), DocumentError> {
        self.spacing.set_page_width(self.options.page_size.x);
        self.spacing.set_drum_key(self.options.drum_key);
        self.spacing.rebuild(&self.score);
        self.pages.clear();
        self.elements.clear();
        for page_index in 0..self.spacing.page_count() {
            let placement = &self.spacing.pages()[page_index];
            let (list, elements) = crate::engrave::make_page_with_options(
                &self.score,
                placement,
                page_index,
                self.frame,
                self.options,
            )?;
            let list = Arc::new(list);
            self.cache.insert(list.clone(), self.frame);
            self.pages.push(list);
            self.elements
                .extend(elements.into_iter().map(|element| (element.semantic, element)));
            self.frame = self.frame.saturating_add(1);
        }
        Ok(())
    }
}

pub fn semantic_for_note(id: NoteId) -> SemanticId {
    let (actor, counter) = id.raw();
    SemanticId(NOTE_SEMANTIC_TAG | actor.rotate_left(17) ^ counter)
}

pub fn semantic_for_measure(id: MeasureId) -> SemanticId {
    let (actor, counter) = id.raw();
    SemanticId(MEASURE_SEMANTIC_TAG | actor.rotate_left(11) ^ counter)
}

pub fn pitch_to_midi(pitch: Pitch) -> u8 {
    let natural = match pitch.step {
        Step::C => 0,
        Step::D => 2,
        Step::E => 4,
        Step::F => 5,
        Step::G => 7,
        Step::A => 9,
        Step::B => 11,
    };
    let alter = pitch.alter.0.numerator() as f64 / pitch.alter.0.denominator() as f64;
    ((i16::from(pitch.octave) + 1) * 12 + natural + alter.round() as i16).clamp(0, 127) as u8
}

pub fn pitch_from_midi(midi: u8) -> Pitch {
    let octave = (midi / 12) as i8 - 1;
    let (step, alter) = match midi % 12 {
        0 => (Step::C, 0),
        1 => (Step::C, 1),
        2 => (Step::D, 0),
        3 => (Step::D, 1),
        4 => (Step::E, 0),
        5 => (Step::F, 0),
        6 => (Step::F, 1),
        7 => (Step::G, 0),
        8 => (Step::G, 1),
        9 => (Step::A, 0),
        10 => (Step::A, 1),
        _ => (Step::B, 0),
    };
    Pitch::new(step, Alter::new(alter, 1).unwrap_or(Alter::NATURAL), octave)
}

/// Build the small two-staff fixture used by examples and tests.
pub fn demo_score(measure_count: usize) -> Result<Score, DocumentError> {
    let mut ids = IdGenerator::new(ACTOR);
    let part = ids.next::<PartTag>().map_err(id_error)?;
    let upper = ids.next::<StaffTag>().map_err(id_error)?;
    let lower = ids.next::<StaffTag>().map_err(id_error)?;
    let upper_voice = ids.next::<VoiceTag>().map_err(id_error)?;
    let lower_voice = ids.next::<VoiceTag>().map_err(id_error)?;
    let mut score = Score::new(*b"SCOREVIEWDEMO000");
    score.title = "Demo score".into();
    score.parts.insert(part, Part {
        id: part,
        name: "Keyboard".into(),
        staves: vec![upper, lower],
        transposition: Transposition::NONE,
    });
    score.staves.insert(upper, Staff {
        id: upper, part, parent: None, kind: StaffKind::Standard, voices: vec![upper_voice],
    });
    score.staves.insert(lower, Staff {
        id: lower, part, parent: Some(upper), kind: StaffKind::Standard, voices: vec![lower_voice],
    });
    let mut upper_events = Vec::new();
    let mut lower_events = Vec::new();
    for bar in 0..measure_count.max(1) {
        let measure = ids.next::<MeasureTag>().map_err(id_error)?;
        let start = ScoreTime::new(bar as i64, 1).map_err(native_error)?;
        let extent = Duration::new(1, 1).map_err(native_error)?;
        score.measures.insert(measure, Measure {
            id: measure, ordinal: bar as u32, label: (bar + 1).to_string(), start, extent,
        });
        score.flow.nodes.push(FlowNode { measure, ordinal: bar as u32 });
        for beat in 0..4 {
            let onset = start
                .checked_add_time(ScoreTime::new(beat, 4).map_err(native_error)?)
                .map_err(native_error)?;
            upper_events.push(note_event(
                ids.next::<EventTag>().map_err(id_error)?,
                ids.next::<NoteTag>().map_err(id_error)?,
                upper,
                onset,
                Duration::new(1, 4).map_err(native_error)?,
                pitch_from_midi(60 + ((bar * 4 + beat as usize) % 8) as u8),
            ));
        }
        lower_events.push(note_event(
            ids.next::<EventTag>().map_err(id_error)?,
            ids.next::<NoteTag>().map_err(id_error)?,
            lower,
            start,
            extent,
            pitch_from_midi(40 + (bar % 5) as u8),
        ));
    }
    score.voices.insert(upper_voice, Voice { id: upper_voice, staff: upper, number: 1, events: upper_events });
    score.voices.insert(lower_voice, Voice { id: lower_voice, staff: lower, number: 1, events: lower_events });
    score.maps.time_signature.push(Change {
        at: ScoreTime::ZERO,
        scope: MapScope::Global,
        value: Meter::Measured { groups: vec![4], unit: 4 },
    });
    Ok(score)
}

fn id_error(_: IdError) -> DocumentError {
    DocumentError::Native("score id space exhausted".into())
}

fn native_error(error: impl std::fmt::Display) -> DocumentError {
    DocumentError::Native(error.to_string())
}

pub fn note_event(
    event: EventId,
    note: NoteId,
    staff: StaffId,
    onset: ScoreTime,
    duration: Duration,
    pitch: Pitch,
) -> TimedEvent {
    TimedEvent {
        id: event,
        onset,
        duration: Some(duration),
        grace: None,
        kind: EventKind::Chord(vec![Note {
            id: note,
            performance: None,
            written_pitch: Some(pitch),
            unpitched_sound: None,
            display_staff: staff,
            tie_from: None,
            tie_to: None,
            tab: None,
            notehead: Notehead::Normal,
        }]),
        beams: Vec::new(),
        tuplets: Vec::new(),
        articulations: Vec::new(),
        ornaments: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        build_drum_score, build_pitched_score, BuildOptions, DrumHit, DrumVoice, PitchedNote,
    };
    use makepad_score_render::{PaintKind, Primitive, RuleKind};

    fn drum_document(hide_labels: bool) -> ScoreDocument {
        crate::font::ensure_default_font();
        let hits = [
            DrumHit { time_beats: 0.0, voice: DrumVoice::HiHatClosed, velocity: 1.0 },
            DrumHit { time_beats: 1.0, voice: DrumVoice::HiHatClosed, velocity: 1.0 },
            DrumHit { time_beats: 2.0, voice: DrumVoice::HiHatClosed, velocity: 1.0 },
            DrumHit { time_beats: 3.0, voice: DrumVoice::HiHatClosed, velocity: 1.0 },
        ];
        let score = build_drum_score(&hits, &BuildOptions::default());
        let options = DocumentOptions::content(&score, hide_labels);
        ScoreDocument::with_options(score, options).expect("the drum fixture engraves")
    }

    fn text_items(document: &ScoreDocument) -> Vec<&makepad_score_render::PaintItem> {
        document.pages()[0]
            .items()
            .iter()
            .filter(|item| matches!(item.kind, PaintKind::Text(_)))
            .collect()
    }

    #[test]
    fn compact_drum_content_is_smaller_than_its_page() {
        let document = drum_document(true);
        let page = &document.pages()[0];
        let bounds = document.content_bounds(0).expect("engraved content has bounds");
        assert!(bounds.width() > 0.0 && bounds.height() > 0.0);
        assert!(bounds.width() < page.page_size().x);
        assert!(bounds.height() < page.page_size().y);
        assert_eq!(page.page_size().x, PAGE_WIDTH_SP * 0.5);
    }

    #[test]
    fn hide_labels_does_not_remove_the_drum_key() {
        let visible = drum_document(false);
        assert!(text_items(&visible).len() > 1);

        let hidden = drum_document(true);
        let texts: Vec<&str> = text_items(&hidden)
            .into_iter()
            .filter_map(|item| match &item.kind {
                PaintKind::Text(run) => Some(run.text.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["HH"]);
    }

    #[test]
    fn adjacent_drum_voices_share_one_key_line() {
        crate::font::ensure_default_font();
        let score = build_drum_score(
            &[
                DrumHit { time_beats: 0.0, voice: DrumVoice::HiHatClosed, velocity: 1.0 },
                DrumHit { time_beats: 1.0, voice: DrumVoice::Crash, velocity: 1.0 },
            ],
            &BuildOptions::default(),
        );
        let options = DocumentOptions::content(&score, true);
        let document = ScoreDocument::with_options(score, options).expect("drum fixture engraves");
        let texts: Vec<&str> = text_items(&document)
            .into_iter()
            .filter_map(|item| match &item.kind {
                PaintKind::Text(run) => Some(run.text.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["Crash / HH"]);
    }

    #[test]
    fn content_drum_key_contains_only_used_voices_at_their_display_pitches() {
        crate::font::ensure_default_font();
        let score = build_drum_score(
            &[
                DrumHit { time_beats: 0.0, voice: DrumVoice::Kick, velocity: 1.0 },
                DrumHit { time_beats: 1.0, voice: DrumVoice::Snare, velocity: 1.0 },
                DrumHit { time_beats: 2.0, voice: DrumVoice::HiHatClosed, velocity: 1.0 },
            ],
            &BuildOptions::default(),
        );
        let options = DocumentOptions::content(&score, true);
        let without_key = ScoreDocument::with_options(
            score.clone(),
            DocumentOptions { drum_key: false, ..options },
        )
        .expect("the unlabelled drum fixture engraves");
        let document = ScoreDocument::with_options(score, options)
            .expect("the labelled drum fixture engraves");
        let reserved = document.spacing().pages()[0].systems[0].music_left
            - without_key.spacing().pages()[0].systems[0].music_left;
        assert!((reserved - crate::engrave::drum_key_width(document.score())).abs() < 1e-9);
        let system_top = document.spacing().pages()[0].systems[0].top;
        let staff = crate::engrave::score_staff_frames(document.score(), system_top)[0];
        let mut labels: Vec<(&str, f64, f64)> = text_items(&document)
            .into_iter()
            .filter_map(|item| match &item.kind {
                PaintKind::Text(run) => Some((run.text.as_ref(), item.bounds.center().y, item.bounds.max.x)),
                _ => None,
            })
            .collect();
        labels.sort_by_key(|(label, _, _)| *label);
        assert_eq!(labels.iter().map(|(label, _, _)| *label).collect::<Vec<_>>(), vec!["HH", "Kick", "Snare"]);

        let clef_x = document.pages()[0]
            .items()
            .iter()
            .find_map(|item| match &item.kind {
                PaintKind::Glyph(glyph) if glyph.glyph.0.as_ref() == "unpitchedPercussionClef1" => {
                    Some(glyph.origin.x)
                }
                _ => None,
            })
            .expect("percussion clef");
        for (label, center_y, right) in labels {
            let voice = match label {
                "Kick" => DrumVoice::Kick,
                "Snare" => DrumVoice::Snare,
                "HH" => DrumVoice::HiHatClosed,
                _ => unreachable!(),
            };
            let (display, _) = voice.display();
            let diatonic = i32::from(display.octave) * 7 + i32::from(display.step.index());
            assert!((center_y - staff.y_of(diatonic)).abs() < 1e-9);
            assert!(right < clef_x, "{label} must sit left of the clef");
        }
    }

    #[test]
    fn pitched_content_score_has_no_drum_key() {
        crate::font::ensure_default_font();
        let score = build_pitched_score(
            &[PitchedNote {
                onset_beats: 0.0,
                duration_beats: 1.0,
                midi: 60,
                velocity: 1.0,
            }],
            &BuildOptions::default(),
        );
        let options = DocumentOptions::content(&score, true);
        let document = ScoreDocument::with_options(score, options).expect("pitched fixture engraves");
        assert!(text_items(&document).is_empty());
    }

    #[test]
    fn drum_page_retains_percussion_clef_staff_and_x_heads() {
        let document = drum_document(true);
        let items = document.pages()[0].items();
        let glyphs: Vec<&str> = items
            .iter()
            .filter_map(|item| match &item.kind {
                PaintKind::Glyph(glyph) => Some(glyph.glyph.0.as_ref()),
                _ => None,
            })
            .collect();
        assert!(glyphs.contains(&"unpitchedPercussionClef1"));
        assert!(
            glyphs.iter().filter(|name| **name == "noteheadXBlack").count() >= 4,
            "expected the four hi-hat X noteheads, got {glyphs:?}"
        );
        let staff_lines = items
            .iter()
            .filter(|item| {
                matches!(
                    item.kind,
                    PaintKind::Primitive(Primitive::Rule {
                        kind: RuleKind::Staff,
                        staff_group: Some(_),
                        ..
                    })
                )
            })
            .count();
        assert_eq!(staff_lines, 5);
    }
}
