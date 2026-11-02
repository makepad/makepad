//! Pseudo boxes: non-axis-aligned, possibly non-orthogonal boxes given by three unit
//! basis vectors and an interval along each (tight bounding volumes for curves and
//! surfaces).
//!
//! Upstream: `source/SMLib/src/SmPseudoBox.cpp`, `source/SMLib/inc/SmPseudoBox.h`.
//!
//! | Rust                                          | Upstream                                   |
//! |-----------------------------------------------|--------------------------------------------|
//! | [`PseudoBox`]                                 | `SmPseudoBox`                              |
//! | [`Projection`]                                | `SmProjectionType` + its point/vector args |
//! | [`PseudoBox::new`], `Default`                 | `SmPseudoBox()`                            |
//! | [`PseudoBox::from_basis_intervals`]           | `SmPseudoBox(xDir, yDir, zDir, xIvl, ..)`  |
//! | `Clone`/`Copy`                                | copy constructor, `operator=(SmPseudoBox)` |
//! | [`PseudoBox::from_extent3d`]                  | `SmPseudoBox(const SmExtent3d&)`           |
//! | [`PseudoBox::assign_extent3d`]                | `operator=(const SmExtent3d&)`             |
//! | `Drop` (implicit)                             | `~SmPseudoBox`                             |
//! | [`PseudoBox::init`]                           | `Init`                                     |
//! | [`PseudoBox::init_basis`]                     | `InitBasis`                                |
//! | [`PseudoBox::add_point`]                      | `AddPoint3d`                               |
//! | [`PseudoBox::expand_absolute`]                | `ExpandAbsolute`                           |
//! | [`PseudoBox::contains_point`]                 | `ContainsPoint3d`                          |
//! | [`PseudoBox::evaluate`]                       | `Evaluate`                                 |
//! | [`PseudoBox::invert_absolute`]                | `InvertAbsolute`                           |
//! | [`PseudoBox::union_into`]                     | `Union`                                    |
//! | [`PseudoBox::set_basis`]                      | `SetBasis(b1, b2, b3)`                     |
//! | [`PseudoBox::basis`], [`PseudoBox::basis_vector`] | `GetBasis()`, `GetBasis(b1, b2, b3)`, `GetBasis(lIndex)` |
//! | [`PseudoBox::set_intervals`]                  | `SetIntervals`                             |
//! | [`PseudoBox::set_interval`]                   | `SetInterval`                              |
//! | [`PseudoBox::intervals`], [`PseudoBox::interval`] | `GetIntervals`, `GetInterval`          |
//! | [`PseudoBox::set_unbounded`]                  | `SetUnbounded`                             |
//! | [`PseudoBox::distance_to_point`]              | `DistanceToPoint`                          |
//! | [`PseudoBox::intersect_line`]                 | `IntersectLine`                            |
//! | [`PseudoBox::has_negative_volume`]            | `HasNegativeVolume`                        |
//! | [`PseudoBox::max_dimension`]                  | `GetMaxDimension`                          |
//! | [`PseudoBox::xy_area`]                        | `GetXYArea`                                |
//! | [`PseudoBox::volume`]                         | `GetVolume`                                |
//! | [`PseudoBox::is_point_sized`]                 | `IsPointSized`                             |
//! | [`PseudoBox::is_linear`]                      | `IsLinear`                                 |
//! | [`PseudoBox::is_planar`]                      | `IsPlanar`                                 |
//! | [`PseudoBox::are_disjoint`]                   | `AreDisjoint`                              |
//! | [`PseudoBox::are_project_disjoint`]           | `AreProjectDisjoint`                       |
//! | [`PseudoBox::is_contained_by`]                | `IsContainedBy`                            |
//! | [`PseudoBox::classify_point`]                 | `ClassifyPoint3d`                          |
//! | [`PseudoBox::calc_planes`]                    | `CalcPlanes`                               |
//! | [`PseudoBox::is_bounded`], [`PseudoBox::boundary_types`] | `IsBounded`                     |
//! | [`PseudoBox::is_axis_aligned`]                | `IsAxisAligned`                            |
//! | [`PseudoBox::are_aligned`]                    | `AreAligned`                               |
//! | [`PseudoBox::corners`]                        | `CalcCorners`, `GetCorners`                |
//! | [`PseudoBox::edges`]                          | `GetEdges`                                 |
//! | [`PseudoBox::assert_defined`]                 | `AssertDefined`                            |
//! | [`PseudoBox::project_to_plane`], [`PseudoBox::project_to_plane_into`] | `ProjectToPlane` |
//! | `impl` [`ScaledZero`]                         | `SmTol::GetScaledZero(const SmPseudoBox&)` (`SmTol.cpp`, unit `geom.tol`) |
//!
//! The other members of `SmPseudoBox.cpp` (`SetMinMax`, `Set{U,V,W}{Min,Max}`,
//! `ExpandSweep`, `Invert`, `EvaluateAbsolute`, `ConvertAbsoluteToNormal`,
//! `GetIntervalSizes`, `DistanceToPlane`, `GetPlanes`, `Get{U,V,W}Interval`, `IsInit`,
//! `ApproximateUnbounded`, `operator==`, `Dump`, `Draw`, `AssertValid`) are not reached
//! by in-scope code (`harness/callgraph`) and are not ported.
//!
//! Deviations:
//! - Out-params become return values: `IsPointSized`/`IsLinear`/`IsPlanar` return a
//!   struct with every optional output, `IsBounded` splits into a flag and
//!   [`PseudoBox::boundary_types`], `IsAxisAligned`/`AreAligned` return the axis map
//!   only on success (upstream leaves a partial map on failure, which callers ignore),
//!   `IntersectLine` returns a [`LineHit`] (upstream leaves `rdTEnter`/`rdTExit`
//!   untouched on a miss), `CalcCorners`/`CalcPlanes`/`GetEdges` return arrays.
//! - `ProjectToPlane` writes into a caller-provided box whose *basis* survives its
//!   `Init()` in two cases (a basis `SetBasis` rejects, never seen in practice, and the
//!   `MakeUnitOrthoVectors` branch, which also does not refresh the orthogonality flag).
//!   [`PseudoBox::project_to_plane_into`] keeps that behaviour exactly (including the
//!   `(0, 0, 1)` that a failing `MakeUnitOrthoVectors` leaves in basis vector 0);
//!   [`PseudoBox::project_to_plane`] starts from [`PseudoBox::new`], which is what
//!   `AreDisjoint`/`AreProjectDisjoint` do. Projecting a box onto itself
//!   (`b.ProjectToPlane(b, ..)`) is `let src = b; src.project_to_plane_into(.., &mut b)`.
//! - `Union` likewise writes into a caller-provided box (only its basis is reset, its
//!   intervals keep growing): [`PseudoBox::union_into`].
//! - `SmProjectionType` plus the nullable point/vector arguments become [`Projection`];
//!   the arguments upstream asserts to be non-null are always present.
//! - Failing `InvertAbsolute` calls whose status upstream ignores (`AddPoint3d`,
//!   `ContainsPoint3d`, `ClassifyPoint3d`) use the uninitialised point
//!   ([`Vec3::UNDEF`]), as upstream; `DistanceToPoint` likewise uses the
//!   uninitialised line/vertex when `smgu_IntersectTwoPlanes`/`ThreePlanes` fail, and
//!   returns `SM_ERR` (1001.0) as a distance for a point outside more than three
//!   planes (possible only for inside-out boxes).
//! - `GetBasis(lIndex)`/`GetInterval(lIndex)`/`SetInterval(lIndex, ..)` with an index
//!   above 2 are undefined behaviour upstream; here they return `None` or an error.
//! - `SM_ASSERT*` checks and `SM_DEBUG_CODE` blocks are compiled out upstream (release
//!   build) and omitted.

use crate::error::{Result, SolidError, SM_ERR};
use crate::geom::extent1d::{Extent1d, ExtentPointType};
use crate::geom::extent3d::{BoundaryType, Extent3d};
use crate::geom::geom_utility::p1::{
    determinant_3_vectors, intersect_three_planes, line_closest_point, solve_quadratic_eqn,
};
use crate::geom::geom_utility::p2::{intersect_two_planes, PlanesIntersection};
use crate::geom::tol::{are_same, is_zero, is_zero_to_tol, tol3d, ScaledZero};
use crate::geom::vector3d::{sqrt, Point3, Vec3};
use crate::math::{max2, max3, min3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, INFINITE_PARAMETER};

/// A projection onto a plane (`SmProjectionType` with its point and vector
/// arguments).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Projection {
    /// `SM_PT_PARALLEL`: along the unit plane `normal` onto the plane through `point`.
    Parallel {
        /// Point on the plane.
        point: Point3,
        /// Unit normal of the plane.
        normal: Vec3,
    },
    /// `SM_PT_PERSPECTIVE`: towards `eye` onto the plane (`point`, unit `normal`).
    Perspective {
        /// Point on the plane.
        point: Point3,
        /// Unit normal of the plane.
        normal: Vec3,
        /// Eye point.
        eye: Point3,
    },
    /// `SM_PT_ROTATION`: about the axis (`point`, unit `axis`) into the plane spanned
    /// by `axis` and the unit `x_axis`.
    Rotation {
        /// Point on the rotation axis.
        point: Point3,
        /// Unit direction of the rotation axis.
        axis: Vec3,
        /// Unit vector perpendicular to the axis, in the common plane.
        x_axis: Vec3,
    },
}

impl Projection {
    /// `cpProjPoint`: the plane point or the point on the rotation axis.
    #[must_use]
    pub const fn point(&self) -> Point3 {
        match *self {
            Self::Parallel { point, .. }
            | Self::Perspective { point, .. }
            | Self::Rotation { point, .. } => point,
        }
    }

    /// `cpProjUnitVector`: the plane normal or the rotation axis direction.
    #[must_use]
    pub const fn unit_vector(&self) -> Vec3 {
        match *self {
            Self::Parallel { normal, .. } | Self::Perspective { normal, .. } => normal,
            Self::Rotation { axis, .. } => axis,
        }
    }

    /// The upstream `SmProjectionType` value (`SM_PT_PARALLEL` = 0,
    /// `SM_PT_PERSPECTIVE` = 1, `SM_PT_ROTATION` = 2).
    #[must_use]
    pub const fn code(&self) -> i32 {
        match self {
            Self::Parallel { .. } => 0,
            Self::Perspective { .. } => 1,
            Self::Rotation { .. } => 2,
        }
    }
}

/// Result of [`PseudoBox::intersect_line`] (`rlNumFound`, `rdTEnter`, `rdTExit`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineHit {
    /// No intersection (`rlNumFound = 0`).
    Miss,
    /// The line grazes the box at one parameter (`rlNumFound = 1`).
    Graze(f64),
    /// The line is inside the box between the two parameters (`rlNumFound = 2`).
    Segment(f64, f64),
}

impl LineHit {
    /// Upstream `rlNumFound`.
    #[must_use]
    pub const fn count(&self) -> u32 {
        match self {
            Self::Miss => 0,
            Self::Graze(_) => 1,
            Self::Segment(..) => 2,
        }
    }
}

/// Outputs of [`PseudoBox::is_point_sized`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointSized {
    /// True when every half interval length is below the tolerance.
    pub is_point_sized: bool,
    /// The box centre (`pOptCenterPoint`).
    pub center: Point3,
    /// Largest half interval length (`pOptActualDeviation`).
    pub deviation: f64,
}

/// Outputs of [`PseudoBox::is_linear`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Linear {
    /// True when the second and third half interval lengths are below the tolerance.
    pub is_linear: bool,
    /// The box centre (`pOptLinePoint`).
    pub point: Point3,
    /// Basis vector 0 (`pOptLineTangent`).
    pub tangent: Vec3,
    /// Larger of the second and third half interval lengths (`pOptActualDeviation`).
    pub deviation: f64,
}

/// Outputs of [`PseudoBox::is_planar`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Planar {
    /// True when some half interval length is below the tolerance.
    pub is_planar: bool,
    /// The box centre (`pOptPlanePoint`).
    pub point: Point3,
    /// `basis[0] × basis[1]` (`pOptPlaneNormal`; not unitised, as upstream).
    pub normal: Vec3,
    /// Smallest half interval length (`pOptActualDeviation`).
    pub deviation: f64,
}

/// A box spanned by three (not necessarily orthogonal) unit basis vectors and an
/// interval along each (`SmPseudoBox`): the points `a·basis[0] + b·basis[1] +
/// c·basis[2]` with `a`, `b`, `c` in the respective intervals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PseudoBox {
    orthogonal_basis: bool,
    basis: [Vec3; 3],
    intervals: [Extent1d; 3],
}

impl Default for PseudoBox {
    fn default() -> Self {
        Self::new()
    }
}

impl ScaledZero for PseudoBox {
    /// `SmTol::GetScaledZero(const SmPseudoBox&)`: `SM_EFF_ZERO * (1 + max |end|)`
    /// over the three intervals.
    fn scaled_zero(&self) -> f64 {
        let [i0, i1, i2] = &self.intervals;
        tol3d(EFF_ZERO * (1.0 + max3(i0.max_dimension(), i1.max_dimension(), i2.max_dimension())))
    }
}

/// Corner index pairs of the 12 box edges, in `ProjectToPlane`'s order.
const EDGE_CORNERS: [(usize, usize); 12] = [
    (0, 1),
    (1, 2),
    (2, 3),
    (3, 0),
    (4, 5),
    (5, 6),
    (6, 7),
    (7, 4),
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7),
];

impl PseudoBox {
    /// The axis-aligned basis `x, y, z` with "inside out" intervals ready for
    /// [`PseudoBox::add_point`] (`SmPseudoBox()`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            orthogonal_basis: true,
            basis: [Vec3::X, Vec3::Y, Vec3::Z],
            intervals: [Extent1d::new(), Extent1d::new(), Extent1d::new()],
        }
    }

    /// The bytes of an `SmPseudoBox` whose constructor never ran in zero-filled
    /// memory: zero basis vectors and intervals, not orthogonal. Upstream's curve cache
    /// spans live in `smos_Calloc` blocks (`SmMemBlockMgr::GetNewElement`), so a
    /// span's box starts so until `CalculateBoundingBox` sets its basis.
    #[must_use]
    pub const fn zero_filled() -> Self {
        Self {
            orthogonal_basis: false,
            basis: [Vec3::ZERO, Vec3::ZERO, Vec3::ZERO],
            intervals: [
                Extent1d::from_raw(0.0, 0.0),
                Extent1d::from_raw(0.0, 0.0),
                Extent1d::from_raw(0.0, 0.0),
            ],
        }
    }

    /// The header constructor `SmPseudoBox(xDir, yDir, zDir, xIvl, yIvl, zIvl)`:
    /// [`PseudoBox::set_basis`] (whose failure leaves the basis uninitialised,
    /// [`Vec3::UNDEF`]) then [`PseudoBox::set_intervals`].
    #[must_use]
    pub fn from_basis_intervals(basis: [Vec3; 3], intervals: [Extent1d; 3]) -> Self {
        let mut b = Self {
            orthogonal_basis: true,
            basis: [Vec3::UNDEF; 3],
            intervals: [Extent1d::new(); 3],
        };
        let [b0, b1, b2] = basis;
        // upstream ignores the status (the basis stays uninitialised)
        let _ = b.set_basis(b0, b1, b2);
        let [i0, i1, i2] = intervals;
        b.set_intervals(i0, i1, i2);
        b
    }

    /// The axis-aligned box of `e` (`SmPseudoBox(const SmExtent3d&)`).
    #[must_use]
    pub fn from_extent3d(e: &Extent3d) -> Self {
        let mut b = Self::new();
        b.assign_extent3d(e);
        b
    }

    /// Becomes the axis-aligned box of `e` (`operator=(const SmExtent3d&)`); an
    /// inside-out interval of `e` leaves the corresponding interval unchanged.
    pub fn assign_extent3d(&mut self, e: &Extent3d) {
        self.orthogonal_basis = true;
        self.basis = [Vec3::X, Vec3::Y, Vec3::Z];
        let [i0, i1, i2] = &mut self.intervals;
        // upstream ignores the SetMinMax statuses
        let _ = i0.set_min_max(e.u_min(), e.u_max());
        let _ = i1.set_min_max(e.v_min(), e.v_max());
        let _ = i2.set_min_max(e.w_min(), e.w_max());
    }

    /// Resets the intervals to "inside out", keeping the basis (`Init`).
    pub fn init(&mut self) {
        for ivl in &mut self.intervals {
            ivl.init();
        }
    }

    /// Resets the basis to `x, y, z`, keeping the intervals (`InitBasis`).
    pub fn init_basis(&mut self) {
        self.orthogonal_basis = true;
        self.basis = [Vec3::X, Vec3::Y, Vec3::Z];
    }

    /// True when the basis vectors are mutually orthogonal (`m_bOrthogonalBasis`).
    #[must_use]
    pub const fn is_orthogonal_basis(&self) -> bool {
        self.orthogonal_basis
    }

    /// The basis vectors (`GetBasis`).
    #[must_use]
    pub const fn basis(&self) -> &[Vec3; 3] {
        &self.basis
    }

    /// Basis vector `i` (`GetBasis(lIndex)`), `None` for `i > 2`.
    #[must_use]
    pub fn basis_vector(&self, i: usize) -> Option<Vec3> {
        self.basis.get(i).copied()
    }

    /// The intervals (`GetIntervals`).
    #[must_use]
    pub const fn intervals(&self) -> &[Extent1d; 3] {
        &self.intervals
    }

    /// Interval `i` (`GetInterval(lIndex)`), `None` for `i > 2`.
    #[must_use]
    pub fn interval(&self, i: usize) -> Option<Extent1d> {
        self.intervals.get(i).copied()
    }

    /// Sets all three intervals (`SetIntervals`).
    pub fn set_intervals(&mut self, i0: Extent1d, i1: Extent1d, i2: Extent1d) {
        self.intervals = [i0, i1, i2];
    }

    /// Sets interval `i` (`SetInterval`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for `i > 2` (undefined behaviour upstream).
    pub fn set_interval(&mut self, i: usize, ivl: Extent1d) -> Result<()> {
        let slot = self.intervals.get_mut(i).ok_or(SolidError::InvalidInput {
            operation: "geom::pseudo_box::PseudoBox::set_interval",
            reason: "interval index above 2",
        })?;
        *slot = ivl;
        Ok(())
    }

    /// Makes every interval `[-INFINITE_PARAMETER, INFINITE_PARAMETER]`
    /// (`SetUnbounded`).
    pub fn set_unbounded(&mut self) {
        for ivl in &mut self.intervals {
            // cannot fail: min < max
            let _ = ivl.set_min_max(-INFINITE_PARAMETER, INFINITE_PARAMETER);
        }
    }

    /// Sets the basis, unitising vectors whose squared length is not within the scaled
    /// zero of 1, and records whether it is orthogonal (`SetBasis(b1, b2, b3)`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] (box unchanged) when `|det(b1, b2, b3)| <
    /// EFF_ZERO`.
    pub fn set_basis(&mut self, b1: Vec3, b2: Vec3, b3: Vec3) -> Result<()> {
        // check input: Basis vectors must be linearly independent
        let det = determinant_3_vectors(b1, b2, b3);
        if det.abs() < EFF_ZERO {
            return Err(SolidError::InvalidInput {
                operation: "geom::pseudo_box::PseudoBox::set_basis",
                reason: "basis vectors are not linearly independent",
            });
        }

        self.basis = [b1, b2, b3];

        // Basis vectors must be unitized
        for (v, b) in self.basis.iter_mut().zip([b1, b2, b3]) {
            let d = b.length_squared();
            if !is_zero(d - 1.0) {
                *v /= sqrt(d);
            }
        }

        let [v0, v1, v2] = self.basis;
        self.orthogonal_basis = v0.dot(v1).abs() < EFF_ZERO
            && v1.dot(v2).abs() < EFF_ZERO
            && v2.dot(v0).abs() < EFF_ZERO;
        Ok(())
    }

    /// True when every interval and basis vector is defined (`AssertDefined`).
    #[must_use]
    pub fn assert_defined(&self) -> bool {
        self.intervals.iter().all(Extent1d::is_defined)
            && self.basis.iter().all(|v| v.is_initialized())
    }

    /// The coefficients of `p` in the basis (Cramer's rule) (`InvertAbsolute`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` ([`SolidError::Upstream`]) for an undefined box or a singular basis
    /// (`SM_IS_ZERO(det)`).
    pub fn invert_absolute(&self, p: Point3) -> Result<Point3> {
        const OP: &str = "geom::pseudo_box::PseudoBox::invert_absolute";
        if !self.assert_defined() {
            return Err(SolidError::sm_err(OP));
        }
        let [b0, b1, b2] = self.basis;
        let det = determinant_3_vectors(b0, b1, b2);
        if is_zero(det) {
            return Err(SolidError::sm_err(OP));
        }
        Ok(Vec3::new(
            determinant_3_vectors(p, b1, b2) / det,
            determinant_3_vectors(b0, p, b2) / det,
            determinant_3_vectors(b0, b1, p) / det,
        ))
    }

    /// [`PseudoBox::invert_absolute`] with upstream's ignored status: the
    /// uninitialised point on failure.
    fn invert_absolute_or_undef(&self, p: Point3) -> Point3 {
        self.invert_absolute(p).unwrap_or(Vec3::UNDEF)
    }

    /// Grows the intervals to contain `p` (`AddPoint3d`; used after
    /// [`PseudoBox::set_basis`]).
    pub fn add_point(&mut self, p: Point3) {
        let a = self.invert_absolute_or_undef(p);
        let [i0, i1, i2] = &mut self.intervals;
        i0.add_value(a.x);
        i1.add_value(a.y);
        i2.add_value(a.z);
    }

    /// Moves every bounded interval end outwards by `d` (`ExpandAbsolute`).
    pub fn expand_absolute(&mut self, d: f64) {
        for ivl in &mut self.intervals {
            ivl.expand_absolute(d);
        }
    }

    /// True when every basis coefficient of `p` is within `tol` of its interval
    /// (`ContainsPoint3d`; upstream default `tol` is 0).
    #[must_use]
    pub fn contains_point(&self, p: Point3, tol: f64) -> bool {
        let a = self.invert_absolute_or_undef(p);
        let [i0, i1, i2] = &self.intervals;
        i0.contains_value(a.x, tol) && i1.contains_value(a.y, tol) && i2.contains_value(a.z, tol)
    }

    /// The point at normalised coordinates (`(0,0,0)` is the minimum corner,
    /// `(1,1,1)` the maximum) (`Evaluate`).
    #[must_use]
    pub fn evaluate(&self, nx: f64, ny: f64, nz: f64) -> Point3 {
        let [b0, b1, b2] = self.basis;
        let [i0, i1, i2] = &self.intervals;
        b0 * i0.evaluate(nx) + b1 * i1.evaluate(ny) + b2 * i2.evaluate(nz)
    }

    /// Writes into `result` the box with basis `basis` (rejected bases leave
    /// `result`'s basis as it was) grown by the corners of `self` and `other`
    /// (`Union`; `result`'s intervals are not reset first, as upstream).
    pub fn union_into(&self, other: &Self, basis: &[Vec3; 3], result: &mut Self) {
        let mine = self.corners();
        let theirs = other.corners();
        let [b0, b1, b2] = *basis;
        // upstream ignores the status
        let _ = result.set_basis(b0, b1, b2);
        for p in mine.into_iter().chain(theirs) {
            result.add_point(p);
        }
    }

    /// Distance from `p` to the box, `0` inside (`DistanceToPoint`).
    #[must_use]
    pub fn distance_to_point(&self, p: Point3) -> f64 {
        let (pl_pnts, pl_norms) = self.calc_planes();
        let mut found: Vec<(Point3, Vec3, f64)> = Vec::with_capacity(6);
        for (pnt, norm) in pl_pnts.into_iter().zip(pl_norms) {
            // Calculate D of plane equation, A, B, C are in aVectors
            let d = -pnt.dot(norm);
            let res = norm.dot(p) + d;
            // when point is outside the pseudo box plane
            if res > EFF_ZERO {
                found.push((pnt, norm, res));
            }
        }

        match found.as_slice() {
            // point inside pseudo box
            [] => 0.0,
            // point closest to plane of pseudo box
            [(_, _, dist)] => *dist,
            // point closest to edge of pseudo box
            [(p1, n1, _), (p2, n2, _)] => {
                let (line_pnt, line_vec) = match intersect_two_planes(*p1, *n1, *p2, *n2) {
                    Ok(PlanesIntersection::Line { point, direction }) => (point, direction),
                    // upstream ignores the status and uses the uninitialised outputs
                    _ => (Vec3::UNDEF, Vec3::UNDEF),
                };
                let t = line_closest_point(line_pnt, line_vec, p);
                let pnt_on_line = line_pnt + t * line_vec;
                pnt_on_line.distance(p)
            }
            // point closest to vertex of pseudo box
            [(p1, n1, _), (p2, n2, _), (p3, n3, _)] => {
                let vertex =
                    intersect_three_planes(*p1, *n1, *p2, *n2, *p3, *n3).unwrap_or(Vec3::UNDEF);
                vertex.distance(p)
            }
            // a point should not be outside more than 3 pseudo box planes at one time
            _ => f64::from(SM_ERR),
        }
    }

    /// Intersects the line `p + t * v` with the box (`IntersectLine`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when `|v|² < EFF_ZERO_SQ` or the box has negative
    /// volume.
    pub fn intersect_line(&self, p: Point3, v: Vec3) -> Result<LineHit> {
        if v.length_squared() < EFF_ZERO_SQ || self.has_negative_volume() {
            return Err(SolidError::InvalidInput {
                operation: "geom::pseudo_box::PseudoBox::intersect_line",
                reason: "zero line vector or negative-volume box",
            });
        }

        let [b0, b1, b2] = self.basis;
        let [i0, i1, i2] = &self.intervals;
        // get pseudobox plane normals
        let normals = [b1.cross(b2), b2.cross(b0), b0.cross(b1)];

        // Get pseudobox plane min/max corner points
        let min_pt = i0.min() * b0 + i1.min() * b1 + i2.min() * b2;
        let max_pt = i0.max() * b0 + i1.max() * b1 + i2.max() * b2;

        let slab = |n: Vec3| -> (f64, f64) {
            let ndv = n.dot(v);
            if ndv > EFF_ZERO {
                ((min_pt - p).dot(n) / ndv, (max_pt - p).dot(n) / ndv)
            } else if ndv < -EFF_ZERO {
                ((max_pt - p).dot(n) / ndv, (min_pt - p).dot(n) / ndv)
            } else {
                (-BIG_DOUBLE, BIG_DOUBLE)
            }
        };
        let [n0, n1, n2] = normals;
        let (tmin_x, tmax_x) = slab(n0);
        let (tmin_y, tmax_y) = slab(n1);
        let (tmin_z, tmax_z) = slab(n2);

        // the intersection interval is bounded by the Max(minParam) and Min(maxParam)
        let d_min = max3(tmin_z, tmin_x, tmin_y);
        let d_max = min3(tmax_z, tmax_x, tmax_y);

        let mid = p + ((d_min + d_max) / 2.0) * v;

        let tol = EFF_ZERO * (1.0 + mid.max_dimension());
        Ok(if !self.contains_point(mid, tol) {
            LineHit::Miss
        } else if (d_min - d_max).abs() < EFF_ZERO {
            // avg dT to make ivl exactly degenerate and never inverted
            LineHit::Graze((d_min + d_max) / 2.0)
        } else if d_min < d_max {
            LineHit::Segment(d_min, d_max)
        } else {
            LineHit::Miss
        })
    }

    /// True when some interval is inside out (`HasNegativeVolume`).
    #[must_use]
    pub fn has_negative_volume(&self) -> bool {
        self.intervals.iter().any(Extent1d::has_negative_length)
    }

    /// Longest interval length (`GetMaxDimension`).
    #[must_use]
    pub fn max_dimension(&self) -> f64 {
        let [i0, i1, i2] = &self.intervals;
        max3(i0.length(), i1.length(), i2.length())
    }

    /// Area of the parallelogram spanned by the first two sized basis vectors
    /// (`GetXYArea`).
    #[must_use]
    pub fn xy_area(&self) -> f64 {
        let [i0, i1, _] = &self.intervals;
        let d01 = i0.length() * i1.length();
        let area = self.basis[0].cross(self.basis[1]).length();
        d01 * area
    }

    /// Signed volume: product of the interval lengths times the basis triple product
    /// (`GetVolume`).
    #[must_use]
    pub fn volume(&self) -> f64 {
        let [i0, i1, i2] = &self.intervals;
        let d012 = i0.length() * i1.length() * i2.length();
        let [b0, b1, b2] = self.basis;
        d012 * b0.triple_product(b1, b2)
    }

    /// `SM_EFF_ZERO * (1 + max dimension)` when `tol == EFF_ZERO` exactly, else `tol`.
    fn scaled_tol(&self, tol: f64) -> f64 {
        if tol == EFF_ZERO {
            EFF_ZERO * (1.0 + self.max_dimension())
        } else {
            tol
        }
    }

    /// Whether every half interval length is below `tol` (`IsPointSized`; `tol ==
    /// EFF_ZERO` means "scaled zero of the box").
    #[must_use]
    pub fn is_point_sized(&self, tol: f64) -> PointSized {
        let [i0, i1, i2] = &self.intervals;
        let (d0, d1, d2) = (i0.length(), i1.length(), i2.length());
        let scaled_zero = self.scaled_tol(tol);
        PointSized {
            is_point_sized: d0 / 2.0 < scaled_zero
                && d1 / 2.0 < scaled_zero
                && d2 / 2.0 < scaled_zero,
            center: self.evaluate(0.5, 0.5, 0.5),
            deviation: max3(d0, d1, d2) / 2.0,
        }
    }

    /// Whether the second and third half interval lengths are below `tol`
    /// (`IsLinear`; `tol == EFF_ZERO` means "scaled zero of the box").
    #[must_use]
    pub fn is_linear(&self, tol: f64) -> Linear {
        let [_, i1, i2] = &self.intervals;
        let (d1, d2) = (i1.length(), i2.length());
        let scaled_zero = self.scaled_tol(tol);
        Linear {
            is_linear: d1 / 2.0 < scaled_zero && d2 / 2.0 < scaled_zero,
            point: self.evaluate(0.5, 0.5, 0.5),
            tangent: self.basis[0],
            deviation: max2(d1, d2) / 2.0,
        }
    }

    /// Whether some half interval length is below `tol` (`IsPlanar`; planes, lines and
    /// points are all planar; `tol == EFF_ZERO`, the upstream default, means "scaled
    /// zero of the box").
    #[must_use]
    pub fn is_planar(&self, tol: f64) -> Planar {
        let [i0, i1, i2] = &self.intervals;
        let (d0, d1, d2) = (i0.length(), i1.length(), i2.length());
        let scaled_zero = self.scaled_tol(tol);
        let degen_dim_cnt = [d0, d1, d2]
            .iter()
            .filter(|&&d| d / 2.0 < scaled_zero)
            .count();
        Planar {
            is_planar: degen_dim_cnt >= 1,
            point: self.evaluate(0.5, 0.5, 0.5),
            normal: self.basis[0].cross(self.basis[1]),
            deviation: min3(d0, d1, d2) / 2.0,
        }
    }

    /// True when every one of `corners` is outside (by more than `EFF_ZERO`) one of
    /// the planes.
    fn some_plane_separates(planes: &([Point3; 6], [Vec3; 6]), corners: &[Point3; 8]) -> bool {
        let (points, vectors) = planes;
        points.iter().zip(vectors).any(|(pnt, vec)| {
            // Calculate D of plane equation, A, B, C are in aVectors
            let d = -pnt.dot(*vec);
            // `!any(< EFF_ZERO)`, as upstream's loop: a NaN corner (a perspective eye on the
            // view plane projects the boxes to NaN) is not below the limit, so the plane
            // separates. `all(>= EFF_ZERO)` says the opposite for NaN (#59).
            !corners.iter().any(|c| vec.dot(*c) + d < EFF_ZERO)
        })
    }

    /// True when the boxes share no point; with `proj_unit_vector`, when their
    /// parallel projections onto the plane through the origin normal to it do not
    /// (`AreDisjoint`; false when a projection fails).
    #[must_use]
    pub fn are_disjoint(&self, other: &Self, proj_unit_vector: Option<Vec3>) -> bool {
        let (this, that) = match proj_unit_vector {
            Some(normal) => {
                let proj = Projection::Parallel {
                    point: Vec3::ZERO,
                    normal,
                };
                let (Ok(a), Ok(b)) = (self.project_to_plane(&proj), other.project_to_plane(&proj))
                else {
                    return false;
                };
                (a, b)
            }
            None => (*self, *other),
        };

        // quick test: when all OtherBox corners are outside any one ThisBox Plane
        //             then boxes are disjoint
        if Self::some_plane_separates(&this.calc_planes(), &that.corners()) {
            return true;
        }
        // switch things around - check all ThisBox corners against OtherBox Planes
        Self::some_plane_separates(&that.calc_planes(), &this.corners())
    }

    /// True when the projections of the boxes are disjoint (`AreProjectDisjoint`;
    /// false when a projection fails).
    #[must_use]
    pub fn are_project_disjoint(&self, other: &Self, proj: &Projection) -> bool {
        let (Ok(this), Ok(that)) = (self.project_to_plane(proj), other.project_to_plane(proj))
        else {
            return false;
        };
        this.are_disjoint(&that, None)
    }

    /// True when every corner of `self` is in `other` within `tol`
    /// (`IsContainedBy`; upstream default `tol` is 0).
    #[must_use]
    pub fn is_contained_by(&self, other: &Self, tol: f64) -> bool {
        self.corners().iter().all(|c| other.contains_point(*c, tol))
    }

    /// Classifies the basis coefficients of `p` against each interval
    /// (`ClassifyPoint3d`; upstream default `tol` is `EFF_ZERO`).
    #[must_use]
    pub fn classify_point(&self, p: Point3, tol: f64) -> [ExtentPointType; 3] {
        let a = self.invert_absolute_or_undef(p);
        let [i0, i1, i2] = &self.intervals;
        [
            i0.classify_point(a.x, tol),
            i1.classify_point(a.y, tol),
            i2.classify_point(a.z, tol),
        ]
    }

    /// The six bounding planes as (centre point, outward unit normal), in the order
    /// basis 0 min/max, basis 1 min/max, basis 2 min/max (`CalcPlanes`).
    #[must_use]
    pub fn calc_planes(&self) -> ([Point3; 6], [Vec3; 6]) {
        let [b0, b1, b2] = self.basis;
        let [i0, i1, i2] = &self.intervals;
        let det = determinant_3_vectors(b0, b1, b2);
        let no_flip = det > 0.0;

        let origin = b0 * i0.min() + b1 * i1.min() + b2 * i2.min();

        let vec0 = b0 * i0.length();
        let vec1 = b1 * i1.length();
        let vec2 = b2 * i2.length();

        let pair = |normal: Vec3, a: Vec3, b: Vec3, far: Vec3| {
            let n = normal.unitize_or_z();
            let p_lo = origin + 0.5 * (a + b);
            let v_lo = if no_flip { -n } else { n };
            (p_lo, v_lo, p_lo + far, -v_lo)
        };
        let (p0, v0, p1, v1) = pair(b1.cross(b2), vec1, vec2, vec0);
        let (p2, v2, p3, v3) = pair(b2.cross(b0), vec2, vec0, vec1);
        let (p4, v4, p5, v5) = pair(b0.cross(b1), vec0, vec1, vec2);
        ([p0, p1, p2, p3, p4, p5], [v0, v1, v2, v3, v4, v5])
    }

    /// True when no interval end is an unbounded marker (`IsBounded`).
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        self.boundary_types()
            .iter()
            .all(|t| *t == BoundaryType::Bounded)
    }

    /// The boundary classification of each interval (`IsBounded`'s out-param).
    #[must_use]
    pub fn boundary_types(&self) -> [BoundaryType; 3] {
        let [i0, i1, i2] = &self.intervals;
        [i0.boundary_type(), i1.boundary_type(), i2.boundary_type()]
    }

    /// Maps every basis vector to the signed 1-based index of the reference axis it is
    /// parallel to, or `None` when some basis vector is parallel to none.
    fn axis_map(&self, refs: [Vec3; 3]) -> Option<[i32; 3]> {
        let mut map = [0_i32; 3];
        for (m, b) in map.iter_mut().zip(self.basis) {
            let len = b.length();
            let [rx, ry, rz] = refs;
            let x_dot = b.dot(rx) / len;
            let y_dot = b.dot(ry) / len;
            let z_dot = b.dot(rz) / len;
            *m = if are_same(x_dot, 1.0) {
                1
            } else if are_same(x_dot, -1.0) {
                -1
            } else if are_same(y_dot, 1.0) {
                2
            } else if are_same(y_dot, -1.0) {
                -2
            } else if are_same(z_dot, 1.0) {
                3
            } else if are_same(z_dot, -1.0) {
                -3
            } else {
                return None;
            };
        }
        Some(map)
    }

    /// For an orthogonal basis parallel to the `x`, `y`, `z` axes (in any order): for
    /// each basis vector, the 1-based index of its axis, negative when opposite
    /// (`IsAxisAligned`).
    #[must_use]
    pub fn is_axis_aligned(&self) -> Option<[i32; 3]> {
        if !self.orthogonal_basis {
            return None;
        }
        self.axis_map([Vec3::X, Vec3::Y, Vec3::Z])
    }

    /// For an orthogonal basis parallel to `other`'s (in any order): for each basis
    /// vector, the 1-based index of `other`'s parallel basis vector, negative when
    /// opposite (`AreAligned`).
    #[must_use]
    pub fn are_aligned(&self, other: &Self) -> Option<[i32; 3]> {
        if !self.orthogonal_basis {
            return None;
        }
        let [o0, o1, o2] = other.basis;
        self.axis_map([o0 / o0.length(), o1 / o1.length(), o2 / o2.length()])
    }

    /// The eight corners in the order `000, 100, 110, 010, 001, 101, 111, 011`
    /// (`CalcCorners`, `GetCorners`).
    #[must_use]
    pub fn corners(&self) -> [Point3; 8] {
        let [b0, b1, b2] = self.basis;
        let [i0, i1, i2] = &self.intervals;
        // min corner
        let c0 = b0 * i0.min() + b1 * i1.min() + b2 * i2.min();

        // vectors to far corners
        let del0 = b0 * i0.length();
        let del1 = b1 * i1.length();
        let del2 = b2 * i2.length();

        // hop around to all other corners
        let c1 = c0 + del0;
        let c2 = c1 + del1;
        let c3 = c0 + del1;
        [c0, c1, c2, c3, c0 + del2, c1 + del2, c2 + del2, c3 + del2]
    }

    /// The twelve edges as 24 end points; points `2k` and `2k + 1` bound edge `k`
    /// (`GetEdges`).
    #[must_use]
    pub fn edges(&self) -> [Point3; 24] {
        const N: [[f64; 3]; 24] = [
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
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

    /// The box projected onto a plane, starting from [`PseudoBox::new`]
    /// (`ProjectToPlane` into a fresh `SmPseudoBox`).
    ///
    /// # Errors
    ///
    /// As [`PseudoBox::project_to_plane_into`].
    pub fn project_to_plane(&self, proj: &Projection) -> Result<Self> {
        let mut out = Self::new();
        self.project_to_plane_into(proj, &mut out)?;
        Ok(out)
    }

    /// Writes into `out` a box bounding the projection of `self` (`ProjectToPlane`).
    ///
    /// The new basis puts `x` along the longest projected sized basis vector, `y`
    /// along the next independent one and `z` along their cross product; the
    /// intervals bound the corners projected *parallel* onto the plane (for every
    /// projection type, as upstream), plus, for [`Projection::Rotation`], the edge
    /// points where the rotated edges (parabolas) are extreme along the new `x` and
    /// `y` axes.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when no basis can be built around a near-zero
    /// longest projected basis vector (upstream `MakeUnitOrthoVectors` failing; `out`
    /// is then left with "inside out" intervals and basis vector 0 set to
    /// `(0, 0, 1)`).
    pub fn project_to_plane_into(&self, proj: &Projection, out: &mut Self) -> Result<()> {
        let proj_point = proj.point();
        let proj_unit = proj.unit_vector();

        let mut len = [0.0_f64; 4]; // projected vector * Ivl.size lengths
        let mut basis = [Vec3::UNDEF; 4]; // projected basis vectors

        // get pseudo box corners
        let mut corners = self.corners();

        // project every Basis vector into array basis sorted by projected length
        for (ii, (b, ivl)) in self.basis.iter().zip(&self.intervals).enumerate() {
            let projected = match *proj {
                Projection::Parallel { normal, .. } => b.project_to_plane(normal),
                Projection::Rotation { axis, x_axis, .. } => {
                    b.rotate_project_to_plane(axis, x_axis)
                }
                Projection::Perspective {
                    point, normal, eye, ..
                } => b.perspective_project_to_plane(point, point, normal, eye),
            };
            basis[3] = projected;
            len[3] = projected.length() * (ivl.max() - ivl.min());
            match ii {
                0 => {
                    basis[1] = basis[3];
                    len[1] = len[3];
                }
                1 => {
                    basis[2] = basis[3];
                    len[2] = len[3];
                    if len[2] > len[1] {
                        len[0] = len[2];
                        basis[0] = basis[2];
                    } else {
                        len[0] = len[1];
                        basis[0] = basis[1];
                        len[1] = len[2];
                        basis[1] = basis[2];
                    }
                }
                _ => {
                    if len[3] > len[0] {
                        len[2] = len[1];
                        basis[2] = basis[1];
                        len[1] = len[0];
                        basis[1] = basis[0];
                        len[0] = len[3];
                        basis[0] = basis[3];
                    } else if len[3] > len[1] {
                        len[2] = len[1];
                        basis[2] = basis[1];
                        len[1] = len[3];
                        basis[1] = basis[3];
                    } else {
                        len[2] = len[3];
                        basis[2] = basis[3];
                    }
                }
            }
        }

        // init output - set out to an empty space, leave basis vectors alone
        out.init();

        if len[0] < EFF_ZERO {
            // when all basis vectors project to zero length - make any basis vector set
            out.init_basis();
        } else {
            // make basis set placing 'X' axis in direction of largest projected vector
            let x_cross_y = basis[0].cross(basis[1]);
            let x_cross_z = basis[0].cross(basis[2]);

            if x_cross_y.length_squared() > EFF_ZERO_SQ {
                // Projected X and Y are independent; upstream ignores the status
                let _ = out.set_basis(
                    basis[0].unitize_or_z(),
                    basis[1].unitize_or_z(),
                    x_cross_y.unitize_or_z(),
                );
            } else if x_cross_z.length_squared() > EFF_ZERO_SQ {
                // Projected X and Z are independent; upstream ignores the status
                let _ = out.set_basis(
                    basis[0].unitize_or_z(),
                    basis[2].unitize_or_z(),
                    x_cross_z.unitize_or_z(),
                );
            } else {
                // build any orthogonal basis about basis[0] placing X and Y in the
                // projection plane
                let y_ref = match *proj {
                    Projection::Rotation { axis, x_axis, .. } => {
                        let x_cross_rot_axis = basis[0].cross(axis);
                        let x_cross_x_axis = basis[0].cross(x_axis);
                        if x_cross_rot_axis.length_squared() > x_cross_x_axis.length_squared() {
                            axis
                        } else {
                            x_axis
                        }
                    }
                    _ => basis[0].cross(proj_unit),
                };
                match basis[0].make_unit_ortho_vectors(Some(y_ref)) {
                    // upstream writes the vectors straight into m_aBasis (the
                    // orthogonality flag is not refreshed)
                    Ok(axes) => out.basis = axes,
                    Err(e) => {
                        // keep what upstream wrote into m_aBasis before failing
                        for (slot, w) in out
                            .basis
                            .iter_mut()
                            .zip(basis[0].unit_ortho_vectors_on_failure())
                        {
                            if let Some(w) = w {
                                *slot = w;
                            }
                        }
                        return Err(e);
                    }
                }
            }
        }

        // Size new pseudo box by adding every original corner to it
        for c in &mut corners {
            *c = c.project_point_to_plane(proj_point, proj_unit);
            out.add_point(*c);
        }

        if let Projection::Rotation {
            point: c_pt,
            axis: n,
            x_axis: x,
        } = *proj
        {
            add_rotated_edge_extrema(&corners, c_pt, n, x, proj_point, proj_unit, out);
        }
        Ok(())
    }
}

/// The `SM_PT_ROTATION` part of `ProjectToPlane`: adds to `out` the (parallel
/// projected) edge points where the rotated-projected edges of the box are extreme
/// along `out`'s first two basis vectors (see the derivation in upstream
/// `SmPseudoBox::ProjectToPlane`).
fn add_rotated_edge_extrema(
    corners: &[Point3; 8],
    c: Vec3,
    n: Vec3,
    x: Vec3,
    proj_point: Point3,
    proj_unit: Vec3,
    out: &mut PseudoBox,
) {
    let a0 = out.basis[0];
    let a1 = out.basis[1];
    let dcc = c.dot(c);
    let a0x = a0.dot(x);
    let a0n = a0.dot(n);
    let a1x = a1.dot(x);
    let a1n = a1.dot(n);

    for &(i0, i1) in &EDGE_CORNERS {
        let (Some(&p0), Some(&p1)) = (corners.get(i0), corners.get(i1)) else {
            continue;
        };
        let p10 = p1 - p0;

        // build quadratic term EdgePoint dependent constants
        let p0p0 = p0.dot(p0);
        let p0p1 = p0.dot(p1);
        let p1p1 = p1.dot(p1);

        let p0p10 = p0.dot(p10);
        let p1p10 = p1.dot(p10);
        let p0n = p0.dot(n);
        let p1n = p1.dot(n);
        let p10n = p10.dot(n);
        let p0c = p0.dot(c);
        let p1c = p1.dot(c);
        let p10c = p10.dot(c);
        let cn = c.dot(n);

        let ax_ss_coef =
            (p0p10 * p0p10) - 2.0 * (p0p10 * p1p10) + (p1p10 * p1p10) + (p0n * p0n * p10n * p10n)
                - 2.0 * (p0n * p1n * p10n * p10n)
                + (p1n * p1n * p10n * p10n)
                + (p0p10 * p0n * -2.0 * p10n)
                - (p1p10 * p0n * -2.0 * p10n)
                - (p0p10 * p1n * -2.0 * p10n)
                + (p1p10 * p1n * -2.0 * p10n);
        let ax_s_coef = 2.0 * (p0p10 * p1p10) - 2.0 * (p0p10 * p0p10) - 2.0 * (p0p10 * -p10c)
            + 2.0 * (p1p10 * -p10c)
            - 2.0 * (p0p10 * p0n * -2.0 * p10n)
            + (p1p10 * p0n * -2.0 * p10n)
            - (-p10c * p0n * -2.0 * p10n)
            + (p0p10 * p1n * -2.0 * p10n)
            + (-p10c * p1n * -2.0 * p10n)
            - (p0p10 * -cn * -2.0 * p10n)
            + (p1p10 * -cn * -2.0 * p10n)
            - 2.0 * (p0n * p0n * p10n * p10n)
            + 2.0 * (p1n * p0n * p10n * p10n)
            - 2.0 * (p0n * -cn * p10n * p10n)
            + 2.0 * (p1n * -cn * p10n * p10n);
        let ax_const = 1.0 * (p0p10 * p0p10)
            + 2.0 * (p0p10 * -p10c)
            + 1.0 * (-p10c * -p10c)
            + 1.0 * (p0p10 * p0n * -2.0 * p10n)
            + 1.0 * (p0p10 * -cn * -2.0 * p10n)
            + 1.0 * (-p10c * p0n * -2.0 * p10n)
            + 1.0 * (-p10c * -cn * -2.0 * p10n)
            + 1.0 * (p0n * p0n * p10n * p10n)
            + 2.0 * (p0n * -cn * p10n * p10n)
            + 1.0 * (-cn * -cn * p10n * p10n);

        let an_ss_coef =
            1.0 * (p0p0 + p0n * -p0n) - 2.0 * (p0p1 + p1n * -p0n) + 1.0 * (p1p1 + p1n * -p1n);
        let an_s_coef = -2.0 * (p0p0 + p0n * -p0n) + 2.0 * (p0p1 + p1n * -p0n)
            - 2.0 * (-p0c + p0n * cn)
            + 2.0 * (-p1c + p1n * cn);
        let an_const = 1.0 * (p0p0 + p0n * -p0n) + 2.0 * (-p0c + p0n * cn) + 1.0 * (dcc + -cn * cn);

        // for both basis vectors
        for (ax, an) in [(a0x, a0n), (a1x, a1n)] {
            // build the quadratic coefficients
            let coef = [
                ax * ax * ax_ss_coef - p10n * p10n * an * an * an_ss_coef,
                ax * ax * ax_s_coef - p10n * p10n * an * an * an_s_coef,
                ax * ax * ax_const - p10n * p10n * an * an * an_const,
            ];

            // solve for s in A*s*s + B*s*s + C = 0
            let scaled_zero = EFF_ZERO * (1.0 + max3(coef[0], coef[1], coef[2]));
            let sols = solve_quadratic_eqn(&coef, scaled_zero);

            for (mm, &s) in sols.iter().enumerate() {
                // look for unique solutions between 0 and 1
                // SM_IS_BETWEEN_TO_TOL(s, 0, 1, EFF_ZERO); its reversed-interval half
                // (1 + tol < s < 0 - tol) is always false
                let between = (0.0 + EFF_ZERO) < s && s < (1.0 - EFF_ZERO);
                let unique = mm == 0
                    || match (sols.first(), sols.get(1)) {
                        (Some(&s0), Some(&s1)) => !is_zero_to_tol(s0 - s1, EFF_ZERO),
                        _ => true,
                    };
                if between && unique {
                    // evaluate 3d Point P(s)
                    let p = (1.0 - s) * p0 + s * p1;
                    // Project EdgePoint to Plane and add to outputBox
                    let p = p.project_point_to_plane(proj_point, proj_unit);
                    out.add_point(p);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rotated_basis() -> [Vec3; 3] {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        [
            Vec3::new(s, 0.0, -s),
            Vec3::new(0.5, s, 0.5),
            Vec3::new(0.5, -s, 0.5),
        ]
    }

    /// `my_test_pseudobox` (`tests/prog_test/src/smCurveTest.cpp`).
    #[test]
    fn my_test_pseudobox() {
        let [b0, b1, b2] = rotated_basis();
        let mut pb = PseudoBox::new();
        pb.set_basis(b0, b1, b2).unwrap();
        pb.add_point(Vec3::new(0.3, 0.2, 0.1));
        pb.add_point(Vec3::new(0.2, 0.3, 0.4));
        pb.add_point(Vec3::new(0.25, 0.4, 0.3));
        assert!(pb.distance_to_point(Vec3::new(0.25, 0.3, 0.2)) <= EFF_ZERO);
        assert!(pb.distance_to_point(Vec3::new(6.0, 3.0, 0.5)) >= EFF_ZERO);

        let mut pb2 = PseudoBox::new();
        pb2.add_point(Vec3::new(0.8, 0.2, 0.3));
        pb2.add_point(Vec3::new(0.9, 0.3, 0.4));
        assert!(pb.are_disjoint(&pb2, None));
        pb2.add_point(Vec3::new(0.25, 0.4, 0.3));
        assert!(!pb.are_disjoint(&pb2, None));
    }

    #[test]
    fn axis_aligned_box_queries() {
        let e = Extent3d::from_bounds(0.0, 0.0, 0.0, 2.0, 3.0, 4.0);
        let pb = PseudoBox::from_extent3d(&e);
        assert_eq!(pb.volume(), 24.0);
        assert_eq!(pb.xy_area(), 6.0);
        assert_eq!(pb.max_dimension(), 4.0);
        assert_eq!(pb.is_axis_aligned(), Some([1, 2, 3]));
        assert!(pb.contains_point(Vec3::new(1.0, 1.0, 1.0), 0.0));
        assert_eq!(pb.distance_to_point(Vec3::new(5.0, 1.0, 1.0)), 3.0);
        assert_eq!(pb.distance_to_point(Vec3::new(5.0, 7.0, 1.0)), 5.0);
        assert_eq!(
            pb.intersect_line(Vec3::new(-1.0, 1.0, 1.0), Vec3::X)
                .unwrap(),
            LineHit::Segment(1.0, 3.0)
        );
        assert_eq!(
            pb.intersect_line(Vec3::new(-1.0, 9.0, 1.0), Vec3::X)
                .unwrap(),
            LineHit::Miss
        );
        assert!(pb.intersect_line(Vec3::ZERO, Vec3::ZERO).is_err());
        let c = pb.corners();
        assert_eq!(c[6], Vec3::new(2.0, 3.0, 4.0));
        assert_eq!(pb.edges()[11], Vec3::new(2.0, 3.0, 4.0));
        assert!(pb.is_bounded());
        assert!(!pb.is_planar(EFF_ZERO).is_planar);
        let flat = pb.project_to_plane(&Projection::Parallel {
            point: Vec3::ZERO,
            normal: Vec3::Z,
        });
        let flat = flat.unwrap();
        assert!(flat.is_planar(EFF_ZERO).is_planar);
    }

    #[test]
    fn singular_basis_rejected() {
        let mut pb = PseudoBox::new();
        assert!(pb.set_basis(Vec3::X, Vec3::X, Vec3::Z).is_err());
        assert_eq!(pb, PseudoBox::new());
        assert!(pb.set_interval(3, Extent1d::new()).is_err());
    }

    /// Upstream quirk kept by `Extent3d::circumscribe`: the basis -> axis map is
    /// read as axis -> basis.
    #[test]
    fn circumscribe_axis_cycle_quirk() {
        let ivl = Extent1d::from_min_max;
        let pb = PseudoBox::from_basis_intervals(
            [Vec3::Y, Vec3::Z, Vec3::X],
            [ivl(1.0, 2.0), ivl(3.0, 5.0), ivl(-7.0, -6.0)],
        );
        assert_eq!(pb.is_axis_aligned(), Some([2, 3, 1]));
        let e = Extent3d::from_pseudo_box(&pb);
        assert_eq!(e.min(), Vec3::new(3.0, -7.0, 1.0));
        assert_eq!(e.max(), Vec3::new(5.0, -6.0, 2.0));
    }

    /// A singular (uninitialised) basis with huge intervals reaches the failing
    /// `MakeUnitOrthoVectors` branch of `ProjectToPlane`.
    #[test]
    fn project_make_ortho_failure() {
        let ivl = Extent1d::from_min_max;
        let pb = PseudoBox::from_basis_intervals(
            [Vec3::X, Vec3::X, Vec3::Z],
            [ivl(-1e70, 1e70), ivl(-1e70, 1e70), ivl(0.0, 1e70)],
        );
        assert!(!pb.assert_defined());
        let mut out = PseudoBox::new();
        out.set_basis(Vec3::Y, Vec3::Z, Vec3::X).unwrap();
        let proj = Projection::Parallel {
            point: Vec3::ZERO,
            normal: Vec3::Z,
        };
        assert!(pb.project_to_plane_into(&proj, &mut out).is_err());
        assert_eq!(out.basis_vector(0), Some(Vec3::Z));
        assert!(out.intervals().iter().all(Extent1d::is_init));
    }
}
