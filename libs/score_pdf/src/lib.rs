//! Semantic recovery and reversible local editing for engraved PDF scores.
//!
//! The original PDF remains immutable. Ingest produces a lossless decoded
//! operator ledger plus page-space primitives, then an evidence-backed score
//! hypothesis. Applying a splice records intent; export appends a new PDF
//! revision and leaves every original byte untouched.

mod confidence;
mod display;
mod export;
mod geometry;
mod music;
mod normalize;
mod provenance;
mod recover;
mod sha256;
mod splice;

pub use confidence::*;
pub use display::*;
pub use export::*;
pub use geometry::*;
pub use music::*;
pub use normalize::*;
pub use provenance::*;
pub use recover::*;
pub use splice::*;

use makepad_pdf_parse::PdfDocument;
use makepad_score::model::{
    BeamMembership, Change, Duration, EventKind, GlobalMaps, Id, MapScope, Measure, Meter, Note,
    NoteId, Notehead, Part, PartId, Score, ScoreTime, Spanner,
    SpannerEndpoint, SpannerKind, Staff, StaffId, StaffKind, TimedEvent, Transposition, Voice,
    VoiceId,
};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct PdfIngestOptions {
    pub display_list: DisplayListOptions,
    pub normalizer: SymbolNormalizer,
}

impl Default for PdfIngestOptions {
    fn default() -> Self {
        Self {
            display_list: DisplayListOptions::default(),
            normalizer: SymbolNormalizer::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ElementConfidence {
    pub class: Estimate<String>,
    pub pitch: Option<Estimate<makepad_score::model::Pitch>>,
    pub duration: Option<Estimate<DurationValue>>,
    pub voice: Option<Estimate<u8>>,
    pub attachments: Vec<Estimate<String>>,
}

#[derive(Clone, Debug)]
pub struct SemanticBinding {
    pub score_note: NoteId,
    pub semantic: SemanticId,
    pub page: PageIndex,
    pub bounds: Rect,
    pub primitives: Vec<PrimitiveId>,
    pub confidence: ElementConfidence,
}

#[derive(Clone, Debug)]
pub struct RecognizedPage {
    pub display: DisplayList,
    pub classification: PageClassification,
    pub geometry: PageGeometry,
    pub semantics: SemanticPage,
}

#[derive(Clone, Debug)]
pub struct RecognizedDocument {
    pub original: Arc<[u8]>,
    pub original_sha256: [u8; 32],
    pub pages: Vec<RecognizedPage>,
    pub score: Score,
    pub bindings: Vec<SemanticBinding>,
    pub edits: EditLog,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PdfIngestError {
    Parse(String),
}

impl std::fmt::Display for PdfIngestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(message) => write!(formatter, "PDF ingest failed: {message}"),
        }
    }
}

impl std::error::Error for PdfIngestError {}

pub fn ingest_pdf(
    bytes: Arc<[u8]>,
    options: &PdfIngestOptions,
) -> Result<RecognizedDocument, PdfIngestError> {
    let compatible = display::parser_compatible_bytes(&bytes);
    let parser_bytes = compatible.as_deref().unwrap_or(&bytes);
    let mut parser = PdfDocument::parse(parser_bytes)
        .map_err(|error| PdfIngestError::Parse(error.to_string()))?;
    let page_count = parser.page_count();
    let mut pages = Vec::with_capacity(page_count);
    for page_index in 0..page_count {
        let display = build_display_list(&mut parser, page_index, options.display_list)
            .map_err(|error| PdfIngestError::Parse(error.to_string()))?;
        let classification = classify_page(&display, &options.normalizer);
        let geometry = if classification.recognition_available {
            recover_page_geometry(&display)
        } else {
            PageGeometry::default()
        };
        let semantics = if classification.recognition_available {
            reconstruct_page(&display, &geometry, &options.normalizer)
        } else {
            SemanticPage::default()
        };
        pages.push(RecognizedPage {
            display,
            classification,
            geometry,
            semantics,
        });
    }
    let (score, bindings) = build_score(&pages);
    Ok(RecognizedDocument {
        original_sha256: sha256::sha256(&bytes),
        original: bytes,
        pages,
        score,
        bindings,
        edits: EditLog::default(),
    })
}

pub fn apply_plan(
    document: &mut RecognizedDocument,
    plan: SplicePlan,
) -> Result<EditId, SpliceError> {
    if document.pages.get(plan.page.0 as usize).is_none() {
        return Err(SpliceError::PageNotFound(plan.page));
    }
    Ok(provenance::record_plan(document, plan))
}

pub fn revert_pdf(document: &RecognizedDocument) -> Arc<[u8]> {
    document.original.clone()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StaffImage {
    pub width: u32,
    pub height: u32,
    pub gray: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScanHypotheses {
    pub notes: Vec<(Point, f32)>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScanRecognitionError {
    Unavailable,
    InvalidCrop,
    Model(String),
}

pub trait ScanRecognizer: Send + Sync {
    fn recognize(
        &self,
        crop: &StaffImage,
        geometry: &StaffGeometry,
    ) -> Result<ScanHypotheses, ScanRecognitionError>;
}

fn build_score(pages: &[RecognizedPage]) -> (Score, Vec<SemanticBinding>) {
    const ACTOR: u64 = 0x5343_4f52_4550_4446;
    let part_id: PartId = Id::new(ACTOR, 1);
    let mut score = Score::new(*b"makepad-scorepdf");
    score.title = "Recovered PDF score".to_string();
    let mut staff_ids = BTreeMap::new();
    let mut all_part_staves = Vec::new();
    let mut global_staff = 0_u64;
    for (page_index, page) in pages.iter().enumerate() {
        for staff in &page.geometry.staves {
            global_staff += 1;
            let staff_id: StaffId = Id::new(ACTOR, 1_000 + global_staff);
            staff_ids.insert((page_index, staff.index), staff_id);
            all_part_staves.push(staff_id);
        }
    }
    score.parts.insert(
        part_id,
        Part {
            id: part_id,
            name: "Recovered score".to_string(),
            staves: all_part_staves.clone(),
            transposition: Transposition::NONE,
        },
    );

    let mut measure_ids = BTreeMap::new();
    let mut measure_start = ScoreTime::ZERO;
    let measure_extent = Duration::new(1, 1).expect("one whole note is a valid duration");
    let mut ordinal = 0_u32;
    for (page_index, page) in pages.iter().enumerate() {
        for measure in &page.geometry.measures {
            let measure_id = Id::new(ACTOR, 200_000 + u64::from(ordinal) + 1);
            measure_ids.insert((page_index, measure.index), measure_id);
            score.measures.insert(
                measure_id,
                Measure {
                    id: measure_id,
                    ordinal,
                    label: (ordinal + 1).to_string(),
                    start: measure_start,
                    extent: measure_extent,
                },
            );
            measure_start = measure_start
                .checked_add(measure_extent)
                .unwrap_or(measure_start);
            ordinal += 1;
        }
    }
    score.maps = GlobalMaps {
        tempo: Vec::new(),
        time_signature: vec![Change {
            at: ScoreTime::ZERO,
            scope: MapScope::Global,
            value: Meter::Measured {
                groups: vec![4],
                unit: 4,
            },
        }],
        key: vec![Change {
            at: ScoreTime::ZERO,
            scope: MapScope::Global,
            value: makepad_score::model::KeySignature::C_MAJOR,
        }],
    };

    let mut voice_ids = BTreeMap::new();
    let mut voice_counter = 0_u64;
    for ((page_index, local_staff), staff_id) in &staff_ids {
        let page = &pages[*page_index];
        let mut numbers: Vec<_> = page
            .semantics
            .notes
            .iter()
            .filter(|note| note.staff == *local_staff)
            .map(|note| note.voice.value)
            .collect();
        if numbers.is_empty() {
            numbers.push(1);
        }
        numbers.sort_unstable();
        numbers.dedup();
        let mut ids = Vec::new();
        for number in numbers {
            voice_counter += 1;
            let voice_id: VoiceId = Id::new(ACTOR, 100_000 + voice_counter);
            voice_ids.insert((*page_index, *local_staff, number), voice_id);
            ids.push(voice_id);
            score.voices.insert(
                voice_id,
                Voice {
                    id: voice_id,
                    staff: *staff_id,
                    number: u16::from(number),
                    events: Vec::new(),
                },
            );
        }
        score.staves.insert(
            *staff_id,
            Staff {
                id: *staff_id,
                part: part_id,
                parent: None,
                kind: StaffKind::Standard,
                voices: ids,
            },
        );
    }

    let mut bindings = Vec::new();
    let mut score_note_ids = BTreeMap::new();
    let mut event_counter = 0_u64;
    for (page_index, page) in pages.iter().enumerate() {
        let mut chords: BTreeMap<(usize, usize, u8, u64), Vec<&RecognizedNote>> = BTreeMap::new();
        for note in &page.semantics.notes {
            chords
                .entry((note.staff, note.measure, note.voice.value, note.chord))
                .or_default()
                .push(note);
        }
        for ((staff, measure, voice_number, _), notes) in chords {
            let Some(&voice_id) = voice_ids.get(&(page_index, staff, voice_number)) else {
                continue;
            };
            let Some(&measure_id) = measure_ids.get(&(page_index, measure)) else {
                continue;
            };
            let Some(measure_model) = score.measures.get(&measure_id) else {
                continue;
            };
            let Some(measure_geometry) = page.geometry.measures.get(measure) else {
                continue;
            };
            let x = notes.iter().map(|note| note.origin.x).sum::<f64>() / notes.len() as f64;
            let normalized = ((x - measure_geometry.x_range.0)
                / (measure_geometry.x_range.1 - measure_geometry.x_range.0).max(0.001))
                .clamp(0.0, 0.9375);
            let sixteenths = (normalized * 16.0).round() as i64;
            let local_onset = ScoreTime::new(sixteenths, 16).unwrap_or(ScoreTime::ZERO);
            let onset = measure_model
                .start
                .checked_add_time(local_onset)
                .unwrap_or(measure_model.start);
            let duration_value = notes[0].duration.value;
            let duration = Duration::new(
                i64::from(duration_value.numerator),
                u64::from(duration_value.denominator),
            )
            .ok();
            event_counter += 1;
            let event_id = Id::new(ACTOR, 300_000 + event_counter);
            let staff_id = staff_ids[&(page_index, staff)];
            let mut score_notes = Vec::new();
            for note in notes {
                let note_id: NoteId = Id::new(
                    ACTOR,
                    400_000 + page_index as u64 * 1_000_000 + note.id.0,
                );
                score_note_ids.insert((page_index, note.id), note_id);
                score_notes.push(Note {
                    id: note_id,
                    written_pitch: note.pitch.as_ref().map(|pitch| pitch.value),
                    unpitched_sound: None,
                    display_staff: staff_id,
                    tie_from: None,
                    tie_to: None,
                    tab: None,
                    notehead: Notehead::Normal,
                });
                let class = page
                    .semantics
                    .symbols
                    .iter()
                    .find(|symbol| symbol.primitive == note.page_primitive)
                    .map(|symbol| symbol.symbol.clone());
                let class_confidence = class.map_or_else(
                    || {
                        Estimate::new(
                            "notehead".to_string(),
                            0.5,
                            0.0,
                            vec![Evidence::NoEvidence("symbol class".to_string())],
                            Verification::Ambiguous,
                        )
                    },
                    |class| {
                        Estimate::new(
                            class.value.canonical_name,
                            class.probability,
                            class.runner_up_margin,
                            class.evidence,
                            class.verification,
                        )
                    },
                );
                bindings.push(SemanticBinding {
                    score_note: note_id,
                    semantic: note.id,
                    page: PageIndex(page_index as u32),
                    bounds: note.bounds,
                    primitives: dependency_primitives(note).into_iter().collect(),
                    confidence: ElementConfidence {
                        class: class_confidence,
                        pitch: note.pitch.clone(),
                        duration: Some(note.duration.clone()),
                        voice: Some(note.voice.clone()),
                        attachments: note
                            .attachments
                            .curves
                            .iter()
                            .map(|primitive| {
                                Estimate::inferred(
                                    format!("curve:{}", primitive.0),
                                    0.8,
                                    vec![Evidence::AttachmentDistance(0.0)],
                                )
                            })
                            .collect(),
                    },
                });
            }
            if let Some(voice) = score.voices.get_mut(&voice_id) {
                voice.events.push(TimedEvent {
                    id: event_id,
                    onset,
                    duration,
                    grace: None,
                    kind: EventKind::Chord(score_notes),
                    beams: Vec::<BeamMembership>::new(),
                    tuplets: Vec::new(),
                    articulations: Vec::new(),
                    ornaments: Vec::new(),
                });
            }
        }
    }

    let mut spanner_counter = 0_u64;
    for (page_index, page) in pages.iter().enumerate() {
        for curve in &page.semantics.curves {
            let (Some(start), Some(end)) = (curve.start_note, curve.end_note) else {
                continue;
            };
            let (Some(&start), Some(&end)) = (
                score_note_ids.get(&(page_index, start)),
                score_note_ids.get(&(page_index, end)),
            ) else {
                continue;
            };
            spanner_counter += 1;
            let id = Id::new(ACTOR, 500_000 + spanner_counter);
            let kind = match curve.kind.value {
                CurveKind::Tie => SpannerKind::Tie,
                CurveKind::Slur => SpannerKind::Slur { placement: None },
                CurveKind::Ambiguous => SpannerKind::Other("ambiguous PDF curve".to_string()),
            };
            score.spanners.insert(
                id,
                Spanner {
                    id,
                    kind,
                    start: SpannerEndpoint::Note(start),
                    end: SpannerEndpoint::Note(end),
                },
            );
            if curve.kind.value == CurveKind::Tie {
                if let Some(note) = score.note_mut(start) {
                    note.tie_to = Some(end);
                }
                if let Some(note) = score.note_mut(end) {
                    note.tie_from = Some(start);
                }
            }
        }
    }
    for voice in score.voices.values_mut() {
        voice.events.sort_by_key(|event| (event.onset, event.id));
    }
    (score, bindings)
}

#[cfg(test)]
mod tests;
