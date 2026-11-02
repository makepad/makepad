//! Projected curves: the image of a 3D curve under a parallel or perspective
//! projection onto a view plane, or a rotation about an axis into a plane.
//!
//! Upstream: `source/SMLib/src/SmProjectedCurve.cpp`, `source/SMLib/inc/SmProjectedCurve.h`.
//!
//! | Rust                                         | Upstream                                         |
//! |----------------------------------------------|--------------------------------------------------|
//! | [`ProjectedCurve`]                           | `SmProjectedCurve`                               |
//! | [`OwnsCurve`]                                | `m_bOwnsCurve` (`SmBoolean`: `FALSE`, `TRUE`, `UNSURE`) |
//! | [`ProjectedCurve::new`]                      | `SmProjectedCurve(const SmCurve*, ...)`          |
//! | `Clone`                                      | the copy constructor                             |
//! | [`ProjectedCurve::copy`]                     | `Copy`                                           |
//! | `Drop` (implicit)                            | `~SmProjectedCurve`                              |
//! | [`ProjectedCurve::approx_eq`]                | `operator==`                                     |
//! | [`ProjectedCurve::set_canonical`]            | `SetCanonical`                                   |
//! | [`ProjectedCurve::make_ortho_normal`]        | `MakeOrthoNormal`                                |
//! | [`ProjectedCurve::edit_parameterization`]    | `EditParameterization`                           |
//! | [`ProjectedCurve::natural_interval`]         | `GetNaturalInterval`                             |
//! | [`ProjectedCurve::knots`]                    | `GetKnots`                                       |
//! | [`ProjectedCurve::number_natural_knots`]     | `GetNumberNaturalKnots`                          |
//! | [`ProjectedCurve::calculate_continuities`]   | `CalculateContinuities`                          |
//! | [`ProjectedCurve::reverse_parameterization`] | `ReverseParameterization`                        |
//! | [`ProjectedCurve::trim`]                     | `Trim`                                           |
//! | [`ProjectedCurve::transform`]                | `Transform`                                      |
//! | [`ProjectedCurve::evaluate`]                 | `Evaluate`                                       |
//! | [`ProjectedCurve::evaluate_point`]           | `EvaluatePoint`                                  |
//! | [`ProjectedCurve::read_from_db`]             | `ReadFromDB`                                     |
//! | [`ProjectedCurve::is_analytic`], [`ProjectedCurve::is_bounded`] | `IsAnalytic`, `IsBounded`     |
//! | [`ProjectedCurve::TYPE`], [`ProjectedCurve::type_code`], [`ProjectedCurve::type_string`], [`ProjectedCurve::class_string`] | `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`ProjectedCurve::is_kind_of`]               | `IsKindOf`                                       |
//!
//! Deviations:
//! - **Nesting.** `new` and `set_canonical` return `InvalidInput` when the projected curve would nest
//!   deeper than [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING)
//!   (32; upstream fixtures nest at most 3), which bounds the recursive dispatch,
//!   cloning and dropping of nested geometry (no limit upstream).
//! - **Base curve.** Upstream keeps a pointer to the projected curve that it owns
//!   or merely borrows (`m_bOwnsCurve`). Here the projected curve always holds its
//!   own [`Curve`] (callers that borrowed a curve upstream pass a clone); the
//!   ownership flag is kept as [`OwnsCurve`] because upstream's edits test it
//!   (`EditParameterization`, `ReverseParameterization` and `Trim` refuse a
//!   `FALSE` owner, `Transform` anything below `UNSURE`). The copy constructor
//!   (`Clone`) copies an owned base curve (`Copy`, whose B-spline copy fixes its
//!   knots up) and a borrowed one exactly (no knot fix-up, owner kept), the values
//!   upstream has when it shares the pointer; [`ProjectedCurve::copy`] (`Copy`) does
//!   the same. When the owned base
//!   curve has no `Copy` (a Hermite curve, `SM_ERR`) upstream ignores the status
//!   and returns a projected curve with a null base curve, which crashes on first
//!   use; [`ProjectedCurve::copy`] returns that `SM_ERR` instead, and `Clone` (which
//!   cannot fail) copies such a base anyway.
//! - **Curve base.** The class derives from `SmCurve`; its base state is the
//!   dimension, always 3 (`SmCurve(3)`). `SmCurve::operator==` (same type, same
//!   dimension) is implied by the Rust type. `SetContext`, the context arguments
//!   and the `Notify` calls (`SM_NO_PRE_EDIT` / `SM_NO_POST_EDIT`) are not
//!   modelled: the curve object has no context to reach.
//! - **Arguments.** Out-params become return values. `EditParameterization`'s
//!   notify flag has no counterpart ([`Curve::edit_parameterization`] does not
//!   notify); `Trim`'s tri-state flag is [`TrimMode`]; `Trim`'s debug-check flag is
//!   dropped.
//! - **Projection type.** `SmProjectionType` is
//!   [`ProjectionType`]; a value read from a database outside `0..=2` becomes
//!   [`ProjectionType::Unknown`] (upstream casts the raw value to the enum, which is
//!   undefined beyond `0..=3`; every method treats such values as "no projection").
//! - **`ReadFromDB`** always builds a new curve (upstream's "fill the curve passed
//!   in" branch and its `IsKindOf` check have no counterpart; the unused type and
//!   dimension arguments are dropped). An ASCII value whose extraction is skipped
//!   (the stream already failed) stays `SM_UNDEF_DOUBLE`, the member initialiser;
//!   a projection type or type header that is never extracted is
//!   [`SolidError::InvalidInput`] (upstream reads an uninitialised local).
//! - `GetRootCurve`, `DrawPolygon`, `GetMemoryUsed`, `WriteToDB`, `AssertValid`
//!   and `Dump` are not part of this unit.
//! - Upstream behaviours kept as they are (see `plan/findings.md`): `Transform`
//!   fails unless the ownership flag is `UNSURE`. `ReadFromDB` does not call
//!   `MakeOrthoNormal`.

use crate::curve::curve::p2::{self as curve_base, Curve, PROJECTED_CURVE_TYPE};
use crate::error::{Result, SolidError, SM_ERR_INVALID_INPUT};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, EFF_ZERO_SQ};
use crate::nurbs::b_spline_curve::ProjectionType;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::surface::surface::p2::{check_geometry_nesting, NestedGeometry};
use crate::topology::database_io::{DatabaseReader, FileType, InputStream};

const OP: &str = "curve::projected_curve::ProjectedCurve";
const OP_READ: &str = "curve::projected_curve::ProjectedCurve::read_from_db";

/// Whether a projected curve owns its base curve (`m_bOwnsCurve`, an `SmBoolean`
/// whose three values the edits test differently).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OwnsCurve {
    /// `FALSE` (the constructor default): the base curve is borrowed.
    #[default]
    No = 0,
    /// `TRUE`: the base curve is owned (always after `ReadFromDB`).
    Yes = 1,
    /// `UNSURE` (2): the member initialiser; the only value `Transform` accepts.
    Unsure = 2,
}

/// A curve projected onto a plane (`SmProjectedCurve`).
#[derive(Debug)]
pub struct ProjectedCurve {
    /// `m_cpCurve`: the curve being projected.
    curve: Box<Curve>,
    /// `m_sProjPoint`: a point on the view plane (parallel, perspective) or on the
    /// rotation axis (rotation).
    proj_point: Point3,
    /// `m_sProjVec`: the view plane normal (parallel, perspective) or the rotation
    /// axis vector (rotation).
    proj_vec: Vec3,
    /// `m_sAuxData`: unused (parallel), the eye point (perspective), or the X axis
    /// of the plane spanned with the rotation axis (rotation).
    aux_data: Vec3,
    /// `m_eProjType`.
    proj_type: ProjectionType,
    /// `m_bOwnsCurve`.
    owns_curve: OwnsCurve,
}

/// The copy constructor: an owned base curve is copied ([`Curve::clone`], whose
/// B-spline copy fixes its knots up), a borrowed one exactly (the bounded exact copy:
/// no knot fix-up, owner kept), which is what sharing the borrowed pointer upstream
/// amounts to. Every other member is copied verbatim, and the enclosing
/// [`Curve::clone`] still resets *its* owner and identity.
///
/// The exact branch recurses over the copied base without a depth argument; that is
/// sound because every path that stores a base curve keeps the graph within
/// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING):
/// [`ProjectedCurve::new`] and [`ProjectedCurve::set_canonical`] check it,
/// [`ProjectedCurve::read_from_db`] reads its base through [`Curve::read_from_db`]
/// (bounded by [the database reader's nesting
/// limit](crate::topology::database_io::MAX_READ_NESTING)), and propagates from
/// bounded instances.
impl Clone for ProjectedCurve {
    fn clone(&self) -> Self {
        let curve = if self.owns_curve == OwnsCurve::No {
            self.curve.clone_exact_bounded()
        } else {
            (*self.curve).clone()
        };
        Self {
            curve: Box::new(curve),
            proj_point: self.proj_point,
            proj_vec: self.proj_vec,
            aux_data: self.aux_data,
            proj_type: self.proj_type,
            owns_curve: self.owns_curve,
        }
    }
}

impl ProjectedCurve {
    /// Exact evaluation copy after the caller validates the geometry depth.
    pub(crate) fn clone_exact_bounded(&self) -> Self {
        Self {
            curve: Box::new(self.curve.clone_exact_bounded()),
            proj_point: self.proj_point,
            proj_vec: self.proj_vec,
            aux_data: self.aux_data,
            proj_type: self.proj_type,
            owns_curve: self.owns_curve,
        }
    }

    /// An exact evaluation copy; `depth` is the base curve's depth in the shared
    /// `MAX_GEOMETRY_NESTING` (32) curve/surface/volume traversal.
    pub(crate) fn clone_for_evaluation(&self, depth: usize) -> Result<Self> {
        Ok(Self {
            curve: Box::new(self.curve.clone_for_evaluation_depth(depth)?),
            proj_point: self.proj_point,
            proj_vec: self.proj_vec,
            aux_data: self.aux_data,
            proj_type: self.proj_type,
            owns_curve: self.owns_curve,
        })
    }

    /// `SmProjectedCurve_TYPE`.
    pub const TYPE: i64 = PROJECTED_CURVE_TYPE;

    /// A projection of `curve` (constructor). Data not given keeps the member
    /// initialiser ([`Vec3::UNDEF`]); when `proj_vec` is given the vectors go
    /// through [`Self::make_ortho_normal`], whose status is ignored as upstream.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when `curve` nests deeper than
    /// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING) - 1.
    pub fn new(
        curve: Curve,
        proj_point: Option<Point3>,
        proj_vec: Option<Vec3>,
        aux_data: Option<Vec3>,
        proj_type: ProjectionType,
        owns_curve: OwnsCurve,
    ) -> Result<Self> {
        check_geometry_nesting(NestedGeometry::Curve(&curve), 1, OP)?;
        let mut c = Self {
            curve: Box::new(curve),
            proj_point: proj_point.unwrap_or(Vec3::UNDEF),
            proj_vec: proj_vec.unwrap_or(Vec3::UNDEF),
            aux_data: aux_data.unwrap_or(Vec3::UNDEF),
            proj_type,
            owns_curve,
        };
        if proj_vec.is_some() {
            let _ = c.make_ortho_normal();
        }
        Ok(c)
    }

    /// The curve being projected.
    #[must_use]
    pub fn curve(&self) -> &Curve {
        &self.curve
    }

    /// The point on the view plane or rotation axis (`m_sProjPoint`).
    #[must_use]
    pub const fn proj_point(&self) -> Point3 {
        self.proj_point
    }

    /// The view plane normal or rotation axis (`m_sProjVec`).
    #[must_use]
    pub const fn proj_vec(&self) -> Vec3 {
        self.proj_vec
    }

    /// The eye point or rotation plane X axis (`m_sAuxData`).
    #[must_use]
    pub const fn aux_data(&self) -> Vec3 {
        self.aux_data
    }

    /// The projection type (`m_eProjType`).
    #[must_use]
    pub const fn proj_type(&self) -> ProjectionType {
        self.proj_type
    }

    /// The ownership flag (`m_bOwnsCurve`).
    #[must_use]
    pub const fn owns_curve(&self) -> OwnsCurve {
        self.owns_curve
    }

    // ---- type queries ---------------------------------------------------------------

    /// `SmProjectedCurve_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmProjectedCurve_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmProjectedCurve_TYPE"
    }

    /// `"SmProjectedCurve"` (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        "SmProjectedCurve"
    }

    /// True for `SmProjectedCurve_TYPE` and the `SmCurve` base types (`IsKindOf`).
    #[must_use]
    pub const fn is_kind_of(t: i64) -> bool {
        t == Self::TYPE || curve_base::is_kind_of(t)
    }

    /// Image-space dimension: always 3 (`SmCurve(3)`).
    #[must_use]
    pub const fn dim(&self) -> u64 {
        3
    }

    /// `IsAnalytic`: `FALSE`.
    #[must_use]
    pub const fn is_analytic(&self) -> bool {
        false
    }

    /// `IsBounded`: that of the base curve.
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        self.curve.is_bounded()
    }

    /// `operator==`: same object, or equal base curves (under
    /// [`Curve::approx_eq`]) and equal projection data (under [`Vec3::approx_eq`]).
    ///
    /// # Errors
    ///
    /// Those of [`Curve::approx_eq`] on the base curves.
    pub fn approx_eq(&self, k: &Kernel, other: &Self) -> Result<bool> {
        // low work
        if core::ptr::eq(self, other) {
            return Ok(true);
        }

        // check equivalence of these objects
        Ok((core::ptr::eq(self.curve.as_ref(), other.curve.as_ref())
            || self.curve.approx_eq(k, &other.curve)?)
            && self.proj_point.approx_eq(other.proj_point)
            && self.proj_vec.approx_eq(other.proj_vec)
            && self.aux_data.approx_eq(other.aux_data)
            && self.proj_type == other.proj_type)
    }

    /// `Copy`: a copy of the projected curve (copy constructor).
    ///
    /// # Errors
    ///
    /// When the base curve is owned, those of its [`Curve::copy`] (`SM_ERR` for a
    /// Hermite curve; upstream ignores it, see the module notes); when it is borrowed,
    /// those of [`Curve::clone_for_evaluation`] (geometry nested too deeply).
    pub fn copy(&self, k: &Kernel) -> Result<Self> {
        let curve = if self.owns_curve == OwnsCurve::No {
            // upstream shares the borrowed base curve: an exact copy that keeps its
            // owner (`Curve::clone` would fix its knots up)
            self.curve.clone_for_evaluation()?
        } else {
            // copy the Curve
            self.curve.copy(k)?
        };
        Ok(Self {
            curve: Box::new(curve),
            proj_point: self.proj_point,
            proj_vec: self.proj_vec,
            aux_data: self.aux_data,
            proj_type: self.proj_type,
            owns_curve: self.owns_curve,
        })
    }

    // ---- set up ---------------------------------------------------------------------

    /// Sets every canonical field, then [`Self::make_ortho_normal`]
    /// (`SetCanonical`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` when `curve` nests deeper than
    /// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING) - 1
    /// (all fields are left unchanged); `SM_ERR` from [`Self::make_ortho_normal`]
    /// (the fields are set anyway).
    pub fn set_canonical(
        &mut self,
        curve: Curve,
        proj_point: Point3,
        proj_vec: Vec3,
        aux_data: Vec3,
        proj_type: ProjectionType,
        owns_curve: OwnsCurve,
    ) -> Result<()> {
        check_geometry_nesting(NestedGeometry::Curve(&curve), 1, OP)?;
        *self.curve = curve;
        self.proj_point = proj_point;
        self.proj_vec = proj_vec;
        self.aux_data = aux_data;
        self.proj_type = proj_type;
        self.owns_curve = owns_curve;

        // unitize and orthoganlize ProjVec and AuxData
        self.make_ortho_normal()
    }

    /// Unitizes the projection vector and, for rotations, replaces the auxiliary
    /// vector by its unit component normal to the projection vector
    /// (`MakeOrthoNormal`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the projection vector (or, for rotations, the auxiliary
    /// vector) is uninitialised or zero; `SM_ERR_INVALID_INPUT` when the auxiliary
    /// vector is parallel to the projection vector (it becomes `(0, 0, 1)`).
    pub fn make_ortho_normal(&mut self) -> Result<()> {
        // when ProjVec is nonZero - unitize it
        let proj_init = self.proj_vec.is_initialized();
        let proj_zero = self.proj_vec.is_zero(EFF_ZERO);
        if proj_init && !proj_zero {
            self.proj_vec = self.proj_vec.unitize_or_z();
        } else {
            return Err(SolidError::sm_err(OP));
        }

        // for rotations - make the sAuxData vector orthonormal to the ProjVec
        if self.proj_type == ProjectionType::Rotation {
            // when xAuxData is nonZero - make orthonormal to ProjVec
            let aux_init = self.aux_data.is_initialized();
            let aux_zero = self.aux_data.is_zero(EFF_ZERO);
            if aux_init && !aux_zero {
                self.aux_data = self.proj_vec.normal_in_plane(self.aux_data);
                // SER(Unitize()): a zero vector becomes (0, 0, 1) and fails
                let zero = self.aux_data.length_squared() < EFF_ZERO_SQ;
                self.aux_data = self.aux_data.unitize_or_z();
                if zero {
                    return Err(SolidError::Upstream {
                        operation: OP,
                        code: SM_ERR_INVALID_INPUT,
                        entity: None,
                    });
                }
            } else {
                return Err(SolidError::sm_err(OP));
            }
        }
        Ok(())
    }

    // ---- queries passed to the base curve -------------------------------------------

    /// The natural interval of the base curve (`GetNaturalInterval`).
    #[must_use]
    pub fn natural_interval(&self) -> Extent1d {
        // pass the call along
        self.curve.natural_interval()
    }

    /// Unique knots and multiplicities of the base curve (`GetKnots`).
    ///
    /// # Errors
    ///
    /// Those of [`Curve::knots`].
    pub fn knots(&self, k: &Kernel, interval: Option<&Extent1d>) -> Result<(Vec<f64>, Vec<usize>)> {
        // pass the call along
        self.curve.knots(k, interval)
    }

    /// The sum of the knot multiplicities over the natural interval
    /// (`GetNumberNaturalKnots`); `0` when the knots cannot be read (upstream
    /// ignores the status).
    ///
    /// # Errors
    ///
    /// Only [`SolidError::Unported`].
    pub fn number_natural_knots(&self, k: &Kernel) -> Result<usize> {
        let (_, mults) = match self.knots(k, None) {
            Ok(km) => km,
            Err(e @ SolidError::Unported { .. }) => return Err(e),
            Err(_) => (Vec::new(), Vec::new()),
        };

        // count the knots
        Ok(mults.iter().sum())
    }

    /// The continuities of the base curve (`CalculateContinuities`).
    ///
    /// # Errors
    ///
    /// Those of [`Curve::calculate_continuities`].
    pub fn calculate_continuities(
        &self,
        k: &Kernel,
        angle_tol: f64,
    ) -> Result<(ContinuityType, Vec<ContinuityType>)> {
        // pass the call along
        self.curve.calculate_continuities(k, angle_tol)
    }

    // ---- edits ----------------------------------------------------------------------

    /// `EditParameterization` of the base curve.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the base curve is not owned; those of
    /// [`Curve::edit_parameterization`].
    pub fn edit_parameterization(&mut self, k: &Kernel, new_param: &Extent1d) -> Result<()> {
        // Don't own curve can not reverse parameterization
        if self.owns_curve == OwnsCurve::No {
            return Err(SolidError::sm_err(OP));
        }

        // pass the call along to m_cpCurve
        self.curve.edit_parameterization(k, new_param)
    }

    /// `ReverseParameterization` of the base curve: the new interval.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the base curve is not owned; those of
    /// [`Curve::reverse_parameterization`].
    pub fn reverse_parameterization(
        &mut self,
        k: &Kernel,
        old_interval: &Extent1d,
    ) -> Result<Extent1d> {
        // Don't own curve can not reverse parameterization
        if self.owns_curve == OwnsCurve::No {
            return Err(SolidError::sm_err(OP));
        }

        // pass the call along to the cpCurve
        self.curve.reverse_parameterization(k, old_interval)
    }

    /// `Trim` of the base curve; `trim_interval` may be snapped.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the base curve is not owned; those of [`Curve::trim`].
    pub fn trim(&mut self, k: &Kernel, trim_interval: &mut Extent1d, mode: TrimMode) -> Result<()> {
        self.trim_with_zone_tol(
            k,
            trim_interval,
            mode,
            crate::geom::tol::zone_tol_3d(k, crate::geom::tol::SmallTopoType::NormalSize),
        )
    }

    /// Trims the owned curve with the caller-resolved owning object's tolerance.
    pub(crate) fn trim_with_zone_tol(
        &mut self,
        k: &Kernel,
        trim_interval: &mut Extent1d,
        mode: TrimMode,
        zone_tol_3d: f64,
    ) -> Result<()> {
        // Don't own curve can not trim the curve
        if self.owns_curve == OwnsCurve::No {
            return Err(SolidError::sm_err(OP));
        }

        // pass the call along to the UVCurve
        self.curve
            .trim_with_zone_tol(k, trim_interval, mode, zone_tol_3d)
    }

    /// `Transform`: nothing for an identity placement without (or with unit)
    /// scale; else transforms the base curve, then the projection data.
    ///
    /// # Errors
    ///
    /// `SM_ERR` unless the ownership flag is [`OwnsCurve::Unsure`] (upstream tests
    /// `m_bOwnsCurve < 2`); those of [`Curve::transform`].
    pub fn transform(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
    ) -> Result<()> {
        self.transform_at_depth(k, rotate_n_move, scale, 0)
    }

    pub(crate) fn transform_at_depth(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
        depth: usize,
    ) -> Result<()> {
        let identity_scale = Vec3::new(1.0, 1.0, 1.0);

        // no work - identity transform
        if rotate_n_move.is_identity(EFF_ZERO) && scale.is_none_or(|s| s.approx_eq(identity_scale))
        {
            return Ok(());
        }

        // Don't own surface, can not transform the curve
        if self.owns_curve != OwnsCurve::Unsure {
            return Err(SolidError::sm_err(OP));
        }

        // pass the call along to the curve
        self.curve
            .transform_at_depth(k, rotate_n_move, scale, depth + 1)?;

        // apply rotations and moves to points and vecs
        self.proj_point = rotate_n_move.transform_point(self.proj_point);
        self.proj_vec = rotate_n_move.transform_vector(self.proj_vec);
        if self.proj_type == ProjectionType::Perspective {
            self.aux_data = rotate_n_move.transform_point(self.aux_data);
        } else if self.proj_type == ProjectionType::Rotation {
            self.aux_data = rotate_n_move.transform_vector(self.aux_data);
        }

        // apply opt scaling to points
        if let Some(s) = scale {
            // scale the points
            self.proj_point.x *= s.x;
            self.proj_point.y *= s.y;
            self.proj_point.z *= s.z;

            if self.proj_type == ProjectionType::Perspective {
                self.aux_data.x *= s.x;
                self.aux_data.y *= s.y;
                self.aux_data.z *= s.z;
            }
        }
        Ok(())
    }

    // ---- evaluation -----------------------------------------------------------------

    /// Position and derivatives `0..=num_derivs` of the base curve at `t`, projected
    /// (`Evaluate`). A perspective projection of the derivatives uses the projected
    /// position, as upstream.
    ///
    /// # Errors
    ///
    /// Those of [`Curve::evaluate`].
    pub fn evaluate(
        &self,
        k: &Kernel,
        t: f64,
        num_derivs: usize,
        from_left: bool,
        non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        // pass the call along
        let mut pd = self
            .curve
            .evaluate(k, t, num_derivs, from_left, non_zero_tangents)?;

        // project the results
        match self.proj_type {
            ProjectionType::Parallel => {
                let mut it = pd.iter_mut();
                // position point
                if let Some(p) = it.next() {
                    *p = p.project_point_to_plane(self.proj_point, self.proj_vec);
                }
                // derivative vectors
                for d in it.take(num_derivs) {
                    *d = d.project_to_plane(self.proj_vec);
                }
            }
            ProjectionType::Perspective => {
                let mut it = pd.iter_mut();
                // position point
                let mut base = Vec3::UNDEF;
                if let Some(p) = it.next() {
                    *p = p.perspective_project_point_to_plane(
                        self.proj_point,
                        self.proj_vec,
                        self.aux_data,
                    );
                    base = *p;
                }
                // derivative vectors
                for d in it.take(num_derivs) {
                    *d = d.perspective_project_to_plane(
                        base,
                        self.proj_point,
                        self.proj_vec,
                        self.aux_data,
                    );
                }
            }
            ProjectionType::Rotation => {
                let mut it = pd.iter_mut();
                // position point
                if let Some(p) = it.next() {
                    *p = p.rotate_project_point_to_plane(
                        self.proj_point,
                        self.proj_vec,
                        self.aux_data,
                    );
                }
                // derivative vectors
                for d in it.take(num_derivs) {
                    *d = d.rotate_project_to_plane(self.proj_vec, self.aux_data);
                }
            }
            ProjectionType::Unknown => {}
        }
        Ok(pd)
    }

    /// The projected point of the base curve at `t` (`EvaluatePoint`).
    ///
    /// # Errors
    ///
    /// Those of [`Curve::evaluate_point`].
    pub fn evaluate_point(&self, k: &Kernel, t: f64) -> Result<Point3> {
        // base curve evaluation
        let pnt = self.curve.evaluate_point(k, t)?;

        let mut point = pnt.project_point_to_plane(self.proj_point, self.proj_vec);

        // project the point
        match self.proj_type {
            ProjectionType::Parallel => {
                point = pnt.project_point_to_plane(self.proj_point, self.proj_vec);
            }
            ProjectionType::Perspective => {
                point = pnt.perspective_project_point_to_plane(
                    self.proj_point,
                    self.proj_vec,
                    self.aux_data,
                );
            }
            ProjectionType::Rotation => {
                point = pnt.rotate_project_point_to_plane(
                    self.proj_point,
                    self.proj_vec,
                    self.aux_data,
                );
            }
            ProjectionType::Unknown => {}
        }
        Ok(point)
    }

    // ---- persistence ----------------------------------------------------------------

    /// Reads a projected curve written in database format `version` (`ReadFromDB`):
    /// the projection data (versions above 34 include the auxiliary vector), then
    /// the base curve's type header and the base curve. The result owns its base
    /// curve; its vectors are not normalised.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a short binary stream or an ASCII value
    /// upstream would leave uninitialised; those of [`Curve::read_from_db`].
    pub fn read_from_db(k: &Kernel, db: &mut DatabaseReader, version: u64) -> Result<Self> {
        let undefined = || SolidError::InvalidInput {
            operation: OP_READ,
            reason: "value not extracted from a failed ASCII stream",
        };
        let ascii = db.file_type() == FileType::Ascii;

        // locals
        let mut proj_point = Vec3::UNDEF;
        let mut proj_vec = Vec3::UNDEF;
        let mut aux_data = Vec3::UNDEF;
        // version 34 = pre-rotation extension database format
        // version 36 = post-rotation extension database format (added member m_sAuxData)
        let with_aux = version > 34;
        let proj_type = if ascii {
            let mut vecs: Vec<&mut Vec3> = vec![&mut proj_point, &mut proj_vec];
            if with_aux {
                vecs.push(&mut aux_data);
            }
            for v in vecs {
                if let Some(s) = db.in_stream_mut() {
                    for coord in [&mut v.x, &mut v.y, &mut v.z] {
                        if let Some(x) = s.extract_double() {
                            *coord = x;
                        }
                    }
                }
                db.go_to_next_line();
            }
            let pt = db.in_stream_mut().and_then(InputStream::extract_ulong);
            db.go_to_next_line();
            pt.ok_or_else(undefined)?
        } else {
            let mut vecs: Vec<&mut Vec3> = vec![&mut proj_point, &mut proj_vec];
            if with_aux {
                vecs.push(&mut aux_data);
            }
            for v in vecs {
                v.x = db.read_double()?;
                v.y = db.read_double()?;
                v.z = db.read_double()?;
            }
            db.read_ulong()?
        };

        // base curve
        if ascii {
            db.go_to_next_line();
        }
        let header = db.read_type(true)?;
        let curve_type = header.type_code.ok_or_else(undefined)?;
        let curve_dim = header.dim.ok_or_else(undefined)?;
        let curve = Curve::read_from_db(k, curve_type, db, curve_dim, version)?;

        // load the obj
        Ok(Self {
            curve: Box::new(curve),
            proj_point,
            proj_vec,
            aux_data,
            proj_type: ProjectionType::from_code(proj_type).unwrap_or(ProjectionType::Unknown),
            owns_curve: OwnsCurve::Yes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::hermite_curve::HermiteCurve;

    fn base() -> Curve {
        Curve::from(HermiteCurve::new(
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, 2.0),
            Vec3::new(2.0, 1.0, 3.0),
            Vec3::new(1.0, 1.0, 0.0),
            3,
        ))
    }

    #[test]
    fn set_canonical_rejects_excessive_nesting_without_changing_fields() {
        use crate::surface::surface::p2::MAX_GEOMETRY_NESTING;

        let mut curve = base();
        for _ in 0..MAX_GEOMETRY_NESTING {
            curve = Curve::from(
                ProjectedCurve::new(
                    curve,
                    None,
                    None,
                    None,
                    ProjectionType::Parallel,
                    OwnsCurve::No,
                )
                .unwrap(),
            );
        }
        let mut projected = ProjectedCurve::new(
            base(),
            None,
            None,
            None,
            ProjectionType::Unknown,
            OwnsCurve::No,
        )
        .unwrap();
        let before = format!("{projected:?}");
        assert!(matches!(
            projected.set_canonical(
                curve,
                Vec3::ZERO,
                Vec3::ZERO,
                Vec3::ZERO,
                ProjectionType::Rotation,
                OwnsCurve::Yes,
            ),
            Err(SolidError::InvalidInput { .. })
        ));
        assert_eq!(format!("{projected:?}"), before);
        // a zero projection vector fails, the fields are set anyway
        assert!(projected
            .set_canonical(
                base(),
                Vec3::ZERO,
                Vec3::ZERO,
                Vec3::ZERO,
                ProjectionType::Parallel,
                OwnsCurve::Yes,
            )
            .is_err());
        assert_eq!(projected.proj_type(), ProjectionType::Parallel);
        assert!(projected
            .set_canonical(
                base(),
                Vec3::ZERO,
                Vec3::Z,
                Vec3::ZERO,
                ProjectionType::Parallel,
                OwnsCurve::Yes,
            )
            .is_ok());
    }

    #[test]
    fn nonzero_proj_vec_is_unitized() {
        let mut c = ProjectedCurve::new(
            base(),
            Some(Vec3::new(0.0, 0.0, 0.0)),
            Some(Vec3::new(0.0, 0.0, 2.0)),
            None,
            ProjectionType::Parallel,
            OwnsCurve::No,
        )
        .unwrap();
        assert_eq!(c.proj_vec(), Vec3::Z);
        assert!(c.make_ortho_normal().is_ok());
    }

    #[test]
    fn rotation_aux_is_made_orthonormal() {
        let mut c = ProjectedCurve::new(
            base(),
            Some(Vec3::new(0.0, 0.0, 0.0)),
            Some(Vec3::new(0.0, 0.0, 2.0)),
            Some(Vec3::new(3.0, 0.0, 5.0)),
            ProjectionType::Rotation,
            OwnsCurve::No,
        )
        .unwrap();
        assert_eq!(c.proj_vec(), Vec3::Z);
        assert!((c.aux_data() - Vec3::X).length() < 1e-15);
        assert!(c.make_ortho_normal().is_ok());
        // an auxiliary vector along the projection vector has no normal part
        c.aux_data = Vec3::new(0.0, 0.0, 4.0);
        assert!(matches!(
            c.make_ortho_normal(),
            Err(SolidError::Upstream {
                code: SM_ERR_INVALID_INPUT,
                ..
            })
        ));
        assert_eq!(c.aux_data(), Vec3::Z);
    }

    #[test]
    fn zero_proj_vec_is_rejected() {
        let mut c = ProjectedCurve::new(
            base(),
            Some(Vec3::new(0.0, 0.0, 0.0)),
            Some(Vec3::new(0.0, 0.0, 0.0)),
            Some(Vec3::new(0.0, 0.0, 0.0)),
            ProjectionType::Rotation,
            OwnsCurve::No,
        )
        .unwrap();
        assert_eq!(c.proj_vec(), Vec3::ZERO);
        assert_eq!(c.aux_data(), Vec3::ZERO);
        assert!(c.make_ortho_normal().is_err());
    }

    #[test]
    fn parallel_projection_flattens() {
        let k = &Kernel::default();
        let c = ProjectedCurve::new(
            base(),
            Some(Vec3::new(0.0, 0.0, -1.0)),
            None,
            None,
            ProjectionType::Parallel,
            OwnsCurve::No,
        )
        .unwrap();
        let mut c2 = c.clone();
        c2.proj_vec = Vec3::Z;
        let p = c2.evaluate_point(k, 0.5).expect("eval");
        assert!((p.z + 1.0).abs() < 1e-12);
        let pd = c2.evaluate(k, 0.3, 2, true, false).expect("eval");
        assert_eq!(pd.len(), 3);
        for v in pd.iter().skip(1) {
            assert!(v.z.abs() < 1e-12);
        }
        assert!(c.approx_eq(k, &c.clone()).unwrap());
        assert!(!c.approx_eq(k, &c2).unwrap());
    }

    #[test]
    fn edits_respect_ownership() {
        let k = Kernel::default();
        let mut c = ProjectedCurve::new(
            base(),
            None,
            None,
            None,
            ProjectionType::Parallel,
            OwnsCurve::No,
        )
        .unwrap();
        assert!(c
            .edit_parameterization(&k, &Extent1d::from_min_max(0.0, 2.0))
            .is_err());
        let mut a = Axis2Placement::new(Vec3::new(1.0, 0.0, 0.0), Vec3::X, Vec3::Y);
        assert!(c.transform(&k, &a, None).is_err());
        a = Axis2Placement::new(Vec3::new(0.0, 0.0, 0.0), Vec3::X, Vec3::Y);
        assert!(c.transform(&k, &a, None).is_ok());
        let (_, mults) = c.knots(&k, None).expect("knots");
        assert_eq!(
            c.number_natural_knots(&k).expect("knots"),
            mults.iter().sum::<usize>()
        );
        // owned Hermite base has no Copy
        c.owns_curve = OwnsCurve::Yes;
        assert!(c.copy(&k).is_err());
    }

    #[test]
    fn reads_ascii() {
        let text = "1 2 3 c\n0 0 1 c\n4 5 6 c\n1 c\n ProjectedCurve->ProjectedCurve \n\
                    Object Type 14034\nCurve Dim 3\n\
                    0 0 0\n1 0 0\n1 1 0\n0 1 0\n0 0 0\n0 0 0\n";
        let mut db = DatabaseReader::new();
        db.open_for_read(text.as_bytes().to_vec(), FileType::Ascii);
        let c = ProjectedCurve::read_from_db(&Kernel::default(), &mut db, 36).expect("read");
        assert_eq!(c.proj_point(), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(c.aux_data(), Vec3::new(4.0, 5.0, 6.0));
        assert_eq!(c.proj_type(), ProjectionType::Perspective);
        assert_eq!(c.owns_curve(), OwnsCurve::Yes);
        assert_eq!(c.curve().type_code(), HermiteCurve::TYPE);
    }
}
