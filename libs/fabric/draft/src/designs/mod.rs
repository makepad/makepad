mod aline_skirt;
mod easy_trousers;
mod tshirt;

use crate::geom::distance_to_polyline;
use crate::{flatten, Design, DraftError, Label, Measurements, Part, Path, Point, Segment};

pub(crate) fn all() -> Vec<Box<dyn Design>> {
    vec![
        Box::new(tshirt::Tshirt),
        Box::new(aline_skirt::AlineSkirt),
        Box::new(easy_trousers::EasyTrousers),
    ]
}

pub(super) fn measurement(value_cm: f32, key: &'static str) -> Result<f64, DraftError> {
    if !value_cm.is_finite() || value_cm <= 0.0 {
        Err(DraftError::MissingMeasurement(key))
    } else {
        Ok(value_cm as f64 * 10.0)
    }
}

pub(super) fn labels(
    design: &str,
    part: &str,
    cut_count: u8,
    on_fold: bool,
    at: Point,
    measurements: [(&str, f64); 3],
) -> Vec<Label> {
    let cut = if on_fold {
        format!("cut {cut_count} on fold")
    } else {
        format!("cut {cut_count}")
    };
    vec![
        Label { at, text: part.to_owned(), angle_deg: 0.0 },
        Label { at: Point::new(at.x, at.y + 15.0), text: design.to_owned(), angle_deg: 0.0 },
        Label { at: Point::new(at.x, at.y + 27.0), text: cut, angle_deg: 0.0 },
        Label {
            at: Point::new(at.x, at.y + 39.0),
            text: format!("{} {:.0} / {} {:.0} / {} {:.0} mm", measurements[0].0, measurements[0].1, measurements[1].0, measurements[1].1, measurements[2].0, measurements[2].1),
            angle_deg: 0.0,
        },
    ]
}

pub(super) fn line_path(a: Point, b: Point) -> Path {
    Path { start: a, segments: vec![Segment::Line { to: b }], closed: false }
}

pub(super) fn append_only_segment(path: &mut Path, curve: Path) {
    debug_assert_eq!(curve.start, path_end(path));
    path.segments.extend(curve.segments);
}

pub(super) fn path_end(path: &Path) -> Point {
    path.segments.last().map_or(path.start, |segment| match *segment {
        Segment::Line { to } | Segment::Curve { to, .. } => to,
    })
}

fn segment_length(from: Point, segment: Segment) -> f64 {
    Path { start: from, segments: vec![segment], closed: false }.length()
}

fn segment_lengths(path: &Path) -> Vec<f64> {
    let mut from = path.start;
    path.segments
        .iter()
        .map(|segment| {
            let length = segment_length(from, *segment);
            from = match *segment {
                Segment::Line { to } | Segment::Curve { to, .. } => to,
            };
            length
        })
        .collect()
}

fn paths_intersect(path: &Path) -> bool {
    let mut points = flatten(path, 0.15);
    if points.len() < 4 {
        return false;
    }
    if points[0].distance(*points.last().unwrap()) > 1.0e-7 {
        points.push(points[0]);
    }
    let edges = points.len() - 1;
    for first in 0..edges {
        for second in (first + 1)..edges {
            if second == first + 1 || (first == 0 && second + 1 == edges) {
                continue;
            }
            if segments_intersect(points[first], points[first + 1], points[second], points[second + 1]) {
                return true;
            }
        }
    }
    false
}

fn orientation(a: Point, b: Point, c: Point) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn segments_intersect(a: Point, b: Point, c: Point, d: Point) -> bool {
    let ab_c = orientation(a, b, c);
    let ab_d = orientation(a, b, d);
    let cd_a = orientation(c, d, a);
    let cd_b = orientation(c, d, b);
    ab_c * ab_d < -1.0e-8 && cd_a * cd_b < -1.0e-8
}

fn compare(messages: &mut Vec<String>, label: &str, a: f64, b: f64, tolerance: f64) {
    if (a - b).abs() > tolerance {
        messages.push(format!("{label} differ by {:.2} mm ({a:.2} vs {b:.2})", (a - b).abs()));
    }
}

pub(crate) fn validate(pattern: &crate::Pattern) -> Vec<String> {
    let mut messages = Vec::new();
    for part in &pattern.parts {
        if !part.outline.closed {
            messages.push(format!("{} outline is not closed", part.name));
            continue;
        }
        let points = flatten(&part.outline, 0.1);
        if points.len() < 4 || points[0].distance(*points.last().unwrap()) > 1.0e-6 {
            messages.push(format!("{} outline does not flatten to a closed polygon", part.name));
        } else if paths_intersect(&part.outline) {
            messages.push(format!("{} outline self-intersects", part.name));
        }
        for (index, notch) in part.notches.iter().enumerate() {
            if distance_to_polyline(*notch, &points) >= 0.5 {
                messages.push(format!("{} notch {} is not on the outline", part.name, index + 1));
            }
        }
    }

    match pattern.design_id.as_str() {
        "tshirt" if pattern.parts.len() >= 3 => {
            let front = segment_lengths(&pattern.parts[0].outline);
            let back = segment_lengths(&pattern.parts[1].outline);
            let sleeve = segment_lengths(&pattern.parts[2].outline);
            if front.len() >= 4 && back.len() >= 4 {
                compare(&mut messages, "T-shirt side seams", front[3], back[3], 2.0);
            }
            if front.len() >= 3 && back.len() >= 3 && sleeve.len() >= 2 {
                compare(&mut messages, "sleeve cap and armholes", sleeve[0] + sleeve[1] - 10.0, front[2] + back[2], 3.0);
            }
        }
        "aline_skirt" if pattern.parts.len() >= 3 => {
            let front = segment_lengths(&pattern.parts[0].outline);
            let back = segment_lengths(&pattern.parts[1].outline);
            if front.len() >= 3 && back.len() >= 3 {
                compare(&mut messages, "skirt side seams", front[1] + front[2], back[1] + back[2], 2.0);
            }
            if let (Some(Segment::Curve { to: front_side, .. }), Some(Segment::Curve { to: back_side, .. })) =
                (pattern.parts[0].outline.segments.first(), pattern.parts[1].outline.segments.first())
            {
                let front_intake = dart_intake(&pattern.parts[0]);
                let back_intake = dart_intake(&pattern.parts[1]);
                let skirt_waist = 2.0 * (front_side.x - front_intake) + 2.0 * (back_side.x - back_intake);
                let (min, max) = pattern.parts[2].outline.bounds();
                let band_length = (max.x - min.x).max(max.y - min.y);
                compare(&mut messages, "waistband and skirt waist plus overlap", band_length, skirt_waist + 30.0, 2.0);
            }
        }
        "easy_trousers" if pattern.parts.len() >= 2 => {
            let front = segment_lengths(&pattern.parts[0].outline);
            let back = segment_lengths(&pattern.parts[1].outline);
            if front.len() >= 5 && back.len() >= 5 {
                compare(&mut messages, "trouser side seams", front[1] + front[2], back[1] + back[2], 2.0);
                compare(&mut messages, "trouser inseams", front[4], back[4], 2.0);
            }
        }
        _ => {}
    }
    messages
}

fn dart_intake(part: &Part) -> f64 {
    part.internal
        .first()
        .and_then(|dart| dart.segments.last().map(|segment| match *segment {
            Segment::Line { to } | Segment::Curve { to, .. } => (to.x - dart.start.x).abs(),
        }))
        .unwrap_or(0.0)
}

#[allow(dead_code)]
fn _measurement_contract(_: &Measurements) {}

#[cfg(test)]
pub(crate) fn print_seam_metrics(pattern: &crate::Pattern) {
    match pattern.design_id.as_str() {
        "tshirt" => {
            let front = segment_lengths(&pattern.parts[0].outline);
            let back = segment_lengths(&pattern.parts[1].outline);
            let sleeve = segment_lengths(&pattern.parts[2].outline);
            println!(
                "T-shirt seams: sides {:.2}/{:.2} mm; armholes {:.2} mm; cap {:.2} mm (less ease {:.2})",
                front[3],
                back[3],
                front[2] + back[2],
                sleeve[0] + sleeve[1],
                sleeve[0] + sleeve[1] - 10.0
            );
        }
        "aline_skirt" => {
            let front = segment_lengths(&pattern.parts[0].outline);
            let back = segment_lengths(&pattern.parts[1].outline);
            println!(
                "Skirt side seams: {:.2}/{:.2} mm",
                front[1] + front[2],
                back[1] + back[2]
            );
        }
        "easy_trousers" => {
            let front = segment_lengths(&pattern.parts[0].outline);
            let back = segment_lengths(&pattern.parts[1].outline);
            println!(
                "Trouser seams: outseams {:.2}/{:.2} mm; inseams {:.2}/{:.2} mm",
                front[1] + front[2],
                back[1] + back[2],
                front[4],
                back[4]
            );
        }
        _ => {}
    }
}
