//! Units. The scene is always meters internally; `Units` remembers the source
//! scale and the unit the user wants to *see*, and formats values for the UI.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LengthUnit {
    Millimeter,
    Centimeter,
    #[default]
    Meter,
    Inch,
    Foot,
}

impl LengthUnit {
    /// Meters per one of this unit.
    pub fn meters(self) -> f64 {
        match self {
            LengthUnit::Millimeter => 0.001,
            LengthUnit::Centimeter => 0.01,
            LengthUnit::Meter => 1.0,
            LengthUnit::Inch => 0.0254,
            LengthUnit::Foot => 0.3048,
        }
    }

    pub fn suffix(self) -> &'static str {
        match self {
            LengthUnit::Millimeter => "mm",
            LengthUnit::Centimeter => "cm",
            LengthUnit::Meter => "m",
            LengthUnit::Inch => "in",
            LengthUnit::Foot => "ft",
        }
    }

    pub fn all() -> &'static [LengthUnit] {
        &[
            LengthUnit::Millimeter,
            LengthUnit::Centimeter,
            LengthUnit::Meter,
            LengthUnit::Inch,
            LengthUnit::Foot,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Units {
    /// Multiply a raw parser coordinate by this to get metres.
    ///
    /// **Fab is metres, so this is `1.0`.** Other sources declare their own factor;
    /// millimetre formats use `0.001`.
    pub source_to_meters: f32,
    /// Unit used for display in the UI. Changing it never touches geometry.
    pub display: LengthUnit,
    /// Decimal places for lengths in the display unit.
    pub precision: u8,
}

impl Default for Units {
    fn default() -> Self {
        Units {
            source_to_meters: 1.0,
            display: LengthUnit::Meter,
            precision: 3,
        }
    }
}

impl Units {
    pub fn millimeters() -> Self {
        Units {
            source_to_meters: 0.001,
            display: LengthUnit::Millimeter,
            precision: 0,
        }
    }

    /// Format a length given in meters, e.g. `"3.250 m"` or `"3250 mm"`.
    pub fn format_length(&self, meters: f64) -> String {
        let v = meters / self.display.meters();
        format!("{:.*} {}", self.precision as usize, v, self.display.suffix())
    }

    /// Format an area given in square meters in the display unit squared.
    pub fn format_area(&self, sqm: f64) -> String {
        let m = self.display.meters();
        let v = sqm / (m * m);
        format!("{:.*} {}²", self.precision as usize, v, self.display.suffix())
    }

    /// Format a volume given in cubic meters in the display unit cubed.
    pub fn format_volume(&self, cbm: f64) -> String {
        let m = self.display.meters();
        let v = cbm / (m * m * m);
        format!("{:.*} {}³", self.precision as usize, v, self.display.suffix())
    }

    /// Format an angle given in degrees.
    pub fn format_angle(&self, degrees: f64) -> String {
        format!("{:.2}°", degrees)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_in_display_unit() {
        let u = Units::millimeters();
        assert_eq!(u.format_length(3.25), "3250 mm");
        let u = Units::default();
        assert_eq!(u.format_length(3.25), "3.250 m");
        assert_eq!(u.format_area(2.0), "2.000 m²");
    }
}
