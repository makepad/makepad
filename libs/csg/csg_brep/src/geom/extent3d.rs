//! Axis-aligned 3D extents (bounding boxes): construction, growth, set operations,
//! distances, containment and classification predicates, line clipping, and
//! approximation of unbounded extents.
//!
//! Upstream: `source/SMLib/src/SmExtent3d.cpp`, `source/SMLib/inc/SmExtent3d.h`.
//!
//! | Rust                                   | Upstream                                       |
//! |----------------------------------------|------------------------------------------------|
//! | [`Extent3d`]                           | `SmExtent3d`                                   |
//! | [`Extent3d::new`], [`Default`]         | `SmExtent3d()`, `Init`                         |
//! | [`Extent3d::init`]                     | `Init`                                         |
//! | [`Extent3d::from_points`]              | `SmExtent3d(const SmPoint3d&, const SmPoint3d&)` |
//! | [`Extent3d::from_point`]               | `SmExtent3d(const SmPoint3d&)`                 |
//! | [`Extent3d::from_bounds`]              | `SmExtent3d(double x6)`                        |
//! | `Clone`/`Copy`                         | copy constructor, `operator=`, destructor      |
//! | [`Extent3d::is_defined`]               | `AssertDefined`                                |
//! | [`Extent3d::set_unbounded`]            | `SetUnbounded`                                 |
//! | [`Extent3d::min`], [`Extent3d::max`], [`Extent3d::mid`] | `GetMin()`, `GetMax()`, `GetMid()` |
//! | [`Extent3d::u_min`] .. [`Extent3d::w_max`] | `GetUMin` .. `GetWMax`                     |
//! | [`Extent3d::min_at`], [`Extent3d::max_at`] | `GetMin(ULONG)`, `GetMax(ULONG)`           |
//! | [`Extent3d::max_length`]               | `GetMaxLength`                                 |
//! | [`Extent3d::max_dimension`]            | `GetMaxDimension`                              |
//! | [`Extent3d::add_point`]                | `AddPoint3d`                                   |
//! | [`Extent3d::set_min_max`]              | `SetMinMax(const SmPoint3d&, const SmPoint3d&)` |
//! | [`Extent3d::set_min_max_coords`]       | `SetMinMax(double x6)`                         |
//! | [`Extent3d::set_u_min`] .. [`Extent3d::set_v_max`] | `SetUMin`, `SetUMax`, `SetVMin`, `SetVMax` |
//! | [`Extent3d::evaluate`]                 | `Evaluate`                                     |
//! | [`Extent3d::union`]                    | `Union`                                        |
//! | [`Extent3d::intersect`]                | `Intersect`                                    |
//! | [`Extent3d::max_distance_squared`]     | `MaximumDistanceSquared`                       |
//! | [`Extent3d::min_distance_squared`]     | `MinimumDistanceSquared`                       |
//! | [`Extent3d::max_distance`]             | `MaximumDistance`                              |
//! | [`Extent3d::min_distance`]             | `MinimumDistance`                              |
//! | [`Extent3d::expand_relative`]          | `ExpandRelative`                               |
//! | [`Extent3d::expand_absolute`]          | `ExpandAbsolute`                               |
//! | [`Extent3d::size`]                     | `GetSize`                                      |
//! | [`Extent3d::corners`]                  | `GetCorners`                                   |
//! | [`Extent3d::edges`]                    | `GetEdges`                                     |
//! | [`Extent3d::planes`]                   | `GetPlanes`                                    |
//! | [`Extent3d::x_length`] .. [`Extent3d::z_length`] | `XLength`, `YLength`, `ZLength`      |
//! | [`Extent3d::sphere_bound`]             | `ComputeSphereBound`                           |
//! | [`Extent3d::distance_to_plane`]        | `DistanceToPlane`                              |
//! | [`Extent3d::cone_bound_to`]            | `ComputeConeBoundTo`                           |
//! | [`Extent3d::are_disjoint`]             | `AreDisjoint`                                  |
//! | [`Extent3d::is_contained_by`]          | `IsContainedBy`                                |
//! | [`Extent3d::is_point_sized`]           | `IsPointSized`                                 |
//! | [`Extent3d::contains_point`]           | `ContainsPoint3d`                              |
//! | [`Extent3d::contains_line_seg`]        | `ContainsLineSeg3d`                            |
//! | [`Extent3d::is_point_on_boundary`]     | `IsPoint3dOnBoundary`                          |
//! | [`Extent3d::intersect_line`], [`LineHit`] | `IntersectLine`                             |
//! | [`Extent3d::has_negative_volume`]      | `HasNegativeVolume`                            |
//! | [`Extent3d::is_init`]                  | `IsInit`                                       |
//! | [`Extent3d::boundary_types`], [`Extent3d::is_bounded`] | `IsBounded`                    |
//! | [`Extent3d::any_bounds`]               | `AnyBounds`                                    |
//! | [`Extent3d::u_interval`], [`Extent3d::v_interval`], [`Extent3d::w_interval`] | `GetUInterval`, `GetVInterval`, `GetWInterval` |
//! | [`Extent3d::set_u_interval`], [`Extent3d::set_v_interval`] | `SetUInterval`, `SetVInterval` |
//! | [`Extent3d::approximate_unbounded`], [`Extent3d::approximate_unbounded_expanded`] | `ApproximateUnbounded` |
//! | [`Extent3d::from_pseudo_box`]          | `SmExtent3d(const SmPseudoBox&)`               |
//! | [`Extent3d::circumscribe`]             | `Circumscribe`                                 |
//! | [`BoundaryType`]                       | `SmBoundaryType` (`SmTypes.h`)                 |
//!
//! Deviations:
//! - Out-params become return values ([`LineHit`], [`SphereBound`], tuples, arrays).
//!   `GetCorners`/`GetEdges`/`GetPlanes` return fixed-size arrays.
//! - `IntersectLine` leaves `rdTEnter`/`rdTExit` unset when nothing is found; the
//!   [`LineHit`] variants carry only the parameters upstream sets.
//! - `ComputeSphereBound`'s optional inner radius is always computed.
//! - `ApproximateUnbounded`'s optional expanded extent is returned as an `Option`:
//!   `None` where upstream's `SetMinMax` rejects it and leaves the caller's extent
//!   untouched.
//! - Status codes: `SM_ERR_INVALID_INPUT` is [`SolidError::InvalidInput`]; the
//!   `SM_ERR` of `ComputeConeBoundTo` is [`SolidError::Upstream`] with code 1001.
//! - `SetUInterval`/`SetVInterval` only validate their argument under
//!   `SM_DEBUG_CODE`; in the release build they always succeed, so the Rust methods
//!   return nothing.
//! - The destructor's "undefined" sentinel values have no Rust counterpart (a dropped
//!   value cannot be used); [`Extent3d::is_defined`] still checks for them.
//! - `SM_ASSERT*` debug checks are not ported (they do not change results).
//! - Upstream bug kept: `ContainsLineSeg3d` returns true when *either* end point is
//!   inside (it ORs the two tests). See `plan/findings.md`.
//! - Upstream quirk kept: `Circumscribe` of an axis-aligned pseudo box reads the
//!   `IsAxisAligned` map (basis -> axis) as if it were axis -> basis, so a basis that is
//!   a non-involutive permutation of the axes (e.g. `y, z, x`) gets its intervals on the
//!   wrong axes. See `plan/findings.md`.

use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::vector3d::{asin, sqrt};
use crate::math::{
    is_infinite_param, max2, max3, min2, min3, Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ,
    INFINITE_PARAMETER, PI, UNDEF_DOUBLE,
};

/// Upstream status `SM_ERR`.
const SM_ERR: i32 = 1001;

/// Classification of one interval of an extent (`SmBoundaryType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoundaryType {
    /// `SM_BT_UNKNOWN`: not classified yet.
    Unknown,
    /// `SM_BT_BOUNDED`: bounded below and above.
    Bounded,
    /// `SM_BT_UNBOUNDED_MIN`: the minimum is `-INFINITE_PARAMETER`.
    UnboundedMin,
    /// `SM_BT_UNBOUNDED_MAX`: the maximum is `INFINITE_PARAMETER`.
    UnboundedMax,
    /// `SM_BT_UNBOUNDED`: unbounded in both directions.
    Unbounded,
}

impl BoundaryType {
    /// Upstream enum value (`SM_BT_UNKNOWN` = 0 .. `SM_BT_UNBOUNDED` = 4).
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::Unknown => 0,
            Self::Bounded => 1,
            Self::UnboundedMin => 2,
            Self::UnboundedMax => 3,
            Self::Unbounded => 4,
        }
    }

    /// Classifies the interval `[min, max]` (the per-axis expression of `IsBounded`).
    fn classify(min: f64, max: f64) -> Self {
        if is_infinite_param(max) && is_infinite_param(min) {
            Self::Unbounded
        } else if is_infinite_param(min) {
            Self::UnboundedMin
        } else if is_infinite_param(max) {
            Self::UnboundedMax
        } else {
            Self::Bounded
        }
    }
}

/// Result of [`Extent3d::intersect_line`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineHit {
    /// No intersection (upstream count 0).
    Miss,
    /// The line grazes the box at one parameter (upstream count 1,
    /// `rdTEnter == rdTExit == t`).
    Touch {
        /// Line parameter of the touching point.
        t: f64,
    },
    /// A portion of the line is inside the box (upstream count 2).
    Through {
        /// Parameter where the line enters the box.
        enter: f64,
        /// Parameter where the line leaves the box.
        exit: f64,
    },
}

impl LineHit {
    /// Upstream `rlNumFound` (0, 1 or 2).
    #[must_use]
    pub const fn count(self) -> u32 {
        match self {
            Self::Miss => 0,
            Self::Touch { .. } => 1,
            Self::Through { .. } => 2,
        }
    }
}

/// Result of [`Extent3d::sphere_bound`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SphereBound {
    /// Centre of the extent (and of both spheres).
    pub center: Point3,
    /// Radius of the sphere that contains the whole extent.
    pub radius: f64,
    /// Radius of the largest centred sphere inside the extent (smallest half side).
    pub min_radius: f64,
}

/// An axis-aligned 3D box `[min, max]` (`SmExtent3d`), often a bounding box.
///
/// A fresh extent ([`Extent3d::new`]) is "inside out" (`min = BIG_DOUBLE`,
/// `max = -BIG_DOUBLE`) so that the first [`Extent3d::add_point`] makes it the point.
/// Unbounded interval ends are stored as `±INFINITE_PARAMETER`.
///
/// `PartialEq` compares the corner points exactly (not upstream `operator==`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Extent3d {
    pub(crate) min: Point3,
    pub(crate) max: Point3,
}

impl Default for Extent3d {
    fn default() -> Self {
        Self::new()
    }
}

/// `x - d` unless `x` is an unbounded marker (`ExpandAbsolute`/`ExpandRelative`).
fn shrink_unless_infinite(x: f64, d: f64) -> f64 {
    x - if is_infinite_param(x) { 0.0 } else { d }
}

/// `x + d` unless `x` is an unbounded marker.
fn grow_unless_infinite(x: f64, d: f64) -> f64 {
    x + if is_infinite_param(x) { 0.0 } else { d }
}

/// `IntersectLine`'s slab parameters `(tmin, tmax)` along one axis.
fn slab(min: f64, max: f64, p: f64, v: f64) -> (f64, f64) {
    if v > EFF_ZERO {
        let lo = if is_infinite_param(min) {
            -INFINITE_PARAMETER
        } else {
            (min - p) / v
        };
        let hi = if is_infinite_param(max) {
            INFINITE_PARAMETER
        } else {
            (max - p) / v
        };
        (lo, hi)
    } else if v < -EFF_ZERO {
        let hi = if is_infinite_param(min) {
            INFINITE_PARAMETER
        } else {
            (min - p) / v
        };
        let lo = if is_infinite_param(max) {
            -INFINITE_PARAMETER
        } else {
            (max - p) / v
        };
        (lo, hi)
    } else {
        (-BIG_DOUBLE, BIG_DOUBLE)
    }
}

/// One axis of `ApproximateUnbounded`: the replacement `(min, max)` of an interval
/// classified as `ty`. `center` is the caller's optional centre, `c` its value or 0.
/// The max is computed from the already replaced min, as upstream does.
fn approx_axis(
    ty: BoundaryType,
    min: f64,
    max: f64,
    center: Option<f64>,
    c: f64,
    h: f64,
) -> (f64, f64) {
    // Upstream: `(pUnboundedCenter == NULL || <cmp>) ? <near> : <centred>`; the same
    // comparisons are used so NaN inputs take the same branch.
    let max_near_center = center.is_none_or(|pc| max < pc + h);
    let min_near_center = center.is_none_or(|pc| min > pc - h);
    let new_min = match (ty, center) {
        (BoundaryType::UnboundedMin, Some(pc)) if !max_near_center => pc - h,
        (BoundaryType::UnboundedMin, _) => max - 2.0 * h,
        (BoundaryType::UnboundedMax, Some(pc)) if !min_near_center => pc - h,
        (BoundaryType::Unbounded, _) => c - h,
        (BoundaryType::UnboundedMax | BoundaryType::Bounded | BoundaryType::Unknown, _) => min,
    };
    // the max of an interval unbounded above is computed from the replaced min
    let new_min_near_center = center.is_none_or(|pc| new_min > pc - h);
    let new_max = match (ty, center) {
        (BoundaryType::UnboundedMin, Some(pc)) if !max_near_center => pc + h,
        (BoundaryType::UnboundedMax, Some(pc)) if !new_min_near_center => pc + h,
        (BoundaryType::UnboundedMax, _) => new_min + 2.0 * h,
        (BoundaryType::Unbounded, _) => c + h,
        (BoundaryType::UnboundedMin | BoundaryType::Bounded | BoundaryType::Unknown, _) => max,
    };
    (new_min, new_max)
}

impl Extent3d {
    /// An "inside out" extent ready for [`Extent3d::add_point`] (`SmExtent3d()`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            min: Vec3::new(BIG_DOUBLE, BIG_DOUBLE, BIG_DOUBLE),
            max: Vec3::new(-BIG_DOUBLE, -BIG_DOUBLE, -BIG_DOUBLE),
        }
    }

    /// The box spanned by two corner points; coordinates are swapped per axis so
    /// that `min <= max` (`SmExtent3d(rMin, rMax)`).
    #[must_use]
    pub fn from_points(a: Point3, b: Point3) -> Self {
        let mut e = Self { min: a, max: b };
        if e.min.x > e.max.x {
            core::mem::swap(&mut e.min.x, &mut e.max.x);
        }
        if e.min.y > e.max.y {
            core::mem::swap(&mut e.min.y, &mut e.max.y);
        }
        if e.min.z > e.max.z {
            core::mem::swap(&mut e.min.z, &mut e.max.z);
        }
        e
    }

    /// The point-sized box at `p` (`SmExtent3d(rMinAndMax)`).
    #[must_use]
    pub const fn from_point(p: Point3) -> Self {
        Self { min: p, max: p }
    }

    /// The box with the given bounds, stored as given (no swapping)
    /// (`SmExtent3d(UMin, VMin, WMin, UMax, VMax, WMax)`).
    #[must_use]
    pub const fn from_bounds(
        u_min: f64,
        v_min: f64,
        w_min: f64,
        u_max: f64,
        v_max: f64,
        w_max: f64,
    ) -> Self {
        Self {
            min: Vec3::new(u_min, v_min, w_min),
            max: Vec3::new(u_max, v_max, w_max),
        }
    }

    /// The box circumscribing `pb` (`SmExtent3d(const SmPseudoBox&)`); see
    /// [`Extent3d::circumscribe`].
    #[must_use]
    pub fn from_pseudo_box(pb: &PseudoBox) -> Self {
        let mut e = Self::new();
        e.circumscribe(pb);
        e
    }

    /// Becomes the box circumscribing `pb` (`Circumscribe`): the same size for an
    /// axis-aligned pseudo box (copied from its intervals, so partially unbounded
    /// pseudo boxes stay partially unbounded), fully unbounded for an unaligned
    /// unbounded one, else the bounding box of its corners.
    pub fn circumscribe(&mut self, pb: &PseudoBox) {
        // low work - axis aligned PseudoBox
        if let Some(axis_map) = pb.is_axis_aligned() {
            for (ii, &m) in axis_map.iter().enumerate() {
                // upstream indexes the basis with the axis map entry of axis `ii`
                let lx = (m.unsigned_abs() as usize).wrapping_sub(1);
                let ivl = pb.interval(lx).unwrap_or_default();
                let (lo, hi) = if m > 0 {
                    (ivl.min(), ivl.max())
                } else {
                    (-ivl.max(), -ivl.min())
                };
                if let Ok(c) = self.min.component_mut(ii) {
                    *c = lo;
                }
                if let Ok(c) = self.max.component_mut(ii) {
                    *c = hi;
                }
            }
            return;
        }

        if pb.is_bounded() {
            // circumscribe unaligned finite PseudoBox corners
            let corners = pb.corners();
            let [first, rest @ ..] = corners;
            self.min = first;
            self.max = first;
            for c in rest {
                if self.min.x > c.x {
                    self.min.x = c.x;
                }
                if self.max.x < c.x {
                    self.max.x = c.x;
                }
                if self.min.y > c.y {
                    self.min.y = c.y;
                }
                if self.max.y < c.y {
                    self.max.y = c.y;
                }
                if self.min.z > c.z {
                    self.min.z = c.z;
                }
                if self.max.z < c.z {
                    self.max.z = c.z;
                }
            }
        } else {
            // circumscribe unaligned unbounded PseudoBoxes with infinite extents
            self.min = Vec3::new(
                -INFINITE_PARAMETER,
                -INFINITE_PARAMETER,
                -INFINITE_PARAMETER,
            );
            self.max = Vec3::new(INFINITE_PARAMETER, INFINITE_PARAMETER, INFINITE_PARAMETER);
        }
    }

    /// Resets to the "inside out" state of [`Extent3d::new`] (`Init`).
    pub fn init(&mut self) {
        *self = Self::new();
    }

    /// True unless a coordinate holds upstream's destructor sentinel
    /// (`±UNDEF_DOUBLE`) (`AssertDefined`).
    #[must_use]
    pub fn is_defined(&self) -> bool {
        self.min.x != UNDEF_DOUBLE
            && self.min.y != UNDEF_DOUBLE
            && self.min.z != UNDEF_DOUBLE
            && self.max.x != -UNDEF_DOUBLE
            && self.max.y != -UNDEF_DOUBLE
            && self.max.z != -UNDEF_DOUBLE
    }

    /// Makes every interval `[-INFINITE_PARAMETER, INFINITE_PARAMETER]` (`SetUnbounded`).
    pub fn set_unbounded(&mut self) {
        self.min = Vec3::new(
            -INFINITE_PARAMETER,
            -INFINITE_PARAMETER,
            -INFINITE_PARAMETER,
        );
        self.max = Vec3::new(INFINITE_PARAMETER, INFINITE_PARAMETER, INFINITE_PARAMETER);
    }

    /// Minimum corner (`GetMin()`).
    #[must_use]
    pub const fn min(&self) -> Point3 {
        self.min
    }

    /// Maximum corner (`GetMax()`).
    #[must_use]
    pub const fn max(&self) -> Point3 {
        self.max
    }

    /// Centre, `(max + min) / 2` (`GetMid()`).
    #[must_use]
    pub fn mid(&self) -> Point3 {
        let s = self.max + self.min;
        Vec3::new(s.x / 2.0, s.y / 2.0, s.z / 2.0)
    }

    /// Minimum x (`GetUMin`).
    #[must_use]
    pub const fn u_min(&self) -> f64 {
        self.min.x
    }

    /// Maximum x (`GetUMax`).
    #[must_use]
    pub const fn u_max(&self) -> f64 {
        self.max.x
    }

    /// Minimum y (`GetVMin`).
    #[must_use]
    pub const fn v_min(&self) -> f64 {
        self.min.y
    }

    /// Maximum y (`GetVMax`).
    #[must_use]
    pub const fn v_max(&self) -> f64 {
        self.max.y
    }

    /// Minimum z (`GetWMin`).
    #[must_use]
    pub const fn w_min(&self) -> f64 {
        self.min.z
    }

    /// Maximum z (`GetWMax`).
    #[must_use]
    pub const fn w_max(&self) -> f64 {
        self.max.z
    }

    /// Minimum along axis `i` (0 = x, 1 = y, 2 = z); `0.0` for any other `i`, as
    /// upstream's `SmVector3d::operator[]` (`GetMin(ULONG)`).
    #[must_use]
    pub fn min_at(&self, i: usize) -> f64 {
        self.min.component(i)
    }

    /// Maximum along axis `i`; `0.0` for `i > 2` (`GetMax(ULONG)`).
    #[must_use]
    pub fn max_at(&self, i: usize) -> f64 {
        self.max.component(i)
    }

    /// Largest side length (`GetMaxLength`).
    #[must_use]
    pub fn max_length(&self) -> f64 {
        max3(self.x_length(), self.y_length(), self.z_length())
    }

    /// The x interval, ends swapped if inverted (`GetUInterval`).
    #[must_use]
    pub fn u_interval(&self) -> Extent1d {
        Extent1d::from_min_max(self.min.x, self.max.x)
    }

    /// The y interval, ends swapped if inverted (`GetVInterval`).
    #[must_use]
    pub fn v_interval(&self) -> Extent1d {
        Extent1d::from_min_max(self.min.y, self.max.y)
    }

    /// The z interval, ends swapped if inverted (`GetWInterval`).
    #[must_use]
    pub fn w_interval(&self) -> Extent1d {
        Extent1d::from_min_max(self.min.z, self.max.z)
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

    /// Largest absolute coordinate of either corner (`GetMaxDimension`).
    #[must_use]
    pub fn max_dimension(&self) -> f64 {
        max2(self.min.max_dimension(), self.max.max_dimension())
    }

    /// Grows the box to contain `p` (`AddPoint3d`).
    pub fn add_point(&mut self, p: Point3) {
        if p.x < self.min.x {
            self.min.x = p.x;
        }
        if p.y < self.min.y {
            self.min.y = p.y;
        }
        if p.z < self.min.z {
            self.min.z = p.z;
        }
        if p.x > self.max.x {
            self.max.x = p.x;
        }
        if p.y > self.max.y {
            self.max.y = p.y;
        }
        if p.z > self.max.z {
            self.max.z = p.z;
        }
    }

    /// Sets both corners; fails (leaving `self` unchanged) when `min > max` on any
    /// axis (`SetMinMax(const SmPoint3d&, const SmPoint3d&)`).
    pub fn set_min_max(&mut self, min: Point3, max: Point3) -> Result<()> {
        if min.x > max.x || min.y > max.y || min.z > max.z {
            return Err(SolidError::InvalidInput {
                operation: "geom::extent3d::Extent3d::set_min_max",
                reason: "min > max",
            });
        }
        self.min = min;
        self.max = max;
        Ok(())
    }

    /// [`Extent3d::set_min_max`] from six coordinates (`SetMinMax(double x6)`).
    pub fn set_min_max_coords(
        &mut self,
        min_x: f64,
        min_y: f64,
        min_z: f64,
        max_x: f64,
        max_y: f64,
        max_z: f64,
    ) -> Result<()> {
        if min_x > max_x || min_y > max_y || min_z > max_z {
            return Err(SolidError::InvalidInput {
                operation: "geom::extent3d::Extent3d::set_min_max_coords",
                reason: "min > max",
            });
        }
        self.min = Vec3::new(min_x, min_y, min_z);
        self.max = Vec3::new(max_x, max_y, max_z);
        Ok(())
    }

    /// Sets the minimum x; fails when it exceeds the maximum x (`SetUMin`).
    pub fn set_u_min(&mut self, v: f64) -> Result<()> {
        if v > self.max.x {
            return Err(Self::bad_bound("geom::extent3d::Extent3d::set_u_min"));
        }
        self.min.x = v;
        Ok(())
    }

    /// Sets the maximum x; fails when it is below the minimum x (`SetUMax`).
    pub fn set_u_max(&mut self, v: f64) -> Result<()> {
        if v < self.min.x {
            return Err(Self::bad_bound("geom::extent3d::Extent3d::set_u_max"));
        }
        self.max.x = v;
        Ok(())
    }

    /// Sets the minimum y; fails when it exceeds the maximum y (`SetVMin`).
    pub fn set_v_min(&mut self, v: f64) -> Result<()> {
        if v > self.max.y {
            return Err(Self::bad_bound("geom::extent3d::Extent3d::set_v_min"));
        }
        self.min.y = v;
        Ok(())
    }

    /// Sets the maximum y; fails when it is below the minimum y (`SetVMax`).
    pub fn set_v_max(&mut self, v: f64) -> Result<()> {
        if v < self.min.y {
            return Err(Self::bad_bound("geom::extent3d::Extent3d::set_v_max"));
        }
        self.max.y = v;
        Ok(())
    }

    fn bad_bound(operation: &'static str) -> SolidError {
        SolidError::InvalidInput {
            operation,
            reason: "assignment would make a negative interval",
        }
    }

    /// The point at normalised coordinates `(nx, ny, nz)`: `0` gives the minimum, `1`
    /// the maximum (exactly), values outside `[0, 1]` extrapolate (`Evaluate`).
    #[must_use]
    pub fn evaluate(&self, nx: f64, ny: f64, nz: f64) -> Point3 {
        Vec3::new(
            Extent1d::from_raw(self.min.x, self.max.x).evaluate(nx),
            Extent1d::from_raw(self.min.y, self.max.y).evaluate(ny),
            Extent1d::from_raw(self.min.z, self.max.z).evaluate(nz),
        )
    }

    /// The smallest box containing both boxes (`Union`).
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        Self {
            min: Vec3::new(
                min2(self.min.x, other.min.x),
                min2(self.min.y, other.min.y),
                min2(self.min.z, other.min.z),
            ),
            max: Vec3::new(
                max2(self.max.x, other.max.x),
                max2(self.max.y, other.max.y),
                max2(self.max.z, other.max.z),
            ),
        }
    }

    /// The overlap of two boxes; fails when they are disjoint (`Intersect`).
    pub fn intersect(&self, other: &Self) -> Result<Self> {
        if self.are_disjoint(other, 0.0) {
            return Err(SolidError::InvalidInput {
                operation: "geom::extent3d::Extent3d::intersect",
                reason: "extents are disjoint",
            });
        }
        Ok(Self {
            min: Vec3::new(
                max2(self.min.x, other.min.x),
                max2(self.min.y, other.min.y),
                max2(self.min.z, other.min.z),
            ),
            max: Vec3::new(
                min2(self.max.x, other.max.x),
                min2(self.max.y, other.max.y),
                min2(self.max.z, other.max.z),
            ),
        })
    }

    /// Squared diagonal of the union: the largest squared distance between two
    /// points of the two boxes (`MaximumDistanceSquared`).
    #[must_use]
    pub fn max_distance_squared(&self, other: &Self) -> f64 {
        let u = self.union(other);
        (u.max - u.min).length_squared()
    }

    /// Smallest squared distance between a point of each box; `0` when they touch
    /// or overlap (`MinimumDistanceSquared`).
    #[must_use]
    pub fn min_distance_squared(&self, other: &Self) -> f64 {
        let mut d = Vec3::ZERO;
        if self.min.x > other.max.x {
            d.x += self.min.x - other.max.x;
        }
        if self.max.x < other.min.x {
            d.x += other.min.x - self.max.x;
        }
        if self.min.y > other.max.y {
            d.y += self.min.y - other.max.y;
        }
        if self.max.y < other.min.y {
            d.y += other.min.y - self.max.y;
        }
        if self.min.z > other.max.z {
            d.z += self.min.z - other.max.z;
        }
        if self.max.z < other.min.z {
            d.z += other.min.z - self.max.z;
        }
        d.length_squared()
    }

    /// Square root of [`Extent3d::max_distance_squared`] (`MaximumDistance`).
    #[must_use]
    pub fn max_distance(&self, other: &Self) -> f64 {
        sqrt(self.max_distance_squared(other))
    }

    /// Square root of [`Extent3d::min_distance_squared`] (`MinimumDistance`).
    #[must_use]
    pub fn min_distance(&self, other: &Self) -> f64 {
        sqrt(self.min_distance_squared(other))
    }

    /// Moves every bounded side outwards by `factor` times the box size on that
    /// axis; unbounded ends stay put (`ExpandRelative`).
    pub fn expand_relative(&mut self, factor: f64) -> &mut Self {
        let e = self.size() * factor;
        self.min.x = shrink_unless_infinite(self.min.x, e.x);
        self.min.y = shrink_unless_infinite(self.min.y, e.y);
        self.min.z = shrink_unless_infinite(self.min.z, e.z);
        self.max.x = grow_unless_infinite(self.max.x, e.x);
        self.max.y = grow_unless_infinite(self.max.y, e.y);
        self.max.z = grow_unless_infinite(self.max.z, e.z);
        self
    }

    /// Moves every bounded side outwards by `amount` (inwards if negative);
    /// unbounded ends stay put (`ExpandAbsolute`).
    pub fn expand_absolute(&mut self, amount: f64) -> &mut Self {
        self.min.x = shrink_unless_infinite(self.min.x, amount);
        self.min.y = shrink_unless_infinite(self.min.y, amount);
        self.min.z = shrink_unless_infinite(self.min.z, amount);
        self.max.x = grow_unless_infinite(self.max.x, amount);
        self.max.y = grow_unless_infinite(self.max.y, amount);
        self.max.z = grow_unless_infinite(self.max.z, amount);
        self
    }

    /// The diagonal vector `max - min` (`GetSize`).
    #[must_use]
    pub fn size(&self) -> Vec3 {
        Vec3::new(
            self.max.x - self.min.x,
            self.max.y - self.min.y,
            self.max.z - self.min.z,
        )
    }

    /// The eight corners in upstream's fixed order
    /// `000 010 100 110 001 011 101 111` (digits are x, y, z) (`GetCorners`).
    #[must_use]
    pub fn corners(&self) -> [Point3; 8] {
        [
            self.evaluate(0.0, 0.0, 0.0),
            self.evaluate(0.0, 1.0, 0.0),
            self.evaluate(1.0, 0.0, 0.0),
            self.evaluate(1.0, 1.0, 0.0),
            self.evaluate(0.0, 0.0, 1.0),
            self.evaluate(0.0, 1.0, 1.0),
            self.evaluate(1.0, 0.0, 1.0),
            self.evaluate(1.0, 1.0, 1.0),
        ]
    }

    /// The twelve edges as 24 end points; points `2k` and `2k+1` bound edge `k`
    /// (`GetEdges`; upstream order).
    #[must_use]
    pub fn edges(&self) -> [Point3; 24] {
        const N: [[f64; 3]; 24] = [
            // x == 0 plane
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0],
            // x == 1 plane
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            // edges from x == 0 to x == 1
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ];
        N.map(|[x, y, z]| self.evaluate(x, y, z))
    }

    /// A point and inward normal for each of the six bounding planes: the min
    /// corner with `+x, +y, +z`, then the max corner with `-x, -y, -z` (`GetPlanes`).
    #[must_use]
    pub fn planes(&self) -> ([Point3; 6], [Vec3; 6]) {
        (
            [self.min, self.min, self.min, self.max, self.max, self.max],
            [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
                Vec3::new(-1.0, 0.0, 0.0),
                Vec3::new(0.0, -1.0, 0.0),
                Vec3::new(0.0, 0.0, -1.0),
            ],
        )
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

    /// `max.z - min.z` (`ZLength`).
    #[must_use]
    pub fn z_length(&self) -> f64 {
        self.max.z - self.min.z
    }

    /// The signed distance along the unit `plane_unit_normal` from the plane through
    /// `plane_point` to the nearest corner of the box, `0` when corners lie on both
    /// sides of (or in) the plane (`DistanceToPlane`).
    #[must_use]
    pub fn distance_to_plane(&self, plane_point: Point3, plane_unit_normal: Vec3) -> f64 {
        let mut min_dist = BIG_DOUBLE;
        let mut x = self.min.x;
        for _ in 0..2 {
            let mut y = self.min.y;
            for _ in 0..2 {
                let mut z = self.min.z;
                for _ in 0..2 {
                    let this_pt = Point3::new(x, y, z);
                    let this_dist = plane_unit_normal.dot(this_pt - plane_point);
                    if min_dist < BIG_DOUBLE - 1.0 && this_dist * min_dist <= 0.0 {
                        return 0.0; // opposite sides of (or in) plane
                    }
                    if this_dist.abs() < min_dist.abs() {
                        min_dist = this_dist;
                    }
                    z = self.max.z;
                }
                y = self.max.y;
            }
            x = self.max.x;
        }
        min_dist
    }

    /// Centre and radii of the enclosing and the inscribed centred spheres
    /// (`ComputeSphereBound`).
    #[must_use]
    pub fn sphere_bound(&self) -> SphereBound {
        let center = self.evaluate(0.5, 0.5, 0.5);
        let corner = self.evaluate(1.0, 1.0, 1.0);
        let radius = corner.distance(center);
        let dx = (center.x - corner.x).abs();
        let dy = (center.y - corner.y).abs();
        let dz = (center.z - corner.z).abs();
        let d = min2(dx, dy);
        SphereBound {
            center,
            radius,
            min_radius: min2(d, dz),
        }
    }

    /// The cone of directions from points of `self` to points of `other`: an axis
    /// and a half angle in radians (`ComputeConeBoundTo`).
    ///
    /// When the boxes overlap (the centre of `self` lies in the bounding sphere of
    /// the grown `other`) the angle is `PI` and the axis is the unit vector between
    /// the centres (or `+z` if they coincide); otherwise the axis is the
    /// (non-unit) vector between the centres.
    pub fn cone_bound_to(&self, other: &Self) -> Result<(Vec3, f64)> {
        let size = self.size();
        let half = Vec3::new(size.x / 2.0, size.y / 2.0, size.z / 2.0);
        let temp = Self {
            min: other.min - half,
            max: other.max + half,
        };
        let sphere = temp.sphere_bound();
        let (sph_cent, sph_rad) = (sphere.center, sphere.radius);
        let this_cent = self.evaluate(0.5, 0.5, 0.5);
        let vec = sph_cent - this_cent;
        let dist = vec.length();
        let scale = 1.0 + sph_cent.max_dimension();
        if dist < sph_rad + EFF_ZERO * scale {
            let axis = if vec.length_squared() > EFF_ZERO * scale {
                // `Unitize` fails only below EFF_ZERO_SQ, which the test above
                // excludes; upstream then returns SM_ERR.
                vec.unitize().map_err(|_| SolidError::Upstream {
                    operation: "geom::extent3d::Extent3d::cone_bound_to",
                    code: SM_ERR,
                    entity: None,
                })?
            } else {
                Vec3::new(0.0, 0.0, 1.0)
            };
            Ok((axis, PI))
        } else {
            Ok((vec, asin(sph_rad / dist)))
        }
    }

    /// True when the boxes are separated by more than `tol` on some axis
    /// (`AreDisjoint`).
    #[must_use]
    pub fn are_disjoint(&self, other: &Self, tol: f64) -> bool {
        self.min.x > other.max.x + tol
            || self.min.y > other.max.y + tol
            || self.min.z > other.max.z + tol
            || self.max.x < other.min.x - tol
            || self.max.y < other.min.y - tol
            || self.max.z < other.min.z - tol
    }

    /// True when `self` lies inside `other` grown by `tol` (`IsContainedBy`).
    #[must_use]
    pub fn is_contained_by(&self, other: &Self, tol: f64) -> bool {
        !(self.min.x < other.min.x - tol
            || self.min.y < other.min.y - tol
            || self.min.z < other.min.z - tol
            || self.max.x > other.max.x + tol
            || self.max.y > other.max.y + tol
            || self.max.z > other.max.z + tol)
    }

    /// True when no side is longer than `tol` (`IsPointSized`; upstream's default
    /// `tol` is `EFF_ZERO`).
    #[must_use]
    pub fn is_point_sized(&self, tol: f64) -> bool {
        !((self.max.x - self.min.x).abs() > tol
            || (self.max.y - self.min.y).abs() > tol
            || (self.max.z - self.min.z).abs() > tol)
    }

    /// True when `p` lies inside the box grown by `tol` (`ContainsPoint3d`).
    #[must_use]
    pub fn contains_point(&self, p: Point3, tol: f64) -> bool {
        !(p.x < self.min.x - tol
            || p.y < self.min.y - tol
            || p.z < self.min.z - tol
            || p.x > self.max.x + tol
            || p.y > self.max.y + tol
            || p.z > self.max.z + tol)
    }

    /// Upstream `ContainsLineSeg3d`: true when **either** end point is in the box
    /// grown by `tol` (upstream ORs the two point tests although its comment says
    /// both; kept for fidelity).
    #[must_use]
    pub fn contains_line_seg(&self, start: Point3, end: Point3, tol: f64) -> bool {
        let a = self.contains_point(start, tol);
        let b = self.contains_point(end, tol);
        a | b
    }

    /// Whether `p` is within `tol` of a side, and the inward "binormal" `(du, dv, dw)`
    /// with `+1` near a min side, `-1` near a max side (min checked first), `0`
    /// otherwise (`IsPoint3dOnBoundary`; upstream's default `tol` is `EFF_ZERO`).
    #[must_use]
    pub fn is_point_on_boundary(&self, p: Point3, tol: f64) -> (bool, Vec3) {
        let mut on = false;
        let axis = |pc: f64, lo: f64, hi: f64, on: &mut bool| {
            if (pc - lo).abs() <= tol {
                *on = true;
                1.0
            } else if (pc - hi).abs() <= tol {
                *on = true;
                -1.0
            } else {
                0.0
            }
        };
        let du = axis(p.x, self.min.x, self.max.x, &mut on);
        let dv = axis(p.y, self.min.y, self.max.y, &mut on);
        let dw = axis(p.z, self.min.z, self.max.z, &mut on);
        (on, Vec3::new(du, dv, dw))
    }

    /// Clips the line `point + t * dir` against the box (any mix of unbounded
    /// intervals is allowed) (`IntersectLine`).
    ///
    /// Fails when `dir` is shorter than `EFF_ZERO` or the extent is not
    /// [defined](Extent3d::is_defined).
    pub fn intersect_line(&self, point: Point3, dir: Vec3) -> Result<LineHit> {
        if dir.length_squared() < EFF_ZERO_SQ || !self.is_defined() {
            return Err(SolidError::InvalidInput {
                operation: "geom::extent3d::Extent3d::intersect_line",
                reason: "zero-length line direction or undefined extent",
            });
        }
        let (tmin_x, tmax_x) = slab(self.min.x, self.max.x, point.x, dir.x);
        let (tmin_y, tmax_y) = slab(self.min.y, self.max.y, point.y, dir.y);
        let (tmin_z, tmax_z) = slab(self.min.z, self.max.z, point.z, dir.z);
        let d_min = max3(tmin_z, tmin_x, tmin_y);
        let d_max = min3(tmax_z, tmax_x, tmax_y);

        // Upstream note [B70]: in an extreme grazing case the midpoint can be far
        // from the box although the line passes within tolerance, so the ends are
        // checked too when the midpoint is outside.
        let mid = point + ((d_min + d_max) / 2.0) * dir;
        let tol = EFF_ZERO * (1.0 + mid.max_dimension());

        if self.contains_point(mid, tol) {
            if (d_min - d_max).abs() < EFF_ZERO {
                // Average so the interval is exactly degenerate and never inverted.
                return Ok(LineHit::Touch {
                    t: (d_min + d_max) / 2.0,
                });
            }
            if d_min < d_max {
                return Ok(LineHit::Through {
                    enter: d_min,
                    exit: d_max,
                });
            }
            return Ok(LineHit::Miss);
        }
        if self.contains_point(point + d_min * dir, tol) {
            return Ok(LineHit::Touch { t: d_min });
        }
        if self.contains_point(point + d_max * dir, tol) {
            return Ok(LineHit::Touch { t: d_max });
        }
        Ok(LineHit::Miss)
    }

    /// True when `min > max` on some axis (e.g. a fresh extent)
    /// (`HasNegativeVolume`).
    #[must_use]
    pub fn has_negative_volume(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    /// True when the extent is exactly in its [`Extent3d::new`] state (`IsInit`).
    #[must_use]
    pub fn is_init(&self) -> bool {
        self.min.x == BIG_DOUBLE
            && self.min.y == BIG_DOUBLE
            && self.min.z == BIG_DOUBLE
            && self.max.x == -BIG_DOUBLE
            && self.max.y == -BIG_DOUBLE
            && self.max.z == -BIG_DOUBLE
    }

    /// Classification of the x, y and z intervals (the out-param of `IsBounded`).
    #[must_use]
    pub fn boundary_types(&self) -> [BoundaryType; 3] {
        [
            BoundaryType::classify(self.min.x, self.max.x),
            BoundaryType::classify(self.min.y, self.max.y),
            BoundaryType::classify(self.min.z, self.max.z),
        ]
    }

    /// True when no interval end is an unbounded marker (`IsBounded`).
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        self.boundary_types() == [BoundaryType::Bounded; 3]
    }

    /// True when some interval is bounded at one end or more (`AnyBounds`).
    #[must_use]
    pub fn any_bounds(&self) -> bool {
        self.boundary_types()
            .iter()
            .any(|t| *t != BoundaryType::Unbounded)
    }

    /// A bounded extent approximating this one: unbounded ends are replaced using
    /// `half_size` and the optional `center` (`[0, 0, 0]` if `None`); a bounded
    /// extent is returned unchanged (`ApproximateUnbounded` without the expanded
    /// output; upstream's default `half_size` is
    /// [`BOUNDED_INFINITE_PARAM`](crate::math::BOUNDED_INFINITE_PARAM)).
    #[must_use]
    pub fn approximate_unbounded(&self, center: Option<Point3>, half_size: f64) -> Self {
        self.approximate_unbounded_expanded(center, half_size).0
    }

    /// [`Extent3d::approximate_unbounded`] plus upstream's optional second extent,
    /// which pushes the replaced ends out by a further `half_size / 12` (used for
    /// graphics). The second value is `None` where upstream's `SetMinMax` rejects it
    /// (an inverted interval), leaving its output untouched.
    #[must_use]
    pub fn approximate_unbounded_expanded(
        &self,
        center: Option<Point3>,
        half_size: f64,
    ) -> (Self, Option<Self>) {
        let c = center.unwrap_or(Vec3::ZERO);
        let types = self.boundary_types();
        let bounded = types == [BoundaryType::Bounded; 3];
        let (mut lo, mut hi) = (self.min, self.max);
        if !bounded {
            let h = half_size;
            let [tx, ty, tz] = types;
            (lo.x, hi.x) = approx_axis(tx, lo.x, hi.x, center.map(|p| p.x), c.x, h);
            (lo.y, hi.y) = approx_axis(ty, lo.y, hi.y, center.map(|p| p.y), c.y, h);
            (lo.z, hi.z) = approx_axis(tz, lo.z, hi.z, center.map(|p| p.z), c.z, h);
        }

        let (mut lo_inc, mut hi_inc) = (lo, hi);
        if !bounded {
            let inc = half_size / 12.0;
            let grows_min =
                |t: BoundaryType| matches!(t, BoundaryType::UnboundedMin | BoundaryType::Unbounded);
            let grows_max =
                |t: BoundaryType| matches!(t, BoundaryType::UnboundedMax | BoundaryType::Unbounded);
            let [tx, ty, tz] = types;
            if grows_min(tx) {
                lo_inc.x -= inc;
            }
            if grows_max(tx) {
                hi_inc.x += inc;
            }
            if grows_min(ty) {
                lo_inc.y -= inc;
            }
            if grows_max(ty) {
                hi_inc.y += inc;
            }
            if grows_min(tz) {
                lo_inc.z -= inc;
            }
            if grows_max(tz) {
                hi_inc.z += inc;
            }
        }
        let mut expanded = Self::new();
        let expanded = expanded.set_min_max(lo_inc, hi_inc).ok().map(|()| expanded);

        (Self::from_points(lo, hi), expanded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::BOUNDED_INFINITE_PARAM;

    fn p(x: f64, y: f64, z: f64) -> Point3 {
        Vec3::new(x, y, z)
    }

    /// upstream `my_test_extent3d` (`tests/prog_test/src/smCurveTest.cpp`).
    #[test]
    fn upstream_my_test_extent3d() {
        let (v1, v2) = (p(1.0, 1.0, 1.0), p(2.0, 2.0, 1.0));
        let e1 = Extent3d::from_points(v1, v2);
        assert_eq!(e1.min(), v1);
        assert_eq!(Extent3d::from_point(v2).max(), v2);
        let mut e4 = Extent3d::new();
        assert!(e4.is_init() && e4.has_negative_volume());
        let (v3, v4, v5) = (p(6.0, 6.0, 6.0), p(5.0, 5.0, 5.0), p(7.0, 7.0, 7.0));
        e4.add_point(v3);
        e4.add_point(v4);
        e4.add_point(v5);
        assert_eq!((e4.min(), e4.max()), (v4, v5));
        let mut e5 = Extent3d::new();
        e5.set_min_max(v4, v5).unwrap();
        let e6 = Extent3d::from_points(-v5, -v4);
        assert!(e5.are_disjoint(&e6, 0.0));
        let u = e5.union(&e6);
        assert_eq!((u.min(), u.max()), (-v5, v5));
        let e7 = Extent3d::from_points(v4, v5);
        assert_eq!(e4.intersect(&e7).unwrap(), e7);
        assert!(e5.intersect(&e6).is_err());
    }

    #[test]
    fn swap_and_setters() {
        let e = Extent3d::from_points(p(3.0, -1.0, 2.0), p(1.0, 4.0, -2.0));
        assert_eq!(e.min(), p(1.0, -1.0, -2.0));
        assert_eq!(e.max(), p(3.0, 4.0, 2.0));
        let mut f = e;
        assert!(f.set_min_max(p(1.0, 0.0, 0.0), p(0.0, 1.0, 1.0)).is_err());
        assert_eq!(f, e);
        assert!(f.set_u_min(5.0).is_err());
        f.set_u_max(10.0).unwrap();
        f.set_u_min(5.0).unwrap();
        assert!(f.set_v_max(-3.0).is_err());
        f.set_v_min(4.0).unwrap();
        assert_eq!(f.min(), p(5.0, 4.0, -2.0));
        assert_eq!(f.min_at(7), 0.0);
        assert_eq!(f.max_at(2), 2.0);
    }

    #[test]
    fn evaluate_exact_ends_and_corners() {
        let e = Extent3d::from_points(p(0.1, 0.2, 0.3), p(0.7, 1.9, 2.3));
        assert_eq!(e.evaluate(1.0, 1.0, 1.0), e.max());
        assert_eq!(e.corners()[3], p(0.7, 1.9, 0.3));
        let edges = e.edges();
        assert_eq!(edges[23], p(0.7, 1.9, 0.3));
        let (pts, nrm) = e.planes();
        assert_eq!((pts[4], nrm[4]), (e.max(), p(0.0, -1.0, 0.0)));
    }

    #[test]
    fn distances() {
        let a = Extent3d::from_points(p(0.0, 0.0, 0.0), p(1.0, 1.0, 1.0));
        let b = Extent3d::from_points(p(4.0, 5.0, 1.0), p(6.0, 6.0, 2.0));
        assert_eq!(a.min_distance_squared(&b), 9.0 + 16.0);
        assert_eq!(a.min_distance(&b), 5.0);
        assert_eq!(a.max_distance_squared(&b), 36.0 + 36.0 + 4.0);
        assert_eq!(a.min_distance(&a), 0.0);
    }

    #[test]
    fn line_intersections() {
        let e = Extent3d::from_points(p(0.0, 0.0, 0.0), p(1.0, 1.0, 1.0));
        let hit = e
            .intersect_line(p(-1.0, 0.5, 0.5), p(1.0, 0.0, 0.0))
            .unwrap();
        assert_eq!(
            hit,
            LineHit::Through {
                enter: 1.0,
                exit: 2.0
            }
        );
        assert_eq!(
            e.intersect_line(p(-1.0, 2.0, 0.5), p(1.0, 0.0, 0.0))
                .unwrap(),
            LineHit::Miss
        );
        let graze = e
            .intersect_line(p(0.0, 0.0, 0.0), p(1.0, -1.0, 0.0))
            .unwrap();
        assert_eq!(graze.count(), 1);
        assert!(e.intersect_line(p(0.0, 0.0, 0.0), Vec3::ZERO).is_err());
    }

    #[test]
    fn bounded_and_approximation() {
        let mut e = Extent3d::new();
        e.set_unbounded();
        assert!(!e.is_bounded() && !e.any_bounds());
        let a = e.approximate_unbounded(None, BOUNDED_INFINITE_PARAM);
        assert_eq!(a.min(), p(-33.0, -33.0, -33.0));
        let half = Extent3d::from_bounds(-INFINITE_PARAMETER, 0.0, 0.0, 2.0, 1.0, 1.0);
        assert_eq!(
            half.boundary_types(),
            [
                BoundaryType::UnboundedMin,
                BoundaryType::Bounded,
                BoundaryType::Bounded
            ]
        );
        assert!(half.any_bounds());
        let (a, x) = half.approximate_unbounded_expanded(None, 10.0);
        assert_eq!((a.u_min(), a.u_max()), (-18.0, 2.0));
        let x = x.unwrap();
        assert_eq!(x.u_min(), -18.0 - 10.0 / 12.0);
        // A bounded extent is copied unchanged.
        let b = Extent3d::from_points(p(0.0, 0.0, 0.0), p(1.0, 2.0, 3.0));
        assert_eq!(b.approximate_unbounded(None, 1.0), b);
    }

    #[test]
    fn sphere_and_cone() {
        let a = Extent3d::from_points(p(0.0, 0.0, 0.0), p(2.0, 2.0, 2.0));
        let s = a.sphere_bound();
        assert_eq!(s.center, p(1.0, 1.0, 1.0));
        assert_eq!(s.min_radius, 1.0);
        let (axis, ang) = a.cone_bound_to(&a).unwrap();
        assert_eq!((axis, ang), (Vec3::Z, PI));
        let far = Extent3d::from_points(p(100.0, 0.0, 0.0), p(102.0, 2.0, 2.0));
        let (axis, ang) = a.cone_bound_to(&far).unwrap();
        assert_eq!(axis, p(100.0, 0.0, 0.0));
        assert!(ang > 0.0 && ang < 0.1);
    }

    #[test]
    fn predicates() {
        let e = Extent3d::from_points(p(0.0, 0.0, 0.0), p(1.0, 1.0, 1.0));
        assert!(e.contains_point(p(1.0 + 1e-13, 0.5, 0.5), 1e-12));
        assert!(!e.contains_point(p(1.1, 0.5, 0.5), 0.0));
        // Upstream bug kept: one end inside is enough.
        assert!(e.contains_line_seg(p(0.5, 0.5, 0.5), p(9.0, 9.0, 9.0), 0.0));
        let (on, bn) = e.is_point_on_boundary(p(0.0, 1.0, 0.5), EFF_ZERO);
        assert!(on);
        assert_eq!(bn, p(1.0, -1.0, 0.0));
        assert!(Extent3d::from_point(p(1.0, 1.0, 1.0)).is_point_sized(EFF_ZERO));
        assert!(e.is_contained_by(&e, 0.0));
        assert!(e.is_defined());
        let mut g = e;
        g.expand_absolute(1.0);
        assert_eq!(g.min(), p(-1.0, -1.0, -1.0));
        g.expand_relative(0.5);
        assert_eq!(g.max(), p(3.5, 3.5, 3.5));
        assert_eq!(g.max_length(), 6.0);
        assert_eq!(g.max_dimension(), 3.5);
        assert_eq!(e.mid(), p(0.5, 0.5, 0.5));
    }
}
