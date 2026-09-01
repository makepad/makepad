//! Score-space units and coordinate conventions.
//!
//! Score geometry uses a Cartesian coordinate system whose origin is on the
//! middle line of a conventional five-line staff. Positive x points right and
//! positive y points up. This matches SMuFL font-metadata coordinates. UI
//! layout coordinates normally point down; [`StaffPoint::to_layout_point`]
//! performs that single, explicit y-axis inversion.
//!
//! A staff space is the distance between adjacent staff lines. Consequently,
//! the height from the bottom line to the top line of a five-line staff is four
//! staff spaces. Pitch positions are represented by [`StaffStep`]: one staff
//! step is half a staff space, and positive steps move upward from a caller-
//! selected reference line.

use std::num::NonZeroU16;
use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

/// The number of staff spaces represented by a SMuFL font em square.
pub const STAFF_SPACES_PER_EM: f64 = 4.0;

/// A distance or coordinate expressed in staff spaces.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct StaffSpaces(f64);

impl StaffSpaces {
    pub const ZERO: Self = Self(0.0);

    pub const fn new(value: f64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> f64 {
        self.0
    }

    /// Converts font design units using the SMuFL scale of four staff spaces
    /// per em.
    pub fn from_design_units(value: DesignUnits, metrics: FontMetrics) -> Self {
        Self(value.get() * STAFF_SPACES_PER_EM / f64::from(metrics.units_per_em()))
    }

    /// Converts this value to font design units using four staff spaces per em.
    pub fn to_design_units(self, metrics: FontMetrics) -> DesignUnits {
        DesignUnits::new(self.0 * f64::from(metrics.units_per_em()) / STAFF_SPACES_PER_EM)
    }

    /// Converts a distance to layout points for a five-line staff of the given
    /// total height. This scalar conversion does not change sign.
    pub fn to_layout_points(self, staff_size: StaffSize) -> f64 {
        self.0 * staff_size.points_per_space()
    }

    /// Converts a layout-point distance to staff spaces.
    pub fn from_layout_points(value: f64, staff_size: StaffSize) -> Self {
        Self(value / staff_size.points_per_space())
    }
}

impl From<f64> for StaffSpaces {
    fn from(value: f64) -> Self {
        Self::new(value)
    }
}

impl From<StaffSpaces> for f64 {
    fn from(value: StaffSpaces) -> Self {
        value.get()
    }
}

impl Add for StaffSpaces {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl AddAssign for StaffSpaces {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl Sub for StaffSpaces {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl SubAssign for StaffSpaces {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

impl Neg for StaffSpaces {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self(-self.0)
    }
}

impl Mul<f64> for StaffSpaces {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self::Output {
        Self(self.0 * rhs)
    }
}

impl Mul<StaffSpaces> for f64 {
    type Output = StaffSpaces;

    fn mul(self, rhs: StaffSpaces) -> Self::Output {
        rhs * self
    }
}

impl MulAssign<f64> for StaffSpaces {
    fn mul_assign(&mut self, rhs: f64) {
        self.0 *= rhs;
    }
}

impl Div<f64> for StaffSpaces {
    type Output = Self;

    fn div(self, rhs: f64) -> Self::Output {
        Self(self.0 / rhs)
    }
}

impl DivAssign<f64> for StaffSpaces {
    fn div_assign(&mut self, rhs: f64) {
        self.0 /= rhs;
    }
}

impl Div<StaffSpaces> for StaffSpaces {
    type Output = f64;

    fn div(self, rhs: StaffSpaces) -> Self::Output {
        self.0 / rhs.0
    }
}

/// A coordinate or distance in a font's native design-unit grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct DesignUnits(f64);

impl DesignUnits {
    pub const fn new(value: f64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

/// The scale information required to convert a font's design units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontMetrics {
    units_per_em: NonZeroU16,
}

impl FontMetrics {
    pub const fn new(units_per_em: NonZeroU16) -> Self {
        Self { units_per_em }
    }

    /// Returns `None` for an invalid zero-sized em square.
    pub const fn from_units_per_em(units_per_em: u16) -> Option<Self> {
        match NonZeroU16::new(units_per_em) {
            Some(units_per_em) => Some(Self::new(units_per_em)),
            None => None,
        }
    }

    pub const fn units_per_em(self) -> u16 {
        self.units_per_em.get()
    }
}

/// The height, in layout points, from the bottom staff line to the top staff
/// line of a conventional five-line staff.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct StaffSize(f64);

impl StaffSize {
    /// Constructs a staff size when `height_points` is finite and positive.
    pub fn new(height_points: f64) -> Option<Self> {
        (height_points.is_finite() && height_points > 0.0).then_some(Self(height_points))
    }

    /// Constructs a staff size from the desired distance between adjacent
    /// staff lines.
    pub fn from_points_per_space(points: f64) -> Option<Self> {
        Self::new(points * STAFF_SPACES_PER_EM)
    }

    pub const fn height_points(self) -> f64 {
        self.0
    }

    pub fn points_per_space(self) -> f64 {
        self.0 / STAFF_SPACES_PER_EM
    }
}

/// A point in score coordinates: x right, y up, with both axes in staff spaces.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StaffPoint {
    pub x: StaffSpaces,
    pub y: StaffSpaces,
}

impl StaffPoint {
    pub const fn new(x: StaffSpaces, y: StaffSpaces) -> Self {
        Self { x, y }
    }

    /// Converts to layout coordinates relative to the same origin. Layout x is
    /// rightward and layout y is downward, so y is negated.
    pub fn to_layout_point(self, staff_size: StaffSize) -> LayoutPoint {
        LayoutPoint {
            x: self.x.to_layout_points(staff_size),
            y: -self.y.to_layout_points(staff_size),
        }
    }

    /// Converts layout coordinates relative to the staff origin back to score
    /// coordinates, including the y-axis inversion.
    pub fn from_layout_point(point: LayoutPoint, staff_size: StaffSize) -> Self {
        Self {
            x: StaffSpaces::from_layout_points(point.x, staff_size),
            y: -StaffSpaces::from_layout_points(point.y, staff_size),
        }
    }
}

/// A point in UI layout coordinates: x right and y down, measured in points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayoutPoint {
    pub x: f64,
    pub y: f64,
}

/// A diatonic vertical staff position measured in half-space increments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct StaffStep(i32);

impl StaffStep {
    pub const fn new(value: i32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> i32 {
        self.0
    }

    pub fn to_staff_spaces(self) -> StaffSpaces {
        StaffSpaces::new(f64::from(self.0) * 0.5)
    }
}

impl Add for StaffStep {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl Sub for StaffStep {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}
