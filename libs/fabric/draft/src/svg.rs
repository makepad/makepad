use crate::geom::{nearest_tangent, polyline_path};
use crate::nest::{transform_path, transform_point};
use crate::{flatten, offset, Layout, Part, Path, Pattern, Placement, Point, Segment};
use std::fmt::Write;

fn number(value: f64) -> String {
    let rounded = if value.abs() < 0.0005 { 0.0 } else { value };
    format!("{rounded:.2}")
}

fn escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn id(value: &str, index: usize) -> String {
    let mut result = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            result.push(character.to_ascii_lowercase());
        } else if !result.ends_with('-') {
            result.push('-');
        }
    }
    format!("part-{index}-{}", result.trim_matches('-'))
}

pub(crate) fn cut_outline(part: &Part) -> Path {
    let cut = offset(&part.outline, part.seam_allowance_mm);
    if !part.on_fold {
        return cut;
    }
    let (seam_min, _) = part.outline.bounds();
    let mut points = flatten(&cut, 0.1);
    for point in &mut points {
        if point.x < seam_min.x {
            point.x = seam_min.x;
        }
    }
    polyline_path(&points, true)
}

pub(crate) fn path_data(path: &Path) -> String {
    let mut data = format!("M {} {}", number(path.start.x), number(path.start.y));
    for segment in &path.segments {
        match *segment {
            Segment::Line { to } => {
                let _ = write!(data, " L {} {}", number(to.x), number(to.y));
            }
            Segment::Curve { c1, c2, to } => {
                let _ = write!(
                    data,
                    " C {} {},{} {},{} {}",
                    number(c1.x), number(c1.y), number(c2.x), number(c2.y), number(to.x), number(to.y)
                );
            }
        }
    }
    if path.closed {
        data.push_str(" Z");
    }
    data
}

fn polyline_points(path: &Path, placement: &Placement) -> String {
    flatten(path, 0.1)
        .into_iter()
        .map(|point| {
            let point = transform_point(point, placement);
            format!("{},{}", number(point.x), number(point.y))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn line(output: &mut String, a: Point, b: Point, class: &str) {
    let _ = writeln!(
        output,
        "    <line class=\"{class}\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" />",
        number(a.x), number(a.y), number(b.x), number(b.y)
    );
}

fn grainline(output: &mut String, a: Point, b: Point) {
    line(output, a, b, "grainline");
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length = dx.hypot(dy).max(1.0);
    let ux = dx / length;
    let uy = dy / length;
    let nx = -uy;
    let ny = ux;
    for (tip, direction) in [(a, 1.0), (b, -1.0)] {
        let base = Point::new(tip.x + ux * 8.0 * direction, tip.y + uy * 8.0 * direction);
        line(output, tip, Point::new(base.x + nx * 3.0, base.y + ny * 3.0), "grainline");
        line(output, tip, Point::new(base.x - nx * 3.0, base.y - ny * 3.0), "grainline");
    }
}

pub(crate) fn to_svg(pattern: &Pattern, layout: &Layout) -> String {
    let width = layout.width_mm.max(100.0);
    let height = layout.height_mm.max(100.0);
    let mut output = String::new();
    let _ = writeln!(
        output,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}mm\" height=\"{}mm\" viewBox=\"0 0 {} {}\">",
        number(width), number(height), number(width), number(height)
    );
    output.push_str("  <style>.cut{fill:none;stroke:#000;stroke-width:.5}.seam{fill:none;stroke:#777;stroke-width:.3;stroke-dasharray:4 3}.internal{fill:none;stroke:#777;stroke-width:.3;stroke-dasharray:5 3}.notch,.grainline{fill:none;stroke:#000;stroke-width:.4}text{font-family:Helvetica,Arial,sans-serif;font-size:8mm}.part-name{font-size:12mm;font-weight:bold}</style>\n");
    output.push_str("  <g id=\"test-square\">\n    <rect x=\"0\" y=\"0\" width=\"100\" height=\"100\" fill=\"none\" stroke=\"#000\" stroke-width=\"0.5\" />\n    <text x=\"38\" y=\"53\">10 cm</text>\n  </g>\n");

    for placement in &layout.placements {
        let part = &pattern.parts[placement.part];
        let _ = writeln!(output, "  <g id=\"{}\">", id(&part.name, placement.part));
        let cut = transform_path(&cut_outline(part), placement);
        let seam = transform_path(&part.outline, placement);
        let _ = writeln!(output, "    <path class=\"cut\" d=\"{}\" />", path_data(&cut));
        let _ = writeln!(output, "    <path class=\"seam\" d=\"{}\" />", path_data(&seam));
        for internal in &part.internal {
            let _ = writeln!(
                output,
                "    <polyline class=\"internal\" points=\"{}\" />",
                polyline_points(internal, placement)
            );
        }
        for notch in &part.notches {
            let tangent = nearest_tangent(&part.outline, *notch);
            let normal = Point::new(-tangent.y, tangent.x);
            let a = Point::new(notch.x - normal.x * 3.0, notch.y - normal.y * 3.0);
            let b = Point::new(notch.x + normal.x * 3.0, notch.y + normal.y * 3.0);
            line(&mut output, transform_point(a, placement), transform_point(b, placement), "notch");
        }
        grainline(
            &mut output,
            transform_point(part.grainline.0, placement),
            transform_point(part.grainline.1, placement),
        );
        for label in &part.labels {
            let at = transform_point(label.at, placement);
            let class = if label.text == part.name { " class=\"part-name\"" } else { "" };
            let angle = label.angle_deg + placement.rotation_deg;
            let transform = if angle.abs() > 0.01 {
                format!(" transform=\"rotate({} {} {})\"", number(angle), number(at.x), number(at.y))
            } else {
                String::new()
            };
            let _ = writeln!(
                output,
                "    <text{class} x=\"{}\" y=\"{}\"{transform}>{}</text>",
                number(at.x), number(at.y), escape(&label.text)
            );
        }
        output.push_str("  </g>\n");
    }
    output.push_str("</svg>\n");
    output
}
