//! Lines (`SmLine`): an analytic line `P + s * scale * V` over a STEP (analytic)
//! domain, kept in sync with a degree-1 B-spline (its NURB representation), with
//! evaluation in both parameterisations, closest-point and line/line solvers, joins,
//! trims, transforms and database input.
//!
//! Upstream: `source/SMLib/src/SmLine.cpp`, `source/SMLib/inc/SmLine.h`.
//!
//! | Rust                                         | Upstream                                          |
//! |----------------------------------------------|---------------------------------------------------|
//! | [`Line`]                                     | `SmLine`                                          |
//! | [`Line::default`]                            | `SmLine()`                                        |
//! | [`Line::from_points`]                        | `SmLine(StartPoint, EndPoint, lDimension, cpContext)` |
//! | [`Line::from_parts`]                         | the members as they are (an importer's constructor) |
//! | [`Line::infinite`]                           | `SmLine(LinePoint, NonUnitVector, lDim, bMakeNurb, cpContext, bInsideOut)` |
//! | [`Line::bounded`], [`BoundedLineOptions`]    | `SmLine(LinePoint, UnitVector, AnalDomain, dScale, lDim, cpContext, pOptNurb, bInsideOut)` |
//! | [`Line::create_line_segment`]                | `CreateLineSegment`                               |
//! | [`Line::create_canonical`]                   | `CreateCanonical`                                 |
//! | `Clone`, [`Line::copy`]                      | copy constructor, `Copy`                          |
//! | `Drop` (implicit)                            | `~SmLine`                                         |
//! | [`Line::approx_eq`]                          | `operator==`                                      |
//! | [`Line::adjust_step_interval`]               | `AdjustSTEPInterval`                              |
//! | [`Line::approximate_length`]                 | `ApproximateLength`                               |
//! | [`Line::convert_to_2d`], [`Line::convert_to_3d`] | `ConvertTo2D`, `ConvertTo3D`                  |
//! | [`Line::convert_t_from_step_to_nurbs`]       | `ConvertTFromSTEPToNURBS`                         |
//! | [`Line::convert_t_from_nurbs_to_step`]       | `ConvertTFromNURBSToSTEP`                         |
//! | [`Line::create_simple_offset`]               | `CreateSimpleOffset`                              |
//! | [`Line::calculate_bounding_box`]             | `CalculateBoundingBox`                            |
//! | [`Line::calculate_tight_bounding_box`]       | `CalculateTightBoundingBox`                       |
//! | [`Line::drop_point_fast`]                    | `DropPointFast`                                   |
//! | [`Line::natural_interval`]                   | `GetNaturalInterval`                              |
//! | [`Line::canonical`], [`Line::canonical_unit`], [`LineCanonical`] | `GetCanonical` (both overloads) |
//! | [`Line::global_point_solve_step`]            | `GlobalPointSolveSTEP`                            |
//! | [`Line::evaluate`], [`Line::evaluate_step`], [`Line::evaluate_point`] | `Evaluate`, `EvaluateSTEP`, `EvaluatePoint` |
//! | [`Line::is_line`]                            | `IsLine`                                          |
//! | [`Line::is_bounded`], [`Line::is_degenerate`], [`Line::is_domain_boundary`] | `IsBounded`, `IsDegenerate`, `IsDomainBoundary` |
//! | [`Line::is_analytic`], [`Line::is_closed`], [`Line::is_arc`] | `IsAnalytic`, `IsClosed`, `IsArc` (inline) |
//! | [`Line::intersect_with_ellipse`]             | `IntersectWithEllipse`                            |
//! | [`Line::intersect_with_line`], [`LineLineIntersection`] | `IntersectWithLine`                    |
//! | [`Line::join_with`]                          | `JoinWith`                                        |
//! | [`Line::length`]                             | `Length`                                          |
//! | [`Line::ends`], [`Line::ends_copy`], [`Line::base_with_nurb`] | `SmBSplineCurve::GetEnds`, `GetOrCreateGwNurbPointer` (inherited) |
//! | [`Line::edit_parameterization`]              | `SmBSplineCurve::EditParameterization` (inherited) |
//! | [`Line::create_analytic_curve`]              | `SmBSplineCurve::CreateAnalyticCurve` (inherited) |
//! | [`Line::make_nurb`]                          | `MakeNurb`                                        |
//! | [`Line::refresh_analytics`]                  | `RefreshAnalytics`                                |
//! | [`Line::reverse_parameterization`]           | `ReverseParameterization`                         |
//! | [`Line::step_inversion`], [`CurveLocationType`] | `STEPInversion`, `SmCurveLocationType`         |
//! | [`Line::tessellate`], [`TessellationRequest`] | `Tessellate`                                     |
//! | [`Line::transform`]                          | `Transform`                                       |
//! | [`Line::deform_to_end_point_targets`]        | `DeformToEndPointTargets`                         |
//! | [`Line::trim`]                               | `Trim`                                            |
//! | [`Line::assert_valid`]                       | `AssertValid`                                     |
//! | [`Line::read_from_db`]                       | `ReadFromDB`                                      |
//! | [`Line::write_to_db`]                        | `WriteToDB`                                       |
//! | [`Line::is_kind_of`], [`Line::TYPE`], [`Line::type_string`], [`Line::class_string`] | `IsKindOf`, `GetType`, `GetClassType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`Line::step_interval`], [`Line::max_analytic_domain`], [`Line::degree`], [`Line::number_natural_knots`] | `GetSTEPInterval`, `GetMaxAnalyticDomain`, `GetDegree`, `GetNumberNaturalKnots` (inline) |
//! | [`Line::line_point`], [`Line::line_vector`], [`Line::line_scale`], [`Line::inside_out`] | `GetLinePoint`, `GetLineVector`, `GetLineScale`, `GetInsideOut` (inline) |
//! | [`Line::set_line_scale`], [`Line::set_analytic_domain`] | `SetLineScale`, `SetAnalyticDomain` (inline) |
//!
//! Deviations:
//! - **Derived class.** `SmLine` derives from `SmBSplineCurve`; here a [`Line`] holds
//!   its B-spline part ([`Line::base`]) next to the analytic data. Upstream base-class
//!   methods that reach an `SmLine` override through a virtual call
//!   (`GetOrCreateGwNurbPointer` -> `MakeNurb`, `ReadFromDB` -> `ConvertTo2D`) are
//!   re-dispatched here: a missing NURB is made with [`Line::make_nurb`] before the
//!   base method runs. Queries that upstream answers by lazily making the NURB
//!   through a `const_cast` take `&mut self` and make it ([`Line::ends`],
//!   [`Line::create_analytic_curve`]); read-only callers ([`Line::ends_copy`],
//!   [`Line::base_with_nurb`], the inherited queries dispatched by
//!   [`crate::curve::curve::Curve`]) make it on a copy, so the line keeps none (the
//!   only observable difference is the branch [`Line::length`] takes afterwards).
//!   The inherited `SmBSplineCurve` methods whose bodies call further `SmLine`
//!   overrides (`CalculateContinuities`, `HasRepeatedEndControlPoints`) run on the
//!   B-spline part when dispatched through [`crate::curve::curve::Curve`].
//! - **`SmCurve::EvaluateSTEPPoint`** (unit `curve.curve_p5`) is `EvaluateSTEP(t, 0,
//!   ...)` through the virtual table; for a line that is [`Line::evaluate_step`], which
//!   the port calls directly (co-ported, no stub).
//! - **Context.** Lines are always built with a context, so the constructors that make
//!   a NURB (infinite and bounded lines) always do, and `MakeNurb` never fails for a
//!   missing context. The two-point constructor makes no NURB, as upstream.
//! - **Owner.** The owner pointer lives in [`crate::curve::curve::Curve`].
//!   `MakeNurb` asks it for the generating-curve interval of an `SmSurfOfExtrusion` /
//!   `SmSurfOfRevolution` owner (units `surface.surf_of_extrusion_p1`,
//!   `surface.surf_of_revolution_p1`, [`SolidError::Unported`]); the `Line` methods
//!   act on a line without owner, and [`crate::curve::curve::Curve`] passes its owner
//!   for the edits it dispatches. The `Notify(SM_NO_PRE_EDIT / SM_NO_POST_EDIT)` calls
//!   only drop caches and inform owners and are omitted (as for B-splines).
//! - **Arguments.** Out-params become return values; `SmContext` arguments and the
//!   arguments upstream marks `NotUsed` (`ApproximateLength`'s sample count,
//!   `Evaluate`'s `bFromLeft` and `bNonZeroTangents`, `EvaluateSTEP`'s zone tolerance,
//!   `Length`'s accuracy, `Tessellate`'s tolerances, the unused solver arguments) are
//!   dropped. Solutions name `this` (and the other line) through an optional
//!   caller-supplied object id. Optional outputs (`pOptLoc`, the boxes, the
//!   tessellation arrays) are requested with flags.
//! - **Tessellation size.** `Tessellate` reserves its `min_segments + 1` entries per
//!   requested array first and returns [`SolidError::InvalidInput`] when that
//!   allocation fails (upstream's `SmTArray::Add` fails silently, leaving short arrays
//!   after a very long loop).
//! - **Statuses.** Calls whose status upstream ignores keep upstream's values, except
//!   that [`SolidError::Unported`] is always propagated. `DeformToEndPointTargets`
//!   hands the new curve back only when the B-spline deformation succeeds (upstream
//!   also returns it with the failing status).
//! - **`IntersectWithEllipse`** passes the call to `SmEllipse::IntersectWithLine`
//!   ([`Ellipse::intersect_with_line`], unit `curve.ellipse_p2`).
//! - **`JoinWith`**'s other curve (an `SmBSplineCurve*`) is a
//!   [`crate::curve::curve::Curve`] of B-spline or line kind; its `IsLine` is
//!   [`Line::is_line`] or [`BSplineCurve::is_line`] (which can reach the unported
//!   `SmCurve::IsLine`, `curve.curve_p5`).
//! - **`AssertValid`** runs the base `SmBSplineCurve::AssertValid` unless
//!   `level_given`; the line's four end point checks then follow.
//! - **`ReadFromDB`** always builds a new line (the "fill this object" input is
//!   dropped); the status of the base class read is ignored, as upstream.

use crate::curve::curve::Curve;
use crate::curve::curve::CurveKind;
use crate::curve::ellipse::Ellipse;
use crate::error::{ignore_status, Result, SolidError};
use crate::geom::assert_array::labels::{self, SURF_OF_EXTRUSION_TYPE, SURF_OF_REVOLUTION_TYPE};
use crate::geom::assert_array::{AssertArray, ObjectKey, ReportSpec};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::{line_closest_point, line_point_distance};
use crate::geom::geom_utility::p2::segment_segment_intersect;
use crate::geom::global_solver::SolverOperation;
use crate::geom::object::ObjectRef;
use crate::geom::polar_box::PolarBox;
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::solution_array::{Solution, SolutionType};
use crate::geom::tol::{are_same, are_same_to_tol, is_zero, tol3d};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, INFINITE_PARAMETER, UNDEF_DOUBLE};
use crate::nurbs::b_spline_curve::p3::DerivedOverrides;
use crate::nurbs::b_spline_curve::{
    BSplineCurve, BSplineCurveForm, BoundingBoxRequest, BoundingBoxes, Ends, Tessellation,
};
use crate::nurbs::b_spline_curve_n_lib::{InValueType, TrimMode};
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::math::NOW;
use crate::nurbs::nurbs_crv::allocate_nurb_curve;
use crate::surface::surface::Surface;
use crate::topology::database_io::{DatabaseReader, DatabaseWriter, FileType, InputStream};

const OP_SEGMENT: &str = "curve::line::Line::create_line_segment";
const OP_OFFSET: &str = "curve::line::Line::create_simple_offset";
const OP_DROP: &str = "curve::line::Line::drop_point_fast";
const OP_JOIN: &str = "curve::line::Line::join_with";
const OP_DEFORM: &str = "curve::line::Line::deform_to_end_point_targets";
const OP_TESS: &str = "curve::line::Line::tessellate";

/// `SmCurveLocationType`: where a point lies relative to a curve; discriminants are
/// the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CurveLocationType {
    /// `SM_CL_SEAM`: on the seam of a closed curve.
    Seam = 0,
    /// `SM_CL_INTERIOR`: on the curve.
    Interior = 1,
    /// `SM_CL_EXTERIOR`: not on the curve.
    Exterior = 2,
    /// `SM_CL_UNKNOWN`.
    Unknown = 3,
}

/// The optional inputs of [`Line::bounded`] (the defaulted arguments of the bounded
/// constructor).
#[derive(Debug, Clone, Copy)]
pub struct BoundedLineOptions<'a> {
    /// Curve speed `scale` of `P + s * scale * V` (default `1.0`).
    pub scale: f64,
    /// Image-space dimension (default `3`).
    pub dim: u64,
    /// A B-spline whose NURB is copied instead of making one (`pOptNurb`); its start
    /// is the STEP end when `inside_out`, else the STEP start.
    pub nurb: Option<&'a BSplineCurve>,
    /// `TRUE`: the NURB tangent is `-V` (cone generating lines).
    pub inside_out: bool,
}

impl Default for BoundedLineOptions<'_> {
    fn default() -> Self {
        Self {
            scale: 1.0,
            dim: 3,
            nurb: None,
            inside_out: false,
        }
    }
}

/// The canonical data of a line ([`Line::canonical`], [`Line::canonical_unit`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineCanonical {
    /// `P` of `P + u * scale * V`.
    pub point: Point3,
    /// `V` (unit) for [`Line::canonical_unit`], `scale * V` for [`Line::canonical`].
    pub vector: Vec3,
    /// `scale` ([`Line::canonical_unit`]; `1` for [`Line::canonical`]).
    pub scale: f64,
    /// The analytic (STEP) domain of `u`.
    pub domain: Extent1d,
    /// Whether the NURB and analytic parameterisations run in opposite directions.
    pub inside_out: bool,
}

/// The arrays [`Line::tessellate`] fills (its non-`NULL` output pointers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TessellationRequest {
    /// The NURB parameters (`pOptParameters`).
    pub parameters: bool,
    /// The points (`pOptPoints`).
    pub points: bool,
    /// The tangents (`pOptTangents`).
    pub tangents: bool,
}

/// Result of [`Line::intersect_with_line`] and [`Line::intersect_with_ellipse`].
#[derive(Debug, Clone, PartialEq)]
pub struct LineLineIntersection<O> {
    /// Intersections found (`start.parameters`: `[this param, other param]`).
    pub solutions: Vec<Solution<O>>,
    /// Whether the general curve/curve intersector must be called.
    pub needs_more_intersections: bool,
}

/// A line (`SmLine`): `Line(s) = point + s * vector * scale` for STEP parameters `s`
/// in the analytic domain, with `vector` a unit vector, plus its B-spline
/// representation.
///
/// Bounded lines map their NURB domain linearly onto the analytic domain (reversed
/// when `inside_out`); infinite lines (domain `[-SM_INFINITE_PARAMETER,
/// SM_INFINITE_PARAMETER]`) use the STEP parameter (negated when `inside_out`).
#[derive(Debug, Clone)]
pub struct Line {
    /// The `SmBSplineCurve` part (dimension, NURB, form flags).
    pub(crate) base: BSplineCurve,
    /// `m_vLinePoint`.
    point: Point3,
    /// `m_vLineVector` (unit).
    vector: Vec3,
    /// `m_vAnalDomain`.
    anal_domain: Extent1d,
    /// `m_dScale`.
    scale: f64,
    /// `m_bInsideOut`.
    inside_out: bool,
}

impl Line {
    /// Exact evaluation copy without the B-spline copy constructor's knot fix-up.
    pub(crate) fn clone_exact_bounded(&self) -> Self {
        Self {
            base: self.base.clone_exact(),
            point: self.point,
            vector: self.vector,
            anal_domain: self.anal_domain,
            scale: self.scale,
            inside_out: self.inside_out,
        }
    }
}

impl Default for Line {
    /// The empty line read into by `ReadFromDB` (`SmLine()`): 3D, no NURB, undefined
    /// point, vector and scale, an "inside out" domain.
    fn default() -> Self {
        Self {
            base: BSplineCurve::new(3),
            point: Vec3::UNDEF,
            vector: Vec3::UNDEF,
            anal_domain: Extent1d::new(),
            scale: UNDEF_DOUBLE,
            inside_out: false,
        }
    }
}

/// `SmCurve::IsLine` answer of the other curve of [`Line::join_with`].
fn other_is_line(other: &Curve, num_samples: u64, tol: f64) -> Result<bool> {
    match other.kind() {
        CurveKind::Line(_) => Ok(true),
        CurveKind::BSpline(b) => Ok(b.is_line(num_samples, tol)?.is_some()),
        CurveKind::Hyperbola(h) => Ok(h.base().is_line(num_samples, tol)?.is_some()),
        CurveKind::Ellipse(e) => Ok(e.base().is_line(num_samples, tol)?.is_some()),
        CurveKind::Parabola(p) => Ok(p.base().is_line(num_samples, tol)?.is_some()),
        // curves that are not an SmBSplineCurve
        _ => Err(SolidError::invalid_input(
            OP_JOIN,
            "the other curve is not an SmBSplineCurve",
        )),
    }
}

/// `GetEnds` of the other curve of [`Line::join_with`].
fn other_ends(other: &Curve) -> Result<Ends> {
    match other.kind() {
        CurveKind::Line(l) => l.ends_copy(),
        CurveKind::BSpline(b) => b.ends(),
        CurveKind::Hyperbola(h) => h.base().ends(),
        CurveKind::Ellipse(e) => e.base().ends(),
        CurveKind::Parabola(p) => p.base().ends(),
        // curves that are not an SmBSplineCurve
        _ => Err(SolidError::invalid_input(
            OP_JOIN,
            "the other curve is not an SmBSplineCurve",
        )),
    }
}

impl Line {
    /// `SmLine_TYPE`.
    pub const TYPE: i64 = crate::curve::curve::p2::LINE_TYPE;

    // ---- construction -----------------------------------------------------------------

    /// The bounded segment from `start` to `end` over the domain `[0, 1]`
    /// (`SmLine(StartPoint, EndPoint, lDim, cpContext)`); no NURB is made. A segment
    /// shorter than `SM_EFF_ZERO` becomes the degenerate line at `start` along X with
    /// domain `[0, 0]`.
    #[must_use]
    pub fn from_points(start: Point3, end: Point3, dim: u64) -> Self {
        // init line parameters
        let mut vec = end - start;
        let mut scale = vec.length();
        let mut ivl = Extent1d::from_min_max(0.0, 1.0);

        // watch out for degenerate lines
        if scale < EFF_ZERO {
            vec = Vec3::new(1.0, 0.0, 0.0);
            scale = 1.0;
            let _ = ivl.set_min_max(0.0, 0.0);
        } else {
            vec /= scale; // Unitize.
        }

        Self {
            base: BSplineCurve::new(dim),
            point: start,
            vector: vec,
            anal_domain: ivl,
            scale,
            inside_out: false,
        }
    }

    /// A line from its members exactly as given (`m_vLinePoint`, the unit
    /// `m_vLineVector`, `m_vAnalDomain`, `m_dScale`, `m_bInsideOut`) and its B-spline
    /// part, nothing normalised or rebuilt: for importing a line another kernel made
    /// (the difftest import of an upstream B-rep's curves). No upstream counterpart.
    #[must_use]
    pub fn from_parts(
        base: BSplineCurve,
        point: Point3,
        vector: Vec3,
        anal_domain: Extent1d,
        scale: f64,
        inside_out: bool,
    ) -> Self {
        Self {
            base,
            point,
            vector,
            anal_domain,
            scale,
            inside_out,
        }
    }

    /// The infinite line `point + s * vector` (domain `[-SM_INFINITE_PARAMETER,
    /// SM_INFINITE_PARAMETER]`, scale `|vector|`) with its NURB (`SmLine(LinePoint,
    /// NonUnitVector, lDim, bMakeNurb, cpContext, bInsideOut)`; `bMakeNurb` is not
    /// used upstream). A zero `vector` gives non-finite data, as upstream.
    #[must_use]
    pub fn infinite(point: Point3, vector: Vec3, dim: u64, inside_out: bool) -> Self {
        let length = vector.length();
        let mut line = Self {
            base: BSplineCurve::new(dim),
            point,
            vector: vector / length,
            anal_domain: Extent1d::from_min_max(-INFINITE_PARAMETER, INFINITE_PARAMETER),
            scale: length,
            inside_out,
        };

        // When object has a context - make the m_pNurb (a new line has no owner)
        let _ = line.make_nurb_owned(None);
        line.base.form = BSplineCurveForm::PolylineForm;
        line
    }

    /// The bounded line `point + s * scale * unit(vector)` for `s` in `domain`
    /// (`SmLine(LinePoint, UnitVector, AnalDomain, dScale, lDim, cpContext, pOptNurb,
    /// bInsideOut)`): `vector` is unitised; a zero `vector` gives the X direction and
    /// the degenerate domain `[domain.min, domain.min]`. The NURB is copied from
    /// `opts.nurb` or made.
    #[must_use]
    pub fn bounded(
        point: Point3,
        vector: Vec3,
        domain: &Extent1d,
        opts: &BoundedLineOptions<'_>,
    ) -> Self {
        // force m_vLineVector to be a unit vector
        let length = vector.length();
        let mut line = Self {
            base: BSplineCurve::new(opts.dim),
            point,
            vector: Vec3::UNDEF,
            anal_domain: *domain,
            scale: opts.scale,
            inside_out: opts.inside_out,
        };
        if length > EFF_ZERO {
            line.vector = vector / length;
        } else {
            // give line an arbitrary direction and shrink AnalDomain down to a degenerate point
            line.vector = Vec3::new(1.0, 0.0, 0.0);
            let min = line.anal_domain.min();
            let _ = line.anal_domain.set_min_max(min, min);
        }

        // make the associated NURB curve
        if let Some(nurb) = opts.nurb {
            // copy and save the gw_CURVE (a plain B-spline cannot make a missing NURB)
            let _ = line.base.set_from_gw_nurb(nurb.gw_nurb());
        } else {
            let _ = line.make_nurb_owned(None);
        }

        // set form factor
        line.base.form = BSplineCurveForm::PolylineForm;
        line
    }

    /// The bounded segment from `start` to `end` over `[0, 1]`, reparameterised to
    /// `interval` when given (`CreateLineSegment`; the reparameterisation's status is
    /// ignored, as upstream).
    ///
    /// # Errors
    ///
    /// `InvalidInput` when the points coincide (`Unitize` fails).
    pub fn create_line_segment(
        dim: u64,
        start: Point3,
        end: Point3,
        interval: Option<&Extent1d>,
    ) -> Result<Self> {
        // init line parameters
        let vec = end - start;
        let scale = vec.length();
        let vec = vec.unitize().map_err(|_| SolidError::InvalidInput {
            operation: OP_SEGMENT,
            reason: "start and end points coincide",
        })?;
        let ivl = Extent1d::from_min_max(0.0, 1.0);

        // build line
        let mut line = Self::bounded(
            start,
            vec,
            &ivl,
            &BoundedLineOptions {
                scale,
                dim,
                ..BoundedLineOptions::default()
            },
        );

        // when asked - adjust the parameter range
        if let Some(i) = interval {
            let _ = line.base.edit_parameterization(i);
        }
        Ok(line)
    }

    /// The infinite line through `point` along `vector` (scale `|vector|`) with its
    /// NURB (`CreateCanonical`).
    #[must_use]
    pub fn create_canonical(point: Point3, vector: Vec3) -> Self {
        Self::infinite(point, vector, 3, false)
    }

    /// An exact copy (`Copy`, the copy constructor): the NURB is copied with its knots
    /// fixed up.
    #[must_use]
    pub fn copy(&self) -> Self {
        self.clone()
    }

    /// `operator==`: same B-spline part (dimension and NURB within `SM_EFF_ZERO`), and
    /// point, vector, domain, scale (within the scaled zero) and `inside_out` equal.
    #[must_use]
    pub fn approx_eq(&self, other: &Self) -> bool {
        // low work
        if core::ptr::eq(self, other) {
            return true;
        }

        // first check the base
        if !self.base.approx_eq(&other.base) {
            return false;
        }

        // check equivalence of these objects
        self.point.approx_eq(other.point)
            && self.vector.approx_eq(other.vector)
            && self.anal_domain.are_same(&other.anal_domain)
            && is_zero(self.scale - other.scale)
            && self.inside_out == other.inside_out
    }

    // ---- simple access ----------------------------------------------------------------

    /// The `SmBSplineCurve` part (dimension, NURB, form).
    #[must_use]
    pub const fn base(&self) -> &BSplineCurve {
        &self.base
    }

    /// The `SmBSplineCurve` base object, mutable (for the base-class edits).
    pub(crate) fn base_mut(&mut self) -> &mut BSplineCurve {
        &mut self.base
    }

    /// Image-space dimension (`SmCurve::GetDim`).
    #[must_use]
    pub const fn dim(&self) -> u64 {
        self.base.dim
    }

    /// `m_vLinePoint` (`GetLinePoint`).
    #[must_use]
    pub const fn line_point(&self) -> Point3 {
        self.point
    }

    /// `m_vLineVector`, a unit vector (`GetLineVector`).
    #[must_use]
    pub const fn line_vector(&self) -> Vec3 {
        self.vector
    }

    /// `m_dScale` (`GetLineScale`).
    #[must_use]
    pub const fn line_scale(&self) -> f64 {
        self.scale
    }

    /// `m_bInsideOut` (`GetInsideOut`).
    #[must_use]
    pub const fn inside_out(&self) -> bool {
        self.inside_out
    }

    /// The analytic domain (`GetSTEPInterval`).
    #[must_use]
    pub const fn step_interval(&self) -> Extent1d {
        self.anal_domain
    }

    /// `[-1000, 1000]` (`GetMaxAnalyticDomain`).
    #[must_use]
    pub fn max_analytic_domain(&self) -> Extent1d {
        Extent1d::from_min_max(-1000.0, 1000.0)
    }

    /// `1` (`GetDegree`).
    #[must_use]
    pub const fn degree(&self) -> usize {
        1
    }

    /// `4` (`GetNumberNaturalKnots`).
    #[must_use]
    pub const fn number_natural_knots(&self) -> usize {
        4
    }

    /// Sets `m_dScale` (`SetLineScale`; the NURB is not rebuilt).
    pub fn set_line_scale(&mut self, scale: f64) {
        self.scale = scale;
    }

    /// Sets the analytic domain (`SetAnalyticDomain`; the NURB is not rebuilt).
    pub fn set_analytic_domain(&mut self, domain: Extent1d) {
        self.anal_domain = domain;
    }

    /// `TRUE` (`IsAnalytic`).
    #[must_use]
    pub const fn is_analytic(&self) -> bool {
        true
    }

    /// `FALSE` (`IsClosed`).
    #[must_use]
    pub const fn is_closed(&self, _interval: &Extent1d, _tol: f64) -> bool {
        false
    }

    /// `FALSE` (`IsArc`).
    #[must_use]
    pub const fn is_arc(&self, _num_samples: u64, _tol: f64) -> bool {
        false
    }

    /// `SmLine_TYPE` (`GetType`, `GetClassType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmLine_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmLine_TYPE"
    }

    /// `"SmLine"` (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        "SmLine"
    }

    /// True for `SmLine_TYPE` and the base types of `SmBSplineCurve` (`IsKindOf`).
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == Self::TYPE || t == BSplineCurve::TYPE || crate::curve::curve::p2::is_kind_of(t)
    }

    /// Whether neither end of the analytic domain is infinite (`IsBounded`).
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        self.anal_domain.is_bounded()
    }

    /// The NURB domain, or the analytic domain without NURB (`GetNaturalInterval`).
    #[must_use]
    pub fn natural_interval(&self) -> Extent1d {
        if self.base.nurb.is_some() {
            self.base.natural_interval()
        } else {
            self.anal_domain
        }
    }

    /// `P`, `scale * V`, the domain and `inside_out` (`GetCanonical(Point,
    /// NonUnitVector, pOptAnalDomain, pOptInsideOut)`); [`LineCanonical::scale`] is 1.
    #[must_use]
    pub fn canonical(&self) -> LineCanonical {
        let mut domain = Extent1d::new();
        let _ = domain.set_min_max(self.anal_domain.min(), self.anal_domain.max());
        LineCanonical {
            point: self.point,
            vector: self.scale * self.vector,
            scale: 1.0,
            domain,
            inside_out: self.inside_out,
        }
    }

    /// `P`, unit `V`, `scale`, the domain and `inside_out` (`GetCanonical(Point,
    /// UnitVector, Scale, AnalDomain, pOptInsideOut)`).
    #[must_use]
    pub fn canonical_unit(&self) -> LineCanonical {
        let mut domain = Extent1d::new();
        let _ = domain.set_min_max(self.anal_domain.min(), self.anal_domain.max());
        LineCanonical {
            point: self.point,
            vector: self.vector,
            scale: self.scale,
            domain,
            inside_out: self.inside_out,
        }
    }

    // ---- parameter conversion ---------------------------------------------------------

    /// The NURB parameter of STEP parameter `step_param` (`ConvertTFromSTEPToNURBS`).
    #[must_use]
    pub fn convert_t_from_step_to_nurbs(&self, step_param: f64) -> f64 {
        let nurb_ivl = self.natural_interval();

        // convert linear coordinate
        if self.is_bounded() {
            if self.inside_out {
                nurb_ivl.max()
                    + ((step_param - self.anal_domain.min()) / self.anal_domain.length()
                        * -nurb_ivl.length())
            } else {
                nurb_ivl.min()
                    + ((step_param - self.anal_domain.min()) / self.anal_domain.length()
                        * nurb_ivl.length())
            }
        } else if self.inside_out {
            -step_param
        } else {
            step_param
        }
    }

    /// The STEP parameter of NURB parameter `nurbs_param` (`ConvertTFromNURBSToSTEP`);
    /// a bounded line with a zero-length NURB domain maps a parameter to itself.
    #[must_use]
    pub fn convert_t_from_nurbs_to_step(&self, nurbs_param: f64) -> f64 {
        // swap NurbPoint when needed
        let nurb_ivl = self.natural_interval();

        // convert linear coordinate
        if self.is_bounded() {
            let nurbs_length = nurb_ivl.length();
            if is_zero(nurbs_length) {
                nurbs_param
            } else {
                self.anal_domain.min()
                    + if self.inside_out {
                        (nurbs_param - nurb_ivl.max()) / -nurb_ivl.length()
                            * self.anal_domain.length()
                    } else {
                        (nurbs_param - nurb_ivl.min()) / nurb_ivl.length()
                            * self.anal_domain.length()
                    }
            }
        } else if self.inside_out {
            -nurbs_param
        } else {
            nurbs_param
        }
    }

    // ---- evaluation -------------------------------------------------------------------

    /// Position and derivatives `0..=num_derivs` at NURB parameter `t` (`Evaluate`):
    /// the first derivative is `±V * scale * (analytic length / NURB length)`, higher
    /// ones are zero.
    #[must_use]
    pub fn evaluate(&self, t: f64, num_derivs: usize) -> Vec<Vec3> {
        // convert to STEP parameter
        let step_param = self.convert_t_from_nurbs_to_step(t);

        // Set 2nd order and higher derivatives to zero
        let mut out = vec![Vec3::ZERO; num_derivs + 1];

        // position
        if let Some(p) = out.first_mut() {
            *p = self.point + step_param * self.vector * self.scale;
        }

        // 1st Derivative
        if num_derivs >= 1 {
            let nurb_ivl = self.natural_interval();
            let jacobi = self.anal_domain.length() / nurb_ivl.length();
            if let Some(d) = out.get_mut(1) {
                *d = if self.inside_out {
                    -self.vector * self.scale * jacobi
                } else {
                    self.vector * self.scale * jacobi
                };
            }
        }
        out
    }

    /// Position and derivatives `0..=num_derivs` at STEP parameter `t`
    /// (`EvaluateSTEP`): the first derivative is `V * scale`, higher ones are zero.
    #[must_use]
    pub fn evaluate_step(&self, t: f64, num_derivs: usize) -> Vec<Vec3> {
        // Set 2nd order and higher derivatives to zero
        let mut out = vec![Vec3::ZERO; num_derivs + 1];

        // position
        if let Some(p) = out.first_mut() {
            *p = self.point + t * self.vector * self.scale;
        }

        // 1st Derivative
        if let Some(d) = out.get_mut(1) {
            *d = self.vector * self.scale;
        }
        out
    }

    /// The point at STEP parameter `t` (`SmCurve::EvaluateSTEPPoint`, which calls
    /// `EvaluateSTEP` with no derivatives).
    fn evaluate_step_point(&self, t: f64) -> Point3 {
        self.point + t * self.vector * self.scale
    }

    /// The point at NURB parameter `t` (`EvaluatePoint`).
    #[must_use]
    pub fn evaluate_point(&self, t: f64) -> Point3 {
        // convert to STEP parameter
        let step_param = self.convert_t_from_nurbs_to_step(t);
        self.point + step_param * self.scale * self.vector
    }

    /// The point at the analytic domain start and the line vector over the domain
    /// (`scale * V`, times the domain length for a bounded line) (`IsLine`; always a
    /// line).
    #[must_use]
    pub fn is_line(&self) -> (Point3, Vec3) {
        // set output
        let point = self.evaluate_step_point(self.anal_domain.min());
        let vector = if self.anal_domain.is_bounded() {
            self.anal_domain.length() * self.scale * self.vector
        } else {
            self.scale * self.vector
        };
        (point, vector)
    }

    /// Whether the line over `interval` (NURB domain; `None`: the analytic domain) is
    /// shorter than `max(tol3d, SM_EFF_ZERO)` (`IsDegenerate`).
    #[must_use]
    pub fn is_degenerate(&self, tol3d: f64, interval: Option<&Extent1d>) -> bool {
        let tol = max2(tol3d, EFF_ZERO);

        // get line length in 3D
        let dist = if let Some(ivl) = interval {
            let step_min = self.convert_t_from_nurbs_to_step(ivl.min());
            let step_max = self.convert_t_from_nurbs_to_step(ivl.max());
            self.scale * (step_max - step_min).abs()
        } else {
            self.scale * self.anal_domain.length()
        };

        // return degenerate when line is shorter than tolerance
        dist < tol
    }

    /// The side of `domain` the (2D) line lies on (`IsDomainBoundary`; upstream
    /// default `tol` is `SM_EFF_ZERO`): `UMin`/`UMax` for a line along V on that
    /// boundary, `VMin`/`VMax` for one along U (checked last), else `Neither`.
    #[must_use]
    pub fn is_domain_boundary(&self, domain: &Extent2d, tol: f64) -> SurfParamType {
        let mut ret = SurfParamType::Neither;
        let (min, max) = (domain.min(), domain.max());

        if self.vector.x.abs() < tol {
            if (self.point.x - min.x).abs() < tol {
                ret = SurfParamType::UMin;
            }
            if (self.point.x - max.x).abs() < tol {
                ret = SurfParamType::UMax;
            }
        }
        if self.vector.y.abs() < tol {
            if (self.point.y - min.y).abs() < tol {
                ret = SurfParamType::VMin;
            }
            if (self.point.y - max.y).abs() < tol {
                ret = SurfParamType::VMax;
            }
        }
        ret
    }

    /// The distance between the points at the ends of `interval`, and for
    /// `want_turn_angle` a turning angle of `0` (`ApproximateLength`; exact for a
    /// line).
    #[must_use]
    pub fn approximate_length(
        &self,
        interval: &Extent1d,
        want_turn_angle: bool,
    ) -> (f64, Option<f64>) {
        // get interval endPoints
        let start = self.evaluate_point(interval.min());
        let end = self.evaluate_point(interval.max());

        // return distance between points
        (start.distance(end), want_turn_angle.then_some(0.0))
    }

    /// Length of the line over `interval` (`Length`): `0` for an interval shorter than
    /// `SM_EFF_ZERO`, else the distance between the end points with a NURB, or the
    /// interval length times the scale without.
    #[must_use]
    pub fn length(&self, interval: &Extent1d) -> f64 {
        // check for degenerate intervals
        if interval.length() < EFF_ZERO {
            return 0.0;
        }
        if self.base.nurb.is_some() {
            // get interval endPoints
            let start = self.evaluate_point(interval.min());
            let end = self.evaluate_point(interval.max());
            start.distance(end)
        } else {
            // else return curve length = IntervalLength * Scale
            interval.length() * self.scale
        }
    }

    /// End points and weights of the NURB (`SmBSplineCurve::GetEnds`); a missing NURB
    /// is made first (upstream's `GetOrCreateGwNurbPointer`).
    ///
    /// # Errors
    ///
    /// None for a line without owner.
    pub fn ends(&mut self) -> Result<Ends> {
        self.ensure_nurb(None)?;
        self.base.ends()
    }

    /// [`Line::ends`] for read-only callers: a missing NURB is made on a copy, so the
    /// line keeps none.
    ///
    /// # Errors
    ///
    /// None for a line without owner.
    pub fn ends_copy(&self) -> Result<Ends> {
        self.base_with_nurb()?.ends()
    }

    /// The B-spline part with its NURB (made on a copy when missing), for the
    /// inherited `SmBSplineCurve` queries.
    ///
    /// # Errors
    ///
    /// None in practice (a line without owner always makes its NURB).
    pub fn base_with_nurb(&self) -> Result<std::borrow::Cow<'_, BSplineCurve>> {
        if self.base.nurb.is_some() {
            return Ok(std::borrow::Cow::Borrowed(&self.base));
        }
        let mut tmp = self.clone();
        ignore_status(tmp.make_nurb_owned(None))?;
        Ok(std::borrow::Cow::Owned(tmp.base))
    }

    /// Boxes of the segment between the points at the ends of `interval`
    /// (`CalculateBoundingBox`): the axis aligned box of the two points, a pseudo box
    /// aligned with the segment (default basis for a zero-length segment), a polar box
    /// of the segment vector. Unrequested boxes are `None`.
    #[must_use]
    pub fn calculate_bounding_box(
        &self,
        interval: &Extent1d,
        req: BoundingBoxRequest,
    ) -> BoundingBoxes {
        let mut out = BoundingBoxes::default();

        // get interval endPoints
        let start = self.evaluate_point(interval.min());
        let end = self.evaluate_point(interval.max());

        // Compute the basis vectors of the PseudoBox using the control polygon
        if req.pseudo {
            let v1 = end - start;
            let mut pseudo = PseudoBox::new();

            // align pseudo box with lines tangent vector; default basis for zero length
            if !is_zero(v1.length_squared()) {
                let [b1, b2, b3] = v1.make_unit_ortho_vectors(None).unwrap_or([Vec3::UNDEF; 3]);
                let _ = pseudo.set_basis(b1, b2, b3);
            }
            out.pseudo = Some(pseudo);
        }

        if req.normal {
            let mut normal = Extent3d::from_point(start);
            normal.add_point(end);
            out.normal = Some(normal);
        }
        if let Some(pseudo) = out.pseudo.as_mut() {
            pseudo.add_point(start);
            pseudo.add_point(end);
        }
        if req.polar {
            let mut polar = PolarBox::new();
            polar.reset();
            polar.add_vector(end - start, None);
            out.polar = Some(polar);
        }
        out
    }

    /// The axis aligned box of the segment over `interval`
    /// (`CalculateTightBoundingBox`).
    #[must_use]
    pub fn calculate_tight_bounding_box(&self, interval: &Extent1d) -> Option<Extent3d> {
        self.calculate_bounding_box(
            interval,
            BoundingBoxRequest {
                normal: true,
                ..BoundingBoxRequest::default()
            },
        )
        .normal
    }

    /// Tessellation of the line over `interval` into `min_segments` equal parameter
    /// steps (`Tessellate`): the parameters, points and (first derivative) tangents
    /// requested. The points are only evaluated with their tangents when tangents
    /// are requested.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the `min_segments + 1` entries of a requested
    /// array cannot be allocated (upstream's `SmTArray::Add` then fails silently and
    /// the loop runs on).
    pub fn tessellate(
        &self,
        interval: &Extent1d,
        min_segments: u64,
        req: TessellationRequest,
    ) -> Result<Tessellation> {
        let mut out = Tessellation::default();
        let n = min_segments as f64;

        // The arrays hold `min_segments + 1` entries: reserve them up front so a huge
        // caller count fails cleanly instead of aborting on allocation.
        let too_big = || SolidError::invalid_input(OP_TESS, "min_segments too large to allocate");
        let count = min_segments
            .checked_add(1)
            .and_then(|c| usize::try_from(c).ok())
            .ok_or_else(too_big)?;
        if req.parameters {
            out.parameters
                .try_reserve_exact(count)
                .map_err(|_| too_big())?;
        }
        if req.points {
            out.points.try_reserve_exact(count).map_err(|_| too_big())?;
        }
        if req.tangents {
            out.tangents
                .try_reserve_exact(count)
                .map_err(|_| too_big())?;
        }

        // when asked - build evenly spaced param point array
        if req.parameters {
            // start with Nurb Min Param
            out.parameters.push(interval.min());
            for i in 1..min_segments {
                out.parameters.push(interval.evaluate(i as f64 / n));
            }
            out.parameters.push(interval.max());
        }

        let params = || {
            core::iter::once(interval.min())
                .chain((1..min_segments).map(|i| interval.evaluate((i as f64 * 1.0) / n)))
                .chain(core::iter::once(interval.max()))
        };

        if req.points && !req.tangents {
            // just build array of point samples
            for t in params() {
                out.points
                    .push(self.evaluate(t, 0).first().copied().unwrap_or(Vec3::UNDEF));
            }
        } else if req.points || req.tangents {
            // build array of point and tangent samples
            for t in params() {
                let pd = self.evaluate(t, 1);
                if req.points {
                    out.points.push(pd.first().copied().unwrap_or(Vec3::UNDEF));
                }
                if req.tangents {
                    out.tangents.push(pd.get(1).copied().unwrap_or(Vec3::UNDEF));
                }
            }
        }
        Ok(out)
    }

    // ---- solvers ----------------------------------------------------------------------

    /// The closest point of the line over `interval` (NURB domain) to `test_point`
    /// (`DropPointFast`): the perpendicular foot, clamped to the interval. For
    /// [`SolverOperation::Minimize`] a solution farther than `target_distance + tol`
    /// (when given) is dropped, for [`SolverOperation::Intersect`] one farther than
    /// `tol`. The solution names `this`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for other operations (callers then run the general point solver).
    pub fn drop_point_fast<O: Copy>(
        &self,
        interval: &Extent1d,
        operation: SolverOperation,
        test_point: Point3,
        tol: f64,
        target_distance: Option<f64>,
        this: Option<O>,
    ) -> Result<Vec<Solution<O>>> {
        // only good for MINIMIZE and INTERSECT operations
        if operation != SolverOperation::Minimize && operation != SolverOperation::Intersect {
            return Err(SolidError::sm_err(OP_DROP));
        }

        // init output
        let mut sol = Solution::new();
        sol.solution_type = SolutionType::SingleValue;
        sol.num_objects = 1;
        sol.objects[0] = this;
        sol.num_variables = 1;

        // get distance from TestPoint to infinite line
        let (mut dist, mut anal_param) = line_point_distance(self.point, self.vector, test_point);

        // convert AnalParam to NurbParam
        anal_param /= self.scale;
        let raw_nurb_param = self.convert_t_from_step_to_nurbs(anal_param);

        // clip endPoints
        let nurb_param = interval.clamp_value(raw_nurb_param);

        // when clamped - recompute the distance
        if nurb_param != raw_nurb_param {
            let point = self.evaluate_point(nurb_param);
            dist = point.distance(test_point);
        }

        // skip solutions when MINIMIZING   and sol greater than OptTargetDist + Tol
        //                when INTERSECTING and sol greater than Tol
        if (operation == SolverOperation::Minimize
            && target_distance.is_some_and(|d| dist > d + tol))
            || (operation == SolverOperation::Intersect && dist > tol)
        {
            return Ok(Vec::new());
        }

        // keep solution
        sol.start.solution_value = dist;
        sol.start.parameters[0] = nurb_param;
        Ok(vec![sol])
    }

    /// Extrema of the distance from `test_point` to the line over `step_interval`
    /// (STEP domain) (`GlobalPointSolveSTEP`): for [`SolverOperation::Maximize`] the
    /// farther end (both ends when their distances agree within `tol`, the other one
    /// first); otherwise the perpendicular foot, clamped to the interval (no solution
    /// for [`SolverOperation::Normalize`] outside it, nor for
    /// [`SolverOperation::Intersect`] farther than `tol`).
    #[must_use]
    pub fn global_point_solve_step<O: Copy>(
        &self,
        step_interval: &Extent1d,
        operation: SolverOperation,
        test_point: Point3,
        tol: f64,
        this: Option<O>,
    ) -> Vec<Solution<O>> {
        let mut out = Vec::new();

        // get limited interval
        let anal_ivl = self
            .anal_domain
            .intersect(step_interval)
            .unwrap_or_else(|_| Extent1d::new());
        let anal_ivl = anal_ivl.union(step_interval);

        let mut sol = Solution::new();
        let param;
        let distance_to_curve;

        // For maximize: Sol = furthest line endPoint, 2 solutions when equidistant to both endPoints
        if operation == SolverOperation::Maximize {
            // get distances from TestPoint to Line Start and End Points
            let p1 = self.evaluate_step_point(anal_ivl.min());
            let p2 = self.evaluate_step_point(anal_ivl.max());
            let dist1 = test_point.distance(p1);
            let dist2 = test_point.distance(p2);

            // set params for 1st and possible 2nd solutions
            let (param2, distance2);
            if dist1 > dist2 {
                param = step_interval.max();
                distance_to_curve = dist2;
                param2 = step_interval.min();
                distance2 = dist1;
            } else {
                param = step_interval.min();
                distance_to_curve = dist1;
                param2 = step_interval.max();
                distance2 = dist2;
            }

            // when endPoint distances are equal - output 2 solutions
            if are_same_to_tol(dist1, dist2, tol) {
                sol.solution_type = SolutionType::SingleValue;
                sol.num_objects = 1;
                sol.objects[0] = this;
                sol.nodes[0] = None;
                sol.num_variables = 1;
                sol.start.parameters[0] = param2;
                sol.start.solution_value = distance2;
                out.push(sol);
            }
        } else {
            // get nearest LinePoint to TestPoint
            let line_vec = self.scale * self.vector;
            let mut p = line_closest_point(self.point, line_vec, test_point);

            // when solution is beyond line bounds - clamp or quit
            if !anal_ivl.contains_value(p, 0.0) {
                if operation == SolverOperation::Normalize {
                    return out; // No answer found
                }
                p = step_interval.clamp_value(p);
            }

            // Get the LinePoint/TestPoint distance
            let p1 = self.evaluate_step_point(p);
            let dist1 = test_point.distance(p1);

            // when intersecting - no solutions when dist > tol
            if operation == SolverOperation::Intersect && dist1 > tol {
                return out; // No answer found
            }
            param = p;
            distance_to_curve = dist1;
        }

        // output a solution
        sol.solution_type = SolutionType::SingleValue;
        sol.num_objects = 1;
        sol.objects[0] = this;
        sol.nodes[0] = None;
        sol.num_variables = 1;
        sol.start.parameters[0] = param;
        sol.start.solution_value = distance_to_curve;
        out.push(sol);
        out
    }

    /// The STEP parameter of the line point closest to `point` and the point's
    /// location: interior when the parameter is in the domain and the point within the
    /// scaled zero of the line, else exterior (`STEPInversion`).
    #[must_use]
    pub fn step_inversion(&self, point: Point3) -> (f64, CurveLocationType) {
        // find u to minimize |Line(u) - P| where Line(u) = LinePoint + u * dScale * LineVector
        let line_vec = self.scale * self.vector;
        let param = line_closest_point(self.point, line_vec, point);

        // check that point maps to inside of line
        let mut loc = if self.anal_domain.contains_value(param, 0.0) {
            CurveLocationType::Interior
        } else {
            CurveLocationType::Exterior
        };

        // check distance to point
        if loc == CurveLocationType::Interior {
            let p = self.evaluate_step_point(param);
            let dist = p.distance(point);
            let scaled_zero = EFF_ZERO * (1.0 + p.max_dimension());

            // when dist is too large - point is not on the line
            if dist > scaled_zero {
                loc = CurveLocationType::Exterior;
            }
        }
        (param, loc)
    }

    /// Intersections of this line over `interval` with the ellipse `other` over
    /// `other_interval` (`IntersectWithEllipse`: [`Ellipse::intersect_with_line`] with
    /// the objects and parameters of every solution swapped). Solutions name
    /// `this_obj` and `other_obj`; `start.parameters` holds `[this param, other
    /// param]`.
    ///
    /// # Errors
    ///
    /// The errors of [`Ellipse::intersect_with_line`].
    pub fn intersect_with_ellipse<O: Copy>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: &Ellipse,
        other_interval: &Extent1d,
        tol: f64,
        this_obj: Option<O>,
        other_obj: Option<O>,
    ) -> Result<LineLineIntersection<O>> {
        // pass the call to SmEllipse::IntersectWithLine
        let r = other.intersect_with_line::<O>(
            k,
            other_interval,
            self,
            interval,
            tol,
            other_obj,
            this_obj,
        )?;
        let mut r = LineLineIntersection {
            solutions: r.solutions,
            needs_more_intersections: r.needs_more_intersections,
        };

        // set output - for every solution swap the object and parameter orders
        for sol in &mut r.solutions {
            sol.objects.swap(0, 1);
            sol.start.parameters.swap(0, 1);
            if sol.solution_type == SolutionType::RangeOfValues {
                sol.end.parameters.swap(0, 1);
            }
        }
        Ok(r)
    }

    /// Intersections of this line over `interval` with `other` over `other_interval`
    /// (both NURB domains) within `tol` (`IntersectWithLine`): no solution for
    /// disjoint boxes or a degenerate segment, a single value for crossing segments, a
    /// range (sorted by this line's parameter) for overlapping ones. Solutions name
    /// `this` and `other_obj`; `start.parameters` holds `[this param, other param]`.
    ///
    /// # Errors
    ///
    /// Those of the segment intersection.
    pub fn intersect_with_line<O: Copy>(
        &self,
        interval: &Extent1d,
        other: &Self,
        other_interval: &Extent1d,
        tol: f64,
        this: Option<O>,
        other_obj: Option<O>,
    ) -> Result<LineLineIntersection<O>> {
        let mut out = LineLineIntersection {
            solutions: Vec::new(),
            needs_more_intersections: false,
        };

        // Self-intersection test.
        if core::ptr::eq(self, other) {
            return Ok(out);
        }

        // Get Line endPoints and vecs
        let start1 = self.evaluate_point(interval.min());
        let end1 = self.evaluate_point(interval.max());
        let start2 = other.evaluate_point(other_interval.min());
        let end2 = other.evaluate_point(other_interval.max());
        let vec1 = end1 - start1;

        // check for disjoint solution
        let mut bbox1 = Extent3d::from_point(start1);
        bbox1.add_point(end1);
        bbox1.expand_absolute(tol);
        let mut bbox2 = Extent3d::from_point(start2);
        bbox2.add_point(end2);
        if bbox1.are_disjoint(&bbox2, 0.0) {
            return Ok(out);
        }

        // intersect the 2 segments: 0 = No xSect, 1 = Point xSect, 2 = Coincident segment
        let vec2 = end2 - start2;
        let int_pnts = if vec1.length_squared() > EFF_ZERO_SQ && vec2.length_squared() > EFF_ZERO_SQ
        {
            segment_segment_intersect(start1, end1, start2, end2, tol)?
        } else {
            Vec::new()
        };

        let mut sol = Solution::new();
        sol.num_objects = 2;
        sol.objects[0] = this;
        sol.objects[1] = other_obj;

        // when 1 or more solutions were found
        if let Some(&ip) = int_pnts.first() {
            // find params for 1st solution
            let param1 = line_closest_point(start1, vec1, ip);
            let int_pnt1 = start1 + param1 * vec1;
            let param2 = line_closest_point(start2, vec2, ip);
            let int_pnt2 = start2 + param2 * vec2;

            // set sSol to be a single value solution
            sol.solution_type = SolutionType::SingleValue;
            sol.num_variables = 2;
            sol.start.parameters[0] = interval.evaluate(param1);
            sol.start.parameters[1] = other_interval.evaluate(param2);
            sol.start.solution_value = int_pnt1.distance(int_pnt2);
        }

        // when segments are coincident
        if let Some(&ip) = int_pnts.get(1) {
            // find params for solution
            let param1 = line_closest_point(start1, vec1, ip);
            let int_pnt1 = start1 + param1 * vec1;
            let param2 = line_closest_point(start2, vec2, ip);
            let int_pnt2 = start2 + param2 * vec2;

            // change sSol to be a range solution
            sol.solution_type = SolutionType::RangeOfValues;
            sol.end.parameters[0] = interval.evaluate(param1);
            sol.end.parameters[1] = other_interval.evaluate(param2);
            let dist = int_pnt1.distance(int_pnt2);
            sol.end.solution_value = dist;

            // Sort by parameter values of first curve
            if sol.start.parameters[0] > sol.end.parameters[0] {
                // Swap
                sol.end.parameters[0] = sol.start.parameters[0];
                sol.end.parameters[1] = sol.start.parameters[1];
                sol.end.solution_value = sol.start.solution_value;
                sol.start.parameters[0] = interval.evaluate(param1);
                sol.start.parameters[1] = other_interval.evaluate(param2);
                sol.start.solution_value = dist;
            }
        }

        // set output
        if !int_pnts.is_empty() {
            out.solutions.push(sol);
        }
        Ok(out)
    }

    // ---- editing ----------------------------------------------------------------------

    /// Rebuilds the NURB from the analytic data (`MakeNurb`): a degree 1 curve with
    /// knots `{min, min, max, max}` of the analytic domain and the STEP end points as
    /// control points (swapped when `inside_out`), made 2D for a 2D line.
    ///
    /// # Errors
    ///
    /// None for a line without owner (see `make_nurb_owned`).
    pub fn make_nurb(&mut self) -> Result<()> {
        self.make_nurb_owned(None)
    }

    /// [`Line::make_nurb`] for a line owned by `owner`: the NURB of the generating line
    /// of an `SmSurfOfExtrusion` / `SmSurfOfRevolution` is then reparameterised to the
    /// surface's generating-direction interval.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] for such an owner (`GetGenDirParamExtent`, units
    /// `surface.surf_of_extrusion_p1`, `surface.surf_of_revolution_p1`).
    pub(crate) fn make_nurb_owned(&mut self, owner: Option<ObjectRef>) -> Result<()> {
        // free existing m_pNurb pointer if any
        self.base.nurb = None;

        // get min/max points
        let (start, end) = if self.inside_out {
            (
                self.evaluate_step_point(self.anal_domain.max()),
                self.evaluate_step_point(self.anal_domain.min()),
            )
        } else {
            (
                self.evaluate_step_point(self.anal_domain.min()),
                self.evaluate_step_point(self.anal_domain.max()),
            )
        };

        // allocate new nurb curve
        let mut cur = allocate_nurb_curve(1, 1, 3)?;

        // set naturalInterval = analDomain Interval
        let (min, max) = (self.anal_domain.min(), self.anal_domain.max());
        cur.knots_mut().copy_from_slice(&[min, min, max, max]);

        // set ControlPoints = [MinPoint, MaxPoint] - not rational
        for (cp, p) in cur.cpts_mut().iter_mut().zip([start, end]) {
            cp.x = p.x;
            cp.y = p.y;
            cp.z = p.z;
            cp.w = NOW;
        }

        // save the Nurb pointer
        self.base.nurb = Some(cur);
        self.base.nurb_is_borrowed = false;

        // Set ControlPoint Z values for 2d curves
        if self.base.dim == 2 {
            self.convert_to_2d_owned(owner)?;
        }

        // when a SmSurfOfExtrusion GenCurve, look for Parent Surface NURB Interval constraint
        if owner.is_some_and(|o| o.type_code == SURF_OF_EXTRUSION_TYPE) {
            // DISPATCH GAP: SmSurfOfExtrusion::GetGenDirParamExtent (surface.surf_of_extrusion_p1,
            // ported as SurfOfExtrusion::gen_dir_param_extent; no UNPORTED marker: the owner is an
            // opaque ObjectRef. An owning extrusion does this step itself (adjust_generator_interval,
            // SurfOfExtrusion::make_nurb) and never sets its generator's owner, so nothing reaches here)
            return Err(SolidError::Unported {
                upstream: "SmSurfOfExtrusion::GetGenDirParamExtent",
                unit: "surface.surf_of_extrusion_p1",
            });
        }

        // when a SmSurfOfRevolution GenCurve, look for Parent Surface NURB Interval constraint
        if owner.is_some_and(|o| o.type_code == SURF_OF_REVOLUTION_TYPE) {
            // DISPATCH GAP: SmSurfOfRevolution::GetGenDirParamExtent (surface.surf_of_revolution_p1,
            // ported as SurfOfRevolution::gen_dir_param_extent; no UNPORTED marker: the owner is an
            // opaque ObjectRef. An owning revolution does this step itself (e.g. the cone's
            // adjust_gen_line_with_owner) and never sets its generator's owner, so nothing reaches here)
            return Err(SolidError::Unported {
                upstream: "SmSurfOfRevolution::GetGenDirParamExtent",
                unit: "surface.surf_of_revolution_p1",
            });
        }
        Ok(())
    }

    /// `GetOrCreateGwNurbPointer`: makes the NURB when missing (status ignored).
    fn ensure_nurb(&mut self, owner: Option<ObjectRef>) -> Result<()> {
        if self.base.nurb.is_none() {
            ignore_status(self.make_nurb_owned(owner))?;
        }
        Ok(())
    }

    /// Makes the line 2D: dimension 2, `z = 0` for the point and vector, the NURB's
    /// `z` removed (`ConvertTo2D`).
    ///
    /// # Errors
    ///
    /// None for a line without owner.
    pub fn convert_to_2d(&mut self) -> Result<()> {
        self.convert_to_2d_owned(None)
    }

    /// [`Line::convert_to_2d`] for a line owned by `owner` (see
    /// [`Line::make_nurb_owned`]).
    pub(crate) fn convert_to_2d_owned(&mut self, owner: Option<ObjectRef>) -> Result<()> {
        // project STEP params to XY plane
        self.base.dim = 2;
        self.point.z = 0.0;
        self.vector.z = 0.0;

        // convert the underlying Nurb
        self.ensure_nurb(owner)?;
        let _ = self.base.convert_to_2d();
        Ok(())
    }

    /// Makes the line 3D (`ConvertTo3D`): the NURB's `z` becomes 0; nothing to do for
    /// a 3D line.
    ///
    /// # Errors
    ///
    /// None for a line without owner.
    pub fn convert_to_3d(&mut self) -> Result<()> {
        if self.base.dim == 3 {
            return Ok(());
        }
        self.base.dim = 3;

        // convert the underlying Nurb
        self.ensure_nurb(None)?;
        let _ = self.base.convert_to_3d();
        Ok(())
    }

    /// Reparameterises the NURB to `new_param` (`SmBSplineCurve::EditParameterization`,
    /// inherited): nothing happens when the natural interval already has those ends
    /// (within the scaled zero); a missing NURB is made first.
    ///
    /// # Errors
    ///
    /// Those of [`BSplineCurve::edit_parameterization`].
    pub fn edit_parameterization(&mut self, new_param: &Extent1d) -> Result<()> {
        self.edit_parameterization_owned(None, new_param)
    }

    /// [`Line::edit_parameterization`] for a line owned by `owner` (see
    /// [`Line::make_nurb_owned`]).
    pub(crate) fn edit_parameterization_owned(
        &mut self,
        owner: Option<ObjectRef>,
        new_param: &Extent1d,
    ) -> Result<()> {
        // get current interval
        let old = self.natural_interval();

        // low work: current interval == new interval
        if are_same(old.min(), new_param.min()) && are_same(old.max(), new_param.max()) {
            return Ok(());
        }
        self.ensure_nurb(owner)?;
        self.base.edit_parameterization(new_param)
    }

    /// `SmBSplineCurve::CreateAnalyticCurve` (inherited) of a line over `interval`: a
    /// new bounded line through the line's points over `interval` when its ends there
    /// match this line's within the scaled zero of the point at `0.456789`. A missing
    /// NURB is made (`GetNumberControlPoints`).
    ///
    /// # Errors
    ///
    /// None for a line without owner.
    pub fn create_analytic_curve(&mut self, interval: &Extent1d) -> Result<Option<Self>> {
        // use midPoint to set dScaledZero
        let mid_pnt = self.evaluate_point(interval.evaluate(0.456789));
        let scaled_zero = EFF_ZERO * (1.0 + mid_pnt.max_dimension());

        // check for a line bspline (degree = 1; controlPointCount == 2); the control
        // point count makes a missing NURB
        self.ensure_nurb(None)?;
        if self.degree() != 1 || self.base.number_control_points()? != 2 {
            return Ok(None);
        }

        // IsLine is always TRUE for a line
        let (line_pnt, line_vec) = self.is_line();
        let line_origin = line_pnt - interval.min() * (line_vec / interval.length());
        let line_vector = line_vec / line_vec.length();
        let scale = line_vec.length() / interval.length();
        let analytic = Self::bounded(
            line_origin,
            line_vector,
            interval,
            &BoundedLineOptions {
                scale,
                dim: self.dim(),
                ..BoundedLineOptions::default()
            },
        );

        // if original and analytic given interval endpoints are too far apart
        for t in [interval.min(), interval.max()] {
            let or_pnt = self.evaluate_point(t);
            let an_pnt = analytic.evaluate_point(t);
            if or_pnt.distance(an_pnt) > scaled_zero {
                // don't return the analytic object
                return Ok(None);
            }
        }
        Ok(Some(analytic))
    }

    /// Sets the analytic domain to `new_interval` and rebuilds an existing NURB
    /// (`AdjustSTEPInterval`); nothing changes when the domains agree within
    /// `SM_EFF_ZERO`.
    ///
    /// # Errors
    ///
    /// None for a line without owner.
    pub fn adjust_step_interval(&mut self, new_interval: &Extent1d) -> Result<()> {
        // no work - two intervals are the same
        if new_interval.is_contained_by(&self.anal_domain, EFF_ZERO)
            && self.anal_domain.is_contained_by(new_interval, EFF_ZERO)
        {
            return Ok(());
        }

        // set the domain
        self.anal_domain = *new_interval;

        // rebuild the Nurb Curve
        if self.base.nurb.is_some() {
            self.make_nurb_owned(None)?;
        }
        Ok(())
    }

    /// A copy moved by `offset_distance` along `V x offset_plane_normal`, with its
    /// NURB rebuilt, and the gap `0` (`CreateSimpleOffset`; the approximation
    /// tolerance and parameterisation flag are not used).
    ///
    /// # Errors
    ///
    /// `InvalidInput` when the line is parallel to the normal (no offset direction).
    pub fn create_simple_offset(
        &self,
        offset_plane_normal: Vec3,
        offset_distance: f64,
    ) -> Result<(Self, f64)> {
        // The offset direction is the line vector crossed with the normal.
        let off_dir = self
            .vector
            .cross(offset_plane_normal)
            .unitize()
            .map_err(|_| SolidError::InvalidInput {
                operation: OP_OFFSET,
                reason: "line parallel to the offset plane normal",
            })?;

        // Ok, all is good.  Create an SmLine for output.
        let mut offset_line = self.copy();
        offset_line.point += offset_distance * off_dir;
        ignore_status(offset_line.make_nurb_owned(None))?;
        Ok((offset_line, 0.0))
    }

    /// Joins `other` (a B-spline or line, end `join_end_other`: 0 start, 1 end) onto
    /// this line at `join_end_this` by extending the analytic domain
    /// (`JoinWith`). The join points must be within `gap_tolerance` (default
    /// `length * sqrt(SM_EFF_ZERO) * 1000`), `other` must be a line collinear with
    /// this one.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the curves cannot be joined; those of the other curve's queries
    /// ([`SolidError::Unported`] from `SmCurve::IsLine` for some B-splines).
    pub fn join_with(
        &mut self,
        join_end_this: u64,
        other: &Curve,
        join_end_other: u64,
        gap_tolerance: Option<f64>,
    ) -> Result<()> {
        // We'll need these:
        let ivl1 = self.natural_interval();
        let this_len = self.length(&ivl1);

        // Get a tolerance to use.
        let tol = gap_tolerance.unwrap_or(this_len * EFF_ZERO_SQRT * 1000.0);

        // Grab all end points here.
        self.ensure_nurb(None)?;
        let this_ends = self.base.ends()?;
        let (mut this_common, mut this_end) = (this_ends.start, this_ends.end);
        let other_ends = other_ends(other)?;
        let (mut other_common, mut other_end) = (other_ends.start, other_ends.end);

        // Sort out which are the common point vs. the new end points.
        if join_end_this == 1 {
            core::mem::swap(&mut this_common, &mut this_end);
        }
        if join_end_other == 1 {
            core::mem::swap(&mut other_common, &mut other_end);
        }

        // Check join-point distance.
        if this_common.distance(other_common) > tol {
            return Err(SolidError::sm_err(OP_JOIN));
        }

        // Other curve must be linear.
        if !other_is_line(other, 5, tol)? {
            return Err(SolidError::sm_err(OP_JOIN));
        }

        // And it must be collinear with us.
        let whole_line_vec = other_end - this_end;
        let (dist, _) = line_point_distance(this_end, whole_line_vec, this_common);
        if dist > tol {
            return Err(SolidError::sm_err(OP_JOIN));
        }

        // Length of other curve's param range will be ours, scaled by the 3d lengths.
        let other_len = other_common.distance(other_end);
        let step_ivl = self.step_interval();
        let delta_param = step_ivl.length() * other_len / this_len;

        // Join ends refer to NURBS parameters. For an inside-out line, its NURBS
        // start is the analytic end, so extend the opposite end of the STEP interval.
        let extend_step_min = (join_end_this == 0) != self.inside_out;
        let new_param = if extend_step_min {
            step_ivl.min() - delta_param
        } else {
            step_ivl.max() + delta_param
        };
        self.anal_domain.add_value(new_param);

        // And update the Nurbs.
        ignore_status(self.make_nurb_owned(None))
    }

    /// Refreshes the analytic data from the NURB after the NURB was edited
    /// (`RefreshAnalytics`): domain = NURB domain, point and unit vector from the NURB
    /// end points (in reverse when `inside_out`), scale = chord / domain length (`1`
    /// when below `SM_EFF_ZERO`).
    ///
    /// # Errors
    ///
    /// None for a line without owner.
    pub fn refresh_analytics(&mut self) -> Result<()> {
        // refresh AnalDomain
        self.anal_domain = self.natural_interval();

        // get Nurb endPoints (status ignored upstream)
        self.ensure_nurb(None)?;
        let (start, end) = match self.base.ends() {
            Ok(e) => (e.start, e.end),
            Err(_) => (Vec3::UNDEF, Vec3::UNDEF),
        };

        // switch on m_bInsideOut
        if self.inside_out {
            self.vector = start - end;
            self.scale = self.vector.length() / self.anal_domain.length();
            if self.scale < EFF_ZERO {
                self.scale = 1.0;
            }
            self.vector = self.vector.unitize_or_z();
            self.point = end - self.vector * self.scale * self.anal_domain.min();
        } else {
            self.vector = end - start;
            self.scale = self.vector.length() / self.anal_domain.length();
            if self.scale < EFF_ZERO {
                self.scale = 1.0;
            }
            self.vector = self.vector.unitize_or_z();
            self.point = start - self.vector * self.scale * self.anal_domain.min();
        }
        Ok(())
    }

    /// Reverses the direction of the line and returns the new interval of
    /// `old_interval` (`ReverseParameterization`): a bounded line's point moves to its
    /// mirror position so the domain is kept; the NURB (if any) is reversed.
    #[must_use]
    pub fn reverse_parameterization(&mut self, old_interval: &Extent1d) -> Extent1d {
        // set output interval
        let mut new_interval = *old_interval;

        // when bounded - switch startPoint to current endPoint position
        if self.is_bounded() {
            self.point = self.evaluate_step_point(self.anal_domain.max() + self.anal_domain.min());
        }

        // negate the tangent vector
        self.vector = -self.vector;

        // reverse Nurb curve (status ignored upstream)
        if self.base.nurb.is_some() {
            if let Ok(ivl) = self.base.reverse_parameterization(old_interval) {
                new_interval = ivl;
            }
        }
        new_interval
    }

    /// Scales (about the origin, per axis) and then rotates and moves the line
    /// (`Transform`); point, unit vector and scale are rebuilt from the images of the
    /// STEP points at 0 and 1, and an existing NURB is transformed.
    ///
    /// # Errors
    ///
    /// Those of [`BSplineCurve::transform`].
    pub fn transform(&mut self, rotate_n_move: &Axis2Placement, scale: Option<Vec3>) -> Result<()> {
        let identity_scale = Vec3::new(1.0, 1.0, 1.0);

        // no work - identity transform
        if rotate_n_move.is_identity(EFF_ZERO) && scale.is_none_or(|s| s.approx_eq(identity_scale))
        {
            return Ok(());
        }

        // Get Anal StartPoint and Line(1.0) point.
        let mut start = self.point;
        let mut end = self.evaluate_step_point(1.0);

        // scale the points
        if let Some(s) = scale {
            start = start.multiply(s);
            end = end.multiply(s);
        }

        // rotate and move the points
        start = rotate_n_move.transform_point(start);
        end = rotate_n_move.transform_point(end);

        // reset the analytic parameters
        self.point = start;
        self.vector = end - start;
        self.scale = self.vector.length();
        self.vector = self.vector.unitize_or_z();

        // transform the nurb curve
        if self.base.nurb.is_some() {
            self.base.transform(rotate_n_move, scale)?;
        }
        Ok(())
    }

    /// Moves the line ends to `tgt_start` / `tgt_end` (`DeformToEndPointTargets`). With
    /// both tangent types [`InValueType::Unconstrained`] the line itself becomes the
    /// segment between the targets (domain `[0, 1]`) and `None` is returned;
    /// otherwise a B-spline copy of the line deformed to the targets and tangents is
    /// returned. Nothing happens when nothing is asked.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when a new curve is needed but `want_new_curve` is false; those of
    /// [`BSplineCurve::deform_to_end_point_targets`].
    pub fn deform_to_end_point_targets(
        &mut self,
        tgt_start: Option<Point3>,
        tgt_end: Option<Point3>,
        want_new_curve: bool,
        start_tan: Option<Vec3>,
        start_tan_type: InValueType,
        end_tan: Option<Vec3>,
        end_tan_type: InValueType,
    ) -> Result<Option<BSplineCurve>> {
        let same_or_free =
            |t: InValueType| t == InValueType::Same || t == InValueType::Unconstrained;

        // no work - no requested changes
        if tgt_start.is_none()
            && tgt_end.is_none()
            && same_or_free(start_tan_type)
            && same_or_free(end_tan_type)
        {
            return Ok(None);
        }

        // check input - when either EndTanType is SM_IV_SPECIFIED or SM_IV_SAME, a new curve must be wanted
        let both_free = start_tan_type == InValueType::Unconstrained
            && end_tan_type == InValueType::Unconstrained;
        if !want_new_curve && !both_free {
            return Err(SolidError::InvalidInput {
                operation: OP_DEFORM,
                reason: "a new curve is needed for SM_IV_SAME or SM_IV_SPECIFIED tangents",
            });
        }

        // asked to just move the end points - edit EndPts in-place (changes tan dirs)
        if both_free {
            // init line parameters
            let mut ivl = Extent1d::from_min_max(0.0, 1.0);
            let start =
                tgt_start.unwrap_or(self.point + self.scale * self.anal_domain.min() * self.vector);
            let end =
                tgt_end.unwrap_or(self.point + self.scale * self.anal_domain.max() * self.vector);
            let mut vec = end - start;
            let mut scale = vec.length();

            // watch out for degenerate lines
            if scale < EFF_ZERO {
                vec = Vec3::new(1.0, 0.0, 0.0);
                scale = 1.0;
                let _ = ivl.set_min_max(0.0, 0.0);
            } else {
                vec /= scale; // Unitize.
            }

            // deform the line
            self.point = start;
            self.vector = vec;
            self.anal_domain = ivl;
            self.scale = scale;

            // refresh m_pNurb
            ignore_status(self.make_nurb_owned(None))?;
            Ok(None)
        } else {
            // when needed - make m_pNurb
            self.ensure_nurb(None)?;

            // make new line BSplineCurve and pass the call along to the BSplineCurve
            let mut new_curve = self.base.copy();
            new_curve.deform_to_end_point_targets(
                tgt_start,
                tgt_end,
                start_tan,
                start_tan_type,
                end_tan,
                end_tan_type,
            )?;
            Ok(Some(new_curve))
        }
    }

    /// Trims the line to `trim_interval` (NURB domain, not snapped) (`Trim`): the
    /// analytic domain becomes the STEP image of the interval, the NURB is rebuilt and
    /// reparameterised to `trim_interval`. Nothing happens when the natural interval
    /// already lies inside it (within `SM_EFF_ZERO`) or for
    /// [`TrimMode::IntervalOnly`].
    ///
    /// # Errors
    ///
    /// None for a line without owner.
    pub fn trim(&mut self, trim_interval: &Extent1d, mode: TrimMode) -> Result<()> {
        self.trim_owned(None, trim_interval, mode)
    }

    /// [`Line::trim`] for a line owned by `owner` (see `make_nurb_owned`).
    pub(crate) fn trim_owned(
        &mut self,
        owner: Option<ObjectRef>,
        trim_interval: &Extent1d,
        mode: TrimMode,
    ) -> Result<()> {
        // no work - current natural interval is a subSet of targetInterval
        let nat_ivl = self.natural_interval();
        if nat_ivl.is_contained_by(trim_interval, EFF_ZERO) {
            return Ok(());
        }

        // When asked - just return the Nurb interval
        if mode == TrimMode::IntervalOnly {
            return Ok(());
        }

        // trim the analytic interval
        let step_min = self.convert_t_from_nurbs_to_step(trim_interval.min());
        let step_max = self.convert_t_from_nurbs_to_step(trim_interval.max());
        let _ = if self.inside_out {
            self.anal_domain.set_min_max(step_max, step_min)
        } else {
            self.anal_domain.set_min_max(step_min, step_max)
        };

        // to avoid tolerances - just rebuild the nurbCurve
        ignore_status(self.make_nurb_owned(owner))?;

        // Set Nurb Parameterization
        let _ = self.base.edit_parameterization(trim_interval);
        Ok(())
    }

    // ---- validity and input -----------------------------------------------------------

    /// Checks that the STEP and NURB end points of a bounded line with NURB agree within
    /// the scaled zero (`AssertValid`); failed checks are reported to `list` (as
    /// object `key`). Unless `level_given` the `SmBSplineCurve` checks run first, with
    /// the line's virtual methods; they make a missing NURB first (`MakeNurb` is
    /// virtual: the line's NURB), a side effect kept here.
    ///
    /// # Errors
    ///
    /// Those of [`BSplineCurve::assert_valid`] unless `level_given`;
    /// [`SolidError::Unported`] from `MakeNurb`.
    pub fn assert_valid(
        &mut self,
        k: &Kernel,
        list: Option<&mut AssertArray>,
        level_given: bool,
        key: ObjectKey,
    ) -> Result<bool> {
        self.assert_valid_on(k, list, level_given, key, None)
    }

    /// [`Line::assert_valid`] of a UV trim on `trim_surface`, its face's surface: the
    /// `SmBSplineCurve` checks measure the trim on it with
    /// `Kernel::fixes.uv_trim_checks_on_surface` (WR-560, see
    /// [`BSplineCurve::assert_valid_on`]).
    ///
    /// # Errors
    ///
    /// As [`Line::assert_valid`].
    pub fn assert_valid_on(
        &mut self,
        k: &Kernel,
        list: Option<&mut AssertArray>,
        level_given: bool,
        key: ObjectKey,
        trim_surface: Option<&Surface>,
    ) -> Result<bool> {
        let mut list = list;
        let mut rtn = true;

        // call the base class AssertValid
        if !level_given {
            // SmBSplineCurve::AssertValid makes a missing NURB (virtual MakeNurb,
            // status ignored)
            if self.base.nurb.is_none() {
                ignore_status(self.make_nurb())?;
            }
            let this = &*self;
            let is_degenerate = |tol: f64| Ok(this.is_degenerate(tol, None));
            rtn &= this.base.assert_valid_with(
                k,
                list.as_deref_mut(),
                false,
                key,
                &DerivedOverrides {
                    degree: Some(this.degree()),
                    analytic: this.is_analytic(),
                    bounded: this.is_bounded(),
                    is_degenerate: &is_degenerate,
                    trim_surface,
                },
            )?;
        }

        // when line is bounded and there is a Nurb representation
        if self.is_bounded() && self.base.nurb.is_some() {
            let nurb_domain = self.natural_interval();

            // evaluate STEP endPoints
            let step_start = self.evaluate_step_point(self.anal_domain.min());
            let step_end = self.evaluate_step_point(self.anal_domain.max());

            // evaluate Nurb endPoints (statuses ignored upstream)
            let nurb_start = self
                .base
                .evaluate_point(nurb_domain.min())
                .unwrap_or(Vec3::UNDEF);
            let nurb_end = self
                .base
                .evaluate_point(nurb_domain.max())
                .unwrap_or(Vec3::UNDEF);

            // verify that STEP and Nurb representations are equivalent
            let scaled_zero =
                EFF_ZERO * (1.0 + max2(step_start.max_dimension(), step_end.max_dimension()));

            let checks = if self.inside_out {
                [(0, step_start, nurb_end), (1, step_end, nurb_start)]
            } else {
                [(2, step_start, nurb_start), (3, step_end, nurb_end)]
            };
            for (index, a, b) in checks {
                rtn &= value_report(
                    &mut list,
                    key,
                    index,
                    a.closer_than(scaled_zero, b),
                    scaled_zero,
                    a.distance(b),
                )?;
            }
        }
        Ok(rtn)
    }

    /// Reads a line written by `WriteToDB` (`ReadFromDB`): point, vector, domain
    /// ends, scale and inside-out flag (one group per line in ASCII files), then the
    /// B-spline part (`SmBSplineCurve::ReadFromDB`, status ignored); a 3D file read for
    /// `dim == 2` is made 2D.
    ///
    /// # Errors
    ///
    /// Those of the binary reads of the line data.
    pub fn read_from_db(db: &mut DatabaseReader, dim: u64, version: u64) -> Result<Self> {
        // init output object
        let mut line = Self::default();

        // locals
        let mut point = Vec3::UNDEF;
        let mut vector = Vec3::UNDEF;
        let (mut anal_min, mut anal_max) = (0.0, 0.0);
        let mut scale = 0.0;
        let mut inside_out: i32 = 0;

        if db.file_type() == FileType::Ascii {
            for v in [&mut point, &mut vector] {
                if let Some(s) = db.in_stream_mut() {
                    for c in [&mut v.x, &mut v.y, &mut v.z] {
                        if let Some(x) = s.extract_double() {
                            *c = x;
                        }
                    }
                }
                db.go_to_next_line();
            }
            if let Some(s) = db.in_stream_mut() {
                for c in [&mut anal_min, &mut anal_max] {
                    if let Some(x) = s.extract_double() {
                        *c = x;
                    }
                }
            }
            db.go_to_next_line();
            if let Some(x) = db.in_stream_mut().and_then(InputStream::extract_double) {
                scale = x;
            }
            db.go_to_next_line();
            if let Some(x) = db.in_stream_mut().and_then(InputStream::extract_int) {
                inside_out = x;
            }
            db.go_to_next_line();
        } else {
            point.x = db.read_double()?;
            point.y = db.read_double()?;
            point.z = db.read_double()?;
            vector.x = db.read_double()?;
            vector.y = db.read_double()?;
            vector.z = db.read_double()?;
            anal_min = db.read_double()?;
            anal_max = db.read_double()?;
            scale = db.read_double()?;
            inside_out = db.read_boolean()?;
        }

        // load the obj
        line.point = point;
        line.vector = vector;
        let _ = line.anal_domain.set_min_max(anal_min, anal_max);
        line.scale = scale;
        line.inside_out = inside_out != 0;

        // read the base type (status ignored upstream); its 2D fix-up of a 3D file
        // calls the line's ConvertTo2D, so it is done here
        match BSplineCurve::read_from_db(db, 3, version) {
            Ok(b) => {
                let read_dim = b.dim;
                line.base = b;
                if dim == 2 && read_dim == 3 {
                    line.convert_to_2d()?;
                }
            }
            Err(e @ SolidError::Unported { .. }) => return Err(e),
            Err(_) => {}
        }
        Ok(line)
    }

    /// `WriteToDB`: writes the line point, vector, analytic domain, scale and
    /// inside-out flag, then the B-spline curve part.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the B-spline part has no NURB (upstream ignores the part's
    /// status; its virtual `GetOrCreateGwNurbPointer` always has one to write).
    pub fn write_to_db(&self, db: &mut DatabaseWriter, version: u64) -> Result<()> {
        let (p, v) = (self.point, self.vector);
        let (lo, hi) = (self.anal_domain.min(), self.anal_domain.max());
        let inside_out = i32::from(self.inside_out);
        if db.file_type() == FileType::Ascii {
            db.out_reals(&[p.x, p.y, p.z]);
            db.out_str(" SmLine Line Point \n");
            db.out_reals(&[v.x, v.y, v.z]);
            db.out_str(" SmLine Line Vector \n");
            db.out_reals(&[lo, hi]).out_str(" SmLine Line Domain \n");
            db.out_f64(self.scale).out_str(" SmLine Line Scale \n");
            db.out_i64(inside_out.into());
            db.out_str(" SmLine InsideOut Flag \n");
        } else {
            db.write_doubles(&[p.x, p.y, p.z, v.x, v.y, v.z, lo, hi, self.scale]);
            db.write_boolean(inside_out);
        }
        self.base.write_to_db(db, version)
    }
}

/// The `SM_ASSERT_VALUE_REPORT` macro of `AssertValid` (an `SM_LEVEL_0` test, so the
/// predicate always runs): `true` when `passed`; otherwise, with a list, a report for
/// test `test_index` (tolerance `tol`, value `val`) is added unless an equal one is
/// there.
fn value_report(
    list: &mut Option<&mut AssertArray>,
    key: ObjectKey,
    test_index: u64,
    passed: bool,
    tol: f64,
    val: f64,
) -> Result<bool> {
    if passed {
        if let Some(l) = list.as_deref_mut() {
            l.set_last_add(false);
        }
        return Ok(true);
    }
    if let Some(l) = list.as_deref_mut() {
        let r = crate::geom::assert_array::AssertReport::new(ReportSpec {
            reporter: Some(key),
            test_index,
            test_class: labels::LINE_TYPE,
            owner: Some(key),
            owner_type: labels::LINE_TYPE,
            owner_type_string: Some("SmLine"),
            other: None,
            other_type: 0,
            other_type_string: Some("NULL"),
            tol3d: tol3d(tol),
            value: val,
            list_index: 0,
            message: None,
        })?;
        l.add_unique_delete_duplicate(r);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg() -> Line {
        Line::bounded(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            &Extent1d::from_min_max(2.0, 7.0),
            &BoundedLineOptions {
                scale: 2.0,
                ..BoundedLineOptions::default()
            },
        )
    }

    #[test]
    fn tessellate_rejects_unallocatable_segment_counts() {
        // fuzz (curve_p1): `min_segments = i64::MAX` aborted on allocation
        let l = seg();
        let ivl = l.natural_interval();
        let all = TessellationRequest {
            parameters: true,
            points: true,
            tangents: true,
        };
        assert!(matches!(
            l.tessellate(&ivl, u64::MAX, all),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            l.tessellate(&ivl, u64::MAX >> 1, all),
            Err(SolidError::InvalidInput { .. })
        ));
        let t = l.tessellate(&ivl, 4, all).unwrap();
        assert_eq!(t.parameters.len(), 5);
        assert_eq!(t.points.len(), 5);
        assert_eq!(t.tangents.len(), 5);
    }

    #[test]
    fn bounded_line_maps_domains() {
        let mut l = seg();
        assert_eq!(l.natural_interval(), Extent1d::from_min_max(2.0, 7.0));
        assert_eq!(l.evaluate_point(2.0), Vec3::new(4.0, 0.0, 0.0));
        assert_eq!(l.evaluate_point(7.0), Vec3::new(14.0, 0.0, 0.0));
        let e = l.ends().unwrap();
        assert_eq!(e.start, Vec3::new(4.0, 0.0, 0.0));
        assert_eq!(e.end, Vec3::new(14.0, 0.0, 0.0));
        assert_eq!(l.length(&l.natural_interval()), 10.0);
        assert!(l.is_kind_of(Line::TYPE));
        assert!(l.is_kind_of(BSplineCurve::TYPE));
    }

    #[test]
    fn inside_out_reverses_nurb() {
        let mut l = Line::bounded(
            Vec3::ZERO,
            Vec3::new(1.0, 0.0, 0.0),
            &Extent1d::from_min_max(0.0, 2.0),
            &BoundedLineOptions {
                inside_out: true,
                ..BoundedLineOptions::default()
            },
        );
        let e = l.ends().unwrap();
        assert_eq!(e.start, Vec3::new(2.0, 0.0, 0.0));
        assert_eq!(e.end, Vec3::ZERO);
        assert_eq!(l.evaluate_point(0.0), Vec3::new(2.0, 0.0, 0.0));
        assert_eq!(l.evaluate(0.0, 1)[1], Vec3::new(-1.0, 0.0, 0.0));
    }

    #[test]
    fn two_point_line_has_no_nurb() {
        let l = Line::from_points(Vec3::ZERO, Vec3::new(0.0, 3.0, 4.0), 3);
        assert!(l.base().gw_nurb().is_none());
        assert_eq!(l.line_scale(), 5.0);
        assert_eq!(l.natural_interval(), Extent1d::from_min_max(0.0, 1.0));
        assert_eq!(l.length(&l.natural_interval()), 5.0);
        let d = Line::from_points(Vec3::ZERO, Vec3::ZERO, 3);
        assert_eq!(d.step_interval(), Extent1d::from_min_max(0.0, 0.0));
        assert!(d.is_degenerate(0.0, None));
    }

    #[test]
    fn reversed_line_join() {
        // prog_test my_test_reversed_line_join, one case
        let mut l = seg();
        let other = Line::bounded(
            Vec3::new(14.0, 0.0, 0.0),
            Vec3::new(6.0, 0.0, 0.0),
            &Extent1d::from_min_max(0.0, 2.0),
            &BoundedLineOptions {
                scale: 3.0,
                ..BoundedLineOptions::default()
            },
        );
        l.join_with(1, &Curve::from(other), 0, Some(1e-7)).unwrap();
        let e = l.ends().unwrap();
        assert!(e.end.distance(Vec3::new(20.0, 0.0, 0.0)) < 1e-7);
    }

    #[test]
    fn drop_and_solve() {
        let l = seg();
        let s = l
            .drop_point_fast::<u8>(
                &l.natural_interval(),
                SolverOperation::Minimize,
                Vec3::new(20.0, 1.0, 0.0),
                1e-6,
                None,
                None,
            )
            .unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].start.parameters[0], 7.0);
        let g = l.global_point_solve_step::<u8>(
            &l.step_interval(),
            SolverOperation::Maximize,
            Vec3::new(9.0, 0.0, 0.0),
            1e-6,
            None,
        );
        assert_eq!(g.len(), 2);
        let (t, loc) = l.step_inversion(Vec3::new(6.0, 0.0, 0.0));
        assert_eq!(t, 3.0);
        assert_eq!(loc, CurveLocationType::Interior);
    }

    #[test]
    fn line_line_intersection() {
        let a = Line::from_points(Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0), 3);
        let b = Line::from_points(Vec3::new(1.0, -1.0, 0.0), Vec3::new(1.0, 1.0, 0.0), 3);
        let r = a
            .intersect_with_line::<u8>(
                &a.natural_interval(),
                &b,
                &b.natural_interval(),
                1e-6,
                None,
                None,
            )
            .unwrap();
        assert_eq!(r.solutions.len(), 1);
        assert!((r.solutions[0].start.parameters[0] - 0.5).abs() < 1e-12);
        assert!(!r.needs_more_intersections);
    }

    #[test]
    fn trim_and_reverse() {
        let mut l = seg();
        l.trim(&Extent1d::from_min_max(3.0, 5.0), TrimMode::Notify)
            .unwrap();
        assert_eq!(l.step_interval(), Extent1d::from_min_max(3.0, 5.0));
        assert_eq!(l.natural_interval(), Extent1d::from_min_max(3.0, 5.0));
        let n = l.reverse_parameterization(&Extent1d::from_min_max(3.0, 5.0));
        assert_eq!(n, Extent1d::from_min_max(3.0, 5.0));
        assert_eq!(l.line_vector(), Vec3::new(-1.0, 0.0, 0.0));
    }

    #[test]
    fn segment_requires_distinct_points() {
        assert!(Line::create_line_segment(3, Vec3::ZERO, Vec3::ZERO, None).is_err());
        let s = Line::create_line_segment(
            3,
            Vec3::ZERO,
            Vec3::X,
            Some(&Extent1d::from_min_max(0.0, 4.0)),
        )
        .unwrap();
        assert_eq!(s.natural_interval(), Extent1d::from_min_max(0.0, 4.0));
        assert_eq!(s.step_interval(), Extent1d::from_min_max(0.0, 1.0));
    }
}
