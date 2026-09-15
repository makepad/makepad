use super::{append_only_segment, labels, line_path, measurement};
use crate::{curve_through, Design, DraftError, Measurements, OptionSpec, Options, Part, Path, Pattern, Point};

pub(crate) struct AlineSkirt;

impl AlineSkirt {
    fn specs(default_length: f64) -> Vec<OptionSpec> {
        vec![
            OptionSpec { key: "waist_ease", label: "Waist ease", min: 0.0, max: 40.0, default: 10.0, unit: "mm" },
            OptionSpec { key: "hip_ease", label: "Hip ease", min: 0.0, max: 80.0, default: 30.0, unit: "mm" },
            OptionSpec { key: "flare", label: "Flare", min: 0.0, max: 250.0, default: 80.0, unit: "mm" },
            OptionSpec { key: "length", label: "Length", min: 300.0, max: 1000.0, default: default_length.clamp(300.0, 1000.0), unit: "mm" },
            OptionSpec { key: "dart_length", label: "Front dart length", min: 50.0, max: 200.0, default: 90.0, unit: "mm" },
        ]
    }
}

fn option(options: &Options, spec: &OptionSpec) -> Result<f64, DraftError> {
    let value = options.get(spec);
    if !value.is_finite() || value < spec.min || value > spec.max {
        Err(DraftError::Invalid(format!("option {} must be between {} and {}", spec.key, spec.min, spec.max)))
    } else {
        Ok(value)
    }
}

fn waist_point(side_x: f64, x: f64) -> Point {
    let t = (x / side_x).clamp(0.0, 1.0);
    let y = 10.0 * ((1.0 - t).powi(3) + 3.0 * (1.0 - t).powi(2) * t);
    Point::new(x, y)
}

#[allow(clippy::too_many_arguments)]
fn skirt_piece(
    name: &str,
    design: &str,
    on_fold: bool,
    cut_count: u8,
    waist_quarter: f64,
    hip_quarter: f64,
    hip_line: f64,
    length: f64,
    flare: f64,
    dart_length: f64,
    keys: [(&str, f64); 3],
) -> Part {
    let dart_intake = (hip_quarter - waist_quarter).max(0.0) * 0.6;
    let side_waist = Point::new(waist_quarter + dart_intake, 0.0);
    let hip = Point::new(hip_quarter, hip_line);
    let side_hem = Point::new(hip_quarter + flare, length);
    let center_waist = Point::new(0.0, 10.0);
    let center_hem = Point::new(0.0, length);
    let mut outline = Path { start: center_waist, ..Path::default() };
    outline.curve_to(
        Point::new(side_waist.x / 3.0, 10.0),
        Point::new(side_waist.x * 2.0 / 3.0, 0.0),
        side_waist,
    );
    append_only_segment(&mut outline, curve_through(side_waist, Point::new(0.12, 1.0), hip, Point::new(0.0, 1.0), 0.30));
    outline.line_to(side_hem);
    let side_vector = Point::new(side_hem.x - hip.x, side_hem.y - hip.y);
    append_only_segment(
        &mut outline,
        curve_through(side_hem, Point::new(-side_vector.y, side_vector.x), center_hem, Point::new(-1.0, 0.0), 0.18),
    );
    outline.close();

    let dart_center = waist_quarter * 0.5;
    let left = waist_point(side_waist.x, dart_center - dart_intake * 0.5);
    let right = waist_point(side_waist.x, dart_center + dart_intake * 0.5);
    let tip = Point::new(dart_center, waist_point(side_waist.x, dart_center).y + dart_length);
    let mut dart = Path { start: left, ..Path::default() };
    dart.line_to(tip);
    dart.line_to(right);
    let mut notches = vec![left, right];
    if !on_fold {
        notches.push(Point::new(0.0, center_waist.y + 180.0));
    }
    Part {
        name: name.to_owned(),
        cut_count,
        on_fold,
        outline,
        seam_allowance_mm: 10.0,
        notches,
        grainline: (Point::new(waist_quarter * 0.28, hip_line + 40.0), Point::new(waist_quarter * 0.28, length - 70.0)),
        internal: vec![dart, line_path(Point::new(0.0, length - 25.0), Point::new(side_hem.x, length - 25.0))],
        labels: labels(design, name, cut_count, on_fold, Point::new(waist_quarter * 0.18, length * 0.44), keys),
    }
}

impl Design for AlineSkirt {
    fn id(&self) -> &'static str { "aline_skirt" }
    fn name(&self) -> &'static str { "A-line skirt" }
    fn options(&self) -> Vec<OptionSpec> { Self::specs(630.0) }

    fn draft(&self, m: &Measurements, options: &Options) -> Result<Pattern, DraftError> {
        let waist = measurement(m.waist, "waist")?;
        let hip = measurement(m.hip, "hip")?;
        let waist_to_hip = measurement(m.waist_to_hip, "waist_to_hip")?;
        let waist_to_knee = measurement(m.waist_to_knee, "waist_to_knee")?;

        let specs = Self::specs(waist_to_knee + 50.0);
        let waist_ease = option(options, &specs[0])?;
        let hip_ease = option(options, &specs[1])?;
        let flare = option(options, &specs[2])?;
        let length = option(options, &specs[3])?;
        let front_dart_length = option(options, &specs[4])?;
        let back_dart_length = (front_dart_length + 30.0).min(200.0);
        let waist_quarter = (waist + waist_ease) / 4.0;
        let hip_quarter = (hip + hip_ease) / 4.0;
        let keys = [("waist", waist), ("hip", hip), ("waist-to-hip", waist_to_hip)];

        let front = skirt_piece(
            "Front", self.name(), true, 1, waist_quarter, hip_quarter, waist_to_hip, length, flare, front_dart_length, keys,
        );
        let back = skirt_piece(
            "Back", self.name(), false, 2, waist_quarter, hip_quarter, waist_to_hip, length, flare, back_dart_length, keys,
        );
        let band_length = waist + waist_ease + 30.0;
        let mut band_outline = Path { start: Point::new(0.0, 0.0), ..Path::default() };
        band_outline.line_to(Point::new(80.0, 0.0));
        band_outline.line_to(Point::new(80.0, band_length));
        band_outline.line_to(Point::new(0.0, band_length));
        band_outline.close();
        let waistband = Part {
            name: "Waistband".to_owned(),
            cut_count: 1,
            on_fold: false,
            outline: band_outline,
            seam_allowance_mm: 10.0,
            notches: vec![
                Point::new(0.0, 15.0),
                Point::new(0.0, 15.0 + waist_quarter),
                Point::new(0.0, 15.0 + 3.0 * waist_quarter),
            ],
            grainline: (Point::new(40.0, 25.0), Point::new(40.0, band_length - 25.0)),
            internal: vec![line_path(Point::new(40.0, 0.0), Point::new(40.0, band_length))],
            labels: labels(self.name(), "Waistband", 1, false, Point::new(12.0, band_length * 0.38), keys),
        };

        Ok(Pattern {
            design_id: self.id().to_owned(),
            design_name: self.name().to_owned(),
            parts: vec![front, back, waistband],
            measurements_used: vec!["waist", "hip", "waist_to_hip", "waist_to_knee"],
        })
    }
}
