use super::{append_only_segment, labels, line_path, measurement};
use crate::{curve_through, Design, DraftError, Measurements, OptionSpec, Options, Part, Path, Pattern, Point};

pub(crate) struct EasyTrousers;

impl EasyTrousers {
    fn specs(default_length: f64, default_hem: f64) -> Vec<OptionSpec> {
        vec![
            OptionSpec { key: "hip_ease", label: "Hip ease", min: 20.0, max: 160.0, default: 60.0, unit: "mm" },
            OptionSpec { key: "length", label: "Length", min: 500.0, max: 1200.0, default: default_length.clamp(500.0, 1200.0), unit: "mm" },
            OptionSpec { key: "hem_width", label: "Hem width", min: 250.0, max: 600.0, default: default_hem.clamp(250.0, 600.0), unit: "mm" },
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

fn interpolate_y(a: Point, b: Point, y: f64) -> Point {
    let t = if (b.y - a.y).abs() < 1.0e-9 { 0.0 } else { (y - a.y) / (b.y - a.y) };
    Point::new(a.x + (b.x - a.x) * t, y)
}

#[allow(clippy::too_many_arguments)]
fn trouser_piece(
    name: &str,
    design: &str,
    is_back: bool,
    quarter_hip: f64,
    hip_line: f64,
    crotch_line: f64,
    knee_line: f64,
    length: f64,
    extension: f64,
    hem_piece_width: f64,
    outer_hem_x: f64,
    keys: [(&str, f64); 3],
) -> Part {
    let center_waist = Point::new(0.0, if is_back { -25.0 } else { 0.0 });
    let side_waist = Point::new(quarter_hip + 15.0, 0.0);
    let hip_side = Point::new(quarter_hip, hip_line);
    let outer_hem = Point::new(outer_hem_x, length);
    let inner_hem = Point::new(outer_hem_x - hem_piece_width, length);
    let crotch_tip = Point::new(-extension, crotch_line);
    let curve_start_y = if is_back { crotch_line * 0.5 } else { crotch_line * (2.0 / 3.0) };
    let curve_start = Point::new(0.0, curve_start_y);

    let mut outline = Path { start: center_waist, ..Path::default() };
    outline.line_to(side_waist);
    append_only_segment(&mut outline, curve_through(side_waist, Point::new(-0.06, 1.0), hip_side, Point::new(0.0, 1.0), 0.22));
    outline.line_to(outer_hem);
    outline.line_to(inner_hem);
    outline.line_to(crotch_tip);
    append_only_segment(&mut outline, curve_through(crotch_tip, Point::new(1.0, 0.0), curve_start, Point::new(0.0, -1.0), 0.42));
    outline.line_to(center_waist);
    outline.close();

    let outer_knee = interpolate_y(hip_side, outer_hem, knee_line);
    let inner_knee = interpolate_y(crotch_tip, inner_hem, knee_line);
    let casing_center = Point::new(0.0, center_waist.y + 35.0);
    let casing_side = Point::new(side_waist.x, side_waist.y + 35.0);
    Part {
        name: name.to_owned(),
        cut_count: 2,
        on_fold: false,
        outline,
        seam_allowance_mm: 10.0,
        notches: vec![outer_knee, inner_knee],
        grainline: (Point::new((outer_knee.x + inner_knee.x) * 0.5, hip_line + 60.0), Point::new((outer_knee.x + inner_knee.x) * 0.5, length - 70.0)),
        internal: vec![line_path(casing_center, casing_side)],
        labels: labels(design, name, 2, false, Point::new(-extension * 0.25, length * 0.43), keys),
    }
}

impl Design for EasyTrousers {
    fn id(&self) -> &'static str { "easy_trousers" }
    fn name(&self) -> &'static str { "Easy trousers" }
    fn options(&self) -> Vec<OptionSpec> { Self::specs(1020.0, 450.0) }

    fn draft(&self, m: &Measurements, options: &Options) -> Result<Pattern, DraftError> {
        let hip = measurement(m.hip, "hip")?;
        let crotch_depth = measurement(m.crotch_depth, "crotch_depth")?;
        let outseam = measurement(m.outseam, "outseam")?;
        let knee = measurement(m.knee, "knee")?;
        let waist_to_hip = measurement(m.waist_to_hip, "waist_to_hip")?;
        let waist_to_knee = measurement(m.waist_to_knee, "waist_to_knee")?;

        let specs = Self::specs(outseam - 20.0, knee + 60.0);
        let hip_ease = option(options, &specs[0])?;
        let length = option(options, &specs[1])?;
        let hem_width = option(options, &specs[2])?;
        let quarter_hip = (hip + hip_ease) / 4.0;
        let crotch_line = crotch_depth + 20.0;
        let front_extension = hip / 16.0 - 5.0;
        let back_extension = hip / 8.0 + 10.0;
        let front_hem = hem_width * 0.5 * 0.47;
        let back_hem = hem_width * 0.5 * 0.53;
        // This center makes the straight front and back inseams exactly equal while
        // retaining the requested 47/53 hem split and a shared outseam endpoint.
        let outer_hem_x = (front_hem + back_hem - front_extension - back_extension) * 0.5;
        let keys = [("hip", hip), ("crotch depth", crotch_depth), ("knee", knee)];

        let front = trouser_piece(
            "Front", self.name(), false, quarter_hip, waist_to_hip, crotch_line, waist_to_knee,
            length, front_extension, front_hem, outer_hem_x, keys,
        );
        let back = trouser_piece(
            "Back", self.name(), true, quarter_hip, waist_to_hip, crotch_line, waist_to_knee,
            length, back_extension, back_hem, outer_hem_x, keys,
        );
        Ok(Pattern {
            design_id: self.id().to_owned(),
            design_name: self.name().to_owned(),
            parts: vec![front, back],
            measurements_used: vec!["hip", "crotch_depth", "outseam", "knee", "waist_to_hip", "waist_to_knee"],
        })
    }
}
