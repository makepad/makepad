use super::{append_only_segment, labels, line_path, measurement};
use crate::{curve_through, Design, DraftError, Measurements, OptionSpec, Options, Part, Path, Pattern, Point};

pub(crate) struct Tshirt;

impl Tshirt {
    fn specs() -> Vec<OptionSpec> {
        vec![
            OptionSpec { key: "bust_ease", label: "Bust ease", min: 20.0, max: 300.0, default: 100.0, unit: "mm" },
            OptionSpec { key: "length_below_waist", label: "Length below waist", min: 0.0, max: 400.0, default: 220.0, unit: "mm" },
            OptionSpec { key: "sleeve_length", label: "Sleeve length", min: 100.0, max: 650.0, default: 220.0, unit: "mm" },
            OptionSpec { key: "shoulder_slope", label: "Shoulder slope", min: 20.0, max: 60.0, default: 40.0, unit: "mm" },
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

fn body(
    name: &str,
    design_name: &str,
    width: f64,
    body_length: f64,
    neck_width: f64,
    neck_depth: f64,
    shoulder: Point,
    underarm: Point,
    measurements: [(&str, f64); 3],
) -> (Part, f64, f64) {
    let center_neck = Point::new(0.0, neck_depth);
    let neck_shoulder = Point::new(neck_width, 0.0);
    let neckline = curve_through(center_neck, Point::new(1.0, 0.0), neck_shoulder, Point::new(0.0, -1.0), 0.52);
    let armhole = curve_through(shoulder, Point::new(-0.15, 1.0), underarm, Point::new(-1.0, 0.0), 0.35);
    let neckline_length = neckline.length();
    let armhole_length = armhole.length();

    let mut outline = neckline;
    outline.line_to(shoulder);
    append_only_segment(&mut outline, armhole);
    outline.line_to(Point::new(width, body_length));
    outline.line_to(Point::new(0.0, body_length));
    outline.close();

    let label_at = Point::new(width * 0.22, body_length * 0.45);
    let part = Part {
        name: name.to_owned(),
        cut_count: 1,
        on_fold: true,
        outline,
        seam_allowance_mm: 10.0,
        notches: vec![shoulder],
        grainline: (Point::new(width * 0.55, body_length * 0.28), Point::new(width * 0.55, body_length * 0.75)),
        internal: vec![line_path(Point::new(0.0, body_length - 25.0), Point::new(width, body_length - 25.0))],
        labels: labels(design_name, name, 1, true, label_at, measurements),
    };
    (part, neckline_length, armhole_length)
}

fn cap_halves(half_width: f64, cap_height: f64) -> (Path, Path) {
    let top = Point::new(0.0, 0.0);
    let left = Point::new(-half_width, cap_height);
    let left_half = curve_through(left, Point::new(1.0, -0.1), top, Point::new(0.7, -1.0), 0.38);
    let right_half = left_half.reverse().mirror_x(0.0);
    (left_half, right_half)
}

pub(crate) fn sleeve_half_width(target_cap_length: f64, cap_height: f64) -> f64 {
    let cap_length = |half_width: f64| {
        let (left, right) = cap_halves(half_width, cap_height);
        left.length() + right.length()
    };
    let mut low = 0.01;
    let mut high = target_cap_length.max(cap_height * 2.0);
    while cap_length(high) < target_cap_length {
        high *= 2.0;
    }
    for _ in 0..80 {
        let mid = (low + high) * 0.5;
        if cap_length(mid) < target_cap_length {
            low = mid;
        } else {
            high = mid;
        }
    }
    (low + high) * 0.5
}

impl Design for Tshirt {
    fn id(&self) -> &'static str { "tshirt" }
    fn name(&self) -> &'static str { "T-shirt" }
    fn options(&self) -> Vec<OptionSpec> { Self::specs() }

    fn draft(&self, m: &Measurements, options: &Options) -> Result<Pattern, DraftError> {
        let bust = measurement(m.bust, "bust")?;
        let neck = measurement(m.neck, "neck")?;
        let shoulder_width = measurement(m.shoulder_width, "shoulder_width")?;
        let back_waist_length = measurement(m.back_waist_length, "back_waist_length")?;
        let bicep = measurement(m.bicep, "bicep")?;

        let specs = Self::specs();
        let bust_ease = option(options, &specs[0])?;
        let length_below_waist = option(options, &specs[1])?;
        let sleeve_length = option(options, &specs[2])?;
        let shoulder_slope = option(options, &specs[3])?;
        let width = (bust + bust_ease) / 4.0;
        let body_length = back_waist_length + length_below_waist;
        let neck_width = neck / 5.0;
        let shoulder = Point::new(shoulder_width / 2.0 + 10.0, shoulder_slope);
        let armhole_depth = bust / 8.0 + 60.0;
        let underarm = Point::new(width, shoulder_slope + armhole_depth);
        let keys = [("bust", bust), ("neck", neck), ("shoulder", shoulder_width)];

        let (front, front_neck, front_armhole) = body(
            "Front", self.name(), width, body_length, neck_width, neck / 6.0 + 15.0, shoulder, underarm, keys,
        );
        let (back, back_neck, back_armhole) = body(
            "Back", self.name(), width, body_length, neck_width, 20.0, shoulder, underarm, keys,
        );

        let target_cap = front_armhole + back_armhole + 10.0;
        let cap_height = 0.62 * armhole_depth;
        let half_width = sleeve_half_width(target_cap, cap_height);
        let (left_cap, right_cap) = cap_halves(half_width, cap_height);
        let back_notch_t = (left_cap.length() / 3.0) / left_cap.length();
        let notch_delta = (2.0 / left_cap.length()).min(0.02);
        let shoulder_notch = Point::new(0.0, 0.0);
        let front_notch = right_cap.point_at(1.0 / 3.0);
        let back_notch_1 = left_cap.point_at((back_notch_t - notch_delta).max(0.0));
        let back_notch_2 = left_cap.point_at((back_notch_t + notch_delta).min(1.0));
        let hem_half_width = (bicep + 40.0) * 0.5 * 0.92;
        let mut sleeve_outline = left_cap;
        append_only_segment(&mut sleeve_outline, right_cap);
        sleeve_outline.line_to(Point::new(hem_half_width, sleeve_length));
        sleeve_outline.line_to(Point::new(-hem_half_width, sleeve_length));
        sleeve_outline.close();
        let sleeve = Part {
            name: "Sleeve".to_owned(),
            cut_count: 2,
            on_fold: false,
            outline: sleeve_outline,
            seam_allowance_mm: 10.0,
            notches: vec![shoulder_notch, front_notch, back_notch_1, back_notch_2],
            grainline: (Point::new(0.0, 35.0), Point::new(0.0, sleeve_length - 35.0)),
            internal: vec![line_path(Point::new(-hem_half_width, sleeve_length - 25.0), Point::new(hem_half_width, sleeve_length - 25.0))],
            labels: labels(self.name(), "Sleeve", 2, false, Point::new(-hem_half_width * 0.45, sleeve_length * 0.5), keys),
        };

        let band_length = 0.85 * 2.0 * (front_neck + back_neck);
        let mut band_outline = Path { start: Point::new(0.0, 0.0), ..Path::default() };
        band_outline.line_to(Point::new(40.0, 0.0));
        band_outline.line_to(Point::new(40.0, band_length));
        band_outline.line_to(Point::new(0.0, band_length));
        band_outline.close();
        let mut band_labels = labels(self.name(), "Neckband", 1, false, Point::new(8.0, band_length * 0.35), keys);
        band_labels.push(crate::Label { at: Point::new(8.0, band_length * 0.62), text: "stretch to fit".to_owned(), angle_deg: 90.0 });
        let neckband = Part {
            name: "Neckband".to_owned(),
            cut_count: 1,
            on_fold: false,
            outline: band_outline,
            seam_allowance_mm: 10.0,
            notches: Vec::new(),
            grainline: (Point::new(20.0, 20.0), Point::new(20.0, band_length - 20.0)),
            internal: vec![line_path(Point::new(20.0, 0.0), Point::new(20.0, band_length))],
            labels: band_labels,
        };

        Ok(Pattern {
            design_id: self.id().to_owned(),
            design_name: self.name().to_owned(),
            parts: vec![front, back, sleeve, neckband],
            measurements_used: vec!["bust", "neck", "shoulder_width", "back_waist_length", "bicep"],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleeve_bisection_is_monotone_and_accurate() {
        let target = 430.0;
        let height = 112.0;
        let width = sleeve_half_width(target, height);
        let (left, right) = cap_halves(width, height);
        assert!((left.length() + right.length() - target).abs() < 0.5);
        let (small_left, small_right) = cap_halves(width - 5.0, height);
        let (large_left, large_right) = cap_halves(width + 5.0, height);
        assert!(small_left.length() + small_right.length() < target);
        assert!(large_left.length() + large_right.length() > target);
    }
}
