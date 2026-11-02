//! Parabolas: the analytic curve `O + F (s² X + 2s Y)` over a STEP parameter
//! interval, carried as a B-spline curve whose quadratic NURB is rebuilt from the
//! analytic data.
//!
//! Upstream: `source/SMLib/inc/SmParabola.h`, `source/SMLib/src/SmParabola.cpp`.
//!
//! | Rust                                         | Upstream                                         |
//! |----------------------------------------------|--------------------------------------------------|
//! | [`Parabola`]                                 | `SmParabola`                                     |
//! | [`Parabola::new`]                            | `SmParabola(crCenter, crXAxis, crYAxis, crAnalDomain, dFocalDist, lDimension, cpContext)` |
//! | `Clone`, [`Parabola::copy`]                  | copy constructor, `Copy`                         |
//! | `Drop` (implicit)                            | `~SmParabola`                                    |
//! | [`Parabola::approx_eq`]                      | `operator==`                                     |
//! | [`Parabola::adjust_step_interval`]           | `AdjustSTEPInterval`                             |
//! | [`Parabola::create_canonical`]               | `CreateCanonical`                                |
//! | [`Parabola::canonical`]                      | `GetCanonical`                                   |
//! | [`Parabola::evaluate_step`]                  | `EvaluateSTEP`                                   |
//! | `evaluate_step_point` (private)              | `SmCurve::EvaluateSTEPPoint` (co-ported)         |
//! | [`Parabola::global_point_solve_step`]        | `GlobalPointSolveSTEP`                           |
//! | [`Parabola::step_interval`]                  | `GetSTEPInterval`                                |
//! | [`Parabola::max_analytic_domain`]            | `GetMaxAnalyticDomain`                           |
//! | [`Parabola::is_analytic`]                    | `IsAnalytic`                                     |
//! | [`Parabola::is_arc`]                         | `IsArc`                                          |
//! | [`Parabola::is_bounded`]                     | `IsBounded`                                      |
//! | [`Parabola::make_nurb`]                      | `MakeNurb`                                       |
//! | [`Parabola::reverse_parameterization`]       | `ReverseParameterization`                        |
//! | [`Parabola::step_inversion`]                 | `STEPInversion`                                  |
//! | [`Parabola::transform`]                      | `Transform`                                      |
//! | [`Parabola::read_from_db`]                   | `ReadFromDB`                                     |
//! | [`Parabola::TYPE`], [`Parabola::type_code`], [`Parabola::type_string`], [`Parabola::class_string`] | `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`Parabola::is_kind_of`]                     | `IsKindOf`                                       |
//! | [`CurveLocationType`]                        | `SmCurveLocationType` (`SmCurveTypes.h`)         |
//!
//! Deviations:
//! - **B-spline base.** `SmParabola` derives from `SmBSplineCurve`; the base object is
//!   the [`BSplineCurve`] field ([`Parabola::base`]), and as a
//!   [`crate::curve::curve::Curve`] kind the methods the class does not override
//!   (evaluation, knots, ends, trimming, ...) run on it. The `Notify` calls only drop
//!   caches and notify owners, which a curve does not hold here (see
//!   `nurbs::b_spline_curve::p1`), and are omitted.
//! - **Owner.** `MakeNurb` asks the curve's owner (`m_pOwner`) whether it is a
//!   surface of extrusion or revolution; the owner lives in [`crate::curve::curve::Curve`],
//!   so the methods that make the NURB take it as an argument. An owner of such a
//!   type (`IsKindOf` by type code: `SmSurfOfRevolution` includes the cone,
//!   cylinder, sphere and torus) needs `GetGenDirParamExtent`, which returns
//!   [`SolidError::Unported`] (units `surface.surf_of_extrusion_p1`,
//!   `surface.surf_of_revolution_p1`) after the NURB is built.
//! - **Lazy NURB.** Upstream `ReverseParameterization` frees the NURB and the next
//!   `GetOrCreateGwNurbPointer` rebuilds it with `MakeNurb`; the port rebuilds it at
//!   once (same data, same NURB). Only a `Transform` between the two would differ
//!   (upstream rebuilds from the transformed placement, the port transforms the
//!   control points), at rounding level. `ReadFromDB` ignores the status of the base
//!   class read; when that read fails the curve has no NURB upstream (rebuilt on
//!   first use), so the port makes it there.
//! - **Arguments.** Out-params become return values; the context arguments and the
//!   arguments upstream marks `NotUsed` (`EvaluateSTEP`'s `bFromLeft` and zone
//!   tolerance, `STEPInversion`'s zone tolerance, `GlobalPointSolveSTEP`'s vectors,
//!   `ReadFromDB`'s type) are dropped. `STEPInversion`'s optional classification is
//!   a flag. `IsArc` returns only its `FALSE`.
//! - **Statuses ignored upstream** (`SetCanonical`, `SetMinMax`, the constructor's
//!   `MakeNurb`, `SetFromGwNurb` and `ConvertTo2D` inside `MakeNurb`) are ignored,
//!   except [`SolidError::Unported`].
//! - **Back-edges.** `GlobalPointSolveSTEP` runs `SmCurve::GlobalPointSolve`
//!   ([`Curve::global_point_solve`], unit `curve.curve_p1`) on the parabola as a
//!   [`Curve`]; its solutions name no object. The one-line
//!   `SmCurve::EvaluateSTEPPoint` (unit `curve.curve_p5`) is co-ported as the private
//!   `evaluate_step_point`: `EvaluateSTEP` with no derivatives.
//! - `GetMemoryUsed`, `WriteToDB` and `Dump` are not part of this unit.

use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p2::PARABOLA_TYPE;
use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::geom_utility::p1::{line_line_closest_point, point_project_to_plane};
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object::ObjectRef;
use crate::geom::solution_array::{ObjectKind, Solution, SolutionObject};
use crate::geom::tol::{are_same, is_zero};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, INFINITE_PARAMETER_SQRT};
use crate::nurbs::b_spline_curve::{BSplineCurve, BSplineCurveForm, CanonicalData, KnotType};
use crate::topology::database_io::{DatabaseReader, FileType};

/// `SmSurfOfRevolution_TYPE`.
const SURF_OF_REVOLUTION_TYPE: i64 = 15020;
/// `SmCone_TYPE` (derives from `SmSurfOfRevolution`).
const CONE_TYPE: i64 = 15012;
/// `SmCylinder_TYPE` (derives from `SmCone`).
const CYLINDER_TYPE: i64 = 15013;
/// `SmSphere_TYPE` (derives from `SmSurfOfRevolution`).
const SPHERE_TYPE: i64 = 15014;
/// `SmTorus_TYPE` (derives from `SmSurfOfRevolution`).
const TORUS_TYPE: i64 = 15015;
/// `SmSurfOfExtrusion_TYPE`.
const SURF_OF_EXTRUSION_TYPE: i64 = 15030;

const OP_EVAL: &str = "curve::parabola::Parabola::evaluate_step";
const OP_XFORM: &str = "curve::parabola::Parabola::transform";

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

/// A parabola (`SmParabola`):
///
/// ```text
/// C(s) = O + F s² X + 2 F s Y,    s in the analytic (STEP) domain
/// ```
///
/// with origin `O`, unit axes `X`, `Y` (the placement) and focal distance `F`. The
/// B-spline base holds the quadratic NURB over the same parameter range.
#[derive(Debug, Clone)]
pub struct Parabola {
    /// The `SmBSplineCurve` base.
    base: BSplineCurve,
    /// `m_vPosition`.
    position: Axis2Placement,
    /// `m_vAnalDomain`: the STEP parameter interval.
    anal_domain: Extent1d,
    /// `m_dFocalDist`.
    focal_dist: f64,
}

impl Parabola {
    /// Exact evaluation copy without the B-spline copy constructor's knot fix-up.
    pub(crate) fn clone_exact_bounded(&self) -> Self {
        Self {
            base: self.base.clone_exact(),
            position: self.position,
            anal_domain: self.anal_domain,
            focal_dist: self.focal_dist,
        }
    }
}

/// `[max(min, -SM_INFINITE_PARAMETER_SQRT), min(max, SM_INFINITE_PARAMETER_SQRT)]`
/// (`SetMinMax`, whose status is ignored).
fn trim_infinite(domain: &mut Extent1d) {
    // Trim infinite domains as needed
    let min = if domain.min() < -INFINITE_PARAMETER_SQRT {
        -INFINITE_PARAMETER_SQRT
    } else {
        domain.min()
    };
    let max = if domain.max() > INFINITE_PARAMETER_SQRT {
        INFINITE_PARAMETER_SQRT
    } else {
        domain.max()
    };
    let _ = domain.set_min_max(min, max);
}

/// `Ok` for any result but [`SolidError::Unported`] (a status upstream ignores).
fn ignore_status<T>(r: Result<T>) -> Result<()> {
    match r {
        Err(e @ SolidError::Unported { .. }) => Err(e),
        _ => Ok(()),
    }
}

/// The handle of the parabola in its own global solve: a curve with the given
/// natural interval (the solver's duplicate test reads it). The solutions returned
/// name no object.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SelfHandle(Extent1d);

impl SolutionObject for SelfHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.0)
    }
}

impl Parabola {
    /// `SmParabola_TYPE` (`CURV_BASE_TYPE + 23`).
    pub const TYPE: i64 = PARABOLA_TYPE;

    // ---- construction -----------------------------------------------------------------

    /// The parabola with origin `center`, axes `x_axis`, `y_axis` (made orthonormal by
    /// `SetCanonical`), STEP interval `anal_domain` (clipped to
    /// `±SM_INFINITE_PARAMETER_SQRT`) and focal distance `focal_dist`, in image
    /// dimension `dim`; its NURB is made at once (`MakeNurb`, status ignored) and its
    /// form is `SM_CF_PARABOLIC_ARC`.
    #[must_use]
    pub fn new(
        center: Point3,
        x_axis: Vec3,
        y_axis: Vec3,
        anal_domain: Extent1d,
        focal_dist: f64,
        dim: u64,
    ) -> Self {
        let mut position = Axis2Placement::default();
        // set m_vPosition
        let _ = position.set_canonical(center, x_axis, y_axis);

        let mut anal_domain = anal_domain;
        trim_infinite(&mut anal_domain);

        let mut parabola = Self {
            base: BSplineCurve::new(dim),
            position,
            anal_domain,
            focal_dist,
        };
        // No owner yet: MakeNurb cannot reach an unported callee.
        let _ = parabola.make_nurb(None);
        parabola.base.form = BSplineCurveForm::ParabolicArc;
        parabola
    }

    /// The 3D parabola at placement `origin` with focal distance `focal_dist` over
    /// `[-SM_INFINITE_PARAMETER_SQRT, SM_INFINITE_PARAMETER_SQRT]` (`CreateCanonical`).
    #[must_use]
    pub fn create_canonical(origin: &Axis2Placement, focal_dist: f64) -> Self {
        Self::new(
            origin.origin(),
            origin.x_axis(),
            origin.y_axis(),
            Extent1d::from_min_max(-INFINITE_PARAMETER_SQRT, INFINITE_PARAMETER_SQRT),
            focal_dist,
            3,
        )
    }

    /// A copy (`Copy`, the copy constructor): the B-spline base is copied by its copy
    /// constructor.
    #[must_use]
    pub fn copy(&self) -> Self {
        self.clone()
    }

    // ---- access -----------------------------------------------------------------------

    /// The `SmBSplineCurve` base.
    #[must_use]
    pub const fn base(&self) -> &BSplineCurve {
        &self.base
    }

    /// The `SmBSplineCurve` base, mutable (for the base-class edits).
    pub(crate) fn base_mut(&mut self) -> &mut BSplineCurve {
        &mut self.base
    }

    /// The placement and the focal distance (`GetCanonical`).
    #[must_use]
    pub const fn canonical(&self) -> (Axis2Placement, f64) {
        (self.position, self.focal_dist)
    }

    /// The placement (`m_vPosition`).
    #[must_use]
    pub const fn position(&self) -> &Axis2Placement {
        &self.position
    }

    /// The focal distance (`m_dFocalDist`).
    #[must_use]
    pub const fn focal_dist(&self) -> f64 {
        self.focal_dist
    }

    /// Image-space dimension (`SmCurve::GetDim`).
    #[must_use]
    pub const fn dim(&self) -> u64 {
        self.base.dim()
    }

    /// The STEP parameter interval (`GetSTEPInterval`, `m_vAnalDomain`).
    #[must_use]
    pub const fn step_interval(&self) -> Extent1d {
        self.anal_domain
    }

    /// `[-100, 100]` (`GetMaxAnalyticDomain`).
    #[must_use]
    pub fn max_analytic_domain(&self) -> Extent1d {
        Extent1d::from_min_max(-100.0, 100.0)
    }

    /// Always `true` (`IsAnalytic`).
    #[must_use]
    pub const fn is_analytic(&self) -> bool {
        true
    }

    /// Always `false`: a parabola is not a circular arc (`IsArc`).
    #[must_use]
    pub const fn is_arc(&self) -> bool {
        false
    }

    /// Whether the STEP interval is finite (`IsBounded`).
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        self.anal_domain.is_bounded()
    }

    /// `SmParabola_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmParabola_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmParabola_TYPE"
    }

    /// `"SmParabola"` (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        "SmParabola"
    }

    /// True for `SmParabola_TYPE` and its base types (`IsKindOf`).
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == Self::TYPE || t == BSplineCurve::TYPE || crate::curve::curve::p2::is_kind_of(t)
    }

    /// `operator==`: the B-spline bases are equal (`SmBSplineCurve::operator==`), and
    /// so are the placements, the STEP intervals (`SM_ARE_SAME` ends) and the focal
    /// distances (`SM_IS_ZERO` of the difference).
    #[must_use]
    pub fn approx_eq(&self, other: &Self) -> bool {
        // low work
        if core::ptr::eq(self, other) {
            return true;
        }

        // first check the base, then the members
        self.base.approx_eq(&other.base)
            && self.position.approx_eq(&other.position)
            && self.anal_domain.are_same(&other.anal_domain)
            && is_zero(self.focal_dist - other.focal_dist)
    }

    // ---- evaluation -------------------------------------------------------------------

    /// Position and derivatives `0..=num_derivs` at STEP parameter `t`, clamped to the
    /// STEP interval (`EvaluateSTEP`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for more than 3 derivatives.
    pub fn evaluate_step(&self, t: f64, num_derivs: usize) -> Result<Vec<Vec3>> {
        if num_derivs > 3 {
            return Err(SolidError::sm_err(OP_EVAL));
        }
        let mut t = t;
        if !self.anal_domain.contains_value(t, 0.0) {
            t = self.anal_domain.clamp_value(t);
        }
        let origin = self.position.origin();
        let x_axis = self.position.x_axis();
        let y_axis = self.position.y_axis();
        let f = self.focal_dist;

        // evaluate position
        let mut out = Vec::with_capacity(num_derivs + 1);
        out.push(origin + f * t * t * x_axis + f * 2.0 * t * y_axis);

        // 1st derivative
        if num_derivs > 0 {
            out.push(2.0 * f * t * x_axis + 2.0 * f * y_axis);
        }

        // 2nd derivative
        if num_derivs > 1 {
            out.push(2.0 * f * x_axis);
        }

        // 3rd derivative
        if num_derivs > 2 {
            out.push(Vec3::new(0.0, 0.0, 0.0));
        }
        Ok(out)
    }

    /// The point at STEP parameter `t` (`SmCurve::EvaluateSTEPPoint`).
    fn evaluate_step_point(&self, t: f64) -> Result<Point3> {
        let pd = self.evaluate_step(t, 0)?;
        Ok(pd.first().copied().unwrap_or(Vec3::UNDEF))
    }

    /// The STEP parameter of `point`, read off its `Y` coordinate in the placement
    /// (`STEPInversion`; `point` must lie on the parabola). With `classify`, also
    /// whether the point is on the curve: interior when the parameter is inside the
    /// STEP interval and the curve point there is within `SM_EFF_ZERO * (1 + size)`
    /// of `point`, else exterior.
    #[must_use]
    pub fn step_inversion(
        &self,
        point: Point3,
        classify: bool,
    ) -> (f64, Option<CurveLocationType>) {
        let center = self.position.origin();
        let y_axis = self.position.y_axis();

        // First, find its local coordinates with respect to axes of ellipse
        let vec = point - center;
        let y = vec.dot(y_axis);
        let param = y / 2.0 / self.focal_dist;

        // when asked - classify the point
        if !classify {
            return (param, None);
        }
        // check that point maps to inside of line
        let mut loc = if self.anal_domain.contains_value(param, 0.0) {
            CurveLocationType::Interior
        } else {
            CurveLocationType::Exterior
        };

        // check distance to point
        if loc == CurveLocationType::Interior {
            // The status is ignored upstream (EvaluateSTEP cannot fail for no derivatives).
            let curve_point = self.evaluate_step_point(param).unwrap_or(Vec3::UNDEF);
            let dist = curve_point.distance(point);
            let scaled_zero = EFF_ZERO * (1.0 + curve_point.max_dimension());

            // when dist is too large - point is not on the line
            if dist > scaled_zero {
                loc = CurveLocationType::Exterior;
            }
        }
        (param, Some(loc))
    }

    /// Extrema of the distance from `test_point` to the parabola over the STEP
    /// interval `interval` (`GlobalPointSolveSTEP`): the curve is first adjusted to
    /// `interval` ([`Parabola::adjust_step_interval`]); the solutions carry STEP
    /// parameters.
    ///
    /// # Errors
    ///
    /// Those of [`Parabola::adjust_step_interval`], of `SmCurve::GlobalPointSolve`
    /// ([`Curve::global_point_solve`]) and of the evaluations.
    #[allow(clippy::too_many_arguments)]
    pub fn global_point_solve_step(
        &mut self,
        k: &Kernel,
        owner: Option<ObjectRef>,
        interval: &Extent1d,
        operation: SolverOperation,
        test_point: Point3,
        tol: f64,
        target_distance: Option<f64>,
        requested: SolutionRequested,
    ) -> Result<Vec<Solution<ObjectRef>>> {
        self.adjust_step_interval(interval, owner)?;

        let center = self.position.origin();
        let z_axis = self.position.z_axis();

        // Project the point onto the plane which Ellipse is on (unused upstream)
        let _proj_point = point_project_to_plane(test_point, center, z_axis);

        let curve = Curve::from(self.clone());
        let ivl = curve.natural_interval();
        let opts = GlobalPointSolveOptions {
            target_distance,
            handle: Some(SelfHandle(ivl)),
            ..GlobalPointSolveOptions::default()
        };
        let sols =
            curve.global_point_solve(k, &ivl, operation, test_point, tol, requested, &opts)?;

        let mut out = Vec::with_capacity(sols.len());
        for sol in sols.as_slice() {
            let point_on_curve = curve.evaluate_point(k, sol.start.parameters[0])?;
            let (angle_param, _) = self.step_inversion(point_on_curve, false);
            let mut r: Solution<ObjectRef> = Solution::new();
            r.solution_type = sol.solution_type;
            r.num_variables = sol.num_variables;
            r.start = sol.start;
            r.end = sol.end;
            r.num_objects = sol.num_objects; // (objects: this curve, not named here)
            r.nodes = sol.nodes;
            r.start.parameters[0] = angle_param;
            out.push(r);
        }
        Ok(out)
    }

    // ---- editing ----------------------------------------------------------------------

    /// Makes the NURB from the analytic data (`MakeNurb`): the quadratic B-spline over
    /// the STEP interval through the end points, its middle control point where the
    /// end tangent lines meet; made 2D for a 2D curve. When `owner` (the curve's
    /// owner) is a surface of extrusion or revolution, its generating-direction
    /// interval would become the NURB's parameterisation.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the end tangents are parallel or zero (the curve then has no
    /// NURB); those of [`BSplineCurve::create_canonical`]; [`SolidError::Unported`]
    /// for a surface owner.
    pub fn make_nurb(&mut self, owner: Option<ObjectRef>) -> Result<()> {
        // remove existing parabola Nurb Representation
        self.base.nurb = None;

        // get start and end point and tangent values
        let t1 = self.anal_domain.min();
        let t2 = self.anal_domain.max();
        let start_pv = self.evaluate_step(t1, 1)?;
        let end_pv = self.evaluate_step(t2, 1)?;
        let at = |v: &[Vec3], i: usize| v.get(i).copied().unwrap_or(Vec3::UNDEF);
        let (s0, s1, e0, e1) = (
            at(&start_pv, 0),
            at(&start_pv, 1),
            at(&end_pv, 0),
            at(&end_pv, 1),
        );

        // place control point at endPointTangent Line intersections
        let (x1, _x2) = line_line_closest_point(s0, s1, e0, e1)?;
        let mid_ctrl_pnt = s0 + x1 * s1;

        // Create a canonical 3 ControlPoint Bspline
        let cntrl_poly = [s0, mid_ctrl_pnt, e0];
        let knots = [t1, t2];
        let knot_mult = [3usize, 3];

        // build temporary SmBSplineCurve with appropriate Nurb objecct
        let tmp = BSplineCurve::create_canonical(&CanonicalData {
            dim: 3,
            degree: 2,
            control_points: &cntrl_poly,
            form: BSplineCurveForm::Unspecified,
            knot_multiplicities: &knot_mult,
            knots: &knots,
            knot_type: KnotType::Unspecified,
            weights: None,
            trim: None,
        })?;

        // Place a copy of pTmpBSC->m_pNurb into this Curve's m_pNurb
        ignore_status(self.base.set_from_gw_nurb(tmp.gw_nurb()))?;

        // Set ControlPoint Z values for 2d curves
        if self.base.dim() == 2 {
            ignore_status(self.base.convert_to_2d())?;
        }

        if let Some(o) = owner {
            // when a SmSurfOfExtrusion GenCurve, look for Parent Surface NURB Interval constraint
            if o.type_code == SURF_OF_EXTRUSION_TYPE {
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
            if matches!(
                o.type_code,
                SURF_OF_REVOLUTION_TYPE | CONE_TYPE | CYLINDER_TYPE | SPHERE_TYPE | TORUS_TYPE
            ) {
                // DISPATCH GAP: SmSurfOfRevolution::GetGenDirParamExtent (surface.surf_of_revolution_p1,
                // ported as SurfOfRevolution::gen_dir_param_extent; no UNPORTED marker: the owner is an
                // opaque ObjectRef. An owning revolution does this step itself (e.g. the cone's
                // adjust_gen_line_with_owner) and never sets its generator's owner, so nothing reaches here)
                return Err(SolidError::Unported {
                    upstream: "SmSurfOfRevolution::GetGenDirParamExtent",
                    unit: "surface.surf_of_revolution_p1",
                });
            }
        }

        // all done
        Ok(())
    }

    /// Selects another section of the parabola (`AdjustSTEPInterval`): the STEP
    /// interval becomes `new_interval` (clipped to `±SM_INFINITE_PARAMETER_SQRT`) and
    /// the NURB is rebuilt ([`Parabola::make_nurb`] with the curve's `owner`).
    /// Nothing changes when the intervals are equal.
    ///
    /// # Errors
    ///
    /// Those of [`Parabola::make_nurb`] (the curve then has no NURB).
    pub fn adjust_step_interval(
        &mut self,
        new_interval: &Extent1d,
        owner: Option<ObjectRef>,
    ) -> Result<()> {
        // low work - New == Old interval
        if new_interval.is_contained_by(&self.anal_domain, 0.0)
            && self.anal_domain.is_contained_by(new_interval, 0.0)
        {
            return Ok(());
        }

        // make the assignment
        self.anal_domain = *new_interval;

        // Trim infinite domains as needed
        trim_infinite(&mut self.anal_domain);

        // rebuild the nurb
        self.base.nurb = None;
        self.make_nurb(owner)
    }

    /// Reverses the parameterisation (`ReverseParameterization`): the STEP interval
    /// `[a, b]` becomes `[-b, -a]` and the `Y` axis is negated, so the curve keeps its
    /// shape; returns the reversed `old_interval` (`[-max, -min]`; left as
    /// `SmExtent1d()` when that is inverted). The NURB is rebuilt at once
    /// ([`Parabola::make_nurb`] with the curve's `owner`; upstream rebuilds it on
    /// first use).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from [`Parabola::make_nurb`] (its other failures leave
    /// the curve without NURB, as upstream).
    pub fn reverse_parameterization(
        &mut self,
        old_interval: &Extent1d,
        owner: Option<ObjectRef>,
    ) -> Result<Extent1d> {
        self.base.nurb = None;

        let mut new_interval = Extent1d::new();
        let _ = new_interval.set_min_max(-old_interval.max(), -old_interval.min());
        let (min, max) = (self.anal_domain.min(), self.anal_domain.max());
        let _ = self.anal_domain.set_min_max(-max, -min);

        let neg_y_axis = -self.position.y_axis();
        let (origin, x_axis) = (self.position.origin(), self.position.x_axis());
        let _ = self.position.set_canonical(origin, x_axis, neg_y_axis);

        // upstream: GetOrCreateGwNurbPointer() makes it on first use
        ignore_status(self.make_nurb(owner))?;
        Ok(new_interval)
    }

    /// Scales the parabola uniformly by `scale` about the global origin (optional),
    /// then rotates and moves it by `rotate_n_move` (`Transform`): the placement and
    /// focal distance follow, and the NURB (when present) is transformed by
    /// [`BSplineCurve::transform`]. The identity placement with no or unit scale
    /// changes nothing.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a non-uniform scale; those of `SetCanonical` (a zero axis) and of
    /// [`BSplineCurve::transform`].
    pub fn transform(&mut self, rotate_n_move: &Axis2Placement, scale: Option<Vec3>) -> Result<()> {
        let identity_scale = Vec3::new(1.0, 1.0, 1.0);

        // no work - identity transform
        if rotate_n_move.is_identity(EFF_ZERO) && scale.is_none_or(|s| s.approx_eq(identity_scale))
        {
            return Ok(());
        }

        if let Some(s) = scale {
            let unit_scale = s.x == 1.0 && s.y == 1.0 && s.z == 1.0;
            if !unit_scale && (!are_same(s.x, s.y) || !are_same(s.x, s.z)) {
                // Unable to scale analytical curves non-uniformly
                return Err(SolidError::sm_err(OP_XFORM));
            }
        }

        // Uniform scaling
        let scale_factor = scale.map_or(1.0, |s| s.x);

        let mut place = Axis2Placement::default();
        place.set_canonical(
            self.position.origin() * scale_factor,
            self.position.x_axis(),
            self.position.y_axis(),
        )?;
        self.position = place.transform_axis2_placement(rotate_n_move);

        self.focal_dist *= scale_factor;

        if self.base.nurb.is_some() {
            self.base.transform(rotate_n_move, scale)?;
        }
        Ok(())
    }

    // ---- persistence ------------------------------------------------------------------

    /// Reads a parabola written by `WriteToDB` (`ReadFromDB`): origin, `X` and `Y`
    /// axes, STEP interval and focal distance (one line each in ASCII files), then
    /// the B-spline base ([`BSplineCurve::read_from_db`] with `dim` and `version`).
    /// The axes go through `SetCanonical`; an inverted interval leaves `SmExtent1d()`.
    /// ASCII values whose extraction is skipped keep their initial values (the
    /// `SmVector3d` default [`Vec3::UNDEF`] for the points, `0` for the numbers).
    /// The status of the base read is ignored, as upstream: on failure the base is the
    /// empty 3D B-spline of `SmParabola()` and the NURB is made from the analytic data.
    ///
    /// # Errors
    ///
    /// A failed binary read of the parabola's own values.
    pub fn read_from_db(db: &mut DatabaseReader, dim: u64, version: u64) -> Result<Self> {
        // locals
        let mut orig = Vec3::UNDEF;
        let mut x = Vec3::UNDEF;
        let mut y = Vec3::UNDEF;
        let mut anal_min = 0.0;
        let mut anal_max = 0.0;
        let mut focal_dist = 0.0;

        if db.file_type() == FileType::Ascii {
            for v in [&mut orig, &mut x, &mut y] {
                if let Some(s) = db.in_stream_mut() {
                    for coord in [&mut v.x, &mut v.y, &mut v.z] {
                        if let Some(c) = s.extract_double() {
                            *coord = c;
                        }
                    }
                }
                db.go_to_next_line();
            }
            if let Some(s) = db.in_stream_mut() {
                for value in [&mut anal_min, &mut anal_max] {
                    if let Some(c) = s.extract_double() {
                        *value = c;
                    }
                }
            }
            db.go_to_next_line();
            if let Some(s) = db.in_stream_mut() {
                if let Some(c) = s.extract_double() {
                    focal_dist = c;
                }
            }
            db.go_to_next_line();
        } else {
            for v in [&mut orig, &mut x, &mut y] {
                v.x = db.read_double()?;
                v.y = db.read_double()?;
                v.z = db.read_double()?;
            }
            anal_min = db.read_double()?;
            anal_max = db.read_double()?;
            focal_dist = db.read_double()?;
        }

        // load the obj
        let mut position = Axis2Placement::default();
        let _ = position.set_canonical(orig, x, y);
        let mut anal_domain = Extent1d::new();
        let _ = anal_domain.set_min_max(anal_min, anal_max);

        // read the base type (status ignored upstream)
        let (base, read_ok) = match BSplineCurve::read_from_db(db, dim, version) {
            Ok(b) => (b, true),
            Err(e @ SolidError::Unported { .. }) => return Err(e),
            Err(_) => (BSplineCurve::default(), false),
        };
        let mut parabola = Self {
            base,
            position,
            anal_domain,
            focal_dist,
        };
        if !read_ok {
            // upstream: GetOrCreateGwNurbPointer() makes it on first use
            ignore_status(parabola.make_nurb(None))?;
        }
        Ok(parabola)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Parabola {
        Parabola::new(
            Point3::new(1.0, 2.0, 3.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Extent1d::from_min_max(-2.0, 3.0),
            0.5,
            3,
        )
    }

    #[test]
    fn evaluate_matches_nurb() {
        let p = sample();
        for t in [-2.0, -1.0, 0.0, 0.5, 2.0, 3.0] {
            let a = p.evaluate_step(t, 0).unwrap()[0];
            let b = p.base().evaluate_point(t).unwrap();
            assert!(a.distance(b) < 1e-12, "{t}: {a:?} {b:?}");
        }
        assert_eq!(p.base().form(), BSplineCurveForm::ParabolicArc);
        assert!(p.evaluate_step(0.0, 4).is_err());
        let d = p.evaluate_step(1.0, 3).unwrap();
        assert_eq!(d.len(), 4);
        assert_eq!(d[2], Vec3::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn inversion() {
        let p = sample();
        let pt = p.evaluate_step(1.5, 0).unwrap()[0];
        let (t, loc) = p.step_inversion(pt, true);
        assert!((t - 1.5).abs() < 1e-15);
        assert_eq!(loc, Some(CurveLocationType::Interior));
        let (_, loc) = p.step_inversion(pt + Vec3::new(0.0, 0.0, 1.0), true);
        assert_eq!(loc, Some(CurveLocationType::Exterior));
        let (_, loc) = p.step_inversion(p.evaluate_step(3.0, 0).unwrap()[0] * 2.0, true);
        assert_eq!(loc, Some(CurveLocationType::Exterior));
    }

    #[test]
    fn reverse_keeps_shape() {
        let mut p = sample();
        let before = p.evaluate_step(1.0, 0).unwrap()[0];
        let ivl = p
            .reverse_parameterization(&Extent1d::from_min_max(-2.0, 3.0), None)
            .unwrap();
        assert_eq!((ivl.min(), ivl.max()), (-3.0, 2.0));
        let after = p.evaluate_step(-1.0, 0).unwrap()[0];
        assert!(before.distance(after) < 1e-15);
        assert!(p.base().gw_nurb().is_some());
    }

    #[test]
    fn owner_back_edges() {
        use crate::geom::attribute::ObjectKey;
        let mut p = sample();
        let owner = ObjectRef {
            type_code: CYLINDER_TYPE,
            key: ObjectKey(1),
        };
        assert!(matches!(
            p.make_nurb(Some(owner)),
            Err(SolidError::Unported { .. })
        ));
        assert!(p.base().gw_nurb().is_some());
    }

    #[test]
    fn types_and_eq() {
        let p = sample();
        assert!(p.is_kind_of(Parabola::TYPE));
        assert!(p.is_kind_of(BSplineCurve::TYPE));
        assert!(!p.is_kind_of(14024));
        assert!(p.approx_eq(&p.copy()));
        assert!(p.is_bounded());
        let c = Parabola::create_canonical(&Axis2Placement::default(), 1.0);
        assert!(!c.is_bounded());
        assert!(!p.approx_eq(&c));
    }

    #[test]
    fn transform_rejects_nonuniform() {
        let mut p = sample();
        let r = p.transform(&Axis2Placement::default(), Some(Vec3::new(1.0, 2.0, 1.0)));
        assert!(r.is_err());
        p.transform(&Axis2Placement::default(), Some(Vec3::new(2.0, 2.0, 2.0)))
            .unwrap();
        assert_eq!(p.focal_dist(), 1.0);
    }
}
