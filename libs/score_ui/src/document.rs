//! Semantic document, incremental engraving, retained paint pages, and the
//! narrow native/import loading seam used by the application.

use makepad_score::{
    model::*,
    symbol::{Articulation, Placement},
};
use crate::spacing::{PagesDirty, ScoreSpacing};
use makepad_score_layout::RelayoutStats;
use makepad_score_render::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

pub const PAGE_WIDTH_SP: f64 = 168.0;
pub const PAGE_HEIGHT_SP: f64 = 238.0;

/// The native workspace container: score plus annotations plus edit journal.
pub const NATIVE_EXTENSION: &str = "mpscore";

/// True for a file Save may overwrite in place.
pub fn is_native_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(NATIVE_EXTENSION))
}

/// Give a Save As target the native extension, so a user who types
/// `sonata` or keeps the imported `sonata.mid` name still gets a
/// `sonata.mpscore` instead of a file that no longer matches its contents.
pub fn with_native_extension(path: &Path) -> PathBuf {
    if is_native_extension(path) {
        return path.to_path_buf();
    }
    path.with_extension(NATIVE_EXTENSION)
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
    pub note: Option<makepad_score::model::NoteId>,
    pub event: Option<EventId>,
    pub measure: MeasureId,
    pub staff: StaffId,
    pub voice: VoiceId,
    pub page: usize,
    pub bounds: makepad_score_render::Rect,
    pub midi: Option<u8>,
}

#[derive(Clone, Debug)]
pub struct AnnotationVisual {
    pub id: AnnotationId,
    pub kind: AnnotationKind,
    pub semantic: SemanticId,
    pub text: Option<String>,
    pub color: [u8; 4],
    pub ink_points: Vec<Point>,
}

#[derive(Clone, Debug, Default)]
pub struct PartUiState {
    pub name: String,
    pub gain: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
}

/// The playable range of a piano keyboard; a drag outside it is refused.
const PIANO_LOWEST: u8 = 21;
const PIANO_HIGHEST: u8 = 108;
/// How many grid slots one horizontal drag may cross. A measure at the finest
/// entry duration has fewer slots than this.
const MAX_GRID_SCAN: u32 = 256;

/// A note drag in progress: the snapshot the gesture is resolved against.
#[derive(Clone, Debug)]
pub struct NoteDrag {
    pub semantic: SemanticId,
    pub note: makepad_score::model::NoteId,
    pub event: EventId,
    pub voice: VoiceId,
    display_staff: StaffId,
    pub measure: MeasureId,
    pub page: usize,
    /// Page point of the notehead's centre when the drag started.
    pub origin: Point,
    /// Diatonic index (`octave * 7 + step`) of the dragged note.
    pub diatonic: i32,
    /// Its written alteration, in semitones.
    pub alter: i32,
    pub midi: u8,
    pub onset: ScoreTime,
    pub duration: Option<Duration>,
    pub measure_start: ScoreTime,
    pub measure_end: ScoreTime,
    /// Page x of the measure's opening and closing barlines.
    pub measure_left: f64,
    pub measure_right: f64,
    pub key_fifths: i8,
    /// `[onset, end)` of every other event of this voice touching the measure.
    neighbours: Vec<(ScoreTime, ScoreTime)>,
}

impl NoteDrag {
    /// Whether the dragged event (or its copy) may start at `onset`: inside
    /// the measure, and not overlapping a neighbour of the same voice.
    ///
    /// The model refuses an overlapping voice outright, so this is the same
    /// rule stated early enough to show the user where the wall is.
    fn legal(&self, onset: ScoreTime, copy: bool) -> bool {
        if onset < self.measure_start {
            return false;
        }
        let end = self
            .duration
            .map_or(Ok(onset), |duration| onset.checked_add(duration));
        let Ok(end) = end else { return false };
        if end > self.measure_end {
            return false;
        }
        let overlaps = |(start, finish): &(ScoreTime, ScoreTime)| onset < *finish && end > *start;
        if self.neighbours.iter().any(overlaps) {
            return false;
        }
        // A copy also has to clear the note it was copied from — unless it
        // lands on it, which is how a chord note is added.
        if copy && onset != self.onset {
            let own_end = self
                .duration
                .map_or(self.onset, |duration| {
                    self.onset.checked_add(duration).unwrap_or(self.onset)
                });
            return !overlaps(&(self.onset, own_end));
        }
        true
    }

    /// Page width of one grid slot of `duration_key` inside this measure.
    pub fn slot_width(&self, duration_key: u8) -> f64 {
        let extent = self
            .measure_end
            .checked_sub(self.measure_start)
            .map(|time| rational_f64(time.0))
            .unwrap_or(1.0)
            .max(1e-6);
        let grid = duration_for_key(duration_key)
            .map(|duration| rational_f64(duration.0))
            .unwrap_or(0.25);
        ((self.measure_right - self.measure_left) * grid / extent).max(0.5)
    }
}

/// Why a drag may not be dropped where it is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DragProblem {
    /// Off the keyboard.
    OutOfRange,
    /// Nothing in this measure would leave the voice's rhythm intact.
    NoRoom,
}

impl std::fmt::Display for DragProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::OutOfRange => "outside the keyboard",
            Self::NoRoom => "no room in this bar",
        })
    }
}

/// Where a drag currently points, and whether it may be dropped there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragTarget {
    /// Diatonic staff steps from where the drag started, up positive.
    pub steps: i32,
    pub pitch: Pitch,
    pub midi: u8,
    pub onset: ScoreTime,
    pub page: usize,
    /// Page point the notehead's centre would land on.
    pub at: Point,
    pub problem: Option<DragProblem>,
}

impl DragTarget {
    /// True when dropping here would actually change the score.
    pub fn changes(&self, drag: &NoteDrag) -> bool {
        self.steps != 0 || self.onset != drag.onset
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentError {
    Io(String),
    Native(String),
    ImportUnavailable(&'static str),
    Edit(String),
    InvalidSelection,
}

impl std::fmt::Display for DocumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) | Self::Native(message) | Self::Edit(message) => f.write_str(message),
            Self::ImportUnavailable(kind) => {
                write!(f, "{kind} import is not present in this checkout yet")
            }
            Self::InvalidSelection => f.write_str("select a note first"),
        }
    }
}

impl std::error::Error for DocumentError {}

/// The application-owned loading seam: native score/workspace bytes plus
/// MusicXML and MIDI import.
pub struct ScoreLoader;

impl ScoreLoader {
    pub fn load(path: &Path) -> Result<ScoreWorkspace, DocumentError> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let bytes = std::fs::read(path).map_err(|error| DocumentError::Io(error.to_string()))?;
        if matches!(extension.as_str(), "musicxml" | "mxl" | "xml") {
            let result = makepad_score_import::import_musicxml_bytes(&bytes)
                .map_err(|error| DocumentError::Native(error.to_string()))?;
            return ScoreWorkspace::new(result.score, ACTOR, 32)
                .map_err(|error| DocumentError::Native(error.to_string()));
        }
        if matches!(extension.as_str(), "mid" | "midi") {
            let result = makepad_score_import::import_midi_bytes(&bytes)
                .map_err(|error| DocumentError::Native(error.to_string()))?;
            return ScoreWorkspace::new(result.score, ACTOR, 32)
                .map_err(|error| DocumentError::Native(error.to_string()));
        }
        if let Ok(workspace) = ScoreWorkspace::from_bytes(&bytes) {
            return Ok(workspace);
        }
        let score = Score::from_bytes(&bytes)
            .map_err(|error| DocumentError::Native(error.to_string()))?;
        ScoreWorkspace::new(score, ACTOR, 32)
            .map_err(|error| DocumentError::Native(error.to_string()))
    }
}

pub struct ScoreDocument {
    workspace: ScoreWorkspace,
    path: Option<PathBuf>,
    pages: Vec<Arc<PaintList>>,
    cache: PageCache,
    elements: BTreeMap<SemanticId, SemanticElement>,
    note_semantics: BTreeMap<makepad_score::model::NoteId, SemanticId>,
    measure_semantics: BTreeMap<MeasureId, SemanticId>,
    spacing: ScoreSpacing,
    id_generator: IdGenerator,
    annotation_layer: LayerId,
    frame: u64,
    dirty: bool,
}

impl Default for ScoreDocument {
    fn default() -> Self {
        Self::demo().expect("the built-in score fixture is valid")
    }
}

impl ScoreDocument {
    pub fn demo() -> Result<Self, DocumentError> {
        let (score, next_counter, layer) = demo_score(384)?;
        let workspace = ScoreWorkspace::new(score, ACTOR, 32)
            .map_err(|error| DocumentError::Native(error.to_string()))?;
        Self::from_workspace(workspace, None, next_counter, layer)
    }

    pub fn open(path: PathBuf) -> Result<Self, DocumentError> {
        let workspace = ScoreLoader::load(&path)?;
        let mut generator = IdGenerator::new(ACTOR);
        // Native IDs may come from any actor. Allocating in our actor domain
        // avoids collision; burn a conservative prefix for imported files.
        for _ in 0..10_000 {
            let _ = generator.next::<AnnotationTag>();
        }
        let layer = workspace
            .score()
            .annotation_layers
            .keys()
            .next()
            .copied()
            .unwrap_or(Id::new(ACTOR, 9_999));
        Self::from_workspace(workspace, Some(path), 10_001, layer)
    }

    fn from_workspace(
        workspace: ScoreWorkspace,
        path: Option<PathBuf>,
        next_counter: u64,
        annotation_layer: LayerId,
    ) -> Result<Self, DocumentError> {
        let mut id_generator = IdGenerator::new(ACTOR);
        for _ in 1..next_counter {
            let _ = id_generator.next::<AnnotationTag>();
        }
        let mut result = Self {
            workspace,
            path,
            pages: Vec::new(),
            cache: PageCache::new(192 * 1024 * 1024),
            elements: BTreeMap::new(),
            note_semantics: BTreeMap::new(),
            measure_semantics: BTreeMap::new(),
            spacing: ScoreSpacing::new(),
            id_generator,
            annotation_layer,
            frame: 1,
            dirty: false,
        };
        result.rebuild_all()?;
        Ok(result)
    }

    pub fn score(&self) -> &Score {
        self.workspace.score()
    }

    pub fn workspace(&self) -> &ScoreWorkspace {
        &self.workspace
    }

    pub fn title(&self) -> &str {
        let title = self.workspace.score().title.trim();
        if title.is_empty() { "Untitled Score" } else { title }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The path Save may write to without destroying anything.
    ///
    /// An imported `.mid` or `.musicxml` is a *source*: writing the native
    /// workspace bytes over it would silently replace the user's file with a
    /// format their other tools cannot read, so those go through Save As.
    pub fn native_path(&self) -> Option<&Path> {
        self.path.as_deref().filter(|path| is_native_extension(path))
    }

    /// A sensible Save As default for the loaded document.
    pub fn suggested_save_path(&self) -> PathBuf {
        match &self.path {
            Some(path) => with_native_extension(path),
            None => PathBuf::from(format!("{}.{NATIVE_EXTENSION}", self.title())),
        }
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn pages(&self) -> &[Arc<PaintList>] {
        &self.pages
    }

    pub fn element(&self, semantic: SemanticId) -> Option<&SemanticElement> {
        self.elements.get(&semantic)
    }

    pub fn semantic_for_note(&self, note: makepad_score::model::NoteId) -> Option<SemanticId> {
        self.note_semantics.get(&note).copied()
    }

    pub fn measure_semantic(&self, measure: MeasureId) -> Option<SemanticId> {
        self.measure_semantics.get(&measure).copied()
    }

    pub fn semantic_near_quarter(&self, quarter: f64) -> Option<SemanticId> {
        let whole = quarter.max(0.0) / 4.0;
        let measure = self.score().measures.values().find(|measure| {
            let start = rational_f64(measure.start.0);
            let end = start + rational_f64(measure.extent.0);
            whole >= start && whole < end
        })?;
        self.elements
            .values()
            .find(|element| element.measure == measure.id && element.midi.is_some())
            .map(|element| element.semantic)
    }

    pub fn loop_range_for_selection(&self, selection: &[SemanticId]) -> Option<(f64, f64)> {
        let mut measures: Vec<_> = selection
            .iter()
            .filter_map(|semantic| self.elements.get(semantic))
            .filter_map(|element| self.score().measures.get(&element.measure))
            .collect();
        measures.sort_by_key(|measure| measure.ordinal);
        let first = measures.first()?;
        let last = measures.last()?;
        let end = last.start.checked_add(last.extent).ok()?;
        Some((rational_f64(first.start.0) * 4.0, rational_f64(end.0) * 4.0))
    }

    pub fn last_relayout(&self) -> RelayoutStats {
        self.spacing.stats()
    }

    /// Where a moment in the score sits on the page, from the solved spacing:
    /// the playback cursor rides the real note positions and the real system.
    pub fn locate(&self, whole: f64) -> Option<crate::spacing::CursorLocation> {
        self.spacing.locate(self.score(), whole)
    }

    /// The horizontal spacing and page plan.
    pub fn spacing(&self) -> &ScoreSpacing {
        &self.spacing
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// What Undo and Redo would actually do, so the menu can grey itself.
    pub fn undo_redo_available(&self) -> (bool, bool) {
        (self.workspace.can_undo(), self.workspace.can_redo())
    }

    /// Written duration and articulations of the selected note, formatted for
    /// the inspector. `None` when the selection is not a note.
    pub fn selection_facts(&self, semantic: SemanticId) -> Option<(String, String)> {
        let element = self.elements.get(&semantic)?;
        let event = element.event?;
        let score = self.score();
        let event = score
            .voices
            .values()
            .flat_map(|voice| voice.events.iter())
            .find(|timed| timed.id == event)?;
        let duration = duration_label(event.duration);
        let marks: Vec<&str> = event
            .articulations
            .iter()
            .map(|placed| articulation_label(placed.kind))
            .collect();
        let articulation = if marks.is_empty() {
            "None".to_string()
        } else {
            marks.join(", ")
        };
        Some((duration, articulation))
    }

    /// The score's opening key and meter, as the setup dialog reports them.
    pub fn key_and_meter(&self) -> String {
        let score = self.score();
        let key = score
            .maps
            .key
            .first()
            .map(|change| key_label(&change.value))
            .unwrap_or_else(|| "—".to_string());
        let meter = score
            .maps
            .time_signature
            .first()
            .map(|change| match &change.value {
                Meter::Free => "unmetered".to_string(),
                Meter::Measured { groups, unit } => format!(
                    "{}/{unit}",
                    groups
                        .iter()
                        .map(|group| group.to_string())
                        .collect::<Vec<_>>()
                        .join("+")
                ),
            })
            .unwrap_or_else(|| "—".to_string());
        format!("{key} · {meter}")
    }

    pub fn parts(&self) -> Vec<PartUiState> {
        self.score()
            .parts
            .values()
            .map(|part| PartUiState {
                name: part.name.clone(),
                gain: 0.82,
                pan: 0.0,
                mute: false,
                solo: false,
            })
            .collect()
    }

    pub fn save(&mut self, path: PathBuf) -> Result<(), DocumentError> {
        std::fs::write(&path, self.workspace.to_bytes())
            .map_err(|error| DocumentError::Io(error.to_string()))?;
        self.path = Some(path);
        self.dirty = false;
        Ok(())
    }

    pub fn undo(&mut self) -> Result<(), DocumentError> {
        self.workspace
            .undo()
            .map_err(|error| DocumentError::Edit(error.to_string()))?;
        self.rebuild_all()?;
        self.dirty = true;
        Ok(())
    }

    pub fn redo(&mut self) -> Result<(), DocumentError> {
        self.workspace
            .redo()
            .map_err(|error| DocumentError::Edit(error.to_string()))?;
        self.rebuild_all()?;
        self.dirty = true;
        Ok(())
    }

    pub fn change_pitch(&mut self, semantic: SemanticId, letter: char) -> Result<(), DocumentError> {
        let element = self
            .elements
            .get(&semantic)
            .cloned()
            .ok_or(DocumentError::InvalidSelection)?;
        let note_id = element.note.ok_or(DocumentError::InvalidSelection)?;
        let before = self
            .score()
            .note(note_id)
            .and_then(|note| note.written_pitch)
            .ok_or(DocumentError::InvalidSelection)?;
        let step = match letter.to_ascii_uppercase() {
            'C' => Step::C,
            'D' => Step::D,
            'E' => Step::E,
            'F' => Step::F,
            'G' => Step::G,
            'A' => Step::A,
            'B' => Step::B,
            _ => return Err(DocumentError::InvalidSelection),
        };
        let pitch = Pitch::new(step, Alter::NATURAL, before.octave);
        self.workspace
            .transact(vec![EditCommand::ChangePitch { note: note_id, pitch }])
            .map_err(|error| DocumentError::Edit(error.to_string()))?;
        self.finish_measure_edit(element.measure)
    }

    /// Sharpen, flatten, or naturalise the selected note. This is a written
    /// pitch edit, which is what makes the engraver draw the accidental and
    /// the sampler play the new note; a decoration would do neither.
    pub fn set_alter(&mut self, semantic: SemanticId, semitones: i64) -> Result<(), DocumentError> {
        let element = self
            .elements
            .get(&semantic)
            .cloned()
            .ok_or(DocumentError::InvalidSelection)?;
        let note_id = element.note.ok_or(DocumentError::InvalidSelection)?;
        let before = self
            .score()
            .note(note_id)
            .and_then(|note| note.written_pitch)
            .ok_or(DocumentError::InvalidSelection)?;
        let alter = Alter::new(semitones, 1)
            .map_err(|error| DocumentError::Edit(error.to_string()))?;
        if before.alter == alter {
            return Ok(());
        }
        let pitch = Pitch::new(before.step, alter, before.octave);
        self.workspace
            .transact(vec![EditCommand::ChangePitch { note: note_id, pitch }])
            .map_err(|error| DocumentError::Edit(error.to_string()))?;
        self.finish_measure_edit(element.measure)
    }

    pub fn set_duration(&mut self, semantic: SemanticId, key: u8) -> Result<(), DocumentError> {
        let element = self
            .elements
            .get(&semantic)
            .cloned()
            .ok_or(DocumentError::InvalidSelection)?;
        let event = element.event.ok_or(DocumentError::InvalidSelection)?;
        let duration = duration_for_key(key)?;
        self.workspace
            .transact(vec![EditCommand::ChangeDuration { event, duration }])
            .map_err(|error| DocumentError::Edit(error.to_string()))?;
        self.finish_measure_edit(element.measure)
    }

    /// Mouse entry overwrites an existing chord at the quantized beat or
    /// inserts a new semantic event through the model edit journal.
    pub fn enter_note(
        &mut self,
        measure_semantic: SemanticId,
        midi: u8,
        horizontal_fraction: f64,
        duration_key: u8,
    ) -> Result<SemanticId, DocumentError> {
        let target = self
            .elements
            .get(&measure_semantic)
            .cloned()
            .filter(|element| element.kind == SemanticKind::Measure)
            .ok_or(DocumentError::InvalidSelection)?;
        let measure = self
            .score()
            .measures
            .get(&target.measure)
            .cloned()
            .ok_or(DocumentError::InvalidSelection)?;
        let beat = (horizontal_fraction.clamp(0.0, 0.999_999) * 4.0).floor() as i64;
        let offset = ScoreTime::new(beat, 4)
            .map_err(|error| DocumentError::Edit(error.to_string()))?;
        let onset = measure
            .start
            .checked_add_time(offset)
            .map_err(|error| DocumentError::Edit(error.to_string()))?;
        let existing = self
            .score()
            .voices
            .get(&target.voice)
            .and_then(|voice| voice.events.iter().find(|event| event.onset == onset))
            .cloned();
        if let Some(note) = existing.as_ref().and_then(|event| event.chord_notes().first()) {
            let note_id = note.id;
            self.workspace
                .transact(vec![EditCommand::ChangePitch {
                    note: note_id,
                    pitch: pitch_from_midi(midi),
                }])
                .map_err(|error| DocumentError::Edit(error.to_string()))?;
            self.finish_measure_edit(target.measure)?;
            return Ok(semantic_for_note(note_id));
        }

        let event_id = self
            .id_generator
            .next::<EventTag>()
            .map_err(|_| DocumentError::Edit("event id space exhausted".into()))?;
        let note_id = self
            .id_generator
            .next::<NoteTag>()
            .map_err(|_| DocumentError::Edit("note id space exhausted".into()))?;
        let mut duration = duration_for_key(duration_key)?;
        let measure_end = measure
            .start
            .checked_add(measure.extent)
            .map_err(|error| DocumentError::Edit(error.to_string()))?;
        if onset
            .checked_add(duration)
            .map_err(|error| DocumentError::Edit(error.to_string()))?
            > measure_end
        {
            duration = Duration::new(1, 4)
                .map_err(|error| DocumentError::Edit(error.to_string()))?;
        }
        let event = note_event(
            event_id,
            note_id,
            target.staff,
            onset,
            duration,
            pitch_from_midi(midi),
        );
        let mut commands = Vec::with_capacity(2);
        if let Some(existing) = existing {
            commands.push(EditCommand::DeleteEvent { event: existing.id });
        }
        commands.push(EditCommand::InsertEvent {
            voice: target.voice,
            event,
        });
        self.workspace
            .transact(commands)
            .map_err(|error| DocumentError::Edit(error.to_string()))?;
        self.finish_measure_edit(target.measure)?;
        Ok(semantic_for_note(note_id))
    }

    /// Captures everything a note drag needs, once, when the gesture starts.
    ///
    /// A drag is resolved against this snapshot rather than against the live
    /// score, so the gesture stays cheap (no re-query per pointer sample) and
    /// coherent (the legal window cannot move under the pointer).
    pub fn begin_note_drag(&self, semantic: SemanticId) -> Option<NoteDrag> {
        let element = self.elements.get(&semantic)?;
        if element.kind != SemanticKind::Note {
            return None;
        }
        let note_id = element.note?;
        let event_id = element.event?;
        let score = self.score();
        let pitch = score.note(note_id)?.written_pitch?;
        let event = score.event(event_id)?;
        let measure = score.measures.get(&element.measure)?;
        let measure_end = measure.start.checked_add(measure.extent).ok()?;
        let voice = score.voices.get(&element.voice)?;
        let neighbours = voice
            .events
            .iter()
            .filter(|other| other.id != event_id)
            .filter_map(|other| Some((other.onset, other.end().ok()?)))
            .filter(|(onset, end)| *end > measure.start && *onset < measure_end)
            .collect();
        let measure_bounds = self
            .measure_semantics
            .get(&element.measure)
            .and_then(|semantic| self.elements.get(semantic))
            .map(|element| element.bounds)
            .unwrap_or(element.bounds);
        Some(NoteDrag {
            semantic,
            note: note_id,
            event: event_id,
            voice: element.voice,
            display_staff: score.note(note_id)?.display_staff,
            measure: element.measure,
            page: element.page,
            origin: element.bounds.center(),
            diatonic: i32::from(pitch.octave) * 7 + i32::from(pitch.step.index()),
            alter: rational_f64(pitch.alter.0).round() as i32,
            midi: pitch_to_midi(pitch),
            onset: event.onset,
            duration: event.duration,
            measure_start: measure.start,
            measure_end,
            measure_left: measure_bounds.min.x,
            measure_right: measure_bounds.max.x,
            key_fifths: score
                .maps
                .key_at(measure.start, None, None)
                .map_or(0, |key| key.fifths),
            neighbours,
        })
    }

    /// Where a drag of `steps` diatonic staff steps and `slots` metrical grid
    /// slots would land, and whether it may land there.
    ///
    /// Pure: the score is only read. The canvas calls this on every pointer
    /// sample to draw the target and audition the pitch, and once more at drop
    /// to decide what to commit.
    pub fn resolve_note_drag(
        &self,
        drag: &NoteDrag,
        steps: i32,
        slots: i32,
        duration_key: u8,
        copy: bool,
    ) -> DragTarget {
        let diatonic = drag.diatonic + steps;
        // A pure octave move keeps the note's own accidental; a move to a new
        // letter takes the one the key signature is already sounding.
        let alter = if diatonic.rem_euclid(7) == drag.diatonic.rem_euclid(7) {
            drag.alter
        } else {
            key_alter(drag.key_fifths, diatonic.rem_euclid(7) as usize)
        };
        let pitch = pitch_from_diatonic(diatonic, alter);
        let midi = pitch_to_midi(pitch);
        let grid = duration_for_key(duration_key)
            .or_else(|_| Duration::new(1, 4))
            .unwrap_or(drag.duration.unwrap_or(Duration(Rational::ONE)));

        let mut problem = (!(PIANO_LOWEST..=PIANO_HIGHEST).contains(&midi))
            .then_some(DragProblem::OutOfRange);
        let onset = match self.drag_onset(drag, slots, grid, copy) {
            Some(onset) => onset,
            None => {
                problem = problem.or(Some(DragProblem::NoRoom));
                drag.onset
            }
        };
        let at = Point::new(
            drag.origin.x + self.column_shift(drag.onset, onset),
            drag.origin.y - f64::from(steps) * 0.5,
        );
        DragTarget {
            steps,
            pitch,
            midi,
            onset,
            page: drag.page,
            at,
            problem,
        }
    }

    /// The onset a horizontal drag settles on.
    ///
    /// The drag walks the metrical grid outwards from the note's own beat and
    /// keeps the last slot it could legally occupy, so a note in the way is a
    /// **wall**: the drag stops in front of it instead of teleporting past it
    /// into the next hole, and never writes a voice that overlaps itself.
    fn drag_onset(
        &self,
        drag: &NoteDrag,
        slots: i32,
        grid: Duration,
        copy: bool,
    ) -> Option<ScoreTime> {
        let mut best = drag.legal(drag.onset, copy).then_some(drag.onset);
        if slots == 0 {
            // Zero movement must not re-quantize a note that is already off
            // the grid, or every drag would start with a sideways jump.
            return best;
        }
        let span = rational_f64(grid.0).max(1e-9);
        let from_start = rational_f64(drag.onset.0) - rational_f64(drag.measure_start.0);
        let base = (from_start / span).round() as i64;
        let toward = i64::from(slots.signum());
        for step in 0..=i64::from(slots.unsigned_abs().min(MAX_GRID_SCAN)) {
            let index = base + step * toward;
            let offset = grid.0.checked_mul(Rational::new(index, 1).ok()?).ok()?;
            let onset = drag
                .measure_start
                .checked_add_time(ScoreTime(offset))
                .ok()?;
            if !drag.legal(onset, copy) {
                break;
            }
            best = Some(onset);
        }
        best
    }

    /// Where a drag target sits in its measure, counted in beats from one.
    pub fn beat_in_measure(&self, drag: &NoteDrag, target: &DragTarget) -> f64 {
        let unit = match self
            .score()
            .maps
            .meter_at(drag.measure_start, None, None)
        {
            Some(Meter::Measured { unit, .. }) if *unit > 0 => f64::from(*unit),
            _ => 4.0,
        };
        let from_start = target
            .onset
            .checked_sub(drag.measure_start)
            .map(|time| rational_f64(time.0))
            .unwrap_or(0.0);
        from_start * unit + 1.0
    }

    /// How far a column moves on the page between two onsets, from the solved
    /// spacing rather than from a nominal grid.
    fn column_shift(&self, from: ScoreTime, to: ScoreTime) -> f64 {
        if from == to {
            return 0.0;
        }
        let score = self.score();
        let at = |time: ScoreTime| self.spacing.locate(score, rational_f64(time.0));
        match (at(from), at(to)) {
            (Some(from), Some(to)) => to.x_sp - from.x_sp,
            _ => 0.0,
        }
    }

    /// Applies a resolved drag as **one** journal transaction, so one gesture
    /// is one undo step however many pointer samples it took.
    pub fn commit_note_drag(
        &mut self,
        drag: &NoteDrag,
        target: &DragTarget,
        copy: bool,
    ) -> Result<SemanticId, DocumentError> {
        if let Some(problem) = target.problem {
            return Err(DocumentError::Edit(problem.to_string()));
        }
        let semantic = if copy {
            self.copy_note_drag(drag, target)?
        } else {
            let mut commands = Vec::with_capacity(2);
            if target.pitch != self.dragged_pitch(drag)? {
                commands.push(EditCommand::ChangePitch {
                    note: drag.note,
                    pitch: target.pitch,
                });
            }
            if target.onset != drag.onset {
                commands.push(EditCommand::MoveEvent {
                    event: drag.event,
                    onset: target.onset,
                });
            }
            if commands.is_empty() {
                return Ok(drag.semantic);
            }
            self.workspace
                .transact(commands)
                .map_err(|error| DocumentError::Edit(error.to_string()))?;
            drag.semantic
        };
        self.finish_measure_edit(drag.measure)?;
        Ok(semantic)
    }

    fn dragged_pitch(&self, drag: &NoteDrag) -> Result<Pitch, DocumentError> {
        self.score()
            .note(drag.note)
            .and_then(|note| note.written_pitch)
            .ok_or(DocumentError::InvalidSelection)
    }

    /// Alt-drag. On the note's own onset the copy joins its chord; anywhere
    /// else it becomes a new event of the same voice and length.
    fn copy_note_drag(
        &mut self,
        drag: &NoteDrag,
        target: &DragTarget,
    ) -> Result<SemanticId, DocumentError> {
        let note_id = self
            .id_generator
            .next::<NoteTag>()
            .map_err(|_| DocumentError::Edit("note id space exhausted".into()))?;
        let commands = if target.onset == drag.onset {
            let mut event = self
                .score()
                .event(drag.event)
                .cloned()
                .ok_or(DocumentError::InvalidSelection)?;
            let EventKind::Chord(notes) = &mut event.kind else {
                return Err(DocumentError::InvalidSelection);
            };
            notes.push(Note {
                id: note_id,
                written_pitch: Some(target.pitch),
                unpitched_sound: None,
                display_staff: drag.display_staff,
                tie_from: None,
                tie_to: None,
                tab: None,
                notehead: Notehead::Normal,
            });
            vec![
                EditCommand::DeleteEvent { event: drag.event },
                EditCommand::InsertEvent {
                    voice: drag.voice,
                    event,
                },
            ]
        } else {
            let event_id = self
                .id_generator
                .next::<EventTag>()
                .map_err(|_| DocumentError::Edit("event id space exhausted".into()))?;
            let duration = drag
                .duration
                .map(Ok)
                .unwrap_or_else(|| Duration::new(1, 4))
                .map_err(|error| DocumentError::Edit(error.to_string()))?;
            vec![EditCommand::InsertEvent {
                voice: drag.voice,
                event: note_event(
                    event_id,
                    note_id,
                    drag.display_staff,
                    target.onset,
                    duration,
                    target.pitch,
                ),
            }]
        };
        self.workspace
            .transact(commands)
            .map_err(|error| DocumentError::Edit(error.to_string()))?;
        Ok(semantic_for_note(note_id))
    }

    pub fn apply_articulation(
        &mut self,
        semantic: SemanticId,
        articulation: Articulation,
    ) -> Result<(), DocumentError> {
        let element = self
            .elements
            .get(&semantic)
            .cloned()
            .ok_or(DocumentError::InvalidSelection)?;
        let event_id = element.event.ok_or(DocumentError::InvalidSelection)?;
        let mut values = self
            .score()
            .event(event_id)
            .ok_or(DocumentError::InvalidSelection)?
            .articulations
            .clone();
        if let Some(index) = values.iter().position(|value| value.kind == articulation) {
            values.remove(index);
        } else {
            values.push(PlacedArticulation {
                kind: articulation,
                placement: Some(Placement::Above),
            });
        }
        self.workspace
            .transact(vec![EditCommand::SetArticulations {
                event: event_id,
                articulations: values,
            }])
            .map_err(|error| DocumentError::Edit(error.to_string()))?;
        self.finish_measure_edit(element.measure)
    }

    pub fn add_annotation(
        &mut self,
        semantic: SemanticId,
        kind: AnnotationKind,
        text: Option<String>,
    ) -> Result<(), DocumentError> {
        let target = self
            .elements
            .get(&semantic)
            .cloned()
            .ok_or(DocumentError::InvalidSelection)?;
        let id = self
            .id_generator
            .next::<AnnotationTag>()
            .map_err(|_| DocumentError::Edit("annotation id space exhausted".into()))?;
        let annotation = self.annotation_for_target(id, &target, kind, text, None)?;
        self.workspace
            .transact(vec![EditCommand::PutAnnotation(Some(annotation), id)])
            .map_err(|error| DocumentError::Edit(error.to_string()))?;
        self.dirty = true;
        Ok(())
    }

    pub fn add_ink_annotation(
        &mut self,
        semantic: SemanticId,
        page_points: &[Point],
    ) -> Result<(), DocumentError> {
        let target = self
            .elements
            .get(&semantic)
            .cloned()
            .ok_or(DocumentError::InvalidSelection)?;
        if page_points.len() < 2 {
            return Ok(());
        }
        let id = self
            .id_generator
            .next::<AnnotationTag>()
            .map_err(|_| DocumentError::Edit("annotation id space exhausted".into()))?;
        let width = target.bounds.width().max(0.01);
        let height = target.bounds.height().max(0.01);
        let local_points: Vec<_> = page_points
            .iter()
            .enumerate()
            .map(|(index, point)| LocalInkPoint {
                u: decimal_rational((point.x - target.bounds.min.x) / width),
                v: decimal_rational((point.y - target.bounds.min.y) / height),
                pressure: u16::MAX / 2,
                tilt: 0,
                azimuth: 0,
                elapsed_micros: (index as u32).saturating_mul(8_000),
            })
            .collect();
        let element_ref = target
            .note
            .map(ElementRef::Note)
            .or_else(|| target.event.map(ElementRef::Event))
            .unwrap_or(ElementRef::Measure(target.measure));
        let ink_anchor = InkAnchor::ElementLocal {
            target: element_ref,
            points: local_points,
        };
        let annotation = self.annotation_for_target(
            id,
            &target,
            AnnotationKind::Ink,
            None,
            Some((ink_anchor, page_points)),
        )?;
        self.workspace
            .transact(vec![EditCommand::PutAnnotation(Some(annotation), id)])
            .map_err(|error| DocumentError::Edit(error.to_string()))?;
        self.dirty = true;
        Ok(())
    }

    fn annotation_for_target(
        &self,
        id: AnnotationId,
        target: &SemanticElement,
        kind: AnnotationKind,
        text: Option<String>,
        ink: Option<(InkAnchor, &[Point])>,
    ) -> Result<Annotation, DocumentError> {
        let score = self.score();
        let measure = score
            .measures
            .get(&target.measure)
            .ok_or(DocumentError::InvalidSelection)?;
        let event = target.event.and_then(|event| score.event(event));
        let start = event.map_or(measure.start, |event| event.onset);
        let end = event
            .and_then(|event| event.end().ok())
            .unwrap_or_else(|| measure.start.checked_add(measure.extent).unwrap_or(measure.start));
        let fallback = BeatRange {
            staff: target.staff,
            voice: Some(target.voice),
            start,
            end,
        };
        let (primary, fingerprint) = if let Some(note) = target.note {
            (
                AnchorTarget::Note(note),
                score.note_fingerprint(note).unwrap_or(ContextFingerprint([0; 16])),
            )
        } else if let Some(event) = target.event {
            (
                AnchorTarget::Event(event),
                score.event_fingerprint(event).unwrap_or(ContextFingerprint([0; 16])),
            )
        } else {
            (
                AnchorTarget::Measure(target.measure),
                score
                    .measure_fingerprint(target.measure)
                    .unwrap_or(ContextFingerprint([0; 16])),
            )
        };
        let ink_anchor = ink.as_ref().map(|(anchor, _)| anchor.clone());
        let body = if let Some((anchor, points)) = ink {
            AnnotationBody::Ink(InkStroke {
                anchor_kind: anchor,
                original_polyline: points
                    .iter()
                    .map(|point| SourcePoint {
                        x: decimal_rational(point.x / PAGE_WIDTH_SP),
                        y: decimal_rational(point.y / PAGE_HEIGHT_SP),
                    })
                    .collect(),
                color_rgba: [215, 104, 67, 230],
                nominal_width_milli_staff_space: 180,
            })
        } else {
            match kind {
                AnnotationKind::Text => AnnotationBody::Text(text.unwrap_or_else(|| "Note".into())),
                AnnotationKind::Fingering => {
                    AnnotationBody::Fingering(text.unwrap_or_else(|| "1".into()))
                }
                _ => AnnotationBody::None,
            }
        };
        Ok(Annotation {
            id,
            layer: self.annotation_layer,
            kind,
            anchor: SemanticAnchor {
                primary,
                fallback,
                affinity: Affinity::On,
                context_fingerprint: fingerprint,
                ink: ink_anchor,
            },
            body,
            style: AnnotationStyle {
                color_rgba: [215, 104, 67, 190],
                width_milli_staff_space: 180,
            },
            action: None,
            author: [0x5c; 16],
            created_lamport: self.workspace.journal().len() as u64 + 1,
            modified_lamport: self.workspace.journal().len() as u64 + 1,
        })
    }

    pub fn annotation_visuals(&self) -> Vec<AnnotationVisual> {
        self.score()
            .annotations
            .values()
            .filter_map(|annotation| {
                let semantic = match annotation.anchor.primary {
                    AnchorTarget::Note(note) => self.note_semantics.get(&note).copied(),
                    AnchorTarget::Event(event) => self
                        .elements
                        .values()
                        .find(|element| element.event == Some(event))
                        .map(|element| element.semantic),
                    AnchorTarget::Measure(measure) => self.measure_semantics.get(&measure).copied(),
                    _ => None,
                }?;
                let text = match &annotation.body {
                    AnnotationBody::Text(text) | AnnotationBody::Fingering(text) => Some(text.clone()),
                    _ => None,
                };
                let ink_points = match &annotation.body {
                    AnnotationBody::Ink(stroke) => stroke
                        .original_polyline
                        .iter()
                        .map(|point| Point::new(
                            rational_f64(point.x) * PAGE_WIDTH_SP,
                            rational_f64(point.y) * PAGE_HEIGHT_SP,
                        ))
                        .collect(),
                    _ => Vec::new(),
                };
                Some(AnnotationVisual {
                    id: annotation.id,
                    kind: annotation.kind.clone(),
                    semantic,
                    text,
                    color: annotation.style.color_rgba,
                    ink_points,
                })
            })
            .collect()
    }

    pub fn select_more(&self, selection: &[SemanticId]) -> Vec<SemanticId> {
        let Some(active) = selection.last().and_then(|id| self.elements.get(id)) else {
            return Vec::new();
        };
        if selection.len() == 1 {
            let event_notes: Vec<_> = self
                .elements
                .values()
                .filter(|element| element.kind == SemanticKind::Note && element.event == active.event)
                .map(|element| element.semantic)
                .collect();
            if event_notes.len() > 1 {
                return event_notes;
            }
        }
        let measure_notes: Vec<_> = self
            .elements
            .values()
            .filter(|element| element.kind == SemanticKind::Note && element.measure == active.measure)
            .map(|element| element.semantic)
            .collect();
        let current: BTreeSet<_> = selection.iter().copied().collect();
        if measure_notes.iter().any(|id| !current.contains(id)) {
            return measure_notes;
        }
        self.elements
            .values()
            .filter(|element| element.kind == SemanticKind::Note)
            .map(|element| element.semantic)
            .collect()
    }

    pub fn next_note_semantic(&self, semantic: SemanticId) -> Option<SemanticId> {
        let mut notes: Vec<_> = self
            .elements
            .values()
            .filter(|element| element.kind == SemanticKind::Note)
            .collect();
        notes.sort_by_key(|element| {
            let onset = element
                .event
                .and_then(|event| self.score().event(event))
                .map(|event| event.onset)
                .unwrap_or(ScoreTime::ZERO);
            (onset, element.staff, element.semantic)
        });
        let index = notes.iter().position(|element| element.semantic == semantic)?;
        notes.get(index + 1).map(|element| element.semantic)
    }

    pub fn all_note_semantics(&self) -> Vec<SemanticId> {
        self.elements
            .values()
            .filter(|element| element.kind == SemanticKind::Note)
            .map(|element| element.semantic)
            .collect()
    }

    fn finish_measure_edit(&mut self, measure: MeasureId) -> Result<(), DocumentError> {
        let ordinal = self
            .score()
            .measures
            .get(&measure)
            .map(|measure| measure.ordinal as usize)
            .ok_or(DocumentError::InvalidSelection)?;
        // The kernel re-summarizes the edited measure, keeps the breaks when
        // it legally can, and only then reflows; the pages it reports back
        // are the only ones repainted.
        match self.spacing.touch_measure(self.workspace.score(), ordinal) {
            PagesDirty::All => self.rebuild_all()?,
            PagesDirty::Only(pages) => {
                for page in pages {
                    self.rebuild_page(page)?;
                }
            }
        }
        self.dirty = true;
        Ok(())
    }

    fn rebuild_all(&mut self) -> Result<(), DocumentError> {
        self.spacing.rebuild(self.workspace.score());
        self.elements.clear();
        self.note_semantics.clear();
        self.measure_semantics.clear();
        self.pages.clear();
        for page in 0..self.spacing.page_count() {
            let (list, elements) = {
                let placement = &self.spacing.pages()[page];
                crate::engrave::make_page(self.workspace.score(), placement, page, self.frame)?
            };
            let list = Arc::new(list);
            self.cache.insert(list.clone(), self.frame);
            self.pages.push(list);
            self.insert_elements(elements);
            self.frame = self.frame.saturating_add(1);
        }
        Ok(())
    }

    fn rebuild_page(&mut self, page: usize) -> Result<(), DocumentError> {
        if page >= self.pages.len() || page >= self.spacing.page_count() {
            return self.rebuild_all();
        }
        self.elements.retain(|_, element| element.page != page);
        self.note_semantics
            .retain(|_, semantic| self.elements.contains_key(semantic));
        self.measure_semantics
            .retain(|_, semantic| self.elements.contains_key(semantic));
        let (list, elements) = {
            let placement = &self.spacing.pages()[page];
            crate::engrave::make_page(self.workspace.score(), placement, page, self.frame)?
        };
        let list = Arc::new(list);
        self.cache.insert(list.clone(), self.frame);
        self.pages[page] = list;
        self.insert_elements(elements);
        self.frame = self.frame.saturating_add(1);
        Ok(())
    }

    fn insert_elements(&mut self, elements: Vec<SemanticElement>) {
        for element in elements {
            if let Some(note) = element.note {
                self.note_semantics.insert(note, element.semantic);
            }
            if element.kind == SemanticKind::Measure {
                self.measure_semantics.insert(element.measure, element.semantic);
            }
            self.elements.insert(element.semantic, element);
        }
    }
}

pub(crate) fn semantic_for_note(id: makepad_score::model::NoteId) -> SemanticId {
    let (actor, counter) = id.raw();
    SemanticId(NOTE_SEMANTIC_TAG | counter ^ actor.rotate_left(17))
}

pub(crate) fn semantic_for_measure(id: MeasureId) -> SemanticId {
    let (actor, counter) = id.raw();
    SemanticId(MEASURE_SEMANTIC_TAG | counter ^ actor.rotate_left(11))
}

/// Written duration as a musician reads it.
fn duration_label(duration: Option<Duration>) -> String {
    let Some(duration) = duration else {
        return "grace".to_string();
    };
    let numerator = duration.0.numerator();
    let denominator = duration.0.denominator();
    let name = match (numerator, denominator) {
        (1, 1) => Some("whole"),
        (1, 2) => Some("half"),
        (1, 4) => Some("quarter"),
        (1, 8) => Some("eighth"),
        (1, 16) => Some("16th"),
        (1, 32) => Some("32nd"),
        (1, 64) => Some("64th"),
        (3, 2) => Some("dotted whole"),
        (3, 4) => Some("dotted half"),
        (3, 8) => Some("dotted quarter"),
        (3, 16) => Some("dotted eighth"),
        _ => None,
    };
    match name {
        Some(name) => name.to_string(),
        None => format!("{numerator}/{denominator}"),
    }
}

fn articulation_label(articulation: Articulation) -> &'static str {
    match articulation {
        Articulation::Accent => "accent",
        Articulation::Staccato => "staccato",
        Articulation::Tenuto => "tenuto",
        Articulation::Staccatissimo => "staccatissimo",
        Articulation::Marcato => "marcato",
        Articulation::LaissezVibrer => "laissez vibrer",
        Articulation::Stress => "stress",
        Articulation::SoftAccent => "soft accent",
        Articulation::AccentStaccato => "accent staccato",
        Articulation::TenutoStaccato => "tenuto staccato",
        Articulation::MarcatoStaccato => "marcato staccato",
        Articulation::MarcatoTenuto => "marcato tenuto",
    }
}

/// Major/minor key name from the circle of fifths.
fn key_label(key: &KeySignature) -> String {
    const SHARPS: [&str; 8] = ["C", "G", "D", "A", "E", "B", "F#", "C#"];
    const FLATS: [&str; 8] = ["C", "F", "Bb", "Eb", "Ab", "Db", "Gb", "Cb"];
    let fifths = key.fifths;
    let name = if fifths >= 0 {
        SHARPS.get(fifths as usize).copied()
    } else {
        FLATS.get((-fifths) as usize).copied()
    };
    match name {
        Some(name) => format!("{name} major"),
        None => format!("{fifths} fifths"),
    }
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

fn decimal_rational(value: f64) -> Rational {
    let value = if value.is_finite() { value } else { 0.0 };
    Rational::new((value * 10_000.0).round() as i64, 10_000).unwrap_or(Rational::ZERO)
}

fn rational_f64(value: Rational) -> f64 {
    value.numerator() as f64 / value.denominator() as f64
}

fn demo_score(measure_count: usize) -> Result<(Score, u64, LayerId), DocumentError> {
    let mut ids = IdGenerator::new(ACTOR);
    let piano = ids.next::<PartTag>().map_err(id_error)?;
    let treble = ids.next::<StaffTag>().map_err(id_error)?;
    let bass = ids.next::<StaffTag>().map_err(id_error)?;
    let right_hand = ids.next::<VoiceTag>().map_err(id_error)?;
    let left_hand = ids.next::<VoiceTag>().map_err(id_error)?;
    let layer = ids.next::<LayerTag>().map_err(id_error)?;
    let mut score = Score::new(*b"MAKEPADSCOREDEMO");
    score.title = "Nocturne in Makepad".into();
    score.parts.insert(
        piano,
        Part {
            id: piano,
            name: "Piano".into(),
            staves: vec![treble, bass],
            transposition: Transposition::NONE,
        },
    );
    score.staves.insert(
        treble,
        Staff {
            id: treble,
            part: piano,
            parent: None,
            kind: StaffKind::Standard,
            voices: vec![right_hand],
        },
    );
    score.staves.insert(
        bass,
        Staff {
            id: bass,
            part: piano,
            parent: Some(treble),
            kind: StaffKind::Standard,
            voices: vec![left_hand],
        },
    );
    let mut upper_events = Vec::with_capacity(measure_count * 4);
    let mut lower_events = Vec::with_capacity(measure_count * 2);
    let melody = [0_i8, 2, 4, 7, 9, 7, 4, 2, 5, 9, 12, 9, 7, 4, 2, -1];
    for measure_index in 0..measure_count {
        let measure_id = ids.next::<MeasureTag>().map_err(id_error)?;
        let start = ScoreTime::new(measure_index as i64, 1)
            .map_err(|error| DocumentError::Native(error.to_string()))?;
        let extent = Duration::new(1, 1).map_err(|error| DocumentError::Native(error.to_string()))?;
        score.measures.insert(
            measure_id,
            Measure {
                id: measure_id,
                ordinal: measure_index as u32,
                label: (measure_index + 1).to_string(),
                start,
                extent,
            },
        );
        score.flow.nodes.push(FlowNode {
            measure: measure_id,
            ordinal: measure_index as u32,
        });
        for beat in 0..4 {
            let event_id = ids.next::<EventTag>().map_err(id_error)?;
            let note_id = ids.next::<NoteTag>().map_err(id_error)?;
            let onset = start
                .checked_add_time(ScoreTime::new(beat, 4).map_err(|error| DocumentError::Native(error.to_string()))?)
                .map_err(|error| DocumentError::Native(error.to_string()))?;
            let offset = melody[(measure_index * 4 + beat as usize) % melody.len()];
            upper_events.push(note_event(
                event_id,
                note_id,
                treble,
                onset,
                Duration::new(1, 4).map_err(|error| DocumentError::Native(error.to_string()))?,
                pitch_from_midi((67_i16 + i16::from(offset)).clamp(48, 88) as u8),
            ));
        }
        for half in 0..2 {
            let event_id = ids.next::<EventTag>().map_err(id_error)?;
            let root_id = ids.next::<NoteTag>().map_err(id_error)?;
            let fifth_id = ids.next::<NoteTag>().map_err(id_error)?;
            let onset = start
                .checked_add_time(ScoreTime::new(half, 2).map_err(|error| DocumentError::Native(error.to_string()))?)
                .map_err(|error| DocumentError::Native(error.to_string()))?;
            let root = 43 + ((measure_index + half as usize) % 7) as u8;
            let mut event = note_event(
                event_id,
                root_id,
                bass,
                onset,
                Duration::new(1, 2).map_err(|error| DocumentError::Native(error.to_string()))?,
                pitch_from_midi(root),
            );
            if let EventKind::Chord(notes) = &mut event.kind {
                notes.push(Note {
                    id: fifth_id,
                    written_pitch: Some(pitch_from_midi(root.saturating_add(7))),
                    unpitched_sound: None,
                    display_staff: bass,
                    tie_from: None,
                    tie_to: None,
                    tab: None,
                    notehead: Notehead::Normal,
                });
            }
            lower_events.push(event);
        }
    }
    score.voices.insert(
        right_hand,
        Voice {
            id: right_hand,
            staff: treble,
            number: 1,
            events: upper_events,
        },
    );
    score.voices.insert(
        left_hand,
        Voice {
            id: left_hand,
            staff: bass,
            number: 1,
            events: lower_events,
        },
    );
    score.maps.time_signature.push(Change {
        at: ScoreTime::ZERO,
        scope: MapScope::Global,
        value: Meter::Measured {
            groups: vec![4],
            unit: 4,
        },
    });
    score.annotation_layers.insert(
        layer,
        AnnotationLayer {
            id: layer,
            title: "Personal markings".into(),
            owner: [0x5c; 16],
            color_hint: [215, 104, 67, 190],
            visible_by_default: true,
            scope: LayerScope::AllScore,
            permissions: LayerPermissions::Private,
            export_policy: ExportPolicy::Include,
        },
    );
    Ok((score, ids.next::<AnnotationTag>().map_err(id_error)?.counter() + 1, layer))
}

fn id_error(_: IdError) -> DocumentError {
    DocumentError::Native("built-in score id space exhausted".into())
}

fn note_event(
    event: EventId,
    note: makepad_score::model::NoteId,
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

fn duration_for_key(key: u8) -> Result<Duration, DocumentError> {
    let (numerator, denominator) = match key {
        1 => (1, 64),
        2 => (1, 32),
        3 => (1, 16),
        4 => (1, 8),
        5 => (1, 4),
        6 => (1, 2),
        7 => (1, 1),
        _ => return Err(DocumentError::InvalidSelection),
    };
    Duration::new(numerator, denominator)
        .map_err(|error| DocumentError::Edit(error.to_string()))
}

/// The alteration a key signature is already sounding on a diatonic step.
fn key_alter(fifths: i8, step: usize) -> i32 {
    const SHARP_ORDER: [usize; 7] = [3, 0, 4, 1, 5, 2, 6];
    const FLAT_ORDER: [usize; 7] = [6, 2, 5, 1, 4, 0, 3];
    let count = fifths.unsigned_abs().min(7) as usize;
    let (order, alter) = if fifths > 0 {
        (SHARP_ORDER, 1)
    } else {
        (FLAT_ORDER, -1)
    };
    if order[..count].contains(&step) {
        alter
    } else {
        0
    }
}

/// A written pitch from a diatonic index and an alteration in semitones.
fn pitch_from_diatonic(diatonic: i32, alter: i32) -> Pitch {
    let step = match diatonic.rem_euclid(7) {
        0 => Step::C,
        1 => Step::D,
        2 => Step::E,
        3 => Step::F,
        4 => Step::G,
        5 => Step::A,
        _ => Step::B,
    };
    Pitch::new(
        step,
        Alter::new(i64::from(alter), 1).unwrap_or(Alter::NATURAL),
        diatonic.div_euclid(7).clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8,
    )
}

fn pitch_from_midi(midi: u8) -> Pitch {
    let octave = midi as i16 / 12 - 1;
    let (step, alter) = match midi % 12 {
        0 => (Step::C, 0),
        1 => (Step::C, 1),
        2 => (Step::D, 0),
        3 => (Step::E, -1),
        4 => (Step::E, 0),
        5 => (Step::F, 0),
        6 => (Step::F, 1),
        7 => (Step::G, 0),
        8 => (Step::A, -1),
        9 => (Step::A, 0),
        10 => (Step::B, -1),
        _ => (Step::B, 0),
    };
    Pitch::new(
        step,
        Alter::new(alter, 1).unwrap_or(Alter::NATURAL),
        octave.clamp(i8::MIN as i16, i8::MAX as i16) as i8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;


    /// Save must never write native workspace bytes over the file a score was
    /// imported from: a `.mid` that came in as MIDI has to go out as a
    /// deliberate Save As, or the user's source file is destroyed.
    #[test]
    fn imported_sources_are_not_save_targets() {
        assert!(!is_native_extension(Path::new("/tmp/prelude.mid")));
        assert!(!is_native_extension(Path::new("/tmp/prelude.musicxml")));
        assert!(is_native_extension(Path::new("/tmp/prelude.mpscore")));
        assert!(is_native_extension(Path::new("/tmp/prelude.MPSCORE")));
        assert_eq!(
            with_native_extension(Path::new("/tmp/prelude.mid")),
            PathBuf::from("/tmp/prelude.mpscore")
        );
        assert_eq!(
            with_native_extension(Path::new("/tmp/prelude.mpscore")),
            PathBuf::from("/tmp/prelude.mpscore")
        );
    }

    #[test]
    fn demo_is_multi_page_and_retained() {
        let document = ScoreDocument::demo().unwrap();
        // 384 measures of four quarters over a half-note bass: the breaker
        // fits seven or so to a system and five systems to a page.
        assert_eq!(document.page_count(), 11);
        assert!(document.pages().iter().all(|page| !page.items().is_empty()));
        assert!(document.all_note_semantics().len() > 2_000);
        // Every measure is placed exactly once.
        let mut placed: Vec<usize> = document
            .spacing()
            .pages()
            .iter()
            .flat_map(|page| page.systems.iter())
            .flat_map(|system| system.measures.iter())
            .map(|measure| measure.index)
            .collect();
        placed.sort_unstable();
        assert_eq!(placed, (0..384).collect::<Vec<_>>());
    }

    #[test]
    fn one_pitch_edit_rebuilds_only_its_measure_neighbourhood() {
        let mut document = ScoreDocument::demo().unwrap();
        let semantic = document.all_note_semantics()[0];
        let before: Vec<_> = document.pages().iter().map(Arc::as_ptr).collect();
        document.change_pitch(semantic, 'D').unwrap();
        let after: Vec<_> = document.pages().iter().map(Arc::as_ptr).collect();
        assert_ne!(before[0], after[0]);
        assert_eq!(&before[1..], &after[1..]);
        // One measure re-measured, one system re-solved, no reflow: the
        // kernel's cheapest rung, on a score of 384 measures.
        let stats = document.last_relayout();
        assert!(!stats.full && !stats.rebreak);
        assert_eq!(stats.measures_summarized, 1);
        assert_eq!(stats.systems_spaced, 1);
    }

    #[test]
    fn justified_systems_end_on_the_right_margin() {
        let document = ScoreDocument::demo().unwrap();
        for page in document.spacing().pages() {
            for system in &page.systems {
                let barline = system.measures.last().unwrap().right;
                assert!(
                    (barline - system.right).abs() < 1e-6,
                    "system closes at {barline}, not the {} margin",
                    system.right
                );
            }
        }
    }

    #[test]
    fn both_staves_share_one_column_x() {
        // The melody's beats 1 and 3 carry the bass chord as well: a shared
        // onset is one column, so those noteheads must line up vertically.
        let document = ScoreDocument::demo().unwrap();
        let mut by_onset: BTreeMap<ScoreTime, Vec<f64>> = BTreeMap::new();
        for semantic in document.all_note_semantics() {
            let Some(element) = document.element(semantic) else { continue };
            if element.page != 0 {
                continue;
            }
            let Some(event) = element.event.and_then(|id| document.score().event(id)) else {
                continue;
            };
            by_onset
                .entry(event.onset)
                .or_default()
                .push(element.bounds.min.x);
        }
        let shared = by_onset.values().filter(|xs| xs.len() > 1).count();
        assert!(shared > 8, "expected shared onsets to test, found {shared}");
        for (onset, xs) in &by_onset {
            let first = xs[0];
            for x in xs {
                assert!(
                    (x - first).abs() < 1e-9,
                    "onset {onset:?} places noteheads at {xs:?}"
                );
            }
        }
    }

    #[test]
    fn mouse_entry_is_journaled_and_semantic() {
        let mut document = ScoreDocument::demo().unwrap();
        let measure = document.score().measures.values().next().unwrap().id;
        let semantic = document.measure_semantic(measure).unwrap();
        let note = document.enter_note(semantic, 84, 0.05, 5).unwrap();
        assert_eq!(document.element(note).and_then(|element| element.midi), Some(84));
        assert_eq!(document.workspace().journal().len(), 1);
    }

    /// A one-measure fixture whose upper voice is exactly `events`, so a drag
    /// has a known grid and known free slots to move into.
    fn document_of(score: Score) -> ScoreDocument {
        let workspace = ScoreWorkspace::new(score, ACTOR, 32).unwrap();
        ScoreDocument::from_workspace(workspace, None, 20_000, Id::new(ACTOR, 19_999)).unwrap()
    }

    fn midi_of(document: &ScoreDocument, semantic: SemanticId) -> Option<u8> {
        document.element(semantic).and_then(|element| element.midi)
    }

    /// The reported symptom was "i cannot drag the notes". A vertical drag
    /// moves the note by exactly the staff steps dragged, and one undo puts it
    /// back — however many pointer samples the gesture took.
    #[test]
    fn dragging_a_note_up_moves_it_by_the_steps_dragged_and_undoes_in_one_step() {
        let mut document = ScoreDocument::demo().unwrap();
        let semantic = document.all_note_semantics()[0];
        let before = midi_of(&document, semantic).unwrap();
        let drag = document.begin_note_drag(semantic).unwrap();
        // The gesture: many samples, one commit.
        let mut target = document.resolve_note_drag(&drag, 0, 0, 5, false);
        for steps in [1, 2, 3, 2] {
            target = document.resolve_note_drag(&drag, steps, 0, 5, false);
            assert!(target.problem.is_none());
        }
        assert!(target.changes(&drag));
        let moved = document.commit_note_drag(&drag, &target, false).unwrap();
        // Two diatonic steps up from G5 in C major is B5: four semitones.
        assert_eq!(midi_of(&document, moved), Some(before + 4));
        assert_eq!(
            document.workspace().journal().len(),
            1,
            "a drag is one transaction, not one per pointer sample"
        );
        document.undo().unwrap();
        assert_eq!(midi_of(&document, semantic), Some(before));
    }

    /// A drag that moves both pitch and onset is still one undo step.
    #[test]
    fn a_drag_of_pitch_and_onset_together_undoes_once() {
        use crate::engrave::tests::{fixture_events, Placed};
        let mut document = document_of(fixture_events(&[
            Placed { onset: (0, 4), duration: (1, 4), step: Step::C, octave: 4 },
            Placed { onset: (2, 4), duration: (1, 4), step: Step::E, octave: 4 },
        ]));
        let semantic = document.all_note_semantics()[0];
        let before = midi_of(&document, semantic).unwrap();
        let drag = document.begin_note_drag(semantic).unwrap();
        let target = document.resolve_note_drag(&drag, 1, 1, 5, false);
        assert_eq!(target.problem, None);
        assert_eq!(target.onset, ScoreTime::new(1, 4).unwrap());
        let moved = document.commit_note_drag(&drag, &target, false).unwrap();
        assert_eq!(midi_of(&document, moved), Some(before + 2));
        assert_eq!(document.workspace().journal().len(), 1);
        document.undo().unwrap();
        let restored = document.all_note_semantics()[0];
        assert_eq!(midi_of(&document, restored), Some(before));
        assert_eq!(
            document
                .element(restored)
                .and_then(|element| element.event)
                .and_then(|event| document.score().event(event))
                .map(|event| event.onset),
            Some(ScoreTime::ZERO)
        );
    }

    /// A horizontal drag lands on the metrical grid, and stops at the note in
    /// its way rather than writing an overlapping voice.
    #[test]
    fn a_horizontal_drag_snaps_to_the_grid_and_walls_at_the_next_note() {
        use crate::engrave::tests::{fixture_events, Placed};
        let document = document_of(fixture_events(&[
            Placed { onset: (0, 4), duration: (1, 4), step: Step::C, octave: 4 },
            Placed { onset: (2, 4), duration: (1, 4), step: Step::E, octave: 4 },
        ]));
        let drag = document.begin_note_drag(document.all_note_semantics()[0]).unwrap();
        // One quarter right is free; the beat after it is taken, so the drag
        // stops on the free one instead of overlapping.
        for (slots, beat) in [(0, 0), (1, 1), (2, 1), (5, 1)] {
            let target = document.resolve_note_drag(&drag, 0, slots, 5, false);
            assert_eq!(target.problem, None, "slots {slots}");
            assert_eq!(
                target.onset,
                ScoreTime::new(beat, 4).unwrap(),
                "dragging {slots} slots should land on beat {beat}"
            );
        }
        // And it never leaves the measure to the left.
        let target = document.resolve_note_drag(&drag, 0, -3, 5, false);
        assert_eq!(target.onset, ScoreTime::ZERO);
    }

    /// A drop off the end of the keyboard is refused, loudly and without
    /// touching the score.
    #[test]
    fn a_drag_off_the_keyboard_is_refused() {
        let mut document = ScoreDocument::demo().unwrap();
        let semantic = document.all_note_semantics()[0];
        let before = midi_of(&document, semantic);
        let drag = document.begin_note_drag(semantic).unwrap();
        let target = document.resolve_note_drag(&drag, 60, 0, 5, false);
        assert_eq!(target.problem, Some(DragProblem::OutOfRange));
        assert!(document.commit_note_drag(&drag, &target, false).is_err());
        assert_eq!(midi_of(&document, semantic), before);
        assert!(document.workspace().journal().is_empty());
    }

    /// A drag that never leaves its own notehead is not an edit.
    #[test]
    fn a_drag_that_changes_nothing_writes_nothing() {
        let mut document = ScoreDocument::demo().unwrap();
        let semantic = document.all_note_semantics()[0];
        let drag = document.begin_note_drag(semantic).unwrap();
        let target = document.resolve_note_drag(&drag, 0, 0, 5, false);
        assert!(!target.changes(&drag));
        assert_eq!(document.commit_note_drag(&drag, &target, false).unwrap(), semantic);
        assert!(document.workspace().journal().is_empty());
    }

    /// Dragging spells the landing pitch with the key signature in force: in
    /// two sharps, a step onto F is F sharp, and an octave move keeps the
    /// note's own accidental.
    #[test]
    fn a_drag_is_spelled_by_the_key_signature() {
        use crate::engrave::tests::{fixture_events, Placed};
        let mut score = fixture_events(&[Placed {
            onset: (0, 4),
            duration: (1, 4),
            step: Step::E,
            octave: 4,
        }]);
        score.maps.key.push(Change {
            at: ScoreTime::ZERO,
            scope: MapScope::Global,
            value: KeySignature { fifths: 2, custom: Vec::new() },
        });
        let document = document_of(score);
        let drag = document.begin_note_drag(document.all_note_semantics()[0]).unwrap();
        let up = document.resolve_note_drag(&drag, 1, 0, 5, false);
        assert_eq!(up.pitch.step, Step::F);
        assert_eq!(up.pitch.alter, Alter::new(1, 1).unwrap(), "F is sharp in D major");
        let octave = document.resolve_note_drag(&drag, 7, 0, 5, false);
        assert_eq!(octave.pitch.step, Step::E);
        assert_eq!(octave.pitch.octave, 5);
    }

    /// Alt-drag copies: onto the note's own onset it joins the chord, anywhere
    /// else it is a new event — and either way, one undo removes it.
    #[test]
    fn a_copy_drag_adds_a_note_and_undoes_in_one_step() {
        use crate::engrave::tests::{fixture_events, Placed};
        let mut document = document_of(fixture_events(&[Placed {
            onset: (0, 4),
            duration: (1, 4),
            step: Step::C,
            octave: 4,
        }]));
        let notes = document.all_note_semantics().len();
        let drag = document.begin_note_drag(document.all_note_semantics()[0]).unwrap();
        let chord = document.resolve_note_drag(&drag, 2, 0, 5, true);
        let added = document.commit_note_drag(&drag, &chord, true).unwrap();
        assert_eq!(document.all_note_semantics().len(), notes + 1);
        assert_eq!(midi_of(&document, added), Some(64), "a third above middle C");
        assert_eq!(document.workspace().journal().len(), 1);
        document.undo().unwrap();
        assert_eq!(document.all_note_semantics().len(), notes);

        // The same gesture one beat over inserts a separate event.
        let drag = document.begin_note_drag(document.all_note_semantics()[0]).unwrap();
        let apart = document.resolve_note_drag(&drag, 2, 1, 5, true);
        assert_eq!(apart.onset, ScoreTime::new(1, 4).unwrap());
        document.commit_note_drag(&drag, &apart, true).unwrap();
        assert_eq!(document.score().voices[&drag.voice].events.len(), 2);
    }

    #[test]
    fn native_round_trip_keeps_annotations_semantic() {
        let mut document = ScoreDocument::demo().unwrap();
        let semantic = document.all_note_semantics()[0];
        document
            .add_annotation(semantic, AnnotationKind::Fingering, Some("3".into()))
            .unwrap();
        let bytes = document.workspace().to_bytes();
        let workspace = ScoreWorkspace::from_bytes(&bytes).unwrap();
        assert_eq!(workspace.score().annotations.len(), 1);
        let annotation = workspace.score().annotations.values().next().unwrap();
        assert!(matches!(annotation.anchor.primary, AnchorTarget::Note(_)));
    }
}
