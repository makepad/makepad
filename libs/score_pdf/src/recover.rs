//! Staff, system, barline, measure, beam and page-kind recovery.

use crate::confidence::{Estimate, Evidence, Verification};
use crate::display::{DisplayList, DisplayPrimitive, PathCommand, PathPaint, PdfPath, PrimitiveId};
use crate::geometry::{approximately, median, Point, Rect};
use crate::normalize::SymbolNormalizer;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PdfPageKind {
    Vector,
    Scan,
    Hybrid,
    Opaque,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageEvidence {
    pub glyphs: usize,
    pub named_glyphs: usize,
    pub music_symbols: usize,
    pub painted_paths: usize,
    pub images: usize,
    pub image_coverage: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageClassification {
    pub kind: PdfPageKind,
    pub confidence: f32,
    pub evidence: PageEvidence,
    pub recognition_available: bool,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineSegment {
    pub start: Point,
    pub end: Point,
    pub width: f64,
    pub primitive: PrimitiveId,
    pub stroked: bool,
    pub filled: bool,
}

impl LineSegment {
    pub fn length(self) -> f64 {
        self.start.distance(self.end)
    }

    pub fn bounds(self) -> Rect {
        Rect::from_points(self.start, self.end)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StaffGeometry {
    pub index: usize,
    /// Staff lines from bottom to top in PDF coordinates.
    pub lines: [f64; 5],
    pub x_range: (f64, f64),
    pub staff_space: f64,
    pub line_thickness: f64,
    pub confidence: Estimate<()>,
    pub line_primitives: [PrimitiveId; 5],
}

impl StaffGeometry {
    pub fn bottom_y(&self) -> f64 {
        self.lines[0]
    }

    pub fn top_y(&self) -> f64 {
        self.lines[4]
    }

    pub fn staff_step(&self, y: f64) -> (i16, f64) {
        let raw = 2.0 * (y - self.bottom_y()) / self.staff_space;
        let rounded = raw.round();
        (rounded as i16, (raw - rounded).abs())
    }

    pub fn accepts(&self, point: Point) -> bool {
        point.x >= self.x_range.0 - self.staff_space
            && point.x <= self.x_range.1 + self.staff_space
            && point.y >= self.bottom_y() - self.staff_space * 8.0
            && point.y <= self.top_y() + self.staff_space * 8.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TopologicalBarline {
    pub x: f64,
    pub upper_staff: usize,
    pub lower_staff: usize,
    pub segment: LineSegment,
    pub confidence: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SystemGeometry {
    pub index: usize,
    pub staves: Vec<usize>,
    pub staff_space: f64,
    pub x_range: (f64, f64),
    pub bounds: Rect,
    pub barlines: Vec<TopologicalBarline>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MeasureRegion {
    pub index: usize,
    pub system: usize,
    pub ordinal_in_system: usize,
    pub x_range: (f64, f64),
    pub bounds: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BeamGeometry {
    pub primitive: PrimitiveId,
    pub bounds: Rect,
    pub left_thickness: f64,
    pub right_thickness: f64,
    pub center_y: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleProfile {
    pub staff_space: f64,
    pub staff_line_thickness: f64,
    pub stem_thickness: f64,
    pub beam_thickness: f64,
    pub notehead_width: f64,
    pub notehead_height: f64,
    pub dot_diameter: f64,
    pub curve_thickness: f64,
    pub ink_gray: f64,
}

#[derive(Clone, Debug, Default)]
pub struct PageGeometry {
    pub staves: Vec<StaffGeometry>,
    pub systems: Vec<SystemGeometry>,
    pub barlines: Vec<TopologicalBarline>,
    pub measures: Vec<MeasureRegion>,
    pub beams: Vec<BeamGeometry>,
    pub style: StyleProfile,
}

pub fn classify_page(
    display: &DisplayList,
    normalizer: &SymbolNormalizer,
) -> PageClassification {
    let mut evidence = PageEvidence::default();
    let page_area = display.crop_box.area().max(1.0);
    let mut image_area = 0.0;
    for (_, primitive) in &display.primitives {
        match primitive {
            DisplayPrimitive::Glyph(glyph) => {
                evidence.glyphs += 1;
                evidence.named_glyphs += usize::from(glyph.raw_name.is_some());
                evidence.music_symbols += usize::from(normalizer.normalize(glyph, None).is_some());
            }
            DisplayPrimitive::Path(path) if path.paint != PathPaint::None => {
                evidence.painted_paths += 1;
            }
            DisplayPrimitive::Image(image) => {
                evidence.images += 1;
                image_area += image.bounds.area();
            }
            _ => {}
        }
    }
    evidence.image_coverage = (image_area / page_area).clamp(0.0, 1.0);
    let vector_evidence = evidence.music_symbols > 0
        || evidence.glyphs >= 8
        || evidence.painted_paths >= 20;
    let (kind, confidence, reason) = if evidence.image_coverage >= 0.78
        && evidence.music_symbols == 0
        && evidence.painted_paths < 20
    {
        (
            PdfPageKind::Scan,
            0.99,
            "page-sized raster image with no recoverable music-font evidence".to_string(),
        )
    } else if evidence.image_coverage >= 0.20 && vector_evidence {
        (
            PdfPageKind::Hybrid,
            0.9,
            "substantial raster and vector evidence coexist".to_string(),
        )
    } else if vector_evidence {
        (
            PdfPageKind::Vector,
            0.98,
            "positioned glyphs or painted vector marks are present".to_string(),
        )
    } else {
        (
            PdfPageKind::Opaque,
            0.55,
            "insufficient font, image, and path evidence for a safe route".to_string(),
        )
    };
    PageClassification {
        kind,
        confidence,
        evidence,
        recognition_available: matches!(kind, PdfPageKind::Vector | PdfPageKind::Hybrid),
        reason,
    }
}

pub fn recover_page_geometry(display: &DisplayList) -> PageGeometry {
    let all_segments = line_segments(display);
    let staves = recover_staves(display, &all_segments);
    let barlines = recover_barlines(&all_segments, &staves);
    let systems = recover_systems(&staves, &barlines);
    let measures = recover_measures(&systems);
    let staff_space = {
        let mut values: Vec<_> = staves.iter().map(|staff| staff.staff_space).collect();
        median(&mut values).unwrap_or(0.0)
    };
    let beams = recover_beams(display, staff_space);
    let style = measure_style(display, &staves, &all_segments, &barlines, &beams);
    PageGeometry {
        staves,
        systems,
        barlines,
        measures,
        beams,
        style,
    }
}

pub fn line_segments(display: &DisplayList) -> Vec<LineSegment> {
    let mut output = Vec::new();
    for (primitive, value) in &display.primitives {
        let DisplayPrimitive::Path(path) = value else {
            continue;
        };
        let mut current = None;
        let mut first = None;
        for command in &path.commands {
            match command {
                PathCommand::Move(point) => {
                    current = Some(*point);
                    first = Some(*point);
                }
                PathCommand::Line(point) => {
                    if let Some(start) = current {
                        output.push(LineSegment {
                            start,
                            end: *point,
                            width: effective_path_width(path, start, *point),
                            primitive: *primitive,
                            stroked: path.paint.is_stroked(),
                            filled: path.paint.is_filled(),
                        });
                    }
                    current = Some(*point);
                }
                PathCommand::Cubic(_, _, point) => current = Some(*point),
                PathCommand::Close => {
                    if let (Some(start), Some(end)) = (current, first) {
                        if start.distance(end) > 1e-6 {
                            output.push(LineSegment {
                                start,
                                end,
                                width: effective_path_width(path, start, end),
                                primitive: *primitive,
                                stroked: path.paint.is_stroked(),
                                filled: path.paint.is_filled(),
                            });
                        }
                    }
                }
            }
        }
    }
    output
}

fn effective_path_width(path: &PdfPath, start: Point, end: Point) -> f64 {
    if path.paint.is_stroked() {
        path.line_width
    } else if (end.x - start.x).abs() >= (end.y - start.y).abs() {
        path.bounds.height()
    } else {
        path.bounds.width()
    }
}

#[derive(Clone, Copy)]
struct HorizontalCandidate {
    y: f64,
    left: f64,
    right: f64,
    thickness: f64,
    primitive: PrimitiveId,
    stroked: bool,
}

fn recover_staves(display: &DisplayList, segments: &[LineSegment]) -> Vec<StaffGeometry> {
    let max_rule_width = display.crop_box.height() * 0.006;
    let mut candidates: Vec<_> = segments
        .iter()
        .filter(|segment| is_horizontal(**segment))
        .filter_map(|segment| {
            let bounds = segment.bounds();
            (bounds.width() >= 3.0 && segment.width <= max_rule_width).then_some(
                HorizontalCandidate {
                    y: (segment.start.y + segment.end.y) * 0.5,
                    left: bounds.min_x,
                    right: bounds.max_x,
                    thickness: segment.width.max(0.01),
                    primitive: segment.primitive,
                    stroked: segment.stroked,
                },
            )
        })
        .collect();
    candidates.sort_by(|left, right| {
        left.y
            .total_cmp(&right.y)
            .then(left.left.total_cmp(&right.left))
    });
    candidates.dedup_by(|left, right| {
        (left.y - right.y).abs() <= left.thickness.max(right.thickness) * 1.25
            && (left.left - right.left).abs() < 1.0
            && (left.right - right.right).abs() < 1.0
    });
    let mut merged: Vec<HorizontalCandidate> = Vec::new();
    for candidate in candidates {
        let compatible = merged.iter_mut().rev().find(|existing| {
            let same_rule = (existing.y - candidate.y).abs()
                <= existing
                    .thickness
                    .max(candidate.thickness)
                    .mul_add(1.25, 0.08);
            let fragments_join = existing.stroked
                && candidate.stroked
                && candidate.left <= existing.right + 3.0
                && candidate.right >= existing.left - 3.0;
            let filled_duplicate = !existing.stroked
                && !candidate.stroked
                && (candidate.left - existing.left).abs() < 1.0
                && (candidate.right - existing.right).abs() < 1.0;
            same_rule && (fragments_join || filled_duplicate)
        });
        if let Some(existing) = compatible {
            let left = existing.left.min(candidate.left);
            let right = existing.right.max(candidate.right);
            existing.y = (existing.y + candidate.y) * 0.5;
            existing.left = left;
            existing.right = right;
            existing.thickness = existing.thickness.max(candidate.thickness);
            existing.stroked |= candidate.stroked;
        } else {
            merged.push(candidate);
        }
    }
    let maximum_horizontal = merged
        .iter()
        .map(|candidate| candidate.right - candidate.left)
        .fold(0.0_f64, f64::max);
    let minimum_length = (maximum_horizontal * 0.12).max(12.0);
    let mut candidates: Vec<_> = merged
        .into_iter()
        .filter(|candidate| candidate.right - candidate.left >= minimum_length)
        .collect();
    candidates.sort_by(|left, right| left.y.total_cmp(&right.y));

    #[derive(Clone)]
    struct Proposal {
        indices: [usize; 5],
        score: f64,
        space: f64,
    }
    let mut proposals = Vec::new();
    for first in 0..candidates.len() {
        for second in first + 1..candidates.len().min(first + 20) {
            let space = candidates[second].y - candidates[first].y;
            if !(2.0..=20.0).contains(&space) {
                continue;
            }
            let mut selected = [first, second, usize::MAX, usize::MAX, usize::MAX];
            let mut residual = 0.0;
            let mut valid = true;
            for line in 2..5 {
                let expected = candidates[first].y + space * line as f64;
                let best = (second + 1..candidates.len())
                    .filter(|index| !selected[..line].contains(index))
                    .filter_map(|index| {
                        let candidate = candidates[index];
                        let y_error = (candidate.y - expected).abs();
                        let overlap = overlap_ratio(candidates[first], candidate);
                        (y_error <= space * 0.13 && overlap >= 0.72)
                            .then_some((index, y_error, overlap))
                    })
                    .min_by(|left, right| left.1.total_cmp(&right.1));
                let Some((index, y_error, overlap)) = best else {
                    valid = false;
                    break;
                };
                selected[line] = index;
                residual += y_error / space + (1.0 - overlap) * 0.1;
            }
            if valid {
                let mean_length = selected
                    .iter()
                    .map(|index| candidates[*index].right - candidates[*index].left)
                    .sum::<f64>()
                    / 5.0;
                proposals.push(Proposal {
                    indices: selected,
                    score: 1.0 - residual / 5.0
                        + mean_length / maximum_horizontal.max(1.0) * 0.5,
                    space,
                });
            }
        }
    }
    proposals.sort_by(|left, right| right.score.total_cmp(&left.score));
    let mut used = BTreeSet::new();
    let mut staves = Vec::new();
    for proposal in proposals {
        if proposal.indices.iter().any(|index| used.contains(index)) {
            continue;
        }
        let lines = proposal.indices.map(|index| candidates[index]);
        let left = lines.iter().map(|line| line.left).fold(f64::NEG_INFINITY, f64::max);
        let right = lines.iter().map(|line| line.right).fold(f64::INFINITY, f64::min);
        if right <= left {
            continue;
        }
        let mut thicknesses = lines.map(|line| line.thickness);
        let line_thickness = median(&mut thicknesses).unwrap_or(0.1);
        let index = staves.len();
        staves.push(StaffGeometry {
            index,
            lines: lines.map(|line| line.y),
            x_range: (left, right),
            staff_space: proposal.space,
            line_thickness,
            confidence: Estimate::new(
                (),
                proposal.score.clamp(0.0, 1.0) as f32,
                0.8,
                vec![Evidence::StaffResidual((1.0 - proposal.score) as f32)],
                Verification::Inferred,
            ),
            line_primitives: lines.map(|line| line.primitive),
        });
        used.extend(proposal.indices);
    }
    staves.sort_by(|left, right| right.top_y().total_cmp(&left.top_y()));
    for (index, staff) in staves.iter_mut().enumerate() {
        staff.index = index;
    }
    staves
}

fn overlap_ratio(left: HorizontalCandidate, right: HorizontalCandidate) -> f64 {
    let overlap = (left.right.min(right.right) - left.left.max(right.left)).max(0.0);
    let shortest = (left.right - left.left).min(right.right - right.left).max(0.001);
    overlap / shortest
}

fn is_horizontal(segment: LineSegment) -> bool {
    let dx = (segment.end.x - segment.start.x).abs();
    let dy = (segment.end.y - segment.start.y).abs();
    dx > 0.0 && dy <= (dx * 0.003).max(segment.width * 0.6)
}

fn is_vertical(segment: LineSegment) -> bool {
    let dx = (segment.end.x - segment.start.x).abs();
    let dy = (segment.end.y - segment.start.y).abs();
    dy > 0.0 && dx <= (dy * 0.01).max(segment.width * 0.8)
}

fn recover_barlines(
    segments: &[LineSegment],
    staves: &[StaffGeometry],
) -> Vec<TopologicalBarline> {
    let mut output = Vec::new();
    for segment in segments.iter().copied().filter(|segment| is_vertical(*segment)) {
        let low = if segment.start.y <= segment.end.y {
            segment.start
        } else {
            segment.end
        };
        let high = if segment.start.y > segment.end.y {
            segment.start
        } else {
            segment.end
        };
        let low_staff = endpoint_staff(low, staves);
        let high_staff = endpoint_staff(high, staves);
        let (Some((lower, low_error)), Some((upper, high_error))) = (low_staff, high_staff) else {
            continue;
        };
        if lower == upper {
            continue;
        }
        let space = (staves[lower].staff_space + staves[upper].staff_space) * 0.5;
        if segment.length() < space * 4.5 {
            continue;
        }
        output.push(TopologicalBarline {
            x: (segment.start.x + segment.end.x) * 0.5,
            upper_staff: upper.min(lower),
            lower_staff: upper.max(lower),
            segment,
            confidence: (1.0 - ((low_error + high_error) / (space * 0.5)) as f32)
                .clamp(0.6, 1.0),
        });
    }
    output.sort_by(|left, right| left.x.total_cmp(&right.x));
    output.dedup_by(|left, right| {
        (left.x - right.x).abs() <= left.segment.width.max(right.segment.width) * 1.5
            && left.upper_staff == right.upper_staff
            && left.lower_staff == right.lower_staff
    });
    output
}

fn endpoint_staff(point: Point, staves: &[StaffGeometry]) -> Option<(usize, f64)> {
    staves
        .iter()
        .filter(|staff| point.x >= staff.x_range.0 - 1.0 && point.x <= staff.x_range.1 + 1.0)
        .filter_map(|staff| {
            let error = staff
                .lines
                .iter()
                .map(|line| (point.y - line).abs())
                .fold(f64::INFINITY, f64::min);
            (error <= staff.staff_space * 0.18).then_some((staff.index, error))
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
}

fn recover_systems(
    staves: &[StaffGeometry],
    barlines: &[TopologicalBarline],
) -> Vec<SystemGeometry> {
    let mut parent: Vec<_> = (0..staves.len()).collect();
    for barline in barlines {
        union(&mut parent, barline.upper_staff, barline.lower_staff);
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for staff in staves {
        let root = find(&mut parent, staff.index);
        if let Some(group) = groups.iter_mut().find(|group| {
            let first = group[0];
            find(&mut parent, first) == root
        }) {
            group.push(staff.index);
        } else {
            groups.push(vec![staff.index]);
        }
    }
    let mut systems = Vec::new();
    for mut group in groups {
        group.sort_unstable();
        let left = group
            .iter()
            .map(|index| staves[*index].x_range.0)
            .fold(f64::INFINITY, f64::min);
        let right = group
            .iter()
            .map(|index| staves[*index].x_range.1)
            .fold(f64::NEG_INFINITY, f64::max);
        let bottom = group
            .iter()
            .map(|index| staves[*index].bottom_y())
            .fold(f64::INFINITY, f64::min);
        let top = group
            .iter()
            .map(|index| staves[*index].top_y())
            .fold(f64::NEG_INFINITY, f64::max);
        let connected: Vec<_> = barlines
            .iter()
            .filter(|barline| {
                group.contains(&barline.upper_staff) && group.contains(&barline.lower_staff)
            })
            .cloned()
            .collect();
        let mut spaces: Vec<_> = group
            .iter()
            .map(|index| staves[*index].staff_space)
            .collect();
        systems.push(SystemGeometry {
            index: systems.len(),
            staves: group,
            staff_space: median(&mut spaces).unwrap_or(0.0),
            x_range: (left, right),
            bounds: Rect::new(left, bottom, right, top),
            barlines: connected,
        });
    }
    systems.sort_by(|left, right| right.bounds.max_y.total_cmp(&left.bounds.max_y));
    for (index, system) in systems.iter_mut().enumerate() {
        system.index = index;
    }
    systems
}

fn find(parent: &mut [usize], value: usize) -> usize {
    if parent[value] != value {
        parent[value] = find(parent, parent[value]);
    }
    parent[value]
}

fn union(parent: &mut [usize], left: usize, right: usize) {
    let left = find(parent, left);
    let right = find(parent, right);
    if left != right {
        parent[right] = left;
    }
}

fn recover_measures(systems: &[SystemGeometry]) -> Vec<MeasureRegion> {
    let mut output = Vec::new();
    for system in systems {
        let mut boundaries = vec![system.x_range.0, system.x_range.1];
        boundaries.extend(
            system
                .barlines
                .iter()
                .map(|barline| barline.x)
                .filter(|x| *x > system.x_range.0 + 0.5 && *x < system.x_range.1 - 0.5),
        );
        boundaries.sort_by(f64::total_cmp);
        boundaries.dedup_by(|left, right| {
            approximately(*left, *right, system.staff_space.mul_add(1.25, 0.2).max(0.8))
        });
        for (ordinal, pair) in boundaries.windows(2).enumerate() {
            if pair[1] - pair[0] < 2.0 {
                continue;
            }
            output.push(MeasureRegion {
                index: output.len(),
                system: system.index,
                ordinal_in_system: ordinal,
                x_range: (pair[0], pair[1]),
                bounds: Rect::new(pair[0], system.bounds.min_y, pair[1], system.bounds.max_y),
            });
        }
    }
    output
}

pub fn beam_thickness_at_ends(points: &[Point]) -> Option<(f64, f64)> {
    if points.len() < 4 {
        return None;
    }
    let mut sorted = points[..4].to_vec();
    sorted.sort_by(|left, right| left.x.total_cmp(&right.x).then(left.y.total_cmp(&right.y)));
    let left = sorted[0].distance(sorted[1]);
    let right = sorted[2].distance(sorted[3]);
    Some((left, right))
}

fn recover_beams(display: &DisplayList, staff_space: f64) -> Vec<BeamGeometry> {
    if staff_space <= 0.0 {
        return Vec::new();
    }
    let mut output = Vec::new();
    for (primitive, value) in &display.primitives {
        let DisplayPrimitive::Path(path) = value else {
            continue;
        };
        if !path.paint.is_filled() || path.bounds.width() < staff_space * 1.5 {
            continue;
        }
        let points: Vec<_> = path
            .commands
            .iter()
            .filter_map(|command| match command {
                PathCommand::Move(point) | PathCommand::Line(point) => Some(*point),
                _ => None,
            })
            .take(4)
            .collect();
        let Some((left, right)) = beam_thickness_at_ends(&points) else {
            continue;
        };
        if !(staff_space * 0.08..=staff_space * 0.9).contains(&left)
            || !(staff_space * 0.08..=staff_space * 0.9).contains(&right)
            || left.max(right) / left.min(right).max(0.001) > 1.8
        {
            continue;
        }
        output.push(BeamGeometry {
            primitive: *primitive,
            bounds: path.bounds,
            left_thickness: left,
            right_thickness: right,
            center_y: path.bounds.center().y,
        });
    }
    output
}

fn measure_style(
    display: &DisplayList,
    staves: &[StaffGeometry],
    segments: &[LineSegment],
    barlines: &[TopologicalBarline],
    beams: &[BeamGeometry],
) -> StyleProfile {
    let mut spaces: Vec<_> = staves.iter().map(|staff| staff.staff_space).collect();
    let mut staff_widths: Vec<_> = staves.iter().map(|staff| staff.line_thickness).collect();
    let staff_space = median(&mut spaces).unwrap_or(0.0);
    let staff_line_thickness = median(&mut staff_widths).unwrap_or(0.0);
    let barline_primitives: BTreeSet<_> = barlines
        .iter()
        .map(|barline| barline.segment.primitive)
        .collect();
    let mut stem_widths: Vec<_> = segments
        .iter()
        .filter(|segment| is_vertical(**segment))
        .filter(|segment| !barline_primitives.contains(&segment.primitive))
        .filter(|segment| segment.length() >= staff_space * 2.0)
        .filter(|segment| segment.length() <= staff_space * 7.0)
        .map(|segment| segment.width)
        .collect();
    let mut beam_widths: Vec<_> = beams
        .iter()
        .map(|beam| (beam.left_thickness + beam.right_thickness) * 0.5)
        .collect();
    let mut glyph_widths = Vec::new();
    let mut glyph_heights = Vec::new();
    for (_, primitive) in &display.primitives {
        if let DisplayPrimitive::Glyph(glyph) = primitive {
            if glyph
                .raw_name
                .as_deref()
                .is_some_and(|name| name.contains("notehead"))
            {
                glyph_widths.push(glyph.bounds.width());
                glyph_heights.push(glyph.bounds.height());
            }
        }
    }
    StyleProfile {
        staff_space,
        staff_line_thickness,
        stem_thickness: median(&mut stem_widths).unwrap_or(staff_line_thickness),
        beam_thickness: median(&mut beam_widths).unwrap_or(staff_space * 0.5),
        notehead_width: median(&mut glyph_widths).unwrap_or(staff_space * 1.18),
        notehead_height: median(&mut glyph_heights).unwrap_or(staff_space * 0.84),
        dot_diameter: staff_space * 0.32,
        curve_thickness: staff_line_thickness * 1.2,
        ink_gray: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_beam_is_measured_at_both_ends() {
        let points = [
            Point::new(10.0, 20.0),
            Point::new(10.0, 22.0),
            Point::new(40.0, 20.0),
            Point::new(40.0, 22.0),
        ];
        assert_eq!(beam_thickness_at_ends(&points), Some((2.0, 2.0)));
    }
}
