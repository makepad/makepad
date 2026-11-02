//! One-dimensional intervals (`[min, max]`, often a curve's parameter domain) with
//! clamping, snapping, normalised evaluation/inversion, set operations, predicates,
//! periodic wrapping/containment/intersection, and N-dimensional interval boxes.
//!
//! Upstream: `source/SMLib/src/SmExtent1d.cpp`, `source/SMLib/inc/SmExtent1d.h`,
//! `source/SMLib/src/SmExtentNd.cpp`, `source/SMLib/inc/SmExtentNd.h`.
//!
//! | Rust                                         | Upstream                                     |
//! |----------------------------------------------|----------------------------------------------|
//! | [`Extent1d`]                                 | `SmExtent1d`                                 |
//! | [`Extent1d::new`], [`Default`], [`Extent1d::init`] | `SmExtent1d()`, `Init`                 |
//! | [`Extent1d::from_value`]                     | `SmExtent1d(double)`                         |
//! | [`Extent1d::from_min_max`]                   | `SmExtent1d(double, double)`                 |
//! | [`Extent1d::from_raw`]                       | (direct member assignment)                   |
//! | `Clone`/`Copy`                               | copy constructor, `operator=`, destructor    |
//! | [`Extent1d::is_defined`]                     | `AssertDefined`                              |
//! | [`Extent1d::set_unbounded`]                  | `SetUnbounded`                               |
//! | [`Extent1d::negate`]                         | `Negate`                                     |
//! | [`Extent1d::translate`]                      | `Translate`                                  |
//! | [`Extent1d::min`], [`Extent1d::max`], [`Extent1d::mid`] | `GetMin`, `GetMax`, `GetMid`      |
//! | [`Extent1d::length`]                         | `GetLength`                                  |
//! | [`Extent1d::max_dimension`]                  | `GetMaxDimension`                            |
//! | [`Extent1d::t_left_eval`]                    | `GetTLeftEval`                               |
//! | [`Extent1d::add_value`]                      | `AddValue`                                   |
//! | [`Extent1d::clamp_value`]                    | `ClampValue`                                 |
//! | [`Extent1d::snap_value`]                     | `SnapValue`                                  |
//! | [`Extent1d::evaluate`]                       | `Evaluate`                                   |
//! | [`Extent1d::set_min_max`]                    | `SetMinMax`                                  |
//! | [`Extent1d::set_min`], [`Extent1d::set_max`] | `SetMin`, `SetMax`                           |
//! | [`Extent1d::union`]                          | `Union`                                      |
//! | [`Extent1d::distance_from`]                  | `DistanceFrom`                               |
//! | [`Extent1d::intersect`]                      | `Intersect`                                  |
//! | [`Extent1d::expand_absolute`]                | `ExpandAbsolute`                             |
//! | [`Extent1d::expand_relative`]                | `ExpandRelative`                             |
//! | [`Extent1d::scale`]                          | `Scale`                                      |
//! | [`Extent1d::inversion`], [`Inversion`]       | `Inversion`                                  |
//! | [`Extent1d::are_disjoint`]                   | `AreDisjoint`                                |
//! | [`Extent1d::are_equal`]                      | `AreEqual(const SmExtent1d&, double)`        |
//! | [`Extent1d::are_equal_bounds`]               | `AreEqual(double, double, double)`           |
//! | [`Extent1d::is_contained_by`]                | `IsContainedBy`                              |
//! | [`Extent1d::contains_value`]                 | `ContainsValue`                              |
//! | [`Extent1d::is_value_on_boundary`]           | `IsValueOnBoundary`                          |
//! | [`Extent1d::classify_point`], [`ExtentPointType`] | `ClassifyPoint`, `SmExtentPointType`    |
//! | [`Extent1d::is_degenerate`]                  | `IsDegenerate`                               |
//! | [`Extent1d::has_negative_length`]            | `HasNegativeLength`                          |
//! | [`Extent1d::is_init`]                        | `IsInit`                                     |
//! | [`Extent1d::boundary_type`], [`Extent1d::is_bounded`] | `IsBounded`                         |
//! | [`Extent1d::approximate_unbounded`], [`Extent1d::approximate_unbounded_expanded`] | `ApproximateUnbounded` |
//! | [`Extent1d::are_same`]                       | `operator==`                                 |
//! | [`Extent1d::are_disjoint_periodic`]          | `AreDisjointPeriodic`                        |
//! | [`Extent1d::periodic_wrap`]                  | `PeriodicWrap`                               |
//! | [`Extent1d::intersect_periodic`]             | `IntersectPeriodic`                          |
//! | [`Extent1d::clamp_periodic_value`]           | `ClampPeriodicValue`                         |
//! | [`Extent1d::contains_periodic_value`], [`PeriodicContainment`] | `ContainsPeriodicValue`    |
//! | [`Extent1d::is_closed`]                      | `IsClosed`                                   |
//! | [`Extent1d::is_value_on_periodic_boundary`]  | `IsValueOnPeriodicBoundary`                  |
//! | [`ExtentNd`], [`ExtentNd::new`]              | `SmExtentNd`, `SmExtentNd(ULONG)`, destructor |
//! | [`ExtentNd::get`], [`ExtentNd::get_mut`]     | `operator[] const`, `operator[]`             |
//! | [`ExtentNd::clamp_vector`]                   | `ClampVector`                                |
//! | `are_same`, `are_same_to_tol` (private)      | `SM_ARE_SAME`, `SM_ARE_SAME_TO_TOL`          |
//!
//! Deviations:
//! - Out-params become return values: [`Inversion`], [`PeriodicContainment`], tuples,
//!   `Option` for `ApproximateUnbounded`'s expanded extent (`None` where upstream's
//!   `SetMinMax` rejects it and leaves the caller's extent untouched).
//!   Optional out-params are always computed (they have no side effects).
//! - Status codes: `SM_ERR_INVALID_INPUT` is [`SolidError::InvalidInput`]; `SM_ERR`
//!   is [`SolidError::Upstream`] with code 1001. `Intersect` and the non-periodic
//!   branch of `IntersectPeriodic` leave an `Init()` interval as the result when they
//!   fail; the Rust error carries no result (it is always [`Extent1d::new`]).
//! - `ClampVector` keeps upstream's in/out arrays (`SetSize` keeps the existing
//!   prefix and zero-fills, unbounded dimensions are not written, counts are
//!   incremented); arrays shorter than the dimension (undefined behaviour upstream)
//!   are rejected with [`SolidError::InvalidInput`].
//! - `SM_ASSERT*` debug checks and the error-log side effect of `SE(...)` are not
//!   ported (they do not change results). The destructor's "undefined" sentinel has
//!   no Rust counterpart; [`Extent1d::is_defined`] still checks for it.

use crate::error::{Result, SolidError};
use crate::geom::extent3d::BoundaryType;
use crate::geom::tol::{are_same, are_same_to_tol};
use crate::math::{
    is_infinite_param, max2, min2, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_DEG, INFINITE_PARAMETER,
    UNDEF_DOUBLE,
};

/// Upstream status `SM_ERR`.
const SM_ERR: i32 = 1001;

/// Classification of a value against an interval (`SmExtentPointType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExtentPointType {
    /// `SM_EP_START`: on the minimum boundary.
    Start,
    /// `SM_EP_INSIDE`: in the interior.
    Inside,
    /// `SM_EP_END`: on the maximum boundary.
    End,
    /// `SM_EP_BOTH`: on both boundaries (degenerate interval).
    Both,
    /// `SM_EP_OUTSIDE`: outside the interval.
    Outside,
    /// `SM_EP_UNKNOWN`: not classified.
    Unknown,
}

impl ExtentPointType {
    /// Upstream enum value (`SM_EP_START` = 0 .. `SM_EP_UNKNOWN` = 5).
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::Start => 0,
            Self::Inside => 1,
            Self::End => 2,
            Self::Both => 3,
            Self::Outside => 4,
            Self::Unknown => 5,
        }
    }
}

/// Result of [`Extent1d::inversion`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inversion {
    /// The normalised parameter, `None` for a (near) zero-length interval (upstream
    /// `SM_UNDEF_DOUBLE`).
    pub parameter: Option<f64>,
    /// True when the value lies in the closed interval.
    pub contained: bool,
}

impl Inversion {
    /// True when upstream returns `SM_SUCCESS` (contained and a parameter exists).
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.contained && self.parameter.is_some()
    }
}

/// Result of [`Extent1d::contains_periodic_value`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeriodicContainment {
    /// True when the value, after accounting for periodicity, is inside within the
    /// tolerance.
    pub contained: bool,
    /// Best "contained" periodic value (no snapping).
    pub periodic_value: f64,
    /// Distance from the periodic value to the minimum boundary.
    pub dist_to_min: f64,
    /// Distance from the periodic value to the maximum boundary.
    pub dist_to_max: f64,
}

/// A closed interval `[min, max]` on the real line (`SmExtent1d`).
///
/// A fresh interval ([`Extent1d::new`]) is "inside out" (`min = BIG_DOUBLE`,
/// `max = -BIG_DOUBLE`) so that the first [`Extent1d::add_value`] makes it the value.
/// Unbounded ends are stored as `±INFINITE_PARAMETER`.
///
/// `PartialEq` compares the ends exactly (not upstream `operator==`, which is
/// [`Extent1d::are_same`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Extent1d {
    min: f64,
    max: f64,
}

impl Default for Extent1d {
    fn default() -> Self {
        Self::new()
    }
}

impl Extent1d {
    /// An "inside out" interval ready for [`Extent1d::add_value`] (`SmExtent1d()`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            min: BIG_DOUBLE,
            max: -BIG_DOUBLE,
        }
    }

    /// The degenerate interval `[v, v]` (`SmExtent1d(double)`).
    #[must_use]
    pub const fn from_value(v: f64) -> Self {
        Self { min: v, max: v }
    }

    /// The interval spanned by `a` and `b`, swapped when `a > b`
    /// (`SmExtent1d(double, double)`).
    #[must_use]
    pub fn from_min_max(a: f64, b: f64) -> Self {
        if a > b {
            Self { min: b, max: a }
        } else {
            Self { min: a, max: b }
        }
    }

    /// The interval with the given ends stored as given (no swapping), e.g. an
    /// inverted interval.
    #[must_use]
    pub const fn from_raw(min: f64, max: f64) -> Self {
        Self { min, max }
    }

    /// Resets to the "inside out" state of [`Extent1d::new`] (`Init`).
    pub fn init(&mut self) {
        *self = Self::new();
    }

    /// True unless an end holds upstream's destructor sentinel (`AssertDefined`).
    #[must_use]
    pub fn is_defined(&self) -> bool {
        self.min != UNDEF_DOUBLE && self.max != -UNDEF_DOUBLE
    }

    /// Makes the interval `[-INFINITE_PARAMETER, INFINITE_PARAMETER]` (`SetUnbounded`).
    pub fn set_unbounded(&mut self) {
        self.min = -INFINITE_PARAMETER;
        self.max = INFINITE_PARAMETER;
    }

    /// Mirrors the interval about zero: `[-max, -min]` (`Negate`).
    pub fn negate(&mut self) -> &mut Self {
        let tmp = self.min;
        self.min = -self.max;
        self.max = -tmp;
        self
    }

    /// Shifts both ends by `v`, leaving unbounded markers in place (`Translate`).
    pub fn translate(&mut self, v: f64) -> &mut Self {
        self.min = if is_infinite_param(self.min) {
            self.min
        } else {
            self.min + v
        };
        self.max = if is_infinite_param(self.max) {
            self.max
        } else {
            self.max + v
        };
        self
    }

    /// Lower end (`GetMin`).
    #[must_use]
    pub const fn min(&self) -> f64 {
        self.min
    }

    /// Upper end (`GetMax`).
    #[must_use]
    pub const fn max(&self) -> f64 {
        self.max
    }

    /// Centre, `(max + min) / 2` (`GetMid`).
    #[must_use]
    pub fn mid(&self) -> f64 {
        (self.max + self.min) / 2.0
    }

    /// `max - min` (`GetLength`).
    #[must_use]
    pub fn length(&self) -> f64 {
        self.max - self.min
    }

    /// The larger absolute end value (`GetMaxDimension`).
    #[must_use]
    pub fn max_dimension(&self) -> f64 {
        max2(self.max.abs(), self.min.abs())
    }

    /// True when `t` is in the lower half of the interval (`t <= mid`): evaluate
    /// "from the left" at a boundary between intervals (`GetTLeftEval`).
    #[must_use]
    pub fn t_left_eval(&self, t: f64) -> bool {
        let mid = (self.min + self.max) / 2.0;
        t <= mid
    }

    /// Grows the interval to cover `v` (`AddValue`).
    pub fn add_value(&mut self, v: f64) {
        if v < self.min {
            self.min = v;
        }
        if v > self.max {
            self.max = v;
        }
    }

    /// `v` clamped to the interval (`ClampValue`).
    #[must_use]
    pub fn clamp_value(&self, v: f64) -> f64 {
        if v < self.min {
            self.min
        } else if v > self.max {
            self.max
        } else {
            v
        }
    }

    /// `v` clamped to the interval, snapping values within `tol` of an end to that
    /// end (`SnapValue`; upstream default `tol` is `EFF_ZERO`).
    #[must_use]
    pub fn snap_value(&self, v: f64, tol: f64) -> f64 {
        if v < self.min + tol {
            self.min
        } else if v > self.max - tol {
            self.max
        } else {
            v
        }
    }

    /// The value at normalised parameter `t`: `0` gives `min`, `1` gives `max`
    /// (exactly), other values interpolate or extrapolate (`Evaluate`).
    #[must_use]
    pub fn evaluate(&self, t: f64) -> f64 {
        if t == 0.0 {
            return self.min;
        }
        if t == 1.0 {
            return self.max;
        }
        self.min + t * (self.max - self.min)
    }

    fn negative_interval(operation: &'static str) -> SolidError {
        SolidError::InvalidInput {
            operation,
            reason: "assignment would make a negative interval",
        }
    }

    /// Sets both ends; fails (leaving the interval unchanged) when `min > max`
    /// (`SetMinMax`).
    pub fn set_min_max(&mut self, min: f64, max: f64) -> Result<()> {
        if min > max {
            return Err(Self::negative_interval(
                "geom::extent1d::Extent1d::set_min_max",
            ));
        }
        self.min = min;
        self.max = max;
        Ok(())
    }

    /// Sets the lower end; fails when it exceeds the upper end (`SetMin`).
    pub fn set_min(&mut self, min: f64) -> Result<()> {
        if min > self.max {
            return Err(Self::negative_interval("geom::extent1d::Extent1d::set_min"));
        }
        self.min = min;
        Ok(())
    }

    /// Sets the upper end; fails when it is below the lower end (`SetMax`).
    pub fn set_max(&mut self, max: f64) -> Result<()> {
        if max < self.min {
            return Err(Self::negative_interval("geom::extent1d::Extent1d::set_max"));
        }
        self.max = max;
        Ok(())
    }

    /// The smallest interval containing both (`Union`).
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        Self {
            min: min2(self.min, other.min),
            max: max2(self.max, other.max),
        }
    }

    /// Distance from `v` to the interval, `0` inside (`DistanceFrom`).
    #[must_use]
    pub fn distance_from(&self, v: f64) -> f64 {
        if v < self.min {
            return self.min - v;
        }
        if v > self.max {
            return v - self.max;
        }
        0.0
    }

    /// The overlap of two intervals; fails (`SM_ERR`) when they are disjoint
    /// (`Intersect`; upstream then sets the result to `Init()`).
    pub fn intersect(&self, other: &Self) -> Result<Self> {
        if self.are_disjoint(other, 0.0) {
            return Err(SolidError::Upstream {
                operation: "geom::extent1d::Extent1d::intersect",
                code: SM_ERR,
                entity: None,
            });
        }
        Ok(Self {
            min: max2(self.min, other.min),
            max: min2(self.max, other.max),
        })
    }

    /// Moves both bounded ends outwards by `d` (inwards when negative)
    /// (`ExpandAbsolute`).
    pub fn expand_absolute(&mut self, d: f64) -> &mut Self {
        self.min -= if is_infinite_param(self.min) { 0.0 } else { d };
        self.max += if is_infinite_param(self.max) { 0.0 } else { d };
        self
    }

    /// Moves both bounded ends outwards by `factor * length` (`ExpandRelative`).
    pub fn expand_relative(&mut self, factor: f64) -> &mut Self {
        let inc = (self.max - self.min) * factor;
        self.min -= if is_infinite_param(self.min) {
            0.0
        } else {
            inc
        };
        self.max += if is_infinite_param(self.max) {
            0.0
        } else {
            inc
        };
        self
    }

    /// Multiplies both bounded ends by `s` (any sign), re-ordering them if needed
    /// (`Scale`).
    pub fn scale(&mut self, s: f64) -> &mut Self {
        self.min *= if is_infinite_param(self.min) { 1.0 } else { s };
        self.max *= if is_infinite_param(self.max) { 1.0 } else { s };
        if self.min > self.max {
            core::mem::swap(&mut self.min, &mut self.max);
        }
        self
    }

    /// The normalised parameter of `v` (inverse of [`Extent1d::evaluate`], not
    /// clamped) (`Inversion`).
    #[must_use]
    pub fn inversion(&self, v: f64) -> Inversion {
        let contained = self.contains_value(v, 0.0);
        let numer = v - self.min;
        let denom = self.max - self.min;
        let parameter = if denom < numer.abs() * EFF_ZERO {
            None
        } else {
            Some(numer / denom)
        };
        Inversion {
            parameter,
            contained,
        }
    }

    /// True when the intervals are separated by more than `-tol` (touching intervals
    /// are not disjoint for `tol = 0`) (`AreDisjoint`).
    #[must_use]
    pub fn are_disjoint(&self, other: &Self, tol: f64) -> bool {
        (self.min > other.max - tol) || (self.max < other.min + tol)
    }

    /// True when both ends agree to within `|tol|` (`AreEqual(const SmExtent1d&)`).
    #[must_use]
    pub fn are_equal(&self, other: &Self, tol: f64) -> bool {
        self.are_equal_bounds(other.min, other.max, tol)
    }

    /// True when the ends agree with `min`, `max` to within `|tol|`
    /// (`AreEqual(double, double, double)`).
    #[must_use]
    pub fn are_equal_bounds(&self, min: f64, max: f64, tol: f64) -> bool {
        are_same_to_tol(self.min, min, tol) && are_same_to_tol(self.max, max, tol)
    }

    /// True when `self` lies inside `other` grown by `tol` (`IsContainedBy`).
    #[must_use]
    pub fn is_contained_by(&self, other: &Self, tol: f64) -> bool {
        other.min - tol <= self.min && other.max + tol >= self.max
    }

    /// True when `min - tol <= v <= max + tol` (`ContainsValue`).
    #[must_use]
    pub fn contains_value(&self, v: f64, tol: f64) -> bool {
        v >= self.min - tol && v <= self.max + tol
    }

    /// Whether `v` is within `tol` (strictly) of an end, and the *other* end
    /// (`IsValueOnBoundary`; upstream default `tol` is `EFF_ZERO`; the second value is
    /// its optional out-param, meaningful only when the first is true).
    #[must_use]
    pub fn is_value_on_boundary(&self, v: f64, tol: f64) -> (bool, f64) {
        let del_min = (v - self.min).abs();
        let del_max = (v - self.max).abs();
        let on_min = del_min < tol;
        let on_max = del_max < tol;
        let other = if on_min && on_max {
            if del_min < del_max {
                self.max
            } else {
                self.min
            }
        } else if on_min {
            self.max
        } else {
            self.min
        };
        (on_min || on_max, other)
    }

    /// Classifies `v` as on an end, inside or outside (`ClassifyPoint`; upstream
    /// default `tol` is `EFF_ZERO`).
    #[must_use]
    pub fn classify_point(&self, v: f64, tol: f64) -> ExtentPointType {
        if (v - self.min).abs() <= tol && (v - self.max).abs() <= tol {
            ExtentPointType::Both
        } else if (v - self.min).abs() <= tol {
            ExtentPointType::Start
        } else if (v - self.max).abs() <= tol {
            ExtentPointType::End
        } else if v - self.min >= 0.0 && self.max - v >= 0.0 {
            ExtentPointType::Inside
        } else {
            ExtentPointType::Outside
        }
    }

    /// True when the length is within `tol` of zero (`IsDegenerate`).
    #[must_use]
    pub fn is_degenerate(&self, tol: f64) -> bool {
        (self.min >= self.max - tol) && (self.min <= self.max + tol)
    }

    /// True when `min > max` (`HasNegativeLength`).
    #[must_use]
    pub fn has_negative_length(&self) -> bool {
        self.min > self.max
    }

    /// True in the state of [`Extent1d::new`] (`IsInit`).
    #[must_use]
    pub fn is_init(&self) -> bool {
        self.min == BIG_DOUBLE && self.max == -BIG_DOUBLE
    }

    /// Classifies the ends as bounded or unbounded (`IsBounded`'s out-param).
    #[must_use]
    pub fn boundary_type(&self) -> BoundaryType {
        if is_infinite_param(self.max) && is_infinite_param(self.min) {
            BoundaryType::Unbounded
        } else if is_infinite_param(self.min) {
            BoundaryType::UnboundedMin
        } else if is_infinite_param(self.max) {
            BoundaryType::UnboundedMax
        } else {
            BoundaryType::Bounded
        }
    }

    /// True when neither end is an unbounded marker (`IsBounded`).
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        self.boundary_type() == BoundaryType::Bounded
    }

    /// The approximated ends of `ApproximateUnbounded`, before the final swap.
    fn approx_ends(&self, center: f64, half: f64) -> (BoundaryType, f64, f64) {
        let ty = self.boundary_type();
        let mut min = self.min;
        let mut max = self.max;
        if ty != BoundaryType::Bounded {
            min = match ty {
                BoundaryType::UnboundedMin => {
                    if max < center + half {
                        max - 2.0 * half
                    } else {
                        center - half
                    }
                }
                BoundaryType::UnboundedMax => {
                    if min > center - half {
                        min
                    } else {
                        center - half
                    }
                }
                BoundaryType::Unbounded => center - half,
                BoundaryType::Bounded | BoundaryType::Unknown => min,
            };
            max = match ty {
                BoundaryType::UnboundedMin => {
                    if max < center + half {
                        max
                    } else {
                        center + half
                    }
                }
                BoundaryType::UnboundedMax => {
                    if min > center - half {
                        min + 2.0 * half
                    } else {
                        center + half
                    }
                }
                BoundaryType::Unbounded => center + half,
                BoundaryType::Bounded | BoundaryType::Unknown => max,
            };
        }
        (ty, min, max)
    }

    /// A bounded interval approximating this one: unbounded ends are replaced
    /// using `center` and `half` (upstream defaults `0` and
    /// `BOUNDED_INFINITE_PARAM`); a bounded interval is returned unchanged
    /// (`ApproximateUnbounded` without the expanded extent).
    #[must_use]
    pub fn approximate_unbounded(&self, center: f64, half: f64) -> Self {
        let (_, min, max) = self.approx_ends(center, half);
        Self::from_min_max(min, max)
    }

    /// [`Extent1d::approximate_unbounded`] plus the extent expanded by `half / 12`
    /// at each replaced end (`ApproximateUnbounded` with `pOptExpandedApprox`);
    /// `None` where upstream's `SetMinMax` rejects it (min > max).
    #[must_use]
    pub fn approximate_unbounded_expanded(&self, center: f64, half: f64) -> (Self, Option<Self>) {
        let (ty, min, max) = self.approx_ends(center, half);
        let mut min_inc = min;
        let mut max_inc = max;
        let inf_inc = half / 12.0;
        if ty != BoundaryType::Bounded {
            if ty == BoundaryType::UnboundedMin || ty == BoundaryType::Unbounded {
                min_inc -= inf_inc;
            }
            if ty == BoundaryType::UnboundedMax || ty == BoundaryType::Unbounded {
                max_inc += inf_inc;
            }
        }
        let mut expanded = Self::new();
        let expanded = expanded
            .set_min_max(min_inc, max_inc)
            .ok()
            .map(|()| expanded);
        (Self::from_min_max(min, max), expanded)
    }

    /// Both ends equal up to the scaled zero (`operator==`).
    #[must_use]
    pub fn are_same(&self, other: &Self) -> bool {
        are_same(self.min, other.min) && are_same(self.max, other.max)
    }

    /// True when, in a periodic space of length `period`, neither interval contains
    /// an end of the other (`AreDisjointPeriodic`; upstream default `tol` is `-1`,
    /// meaning `EFF_ZERO_DEG`).
    #[must_use]
    pub fn are_disjoint_periodic(&self, other: &Self, period: f64, tol: f64) -> bool {
        !self
            .contains_periodic_value(other.min, period, tol)
            .contained
            && !self
                .contains_periodic_value(other.max, period, tol)
                .contained
            && !other
                .contains_periodic_value(self.min, period, tol)
                .contained
            && !other
                .contains_periodic_value(self.max, period, tol)
                .contained
    }

    /// `v` wrapped into the interval, treating its length as the period; values
    /// within `EFF_ZERO` of an end snap to it. Returns `v` for a (near) zero-length
    /// interval or when `v` is too large to wrap (`PeriodicWrap`).
    #[must_use]
    pub fn periodic_wrap(&self, v: f64) -> f64 {
        let period = self.max - self.min;
        if period <= EFF_ZERO {
            return v;
        }
        let mut v = v;
        if (v - self.min).abs() < EFF_ZERO {
            v = self.min;
        }
        if (v - self.max).abs() < EFF_ZERO {
            v = self.max;
        }
        if v + period == v {
            return v;
        }
        while v < self.min {
            v += period;
            if (v - self.min).abs() < EFF_ZERO {
                v = self.min;
            }
            if (v - self.max).abs() < EFF_ZERO {
                v = self.max;
            }
        }
        while v > self.max {
            v -= period;
            if (v - self.min).abs() < EFF_ZERO {
                v = self.min;
            }
            if (v - self.max).abs() < EFF_ZERO {
                v = self.max;
            }
        }
        v
    }

    /// The intersection of two intervals of the same periodic space of length
    /// `period` (`0`: not periodic, as [`Extent1d::intersect`]): 0, 1 or 2 intervals,
    /// contiguous where possible. With `dont_cross`, results are split at this
    /// interval's boundaries (`IntersectPeriodic`; upstream default `false`).
    ///
    /// Fails (`SM_ERR`) only for `period == 0` and disjoint intervals (upstream's
    /// result is then one `Init()` interval).
    pub fn intersect_periodic(
        &self,
        other: &Self,
        period: f64,
        dont_cross: bool,
    ) -> Result<Vec<Self>> {
        let mut out = Vec::new();
        if period == 0.0 {
            out.push(self.intersect(other)?);
            return Ok(out);
        }

        let ivl = Self::from_min_max(self.min, self.min + period);
        let this_min = self.min;
        let this_max = self.max;
        let mut other_min = ivl.periodic_wrap(other.min);
        let mut other_max = ivl.periodic_wrap(other.max);

        if other_min < other_max - EFF_ZERO {
            if (this_min <= other_max + EFF_ZERO) && (this_max >= other_min - EFF_ZERO) {
                let min = max2(this_min, other_min);
                let mut max = min2(this_max, other_max);
                if are_same(min, max) {
                    max = min;
                }
                out.push(Self::from_min_max(min, max));
            }
            if !self.is_closed(period)
                && !other.is_closed(period)
                && are_same(other_max, this_min + period)
            {
                out.push(Self::from_min_max(this_min, this_min));
            }
        } else if other_min > other_max + EFF_ZERO {
            let this_closed = self.is_closed(period);
            if this_closed && !dont_cross {
                out.push(*other);
            } else {
                if this_min <= other_max + EFF_ZERO {
                    if this_min > other_max {
                        other_max = this_min;
                    }
                    if !(this_closed && are_same(this_min, other_max)) {
                        out.push(Self::from_min_max(this_min, min2(this_max, other_max)));
                    }
                }
                if this_max >= other_min - EFF_ZERO {
                    if this_max < other_min {
                        other_min = this_max;
                    }
                    if !(this_closed && are_same(this_max, other_min)) {
                        out.push(Self::from_min_max(max2(this_min, other_min), this_max));
                    }
                }
            }
        } else if other.is_closed(period) {
            if dont_cross {
                if self.contains_value(other_min, -EFF_ZERO) {
                    out.push(Self::from_min_max(this_min, other_max));
                    out.push(Self::from_min_max(other_min, this_max));
                } else {
                    out.push(*self);
                }
            } else {
                out.push(*self);
            }
        } else if other_min <= this_max {
            out.push(Self::from_min_max(other_min, other_min));
        }
        Ok(out)
    }

    /// The periodic value of `v` inside the interval, or the nearer end when it is
    /// outside (`ClampPeriodicValue`; upstream default `tol` is `-1`).
    #[must_use]
    pub fn clamp_periodic_value(&self, v: f64, period: f64, tol: f64) -> f64 {
        let c = self.contains_periodic_value(v, period, tol);
        if c.contained {
            c.periodic_value
        } else if c.dist_to_min < c.dist_to_max {
            self.min
        } else {
            self.max
        }
    }

    /// Whether `v`, accounting for a period of length `period` (`0`: not periodic),
    /// lies in the interval to within `tol` (`tol <= 0`: `EFF_ZERO_DEG`), with the
    /// periodic value and its distances to the ends (`ContainsPeriodicValue`).
    #[must_use]
    pub fn contains_periodic_value(&self, v: f64, period: f64, tol: f64) -> PeriodicContainment {
        let zero = if tol > 0.0 { tol } else { EFF_ZERO_DEG };
        let mut contained = self.contains_value(v, zero);
        if contained || period == 0.0 {
            return PeriodicContainment {
                contained,
                periodic_value: v,
                dist_to_min: (v - self.min).abs(),
                dist_to_max: (v - self.max).abs(),
            };
        }

        let ivl = Self::from_min_max(self.min, self.min + period);
        let mut map = v;
        if !ivl.contains_value(v, zero) {
            map = ivl.periodic_wrap(v);
        }

        contained = map > self.min - zero && map < self.max + zero;

        if !contained
            && are_same_to_tol(map, ivl.max(), zero)
            && are_same_to_tol(map - period, self.min, zero)
        {
            map = self.min;
            contained = true;
        }

        let periodic_value =
            if contained && ((v - self.min).abs() < zero || (v - self.max).abs() < zero) {
                v
            } else if contained && map > self.max + zero {
                map - period
            } else {
                map
            };
        let mut dist_to_min = map - self.min;
        if dist_to_min > period / 2.0 {
            dist_to_min = period - dist_to_min;
        }
        let mut dist_to_max = (self.max - map).abs();
        if dist_to_max > period / 2.0 {
            dist_to_max = period - dist_to_max;
        }
        PeriodicContainment {
            contained,
            periodic_value,
            dist_to_min,
            dist_to_max,
        }
    }

    /// True when the interval covers a whole period (`IsClosed`).
    #[must_use]
    pub fn is_closed(&self, period: f64) -> bool {
        period != 0.0 && are_same(self.max - self.min, period)
    }

    /// Whether `v` is within `tol` (strictly) of an end, accounting for periodicity,
    /// and its distance to the nearer end (`IsValueOnPeriodicBoundary`; upstream
    /// default `tol` is `EFF_ZERO`).
    #[must_use]
    pub fn is_value_on_periodic_boundary(&self, v: f64, period: f64, tol: f64) -> (bool, f64) {
        let c = self.contains_periodic_value(v, period, tol);
        let on = c.dist_to_min < tol || c.dist_to_max < tol;
        (on, min2(c.dist_to_min, c.dist_to_max))
    }
}

/// An N-dimensional box of intervals (`SmExtentNd`).
#[derive(Debug, Clone, PartialEq)]
pub struct ExtentNd {
    extents: Vec<Extent1d>,
}

impl ExtentNd {
    /// `dim` intervals, each in the [`Extent1d::new`] state (`SmExtentNd(ULONG)`).
    #[must_use]
    pub fn new(dim: usize) -> Self {
        Self {
            extents: vec![Extent1d::new(); dim],
        }
    }

    /// Number of dimensions.
    #[must_use]
    pub fn dimension(&self) -> usize {
        self.extents.len()
    }

    /// Interval `i` (`operator[] const`).
    #[must_use]
    pub fn get(&self, i: usize) -> Option<Extent1d> {
        self.extents.get(i).copied()
    }

    /// Mutable interval `i` (`operator[]`).
    pub fn get_mut(&mut self, i: usize) -> Option<&mut Extent1d> {
        self.extents.get_mut(i)
    }

    /// Clamps (or, where `periodic[i]`, wraps) each `values[i]` to interval `i`;
    /// unbounded intervals are skipped (`ClampVector`).
    ///
    /// As upstream, `clamped` and `counts` are resized to the dimension keeping their
    /// existing prefix and zero-filling; `clamped[i]` is written only for bounded
    /// intervals, and `counts[i]` is incremented when a non-periodic value moved by
    /// more than `EFF_ZERO`. Returns true when at least one value was clamped.
    /// Fails when `values` or `periodic` is shorter than the dimension.
    pub fn clamp_vector(
        &self,
        values: &[f64],
        periodic: Option<&[bool]>,
        clamped: &mut Vec<f64>,
        mut counts: Option<&mut Vec<u64>>,
    ) -> Result<bool> {
        let dim = self.extents.len();
        if values.len() < dim || periodic.is_some_and(|p| p.len() < dim) {
            return Err(SolidError::InvalidInput {
                operation: "geom::extent1d::ExtentNd::clamp_vector",
                reason: "vector shorter than the dimension",
            });
        }
        let mut rtn = false;
        clamped.resize(dim, 0.0);
        if let Some(c) = counts.as_deref_mut() {
            c.resize(dim, 0);
        }
        for (i, (ext, &value)) in self.extents.iter().zip(values).enumerate() {
            if !ext.is_bounded() {
                continue;
            }
            let is_periodic = periodic.and_then(|p| p.get(i)).copied().unwrap_or(false);
            let Some(slot) = clamped.get_mut(i) else {
                continue;
            };
            if is_periodic {
                *slot = ext.periodic_wrap(value);
            } else if let Some(c) = counts.as_deref_mut() {
                if ext.contains_value(value, 0.0) {
                    *slot = value;
                } else {
                    let tmp = ext.clamp_value(value);
                    *slot = tmp;
                    if (tmp - value).abs() > EFF_ZERO {
                        if let Some(n) = c.get_mut(i) {
                            *n = n.wrapping_add(1);
                        }
                        rtn = true;
                    }
                }
            } else {
                let tmp = ext.clamp_value(value);
                if (tmp - value).abs() > EFF_ZERO {
                    rtn = true;
                }
                *slot = tmp;
            }
        }
        Ok(rtn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::BOUNDED_INFINITE_PARAM;

    #[test]
    fn construct_and_access() {
        let e = Extent1d::from_min_max(3.0, 1.0);
        assert_eq!((e.min(), e.max()), (1.0, 3.0));
        assert_eq!(e.mid(), 2.0);
        assert_eq!(e.length(), 2.0);
        assert_eq!(Extent1d::from_min_max(-5.0, 1.0).max_dimension(), 5.0);
        assert!(Extent1d::new().is_init());
        assert!(Extent1d::new().has_negative_length());
        assert!(e.is_defined());
        assert!(!Extent1d::from_raw(UNDEF_DOUBLE, 0.0).is_defined());
        assert!(e.t_left_eval(2.0));
        assert!(!e.t_left_eval(2.5));
    }

    #[test]
    fn add_clamp_snap_evaluate() {
        let mut e = Extent1d::new();
        e.add_value(2.0);
        e.add_value(-1.0);
        assert_eq!((e.min(), e.max()), (-1.0, 2.0));
        assert_eq!(e.clamp_value(5.0), 2.0);
        assert_eq!(e.clamp_value(-5.0), -1.0);
        assert_eq!(e.snap_value(1.9, 0.2), 2.0);
        assert_eq!(e.snap_value(0.5, 0.2), 0.5);
        assert_eq!(e.evaluate(0.0), -1.0);
        assert_eq!(e.evaluate(1.0), 2.0);
        assert_eq!(e.evaluate(0.5), 0.5);
        let inv = e.inversion(0.5);
        assert!(inv.is_success());
        assert_eq!(inv.parameter, Some(0.5));
        let out = e.inversion(5.0);
        assert!(!out.is_success());
        assert_eq!(out.parameter, Some(2.0));
        assert_eq!(Extent1d::from_value(1.0).inversion(2.0).parameter, None);
    }

    #[test]
    fn setters() {
        let mut e = Extent1d::from_min_max(0.0, 1.0);
        assert!(e.set_min_max(2.0, 1.0).is_err());
        assert_eq!((e.min(), e.max()), (0.0, 1.0));
        assert!(e.set_min(2.0).is_err());
        assert!(e.set_max(-1.0).is_err());
        assert!(e.set_min(0.5).is_ok());
        assert!(e.set_max(0.5).is_ok());
        assert_eq!((e.min(), e.max()), (0.5, 0.5));
    }

    #[test]
    fn set_operations() {
        let a = Extent1d::from_min_max(0.0, 2.0);
        let b = Extent1d::from_min_max(1.0, 3.0);
        assert_eq!(a.union(&b), Extent1d::from_min_max(0.0, 3.0));
        assert_eq!(a.intersect(&b).ok(), Some(Extent1d::from_min_max(1.0, 2.0)));
        let c = Extent1d::from_min_max(5.0, 6.0);
        assert!(a.intersect(&c).is_err());
        assert!(a.are_disjoint(&c, 0.0));
        assert!(!a.are_disjoint(&Extent1d::from_min_max(2.0, 3.0), 0.0));
        assert_eq!(a.distance_from(-1.0), 1.0);
        assert_eq!(a.distance_from(1.0), 0.0);
        assert!(Extent1d::from_min_max(0.5, 1.0).is_contained_by(&a, 0.0));
        assert!(a.are_equal(&Extent1d::from_min_max(0.1, 2.0), -0.2));
        assert!(a.are_same(&Extent1d::from_min_max(1e-13, 2.0)));
    }

    #[test]
    fn expand_scale_negate_translate() {
        let mut e = Extent1d::from_min_max(1.0, 3.0);
        e.expand_absolute(1.0);
        assert_eq!((e.min(), e.max()), (0.0, 4.0));
        e.expand_relative(0.5);
        assert_eq!((e.min(), e.max()), (-2.0, 6.0));
        e.scale(-1.0);
        assert_eq!((e.min(), e.max()), (-6.0, 2.0));
        e.negate();
        assert_eq!((e.min(), e.max()), (-2.0, 6.0));
        let mut u = Extent1d::new();
        u.set_unbounded();
        u.translate(5.0);
        assert_eq!(u.min(), -INFINITE_PARAMETER);
        assert!(!u.is_bounded());
        assert_eq!(u.boundary_type(), BoundaryType::Unbounded);
    }

    #[test]
    fn classify_and_boundary() {
        let e = Extent1d::from_min_max(0.0, 1.0);
        assert_eq!(e.classify_point(0.0, EFF_ZERO), ExtentPointType::Start);
        assert_eq!(e.classify_point(1.0, EFF_ZERO), ExtentPointType::End);
        assert_eq!(e.classify_point(0.5, EFF_ZERO), ExtentPointType::Inside);
        assert_eq!(e.classify_point(2.0, EFF_ZERO), ExtentPointType::Outside);
        assert_eq!(
            Extent1d::from_value(0.0).classify_point(0.0, EFF_ZERO),
            ExtentPointType::Both
        );
        assert_eq!(e.is_value_on_boundary(1.0, EFF_ZERO), (true, 0.0));
        assert!(Extent1d::from_value(1.0).is_degenerate(0.0));
    }

    #[test]
    fn approximate() {
        let mut e = Extent1d::from_raw(-INFINITE_PARAMETER, 5.0);
        let a = e.approximate_unbounded(0.0, BOUNDED_INFINITE_PARAM);
        assert_eq!((a.min(), a.max()), (5.0 - 66.0, 5.0));
        e.set_unbounded();
        let (a, x) = e.approximate_unbounded_expanded(0.0, 12.0);
        assert_eq!((a.min(), a.max()), (-12.0, 12.0));
        assert_eq!(x, Some(Extent1d::from_min_max(-13.0, 13.0)));
        let b = Extent1d::from_min_max(1.0, 2.0);
        assert_eq!(b.approximate_unbounded(0.0, 33.0), b);
    }

    #[test]
    fn periodic() {
        let e = Extent1d::from_min_max(0.0, 2.0);
        assert_eq!(e.periodic_wrap(4.5), 0.5);
        assert_eq!(e.periodic_wrap(-1.0), 1.0);
        let full = Extent1d::from_min_max(0.0, 360.0);
        let r = full
            .intersect_periodic(&Extent1d::from_min_max(-90.0, 90.0), 360.0, false)
            .unwrap_or_default();
        assert_eq!(r, vec![Extent1d::from_min_max(-90.0, 90.0)]);
        let r = full
            .intersect_periodic(&Extent1d::from_min_max(-90.0, 90.0), 360.0, true)
            .unwrap_or_default();
        assert_eq!(
            r,
            vec![
                Extent1d::from_min_max(0.0, 90.0),
                Extent1d::from_min_max(270.0, 360.0)
            ]
        );
        let q = Extent1d::from_min_max(0.0, 90.0);
        let c = q.contains_periodic_value(370.0, 360.0, -1.0);
        assert!(c.contained);
        assert_eq!(c.periodic_value, 10.0);
        assert_eq!(q.clamp_periodic_value(200.0, 360.0, -1.0), 90.0);
        assert!(full.is_closed(360.0));
        assert!(q.is_value_on_periodic_boundary(360.0, 360.0, EFF_ZERO).0);
        assert!(q.are_disjoint_periodic(&Extent1d::from_min_max(100.0, 200.0), 360.0, -1.0));
        assert!(q
            .intersect_periodic(&Extent1d::from_min_max(100.0, 200.0), 0.0, false)
            .is_err());
    }

    #[test]
    fn clamp_vector() {
        let mut nd = ExtentNd::new(3);
        if let Some(e) = nd.get_mut(0) {
            *e = Extent1d::from_min_max(0.0, 1.0);
        }
        if let Some(e) = nd.get_mut(1) {
            *e = Extent1d::from_min_max(0.0, 2.0);
        }
        if let Some(e) = nd.get_mut(2) {
            e.set_unbounded();
        }
        let mut out = vec![9.0];
        let mut counts = vec![5];
        let r = nd.clamp_vector(
            &[2.0, 3.0, 7.0],
            Some(&[false, true, false]),
            &mut out,
            Some(&mut counts),
        );
        assert_eq!(r.ok(), Some(true));
        assert_eq!(out, vec![1.0, 1.0, 0.0]);
        assert_eq!(counts, vec![6, 0, 0]);
        assert!(nd.clamp_vector(&[1.0], None, &mut out, None).is_err());
        assert_eq!(nd.get(5), None);
    }
}
