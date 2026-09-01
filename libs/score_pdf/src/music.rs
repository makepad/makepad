//! Vector music reconstruction built from normalized symbols and geometry.

use crate::confidence::{Estimate, Evidence, Verification};
use crate::display::{DisplayList, DisplayPrimitive, PathCommand, PrimitiveId};
use crate::geometry::{median, Point, Rect};
use crate::normalize::{
    AccidentalKind, BasicDuration, NormalizedSymbol, StemDirection, SymbolClass, SymbolNormalizer,
};
use crate::recover::{line_segments, MeasureRegion, PageGeometry, StaffGeometry};
use makepad_score::model::{Alter, Pitch, Step};
use makepad_score::symbol::Clef;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DurationValue {
    pub numerator: u32,
    pub denominator: u32,
    pub dots: u8,
}

impl DurationValue {
    pub const fn new(numerator: u32, denominator: u32, dots: u8) -> Self {
        Self {
            numerator,
            denominator,
            dots,
        }
    }

    pub fn from_basic(base: BasicDuration, dots: u8) -> Self {
        let (base_numerator, base_denominator): (u32, u32) = match base {
            BasicDuration::DoubleWhole => (2, 1),
            other => (1, u32::from(other.denominator())),
        };
        let factor = 1_u32.checked_shl(u32::from(dots)).unwrap_or(u32::MAX);
        let numerator = base_numerator.saturating_mul(factor.saturating_mul(2).saturating_sub(1));
        let denominator = base_denominator.saturating_mul(factor);
        let divisor = gcd(numerator, denominator);
        Self::new(numerator / divisor, denominator / divisor, dots)
    }

    pub fn add(self, other: Self) -> Self {
        let numerator = self.numerator * other.denominator
            + other.numerator * self.denominator;
        let denominator = self.denominator * other.denominator;
        let divisor = gcd(numerator, denominator);
        Self::new(numerator / divisor, denominator / divisor, 0)
    }
}

const fn gcd(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    if left == 0 { 1 } else { left }
}

#[derive(Clone, Debug)]
pub struct ClassifiedGlyph {
    pub primitive: PrimitiveId,
    pub symbol: Estimate<NormalizedSymbol>,
    pub staff: Option<Estimate<usize>>,
    pub measure: Option<usize>,
    pub origin: Point,
    pub bounds: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClefPlacement {
    pub primitive: PrimitiveId,
    pub staff: usize,
    pub x: f64,
    pub reference_staff_step: i16,
    pub clef: Estimate<Clef>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScopeNote {
    pub measure: usize,
    pub x_milli: i64,
    pub staff_step: i16,
    pub diatonic_absolute: i16,
    pub alter: i8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScopeAccidental {
    pub primitive: PrimitiveId,
    pub measure: usize,
    pub x_milli: i64,
    pub staff_step: i16,
    pub kind: AccidentalKind,
}

#[derive(Clone, Debug)]
pub struct NoteAttachments {
    pub stem: Option<PrimitiveId>,
    pub beams: Vec<PrimitiveId>,
    pub flag: Option<PrimitiveId>,
    pub dots: Vec<PrimitiveId>,
    pub accidental: Option<PrimitiveId>,
    pub ledgers: Vec<PrimitiveId>,
    pub curves: Vec<PrimitiveId>,
}

#[derive(Clone, Debug)]
pub struct RecognizedNote {
    pub id: SemanticId,
    pub page_primitive: PrimitiveId,
    pub staff: usize,
    pub measure: usize,
    pub chord: u64,
    pub origin: Point,
    pub bounds: Rect,
    pub staff_step: i16,
    pub pitch: Option<Estimate<Pitch>>,
    pub duration: Estimate<DurationValue>,
    pub sounding_duration: DurationValue,
    pub voice: Estimate<u8>,
    pub attachments: NoteAttachments,
    pub tie_from: Option<SemanticId>,
    pub tie_to: Option<SemanticId>,
}

#[derive(Clone, Debug)]
pub struct RecognizedRest {
    pub id: SemanticId,
    pub primitive: PrimitiveId,
    pub staff: usize,
    pub measure: usize,
    pub origin: Point,
    pub duration: Estimate<DurationValue>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CurveKind {
    Tie,
    Slur,
    Ambiguous,
}

#[derive(Clone, Debug)]
pub struct CurveRelation {
    pub primitive: PrimitiveId,
    pub primitives: Vec<PrimitiveId>,
    pub start_note: Option<SemanticId>,
    pub end_note: Option<SemanticId>,
    pub kind: Estimate<CurveKind>,
    pub bounds: Rect,
}

#[derive(Clone, Debug, Default)]
pub struct SemanticPage {
    pub symbols: Vec<ClassifiedGlyph>,
    pub clefs: Vec<ClefPlacement>,
    pub notes: Vec<RecognizedNote>,
    pub rests: Vec<RecognizedRest>,
    pub curves: Vec<CurveRelation>,
    pub unclassified_glyphs: usize,
}

pub fn reconstruct_page(
    display: &DisplayList,
    geometry: &PageGeometry,
    normalizer: &SymbolNormalizer,
) -> SemanticPage {
    let mut output = SemanticPage::default();
    for (primitive, value) in &display.primitives {
        let DisplayPrimitive::Glyph(glyph) = value else {
            continue;
        };
        let nearest = nearest_staff(glyph.origin, &geometry.staves);
        let staff_space = nearest.map(|(staff, _)| geometry.staves[staff].staff_space);
        let Some(symbol) = normalizer.normalize(glyph, staff_space) else {
            output.unclassified_glyphs += 1;
            continue;
        };
        let staff = nearest.map(|(staff, residual)| {
            Estimate::new(
                staff,
                (1.0 - residual as f32).clamp(0.5, 0.999),
                0.5,
                vec![Evidence::StaffResidual(residual as f32)],
                Verification::Inferred,
            )
        });
        let measure = staff
            .as_ref()
            .and_then(|staff| measure_for(geometry, staff.value, glyph.origin.x));
        output.symbols.push(ClassifiedGlyph {
            primitive: *primitive,
            symbol,
            staff,
            measure,
            origin: glyph.origin,
            bounds: glyph.bounds,
        });
    }
    output.clefs = recover_clefs(&output.symbols, &geometry.staves);
    output.notes = recover_notes(display, geometry, &output.symbols, &output.clefs);
    output.rests = recover_rests(&output.symbols);
    output.curves = recover_curves(display, geometry, &mut output.notes);
    apply_tie_durations(&mut output.notes, &output.curves);
    output
}

fn nearest_staff(point: Point, staves: &[StaffGeometry]) -> Option<(usize, f64)> {
    staves
        .iter()
        .filter(|staff| staff.accepts(point))
        .map(|staff| {
            let (_, residual) = staff.staff_step(point.y);
            let outside = if point.y < staff.bottom_y() {
                (staff.bottom_y() - point.y) / (staff.staff_space * 8.0)
            } else if point.y > staff.top_y() {
                (point.y - staff.top_y()) / (staff.staff_space * 8.0)
            } else {
                0.0
            };
            (staff.index, residual + outside)
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
}

fn measure_for(geometry: &PageGeometry, staff: usize, x: f64) -> Option<usize> {
    geometry
        .measures
        .iter()
        .find(|measure| {
            geometry.systems[measure.system].staves.contains(&staff)
                && x >= measure.x_range.0 - 0.5
                && x <= measure.x_range.1 + 0.5
        })
        .map(|measure| measure.index)
}

fn recover_clefs(
    symbols: &[ClassifiedGlyph],
    staves: &[StaffGeometry],
) -> Vec<ClefPlacement> {
    let mut output = Vec::new();
    for symbol in symbols {
        let SymbolClass::Clef(clef) = symbol.symbol.value.class else {
            continue;
        };
        let Some(staff) = symbol.staff.as_ref().map(|staff| staff.value) else {
            continue;
        };
        let (reference_staff_step, residual) = staves[staff].staff_step(symbol.origin.y);
        let mut evidence = symbol.symbol.evidence.clone();
        evidence.push(Evidence::StaffResidual(residual as f32));
        output.push(ClefPlacement {
            primitive: symbol.primitive,
            staff,
            x: symbol.origin.x,
            reference_staff_step,
            clef: Estimate::new(
                clef,
                (symbol.symbol.probability * (1.0 - residual as f32 * 0.25)).clamp(0.0, 1.0),
                symbol.symbol.runner_up_margin,
                evidence,
                symbol.symbol.verification,
            ),
        });
    }
    output.sort_by(|left, right| left.staff.cmp(&right.staff).then(left.x.total_cmp(&right.x)));
    output
}

pub fn clef_in_force(clefs: &[ClefPlacement], staff: usize, x: f64) -> Option<&ClefPlacement> {
    clefs
        .iter()
        .filter(|clef| clef.staff == staff && clef.x <= x + 0.001)
        .max_by(|left, right| left.x.total_cmp(&right.x))
}

#[derive(Clone)]
struct NoteSeed {
    primitive: PrimitiveId,
    staff: usize,
    measure: usize,
    origin: Point,
    bounds: Rect,
    base: BasicDuration,
    staff_step: i16,
    diatonic_absolute: Option<i16>,
    pitch_confidence: Option<(f32, Vec<Evidence>)>,
}

fn recover_notes(
    display: &DisplayList,
    geometry: &PageGeometry,
    symbols: &[ClassifiedGlyph],
    clefs: &[ClefPlacement],
) -> Vec<RecognizedNote> {
    let mut seeds = Vec::new();
    for symbol in symbols {
        let SymbolClass::Notehead(base) = symbol.symbol.value.class else {
            continue;
        };
        let (Some(staff), Some(measure)) = (
            symbol.staff.as_ref().map(|staff| staff.value),
            symbol.measure,
        ) else {
            continue;
        };
        let (staff_step, residual) = geometry.staves[staff].staff_step(symbol.origin.y);
        let (diatonic_absolute, pitch_confidence) = match clef_in_force(clefs, staff, symbol.origin.x)
        {
            Some(clef) => {
                let reference = clef_reference_absolute(clef.clef.value);
                let absolute = reference + staff_step - clef.reference_staff_step;
                (
                    Some(absolute),
                    Some((
                        (clef.clef.probability * (1.0 - residual as f32 * 0.5)).clamp(0.0, 1.0),
                        vec![
                            Evidence::ClefInForce {
                                primitive: clef.primitive.0,
                            },
                            Evidence::StaffResidual(residual as f32),
                        ],
                    )),
                )
            }
            None => (None, None),
        };
        seeds.push(NoteSeed {
            primitive: symbol.primitive,
            staff,
            measure,
            origin: symbol.origin,
            bounds: symbol.bounds,
            base,
            staff_step,
            diatonic_absolute,
            pitch_confidence,
        });
    }
    seeds.sort_by(|left, right| {
        left.staff
            .cmp(&right.staff)
            .then(left.measure.cmp(&right.measure))
            .then(left.origin.x.total_cmp(&right.origin.x))
            .then(left.origin.y.total_cmp(&right.origin.y))
    });

    let accidentals = explicit_accidentals(symbols, &seeds, geometry);
    let key_by_staff = infer_keys(symbols, &seeds, geometry);
    let mut scope_notes: Vec<_> = seeds
        .iter()
        .map(|seed| ScopeNote {
            measure: seed.measure,
            x_milli: (seed.origin.x * 1000.0).round() as i64,
            staff_step: seed.staff_step,
            diatonic_absolute: seed.diatonic_absolute.unwrap_or(0),
            alter: 0,
        })
        .collect();
    for staff in 0..geometry.staves.len() {
        let indices: Vec<_> = seeds
            .iter()
            .enumerate()
            .filter_map(|(index, seed)| (seed.staff == staff).then_some(index))
            .collect();
        let mut staff_notes: Vec<_> = indices.iter().map(|index| scope_notes[*index]).collect();
        let staff_accidentals: Vec<_> = accidentals
            .iter()
            .filter(|accidental| accidental.0 == staff)
            .map(|accidental| accidental.1)
            .collect();
        let key = key_by_staff.get(&staff).copied().unwrap_or([0; 7]);
        apply_accidental_scope(&mut staff_notes, &staff_accidentals, key);
        for (index, note) in indices.into_iter().zip(staff_notes) {
            scope_notes[index] = note;
        }
    }

    let segments = line_segments(display);
    let verticals: Vec<_> = segments
        .iter()
        .copied()
        .filter(|segment| {
            let dx = (segment.end.x - segment.start.x).abs();
            let dy = (segment.end.y - segment.start.y).abs();
            dy > 0.0 && dx <= dy * 0.015 + segment.width
        })
        .collect();
    let mut notes = Vec::new();
    let mut chord_counter = 0_u64;
    let mut previous_cluster: Option<(usize, usize, f64, u64)> = None;
    for (index, seed) in seeds.iter().enumerate() {
        let space = geometry.staves[seed.staff].staff_space;
        let chord = match previous_cluster {
            Some((staff, measure, x, chord))
                if staff == seed.staff
                    && measure == seed.measure
                    && (x - seed.origin.x).abs() <= space * 0.28 =>
            {
                chord
            }
            _ => {
                chord_counter += 1;
                chord_counter
            }
        };
        previous_cluster = Some((seed.staff, seed.measure, seed.origin.x, chord));
        let stem = attach_stem(seed.origin, space, &verticals);
        let stem_direction = stem.map(|segment| {
            let upper = segment.start.y.max(segment.end.y) - seed.origin.y;
            let lower = seed.origin.y - segment.start.y.min(segment.end.y);
            if upper >= lower {
                StemDirection::Up
            } else {
                StemDirection::Down
            }
        });
        let beams: Vec<_> = stem
            .iter()
            .flat_map(|stem| {
                geometry.beams.iter().filter_map(move |beam| {
                    let x = (stem.start.x + stem.end.x) * 0.5;
                    (x >= beam.bounds.min_x - stem.width
                        && x <= beam.bounds.max_x + stem.width
                        && (beam.center_y - stem.start.y).abs().min(
                            (beam.center_y - stem.end.y).abs(),
                        ) <= space * 1.3)
                        .then_some(beam.primitive)
                })
            })
            .collect();
        let flag = nearest_flag(symbols, seed, stem_direction, space);
        let dots = nearby_dots(symbols, seed, space);
        let levels = (beams.len() as u8).max(flag.as_ref().map_or(0, |(_, levels)| *levels));
        let duration_base = match seed.base {
            BasicDuration::Quarter if levels > 0 => {
                BasicDuration::from_flag_or_beam_levels(levels)
            }
            value => value,
        };
        let duration = DurationValue::from_basic(duration_base, dots.len() as u8);
        let mut duration_evidence = Vec::new();
        if !beams.is_empty() {
            duration_evidence.push(Evidence::BeamLevels(beams.len() as u8));
        }
        if let Some((_, levels)) = flag {
            duration_evidence.push(Evidence::FlagLevels(levels));
        }
        if !dots.is_empty() {
            duration_evidence.push(Evidence::DotCount(dots.len() as u8));
        }
        if duration_evidence.is_empty() {
            duration_evidence.push(Evidence::StructuralName(format!("{:?}", seed.base)));
        }
        let accidental = accidentals.iter().find_map(|(staff, accidental)| {
            (*staff == seed.staff
                && accidental.measure == seed.measure
                && accidental.staff_step == seed.staff_step
                && accidental.x_milli <= (seed.origin.x * 1000.0) as i64
                && ((seed.origin.x * 1000.0) as i64 - accidental.x_milli)
                    <= (space * 3000.0) as i64)
                .then_some(accidental.primitive)
        });
        let pitch = seed.diatonic_absolute.zip(seed.pitch_confidence.as_ref()).map(
            |(absolute, (confidence, evidence))| {
                let scoped = scope_notes[index];
                let mut evidence = evidence.clone();
                if let Some(primitive) = accidental {
                    evidence.push(Evidence::MeasureAccidental {
                        primitive: primitive.0,
                    });
                } else if scoped.alter != 0 {
                    evidence.push(Evidence::KeySignature(scoped.alter));
                }
                Estimate::new(
                    pitch_from_diatonic(absolute, scoped.alter),
                    *confidence,
                    (*confidence - 0.5).max(0.0),
                    evidence,
                    Verification::Inferred,
                )
            },
        );
        let voice_value = match stem_direction {
            Some(StemDirection::Down) => 2,
            _ => 1,
        };
        let voice_probability = if stem_direction.is_some() { 0.82 } else { 0.55 };
        notes.push(RecognizedNote {
            id: SemanticId(index as u64 + 1),
            page_primitive: seed.primitive,
            staff: seed.staff,
            measure: seed.measure,
            chord,
            origin: seed.origin,
            bounds: seed.bounds,
            staff_step: seed.staff_step,
            pitch,
            duration: Estimate::new(
                duration,
                if levels > 0 { 0.94 } else { 0.84 },
                0.55,
                duration_evidence,
                Verification::Inferred,
            ),
            sounding_duration: duration,
            voice: Estimate::new(
                voice_value,
                voice_probability,
                (voice_probability - 0.5).max(0.0),
                stem_direction
                    .map(|direction| {
                        vec![Evidence::StemDirection(match direction {
                            StemDirection::Up => 1,
                            StemDirection::Down => -1,
                        })]
                    })
                    .unwrap_or_else(|| vec![Evidence::NoEvidence("voice".to_string())]),
                Verification::Inferred,
            ),
            attachments: NoteAttachments {
                stem: stem.map(|segment| segment.primitive),
                beams,
                flag: flag.map(|(primitive, _)| primitive),
                dots,
                accidental,
                ledgers: ledger_lines(seed, &segments, space),
                curves: Vec::new(),
            },
            tie_from: None,
            tie_to: None,
        });
    }
    notes
}

fn clef_reference_absolute(clef: Clef) -> i16 {
    match clef {
        Clef::G | Clef::G8va | Clef::G8vb | Clef::G15ma | Clef::G15mb => 32, // G4
        Clef::F | Clef::F8va | Clef::F8vb | Clef::F15ma | Clef::F15mb => 24, // F3
        Clef::C => 28, // C4
        _ => 28,
    }
}

fn pitch_from_diatonic(absolute: i16, alter: i8) -> Pitch {
    let step = match absolute.rem_euclid(7) {
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
        Alter::new(i64::from(alter), 1).expect("integer alteration is valid"),
        absolute.div_euclid(7) as i8,
    )
}

fn explicit_accidentals(
    symbols: &[ClassifiedGlyph],
    seeds: &[NoteSeed],
    geometry: &PageGeometry,
) -> Vec<(usize, ScopeAccidental)> {
    let mut output = Vec::new();
    for symbol in symbols {
        let SymbolClass::Accidental(kind) = symbol.symbol.value.class else {
            continue;
        };
        let (Some(staff), Some(measure)) = (
            symbol.staff.as_ref().map(|staff| staff.value),
            symbol.measure,
        ) else {
            continue;
        };
        let space = geometry.staves[staff].staff_space;
        let Some(note) = seeds
            .iter()
            .filter(|note| note.staff == staff && note.measure == measure)
            .filter(|note| note.origin.x >= symbol.origin.x)
            .filter(|note| note.origin.x - symbol.origin.x <= space * 3.0)
            .filter(|note| (note.origin.y - symbol.origin.y).abs() <= space * 0.7)
            .min_by(|left, right| left.origin.x.total_cmp(&right.origin.x))
        else {
            continue;
        };
        output.push((
            staff,
            ScopeAccidental {
                primitive: symbol.primitive,
                measure,
                x_milli: (symbol.origin.x * 1000.0).round() as i64,
                staff_step: note.staff_step,
                kind,
            },
        ));
    }
    output
}

fn infer_keys(
    symbols: &[ClassifiedGlyph],
    seeds: &[NoteSeed],
    geometry: &PageGeometry,
) -> BTreeMap<usize, [i8; 7]> {
    let mut output = BTreeMap::new();
    for staff in &geometry.staves {
        let first_note_x = seeds
            .iter()
            .filter(|note| note.staff == staff.index)
            .map(|note| note.origin.x)
            .fold(f64::INFINITY, f64::min);
        let kinds: Vec<_> = symbols
            .iter()
            .filter(|symbol| symbol.staff.as_ref().map(|value| value.value) == Some(staff.index))
            .filter(|symbol| symbol.origin.x < first_note_x)
            .filter_map(|symbol| match symbol.symbol.value.class {
                SymbolClass::Accidental(kind) => Some(kind),
                _ => None,
            })
            .collect();
        let sharps = kinds
            .iter()
            .filter(|kind| **kind == AccidentalKind::Sharp)
            .count()
            .min(7);
        let flats = kinds
            .iter()
            .filter(|kind| **kind == AccidentalKind::Flat)
            .count()
            .min(7);
        let fifths = if sharps > 0 && flats == 0 {
            sharps as i8
        } else if flats > 0 && sharps == 0 {
            -(flats as i8)
        } else {
            0
        };
        output.insert(staff.index, key_alterations(fifths));
    }
    output
}

fn key_alterations(fifths: i8) -> [i8; 7] {
    let mut output = [0; 7];
    let sharp_order = [Step::F, Step::C, Step::G, Step::D, Step::A, Step::E, Step::B];
    let flat_order = [Step::B, Step::E, Step::A, Step::D, Step::G, Step::C, Step::F];
    let (order, value, count) = if fifths >= 0 {
        (&sharp_order, 1, fifths as usize)
    } else {
        (&flat_order, -1, fifths.unsigned_abs() as usize)
    };
    for step in order.iter().take(count.min(7)) {
        output[step.index() as usize] = value;
    }
    output
}

/// Applies key defaults and explicit accidentals after measure segmentation.
/// State resets at each measure boundary and is keyed by staff position.
pub fn apply_accidental_scope(
    notes: &mut [ScopeNote],
    accidentals: &[ScopeAccidental],
    key: [i8; 7],
) {
    notes.sort_by_key(|note| (note.measure, note.x_milli, note.staff_step));
    let mut current_measure = None;
    let mut active = BTreeMap::new();
    for note in notes {
        if current_measure != Some(note.measure) {
            active.clear();
            current_measure = Some(note.measure);
        }
        for accidental in accidentals.iter().filter(|accidental| {
            accidental.measure == note.measure
                && accidental.x_milli <= note.x_milli
                && accidental.staff_step == note.staff_step
        }) {
            active.insert(note.staff_step, accidental.kind.semitones());
        }
        note.alter = active
            .get(&note.staff_step)
            .copied()
            .unwrap_or(key[note.diatonic_absolute.rem_euclid(7) as usize]);
    }
}

fn attach_stem(
    origin: Point,
    staff_space: f64,
    verticals: &[crate::recover::LineSegment],
) -> Option<crate::recover::LineSegment> {
    verticals
        .iter()
        .copied()
        .filter(|segment| {
            let x = (segment.start.x + segment.end.x) * 0.5;
            let bottom = segment.start.y.min(segment.end.y);
            let top = segment.start.y.max(segment.end.y);
            (x - origin.x).abs() <= staff_space * 0.9
                && origin.y >= bottom - staff_space * 0.4
                && origin.y <= top + staff_space * 0.4
                && segment.length() >= staff_space * 2.0
                && segment.length() <= staff_space * 8.0
        })
        .min_by(|left, right| {
            let left_x = ((left.start.x + left.end.x) * 0.5 - origin.x).abs();
            let right_x = ((right.start.x + right.end.x) * 0.5 - origin.x).abs();
            left_x.total_cmp(&right_x)
        })
}

fn nearest_flag(
    symbols: &[ClassifiedGlyph],
    seed: &NoteSeed,
    direction: Option<StemDirection>,
    space: f64,
) -> Option<(PrimitiveId, u8)> {
    symbols
        .iter()
        .filter_map(|symbol| match symbol.symbol.value.class {
            SymbolClass::Flag {
                levels,
                direction: flag_direction,
            } if direction.is_none() || direction == Some(flag_direction) => {
                Some((symbol, levels))
            }
            _ => None,
        })
        .filter(|(symbol, _)| {
            symbol.staff.as_ref().map(|staff| staff.value) == Some(seed.staff)
                && (symbol.origin.x - seed.origin.x).abs() <= space * 2.0
                && (symbol.origin.y - seed.origin.y).abs() <= space * 6.0
        })
        .min_by(|(left, _), (right, _)| {
            left.origin
                .distance(seed.origin)
                .total_cmp(&right.origin.distance(seed.origin))
        })
        .map(|(symbol, levels)| (symbol.primitive, levels))
}

fn nearby_dots(symbols: &[ClassifiedGlyph], seed: &NoteSeed, space: f64) -> Vec<PrimitiveId> {
    symbols
        .iter()
        .filter(|symbol| symbol.symbol.value.class == SymbolClass::AugmentationDot)
        .filter(|symbol| {
            symbol.staff.as_ref().map(|staff| staff.value) == Some(seed.staff)
                && symbol.origin.x > seed.origin.x
                && symbol.origin.x - seed.origin.x <= space * 2.5
                && (symbol.origin.y - seed.origin.y).abs() <= space * 0.65
        })
        .map(|symbol| symbol.primitive)
        .collect()
}

fn ledger_lines(
    seed: &NoteSeed,
    segments: &[crate::recover::LineSegment],
    space: f64,
) -> Vec<PrimitiveId> {
    if (0..=8).contains(&seed.staff_step) {
        return Vec::new();
    }
    segments
        .iter()
        .filter(|segment| {
            let dx = (segment.end.x - segment.start.x).abs();
            let dy = (segment.end.y - segment.start.y).abs();
            dx >= space * 0.8
                && dx <= space * 3.0
                && dy <= segment.width.max(0.05)
                && segment.bounds().contains(seed.origin)
        })
        .map(|segment| segment.primitive)
        .collect()
}

fn recover_rests(symbols: &[ClassifiedGlyph]) -> Vec<RecognizedRest> {
    symbols
        .iter()
        .filter_map(|symbol| {
            let SymbolClass::Rest(base) = symbol.symbol.value.class else {
                return None;
            };
            Some(RecognizedRest {
                id: SemanticId(symbol.primitive.0 | (1_u64 << 63)),
                primitive: symbol.primitive,
                staff: symbol.staff.as_ref()?.value,
                measure: symbol.measure?,
                origin: symbol.origin,
                duration: Estimate::new(
                    DurationValue::from_basic(base, 0),
                    symbol.symbol.probability * 0.96,
                    symbol.symbol.runner_up_margin,
                    symbol.symbol.evidence.clone(),
                    symbol.symbol.verification,
                ),
            })
        })
        .collect()
}

fn recover_curves(
    display: &DisplayList,
    geometry: &PageGeometry,
    notes: &mut [RecognizedNote],
) -> Vec<CurveRelation> {
    let mut output: Vec<CurveRelation> = Vec::new();
    let mut seen_paints = BTreeSet::new();
    let staff_space = geometry.style.staff_space.max(1.0);
    for (primitive, value) in &display.primitives {
        let DisplayPrimitive::Path(path) = value else {
            continue;
        };
        if !path
            .commands
            .iter()
            .any(|command| matches!(command, PathCommand::Cubic(..)))
            || path.bounds.width() < staff_space * 1.2
            || path.bounds.width() > staff_space * 18.0
            || path.bounds.height() > staff_space * 5.0
        {
            continue;
        }
        let paint_key = (
            path.paint_source.object.num,
            path.paint_source.stream_index,
            path.paint_source.operator_index,
        );
        if !seen_paints.insert(paint_key) {
            continue;
        }
        let curve_primitives: Vec<_> = display
            .primitives
            .iter()
            .filter_map(|(candidate_id, candidate)| {
                let DisplayPrimitive::Path(candidate) = candidate else {
                    return None;
                };
                ((candidate.paint_source.object.num,
                    candidate.paint_source.stream_index,
                    candidate.paint_source.operator_index)
                    == paint_key)
                    .then_some(*candidate_id)
            })
            .collect();
        let (left, right) = curve_endpoints(path);
        let start = nearest_curve_note(left, notes, staff_space);
        let end = nearest_curve_note(right, notes, staff_space)
            .filter(|index| Some(*index) != start);
        let kind = classify_curve_relation(
            start.and_then(|index| notes[index].pitch.as_ref().map(|pitch| pitch.value)),
            end.and_then(|index| notes[index].pitch.as_ref().map(|pitch| pitch.value)),
            path.bounds.width() / staff_space,
            path.bounds.height() / staff_space,
        );
        if start.is_none() && end.is_none() {
            continue;
        }
        let start_id = start.map(|index| notes[index].id);
        let end_id = end.map(|index| notes[index].id);
        if let Some(existing) = output.iter_mut().find(|existing| {
            existing.start_note == start_id
                && existing.end_note == end_id
                && existing.kind.value == kind.value
                && rect_nearly_equal(existing.bounds, path.bounds)
        }) {
            existing.primitives.extend(curve_primitives.iter().copied());
            if let Some(index) = start {
                notes[index]
                    .attachments
                    .curves
                    .extend(curve_primitives.iter().copied());
            }
            if let Some(index) = end {
                notes[index]
                    .attachments
                    .curves
                    .extend(curve_primitives.iter().copied());
            }
            continue;
        }
        if let Some(index) = start {
            notes[index]
                .attachments
                .curves
                .extend(curve_primitives.iter().copied());
        }
        if let Some(index) = end {
            notes[index]
                .attachments
                .curves
                .extend(curve_primitives.iter().copied());
        }
        if kind.value == CurveKind::Tie {
            if let (Some(start), Some(end)) = (start, end) {
                notes[start].tie_to = Some(notes[end].id);
                notes[end].tie_from = Some(notes[start].id);
            }
        }
        output.push(CurveRelation {
            primitive: *primitive,
            primitives: curve_primitives,
            start_note: start_id,
            end_note: end_id,
            kind,
            bounds: path.bounds,
        });
    }
    output
}

fn rect_nearly_equal(left: Rect, right: Rect) -> bool {
    (left.min_x - right.min_x).abs() < 0.001
        && (left.min_y - right.min_y).abs() < 0.001
        && (left.max_x - right.max_x).abs() < 0.001
        && (left.max_y - right.max_y).abs() < 0.001
}

fn curve_endpoints(path: &crate::display::PdfPath) -> (Point, Point) {
    let points = path.points();
    let left_x = points
        .iter()
        .map(|point| point.x)
        .fold(f64::INFINITY, f64::min);
    let right_x = points
        .iter()
        .map(|point| point.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let tolerance = path.bounds.width() * 0.08 + 0.01;
    let mut left_y: Vec<_> = points
        .iter()
        .filter(|point| point.x <= left_x + tolerance)
        .map(|point| point.y)
        .collect();
    let mut right_y: Vec<_> = points
        .iter()
        .filter(|point| point.x >= right_x - tolerance)
        .map(|point| point.y)
        .collect();
    (
        Point::new(left_x, median(&mut left_y).unwrap_or(path.bounds.center().y)),
        Point::new(right_x, median(&mut right_y).unwrap_or(path.bounds.center().y)),
    )
}

fn nearest_curve_note(point: Point, notes: &[RecognizedNote], space: f64) -> Option<usize> {
    notes
        .iter()
        .enumerate()
        .filter(|(_, note)| (note.origin.x - point.x).abs() <= space * 4.0)
        .filter(|(_, note)| (note.origin.y - point.y).abs() <= space * 2.6)
        .min_by(|(_, left), (_, right)| {
            left.origin
                .distance(point)
                .total_cmp(&right.origin.distance(point))
        })
        .map(|(index, _)| index)
}

pub fn classify_curve_relation(
    start: Option<Pitch>,
    end: Option<Pitch>,
    span_staff_spaces: f64,
    height_staff_spaces: f64,
) -> Estimate<CurveKind> {
    match (start, end) {
        (Some(start), Some(end)) if start == end && span_staff_spaces <= 8.0 => Estimate::new(
            CurveKind::Tie,
            0.96,
            0.76,
            vec![
                Evidence::SamePitchEndpoints,
                Evidence::AttachmentDistance(height_staff_spaces as f32),
            ],
            Verification::Inferred,
        ),
        (Some(_), Some(_)) => Estimate::new(
            CurveKind::Slur,
            0.93,
            0.68,
            vec![
                Evidence::DifferentPitchEndpoints,
                Evidence::AttachmentDistance(height_staff_spaces as f32),
            ],
            Verification::Inferred,
        ),
        _ => Estimate::new(
            CurveKind::Ambiguous,
            0.0,
            0.0,
            vec![Evidence::NoEvidence("both curve endpoints".to_string())],
            Verification::Ambiguous,
        ),
    }
}

fn apply_tie_durations(notes: &mut [RecognizedNote], curves: &[CurveRelation]) {
    let by_id: BTreeMap<_, _> = notes
        .iter()
        .enumerate()
        .map(|(index, note)| (note.id, index))
        .collect();
    let tied_to: BTreeMap<_, _> = curves
        .iter()
        .filter(|curve| curve.kind.value == CurveKind::Tie)
        .filter_map(|curve| curve.start_note.zip(curve.end_note))
        .collect();
    let durations = notes
        .iter()
        .map(|note| note.duration.value)
        .collect::<Vec<_>>();
    let ids = notes.iter().map(|note| note.id).collect::<Vec<_>>();
    for (start_index, start_id) in ids.into_iter().enumerate() {
        let mut total = durations[start_index];
        let mut current = start_id;
        let mut visited = BTreeSet::from([current]);
        while let Some(next) = tied_to.get(&current).copied() {
            if !visited.insert(next) {
                break;
            }
            let Some(&next_index) = by_id.get(&next) else {
                break;
            };
            total = total.add(durations[next_index]);
            current = next;
        }
        notes[start_index].sounding_duration = total;
    }
}

pub fn dependency_primitives(note: &RecognizedNote) -> BTreeSet<PrimitiveId> {
    let mut output = BTreeSet::from([note.page_primitive]);
    output.extend(note.attachments.stem);
    output.extend(note.attachments.beams.iter().copied());
    output.extend(note.attachments.flag);
    output.extend(note.attachments.dots.iter().copied());
    output.extend(note.attachments.accidental);
    output.extend(note.attachments.ledgers.iter().copied());
    output.extend(note.attachments.curves.iter().copied());
    output
}

pub fn measure_region(geometry: &PageGeometry, index: usize) -> Option<&MeasureRegion> {
    geometry.measures.get(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pitch(step: Step, octave: i8) -> Pitch {
        Pitch::new(step, Alter::NATURAL, octave)
    }

    #[test]
    fn curve_pitch_identity_distinguishes_tie_and_slur() {
        assert_eq!(
            classify_curve_relation(Some(pitch(Step::C, 4)), Some(pitch(Step::C, 4)), 3.0, 0.5)
                .value,
            CurveKind::Tie
        );
        assert_eq!(
            classify_curve_relation(Some(pitch(Step::C, 4)), Some(pitch(Step::D, 4)), 3.0, 0.5)
                .value,
            CurveKind::Slur
        );
    }
}
