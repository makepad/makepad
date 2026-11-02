//! The tangent field of a surface boundary: cross-boundary tangents interpolated
//! between a start and an end vector and projected onto the tangent plane of a
//! surface along a parameter-space curve (`SmTangentField`, a curve on a surface).
//!
//! Upstream: `source/SMLib/src/SmTangentField.cpp`, `source/SMLib/inc/SmTangentField.h`.
//!
//! | Rust                                        | Upstream                                     |
//! |---------------------------------------------|----------------------------------------------|
//! | [`TangentField`]                            | `SmTangentField`                             |
//! | [`TangentDirType`]                          | `SmTangentDirType`                           |
//! | [`TangentField::new`]                       | `SmTangentField::SmTangentField` (data constructor) |
//! | `Drop` (implicit)                           | `SmTangentField::~SmTangentField`            |
//! | [`TangentField::approx_eq`]                 | `SmTangentField::operator==`                 |
//! | [`TangentField::create`], [`CurveEnd`]      | `SmTangentField::Create`                     |
//! | [`TangentField::evaluate_point`]            | `SmTangentField::EvaluatePoint`              |
//! | [`TangentField::evaluate`]                  | `SmTangentField::Evaluate`                   |
//! | [`read_from_db`], [`TangentFieldRecord`]    | `SmTangentField::ReadFromDB`                 |
//! | [`is_kind_of`], [`TangentField::is_kind_of`] | `SmTangentField::IsKindOf`                  |
//! | [`TYPE`], [`TYPE_STRING`], [`CLASS_STRING`] | `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`TangentFieldCurve`]                       | `SmTangentField` as a curve object (`CurveKind::TangentField`) |
//! | [`TangentFieldCurve::new`], [`TangentFieldCurve::create`] | the data constructor, `Create` |
//! | [`TangentFieldCurve::evaluate`], [`TangentFieldCurve::evaluate_point`] | `Evaluate`, `EvaluatePoint` |
//! | [`TangentFieldCurve::approx_eq`]            | `operator==`                                 |
//! | [`TangentFieldCurve::read_from_db`]         | `ReadFromDB` (through `SmCurve::ReadFromDB`) |
//! | `write_tangent_field` (`topology::geometry_write`) | `WriteToDB`                           |
//!
//! Deviations:
//! - **Base class.** `SmTangentField` derives from `SmCrvOnSurf` (unit
//!   `curve.crv_on_surf`, [`CrvOnSurf`]). The members it needs are kept here: the UV
//!   curve, the surface and the UV domain. Like the constructor (owner flag `0`, no
//!   context), the tangent field references both the UV curve and the surface without
//!   owning them, so they are borrowed (`&'a`). The surface type is a parameter `S`
//!   (a [`crate::surface::surface::p2::SurfaceEvaluate`] where the constructors need
//!   the natural UV domain). Three trivial members of the base are co-ported
//!   (`// CO-PORT(curve.crv_on_surf)`): `SetAll` for a given UV domain (it only stores
//!   the references; the context and owner bookkeeping is not modelled),
//!   `GetNaturalInterval` (the UV curve's interval), `IsKindOf`, and `operator==`
//!   (same UV curve, same surface, same UV domain).
//! - **Unported callees.** Comparing two different surfaces needs `SmSurface::operator==`
//!   (`surface.surface_p1`); evaluating inside the interval uses the surface's
//!   `EvaluateNormal` (`SurfaceEvaluate::evaluate_normal`). Reading the `SmCrvOnSurf`
//!   part of a database record ([`CrvOnSurf::read_from_db`]) reads its surface with
//!   `SmSurface::ReadFromDB`, whose non-B-spline types are unported.
//! - **Arguments.** Out-params become return values; the `SmContext` of `Create` and
//!   `ReadFromDB`, `ReadFromDB`'s type and the `NotUsed` arguments of `Evaluate`
//!   (`bFromLeft`, `bNonZeroTangents`) are dropped. `Create`'s base UV curve is typed
//!   `SmBSplineCurve` upstream; here it is any [`Curve`]. Equality compares two tangent
//!   fields (upstream's `SmCurve::operator==` type and dimension check is always true
//!   between them).
//! - **Curve kind.** [`TangentFieldCurve`] is the object `SmCurve::ReadFromDB` makes
//!   (`CurveKind::TangentField`): an owned [`CrvOnSurf`] base plus the members. The
//!   [`Curve`] dispatch uses the `SmTangentField` overrides (`Evaluate`,
//!   `EvaluatePoint`, `operator==`, `WriteToDB`, the type queries) and the base for
//!   everything else; the base-class methods that call a virtual on `this`
//!   (`CalculateContinuities`, `CalculateBoundingBox`, `DropPoint`, `ApproximateCurve`
//!   through `GetOrCreateBSApproxPointer`) are run on the derived object. `Copy` is not
//!   overridden upstream (`new SmCrvOnSurf(*this)`): the copy is a plain curve on
//!   surface. Code that matches `CurveKind::CrvOnSurf` directly (upstream
//!   `IsKindOf(SmCrvOnSurf_TYPE)` casts on B-rep curves) does not see a tangent field;
//!   upstream only makes tangent fields as temporaries of `CreateCornerBlend`.
//! - **Base read failure.** [`TangentFieldCurve::read_from_db`] returns the error of the
//!   `SmCrvOnSurf` read; upstream ignores it and returns an object whose UV curve and
//!   surface are null (every later query dereferences them).
//! - **`ReadFromDB`** (the free [`read_from_db`]) returns the tangent-field members it read ([`TangentFieldRecord`])
//!   instead of an object; the `SmCrvOnSurf` part is read and dropped. Upstream ignores the
//!   status of that base read; a read error of the members themselves is returned as
//!   upstream (`SER`). The empty constructor (`SmTangentField()`, for I/O) has no
//!   counterpart.

use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::p2::{
    self as curve_base, at, Curve, CurveEvaluate, CRV_ON_SURF_TYPE, TANGENT_FIELD_TYPE,
};
use crate::error::{ignore_status, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, PI};
use crate::surface::surface::p2::{Surface, SurfaceEvaluate};
use crate::topology::database_io::{DatabaseReader, FileType};

/// `SmTangentField_TYPE` (`GetType`).
pub const TYPE: i64 = TANGENT_FIELD_TYPE;
/// `SmTangentField::GetTypeString`.
pub const TYPE_STRING: &str = "SmTangentField_TYPE";
/// `SmTangentField::GetClassString`.
pub const CLASS_STRING: &str = "SmTangentField";

const OP_CREATE: &str = "curve::tangent_field::TangentField::create";
const OP_EVALUATE: &str = "curve::tangent_field::TangentField::evaluate";

/// `SmTangentField::IsKindOf`: `SmTangentField_TYPE` or a base type
/// (`SmCrvOnSurf`, `SmCurve`, ...).
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    // CO-PORT(curve.crv_on_surf): SmCrvOnSurf::IsKindOf
    t == TANGENT_FIELD_TYPE || t == CRV_ON_SURF_TYPE || curve_base::is_kind_of(t)
}

/// How the tangents vary along the field (`SmTangentDirType`). Upstream only
/// supports [`TangentDirType::Interpolate`]; the stored type is not used otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TangentDirType {
    /// `SM_CONSTANT_DIR` (`0`).
    Constant,
    /// `SM_INTERPOLATE_DIR` (`1`).
    Interpolate,
    /// `SM_PERPENDICULAR_DIR` (`2`).
    Perpendicular,
}

impl TangentDirType {
    /// The upstream enum value.
    #[must_use]
    pub const fn code(self) -> u64 {
        match self {
            Self::Constant => 0,
            Self::Interpolate => 1,
            Self::Perpendicular => 2,
        }
    }
}

/// Orientation of an end curve's tangent (`SmOrientType`, `SM_OT_SAME` /
/// `SM_OT_OPPOSITE`) and where it is taken, for [`TangentField::create`].
#[derive(Debug, Clone, Copy)]
pub struct CurveEnd<'c> {
    /// The 3D curve meeting the base curve at this end.
    pub curve: &'c Curve,
    /// Parameter of the mated point on `curve`.
    pub param: f64,
    /// `true` (`SM_OT_OPPOSITE`): the field value is minus the curve's tangent.
    pub opposite: bool,
}

/// A tangent field (`SmTangentField`): the curve on `surface` traced by `uv_curve`,
/// whose value at a parameter is the cosine-blended interpolation of the start and
/// end tangents projected onto the surface's tangent plane.
#[derive(Debug)]
pub struct TangentField<'a, S> {
    /// `m_pUVCurve`: the parameter-space curve (not owned).
    uv_curve: &'a Curve,
    /// `m_pSurface`: the base surface (not owned).
    surface: &'a S,
    /// `m_vUVDomain`: the domain of interest on the surface.
    uv_domain: Extent2d,
    /// `m_eType`.
    dir_type: TangentDirType,
    /// `m_vStartTangentField`: the cross tangent at the start point.
    start_tangent: Vec3,
    /// `m_vEndTangentField`: the cross tangent at the end point.
    end_tangent: Vec3,
}

impl<S> Clone for TangentField<'_, S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for TangentField<'_, S> {}

impl<'a, S> TangentField<'a, S> {
    /// The data constructor: the field of `start_tangent` .. `end_tangent` along
    /// `uv_curve` on `surface`, restricted to `uv_domain` (`None` = the surface's
    /// natural UV domain).
    ///
    /// # Errors
    ///
    /// Those of the surface's natural UV domain when `uv_domain` is `None` (a lazy
    /// analytic surface makes its NURB first, [`SolidError::Unported`]).
    pub fn new(
        k: &Kernel,
        uv_curve: &'a Curve,
        surface: &'a S,
        dir_type: TangentDirType,
        start_tangent: Vec3,
        end_tangent: Vec3,
        uv_domain: Option<Extent2d>,
    ) -> Result<Self>
    where
        S: SurfaceEvaluate,
    {
        // CO-PORT(curve.crv_on_surf): SmCrvOnSurf::SetAll (owner flag 0, no context)
        // set m_vUVDomain
        let uv_domain = match uv_domain {
            Some(d) => d,
            None => surface.natural_uv_domain_with_nurb(k)?,
        };
        Ok(Self {
            uv_curve,
            surface,
            uv_domain,
            dir_type,
            start_tangent,
            end_tangent,
        })
    }

    /// The convenience constructor: the start and end tangents are those of
    /// `start` and `end` (negated when `opposite`), whose points must lie within
    /// `approx_tol_3d` of the ends of `base_3d_curve` (only used for this check).
    /// The field runs along `base_uv_curve` on `surface`, interpolated.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when an end point does not match; those of the curve evaluations;
    /// None from the constructor (it takes the surface's natural UV domain).
    pub fn create(
        k: &Kernel,
        approx_tol_3d: f64,
        base_3d_curve: &Curve,
        base_uv_curve: &'a Curve,
        surface: &'a S,
        start: CurveEnd<'_>,
        end: CurveEnd<'_>,
    ) -> Result<Self>
    where
        S: SurfaceEvaluate,
    {
        let (start_tangent, end_tangent) =
            end_tangents(k, approx_tol_3d, base_3d_curve, start, end)?;

        // allocate SmTangentField
        Self::new(
            k,
            base_uv_curve,
            surface,
            TangentDirType::Interpolate, // for now, only value supported
            start_tangent,
            end_tangent,
            None,
        )
    }

    /// The parameter-space curve (`GetUVCurve`).
    #[must_use]
    pub const fn uv_curve(&self) -> &'a Curve {
        self.uv_curve
    }

    /// The base surface (`GetBaseSurface`).
    #[must_use]
    pub const fn surface(&self) -> &'a S {
        self.surface
    }

    /// The domain of interest on the surface (`GetUVDomain`).
    #[must_use]
    pub const fn uv_domain(&self) -> Extent2d {
        self.uv_domain
    }

    /// The interpolation type (`m_eType`).
    #[must_use]
    pub const fn dir_type(&self) -> TangentDirType {
        self.dir_type
    }

    /// The stored start tangent (`m_vStartTangentField`).
    #[must_use]
    pub const fn start_tangent(&self) -> Vec3 {
        self.start_tangent
    }

    /// The stored end tangent (`m_vEndTangentField`).
    #[must_use]
    pub const fn end_tangent(&self) -> Vec3 {
        self.end_tangent
    }

    /// `SmTangentField_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        TYPE
    }

    /// `"SmTangentField_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        TYPE_STRING
    }

    /// `"SmTangentField"` (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        CLASS_STRING
    }

    /// Whether the field is of type `t` or derives from it (`IsKindOf`).
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        is_kind_of(t)
    }

    /// The natural parameter interval: the UV curve's (`GetNaturalInterval`).
    #[must_use]
    pub fn natural_interval(&self) -> Extent1d {
        // CO-PORT(curve.crv_on_surf): SmCrvOnSurf::GetNaturalInterval
        self.uv_curve.natural_interval()
    }

    /// `operator==`: the same UV curve (object or value), the same surface object
    /// (or equal surfaces), the same UV domain, then the same type and end tangents
    /// (upstream's tolerant vector equality).
    ///
    /// # Errors
    ///
    /// Those of the curve and surface comparisons.
    pub fn approx_eq(&self, k: &Kernel, other: &Self) -> Result<bool>
    where
        S: SurfaceEvaluate,
    {
        // low work
        if core::ptr::eq(self, other) {
            return Ok(true);
        }

        // first check the base
        // CO-PORT(curve.crv_on_surf): SmCrvOnSurf::operator==
        // (SmCurve::operator==: same type, both of dimension 3)
        let mut rtn = true;
        rtn &= core::ptr::eq(self.uv_curve, other.uv_curve)
            || self.uv_curve.approx_eq(k, other.uv_curve)?;
        rtn &= core::ptr::eq(self.surface, other.surface)
            || surface_eq(k, self.surface, other.surface)?;
        rtn &= self.uv_domain.are_same(&other.uv_domain);

        if rtn {
            // check equivalence of these objects
            rtn = self.dir_type == other.dir_type
                && self.start_tangent.approx_eq(other.start_tangent)
                && self.end_tangent.approx_eq(other.end_tangent);
        }

        // all done
        Ok(rtn)
    }

    /// The field value at `t` (`EvaluatePoint`): the stored start (end) tangent
    /// within `1e-8` of the start (end) of the interval; elsewhere the tangents
    /// blended by `T = 0.5 cos(pi s) + 0.5` (`s` the normalised parameter, so the
    /// blend has zero slope at both ends) as `T start + (1 - T) end`, projected onto
    /// the tangent plane of the surface at `uv_curve(t)`.
    ///
    /// # Errors
    ///
    /// Those of the UV curve's evaluation; [`SolidError::Unported`] inside the
    /// interval (`EvaluateNormal` of the surface).
    pub fn evaluate_point(&self, k: &Kernel, t: f64) -> Result<Point3>
    where
        S: SurfaceEvaluate,
    {
        field_point(
            k,
            self.uv_curve,
            self.surface,
            self.start_tangent,
            self.end_tangent,
            t,
        )
    }

    /// The field value at `t` and, for `num_derivs == 1`, its derivative by a forward
    /// difference of step `1e-8` (`Evaluate`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for `num_derivs > 1`; those of [`TangentField::evaluate_point`].
    pub fn evaluate(&self, k: &Kernel, t: f64, num_derivs: usize) -> Result<Vec<Vec3>>
    where
        S: SurfaceEvaluate,
    {
        field_evaluate(|t| self.evaluate_point(k, t), t, num_derivs)
    }
}

/// `Create`'s end tangents: those of `start` and `end` (negated when `opposite`), whose
/// points must lie within `approx_tol_3d` of the ends of `base_3d_curve`.
fn end_tangents(
    k: &Kernel,
    approx_tol_3d: f64,
    base_3d_curve: &Curve,
    start: CurveEnd<'_>,
    end: CurveEnd<'_>,
) -> Result<(Vec3, Vec3)> {
    // StartCurve position and tangent
    let pv = start.curve.evaluate(k, start.param, 1, true, true)?;
    let mut start_tangent = at(&pv, 1);
    if start.opposite {
        start_tangent = -start_tangent;
    }

    // Base3DCurve start position
    let ivl = base_3d_curve.natural_interval();
    let start_pnt = base_3d_curve.evaluate_point(k, ivl.evaluate(0.0))?;

    // check state: Start points must match
    let dist = at(&pv, 0).distance(start_pnt);
    if dist > approx_tol_3d {
        // Ends don't match
        return Err(SolidError::sm_err(OP_CREATE));
    }

    // EndCurve position and tangent
    let pv = end.curve.evaluate(k, end.param, 1, true, true)?;
    let mut end_tangent = at(&pv, 1);
    if end.opposite {
        end_tangent = -end_tangent;
    }

    // base3DCurve end position
    let end_pnt = base_3d_curve.evaluate_point(k, ivl.evaluate(1.0))?;

    // check state: End points must match
    let dist = at(&pv, 0).distance(end_pnt);
    if dist > approx_tol_3d {
        // Ends don't match
        return Err(SolidError::sm_err(OP_CREATE));
    }

    Ok((start_tangent, end_tangent))
}

/// `EvaluatePoint` on the parts of a tangent field (the borrowed [`TangentField`] and
/// the owned [`TangentFieldCurve`] share it).
fn field_point<S: SurfaceEvaluate>(
    k: &Kernel,
    uv_curve: &Curve,
    surface: &S,
    start_tangent: Vec3,
    end_tangent: Vec3,
    t: f64,
) -> Result<Point3> {
    // locals
    let param_tol = 1.0e-8;
    // CO-PORT(curve.crv_on_surf): SmCrvOnSurf::GetNaturalInterval
    let ivl = uv_curve.natural_interval();

    // Start Param Value - return stored StartTangent
    if t < ivl.min() + param_tol + EFF_ZERO {
        return Ok(start_tangent);
    }

    // End Param Value - return stored EndTangent
    if t > ivl.max() - param_tol - EFF_ZERO {
        return Ok(end_tangent);
    }

    // linear normalized parameter
    let mut dt = (t - ivl.min()) / ivl.length();

    // Cosine smoothed normalized parameter - dT/dParameter at 0 and 1 = 0
    // This yields a vanishing twist vector at the ends of the tangent interpolation.
    // note: dT = 1.0 at minParam and dT = 0.0 at maxParam
    dt *= PI;
    dt = 0.5 * dt.cos() + 0.5;

    // sTangentField(dParam) value is an interpolated value of the stored endTangentField values
    let tangent_field = start_tangent * dt + end_tangent * (1.0 - dt);

    // Get NormalPlane to m_pSurface at the m_pUVCurve(dParameter) point
    let pnt = uv_curve.evaluate_point(k, t)?;
    let uv = Point2::new(pnt.x, pnt.y);

    let unit_normal = surface_evaluate_normal(k, surface, uv)?;

    // return value = sTangentField projection onto the tangent plane
    Ok(tangent_field.project_to_plane(unit_normal))
}

/// `Evaluate` over the field's (virtual) `EvaluatePoint`.
fn field_evaluate(
    evaluate_point: impl Fn(f64) -> Result<Point3>,
    t: f64,
    num_derivs: usize,
) -> Result<Vec<Vec3>> {
    if num_derivs == 0 {
        return Ok(vec![evaluate_point(t)?]);
    }

    // get 1st derivative using finite differences
    if num_derivs == 1 {
        // Currently just use divided diff.
        let p0 = evaluate_point(t)?;
        let delta_t = 1.0e-8;
        let param1 = t + delta_t;
        let tangent1 = evaluate_point(param1)?;
        let diff = tangent1 - p0;
        return Ok(vec![p0, diff / delta_t]);
    }
    // Can not go higher than 1st derivative
    Err(SolidError::sm_err(OP_EVALUATE))
}

// ---- the curve kind -----------------------------------------------------------------

/// A tangent field as a curve object ([`crate::curve::curve::CurveKind::TangentField`]):
/// an `SmCrvOnSurf` base owning (or sharing) its UV curve and surface, plus the
/// tangent-field members. Only the virtuals `SmTangentField` overrides
/// (`Evaluate`, `EvaluatePoint`, `operator==`, `WriteToDB`, the type queries) are its
/// own; every other curve query is the base's ([`Self::base`]).
#[derive(Debug, Clone)]
pub struct TangentFieldCurve {
    /// The `SmCrvOnSurf` part.
    base: CrvOnSurf,
    /// `m_eType`, as stored (upstream casts whatever a file holds).
    dir_type: u64,
    /// `m_vStartTangentField`.
    start_tangent: Vec3,
    /// `m_vEndTangentField`.
    end_tangent: Vec3,
}

/// The members `SmTangentField` adds to its `SmCrvOnSurf` base: what a base-class
/// method needs to rebuild the derived object for its virtual calls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TangentFieldMembers {
    /// `m_eType`.
    pub(crate) dir_type: u64,
    /// `m_vStartTangentField`.
    pub(crate) start_tangent: Vec3,
    /// `m_vEndTangentField`.
    pub(crate) end_tangent: Vec3,
}

impl TangentFieldCurve {
    /// `SmTangentField_TYPE`.
    pub const TYPE: i64 = TANGENT_FIELD_TYPE;

    /// The data constructor: the field of `start_tangent` .. `end_tangent` along
    /// `uv_curve` on `surface`, restricted to `uv_domain` (`None` = the surface's
    /// natural UV domain). The base shares both parts (owner flag 0).
    ///
    /// # Errors
    ///
    /// Those of [`CrvOnSurf::new`].
    pub fn new(
        k: &Kernel,
        uv_curve: Curve,
        surface: Surface,
        dir_type: TangentDirType,
        start_tangent: Vec3,
        end_tangent: Vec3,
        uv_domain: Option<Extent2d>,
    ) -> Result<Self> {
        Ok(Self {
            base: CrvOnSurf::new(k, uv_curve, surface, uv_domain, 0)?,
            dir_type: dir_type.code(),
            start_tangent,
            end_tangent,
        })
    }

    /// The convenience constructor (`Create`), as [`TangentField::create`]: the end
    /// tangents of `start` and `end`, interpolated along `base_uv_curve` on `surface`
    /// over the surface's natural UV domain.
    ///
    /// # Errors
    ///
    /// As [`TangentField::create`]; those of [`CrvOnSurf::new`].
    pub fn create(
        k: &Kernel,
        approx_tol_3d: f64,
        base_3d_curve: &Curve,
        base_uv_curve: Curve,
        surface: Surface,
        start: CurveEnd<'_>,
        end: CurveEnd<'_>,
    ) -> Result<Self> {
        let (start_tangent, end_tangent) =
            end_tangents(k, approx_tol_3d, base_3d_curve, start, end)?;

        // allocate SmTangentField
        Self::new(
            k,
            base_uv_curve,
            surface,
            TangentDirType::Interpolate, // for now, only value supported
            start_tangent,
            end_tangent,
            None,
        )
    }

    /// The tangent field on `base` with the members `m` (upstream's `this` when a
    /// base-class method of `base` calls a virtual).
    pub(crate) const fn from_parts(base: CrvOnSurf, m: TangentFieldMembers) -> Self {
        Self {
            base,
            dir_type: m.dir_type,
            start_tangent: m.start_tangent,
            end_tangent: m.end_tangent,
        }
    }

    /// The tangent-field members (without the base).
    pub(crate) const fn members(&self) -> TangentFieldMembers {
        TangentFieldMembers {
            dir_type: self.dir_type,
            start_tangent: self.start_tangent,
            end_tangent: self.end_tangent,
        }
    }

    /// The `SmCrvOnSurf` part.
    #[must_use]
    pub const fn base(&self) -> &CrvOnSurf {
        &self.base
    }

    /// The `SmCrvOnSurf` part, consuming the field.
    pub(crate) fn into_base(self) -> CrvOnSurf {
        self.base
    }

    /// The `SmCrvOnSurf` part, mutably (the base-class edits).
    pub(crate) fn base_mut(&mut self) -> &mut CrvOnSurf {
        &mut self.base
    }

    /// An exact evaluation copy (the base's [`CrvOnSurf::clone_for_evaluation`]).
    pub(crate) fn clone_for_evaluation(&self, depth: usize) -> Result<Self> {
        Ok(Self {
            base: self.base.clone_for_evaluation(depth)?,
            dir_type: self.dir_type,
            start_tangent: self.start_tangent,
            end_tangent: self.end_tangent,
        })
    }

    /// The raw interpolation type (`m_eType`).
    #[must_use]
    pub const fn dir_type_code(&self) -> u64 {
        self.dir_type
    }

    /// The stored start tangent (`m_vStartTangentField`).
    #[must_use]
    pub const fn start_tangent(&self) -> Vec3 {
        self.start_tangent
    }

    /// The stored end tangent (`m_vEndTangentField`).
    #[must_use]
    pub const fn end_tangent(&self) -> Vec3 {
        self.end_tangent
    }

    /// `SmTangentField_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        TYPE
    }

    /// `"SmTangentField_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        TYPE_STRING
    }

    /// `"SmTangentField"` (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        CLASS_STRING
    }

    /// Whether the field is of type `t` or derives from it (`IsKindOf`).
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        is_kind_of(t)
    }

    /// The natural parameter interval (the base's `GetNaturalInterval`).
    #[must_use]
    pub fn natural_interval(&self) -> Extent1d {
        self.base.natural_interval()
    }

    /// `operator==`: the bases are equal (`SmCrvOnSurf::operator==`), then the same
    /// type and end tangents (upstream's tolerant vector equality).
    ///
    /// # Errors
    ///
    /// Those of [`CrvOnSurf::approx_eq`].
    pub fn approx_eq(&self, k: &Kernel, other: &Self) -> Result<bool> {
        // low work
        if core::ptr::eq(self, other) {
            return Ok(true);
        }

        // first check the base
        let mut rtn = self.base.approx_eq(k, &other.base)?;

        if rtn {
            // check equivalence of these objects
            rtn = self.dir_type == other.dir_type
                && self.start_tangent.approx_eq(other.start_tangent)
                && self.end_tangent.approx_eq(other.end_tangent);
        }

        // all done
        Ok(rtn)
    }

    /// The field value at `t` (`EvaluatePoint`), as [`TangentField::evaluate_point`].
    ///
    /// # Errors
    ///
    /// Those of the UV curve's evaluation and the surface normal.
    pub fn evaluate_point(&self, k: &Kernel, t: f64) -> Result<Point3> {
        field_point(
            k,
            self.base.uv_curve(),
            self.base.surface(),
            self.start_tangent,
            self.end_tangent,
            t,
        )
    }

    /// The field value at `t` and, for `num_derivs == 1`, its forward-difference
    /// derivative (`Evaluate`), as [`TangentField::evaluate`].
    ///
    /// # Errors
    ///
    /// `SM_ERR` for `num_derivs > 1`; those of [`Self::evaluate_point`].
    pub fn evaluate(&self, k: &Kernel, t: f64, num_derivs: usize) -> Result<Vec<Vec3>> {
        field_evaluate(|t| self.evaluate_point(k, t), t, num_derivs)
    }

    /// `SmTangentField::ReadFromDB` into a new object: the tangent-field members, then
    /// the `SmCrvOnSurf` part ([`CrvOnSurf::read_from_db`]).
    ///
    /// # Errors
    ///
    /// Those of the member reads; those of the base read (upstream ignores that status
    /// and returns an object whose UV curve and surface are null, which every query
    /// then dereferences; see the module deviations).
    pub fn read_from_db(
        k: &Kernel,
        db: &mut DatabaseReader,
        dim: u64,
        version: u64,
    ) -> Result<Self> {
        let (rec, base) = read_parts(k, db, dim, version)?;
        Ok(Self {
            base: base?,
            dir_type: rec.dir_type,
            start_tangent: rec.start_tangent,
            end_tangent: rec.end_tangent,
        })
    }
}

impl CurveEvaluate for TangentFieldCurve {
    fn dim(&self) -> u64 {
        self.base.dim()
    }

    fn natural_interval(&self) -> Extent1d {
        Self::natural_interval(self)
    }

    fn evaluate(
        &self,
        k: &Kernel,
        t: f64,
        num_derivs: usize,
        _from_left: bool,
        _non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        Self::evaluate(self, k, t, num_derivs)
    }

    fn evaluate_point(&self, k: &Kernel, t: f64) -> Result<Point3> {
        Self::evaluate_point(self, k, t)
    }

    fn as_crv_on_surf(&self) -> Option<&CrvOnSurf> {
        Some(&self.base)
    }
}

/// The members of a tangent field read by [`read_from_db`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TangentFieldRecord {
    /// The raw `SmTangentDirType` value (upstream casts whatever was stored).
    pub dir_type: u64,
    /// `m_vStartTangentField`.
    pub start_tangent: Vec3,
    /// `m_vEndTangentField`.
    pub end_tangent: Vec3,
}

/// `SmTangentField::ReadFromDB`: reads the tangent-field members (type, start and
/// end tangents) from `db`, then the `SmCrvOnSurf` part (image dimension `dim`,
/// database version `version`). ASCII values that fail to parse keep their previous
/// value (`0`, [`Vec3::UNDEF`]; upstream leaves them uninitialised).
///
/// # Errors
///
/// Those of the binary reads; [`SolidError::Unported`] for the `SmCrvOnSurf` part.
pub fn read_from_db(
    k: &Kernel,
    db: &mut DatabaseReader,
    dim: u64,
    version: u64,
) -> Result<TangentFieldRecord> {
    let (rec, base) = read_parts(k, db, dim, version)?;
    ignore_status(base)?;

    // all done
    Ok(rec)
}

/// The tangent-field members, then the result of the `SmCrvOnSurf` read.
fn read_parts(
    k: &Kernel,
    db: &mut DatabaseReader,
    dim: u64,
    version: u64,
) -> Result<(TangentFieldRecord, Result<CrvOnSurf>)> {
    let mut rec = TangentFieldRecord {
        dir_type: 0,
        start_tangent: Vec3::UNDEF,
        end_tangent: Vec3::UNDEF,
    };
    if db.file_type() == FileType::Ascii {
        if let Some(s) = db.in_stream_mut() {
            if let Some(v) = s.extract_ulong() {
                rec.dir_type = v;
            }
        }
        db.go_to_next_line();
        for v in [&mut rec.start_tangent, &mut rec.end_tangent] {
            if let Some(s) = db.in_stream_mut() {
                for coord in [&mut v.x, &mut v.y, &mut v.z] {
                    if let Some(x) = s.extract_double() {
                        *coord = x;
                    }
                }
            }
            db.go_to_next_line();
        }
    } else {
        rec.dir_type = db.read_ulong()?;

        rec.start_tangent.x = db.read_double()?;
        rec.start_tangent.y = db.read_double()?;
        rec.start_tangent.z = db.read_double()?;

        rec.end_tangent.x = db.read_double()?;
        rec.end_tangent.y = db.read_double()?;
        rec.end_tangent.z = db.read_double()?;
    }

    // read the base type (status ignored)
    Ok((rec, read_crv_on_surf(k, db, dim, version)))
}

// ---- unported callees -------------------------------------------------------------

/// `SmCrvOnSurf::ReadFromDB` into the tangent field.
fn read_crv_on_surf(
    k: &Kernel,
    db: &mut DatabaseReader,
    _dim: u64,
    version: u64,
) -> Result<CrvOnSurf> {
    CrvOnSurf::read_from_db(k, db, version)
}

/// `*m_pSurface == *rOther.m_pSurface`.
fn surface_eq<S: SurfaceEvaluate>(k: &Kernel, a: &S, b: &S) -> Result<bool> {
    a.surface_approx_eq(k, b)
}

/// `m_pSurface->EvaluateNormal(uv, TRUE, TRUE, normal)`.
fn surface_evaluate_normal<S: SurfaceEvaluate>(
    k: &Kernel,
    surface: &S,
    uv: Point2,
) -> Result<Vec3> {
    surface.evaluate_normal(
        k,
        uv,
        crate::nurbs::knots::Side::Left,
        crate::nurbs::knots::Side::Left,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::hermite_curve::HermiteCurve;

    fn hermite(p1: [f64; 3], d1: [f64; 3], p2: [f64; 3], d2: [f64; 3], dim: u64) -> Curve {
        Curve::from(HermiteCurve::new(
            Vec3::from_array(p1),
            Vec3::from_array(d1),
            Vec3::from_array(p2),
            Vec3::from_array(d2),
            dim,
        ))
    }

    fn domain() -> Extent2d {
        let mut d = Extent2d::new();
        d.set_min_max(Point2::new(0.0, 0.0), Point2::new(1.0, 1.0))
            .unwrap();
        d
    }

    #[test]
    fn type_queries() {
        assert!(is_kind_of(TYPE));
        assert!(is_kind_of(CRV_ON_SURF_TYPE));
        assert!(is_kind_of(curve_base::TYPE));
        assert!(!is_kind_of(curve_base::BSPLINE_CURVE_TYPE));
        assert_eq!(TYPE_STRING, "SmTangentField_TYPE");
        assert_eq!(CLASS_STRING, "SmTangentField");
        assert_eq!(TangentDirType::Interpolate.code(), 1);
    }

    fn unit_square() -> crate::nurbs::b_spline_surface::BSplineSurface {
        crate::nurbs::b_spline_surface::BSplineSurface::create_bilinear_surface(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
        )
        .unwrap()
    }

    #[test]
    fn ends_return_stored_tangents() {
        let k = &Kernel::default();
        let uv = hermite(
            [0.0; 3],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            2,
        );
        let srf = unit_square();
        let s = Vec3::new(0.0, 1.0, 0.0);
        let e = Vec3::new(0.0, 0.0, 1.0);
        let tf = TangentField::new(
            k,
            &uv,
            &srf,
            TangentDirType::Interpolate,
            s,
            e,
            Some(domain()),
        )
        .unwrap();
        assert_eq!(tf.evaluate_point(k, 0.0).unwrap(), s);
        assert_eq!(tf.evaluate_point(k, 1.0).unwrap(), e);
        let d = tf.evaluate(k, 1.0, 1).unwrap();
        assert_eq!(d[1], Vec3::new(0.0, 0.0, 0.0));
        assert!(tf.evaluate(k, 0.0, 2).is_err());
        // inside the interval: the surface normal (surface::surface::p7) is available
        assert!(tf.evaluate_point(k, 0.5).is_ok());
        // without a domain: the surface's natural UV domain
        let n = TangentField::new(k, &uv, &srf, TangentDirType::Interpolate, s, e, None).unwrap();
        assert_eq!(n.evaluate_point(k, 0.0).unwrap(), s);
    }

    #[test]
    fn equality_same_surface() {
        let k = &Kernel::default();
        let uv = hermite(
            [0.0; 3],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            2,
        );
        let srf = unit_square();
        let s = Vec3::new(0.0, 1.0, 0.0);
        let e = Vec3::new(0.0, 0.0, 1.0);
        let a = TangentField::new(
            k,
            &uv,
            &srf,
            TangentDirType::Interpolate,
            s,
            e,
            Some(domain()),
        )
        .unwrap();
        let b = TangentField::new(k, &uv, &srf, TangentDirType::Constant, s, e, Some(domain()))
            .unwrap();
        assert!(a.approx_eq(k, &a).unwrap());
        assert!(!a.approx_eq(k, &b).unwrap());
        let srf2 = unit_square();
        let c = TangentField::new(
            k,
            &uv,
            &srf2,
            TangentDirType::Interpolate,
            s,
            e,
            Some(domain()),
        )
        .unwrap();
        // equal surfaces through SmSurface::operator==
        assert!(a.approx_eq(k, &c).unwrap());
    }

    #[test]
    fn create_checks_ends() {
        let k = &Kernel::default();
        let base = hermite(
            [0.0; 3],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            3,
        );
        let uv = hermite(
            [0.0; 3],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            2,
        );
        let sc = hermite(
            [0.0; 3],
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            3,
        );
        let ec = hermite(
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            3,
        );
        let srf = unit_square();
        let start = CurveEnd {
            curve: &sc,
            param: 0.0,
            opposite: false,
        };
        let end = CurveEnd {
            curve: &ec,
            param: 0.0,
            opposite: true,
        };
        // matching ends build the field over the surface's natural UV domain
        assert!(TangentField::create(k, 1e-6, &base, &uv, &srf, start, end).is_ok());
        // a mismatched end is SM_ERR
        let bad = CurveEnd {
            curve: &ec,
            param: 1.0,
            opposite: false,
        };
        assert!(matches!(
            TangentField::create(k, 1e-6, &base, &uv, &srf, start, bad),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
    }

    #[test]
    fn curve_kind_round_trips_through_the_database() {
        use crate::topology::database_io::DatabaseWriter;
        use crate::topology::geometry_write::write_curve;
        let k = &Kernel::default();
        let uv = Curve::from(
            crate::nurbs::b_spline_curve::BSplineCurve::create_line_segment(
                2,
                Vec3::new(0.1, 0.2, 0.0),
                Vec3::new(0.8, 0.9, 0.0),
            )
            .unwrap(),
        );
        let tf = TangentFieldCurve::new(
            k,
            uv,
            Surface::from(unit_square()),
            TangentDirType::Interpolate,
            Vec3::new(0.0, 1.0, 0.5),
            Vec3::new(1.0, 0.0, -0.5),
            None,
        )
        .unwrap();
        let c = Curve::from(tf);
        assert_eq!(c.type_code(), TYPE);
        assert_eq!(c.type_string(), TYPE_STRING);
        for ft in [FileType::Binary, FileType::Ascii] {
            let mut w = DatabaseWriter::open_for_write(ft);
            w.write_type(c.type_code(), Some(c.dim()));
            write_curve(&c, &mut w, 30).unwrap();
            let mut db = DatabaseReader::new();
            db.open_for_read(w.into_bytes(), ft);
            let h = db.read_type(true).unwrap();
            let r =
                Curve::read_from_db(k, h.type_code.unwrap(), &mut db, h.dim.unwrap(), 30).unwrap();
            assert_eq!(r.type_code(), TYPE);
            assert!(r.approx_eq(k, &c).unwrap());
            for t in [0.0, 0.3, 0.5, 1.0] {
                assert_eq!(
                    r.evaluate_point(k, t).unwrap(),
                    c.evaluate_point(k, t).unwrap()
                );
            }
            assert!(r.evaluate(k, 0.5, 2, true, true).is_err());
            // the base Copy slices the derived object
            assert_eq!(r.copy(k).unwrap().type_code(), CRV_ON_SURF_TYPE);
        }
    }

    #[test]
    fn read_binary_members_then_base() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1_u64.to_ne_bytes());
        for x in [1.0_f64, 2.0, 3.0, 4.0, 5.0, 6.0] {
            bytes.extend_from_slice(&x.to_ne_bytes());
        }
        // the SmCrvOnSurf part runs out of data: its status is ignored
        let mut db = DatabaseReader::new();
        db.open_for_read(bytes.clone(), FileType::Binary);
        let rec = read_from_db(&Kernel::default(), &mut db, 3, 30).unwrap();
        assert_eq!(rec.dir_type, 1);
        assert_eq!(rec.end_tangent, Vec3::new(4.0, 5.0, 6.0));
        let mut db = DatabaseReader::new();
        db.open_for_read(bytes.get(..20).unwrap().to_vec(), FileType::Binary);
        assert!(matches!(
            read_from_db(&Kernel::default(), &mut db, 3, 30),
            Err(SolidError::InvalidInput { .. })
        ));
    }
}
