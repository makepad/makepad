use crate::geom::nearest_tangent;
use crate::nest::transform_point;
use crate::svg::cut_outline;
use crate::{Layout, PageSize, Path, Pattern, Placement, Point, Segment};
use std::fmt::Write;

const POINTS_PER_MM: f64 = 72.0 / 25.4;
const MARGIN: f64 = 8.0;
const OVERLAP: f64 = 10.0;

fn page_dimensions(page: PageSize) -> (f64, f64) {
    match page {
        PageSize::A4 => (210.0, 297.0),
        PageSize::Letter => (215.9, 279.4),
        PageSize::A0 => (841.0, 1189.0),
    }
}

fn n(value: f64) -> String {
    let value = if value.abs() < 0.00005 { 0.0 } else { value };
    format!("{value:.3}")
}

#[derive(Clone, Copy)]
struct PageMap {
    window_x: f64,
    window_y: f64,
    page_height: f64,
}

impl PageMap {
    fn point(self, point: Point, placement: &Placement) -> Point {
        let point = transform_point(point, placement);
        self.global(point)
    }

    fn global(self, point: Point) -> Point {
        Point::new(
            (MARGIN + point.x - self.window_x) * POINTS_PER_MM,
            (self.page_height - MARGIN - (point.y - self.window_y)) * POINTS_PER_MM,
        )
    }
}

fn pdf_path(output: &mut String, path: &Path, placement: &Placement, map: PageMap) {
    let start = map.point(path.start, placement);
    let _ = write!(output, "{} {} m ", n(start.x), n(start.y));
    for segment in &path.segments {
        match *segment {
            Segment::Line { to } => {
                let to = map.point(to, placement);
                let _ = write!(output, "{} {} l ", n(to.x), n(to.y));
            }
            Segment::Curve { c1, c2, to } => {
                let c1 = map.point(c1, placement);
                let c2 = map.point(c2, placement);
                let to = map.point(to, placement);
                let _ = write!(
                    output,
                    "{} {} {} {} {} {} c ",
                    n(c1.x), n(c1.y), n(c2.x), n(c2.y), n(to.x), n(to.y)
                );
            }
        }
    }
    if path.closed {
        output.push_str("h ");
    }
}

fn stroke_path(
    output: &mut String,
    path: &Path,
    placement: &Placement,
    map: PageMap,
    grey: f64,
    width_mm: f64,
    dash_mm: Option<(f64, f64)>,
) {
    let _ = write!(output, "q {} G {} w ", n(grey), n(width_mm * POINTS_PER_MM));
    if let Some((on, off)) = dash_mm {
        let _ = write!(output, "[{} {}] 0 d ", n(on * POINTS_PER_MM), n(off * POINTS_PER_MM));
    } else {
        output.push_str("[] 0 d ");
    }
    pdf_path(output, path, placement, map);
    output.push_str("S Q\n");
}

fn stroke_line(output: &mut String, a: Point, b: Point, map: PageMap, width_mm: f64) {
    let a = map.global(a);
    let b = map.global(b);
    let _ = writeln!(
        output,
        "q 0 G {} w [] 0 d {} {} m {} {} l S Q",
        n(width_mm * POINTS_PER_MM), n(a.x), n(a.y), n(b.x), n(b.y)
    );
}

fn pdf_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)")
}

fn text(output: &mut String, value: &str, at: Point, size_mm: f64) {
    let _ = writeln!(
        output,
        "BT /F1 {} Tf {} {} Td ({}) Tj ET",
        n(size_mm * POINTS_PER_MM), n(at.x), n(at.y), pdf_escape(value)
    );
}

fn draw_grainline(output: &mut String, a: Point, b: Point, placement: &Placement, map: PageMap) {
    let a = transform_point(a, placement);
    let b = transform_point(b, placement);
    stroke_line(output, a, b, map, 0.4);
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length = dx.hypot(dy).max(1.0);
    let unit = Point::new(dx / length, dy / length);
    let normal = Point::new(-unit.y, unit.x);
    for (tip, direction) in [(a, 1.0), (b, -1.0)] {
        let base = Point::new(tip.x + unit.x * 8.0 * direction, tip.y + unit.y * 8.0 * direction);
        stroke_line(output, tip, Point::new(base.x + normal.x * 3.0, base.y + normal.y * 3.0), map, 0.4);
        stroke_line(output, tip, Point::new(base.x - normal.x * 3.0, base.y - normal.y * 3.0), map, 0.4);
    }
}

fn page_content(
    pattern: &Pattern,
    layout: &Layout,
    page_width: f64,
    page_height: f64,
    row: usize,
    column: usize,
    rows: usize,
    columns: usize,
    printable_width: f64,
    printable_height: f64,
) -> String {
    let map = PageMap {
        window_x: column as f64 * (printable_width - OVERLAP),
        window_y: row as f64 * (printable_height - OVERLAP),
        page_height,
    };
    let mut output = String::new();
    let _ = writeln!(
        output,
        "q {} {} {} {} re W n",
        n(MARGIN * POINTS_PER_MM),
        n(MARGIN * POINTS_PER_MM),
        n(printable_width * POINTS_PER_MM),
        n(printable_height * POINTS_PER_MM)
    );
    for placement in &layout.placements {
        let part = &pattern.parts[placement.part];
        stroke_path(&mut output, &cut_outline(part), placement, map, 0.0, 0.5, None);
        stroke_path(&mut output, &part.outline, placement, map, 0.45, 0.3, Some((4.0, 3.0)));
        for internal in &part.internal {
            stroke_path(&mut output, internal, placement, map, 0.45, 0.3, Some((5.0, 3.0)));
        }
        for notch in &part.notches {
            let tangent = nearest_tangent(&part.outline, *notch);
            let normal = Point::new(-tangent.y, tangent.x);
            let a = transform_point(Point::new(notch.x - normal.x * 3.0, notch.y - normal.y * 3.0), placement);
            let b = transform_point(Point::new(notch.x + normal.x * 3.0, notch.y + normal.y * 3.0), placement);
            stroke_line(&mut output, a, b, map, 0.4);
        }
        draw_grainline(&mut output, part.grainline.0, part.grainline.1, placement, map);
        for label in &part.labels {
            let at = map.point(label.at, placement);
            text(&mut output, &label.text, at, if label.text == part.name { 12.0 } else { 8.0 });
        }
    }
    if row == 0 && column == 0 {
        let lower_left = map.global(Point::new(0.0, 100.0));
        let _ = writeln!(
            output,
            "q 0 G {} w [] 0 d {} {} {} {} re S Q",
            n(0.5 * POINTS_PER_MM), n(lower_left.x), n(lower_left.y), n(100.0 * POINTS_PER_MM), n(100.0 * POINTS_PER_MM)
        );
        text(&mut output, "10 cm", map.global(Point::new(38.0, 53.0)), 8.0);
    }
    output.push_str("Q\n");

    // Crop marks sit just outside each corner of the printable window.
    let left = MARGIN * POINTS_PER_MM;
    let right = (page_width - MARGIN) * POINTS_PER_MM;
    let bottom = MARGIN * POINTS_PER_MM;
    let top = (page_height - MARGIN) * POINTS_PER_MM;
    let mark = 5.0 * POINTS_PER_MM;
    for (x1, y1, x2, y2) in [
        (left - mark, bottom, left + mark, bottom),
        (left, bottom - mark, left, bottom + mark),
        (right - mark, bottom, right + mark, bottom),
        (right, bottom - mark, right, bottom + mark),
        (left - mark, top, left + mark, top),
        (left, top - mark, left, top + mark),
        (right - mark, top, right + mark, top),
        (right, top - mark, right, top + mark),
    ] {
        let _ = writeln!(output, "q 0 G 0.5 w {} {} m {} {} l S Q", n(x1), n(y1), n(x2), n(y2));
    }
    text(&mut output, &format!("r{}c{}", row + 1, column + 1), Point::new(left, 3.0 * POINTS_PER_MM), 3.0);
    text(
        &mut output,
        &format!("{} × {}", columns, rows),
        Point::new(right - 25.0 * POINTS_PER_MM, 3.0 * POINTS_PER_MM),
        3.0,
    );
    output
}

fn pages_for(length: f64, window: f64) -> usize {
    if length <= window {
        1
    } else {
        (((length - window) / (window - OVERLAP)).ceil() as usize) + 1
    }
}

fn append_object(pdf: &mut Vec<u8>, offsets: &mut Vec<usize>, number: usize, body: &[u8]) {
    debug_assert_eq!(number, offsets.len());
    offsets.push(pdf.len());
    pdf.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
    pdf.extend_from_slice(body);
    pdf.extend_from_slice(b"\nendobj\n");
}

pub(crate) fn to_pdf(pattern: &Pattern, layout: &Layout, page: PageSize) -> Vec<u8> {
    let (page_width, page_height) = page_dimensions(page);
    let printable_width = page_width - 2.0 * MARGIN;
    let printable_height = page_height - 2.0 * MARGIN;
    let columns = pages_for(layout.width_mm.max(100.0), printable_width);
    let rows = pages_for(layout.height_mm.max(100.0), printable_height);
    let page_count = columns * rows;
    let font_object = 3 + page_count * 2;

    let mut contents = Vec::with_capacity(page_count);
    for row in 0..rows {
        for column in 0..columns {
            contents.push(page_content(
                pattern,
                layout,
                page_width,
                page_height,
                row,
                column,
                rows,
                columns,
                printable_width,
                printable_height,
            ));
        }
    }

    let mut pdf = b"%PDF-1.4\n%makepad-fabric-draft\n".to_vec();
    let mut offsets = vec![0usize];
    append_object(&mut pdf, &mut offsets, 1, b"<< /Type /Catalog /Pages 2 0 R >>");
    let kids = (0..page_count)
        .map(|index| format!("{} 0 R", 3 + index * 2))
        .collect::<Vec<_>>()
        .join(" ");
    append_object(
        &mut pdf,
        &mut offsets,
        2,
        format!("<< /Type /Pages /Kids [{kids}] /Count {page_count} >>").as_bytes(),
    );
    for (index, content) in contents.iter().enumerate() {
        let page_object = 3 + index * 2;
        let stream_object = page_object + 1;
        append_object(
            &mut pdf,
            &mut offsets,
            page_object,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /Font << /F1 {} 0 R >> >> /Contents {} 0 R >>",
                n(page_width * POINTS_PER_MM), n(page_height * POINTS_PER_MM), font_object, stream_object
            )
            .as_bytes(),
        );
        let mut stream = format!("<< /Length {} >>\nstream\n", content.as_bytes().len()).into_bytes();
        stream.extend_from_slice(content.as_bytes());
        stream.extend_from_slice(b"endstream");
        append_object(&mut pdf, &mut offsets, stream_object, &stream);
    }
    append_object(
        &mut pdf,
        &mut offsets,
        font_object,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    );

    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", offsets.len()).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            offsets.len()
        )
        .as_bytes(),
    );
    pdf
}
