//! Hyperbolic arcs (`SmHyperbola`): `C(s) = O + a cosh(s) X + b sinh(s) Y` over a
//! bounded analytic (hyperbolic radian) domain, kept together with the exact rational
//! quadratic B-spline the curve evaluates through.
//!
//! Upstream: `source/SMLib/src/SmHyperbola.cpp`, `source/SMLib/inc/SmHyperbola.h`.
//!
//! | Rust                                         | Upstream                                       |
//! |----------------------------------------------|------------------------------------------------|
//! | [`Hyperbola`]                                | `SmHyperbola`                                  |
//! | [`Hyperbola::new`]                           | `SmHyperbola(center, X, Y, domain, a, b, dim, ctx)` |
//! | [`Hyperbola::empty`]                         | `SmHyperbola()`                                |
//! | `Clone`, [`Hyperbola::copy`]                 | copy constructor, `Copy`                       |
//! | `Drop` (implicit)                            | `~SmHyperbola`                                 |
//! | [`Hyperbola::approx_eq`]                     | `operator==`                                   |
//! | [`Hyperbola::adjust_step_interval`]          | `AdjustSTEPInterval`                           |
//! | [`Hyperbola::step_inversion`], [`CurveLocation`] | `STEPInversion`, `SmCurveLocationType`     |
//! | [`Hyperbola::evaluate_step`]                 | `EvaluateSTEP`                                 |
//! | [`Hyperbola::global_point_solve_step`]       | `GlobalPointSolveSTEP`                         |
//! | [`Hyperbola::step_interval`]                 | `GetSTEPInterval`                              |
//! | [`Hyperbola::max_analytic_domain`]           | `GetMaxAnalyticDomain`                         |
//! | [`Hyperbola::is_analytic`], [`Hyperbola::is_arc`], [`Hyperbola::is_bounded`] | `IsAnalytic`, `IsArc`, `IsBounded` |
//! | [`Hyperbola::make_nurb`]                     | `MakeNurb`                                     |
//! | [`Hyperbola::reverse_parameterization`]      | `ReverseParameterization`                      |
//! | [`Hyperbola::transform`]                     | `Transform`                                    |
//! | [`Hyperbola::read_from_db`]                  | `ReadFromDB`                                   |
//! | [`Hyperbola::is_kind_of`]                    | `IsKindOf`                                     |
//! | [`Hyperbola::TYPE`], [`Hyperbola::type_code`], [`Hyperbola::type_string`], [`Hyperbola::class_string`] | `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//!
//! Deviations:
//! - **Derived class.** `SmHyperbola` derives from `SmBSplineCurve`; the base object
//!   is the [`Hyperbola::base`] field and the curve joins [`Curve`] as
//!   [`CurveKind::Hyperbola`], whose dispatch runs the overrides here and the
//!   B-spline methods on the base otherwise.
//! - **Lazy NURB.** Upstream frees the NURB in `ReverseParameterization` and lets the
//!   next `GetOrCreateGwNurbPointer` call the virtual `MakeNurb`. The B-spline base
//!   here cannot reach the override, so [`Hyperbola::reverse_parameterization`] makes
//!   the NURB again before returning (the same NURB the lazy call would make; a
//!   failure leaves no NURB, as upstream's failed lazy rebuild).
//! - **Owner.** `MakeNurb` asks the owner (`m_pOwner`) whether it is an
//!   `SmSurfOfExtrusion` or `SmSurfOfRevolution`; the owner lives in the [`Curve`]
//!   wrapper, so the methods that make the NURB take it as an argument. Reading the
//!   owner surface's generator parameter range needs units
//!   `surface.surf_of_extrusion_p1` / `surface.surf_of_revolution_p1` and returns
//!   [`SolidError::Unported`].
//! - **Notifications.** As for the B-spline base, a curve has no caches or owner
//!   links to notify here; the `SM_NO_PRE_EDIT` / `SM_NO_POST_EDIT` calls are omitted.
//! - **Context.** The `SmContext` arguments only choose where upstream allocates and
//!   are dropped; so are the arguments upstream marks `NotUsed` (`EvaluateSTEP`'s
//!   `bFromLeft` and zone tolerance, `STEPInversion`'s zone tolerance,
//!   `GlobalPointSolveSTEP`'s vectors, `ReadFromDB`'s type). Out-params become return
//!   values; `ReadFromDB` always builds a new curve (upstream can also fill an empty
//!   one passed in).
//! - **Statuses ignored upstream.** The constructor ignores `MakeNurb`'s status and
//!   `ReadFromDB` the status of the B-spline base reader (the curve then keeps the
//!   empty base of `SmHyperbola()`); here too, except [`SolidError::Unported`],
//!   which always propagates. `STEPInversion` ignores the status of
//!   `SmCurve::EvaluateSTEPPoint` (unit `curve.curve_p5`: `EvaluateSTEP` without
//!   derivatives; a failure leaves the point [`Vec3::UNDEF`]).
//! - **Global solve.** `GlobalPointSolveSTEP` runs `SmCurve::GlobalPointSolve` on a
//!   [`Curve`] made of a copy of the hyperbola (taking the [`Kernel`] for the
//!   curve cache); its solutions name no object.
//! - **Not ported here.** The constructor's out-of-range domain warning (a message
//!   only). `CreateCanonical`, `GetCanonical`, `GetMemoryUsed`, `WriteToDB` and
//!   `Dump` are unreachable from the in-scope entry points
//!   (`harness/callgraph/unused_functions.txt`); the field accessors
//!   ([`Hyperbola::position`], ...) give the canonical data.

use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p2::{self as curve_base, at, HYPERBOLA_TYPE};
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::assert_array::labels::{SURF_OF_EXTRUSION_TYPE, SURF_OF_REVOLUTION_TYPE};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::geom_utility::p1::{line_line_closest_point, point_project_to_plane};
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object::ObjectRef;
use crate::geom::solution_array::{ObjectKind, Solution, SolutionObject};
use crate::geom::tol::{are_same, is_zero};
use crate::geom::vector3d::{sqrt, Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::EFF_ZERO;
use crate::nurbs::b_spline_curve::{BSplineCurve, BSplineCurveForm, CanonicalData, KnotType};
use crate::topology::database_io::{DatabaseReader, FileType, InputStream};

const OP_MAKE_NURB: &str = "curve::hyperbola::Hyperbola::make_nurb";
const OP_EVAL_STEP: &str = "curve::hyperbola::Hyperbola::evaluate_step";
const OP_TRANSFORM: &str = "curve::hyperbola::Hyperbola::transform";

/// Where a point lies relative to a curve (`SmCurveLocationType`); discriminants are
/// the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CurveLocation {
    /// `SM_CL_SEAM`: on the seam of a closed curve.
    Seam = 0,
    /// `SM_CL_INTERIOR`: on the curve.
    Interior = 1,
    /// `SM_CL_EXTERIOR`: not on the curve.
    Exterior = 2,
    /// `SM_CL_UNKNOWN`.
    Unknown = 3,
}

/// A hyperbolic arc (`SmHyperbola`): `C(s) = O + a cosh(s) X + b sinh(s) Y` for `s`
/// in the analytic domain (hyperbolic radians, practically within `[-5, 5]`), with
/// the rational quadratic B-spline over the same domain as its base.
#[derive(Debug, Clone)]
pub struct Hyperbola {
    /// The `SmBSplineCurve` base object.
    pub(crate) base: BSplineCurve,
    /// `m_vPosition`: center `O`, semi axis direction `X`, semi imaginary axis
    /// direction `Y`.
    pub(crate) position: Axis2Placement,
    /// `m_vAnalDomain`: the analytic (STEP) domain.
    pub(crate) anal_domain: Extent1d,
    /// `m_dSemiAxis` (`a`).
    pub(crate) semi_axis: f64,
    /// `m_dSemiImageAxis` (`b`).
    pub(crate) semi_image_axis: f64,
}

impl From<Hyperbola> for Curve {
    fn from(c: Hyperbola) -> Self {
        Self::new(CurveKind::Hyperbola(c))
    }
}

/// The handle of the hyperbola in its own global solve: a curve with the given
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

impl Hyperbola {
    /// `SmHyperbola_TYPE` (`CURV_BASE_TYPE + 24`).
    pub const TYPE: i64 = HYPERBOLA_TYPE;

    /// A hyperbola centred at `center` with semi axis `semi_axis` along `x_axis`,
    /// semi imaginary axis `semi_image_axis` along `y_axis` (axes made orthonormal by
    /// `SetCanonical`, whose status is ignored) over `anal_domain`, of image
    /// dimension `dim` (constructor). The NURB is made at once (`MakeNurb`, status
    /// ignored: a failure leaves no NURB) and the form is `SM_CF_HYPERBOLIC_ARC`.
    #[must_use]
    pub fn new(
        center: Point3,
        x_axis: Vec3,
        y_axis: Vec3,
        anal_domain: Extent1d,
        semi_axis: f64,
        semi_image_axis: f64,
        dim: u64,
    ) -> Self {
        let mut h = Self {
            base: BSplineCurve::new(dim),
            position: Axis2Placement::IDENTITY,
            anal_domain,
            semi_axis,
            semi_image_axis,
        };

        // set m_vPosition
        let _ = h.position.set_canonical(center, x_axis, y_axis);

        // A new curve has no owner, so MakeNurb cannot reach an unported callee.
        let _ = h.make_nurb(None);

        h.base.form = BSplineCurveForm::HyperbolicArc;
        h
    }

    /// The empty curve `ReadFromDB` fills (`SmHyperbola()`): a 3D base without NURB,
    /// the identity placement, the "inside out" domain and zero axes (upstream leaves
    /// the lengths uninitialised).
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            base: BSplineCurve::new(3),
            position: Axis2Placement::IDENTITY,
            anal_domain: Extent1d::new(),
            semi_axis: 0.0,
            semi_image_axis: 0.0,
        }
    }

    // ---- access -----------------------------------------------------------------------

    /// The `SmBSplineCurve` base object.
    #[must_use]
    pub const fn base(&self) -> &BSplineCurve {
        &self.base
    }

    /// The `SmBSplineCurve` base object, mutable (for the base-class edits).
    pub(crate) fn base_mut(&mut self) -> &mut BSplineCurve {
        &mut self.base
    }

    /// The placement: center, semi axis and semi imaginary axis directions.
    #[must_use]
    pub const fn position(&self) -> &Axis2Placement {
        &self.position
    }

    /// The analytic (STEP) domain (`GetSTEPInterval`).
    #[must_use]
    pub const fn step_interval(&self) -> Extent1d {
        self.anal_domain
    }

    /// The length of the semi axis (`a`).
    #[must_use]
    pub const fn semi_axis(&self) -> f64 {
        self.semi_axis
    }

    /// The length of the semi imaginary axis (`b`).
    #[must_use]
    pub const fn semi_image_axis(&self) -> f64 {
        self.semi_image_axis
    }

    /// `[-6, 6]`, the practical limit of the analytic domain
    /// (`GetMaxAnalyticDomain`).
    #[must_use]
    pub fn max_analytic_domain(&self) -> Extent1d {
        Extent1d::from_min_max(-6.0, 6.0)
    }

    /// Always `true` (`IsAnalytic`).
    #[must_use]
    pub const fn is_analytic(&self) -> bool {
        true
    }

    /// Always `false`: a hyperbola is never a circular arc (`IsArc`).
    #[must_use]
    pub const fn is_arc(&self) -> bool {
        false
    }

    /// Whether the analytic domain is finite (`IsBounded`).
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        self.anal_domain.is_bounded()
    }

    /// `SmHyperbola_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmHyperbola_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmHyperbola_TYPE"
    }

    /// `"SmHyperbola"` (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        "SmHyperbola"
    }

    /// True for `SmHyperbola_TYPE` and the types of its bases (`IsKindOf`).
    #[must_use]
    pub const fn is_kind_of(t: i64) -> bool {
        t == Self::TYPE || t == BSplineCurve::TYPE || curve_base::is_kind_of(t)
    }

    // ---- comparison and copies --------------------------------------------------------

    /// `operator==`: equal B-spline bases (same dimension, NURBs equal within
    /// `SM_EFF_ZERO`), placements equal within the scaled zero, equal domains and axis
    /// lengths equal within `SM_EFF_ZERO`.
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
        self.position.approx_eq(&other.position)
            && self.anal_domain.are_same(&other.anal_domain)
            && is_zero(self.semi_axis - other.semi_axis)
            && is_zero(self.semi_image_axis - other.semi_image_axis)
    }

    /// A copy (`Copy`, copy constructor): the base is copied as a B-spline (NURB
    /// copied, knots fixed up), the analytic data as is.
    #[must_use]
    pub fn copy(&self) -> Self {
        self.clone()
    }

    // ---- evaluation -------------------------------------------------------------------

    /// The point and derivatives `1..=num_derivs` (at most 3) at the analytic
    /// parameter `step_param`, clamped to the analytic domain (`EvaluateSTEP`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for more than 3 derivatives.
    pub fn evaluate_step(&self, step_param: f64, num_derivs: usize) -> Result<Vec<Vec3>> {
        if num_derivs > 3 {
            return Err(SolidError::sm_err(OP_EVAL_STEP));
        }
        let mut t = step_param;
        if !self.anal_domain.contains_value(t, 0.0) {
            t = self.anal_domain.clamp_value(t);
        }
        let o = self.position.origin();
        let x = self.position.x_axis();
        let y = self.position.y_axis();
        let a = self.semi_axis;
        let b = self.semi_image_axis;

        let mut out = Vec::with_capacity(num_derivs + 1);

        // evaluate position
        out.push(o + (a * t.cosh()) * x + (b * t.sinh()) * y);

        // 1st derivative
        if num_derivs > 0 {
            out.push((a * t.sinh()) * x + (b * t.cosh()) * y);
        }

        // 2nd derivative
        if num_derivs > 1 {
            out.push((a * t.cosh()) * x + (b * t.sinh()) * y);
        }

        // 3rd derivative
        if num_derivs > 2 {
            out.push((a * t.sinh()) * x + (b * t.cosh()) * y);
        }
        Ok(out)
    }

    /// The analytic parameter of `point`, assumed on the curve: `asinh` of its
    /// `Y` coordinate over `b` (`STEPInversion`). With `classify`, also whether the
    /// point is on the curve: [`CurveLocation::Exterior`] when the parameter is
    /// outside the domain, else the result of comparing the point with the curve
    /// point there.
    ///
    /// # Errors
    ///
    /// None in practice: the STEP evaluation's status is ignored, as upstream.
    pub fn step_inversion(
        &self,
        point: Point3,
        classify: bool,
    ) -> Result<(f64, Option<CurveLocation>)> {
        let center = self.position.origin();
        let y_axis = self.position.y_axis();

        // First, find its local coordinates with respect to axes of ellipse
        let v = point - center;
        let dy = v.dot(y_axis);

        // arc-sinh(u) = ln(u + sqrt(u*u+1.0))
        let val = dy / self.semi_image_axis;
        let param = (val + sqrt(val * val + 1.0)).ln();

        // when asked - classify the point
        let mut loc = None;
        if classify {
            // check that point maps to inside of line
            let l = if self.anal_domain.contains_value(param, 0.0) {
                CurveLocation::Interior
            } else {
                CurveLocation::Exterior
            };

            // check distance to point
            let mut l = l;
            if l == CurveLocation::Interior {
                // SmCurve::EvaluateSTEPPoint: EvaluateSTEP without derivatives
                // (status ignored)
                let point_at = ignored(self.evaluate_step(param, 0))?
                    .and_then(|v| v.first().copied())
                    .unwrap_or(Vec3::UNDEF);
                let dist = point_at.distance(point);
                let scaled_zero = EFF_ZERO * (1.0 + point_at.max_dimension());

                // when dist is too large - point is not on the line
                if dist > scaled_zero {
                    l = CurveLocation::Exterior;
                }
            }
            loc = Some(l);
        }

        // all done
        Ok((param, loc))
    }

    /// Extrema of the distance from `test_point` to the curve over `interval` (an
    /// analytic domain, which first becomes the curve's: `AdjustSTEPInterval`), as
    /// analytic parameters (`GlobalPointSolveSTEP`): `SmCurve::GlobalPointSolve`
    /// ([`Curve::global_point_solve`]) over the natural interval, each solution's
    /// point inverted back to the analytic parameter. `owner` is the curve's owner;
    /// the solutions name no object (the curve has no handle here).
    ///
    /// # Errors
    ///
    /// Those of [`Hyperbola::adjust_step_interval`], the global solve, the
    /// evaluations and [`Hyperbola::step_inversion`].
    pub fn global_point_solve_step(
        &mut self,
        k: &Kernel,
        interval: &Extent1d,
        operation: SolverOperation,
        test_point: Point3,
        distance_tol: f64,
        target_distance: Option<f64>,
        requested: SolutionRequested,
        owner: Option<ObjectRef>,
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
        let sols = curve.global_point_solve(
            k,
            &ivl,
            operation,
            test_point,
            distance_tol,
            requested,
            &opts,
        )?;

        let mut out = Vec::with_capacity(sols.len());
        for sol in sols.as_slice() {
            let point_on_curve = curve.evaluate_point(k, sol.start.parameters[0])?;
            let (angle_param, _) = self.step_inversion(point_on_curve, false)?;
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

    /// Makes `new_interval` the analytic domain and rebuilds the NURB
    /// (`AdjustSTEPInterval`); nothing happens when the domains agree within
    /// `SM_EFF_ZERO`. `owner` is the curve's owner.
    ///
    /// # Errors
    ///
    /// Those of [`Hyperbola::make_nurb`] (the new domain is kept).
    pub fn adjust_step_interval(
        &mut self,
        new_interval: &Extent1d,
        owner: Option<ObjectRef>,
    ) -> Result<()> {
        if new_interval.is_contained_by(&self.anal_domain, EFF_ZERO)
            && self.anal_domain.is_contained_by(new_interval, EFF_ZERO)
        {
            return Ok(());
        }

        self.anal_domain = *new_interval;
        self.make_nurb(owner)
    }

    /// Builds the NURB of the analytic data (`MakeNurb`): a rational quadratic Bezier
    /// over the analytic domain with the domain's end points as end control points,
    /// the end tangents' intersection as middle control point and the middle weight
    /// that puts the domain's middle point on the curve; made 2D for a 2D curve. For
    /// a generator of a surface of extrusion or revolution (`owner`) it would then be
    /// reparameterised to the surface's generator range. The old NURB is dropped
    /// first.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the end tangents do not intersect, the middle point's tangent
    /// line does not cut the chord strictly inside, or the B-spline cannot be built
    /// (the curve then has no NURB); [`SolidError::Unported`] for an owner surface.
    pub fn make_nurb(&mut self, owner: Option<ObjectRef>) -> Result<()> {
        // remove existing hyperbola Nurb Representation
        self.base.nurb = None;

        // get start, mid, and end point and tangent values
        let t1 = self.anal_domain.min();
        let t2 = self.anal_domain.max();
        let t_mid = self.anal_domain.evaluate(0.5);
        let start_pv = self.evaluate_step(t1, 1)?;
        let mid_pnt = at(&self.evaluate_step(t_mid, 0)?, 0);
        let end_pv = self.evaluate_step(t2, 1)?;
        let (s0, s1) = (at(&start_pv, 0), at(&start_pv, 1));
        let (e0, e1) = (at(&end_pv, 0), at(&end_pv, 1));

        // place control point at endPointTangent Line intersections
        let (x1, _) = line_line_closest_point(s0, s1, e0, e1)?;
        let mid_ctrl_pnt = s0 + x1 * s1;

        // Create a canonical 3 ControlPoint Bspline
        let cntrl_poly = [s0, mid_ctrl_pnt, e0];
        let knots = [t1, t2];
        let knot_mult = [3usize, 3];

        // Calculate weight of mid control-point
        let start_to_end_v = e0 - s0;
        let v0 = mid_pnt - s0;
        let v1 = mid_ctrl_pnt - mid_pnt;
        let v2 = mid_pnt - e0;
        let (x1, _) = line_line_closest_point(s0, start_to_end_v, mid_pnt, v1)?;
        // Error: midPoint line fails to intersect hyperpola's base chord
        if x1 < EFF_ZERO || x1 > 1.0 - EFF_ZERO {
            return Err(SolidError::sm_err(OP_MAKE_NURB));
        }

        // mid ControlPoint weight =
        let mut val = sqrt(x1 / (1.0 - x1));
        val = val / (1.0 + val);
        let da = (1.0 - val) * (1.0 - val);
        let db = val * val;
        let dc = 2.0 * val * (1.0 - val);
        let num = da * (v0.dot(v1)) + db * (v1.dot(v2));
        let denom = dc * (v1.dot(v1));
        let weights = [1.0, num / denom, 1.0];

        // build temporary SmBSplineCurve with appropriate Nurb object
        let tmp = BSplineCurve::create_canonical(&CanonicalData {
            dim: 3,
            degree: 2,
            control_points: &cntrl_poly,
            form: BSplineCurveForm::Unspecified,
            knot_multiplicities: &knot_mult,
            knots: &knots,
            knot_type: KnotType::Unspecified,
            weights: Some(&weights),
            trim: None,
        })?;

        // Place a copy of pTmpBSC->m_pNurb into this Curve's m_pNurb
        self.base.set_from_gw_nurb(tmp.gw_nurb())?;

        // Set ControlPoint Z values for 2d curves (status ignored upstream)
        if self.base.dim == 2 {
            let _ = self.base.convert_to_2d();
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

        // all done
        Ok(())
    }

    /// Reverses the direction of the curve (`ReverseParameterization`): the domain
    /// `[m, M]` becomes `[-M, -m]` and the semi imaginary axis is negated; returns
    /// `old_interval` reversed the same way (or unchanged when it is inverted). The
    /// NURB is rebuilt (see the module deviations). `owner` is the curve's owner.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from rebuilding the NURB of a surface generator.
    pub fn reverse_parameterization(
        &mut self,
        old_interval: &Extent1d,
        owner: Option<ObjectRef>,
    ) -> Result<Extent1d> {
        // remove existing hyperbola representation
        self.base.nurb = None;

        let mut new_interval = *old_interval;
        let _ = new_interval.set_min_max(-old_interval.max(), -old_interval.min());
        let (min, max) = (self.anal_domain.min(), self.anal_domain.max());
        let _ = self.anal_domain.set_min_max(-max, -min);

        let neg_y_axis = -self.position.y_axis();
        let (origin, x_axis) = (self.position.origin(), self.position.x_axis());
        let _ = self.position.set_canonical(origin, x_axis, neg_y_axis);

        // upstream: rebuilt by the next GetOrCreateGwNurbPointer (status ignored)
        ignore_status(self.make_nurb(owner))?;
        Ok(new_interval)
    }

    /// Scales the curve uniformly by `scale` about the global origin (optional), then
    /// rotates and moves it by `rotate_n_move` (`Transform`); the identity placement
    /// with no or unit scale changes nothing.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a non-uniform scale; those of
    /// [`Axis2Placement::set_canonical`] and [`BSplineCurve::transform`].
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
                return Err(SolidError::sm_err(OP_TRANSFORM));
            }
        }

        let d_scale = scale.map_or(1.0, |s| s.x); // Uniform scaling

        let mut place = Axis2Placement::IDENTITY;
        place.set_canonical(
            self.position.origin() * d_scale,
            self.position.x_axis(),
            self.position.y_axis(),
        )?;
        self.position = place.transform_axis2_placement(rotate_n_move);

        self.semi_axis *= d_scale;
        self.semi_image_axis *= d_scale;

        if self.base.nurb.is_some() {
            self.base.transform(rotate_n_move, scale)?;
        }
        Ok(())
    }

    // ---- persistence ------------------------------------------------------------------

    /// Reads a hyperbola written by `WriteToDB` (`ReadFromDB`): center, X axis, Y axis
    /// (three values each), domain ends, semi axis, semi imaginary axis (one group per
    /// line in ASCII files), then the B-spline base record
    /// ([`BSplineCurve::read_from_db`], image dimension `dim`, database `version`).
    /// The NURB is the stored one. Unreadable ASCII values keep their defaults
    /// (undefined vectors, zeros); the placement and domain are set with their
    /// statuses ignored.
    ///
    /// # Errors
    ///
    /// A failed binary read of the hyperbola data; [`SolidError::Unported`] from the
    /// base reader (its other errors are ignored, as upstream).
    pub fn read_from_db(db: &mut DatabaseReader, dim: u64, version: u64) -> Result<Self> {
        // init output object
        let mut h = Self::empty();

        // locals
        let mut orig = Vec3::UNDEF;
        let mut x = Vec3::UNDEF;
        let mut y = Vec3::UNDEF;
        let mut anal = [0.0f64; 2];
        let mut semi_axis = 0.0f64;
        let mut semi_image_axis = 0.0f64;

        if db.file_type() == FileType::Ascii {
            for v in [&mut orig, &mut x, &mut y] {
                if let Some(s) = db.in_stream_mut() {
                    for c in [&mut v.x, &mut v.y, &mut v.z] {
                        extract(s, c);
                    }
                }
                db.go_to_next_line();
            }
            if let Some(s) = db.in_stream_mut() {
                for c in &mut anal {
                    extract(s, c);
                }
            }
            db.go_to_next_line();
            for c in [&mut semi_axis, &mut semi_image_axis] {
                if let Some(s) = db.in_stream_mut() {
                    extract(s, c);
                }
                db.go_to_next_line();
            }
        } else {
            for v in [&mut orig, &mut x, &mut y] {
                v.x = db.read_double()?;
                v.y = db.read_double()?;
                v.z = db.read_double()?;
            }
            anal[0] = db.read_double()?;
            anal[1] = db.read_double()?;
            semi_axis = db.read_double()?;
            semi_image_axis = db.read_double()?;
        }

        // load the obj
        let _ = h.position.set_canonical(orig, x, y);
        let _ = h.anal_domain.set_min_max(anal[0], anal[1]);
        h.semi_axis = semi_axis;
        h.semi_image_axis = semi_image_axis;

        // read the base type (status ignored upstream)
        match BSplineCurve::read_from_db(db, dim, version) {
            Ok(base) => h.base = base,
            Err(e @ SolidError::Unported { .. }) => return Err(e),
            Err(_) => {}
        }

        // all done
        Ok(h)
    }
}

/// `is >> value`: stores the extracted value unless the stream's sentry fails.
fn extract(s: &mut InputStream, slot: &mut f64) {
    if let Some(v) = s.extract_double() {
        *slot = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::database_io::InputStream;

    fn canonical() -> Hyperbola {
        Hyperbola::new(
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Extent1d::from_min_max(-1.0, 1.0),
            1.0,
            1.0,
            3,
        )
    }

    #[test]
    fn nurb_matches_analytic_curve() {
        let mut h = canonical();
        h.adjust_step_interval(&Extent1d::from_min_max(-1.0, 2.0), None)
            .unwrap();
        assert_eq!(h.base.form(), BSplineCurveForm::HyperbolicArc);
        assert_eq!(h.base.natural_interval(), Extent1d::from_min_max(-1.0, 2.0));
        for t in [-1.0, 0.5, 2.0] {
            let p = h.evaluate_step(t, 0).unwrap()[0];
            let (s, loc) = h.step_inversion(p, false).unwrap();
            assert!((s - t).abs() < 1e-12);
            assert!(loc.is_none());
        }
        // the end points of the NURB are the analytic end points
        let ends = h.base.ends().unwrap();
        assert!(ends.start.distance(h.evaluate_step(-1.0, 0).unwrap()[0]) < 1e-12);
        assert!(ends.end.distance(h.evaluate_step(2.0, 0).unwrap()[0]) < 1e-12);
        // the middle point is on the NURB
        let mid = h.base.evaluate_point(0.5).unwrap();
        let (s, _) = h.step_inversion(mid, false).unwrap();
        let on = h.evaluate_step(s, 0).unwrap()[0];
        assert!(on.distance(mid) < 1e-9);
    }

    #[test]
    fn queries_and_type() {
        let h = canonical();
        assert!(h.is_analytic());
        assert!(!h.is_arc());
        assert!(h.is_bounded());
        assert_eq!(h.max_analytic_domain(), Extent1d::from_min_max(-6.0, 6.0));
        assert!(Hyperbola::is_kind_of(Hyperbola::TYPE));
        assert!(Hyperbola::is_kind_of(BSplineCurve::TYPE));
        assert!(!Hyperbola::is_kind_of(14023));
        assert_eq!(h.type_string(), "SmHyperbola_TYPE");
        assert_eq!(h.class_string(), "SmHyperbola");
        assert!(h.evaluate_step(0.0, 4).is_err());
        let d = h.evaluate_step(0.25, 3).unwrap();
        assert_eq!(d.len(), 4);
        assert_eq!(d[1], d[3]);
    }

    #[test]
    fn equality_copy_and_reverse() {
        let h = canonical();
        let mut c = h.copy();
        assert!(h.approx_eq(&c));
        let r = c
            .reverse_parameterization(&Extent1d::from_min_max(-1.0, 1.0), None)
            .unwrap();
        assert_eq!(r, Extent1d::from_min_max(-1.0, 1.0));
        assert_eq!(c.position.y_axis(), Vec3::new(0.0, 0.0, -1.0));
        assert!(c.base.gw_nurb().is_some());
        // the reversed curve passes through the same points
        let p = h.evaluate_step(0.5, 0).unwrap()[0];
        let q = c.evaluate_step(-0.5, 0).unwrap()[0];
        assert!(p.distance(q) < 1e-12);
    }

    #[test]
    fn transform_and_failures() {
        let mut h = canonical();
        let mv = Axis2Placement::new(
            Point3::new(1.0, 2.0, 3.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(-1.0, 0.0, 0.0),
        );
        h.transform(&mv, Some(Vec3::new(2.0, 2.0, 2.0))).unwrap();
        assert_eq!(h.semi_axis(), 2.0);
        assert!(h.transform(&mv, Some(Vec3::new(1.0, 2.0, 2.0))).is_err());
        // a degenerate domain has parallel end tangents: no NURB
        let d = Hyperbola::new(
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Extent1d::from_value(0.5),
            1.0,
            2.0,
            3,
        );
        assert!(d.base.gw_nurb().is_none());
    }

    #[test]
    fn unported_paths_and_step_solve() {
        let mut h = canonical();
        let p = h.evaluate_step(0.0, 0).unwrap()[0];
        let (t, loc) = h.step_inversion(p, true).unwrap();
        assert!(t.abs() < 1e-12);
        assert_eq!(loc, Some(CurveLocation::Interior));
        let far = Point3::new(0.0, 0.0, 100.0);
        assert_eq!(
            h.step_inversion(far, true).unwrap().1,
            Some(CurveLocation::Exterior)
        );
        let r = h.global_point_solve_step(
            &Kernel::default(),
            &Extent1d::from_min_max(-1.0, 1.0),
            SolverOperation::Intersect,
            p,
            1e-6,
            None,
            SolutionRequested::All,
            None,
        );
        // the point on the curve at analytic parameter 0
        let sols = r.unwrap();
        assert_eq!(sols.len(), 1);
        assert!(sols[0].start.parameters[0].abs() < 1e-9);
        assert!(sols[0].start.solution_value < 1e-9);
        let owner = ObjectRef {
            type_code: SURF_OF_EXTRUSION_TYPE,
            key: crate::geom::attribute::ObjectKey(0),
        };
        assert!(matches!(
            h.adjust_step_interval(&Extent1d::from_min_max(0.0, 1.0), Some(owner)),
            Err(SolidError::Unported { .. })
        ));
    }

    #[test]
    fn read_ascii() {
        let text = "0 0 0 origin\n1 0 0 x\n0 1 0 y\n-1 1 dom\n2 a\n3 b\n\
                    2\n2\n1\n3\n1\n4\n\
                    1 2 3 1\n4 5 6 0.5\n7 8 9 1\n\
                    0\n0\n0\n1\n1\n1\n";
        let mut db = DatabaseReader::with_stream(
            FileType::Ascii,
            InputStream::new(text.as_bytes().to_vec()),
        );
        let h = Hyperbola::read_from_db(&mut db, 3, 33).unwrap();
        assert_eq!(h.semi_axis(), 2.0);
        assert_eq!(h.semi_image_axis(), 3.0);
        assert_eq!(h.step_interval(), Extent1d::from_min_max(-1.0, 1.0));
        assert_eq!(h.base.form(), BSplineCurveForm::HyperbolicArc);
        assert_eq!(h.base.number_control_points().unwrap(), 3);
    }
}
