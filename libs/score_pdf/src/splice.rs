//! Minimal dependency-closed score splice planning.

use crate::display::{
    DisplayPrimitive, PageIndex, PathCommand, PathPaint, PrimitiveId, RetainedOperator, SourceSpan,
};
use crate::geometry::{Point, Rect};
use crate::music::{dependency_primitives, DurationValue, RecognizedNote, SemanticId};
use crate::recover::{PdfPageKind, StyleProfile};
use crate::RecognizedDocument;
use makepad_score::model::Pitch;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ReflowScope {
    Glyph,
    Beat,
    BeamGroup,
    Measure,
    System,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LicenseEvidence {
    pub spdx: String,
    pub font_name: String,
    pub source: String,
    pub embedding_editable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FontRestriction {
    UnknownLicense,
    RestrictedEmbedding,
    PreviewPrintOnly,
    MissingGlyph,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FontUseDecision {
    OriginalVerified {
        font_resource: String,
        license: LicenseEvidence,
    },
    OriginalViewOnly {
        font_resource: String,
        reason: FontRestriction,
    },
    FallbackOfl {
        family: String,
        license: String,
        visual_delta: f32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct OperatorEdit {
    pub source: SourceSpan,
    pub replacement: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PaintCommand {
    StaffLine {
        start: Point,
        end: Point,
        thickness: f64,
    },
    Notehead {
        center: Point,
        width: f64,
        height: f64,
        filled: bool,
    },
    Stem {
        start: Point,
        end: Point,
        thickness: f64,
    },
    Beam {
        corners: [Point; 4],
    },
    Dot {
        center: Point,
        diameter: f64,
    },
    AccidentalText {
        origin: Point,
        name: String,
    },
    Path {
        commands: Vec<PathCommand>,
        paint: PathPaint,
        line_width: f64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ErasePlan {
    OperatorRewrite {
        edits: Vec<OperatorEdit>,
        replacement: Vec<PaintCommand>,
    },
    ClippedOriginalForm {
        replacement: Vec<PaintCommand>,
    },
    RasterPatchUnavailable,
    OverlayOnly {
        replacement: Vec<PaintCommand>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpliceWarning {
    FontSubstituted,
    OriginalFontLicenseUnknown,
    SpacingInfeasible,
    SystemReflow,
    ScanRecognitionUnavailable,
    OverlayDoesNotEraseOriginal,
    CurveEndpointMoved,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NoteEdit {
    pub page: PageIndex,
    pub note: SemanticId,
    pub pitch: Option<Pitch>,
    pub duration: Option<DurationValue>,
}

#[derive(Clone, Debug)]
pub struct SpliceOptions {
    pub prefer_operator_rewrite: bool,
    pub max_spacing_distortion: f64,
    /// This is explicit authorization for a system-level visual diff.
    pub approve_system_reflow: bool,
    pub verified_original_font: Option<LicenseEvidence>,
}

impl Default for SpliceOptions {
    fn default() -> Self {
        Self {
            prefer_operator_rewrite: true,
            max_spacing_distortion: 0.02,
            approve_system_reflow: false,
            verified_original_font: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplicePlan {
    pub page: PageIndex,
    pub note: SemanticId,
    pub scope: ReflowScope,
    pub patch_bounds: Rect,
    pub erase: ErasePlan,
    pub style: StyleProfile,
    pub font: FontUseDecision,
    pub affected_notes: Vec<SemanticId>,
    pub affected_primitives: Vec<PrimitiveId>,
    pub onset_shifts: Vec<(SemanticId, f64)>,
    pub before_pitch: Option<Pitch>,
    pub after_pitch: Option<Pitch>,
    pub before_duration: DurationValue,
    pub after_duration: DurationValue,
    pub warnings: Vec<SpliceWarning>,
    pub requires_explicit_approval: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpliceError {
    PageNotFound(PageIndex),
    NoteNotFound(SemanticId),
    NoRequestedChange,
    InvalidGeometry,
}

impl std::fmt::Display for SpliceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PageNotFound(page) => write!(formatter, "page {} was not found", page.0),
            Self::NoteNotFound(note) => write!(formatter, "note {} was not found", note.0),
            Self::NoRequestedChange => formatter.write_str("edit does not change pitch or duration"),
            Self::InvalidGeometry => formatter.write_str("splice geometry is invalid"),
        }
    }
}

impl std::error::Error for SpliceError {}

pub fn plan_edit(
    document: &RecognizedDocument,
    edit: &NoteEdit,
    options: &SpliceOptions,
) -> Result<SplicePlan, SpliceError> {
    let page = document
        .pages
        .get(edit.page.0 as usize)
        .ok_or(SpliceError::PageNotFound(edit.page))?;
    let target = page
        .semantics
        .notes
        .iter()
        .find(|note| note.id == edit.note)
        .ok_or(SpliceError::NoteNotFound(edit.note))?;
    let pitch_changed = edit.pitch.is_some_and(|pitch| target.pitch.as_ref().map(|p| p.value) != Some(pitch));
    let duration_changed = edit
        .duration
        .is_some_and(|duration| duration != target.duration.value);
    if !pitch_changed && !duration_changed {
        return Err(SpliceError::NoRequestedChange);
    }

    let mut scope = ReflowScope::Glyph;
    let mut affected_notes = BTreeSet::from([target.id]);
    let mut affected_primitives = dependency_primitives(target);
    let mut warnings = Vec::new();
    if !target.attachments.curves.is_empty() && pitch_changed {
        warnings.push(SpliceWarning::CurveEndpointMoved);
    }
    if duration_changed {
        if target.attachments.beams.is_empty() {
            scope = ReflowScope::Beat;
            for note in page.semantics.notes.iter().filter(|note| {
                note.staff == target.staff
                    && note.measure == target.measure
                    && note.chord == target.chord
            }) {
                affected_notes.insert(note.id);
                affected_primitives.extend(dependency_primitives(note));
            }
        } else {
            scope = ReflowScope::BeamGroup;
            let beams: BTreeSet<_> = target.attachments.beams.iter().copied().collect();
            for note in page.semantics.notes.iter().filter(|note| {
                note.staff == target.staff
                    && note.measure == target.measure
                    && note.attachments.beams.iter().any(|beam| beams.contains(beam))
            }) {
                affected_notes.insert(note.id);
                affected_primitives.extend(dependency_primitives(note));
            }
        }
    }

    let after_pitch = edit.pitch.or_else(|| target.pitch.as_ref().map(|pitch| pitch.value));
    let after_duration = edit.duration.unwrap_or(target.duration.value);
    let replacement_bounds = replacement_note_bounds(target, after_pitch, &page.geometry.style);
    if page
        .semantics
        .notes
        .iter()
        .filter(|note| note.id != target.id && note.measure == target.measure)
        .any(|note| note.bounds.expand(page.geometry.style.staff_line_thickness).intersects(replacement_bounds))
    {
        scope = scope.max(ReflowScope::Beat);
        for note in page.semantics.notes.iter().filter(|note| {
            note.id != target.id
                && note.measure == target.measure
                && note.bounds.expand(page.geometry.style.staff_space * 0.15).intersects(replacement_bounds)
        }) {
            affected_notes.insert(note.id);
            affected_primitives.extend(dependency_primitives(note));
        }
    }

    let measure = page
        .geometry
        .measures
        .get(target.measure)
        .ok_or(SpliceError::InvalidGeometry)?;
    let spacing_items: Vec<_> = page
        .semantics
        .notes
        .iter()
        .filter(|note| note.measure == target.measure && note.staff == target.staff)
        .map(|note| SpacingItem {
            note: note.id,
            x: note.origin.x,
            half_width: if note.id == target.id {
                replacement_bounds.width() * 0.5
            } else {
                note.bounds.width() * 0.5
            },
        })
        .collect();
    let onset_shifts = if pitch_changed && !duration_changed {
        Vec::new()
    } else {
        match solve_spacing(
            &spacing_items,
            measure.x_range,
            page.geometry.style.staff_space * 0.28,
            options.max_spacing_distortion,
        ) {
            Some(shifts) => shifts,
            None if options.approve_system_reflow => {
                scope = ReflowScope::System;
                warnings.push(SpliceWarning::SystemReflow);
                Vec::new()
            }
            None => {
                scope = scope.max(ReflowScope::Measure);
                warnings.push(SpliceWarning::SpacingInfeasible);
                warnings.push(SpliceWarning::OverlayDoesNotEraseOriginal);
                Vec::new()
            }
        }
    };
    if !onset_shifts.is_empty() {
        scope = scope.max(ReflowScope::Measure);
        let shifted: BTreeSet<_> = onset_shifts.iter().map(|(note, _)| *note).collect();
        for note in page
            .semantics
            .notes
            .iter()
            .filter(|note| shifted.contains(&note.id))
        {
            affected_notes.insert(note.id);
            affected_primitives.extend(dependency_primitives(note));
        }
    }

    let patch_bounds = minimal_patch_bounds(
        &page.display,
        &affected_primitives,
        &page.geometry.style,
    )
    .map(|bounds| bounds.union(replacement_bounds).expand(page.geometry.style.staff_space * 0.35))
    .ok_or(SpliceError::InvalidGeometry)?;
    if !patch_bounds.finite() {
        return Err(SpliceError::InvalidGeometry);
    }
    let replacement = replacement_paint(
        page,
        target,
        after_pitch,
        after_duration,
        patch_bounds,
        &affected_notes,
        &onset_shifts,
    );
    let surgical = options.prefer_operator_rewrite
        && surgical_edits(&page.display, &affected_primitives).filter(|edits| !edits.is_empty()).is_some();
    let erase = match page.classification.kind {
        PdfPageKind::Scan => {
            warnings.push(SpliceWarning::ScanRecognitionUnavailable);
            ErasePlan::RasterPatchUnavailable
        }
        _ if warnings.contains(&SpliceWarning::SpacingInfeasible) => {
            ErasePlan::OverlayOnly { replacement }
        }
        _ if surgical => ErasePlan::OperatorRewrite {
            edits: surgical_edits(&page.display, &affected_primitives).unwrap_or_default(),
            replacement,
        },
        _ => ErasePlan::ClippedOriginalForm { replacement },
    };

    let font = match options.verified_original_font.clone() {
        Some(license) if license.embedding_editable => FontUseDecision::OriginalVerified {
            font_resource: target_font_resource(page, target).unwrap_or_default(),
            license,
        },
        _ => {
            warnings.push(SpliceWarning::OriginalFontLicenseUnknown);
            warnings.push(SpliceWarning::FontSubstituted);
            FontUseDecision::FallbackOfl {
                family: "Bravura-compatible vector geometry".to_string(),
                license: "OFL-1.1".to_string(),
                visual_delta: 0.0,
            }
        }
    };
    Ok(SplicePlan {
        page: edit.page,
        note: edit.note,
        scope,
        patch_bounds,
        erase,
        style: page.geometry.style.clone(),
        font,
        affected_notes: affected_notes.into_iter().collect(),
        affected_primitives: affected_primitives.into_iter().collect(),
        onset_shifts,
        before_pitch: target.pitch.as_ref().map(|pitch| pitch.value),
        after_pitch,
        before_duration: target.duration.value,
        after_duration,
        warnings,
        requires_explicit_approval: scope == ReflowScope::System && !options.approve_system_reflow,
    })
}

fn target_font_resource(
    page: &crate::RecognizedPage,
    target: &RecognizedNote,
) -> Option<String> {
    match page.display.primitive(target.page_primitive)? {
        DisplayPrimitive::Glyph(glyph) => Some(glyph.font_resource.clone()),
        _ => None,
    }
}

fn replacement_note_bounds(
    note: &RecognizedNote,
    new_pitch: Option<Pitch>,
    style: &StyleProfile,
) -> Rect {
    let delta_steps = note
        .pitch
        .as_ref()
        .zip(new_pitch)
        .map(|(before, after)| diatonic_absolute(after) - diatonic_absolute(before.value))
        .unwrap_or(0);
    let center = Point::new(
        note.origin.x,
        note.origin.y + f64::from(delta_steps) * style.staff_space * 0.5,
    );
    Rect::new(
        center.x - style.notehead_width * 0.5,
        center.y - style.notehead_height * 0.5,
        center.x + style.notehead_width * 0.5,
        center.y + style.notehead_height * 0.5,
    )
}

fn diatonic_absolute(pitch: Pitch) -> i16 {
    i16::from(pitch.octave) * 7 + pitch.step.index()
}

pub fn minimal_patch_bounds(
    display: &crate::display::DisplayList,
    primitives: &BTreeSet<PrimitiveId>,
    style: &StyleProfile,
) -> Option<Rect> {
    let mut bounds = None;
    for primitive in primitives {
        let Some(value) = display.primitive(*primitive) else {
            continue;
        };
        let Some(value_bounds) = value.bounds() else {
            continue;
        };
        bounds = Some(match bounds {
            Some(current) => Rect::union(current, value_bounds),
            None => value_bounds,
        });
    }
    bounds.map(|bounds| bounds.expand(style.staff_line_thickness.max(0.1) * 2.0))
}

#[derive(Clone, Copy)]
struct SpacingItem {
    note: SemanticId,
    x: f64,
    half_width: f64,
}

fn solve_spacing(
    items: &[SpacingItem],
    measure: (f64, f64),
    gap: f64,
    max_distortion: f64,
) -> Option<Vec<(SemanticId, f64)>> {
    if items.is_empty() {
        return Some(Vec::new());
    }
    let mut items = items.to_vec();
    items.sort_by(|left, right| left.x.total_cmp(&right.x).then(left.note.cmp(&right.note)));
    let mut solved: Vec<f64> = items.iter().map(|item| item.x).collect();
    solved[0] = solved[0].max(measure.0 + items[0].half_width);
    for index in 1..items.len() {
        let minimum = solved[index - 1]
            + items[index - 1].half_width
            + items[index].half_width
            + gap;
        solved[index] = solved[index].max(minimum);
    }
    let overflow = solved[items.len() - 1] + items[items.len() - 1].half_width - measure.1;
    if overflow > 0.0 {
        for value in &mut solved {
            *value -= overflow;
        }
    }
    if solved[0] - items[0].half_width < measure.0 {
        return None;
    }
    let width = (measure.1 - measure.0).max(0.001);
    if solved
        .iter()
        .zip(&items)
        .any(|(solved, item)| (solved - item.x).abs() / width > max_distortion)
    {
        return None;
    }
    Some(
        solved
            .into_iter()
            .zip(items)
            .filter_map(|(solved, item)| {
                let shift = solved - item.x;
                (shift.abs() > 1e-6).then_some((item.note, shift))
            })
            .collect(),
    )
}

fn surgical_edits(
    display: &crate::display::DisplayList,
    primitives: &BTreeSet<PrimitiveId>,
) -> Option<Vec<OperatorEdit>> {
    let mut source_counts = BTreeMap::new();
    for (_, primitive) in &display.primitives {
        let source = primitive.source();
        *source_counts
            .entry((source.object.num, source.stream_index, source.operator_index))
            .or_insert(0_usize) += 1;
    }
    let mut edits = Vec::new();
    for primitive in primitives {
        let value = display.primitive(*primitive)?;
        let source = value.source();
        if !source.form_chain.is_empty()
            || source.subpath_index.unwrap_or(0) != 0
            || source_counts
                .get(&(source.object.num, source.stream_index, source.operator_index))
                .copied()
                != Some(1)
        {
            return None;
        }
        let replacement = match value {
            DisplayPrimitive::Path(_) => b"n".to_vec(),
            DisplayPrimitive::Glyph(glyph) => {
                let operation = display.operators.get(source.operator_index as usize)?;
                if !matches!(
                    operation.operation,
                    RetainedOperator::Parsed(makepad_pdf_parse::PdfOp::ShowText(_))
                ) {
                    return None;
                }
                let adjustment = glyph.invisible_advance_1000?;
                format!("[{adjustment:.9}] TJ").into_bytes()
            }
            _ => return None,
        };
        edits.push(OperatorEdit {
            source: source.clone(),
            replacement,
        });
    }
    Some(edits)
}

fn replacement_paint(
    page: &crate::RecognizedPage,
    target: &RecognizedNote,
    target_pitch: Option<Pitch>,
    target_duration: DurationValue,
    patch: Rect,
    affected_notes: &BTreeSet<SemanticId>,
    onset_shifts: &[(SemanticId, f64)],
) -> Vec<PaintCommand> {
    let style = &page.geometry.style;
    let mut output = Vec::new();
    let notes = page
        .semantics
        .notes
        .iter()
        .filter(|note| affected_notes.contains(&note.id))
        .collect::<Vec<_>>();
    let preserved_paths: BTreeSet<_> = notes
        .iter()
        .flat_map(|note| {
            note.attachments
                .beams
                .iter()
                .chain(note.attachments.ledgers.iter())
                .chain(note.attachments.curves.iter())
                .copied()
        })
        .collect();
    let target_shift = onset_shifts
        .iter()
        .find_map(|(note, shift)| (*note == target.id).then_some(*shift))
        .unwrap_or(0.0);
    let target_center = replacement_note_bounds(target, target_pitch, style).center();
    let target_delta = Point::new(
        target_center.x + target_shift - target.origin.x,
        target_center.y - target.origin.y,
    );
    for primitive in preserved_paths {
        if let Some(DisplayPrimitive::Path(path)) = page.display.primitive(primitive) {
            let commands = if target.attachments.curves.contains(&primitive)
                && (target_delta.x.abs() > 1e-9 || target_delta.y.abs() > 1e-9)
            {
                warp_curve_endpoint(&path.commands, path.bounds, target.origin, target_delta)
            } else {
                path.commands.clone()
            };
            output.push(PaintCommand::Path {
                commands,
                paint: path.paint,
                line_width: path.line_width,
            });
        }
    }
    let affected_staves: BTreeSet<_> = notes.iter().map(|note| note.staff).collect();
    for staff_index in affected_staves {
        if let Some(staff) = page.geometry.staves.get(staff_index) {
            for line in staff.lines {
                if line >= patch.min_y - style.staff_line_thickness
                    && line <= patch.max_y + style.staff_line_thickness
                {
                    output.push(PaintCommand::StaffLine {
                        start: Point::new(patch.min_x, line),
                        end: Point::new(patch.max_x, line),
                        thickness: style.staff_line_thickness,
                    });
                }
            }
        }
    }

    let shifts: BTreeMap<_, _> = onset_shifts.iter().copied().collect();
    for note in notes {
        let pitch = if note.id == target.id {
            target_pitch
        } else {
            note.pitch.as_ref().map(|pitch| pitch.value)
        };
        let duration = if note.id == target.id {
            target_duration
        } else {
            note.duration.value
        };
        let shift = shifts.get(&note.id).copied().unwrap_or(0.0);
        paint_note(page, note, pitch, duration, shift, &mut output);
    }
    output
}

fn warp_curve_endpoint(
    commands: &[PathCommand],
    bounds: Rect,
    target: Point,
    delta: Point,
) -> Vec<PathCommand> {
    let target_is_left = (target.x - bounds.min_x).abs() <= (target.x - bounds.max_x).abs();
    let width = bounds.width().max(0.001);
    let transform = |point: Point| {
        let position = ((point.x - bounds.min_x) / width).clamp(0.0, 1.0);
        let weight = if target_is_left {
            1.0 - position
        } else {
            position
        };
        Point::new(point.x + delta.x * weight, point.y + delta.y * weight)
    };
    commands
        .iter()
        .map(|command| match command {
            PathCommand::Move(point) => PathCommand::Move(transform(*point)),
            PathCommand::Line(point) => PathCommand::Line(transform(*point)),
            PathCommand::Cubic(first, second, third) => {
                PathCommand::Cubic(transform(*first), transform(*second), transform(*third))
            }
            PathCommand::Close => PathCommand::Close,
        })
        .collect()
}

fn paint_note(
    page: &crate::RecognizedPage,
    note: &RecognizedNote,
    pitch: Option<Pitch>,
    duration: DurationValue,
    x_shift: f64,
    output: &mut Vec<PaintCommand>,
) {
    let style = &page.geometry.style;
    let bounds = replacement_note_bounds(note, pitch, style);
    let center = Point::new(bounds.center().x + x_shift, bounds.center().y);
    output.push(PaintCommand::Notehead {
        center,
        width: style.notehead_width,
        height: style.notehead_height,
        filled: duration.denominator >= 4,
    });
    if duration.denominator > 1 {
        let up = note.voice.value != 2;
        let stem_x = if up {
            center.x + style.notehead_width * 0.48
        } else {
            center.x - style.notehead_width * 0.48
        };
        let stem_end = center.y
            + if up {
                style.staff_space * 3.5
            } else {
                -style.staff_space * 3.5
            };
        output.push(PaintCommand::Stem {
            start: Point::new(stem_x, center.y),
            end: Point::new(stem_x, stem_end),
            thickness: style.stem_thickness,
        });
        if duration.denominator > 4 && note.attachments.beams.is_empty() {
            let levels = duration.denominator.ilog2().saturating_sub(2).min(5);
            let direction = if up { -1.0 } else { 1.0 };
            for level in 0..levels {
                let y = stem_end + direction * f64::from(level) * style.staff_space * 0.72;
                output.push(PaintCommand::Path {
                    commands: vec![
                        PathCommand::Move(Point::new(stem_x, y)),
                        PathCommand::Cubic(
                            Point::new(stem_x + style.staff_space * 0.65, y),
                            Point::new(
                                stem_x + style.staff_space * 0.9,
                                y + direction * style.staff_space * 1.2,
                            ),
                            Point::new(
                                stem_x + style.staff_space * 0.45,
                                y + direction * style.staff_space * 1.65,
                            ),
                        ),
                    ],
                    paint: PathPaint::Stroke,
                    line_width: style.stem_thickness.max(style.staff_space * 0.22),
                });
            }
        }
    }
    if let Some(accidental) = note.attachments.accidental {
        let name = page
            .semantics
            .symbols
            .iter()
            .find(|symbol| symbol.primitive == accidental)
            .map(|symbol| symbol.symbol.value.canonical_name.clone())
            .unwrap_or_else(|| "accidentalNatural".to_string());
        if let Some(origin) = page
            .display
            .primitive(accidental)
            .and_then(DisplayPrimitive::bounds)
            .map(|bounds| bounds.center())
        {
            output.push(PaintCommand::AccidentalText {
                origin: Point::new(origin.x + x_shift, center.y),
                name,
            });
        }
    }
    for index in 0..duration.dots {
        output.push(PaintCommand::Dot {
            center: Point::new(
                center.x
                    + style.notehead_width * 0.5
                    + style.staff_space * (0.55 + f64::from(index) * 0.45),
                center.y,
            ),
            diameter: style.dot_diameter,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spacing_preserves_original_positions_when_they_fit() {
        let items = [
            SpacingItem {
                note: SemanticId(1),
                x: 20.0,
                half_width: 2.0,
            },
            SpacingItem {
                note: SemanticId(2),
                x: 40.0,
                half_width: 2.0,
            },
        ];
        assert_eq!(solve_spacing(&items, (10.0, 50.0), 1.0, 0.02), Some(Vec::new()));
    }
}
