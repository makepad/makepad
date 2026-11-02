//! Axis-aligned 2D extents (`[min, max]` rectangles, usually a surface's `(u, v)`
//! parameter domain): construction, growth, set operations, containment and boundary
//! classification, periodic containment, normalised evaluation/inversion, line
//! clipping and domain-to-domain mapping.
//!
//! Upstream: `source/SMLib/src/SmExtent2d.cpp`, `source/SMLib/inc/SmExtent2d.h`.
//!
//! | Rust                                   | Upstream                                       |
//! |----------------------------------------|------------------------------------------------|
//! | [`Extent2d`]                           | `SmExtent2d`                                   |
//! | [`Extent2d::new`], [`Default`]         | `SmExtent2d()`                                 |
//! | [`Extent2d::init`]                     | `Init`                                         |
//! | [`Extent2d::from_point`]               | `SmExtent2d(const SmPoint2d&)`                 |
//! | [`Extent2d::from_points`]              | `SmExtent2d(const SmPoint2d&, const SmPoint2d&)` |
//! | [`Extent2d::from_bounds`]              | `SmExtent2d(double x4)`                        |
//! | [`Extent2d::from_extent3d`], `From<Extent3d>` | `SmExtent2d(const SmExtent3d&)`         |
//! | `Clone`/`Copy`                         | copy constructor, `operator=`, destructor      |
//! | [`Extent2d::are_same`]                 | `operator==`                                   |
//! | [`Extent2d::add_point`]                | `AddPoint2d`                                   |
//! | [`Extent2d::set_min_max`]              | `SetMinMax(const SmPoint2d&, const SmPoint2d&)` |
//! | [`Extent2d::set_min_max_coords`]       | `SetMinMax(double x4)`                         |
//! | [`Extent2d::set_u_interval`], [`Extent2d::set_v_interval`] | `SetUInterval`, `SetVInterval` |
//! | [`Extent2d::set_u_min`], [`Extent2d::set_u_max`], [`Extent2d::set_v_min`], [`Extent2d::set_v_max`] | `SetUMin`, `SetUMax`, `SetVMin`, `SetVMax` |
//! | [`Extent2d::transpose`]                | `Transpose`                                    |
//! | [`Extent2d::expand_absolute`]          | `ExpandAbsolute(double)`                       |
//! | [`Extent2d::expand_absolute_vec`]      | `ExpandAbsolute(const SmVector2d)`             |
//! | [`Extent2d::expand_relative`]          | `ExpandRelative`                               |
//! | [`Extent2d::min`], [`Extent2d::mid`], [`Extent2d::max`] | `GetMin`, `GetMid`, `GetMax`  |
//! | [`Extent2d::u_min`] .. [`Extent2d::v_max`] | `GetUMin`, `GetUMax`, `GetVMin`, `GetVMax` |
//! | [`Extent2d::u_interval`], [`Extent2d::v_interval`] | `GetUInterval`, `GetVInterval`     |
//! | [`Extent2d::max_dimension`]            | `GetMaxDimension`                              |
//! | [`Extent2d::size`]                     | `GetSize`                                      |
//! | [`Extent2d::x_length`], [`Extent2d::y_length`] | `XLength`, `YLength`                   |
//! | [`Extent2d::union`]                    | `Union`                                        |
//! | [`Extent2d::intersect`]                | `Intersect`                                    |
//! | [`Extent2d::max_boundary_dist`]        | `GetMaxBoundaryDist2d`                         |
//! | [`Extent2d::are_disjoint`]             | `AreDisjoint`                                  |
//! | [`Extent2d::are_equal`]                | `AreEqual`                                     |
//! | [`Extent2d::is_degenerate`]            | `IsDegenerate`                                 |
//! | [`Extent2d::has_negative_area`]        | `HasNegativeArea`                              |
//! | [`Extent2d::is_init`]                  | `IsInit`                                       |
//! | [`Extent2d::boundary_types`], [`Extent2d::is_bounded`] | `IsBounded`                    |
//! | [`Extent2d::is_contained_by`]          | `IsContainedBy`                                |
//! | [`Extent2d::contains_point`], [`Extent2d::contains_point_dist`] | `ContainsPoint2d`     |
//! | [`Extent2d::classify_point`]           | `ClassifyPoint2d`                              |
//! | [`Extent2d::is_point_on_boundary`]     | `IsPoint2dOnBoundary`                          |
//! | [`Extent2d::point_boundaries`]         | `GetPoint2dBoundaries`                         |
//! | [`Extent2d::contains_periodic_point`]  | `ContainsPeriodicPoint2d`                      |
//! | [`Extent2d::evaluate`]                 | `Evaluate`                                     |
//! | [`Extent2d::inversion`]                | `Inversion`                                    |
//! | [`Extent2d::clamp_point`]              | `ClampPoint2d`                                 |
//! | [`Extent2d::clip_line`]                | `ClipLine2d`                                   |
//! | [`Extent2d::intersect_with_infinite_line`] | `IntersectWithInfiniteLine`                |
//! | [`Extent2d::u_left_eval`], [`Extent2d::v_left_eval`] | `GetULeftEval`, `GetVLeftEval`   |
//! | [`Extent2d::point_map_to_domain`]      | `Point2dMapToDomain`                           |
//! | [`SS_NONE`], [`SS_UMIN`], [`SS_VMIN`], [`SS_UMAX`], [`SS_VMAX`] | `SM_SS_*` (`SmTypes.h`) |
//!
//! Methods of `SmExtent2d` that no in-scope upstream code reaches
//! (`harness/callgraph/unused_functions.txt`: `Set`, `SetUMin`..`SetVMax`, `Scale`,
//! the distance methods, `IsPoint`, `IsLine`, `AnyBounds`, `ClassifyPointDir2d`,
//! the periodic disjoint/intersect methods, `ApproximateUnbounded`, ...) are not
//! ported.
//!
//! Deviations:
//! - Out-params become return values (tuples, `Option`, `Result`). Optional
//!   out-params (`ContainsPoint2d`'s distance, `IsPoint2dOnBoundary`'s binormal,
//!   `ContainsPeriodicPoint2d`'s periodic point) are always computed; they have no
//!   side effects.
//! - Status codes: `SM_ERR_INVALID_INPUT` is [`SolidError::InvalidInput`]; `SM_ERR`
//!   is [`SolidError::Upstream`] with code 1001. `Intersect` leaves an `Init()`
//!   extent as the result when it fails; the Rust error carries no result.
//!   `Inversion` may write a partial normalised point before failing; the Rust error
//!   carries none (no in-scope caller reads it).
//! - `IntersectWithInfiniteLine` always returns `SM_SUCCESS`; the Rust method
//!   returns the trimmed interval as an `Option` (`None`: `rbFoundInterval` FALSE).
//! - `SetUInterval`/`SetVInterval` only validate their argument under
//!   `SM_DEBUG_CODE`; in the release build they always succeed, so the Rust methods
//!   return nothing.
//! - The destructor's "undefined" sentinel values have no Rust counterpart.
//! - `SM_ASSERT*` checks and the error-log side effect of `SE(...)` are not ported
//!   (they do not change results in the release build).

use crate::error::{Result, SolidError};
use crate::geom::extent1d::{Extent1d, ExtentPointType};
use crate::geom::extent3d::{BoundaryType, Extent3d};
use crate::geom::tol::{are_same, are_same_to_tol, is_zero_to_tol, tol3d, ScaledZero};
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::sqrt;
use crate::math::{is_infinite_param, max2, min2, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT};

/// Upstream status `SM_ERR`.
const SM_ERR: i32 = 1001;

/// `SM_SS_NONE`: the point is on no boundary ([`Extent2d::point_boundaries`]).
pub const SS_NONE: u32 = 0;
/// `SM_SS_UMIN`: the point is on the `u = UMin` boundary.
pub const SS_UMIN: u32 = 1;
/// `SM_SS_VMIN`: the point is on the `v = VMin` boundary.
pub const SS_VMIN: u32 = 2;
/// `SM_SS_UMAX`: the point is on the `u = UMax` boundary.
pub const SS_UMAX: u32 = 4;
/// `SM_SS_VMAX`: the point is on the `v = VMax` boundary.
pub const SS_VMAX: u32 = 8;

/// `x - d` unless `x` is an unbounded marker (`ExpandAbsolute`/`ExpandRelative`).
fn shrink_unless_infinite(x: f64, d: f64) -> f64 {
    x - if is_infinite_param(x) { 0.0 } else { d }
}

/// `x + d` unless `x` is an unbounded marker.
fn grow_unless_infinite(x: f64, d: f64) -> f64 {
    x + if is_infinite_param(x) { 0.0 } else { d }
}

/// One coordinate of `Inversion`: `num / den`, or `1` / `-1` for a (near) zero
/// denominator.
fn normalise(num: f64, den: f64) -> f64 {
    if !is_zero_to_tol(den, EFF_ZERO_SQRT) {
        num / den
    } else if is_zero_to_tol(num, EFF_ZERO_SQRT) {
        1.0
    } else {
        -1.0
    }
}

/// An axis-aligned rectangle `[min, max]` in the plane (`SmExtent2d`), often a
/// surface's parameter domain.
///
/// A fresh extent ([`Extent2d::new`]) is "inside out" (`min = BIG_DOUBLE`,
/// `max = -BIG_DOUBLE`) so that the first [`Extent2d::add_point`] makes it the point.
/// Unbounded interval ends are stored as `±INFINITE_PARAMETER`.
///
/// `PartialEq` compares the corner points exactly (not upstream `operator==`, which
/// is [`Extent2d::are_same`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Extent2d {
    pub(crate) min: Point2,
    pub(crate) max: Point2,
}

impl Default for Extent2d {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Extent3d> for Extent2d {
    fn from(e: Extent3d) -> Self {
        Self::from_extent3d(&e)
    }
}

impl ScaledZero for Extent2d {
    /// `SmTol::GetScaledZero(const SmExtent2d&)`: `SM_EFF_ZERO * (1 + max |coord|)`
    /// over both corners.
    fn scaled_zero(&self) -> f64 {
        tol3d(EFF_ZERO * (1.0 + max2(self.min.max_dimension(), self.max.max_dimension())))
    }
}

impl Extent2d {
    /// An "inside out" extent ready for [`Extent2d::add_point`] (`SmExtent2d()`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            min: Vec2::new(BIG_DOUBLE, BIG_DOUBLE),
            max: Vec2::new(-BIG_DOUBLE, -BIG_DOUBLE),
        }
    }

    /// The point-sized extent at `p` (`SmExtent2d(rMinAndMax)`).
    #[must_use]
    pub const fn from_point(p: Point2) -> Self {
        Self { min: p, max: p }
    }

    /// The rectangle spanned by two corner points; coordinates are swapped per axis
    /// so that `min <= max` (`SmExtent2d(rMin, rMax)`).
    #[must_use]
    pub fn from_points(a: Point2, b: Point2) -> Self {
        let mut e = Self { min: a, max: b };
        if e.min.x > e.max.x {
            core::mem::swap(&mut e.min.x, &mut e.max.x);
        }
        if e.min.y > e.max.y {
            core::mem::swap(&mut e.min.y, &mut e.max.y);
        }
        e
    }

    /// The rectangle with the given bounds, stored as given (no swapping)
    /// (`SmExtent2d(UMin, VMin, UMax, VMax)`).
    #[must_use]
    pub const fn from_bounds(u_min: f64, v_min: f64, u_max: f64, v_max: f64) -> Self {
        Self {
            min: Vec2::new(u_min, v_min),
            max: Vec2::new(u_max, v_max),
        }
    }

    /// The x/y part of a 3D extent (`SmExtent2d(const SmExtent3d&)`).
    #[must_use]
    pub fn from_extent3d(e: &Extent3d) -> Self {
        let (a, b) = (e.min(), e.max());
        Self {
            min: Vec2::new(a.x, a.y),
            max: Vec2::new(b.x, b.y),
        }
    }

    /// Resets to the "inside out" state of [`Extent2d::new`] (`Init`).
    pub fn init(&mut self) {
        *self = Self::new();
    }

    /// Both corners equal up to their scaled zeros (`operator==`).
    #[must_use]
    pub fn are_same(&self, other: &Self) -> bool {
        are_same(self.min.x, other.min.x)
            && are_same(self.min.y, other.min.y)
            && are_same(self.max.x, other.max.x)
            && are_same(self.max.y, other.max.y)
    }

    /// Grows the extent to cover `p` (`AddPoint2d`).
    pub fn add_point(&mut self, p: Point2) {
        if p.x < self.min.x {
            self.min.x = p.x;
        }
        if p.y < self.min.y {
            self.min.y = p.y;
        }
        if p.x > self.max.x {
            self.max.x = p.x;
        }
        if p.y > self.max.y {
            self.max.y = p.y;
        }
    }

    /// Sets both corners; [`SolidError::InvalidInput`] (extent unchanged) when
    /// `min > max` in either coordinate (`SetMinMax(rMin, rMax)`).
    pub fn set_min_max(&mut self, min: Point2, max: Point2) -> Result<()> {
        if min.x > max.x || min.y > max.y {
            return Err(SolidError::InvalidInput {
                operation: "geom::extent2d::Extent2d::set_min_max",
                reason: "min > max",
            });
        }
        self.min = min;
        self.max = max;
        Ok(())
    }

    /// Sets the four bounds; [`SolidError::InvalidInput`] (extent unchanged) when
    /// `min > max` in either coordinate (`SetMinMax(dMinX, dMinY, dMaxX, dMaxY)`).
    pub fn set_min_max_coords(
        &mut self,
        min_x: f64,
        min_y: f64,
        max_x: f64,
        max_y: f64,
    ) -> Result<()> {
        if min_x > max_x || min_y > max_y {
            return Err(SolidError::InvalidInput {
                operation: "geom::extent2d::Extent2d::set_min_max_coords",
                reason: "min > max",
            });
        }
        self.min = Vec2::new(min_x, min_y);
        self.max = Vec2::new(max_x, max_y);
        Ok(())
    }

    /// Sets the x bounds from `ivl` (`SetUInterval`).
    pub fn set_u_interval(&mut self, ivl: &Extent1d) {
        self.min.x = ivl.min();
        self.max.x = ivl.max();
    }

    /// Sets the y bounds from `ivl` (`SetVInterval`).
    pub fn set_v_interval(&mut self, ivl: &Extent1d) {
        self.min.y = ivl.min();
        self.max.y = ivl.max();
    }

    /// Sets the u minimum; [`SolidError::InvalidInput`] (extent unchanged) when it
    /// exceeds the u maximum (`SetUMin`).
    pub fn set_u_min(&mut self, value: f64) -> Result<()> {
        if value > self.max.x {
            return Err(Self::bad_bound("geom::extent2d::Extent2d::set_u_min"));
        }
        self.min.x = value;
        Ok(())
    }

    /// Sets the u maximum; [`SolidError::InvalidInput`] (extent unchanged) when it
    /// is below the u minimum (`SetUMax`).
    pub fn set_u_max(&mut self, value: f64) -> Result<()> {
        if value < self.min.x {
            return Err(Self::bad_bound("geom::extent2d::Extent2d::set_u_max"));
        }
        self.max.x = value;
        Ok(())
    }

    /// Sets the v minimum; [`SolidError::InvalidInput`] (extent unchanged) when it
    /// exceeds the v maximum (`SetVMin`).
    pub fn set_v_min(&mut self, value: f64) -> Result<()> {
        if value > self.max.y {
            return Err(Self::bad_bound("geom::extent2d::Extent2d::set_v_min"));
        }
        self.min.y = value;
        Ok(())
    }

    /// Sets the v maximum; [`SolidError::InvalidInput`] (extent unchanged) when it
    /// is below the v minimum (`SetVMax`).
    pub fn set_v_max(&mut self, value: f64) -> Result<()> {
        if value < self.min.y {
            return Err(Self::bad_bound("geom::extent2d::Extent2d::set_v_max"));
        }
        self.max.y = value;
        Ok(())
    }

    /// The `SM_ERR_INVALID_INPUT` of the single-bound setters.
    const fn bad_bound(operation: &'static str) -> SolidError {
        SolidError::InvalidInput {
            operation,
            reason: "bound crosses the opposite bound",
        }
    }

    /// Swaps the u and v intervals (`Transpose`).
    pub fn transpose(&mut self) {
        core::mem::swap(&mut self.min.x, &mut self.min.y);
        core::mem::swap(&mut self.max.x, &mut self.max.y);
    }

    /// Moves every bounded end outward by `d` (inward for `d < 0`); unbounded ends
    /// stay (`ExpandAbsolute(double)`).
    pub fn expand_absolute(&mut self, d: f64) -> &mut Self {
        self.expand_absolute_vec(Vec2::new(d, d))
    }

    /// Moves the bounded x ends outward by `d.x` and the y ends by `d.y`
    /// (`ExpandAbsolute(const SmVector2d)`).
    pub fn expand_absolute_vec(&mut self, d: Vec2) -> &mut Self {
        self.min.x = shrink_unless_infinite(self.min.x, d.x);
        self.min.y = shrink_unless_infinite(self.min.y, d.y);
        self.max.x = grow_unless_infinite(self.max.x, d.x);
        self.max.y = grow_unless_infinite(self.max.y, d.y);
        self
    }

    /// Moves every bounded end outward by `factor` times its interval's length
    /// (`ExpandRelative`).
    pub fn expand_relative(&mut self, factor: f64) -> &mut Self {
        let d = (self.max - self.min) * factor;
        self.expand_absolute_vec(d)
    }

    /// Minimum corner (`GetMin`).
    #[must_use]
    pub const fn min(&self) -> Point2 {
        self.min
    }

    /// Centre, `(min + max) / 2` (`GetMid`).
    #[must_use]
    pub fn mid(&self) -> Point2 {
        (self.min + self.max) / 2.0
    }

    /// Maximum corner (`GetMax`).
    #[must_use]
    pub const fn max(&self) -> Point2 {
        self.max
    }

    /// Minimum u (`GetUMin`).
    #[must_use]
    pub const fn u_min(&self) -> f64 {
        self.min.x
    }

    /// Maximum u (`GetUMax`).
    #[must_use]
    pub const fn u_max(&self) -> f64 {
        self.max.x
    }

    /// Minimum v (`GetVMin`).
    #[must_use]
    pub const fn v_min(&self) -> f64 {
        self.min.y
    }

    /// Maximum v (`GetVMax`).
    #[must_use]
    pub const fn v_max(&self) -> f64 {
        self.max.y
    }

    /// The u interval, ends swapped if inverted (`GetUInterval`).
    #[must_use]
    pub fn u_interval(&self) -> Extent1d {
        Extent1d::from_min_max(self.min.x, self.max.x)
    }

    /// The v interval, ends swapped if inverted (`GetVInterval`).
    #[must_use]
    pub fn v_interval(&self) -> Extent1d {
        Extent1d::from_min_max(self.min.y, self.max.y)
    }

    /// Largest absolute coordinate of either corner (`GetMaxDimension`).
    #[must_use]
    pub fn max_dimension(&self) -> f64 {
        max2(self.min.max_dimension(), self.max.max_dimension())
    }

    /// The diagonal `max - min` (`GetSize`).
    #[must_use]
    pub fn size(&self) -> Vec2 {
        Vec2::new(self.max.x - self.min.x, self.max.y - self.min.y)
    }

    /// `max.x - min.x` (`XLength`).
    #[must_use]
    pub fn x_length(&self) -> f64 {
        self.max.x - self.min.x
    }

    /// `max.y - min.y` (`YLength`).
    #[must_use]
    pub fn y_length(&self) -> f64 {
        self.max.y - self.min.y
    }

    /// The smallest extent containing both (`Union`).
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        Self {
            min: Vec2::new(min2(self.min.x, other.min.x), min2(self.min.y, other.min.y)),
            max: Vec2::new(max2(self.max.x, other.max.x), max2(self.max.y, other.max.y)),
        }
    }

    /// The common part of two extents; [`SolidError::InvalidInput`] when they are
    /// disjoint (`Intersect`).
    pub fn intersect(&self, other: &Self) -> Result<Self> {
        if self.are_disjoint(other, 0.0) {
            return Err(SolidError::InvalidInput {
                operation: "geom::extent2d::Extent2d::intersect",
                reason: "disjoint extents",
            });
        }
        Ok(Self {
            min: Vec2::new(max2(self.min.x, other.min.x), max2(self.min.y, other.min.y)),
            max: Vec2::new(min2(self.max.x, other.max.x), min2(self.max.y, other.max.y)),
        })
    }

    /// Largest distance between corresponding bounds (`GetMaxBoundaryDist2d`).
    #[must_use]
    pub fn max_boundary_dist(&self, other: &Self) -> f64 {
        let mut d = 0.0;
        for t in [
            (self.min.x - other.min.x).abs(),
            (self.max.x - other.max.x).abs(),
            (self.min.y - other.min.y).abs(),
            (self.max.y - other.max.y).abs(),
        ] {
            if t > d {
                d = t;
            }
        }
        d
    }

    /// True when the extents are separated by more than `tol`; touching extents are
    /// not disjoint (`AreDisjoint`, upstream default `tol = 0`).
    #[must_use]
    pub fn are_disjoint(&self, other: &Self, tol: f64) -> bool {
        self.min.x > other.max.x + tol
            || self.min.y > other.max.y + tol
            || self.max.x < other.min.x - tol
            || self.max.y < other.min.y - tol
    }

    /// True when all four bounds agree within `|tol|` (`AreEqual`, upstream default
    /// `tol = 0`).
    #[must_use]
    pub fn are_equal(&self, other: &Self, tol: f64) -> bool {
        are_same_to_tol(self.min.x, other.min.x, tol)
            && are_same_to_tol(self.min.y, other.min.y, tol)
            && are_same_to_tol(self.max.x, other.max.x, tol)
            && are_same_to_tol(self.max.y, other.max.y, tol)
    }

    /// True when a side is no longer than `tol` (`IsDegenerate`, upstream default
    /// `tol = 0`).
    #[must_use]
    pub fn is_degenerate(&self, tol: f64) -> bool {
        self.min.x >= self.max.x - tol || self.min.y >= self.max.y - tol
    }

    /// True when an interval is inverted (`HasNegativeArea`).
    #[must_use]
    pub fn has_negative_area(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y
    }

    /// True for the exact "inside out" state of [`Extent2d::new`] (`IsInit`).
    #[must_use]
    pub fn is_init(&self) -> bool {
        self.min.x == BIG_DOUBLE
            && self.min.y == BIG_DOUBLE
            && self.max.x == -BIG_DOUBLE
            && self.max.y == -BIG_DOUBLE
    }

    /// Classification of the u and v intervals (`IsBounded`'s optional output).
    #[must_use]
    pub fn boundary_types(&self) -> [BoundaryType; 2] {
        [
            Extent1d::from_raw(self.min.x, self.max.x).boundary_type(),
            Extent1d::from_raw(self.min.y, self.max.y).boundary_type(),
        ]
    }

    /// True when no end is an unbounded marker (`IsBounded`).
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        self.boundary_types() == [BoundaryType::Bounded, BoundaryType::Bounded]
    }

    /// True when `self` lies inside `other` enlarged by `tol` (`IsContainedBy`,
    /// upstream default `tol = 0`).
    #[must_use]
    pub fn is_contained_by(&self, other: &Self, tol: f64) -> bool {
        !(self.min.x < other.min.x - tol
            || self.min.y < other.min.y - tol
            || self.max.x > other.max.x + tol
            || self.max.y > other.max.y + tol)
    }

    /// True when `p` lies in the extent enlarged by `tol` (`ContainsPoint2d` without
    /// the distance output; upstream default `tol = 0`).
    #[must_use]
    pub fn contains_point(&self, p: Point2, tol: f64) -> bool {
        !(p.x + tol < self.min.x
            || p.x - tol > self.max.x
            || p.y + tol < self.min.y
            || p.y - tol > self.max.y)
    }

    /// [`Extent2d::contains_point`] plus, for points outside, the distance to the
    /// extent (0 for points inside) (`ContainsPoint2d` with `pOptMinOutsideDist`).
    #[must_use]
    pub fn contains_point_dist(&self, p: Point2, tol: f64) -> (bool, f64) {
        let mut inside = true;
        let mut d2 = 0.0;
        if p.x + tol < self.min.x {
            inside = false;
            d2 += (self.min.x - p.x) * (self.min.x - p.x);
        } else if p.x - tol > self.max.x {
            inside = false;
            d2 += (p.x - self.max.x) * (p.x - self.max.x);
        }
        if p.y + tol < self.min.y {
            inside = false;
            d2 += (self.min.y - p.y) * (self.min.y - p.y);
        } else if p.y - tol > self.max.y {
            inside = false;
            d2 += (p.y - self.max.y) * (p.y - self.max.y);
        }
        (inside, sqrt(d2))
    }

    /// Classifies each coordinate of `p` against its interval, with boundary
    /// tolerance `tol_u` for u and `tol_v` (default `tol_u`) for v
    /// (`ClassifyPoint2d`; upstream default `tol_u = SM_EFF_ZERO`).
    #[must_use]
    pub fn classify_point(
        &self,
        p: Point2,
        tol_u: f64,
        tol_v: Option<f64>,
    ) -> (ExtentPointType, ExtentPointType) {
        let tol_v = tol_v.unwrap_or(tol_u);
        (
            Extent1d::from_raw(self.min.x, self.max.x).classify_point(p.x, tol_u),
            Extent1d::from_raw(self.min.y, self.max.y).classify_point(p.y, tol_v),
        )
    }

    /// Whether `p` is within `tol` of a boundary, and a vector pointing inward from
    /// the (first) boundary found per coordinate (`IsPoint2dOnBoundary`; upstream
    /// default `tol = SM_EFF_ZERO`).
    #[must_use]
    pub fn is_point_on_boundary(&self, p: Point2, tol: f64) -> (bool, Vec2) {
        let mut on = false;
        let (mut du, mut dv) = (0.0, 0.0);
        if (p.x - self.min.x).abs() <= tol {
            on = true;
            du = 1.0;
        } else if (p.x - self.max.x).abs() <= tol {
            on = true;
            du = -1.0;
        }
        if (p.y - self.min.y).abs() <= tol {
            on = true;
            dv = 1.0;
        } else if (p.y - self.max.y).abs() <= tol {
            on = true;
            dv = -1.0;
        }
        (on, Vec2::new(du, dv))
    }

    /// Bit set of the boundaries within `tol` of `p`: [`SS_NONE`] or an OR of
    /// [`SS_UMIN`], [`SS_VMIN`], [`SS_UMAX`], [`SS_VMAX`] (`GetPoint2dBoundaries`;
    /// upstream default `tol = SM_EFF_ZERO`).
    #[must_use]
    pub fn point_boundaries(&self, p: Point2, tol: f64) -> u32 {
        let mut b = SS_NONE;
        if (p.x - self.min.x).abs() <= tol {
            b |= SS_UMIN;
        }
        if (p.y - self.min.y).abs() <= tol {
            b |= SS_VMIN;
        }
        if (p.x - self.max.x).abs() <= tol {
            b |= SS_UMAX;
        }
        if (p.y - self.max.y).abs() <= tol {
            b |= SS_VMAX;
        }
        b
    }

    /// Whether `p` lies in the extent when u and/or v are periodic (period `0`: not
    /// periodic), with the point mapped to the extent's period
    /// (`ContainsPeriodicPoint2d`; `tol <= 0` means `SM_EFF_ZERO_DEG`, upstream
    /// default `tol = 0`).
    #[must_use]
    pub fn contains_periodic_point(
        &self,
        p: Point2,
        u_period: f64,
        v_period: f64,
        tol: f64,
    ) -> (bool, Point2) {
        let u = self
            .u_interval()
            .contains_periodic_value(p.x, u_period, tol);
        let v = self
            .v_interval()
            .contains_periodic_value(p.y, v_period, tol);
        (
            u.contained && v.contained,
            Vec2::new(u.periodic_value, v.periodic_value),
        )
    }

    /// The point at normalised coordinates `(nx, ny)` (`(0, 0)` = min, `(1, 1)` =
    /// max), clamped to the extent (`Evaluate`).
    #[must_use]
    pub fn evaluate(&self, nx: f64, ny: f64) -> Point2 {
        self.clamp_point(Vec2::new(
            Extent1d::from_raw(self.min.x, self.max.x).evaluate(nx),
            Extent1d::from_raw(self.min.y, self.max.y).evaluate(ny),
        ))
    }

    /// Normalised coordinates of `p` (snapped to the boundary when within `tol` of
    /// it, clamped to `[0, 1]`); [`SolidError::Upstream`] `SM_ERR` when `p` is
    /// outside or a side is degenerate and `p` is off it (`Inversion`; upstream
    /// default `tol = SM_EFF_ZERO`).
    pub fn inversion(&self, p: Point2, tol: f64) -> Result<Point2> {
        let err = || SolidError::Upstream {
            operation: "geom::extent2d::Extent2d::inversion",
            code: SM_ERR,
            entity: None,
        };
        let mut pnt = p;
        if self.is_point_on_boundary(pnt, tol).0 {
            pnt = self.clamp_point(pnt);
        }
        if !self.contains_point(pnt, tol) {
            return Err(err());
        }
        let n = Vec2::new(
            normalise(pnt.x - self.min.x, self.max.x - self.min.x),
            normalise(pnt.y - self.min.y, self.max.y - self.min.y),
        );
        if n.x == -1.0 || n.y == -1.0 {
            return Err(err());
        }
        Ok(Self::from_bounds(0.0, 0.0, 1.0, 1.0).clamp_point(n))
    }

    /// `p` with each coordinate clamped to its interval (`ClampPoint2d`).
    #[must_use]
    pub fn clamp_point(&self, p: Point2) -> Point2 {
        let mut r = p;
        if p.x < self.min.x {
            r.x = self.min.x;
        }
        if p.y < self.min.y {
            r.y = self.min.y;
        }
        if p.x > self.max.x {
            r.x = self.max.x;
        }
        if p.y > self.max.y {
            r.y = self.max.y;
        }
        r
    }

    /// Clips the segment `p + s v`, `s` from 0 to `t`: returns `t` when its end
    /// point is inside, else the parameter of the first boundary crossing; `0` when
    /// `p` is on a boundary and the segment points out; `BIG_DOUBLE` when the
    /// segment cannot be clipped (`ClipLine2d`).
    #[must_use]
    pub fn clip_line(&self, p: Point2, v: Vec2, t: f64) -> f64 {
        let sign = if t < 0.0 { -1.0 } else { 1.0 };
        let out_tol = EFF_ZERO * 100.0;
        if (are_same(self.min.x, p.x) && v.x * sign < -out_tol)
            || (are_same(self.min.y, p.y) && v.y * sign < -out_tol)
            || (are_same(self.max.x, p.x) && v.x * sign > out_tol)
            || (are_same(self.max.y, p.y) && v.y * sign > out_tol)
        {
            return 0.0;
        }

        let mut r = t;
        let ps_tol = EFF_ZERO_SQRT / 10000.00;
        let mut end = p + r * v;
        if self.contains_point(end, ps_tol) {
            return r;
        }

        if end.x < self.min.x - ps_tol {
            if (end.x - p.x).abs() < EFF_ZERO * (self.min.x - p.x).abs() {
                return BIG_DOUBLE;
            }
            r = r * (self.min.x - p.x) / (end.x - p.x);
            end = p + r * v;
        } else if end.x > self.max.x + ps_tol {
            if (end.x - p.x).abs() < EFF_ZERO * (self.max.x - p.x).abs() {
                return BIG_DOUBLE;
            }
            r = r * (self.max.x - p.x) / (end.x - p.x);
            end = p + r * v;
        }

        if end.y < self.min.y - ps_tol {
            if (end.y - p.y).abs() < EFF_ZERO * (self.min.y - p.y).abs() {
                return BIG_DOUBLE;
            }
            r = r * (self.min.y - p.y) / (end.y - p.y);
        } else if end.y > self.max.y + ps_tol {
            if (end.y - p.y).abs() < EFF_ZERO * (self.max.y - p.y).abs() {
                return BIG_DOUBLE;
            }
            r = r * (self.max.y - p.y) / (end.y - p.y);
        }
        r
    }

    /// The parameter interval of the infinite line `p + s v` inside the extent
    /// enlarged by `tol_u` / `tol_v` (defaults `100 SM_EFF_ZERO` and `tol_u`);
    /// `None` when the line misses it (`IntersectWithInfiniteLine`).
    #[must_use]
    pub fn intersect_with_infinite_line(
        &self,
        p: Point2,
        v: Vec2,
        tol_u: Option<f64>,
        tol_v: Option<f64>,
    ) -> Option<Extent1d> {
        let mut ivl = Extent1d::new();
        let tol_u = tol_u.unwrap_or(EFF_ZERO * 100.0);
        let tol_v = tol_v.unwrap_or(tol_u);
        let min_x = self.min.x - tol_u;
        let max_x = self.max.x + tol_u;
        let min_y = self.min.y - tol_v;
        let max_y = self.max.y + tol_v;

        if v.x.abs() > EFF_ZERO {
            for bound in [self.min.x, self.max.x] {
                let prm = (bound - p.x) / v.x;
                let int = p.y + prm * v.y;
                if int >= min_y && int <= max_y {
                    ivl.add_value(prm);
                }
            }
        }
        if v.y.abs() > EFF_ZERO {
            for bound in [self.min.y, self.max.y] {
                let prm = (bound - p.y) / v.y;
                let int = p.x + prm * v.x;
                if int >= min_x && int <= max_x {
                    ivl.add_value(prm);
                }
            }
        }

        (ivl.max() > ivl.min() - max2(tol_u, tol_v)).then_some(ivl)
    }

    /// True when `u` is in the lower half of the u interval (`GetULeftEval`).
    #[must_use]
    pub fn u_left_eval(&self, u: f64) -> bool {
        let mid = (self.min.x + self.max.x) / 2.0;
        // upstream: TRUE unless `u > mid` (so NaN gives TRUE)
        !matches!(u.partial_cmp(&mid), Some(core::cmp::Ordering::Greater))
    }

    /// True when `v` is in the lower half of the v interval (`GetVLeftEval`).
    #[must_use]
    pub fn v_left_eval(&self, v: f64) -> bool {
        let mid = (self.min.y + self.max.y) / 2.0;
        !matches!(v.partial_cmp(&mid), Some(core::cmp::Ordering::Greater))
    }

    /// Maps `p` from this domain to `other` through normalised coordinates,
    /// swapping u and v when `switch_params`; [`SolidError::Upstream`] `SM_ERR` when
    /// [`Extent2d::inversion`] fails (`Point2dMapToDomain`).
    pub fn point_map_to_domain(
        &self,
        p: Point2,
        other: &Self,
        switch_params: bool,
    ) -> Result<Point2> {
        let n = self
            .inversion(p, EFF_ZERO)
            .map_err(|_| SolidError::Upstream {
                operation: "geom::extent2d::Extent2d::point_map_to_domain",
                code: SM_ERR,
                entity: None,
            })?;
        Ok(if switch_params {
            other.evaluate(n.y, n.x)
        } else {
            other.evaluate(n.x, n.y)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::INFINITE_PARAMETER;

    fn p(x: f64, y: f64) -> Point2 {
        Vec2::new(x, y)
    }

    /// `my_test_extent2d` (`tests/prog_test/src/smCurveTest.cpp:2688`), owned parts.
    #[test]
    fn upstream_my_test_extent2d() {
        let e1 = Extent2d::from_points(p(1.0, 2.0), p(3.0, 4.0));
        assert_eq!(e1.min(), p(1.0, 2.0));
        assert_eq!(e1.max(), p(3.0, 4.0));
        let e3 = Extent2d::from_bounds(11.0, 12.0, 13.0, 14.0);
        assert_eq!((e3.min(), e3.max()), (p(11.0, 12.0), p(13.0, 14.0)));
        let e4 = Extent2d::from_point(p(1.0, 2.0));
        assert_eq!(e4.max(), p(1.0, 2.0));
        let x3 = Extent3d::from_bounds(1.0, 2.0, 3.0, 4.0, 5.0, 6.0);
        assert_eq!(Extent2d::from(x3).max(), p(4.0, 5.0));
        let mut e2 = Extent2d::new();
        assert!(e2.is_init());

        e2 = e1;
        e2.add_point(p(5.0, 6.0));
        assert_eq!(e2.max(), p(5.0, 6.0));
        e2.set_min_max_coords(1.0, 2.0, 3.0, 4.0).unwrap();
        e2.set_min_max(p(1.0, 2.0), p(3.0, 4.0)).unwrap();
        assert_eq!(e2.min(), p(1.0, 2.0));

        let e6 = Extent2d::from_bounds(-3.0, -2.0, 0.0, 1.0);
        assert!(!e1.are_disjoint(&e6, 2.0));
        assert!(e1.are_disjoint(&e6, 0.0));

        let e6 = Extent2d::from_bounds(-4.0, -2.0, -3.0, -1.0);
        let u = e1.union(&e6);
        assert_eq!((u.min(), u.max()), (p(-4.0, -2.0), p(3.0, 4.0)));
        let i = e1
            .intersect(&Extent2d::from_bounds(0.0, 0.0, 4.0, 3.0))
            .unwrap();
        assert_eq!(i.max(), p(3.0, 3.0));
        assert!(e1.intersect(&e6).is_err());

        e2.set_u_interval(&Extent1d::from_min_max(7.0, 8.0));
        e2.set_v_interval(&Extent1d::from_min_max(4.0, 5.0));
        assert_eq!((e2.min(), e2.max()), (p(7.0, 4.0), p(8.0, 5.0)));

        let mut e2 = Extent2d::from_bounds(2.0, 3.0, 5.0, 6.0);
        e2.transpose();
        assert_eq!(e2.min(), p(3.0, 2.0));
        e2.expand_absolute(1.0);
        assert_eq!(e2.min(), p(2.0, 1.0));
        e2.expand_relative(1.0);
        assert_eq!((e2.min(), e2.max()), (p(-3.0, -4.0), p(12.0, 11.0)));
        assert_eq!(e2.size(), p(15.0, 15.0));
        assert_eq!(e2.u_interval(), Extent1d::from_min_max(-3.0, 12.0));
        assert_eq!(e2.v_interval(), Extent1d::from_min_max(-4.0, 11.0));
        assert_eq!((e2.x_length(), e2.y_length()), (15.0, 15.0));
        assert_eq!(e2.max_dimension(), 12.0);

        let e2 = Extent2d::from_bounds(1.0, 2.1, 3.0, 4.0);
        assert!(e1.are_equal(&e2, 0.2));
        assert!(!e1.are_equal(&e2, 0.0));
        assert!(e1.are_same(&e1));

        let e2 = Extent2d::from_bounds(4.0, 5.0, 4.1, 5.1);
        assert!(e2.is_degenerate(0.2));
        assert!(!e2.is_degenerate(0.0));
        let mut e2 = Extent2d::from_bounds(0.0, 0.0, 0.0, 0.0);
        e2.expand_absolute(-0.1);
        assert!(e2.has_negative_area());

        let mut e2 = e1;
        e2.expand_absolute(0.01);
        assert!(e2.is_contained_by(&e1, 0.1));
        assert!(!e2.is_contained_by(&e1, 0.0));

        assert!(e1.contains_point(p(0.9, 3.0), 0.2));
        assert!(!e1.contains_point(p(0.9, 3.0), 0.0));

        let e2 = Extent2d::from_bounds(4.0, 5.0, 4.1, 5.1);
        let (cu, cv) = e2.classify_point(p(1.1, 2.1), 0.2, None);
        assert_eq!(
            (cu, cv),
            (ExtentPointType::Outside, ExtentPointType::Outside)
        );
        let (cu, cv) = e1.classify_point(p(1.1, 2.1), 0.2, None);
        assert_eq!((cu, cv), (ExtentPointType::Start, ExtentPointType::Start));
        let (cu, cv) = e1.classify_point(p(1.1, 2.1), EFF_ZERO, None);
        assert_eq!((cu, cv), (ExtentPointType::Inside, ExtentPointType::Inside));

        assert_eq!(
            e1.is_point_on_boundary(p(1.1, 2.1), 0.2),
            (true, p(1.0, 1.0))
        );
        assert!(!e1.is_point_on_boundary(p(1.1, 2.1), EFF_ZERO).0);

        assert_eq!(
            e1.contains_periodic_point(p(8.9, 3.0), 4.0, 0.0, 0.2),
            (true, p(1.0, 3.0))
        );
        assert!(!e1.contains_periodic_point(p(8.9, 3.0), 4.0, 0.0, 0.0).0);

        assert_eq!(e1.evaluate(0.5, 0.5), p(2.0, 3.0));
        assert_eq!(e1.inversion(p(2.0, 3.0), 0.1).unwrap(), p(0.5, 0.5));
        assert_eq!(e1.inversion(p(2.0, 3.0), EFF_ZERO).unwrap(), p(0.5, 0.5));
        assert_eq!(e1.clamp_point(p(0.0, 0.0)), p(1.0, 2.0));
        assert_eq!(e1.clip_line(p(2.0, 3.0), p(0.0, 1.0), 4.0), 1.0);

        let ivl = e1
            .intersect_with_infinite_line(p(0.0, 3.1), p(1.0, 1.0), Some(0.2), None)
            .unwrap();
        assert!((ivl.min() - 0.9).abs() <= EFF_ZERO);
        assert_eq!(ivl.max(), 1.0);
        assert!(e1
            .intersect_with_infinite_line(p(0.0, 3.1), p(1.0, 1.0), None, None)
            .is_none());

        assert!(!e1.u_left_eval(2.5));
        assert!(e1.v_left_eval(2.5));

        let e2 = Extent2d::from_bounds(4.0, 0.0, 10.0, 8.0);
        assert_eq!(
            e1.point_map_to_domain(p(2.0, 3.0), &e2, false).unwrap(),
            p(7.0, 4.0)
        );
    }

    #[test]
    fn set_min_max_rejects_inverted() {
        let mut e = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        assert!(e.set_min_max(p(2.0, 0.0), p(1.0, 1.0)).is_err());
        assert!(e.set_min_max_coords(0.0, 2.0, 1.0, 1.0).is_err());
        assert_eq!(e, Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0));
    }

    #[test]
    fn unbounded_ends_do_not_expand() {
        let mut e = Extent2d::from_bounds(-INFINITE_PARAMETER, 0.0, 1.0, INFINITE_PARAMETER);
        assert_eq!(
            e.boundary_types(),
            [BoundaryType::UnboundedMin, BoundaryType::UnboundedMax]
        );
        assert!(!e.is_bounded());
        e.expand_absolute(1.0);
        assert_eq!(
            e,
            Extent2d::from_bounds(-INFINITE_PARAMETER, -1.0, 2.0, INFINITE_PARAMETER)
        );
    }

    #[test]
    fn point_boundaries_and_distance() {
        let e = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        assert_eq!(e.point_boundaries(p(0.0, 1.0), EFF_ZERO), SS_UMIN | SS_VMAX);
        assert_eq!(e.point_boundaries(p(0.5, 0.5), EFF_ZERO), SS_NONE);
        assert_eq!(e.contains_point_dist(p(4.0, 5.0), 0.0), (false, 5.0));
        assert_eq!(e.contains_point_dist(p(0.5, 0.5), 0.0), (true, 0.0));
    }

    #[test]
    fn inversion_of_degenerate_side() {
        let e = Extent2d::from_bounds(0.0, 0.0, 0.0, 1.0);
        assert_eq!(e.inversion(p(0.0, 0.5), EFF_ZERO).unwrap(), p(1.0, 0.5));
        assert!(e.inversion(p(2.0, 0.5), EFF_ZERO).is_err());
    }

    #[test]
    fn scaled_zero_of_extent() {
        let e = Extent2d::from_bounds(-3.0, 1.0, 2.0, 1.0);
        assert_eq!(e.scaled_zero(), EFF_ZERO * 4.0);
    }
}
