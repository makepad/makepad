//! Parametric sewing patterns from measurements.
//!
//! Geometry is in MILLIMETRES, y down (SVG convention). A `Design` turns a
//! `Measurements` set plus its options into a `Pattern` of `Part`s; the
//! output side nests parts onto fabric, writes SVG at true scale, and tiles
//! a PDF for home printing.

use std::collections::BTreeMap;
use std::fmt;

mod designs;
mod geom;
mod nest;
mod pdf;
mod svg;

pub use makepad_fabric_measure::Measurements;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }
}

/// One path segment after the current point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Segment {
    Line { to: Point },
    /// Cubic Bézier.
    Curve { c1: Point, c2: Point, to: Point },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    pub start: Point,
    pub segments: Vec<Segment>,
    pub closed: bool,
}

impl Path {
    pub fn line_to(&mut self, to: Point) -> &mut Self {
        self.segments.push(Segment::Line { to });
        self
    }

    pub fn curve_to(&mut self, c1: Point, c2: Point, to: Point) -> &mut Self {
        self.segments.push(Segment::Curve { c1, c2, to });
        self
    }

    pub fn close(&mut self) -> &mut Self {
        self.closed = true;
        self
    }

    pub fn length(&self) -> f64 {
        geom::path_length(self)
    }

    pub fn point_at(&self, t: f64) -> Point {
        geom::point_at(self, t)
    }

    pub fn reverse(&self) -> Path {
        geom::reverse(self)
    }

    pub fn translate(&self, dx: f64, dy: f64) -> Path {
        geom::translate(self, dx, dy)
    }

    pub fn mirror_x(&self, axis_x: f64) -> Path {
        geom::mirror_x(self, axis_x)
    }

    /// `(minimum, maximum)` axis-aligned bounds, including cubic extrema.
    pub fn bounds(&self) -> (Point, Point) {
        geom::bounds(self)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub at: Point,
    pub text: String,
    /// Degrees, counter-clockwise, 0 = reading left to right.
    pub angle_deg: f64,
}

/// One pattern piece. `outline` is the SEAM line; the cut line is the
/// outline offset outward by `seam_allowance_mm` (0 on a fold edge).
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub name: String,
    /// "cut 2", "cut 1 on fold"…: how many to cut from fabric.
    pub cut_count: u8,
    pub on_fold: bool,
    pub outline: Path,
    pub seam_allowance_mm: f64,
    /// Points on the outline where notches go.
    pub notches: Vec<Point>,
    /// Grain arrow, from → to.
    pub grainline: (Point, Point),
    /// Darts, fold lines, bust point marks.
    pub internal: Vec<Path>,
    pub labels: Vec<Label>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub design_id: String,
    pub design_name: String,
    pub parts: Vec<Part>,
    /// Which measurement keys the draft read.
    pub measurements_used: Vec<&'static str>,
}

/// A numeric design option (ease, length…), millimetres unless stated.
#[derive(Clone, Debug, PartialEq)]
pub struct OptionSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub unit: &'static str,
}

/// Chosen option values by key; missing keys take the spec default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options(pub BTreeMap<String, f64>);

impl Options {
    pub fn get(&self, spec: &OptionSpec) -> f64 {
        self.0.get(spec.key).copied().unwrap_or(spec.default)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DraftError {
    MissingMeasurement(&'static str),
    Invalid(String),
    NotImplemented,
}

impl fmt::Display for DraftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DraftError::MissingMeasurement(key) => write!(f, "measurement {key} is missing or zero"),
            DraftError::Invalid(why) => write!(f, "cannot draft: {why}"),
            DraftError::NotImplemented => write!(f, "this design is not drafted yet"),
        }
    }
}

impl std::error::Error for DraftError {}

pub trait Design {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn options(&self) -> Vec<OptionSpec>;
    fn draft(&self, m: &Measurements, options: &Options) -> Result<Pattern, DraftError>;
}

/// Every design this crate can draft, in menu order.
pub fn designs() -> Vec<Box<dyn Design>> {
    designs::all()
}

/// Flatten a path to a polyline within `tolerance_mm`.
pub fn flatten(path: &Path, tolerance_mm: f64) -> Vec<Point> {
    geom::flatten(path, tolerance_mm)
}

/// Offset a closed path outward (positive) or inward (negative).
pub fn offset(path: &Path, distance_mm: f64) -> Path {
    geom::offset(path, distance_mm)
}

pub use geom::{curve_through, seam_length};

/// Check geometric closure, intersections, seam matching, and notch placement.
pub fn validate(pattern: &Pattern) -> Vec<String> {
    designs::validate(pattern)
}

/// Where each part sits on the fabric.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub part: usize,
    pub offset: Point,
    /// 0 or 180 for grain-true placements; 90/270 only for parts whose
    /// grainline allows it.
    pub rotation_deg: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub width_mm: f64,
    pub height_mm: f64,
    pub placements: Vec<Placement>,
}

/// Nest the parts onto fabric of the given width. Height is whatever it
/// takes; the layout reports it.
pub fn nest(pattern: &Pattern, fabric_width_mm: f64) -> Layout {
    nest::nest(pattern, fabric_width_mm)
}

/// True-scale SVG (mm user units) of the laid-out pattern: cut lines,
/// seam lines, notches, grainlines, labels, and a 100 mm test square.
pub fn to_svg(pattern: &Pattern, layout: &Layout) -> String {
    svg::to_svg(pattern, layout)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PageSize {
    A4,
    Letter,
    A0,
}

/// A PDF whose pages tile the layout at 100 % with crop marks, page ids
/// and overlap guides, plus the test square on page one.
pub fn to_pdf(pattern: &Pattern, layout: &Layout, page: PageSize) -> Vec<u8> {
    pdf::to_pdf(pattern, layout, page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn output_dir() -> PathBuf {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/fabric_draft");
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn sample_tshirt() -> Pattern {
        designs()
            .into_iter()
            .find(|design| design.id() == "tshirt")
            .unwrap()
            .draft(&Measurements::sample(), &Options::default())
            .unwrap()
    }

    #[test]
    fn default_designs_are_valid() {
        let expected = [("tshirt", 4), ("aline_skirt", 3), ("easy_trousers", 2)];
        for (design, (id, count)) in designs().into_iter().zip(expected) {
            assert_eq!(design.id(), id);
            let pattern = design.draft(&Measurements::sample(), &Options::default()).unwrap();
            assert_eq!(pattern.parts.len(), count);
            let messages = validate(&pattern);
            assert!(messages.is_empty(), "{}: {messages:#?}", design.id());
            crate::designs::print_seam_metrics(&pattern);
        }
    }

    #[test]
    fn missing_measurements_are_reported() {
        for design in designs() {
            let mut measurements = Measurements::sample();
            let key = design
                .draft(&measurements, &Options::default())
                .unwrap()
                .measurements_used[0];
            measurements.set(key, f32::NAN);
            assert_eq!(design.draft(&measurements, &Options::default()), Err(DraftError::MissingMeasurement(key)));
        }
    }

    #[test]
    fn svg_golden_is_complete() {
        let pattern = sample_tshirt();
        let layout = nest(&pattern, 1200.0);
        let svg = to_svg(&pattern, &layout);
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.ends_with("</svg>\n"));
        assert_eq!(svg.matches("<g ").count(), pattern.parts.len() + 1);
        assert!(svg.contains("id=\"test-square\""));
        for tag in svg.match_indices("<path") {
            let end = svg[tag.0..].find("/>").unwrap() + tag.0;
            assert!(svg[tag.0..end].contains(" Z\""));
        }
        let path = output_dir().join("tshirt.svg");
        fs::write(&path, svg).unwrap();
        println!("{}", path.display());
    }

    #[test]
    fn tiled_pdf_has_exact_xref() {
        let pattern = sample_tshirt();
        let layout = nest(&pattern, 1200.0);
        let pdf = to_pdf(&pattern, &layout, PageSize::A4);
        assert!(pdf.starts_with(b"%PDF-1.4"));
        let text = String::from_utf8(pdf.clone()).unwrap();
        let page_objects = text.matches("/Type /Page ").count();
        assert!(page_objects > 1);
        let startxref = text.rsplit_once("startxref\n").unwrap().1.lines().next().unwrap().parse::<usize>().unwrap();
        assert_eq!(&pdf[startxref..startxref + 4], b"xref");
        let xref = &text[startxref..];
        let mut lines = xref.lines();
        assert_eq!(lines.next(), Some("xref"));
        let header = lines.next().unwrap().split_whitespace().collect::<Vec<_>>();
        assert_eq!(header[0], "0");
        let object_count = header[1].parse::<usize>().unwrap();
        lines.next();
        for object in 1..object_count {
            let offset = lines.next().unwrap()[..10].parse::<usize>().unwrap();
            let marker = format!("{object} 0 obj");
            assert_eq!(&pdf[offset..offset + marker.len()], marker.as_bytes());
        }
        let path = output_dir().join("tshirt-a4.pdf");
        fs::write(&path, pdf).unwrap();
        println!("PDF pages: {page_objects}; {}", path.display());
    }
}
