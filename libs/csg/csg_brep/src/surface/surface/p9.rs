//! Surface tessellation splits, iso boundaries and curve/surface leaf intersection.
//!
//! Upstream: `source/SMLib/src/SmSurface.cpp`, `source/SMLib/inc/SmSurface.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`Surface::find_tessellation_splits`] | `SmSurface::FindTessellationSplits` |
//! | [`Surface::create_iso_boundaries`] | `SmSurface::CreateIsoBoundaries` |
//! | [`CsiGlobalSolver::local_solve_branch`] | `SmCSIGlobalSolver::LocalSolve` |
//!
//! Deviations: return values replace output arrays. The maximum-length branch
//! recurses once with maximum length zero; an explicit depth limit of two guards
//! that invariant. Split arrays are limited to 1,000,000 entries before allocation
//! or concatenation. Unsupported surface virtuals return `Unported`, never an
//! approximate replacement. The diagonal curves of a cone, sphere or torus are
//! `SmCrvOnSurf` over the analytic surface itself ([`CrvOnSurf`] holds any
//! [`Surface`]). Iso curves come from the
//! kind's own `CreateIsoParametricCurve`, and implicit iso curves evaluate the plane,
//! cone, sphere and torus through their own evaluation, as upstream's virtuals;
//! curve-bounded, STEP, offset and in-volume surfaces build both kinds of iso curve
//! over the surface itself (the `SmSurface` body, with the caller's approximation
//! tolerance). Iso extraction is exact for B-splines, so the tolerance has no
//! numerical effect for that kind.
//! Failed tessellations retain the empty parameter array;
//! unported errors are propagated even where upstream ignores a status.

use std::borrow::Cow;

use super::p1::CsiGlobalSolver;
use super::p2::{merge_double_arrays, Surface, SurfaceKind};
use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::p3::{local_curve_intersect, PointSolveOptions};
use crate::curve::curve::{Curve, CurveKind};
use crate::curve::iso_curve::IsoCurve;
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{
    Branch, GlobalSolve, GlobalSolver, PreferredSolution, SolutionPairType, SolverOperation,
};
use crate::geom::solution_array::{Solution, SolutionObject, SolutionType, SortKey};
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::Point3;
use crate::kernel::Kernel;
use crate::math::EFF_ZERO_SQRT;
use crate::math::{max2, EFF_ZERO};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::{BSplineSurface, IsoCurveOptions, SurfParamType};
use crate::nurbs::crv_basic::Curve as NurbCurve;
use crate::nurbs::math::{CPoint, NOW, NOZ};
use crate::surface::surface_cache::BezierPatch;

const OP: &str = "surface::surface::p9::find_tessellation_splits";
const MAX_SPLITS: usize = 1_000_000;

fn bss<'a>(k: &Kernel, surface: &'a Surface) -> Result<Cow<'a, BSplineSurface>> {
    match surface.kind() {
        SurfaceKind::BSpline(s) => Ok(Cow::Borrowed(s)),
        SurfaceKind::Plane(p) => Ok(p.base_for_query()),
        SurfaceKind::Cone(_) | SurfaceKind::Sphere(_) | SurfaceKind::Torus(_) => {
            // (unreachable: iso curves of these kinds dispatch to the kinds' own
            // overrides, and the diagonal `SmCrvOnSurf` of `FindTessellationSplits`
            // holds the analytic surface itself, as upstream, not a B-spline view)
            Err(SolidError::invalid_input(
                OP,
                "surface has no B-spline view",
            ))
        }
        SurfaceKind::SurfOfRevolution(s) => Ok(Cow::Borrowed(s.base())),
        // (the B-spline evaluation makes a missing NURB: GetOrCreateGwNurbPointer)
        SurfaceKind::SurfOfExtrusion(s) => s.base_with_nurb(k),
        SurfaceKind::CurveBounded(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => {
            // (unreachable: the iso curves of these kinds are built over the surface
            // itself, as upstream's `SmIsoCurve` and `SmSurface::CreateIsoParametricCurve`)
            Err(SolidError::invalid_input(
                OP,
                "surface has no B-spline view",
            ))
        }
    }
}

/// The `SmBSplineCurve` part of an iso curve as upstream types it: the B-spline
/// itself, a circle's B-spline or a line's NURB.
fn iso_base(curve: &Curve) -> Result<BSplineCurve> {
    match curve.kind() {
        CurveKind::BSpline(c) => Ok(c.copy()),
        CurveKind::Ellipse(c) => Ok(c.base().copy()),
        CurveKind::Line(c) => Ok(c.base_with_nurb()?.into_owned()),
        _ => Err(SolidError::invalid_input(OP, "unexpected iso curve kind")),
    }
}

/// `CreateIsoParametricCurve` through the surface virtual: the B-spline view
/// used for lengths and point spacing, and the curve as upstream types it
/// (`SmLine` for planes and cone rulings, `SmCircle` for revolution sweeps), whose
/// own `IsDegenerate` and `ApproximateLength` overrides upstream calls.
fn typed_iso_curve(
    k: &Kernel,
    surface: &Surface,
    direction: SurfParamType,
    value: f64,
    approx_tol: f64,
) -> Result<(BSplineCurve, Curve)> {
    let typed = match surface.kind_with_nurb(k)? {
        // (no override: the `SmSurface` body, whose approximation uses the tolerance)
        SurfaceKind::CurveBounded(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => {
            surface
                .create_iso_parametric_curve(k, direction, value, approx_tol, None, false)?
                .curve
        }
        SurfaceKind::Plane(p) => Curve::from(
            p.create_iso_parametric_curve(direction, value, None, false)?
                .curve,
        ),
        SurfaceKind::SurfOfRevolution(s) => {
            s.create_iso_parametric_curve(k, direction, value, None)?
                .curve
        }
        SurfaceKind::Cone(s) => {
            s.create_iso_parametric_curve(k, direction, value, None, false)?
                .curve
        }
        SurfaceKind::Sphere(s) => {
            s.create_iso_parametric_curve(k, direction, value, None, false)?
                .curve
        }
        SurfaceKind::Torus(s) => {
            s.create_iso_parametric_curve(k, direction, value, None, false)?
                .curve
        }
        _ => {
            let base = bss(k, surface)?
                .create_iso_parametric_curve(k, direction, value, IsoCurveOptions::default())?
                .curve;
            let typed = Curve::from(base.clone());
            return Ok((base, typed));
        }
    };
    Ok((iso_base(&typed)?, typed))
}

/// `SmIsoCurve(surface, direction, value, left)` (clamped input): over the B-spline
/// view of `surface`, or over the surface itself for a plane, cone, sphere or torus.
pub(super) fn implicit_iso(
    k: &Kernel,
    surface: &Surface,
    direction: SurfParamType,
    value: f64,
    left: bool,
) -> Result<Curve> {
    // upstream's SmIsoCurve evaluates through the surface's virtual Evaluate: for an
    // SmPlane that is the analytic evaluation, not the NURB (whose bilinear
    // interpolation over the plane's +-1234567 analytic domain differs in the last bits),
    // likewise SmCone's; the sphere and torus dispatch to their inherited evaluation
    if let SurfaceKind::Plane(_)
    | SurfaceKind::Cone(_)
    | SurfaceKind::Sphere(_)
    | SurfaceKind::Torus(_)
    | SurfaceKind::CurveBounded(_)
    | SurfaceKind::Step(_)
    | SurfaceKind::OffsetSurface(_)
    | SurfaceKind::SrfInVolume(_) = surface.kind()
    {
        return Ok(Curve::from(IsoCurve::from_surface(
            k,
            surface.clone_for_evaluation()?,
            direction,
            value,
            left,
            true,
        )?));
    }
    // (`SmIsoCurve(*this, ..)` refers to the surface itself: an exact copy, not the copy
    // constructor, whose knot fix-up can move near-coincident knots)
    Ok(Curve::from(IsoCurve::new(
        bss(k, surface)?.clone_exact(),
        direction,
        value,
        left,
        true,
    )))
}

fn tessellate(
    k: &Kernel,
    curve: &Curve,
    interval: &Extent1d,
    chord: f64,
    angle: f64,
    max_side: f64,
) -> Result<Vec<f64>> {
    Ok(
        ignored(curve.tessellate_by_bisection(k, interval, chord, angle, max_side))?
            .map(|t| t.parameters)
            .unwrap_or_default(),
    )
}

fn check_count(n: usize) -> Result<()> {
    if n > MAX_SPLITS {
        return Err(SolidError::invalid_input(
            OP,
            "tessellation split limit exceeded",
        ));
    }
    Ok(())
}

impl Surface {
    /// Creates the curves at the minimum and maximum natural-domain boundary in
    /// `direction`. Other enumerators leave both outputs unset, as upstream.
    /// The curves keep the kind the surface's `CreateIsoParametricCurve` override
    /// builds (`SmLine` for plane lines and cone rulings, `SmCircle` for revolution
    /// sweeps): upstream's `SmBSplineCurve*` outputs dispatch on it.
    ///
    /// # Errors
    /// Those of iso-curve construction; `Unported` for unsupported surface kinds.
    pub fn create_iso_boundaries(
        &self,
        k: &Kernel,
        direction: SurfParamType,
        tolerance: f64,
    ) -> Result<Option<(Curve, Curve)>> {
        let domain = self.natural_uv_domain_with_nurb(k)?;
        let (min, max) = match direction {
            SurfParamType::U => (domain.min().x, domain.max().x),
            SurfParamType::V => (domain.min().y, domain.max().y),
            _ => return Ok(None),
        };
        let first = typed_iso_curve(k, self, direction, min, tolerance)?.1;
        let second = typed_iso_curve(k, self, direction, max, tolerance)?.1;
        Ok(Some((first, second)))
    }

    /// Selects increasing parameters along the direction perpendicular to the
    /// constant iso parameter, including the interval endpoints.
    ///
    /// # Errors
    /// Evaluation/construction errors, unsupported surface virtuals and bounded
    /// split-count or recursion limits.
    pub fn find_tessellation_splits(
        &self,
        k: &Kernel,
        direction: SurfParamType,
        domain: &Extent2d,
        chord_height: f64,
        angle_deg: f64,
        max_side: f64,
        check_diagonals: bool,
    ) -> Result<Vec<f64>> {
        self.find_splits_at_depth(
            k,
            direction,
            domain,
            chord_height,
            angle_deg,
            max_side,
            check_diagonals,
            0,
        )
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn find_splits_at_depth(
        &self,
        k: &Kernel,
        direction: SurfParamType,
        domain: &Extent2d,
        chord: f64,
        angle: f64,
        max_side: f64,
        diagonals: bool,
        depth: usize,
    ) -> Result<Vec<f64>> {
        if depth >= 2 {
            return Err(SolidError::invalid_input(
                OP,
                "tessellation recursion limit exceeded",
            ));
        }
        let interval = if direction == SurfParamType::V {
            domain.u_interval()
        } else {
            domain.v_interval()
        };
        if max_side > 0.0 {
            let mut length = 0.0;
            let mut max_length = -1.0;
            let mut longest = None;
            for i in 0..=5 {
                let uv = domain.evaluate(f64::from(i) / 5.0, f64::from(i) / 5.0);
                let iso = if direction == SurfParamType::V {
                    uv.y
                } else {
                    uv.x
                };
                let (curve, typed) = typed_iso_curve(k, self, direction, iso, max_side / 100.0)?;
                // `pBSC->IsDegenerate()` is virtual: a revolution circle uses
                // `SmEllipse::IsDegenerate` (radius < tol/2 or arc < tol) and a
                // plane line `SmLine::IsDegenerate`, not the B-spline test.
                if typed.is_degenerate(k, EFF_ZERO, None)? {
                    continue;
                }
                // (`ApproximateLength` is virtual: `SmLine` overrides it, the other
                // iso curve kinds run the B-spline's)
                length = if let CurveKind::Line(_) = typed.kind() {
                    typed.approximate_length(k, &interval, 30, false)?.0
                } else {
                    Curve::from(curve.clone())
                        .approximate_length(k, &interval, 30, false)?
                        .0
                };
                if length > max_length {
                    max_length = length;
                    longest = Some(curve);
                }
            }
            let count = max_length / max_side + 1.0;
            if !count.is_finite() || count < 0.0 || count > (MAX_SPLITS - 1) as f64 {
                return Err(SolidError::invalid_input(
                    OP,
                    "tessellation split limit exceeded",
                ));
            }
            let count = count.max(0.0) as usize;
            let splits = if count < 2 {
                vec![interval.min(), interval.max()]
            } else {
                longest
                    .ok_or(SolidError::sm_err(OP))?
                    .equally_spaced_points(interval.min(), interval.max(), count, length / 1000.0)?
                    .1
            };
            let mut all = Vec::new();
            for (index, span) in splits.windows(2).enumerate() {
                let (Some(&start), Some(&end)) = (span.first(), span.last()) else {
                    continue;
                };
                let mut sub_domain = *domain;
                let span_interval = Extent1d::from_min_max(start, end);
                if direction == SurfParamType::V {
                    sub_domain.set_u_interval(&span_interval);
                } else {
                    sub_domain.set_v_interval(&span_interval);
                }
                let mut part = self.find_splits_at_depth(
                    k,
                    direction,
                    &sub_domain,
                    chord,
                    angle,
                    0.0,
                    diagonals,
                    depth + 1,
                )?;
                if index + 2 < splits.len() {
                    part.pop();
                }
                check_count(all.len().saturating_add(part.len()))?;
                all.extend(part);
            }
            return Ok(if all.len() > splits.len() {
                all
            } else {
                splits
            });
        }
        if chord == 0.0 && angle == 0.0 {
            return Ok(vec![interval.min(), interval.max()]);
        }
        let linear_tol = if chord == 0.0 {
            EFF_ZERO_SQRT
        } else {
            chord / 10.0
        };
        let mut params = [Vec::new(), Vec::new()];
        let mut linear = [false; 2];
        let mut failures = 0;
        for (index, (boundary, fallback)) in [(domain.min(), 0.4), (domain.max(), 0.6)]
            .into_iter()
            .enumerate()
        {
            let value = if direction == SurfParamType::V {
                boundary.y
            } else {
                boundary.x
            };
            let curve = implicit_iso(k, self, direction, value, true)?;
            let degenerate = curve.is_degenerate(k, EFF_ZERO, None)?;
            let is_linear = curve.is_linear(k, linear_tol, true)?.linear;
            let mut splits = Vec::new();
            let mut fallback_needed = degenerate || is_linear;
            if !fallback_needed {
                match curve.tessellate_by_bisection(k, &interval, chord, angle, max_side) {
                    Ok(result) => splits = result.parameters,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => fallback_needed = true,
                }
            }
            let mut final_linear = is_linear;
            if fallback_needed {
                let uv = domain.evaluate(fallback, fallback);
                let value = if direction == SurfParamType::V {
                    uv.y
                } else {
                    uv.x
                };
                let curve = implicit_iso(k, self, direction, value, true)?;
                final_linear = curve.is_linear(k, linear_tol, true)?.linear;
                if !final_linear {
                    match curve.tessellate_by_bisection(k, &interval, chord, angle, max_side) {
                        Ok(result) => splits = result.parameters,
                        Err(e) if e.is_unported() => return Err(e),
                        Err(_) => failures += 1,
                    }
                }
            }
            if let Some(p) = params.get_mut(index) {
                *p = splits;
            }
            if let Some(l) = linear.get_mut(index) {
                *l = final_linear;
            }
        }
        if diagonals && (linear.iter().all(|&l| l) || failures == 2) {
            for (index, (start, end)) in [
                (domain.min(), domain.max()),
                (
                    Point2::new(domain.min().x, domain.max().y),
                    Point2::new(domain.max().x, domain.min().y),
                ),
            ]
            .into_iter()
            .enumerate()
            {
                let uv_curve = Curve::from(BSplineCurve::from_nurb(
                    NurbCurve::new(
                        vec![
                            CPoint::new(start.x, start.y, NOZ, NOW),
                            CPoint::new(end.x, end.y, NOZ, NOW),
                        ],
                        1,
                        vec![0.0, 0.0, 1.0, 1.0],
                    )
                    .map_err(SolidError::nl_ser(OP))?,
                    2,
                    false,
                ));
                let curve = Curve::from(CrvOnSurf::new(
                    k,
                    uv_curve.clone_for_evaluation()?,
                    self.clone_for_evaluation()?,
                    Some(*domain),
                    0,
                )?);
                let mut p = tessellate(
                    k,
                    &curve,
                    &Extent1d::from_min_max(0.0, 1.0),
                    chord,
                    angle,
                    max_side,
                )?;
                if index == 1 && direction == SurfParamType::U {
                    p.reverse();
                }
                for t in &mut p {
                    let uv = uv_curve.evaluate_point(k, *t)?;
                    *t = if direction == SurfParamType::V {
                        uv.x
                    } else {
                        uv.y
                    };
                }
                if let Some(slot) = params.get_mut(index) {
                    *slot = p;
                }
            }
        }
        let [first, second] = params;
        check_count(first.len().saturating_add(second.len()))?;
        let merged = merge_double_arrays(&first, &second);
        Ok(if merged.is_empty() {
            vec![interval.min(), interval.max()]
        } else {
            merged
        })
    }
}

// ---- curve/surface leaf solver -----------------------------------------------------

struct CsiIntersection {
    uv: Point2,
    parameter: f64,
    deviation: f64,
}

fn surface_closed(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    direction: SurfParamType,
) -> Result<(bool, ContinuityType)> {
    let (closed, continuity) = surface.is_closed(k, domain, direction, None, true)?;
    Ok((closed, continuity.ok_or(SolidError::sm_err(OP))?))
}
fn tangent_drop(k: &Kernel, surface: &Surface, uv: Point2, point: Point3) -> Result<Point2> {
    super::p7::drop_points_to_tangent_plane(
        k,
        surface,
        uv,
        crate::nurbs::knots::Side::Left,
        crate::nurbs::knots::Side::Left,
        &[point],
    )?
    .first()
    .copied()
    .ok_or(SolidError::sm_err(OP))
}
/// `SmSurface::LocalCurveIntersect` (virtual; no kind overrides it).
fn intersect_surface_curve(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    tol: f64,
    uv: Point2,
    t: f64,
) -> Result<Option<CsiIntersection>> {
    Ok(surface
        .local_curve_intersect(k, domain, curve, interval, tol, uv, t)?
        .map(|x| CsiIntersection {
            uv: x.uv,
            parameter: x.t,
            deviation: x.deviation,
        }))
}
fn filter_solution<O: SolutionObject>(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    conts: [[ContinuityType; 2]; 3],
    solution: &Solution<O>,
) -> Result<bool> {
    // conts: the surface u and v continuities, then the curve's
    let [u_conts, v_conts, t_conts] = conts;
    super::p5::filter_solution(
        k, surface, domain, curve, interval, u_conts, v_conts, t_conts, solution,
    )
}
/// The tolerance of the edge owning `curve` (`pCurveOwner->GetTolerance()` in
/// `sm_ClassifyCurveSurfaceXSectPoint`), through a face-aware caller's `queries` and
/// the curve's `handle`; `None` for a curve no edge owns, or when the caller cannot
/// resolve it.
///
/// # Errors
///
/// Those of the caller's edge resolution.
pub(super) fn curve_edge_tolerance<O>(
    curve: &Curve,
    handle: Option<O>,
    queries: Option<&dyn crate::surface::surface_cache::face_context::FaceQueries<O>>,
) -> Result<Option<f64>> {
    if curve
        .owner()
        .is_none_or(|o| o.type_code != crate::geom::assert_array::labels::EDGE_TYPE)
    {
        return Ok(None);
    }
    match (queries, handle) {
        (Some(queries), Some(handle)) => queries.curve_edge_tolerance(handle).transpose(),
        _ => Ok(None),
    }
}

#[allow(clippy::too_many_arguments)]
fn nearby_intersection(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    uv: Point2,
    t: f64,
    edge_tolerance: Option<f64>,
) -> Result<Option<(Point2, f64)>> {
    let c = super::p4::classify_curve_surface_xsect_point_with(
        k,
        surface,
        domain,
        curve,
        interval,
        uv,
        t,
        edge_tolerance,
    )?;
    Ok(c.found_guess.then_some((c.new_uv_guess, c.new_t_guess)))
}
fn surface_point_solve<O: Copy>(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    point: Point3,
    guess: Point2,
) -> Result<(bool, Solution<O>)> {
    let r = surface.local_point_solve::<O>(k, domain, SolverOperation::Minimize, point, guess)?;
    Ok((r.found, r.solution))
}

impl<'a, O: SolutionObject + 'a> GlobalSolve<'a, O> for CsiGlobalSolver<'a, '_, O> {
    fn solver(&self) -> &GlobalSolver<'a, O> {
        &self.base
    }
    fn solver_mut(&mut self) -> &mut GlobalSolver<'a, O> {
        &mut self.base
    }
    fn local_solve(&mut self, branch: &Branch) -> Result<bool> {
        let k = self.kernel;
        self.local_solve_branch(k, branch)
    }
    fn identical_solutions(
        &self,
        first: &Solution<O>,
        second: &Solution<O>,
    ) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
        let k = self.kernel;
        // the solutions name the solve's curve and surface (the trees' owners)
        if let Some(operands) = &self.operands {
            super::p5::csi_identical_solutions_in(
                k,
                &self.base,
                self.surface,
                first,
                second,
                operands,
            )
        } else {
            let domains = super::p1::SolveOperands {
                operands: [
                    super::p5::curve_operand(&self.base, 0, self.curve),
                    super::p5::surface_operand(&self.base, 1, self.surface_cache, self.surface),
                ],
            };
            super::p5::csi_identical_solutions_in(
                k,
                &self.base,
                self.surface,
                first,
                second,
                &domains,
            )
        }
    }
}

impl<'a, O: SolutionObject + 'a> CsiGlobalSolver<'a, '_, O> {
    /// The tolerance of the edge owning `curve` ([`curve_edge_tolerance`]) through
    /// this solve's face services and curve operand.
    fn curve_edge_tolerance(&self, curve: &Curve) -> Result<Option<f64>> {
        let handle = self.operands.and_then(|o| o.operands[0]).map(|(h, _)| h);
        curve_edge_tolerance(curve, handle, self.face_queries)
    }

    fn solution(&self, branch: &Branch, t: f64, uv: Point2, deviation: f64) -> Result<Solution<O>> {
        let mut solution = Solution::new();
        solution.num_variables = 3;
        for (slot, value) in solution.start.parameters.iter_mut().zip([t, uv.x, uv.y]) {
            *slot = value;
        }
        solution.start.solution_value = deviation;
        solution.solution_type = SolutionType::SingleValue;
        solution.num_objects = 2;
        solution.objects[0] = self.base.tree(0)?.owner_object()?;
        solution.objects[1] =
            super::p2::original_surface(self.base.tree(1)?, self.base.branch_node(branch, 1)?)?
                .into();
        for (slot, id) in solution
            .nodes
            .iter_mut()
            .take(2)
            .zip(branch.iter().copied().take(2))
        {
            *slot = id;
        }
        Ok(solution)
    }

    fn keep(
        &mut self,
        k: &Kernel,
        _branch: &Branch,
        surface: &Surface,
        domain: &Extent2d,
        interval: &Extent1d,
        conts: [[ContinuityType; 2]; 3],
        solution: &Solution<O>,
    ) -> Result<()> {
        if filter_solution(
            k,
            surface,
            domain,
            self.curve.ok_or(SolidError::null_pointer(OP))?,
            interval,
            conts,
            solution,
        )? {
            let uv = Point2::new(solution.start.param(1)?, solution.start.param(2)?);
            if super::p1::is_point_valid_with(
                k,
                self.base.tree(1)?.owner(),
                self.surface_cache.ok_or(SolidError::null_pointer(OP))?,
                uv,
                None,
                self.face_queries,
            )? {
                self.add_sorted_solution(solution, SortKey::ByAllParameters, true)?;
            }
        }
        Ok(())
    }

    /// `SmCSIGlobalSolver::LocalSolve`. Returns the request for more subdivision
    /// (always false in the upstream implementation), appending kept solutions to
    /// `base.solutions` in upstream order.
    ///
    /// # Errors
    /// Invalid branch/patch data and explicit unported surface operations. No
    /// missing solver operation is replaced with a synthetic intersection.
    #[allow(clippy::too_many_lines)]
    pub fn local_solve_branch(&mut self, k: &Kernel, branch: &Branch) -> Result<bool> {
        let curve_node = *self.base.branch_node(branch, 0)?;
        let surface_node = *self.base.branch_node(branch, 1)?;
        let curve_tree = self.base.tree(0)?;
        let span = *curve_tree.curve_span(&curve_node)?;
        let surface_id = branch
            .get(1)
            .copied()
            .flatten()
            .ok_or(SolidError::null_pointer(OP))?;
        let patch: BezierPatch<O> = self
            .surface_cache
            .ok_or(SolidError::null_pointer(OP))?
            .patch(surface_id)?
            .clone();
        let original = super::p2::original_surface(self.base.tree(1)?, &surface_node)?;
        if Some(original)
            != self
                .surface_cache
                .ok_or(SolidError::null_pointer(OP))?
                .surface_handle()
        {
            return Err(SolidError::invalid_input(
                OP,
                "surface cache does not own the original surface",
            ));
        }
        // Resolve the original tree-owner handle, not the Bezier patch surface.
        let parent_surface = self
            .surface_cache
            .ok_or(SolidError::null_pointer(OP))?
            .surface();
        let eval_handle = super::p1::surface_to_evaluate(self.base.tree(1)?, &surface_node)?;
        if Some(eval_handle)
            != self
                .surface_cache
                .ok_or(SolidError::null_pointer(OP))?
                .surface_handle()
        {
            return Err(SolidError::invalid_input(
                OP,
                "surface evaluation handle has no geometry resolver",
            ));
        }
        let eval_surface = self
            .surface_cache
            .ok_or(SolidError::null_pointer(OP))?
            .surface();
        let mut domain = patch.aux.uv_domain;
        let u_interval = domain.u_interval();
        let v_interval = domain.v_interval();
        let mut interval = *span.interval();
        if !super::p1::is_node_valid(
            self.base.tree(1)?.owner(),
            self.surface_cache.ok_or(SolidError::null_pointer(OP))?,
            &surface_node,
        )? {
            return Ok(false);
        }
        if domain.are_disjoint(&self.uv_domain, 0.0) || self.interval.are_disjoint(&interval, 0.0) {
            return Ok(false);
        }
        let mut pseudo_box = span.pseudo_box;
        pseudo_box.expand_absolute(self.base.tolerance_3d);
        if pseudo_box.are_disjoint(&patch.pseudo_box, None) {
            return Ok(false);
        }
        let scaled_zero = EFF_ZERO * (1.0 + curve_node.bbox.max().max_dimension());
        let bigger = curve_node.bbox.size().length() > surface_node.bbox.size().length() * 2.0;
        let mut t_conts = span.aux.conts;
        let mut u_conts = patch.u_curve_conts;
        let mut v_conts = patch.v_curve_conts;
        if self.uv_domain.min().x >= domain.min().x {
            u_conts[0] = ContinuityType::Discontinuous;
        }
        if self.uv_domain.max().x <= domain.max().x {
            u_conts[1] = ContinuityType::Discontinuous;
        }
        if self.uv_domain.min().y >= domain.min().y {
            v_conts[0] = ContinuityType::Discontinuous;
        }
        if self.uv_domain.max().y <= domain.max().y {
            v_conts[1] = ContinuityType::Discontinuous;
        }
        if !domain.is_contained_by(&self.uv_domain, EFF_ZERO) {
            domain = domain.intersect(&self.uv_domain)?;
            if crate::geom::tol::are_same(domain.max().x, domain.min().x)
                || crate::geom::tol::are_same(domain.max().y, domain.min().y)
            {
                return Ok(false);
            }
        }
        if self.interval.min() >= interval.min() {
            t_conts[0] = ContinuityType::Discontinuous;
        }
        if self.interval.max() <= interval.max() {
            t_conts[1] = ContinuityType::Discontinuous;
        }
        if !interval.is_contained_by(&self.interval, EFF_ZERO) {
            interval = interval.intersect(&self.interval)?;
            if crate::geom::tol::are_same(interval.max(), interval.min()) {
                return Ok(false);
            }
        }
        let natural = self
            .curve
            .ok_or(SolidError::null_pointer(OP))?
            .natural_interval();
        if (crate::geom::tol::is_zero(interval.min() - natural.min())
            || crate::geom::tol::is_zero(interval.max() - natural.max()))
            && crate::geom::tol::is_zero(self.interval.min() - natural.min())
            && crate::geom::tol::is_zero(self.interval.max() - natural.max())
            && self
                .curve
                .ok_or(SolidError::null_pointer(OP))?
                .is_closed(k, &natural, 0.0)?
        {
            let continuity = ignored(crate::curve::curve::p2::evaluate_continuity(
                k,
                self.curve.ok_or(SolidError::null_pointer(OP))?,
                natural.min(),
                false,
                self.curve.ok_or(SolidError::null_pointer(OP))?,
                natural.max(),
                true,
                0.5,
            ))?
            .map_or(ContinuityType::Discontinuous, |c| c.continuity);
            if crate::geom::tol::is_zero(interval.min() - natural.min()) {
                t_conts[0] = continuity;
            }
            if crate::geom::tol::is_zero(interval.max() - natural.max()) {
                t_conts[1] = continuity;
            }
        }
        let natural_domain = parent_surface.natural_uv_domain_with_nurb(k)?;
        for (dir, leaf, parent, work, conts) in [
            (
                SurfParamType::U,
                u_interval,
                natural_domain.u_interval(),
                self.uv_domain.u_interval(),
                &mut u_conts,
            ),
            (
                SurfParamType::V,
                v_interval,
                natural_domain.v_interval(),
                self.uv_domain.v_interval(),
                &mut v_conts,
            ),
        ] {
            if (crate::geom::tol::is_zero(leaf.min() - parent.min())
                || crate::geom::tol::is_zero(leaf.max() - parent.max()))
                && crate::geom::tol::is_zero(work.min() - parent.min())
                && crate::geom::tol::is_zero(work.max() - parent.max())
            {
                let (closed, continuity) = surface_closed(k, parent_surface, &natural_domain, dir)?;
                if closed {
                    if crate::geom::tol::is_zero(leaf.min() - parent.min()) {
                        conts[0] = continuity;
                    }
                    if crate::geom::tol::is_zero(leaf.max() - parent.max()) {
                        conts[1] = continuity;
                    }
                }
            }
        }
        let conts = [u_conts, v_conts, t_conts];
        let mut guesses = vec![(domain.evaluate(0.5, 0.5), interval.evaluate(0.5))];
        let mut uv = domain.evaluate(0.5, 0.5);
        let mut t = interval.evaluate(0.5);
        if bigger {
            let point = eval_surface.evaluate_point(k, uv)?;
            let (found, solution) = self
                .curve
                .ok_or(SolidError::null_pointer(OP))?
                .local_point_solve::<O>(
                    k,
                    &interval,
                    SolverOperation::Minimize,
                    point,
                    &PointSolveOptions::default(),
                    t,
                )?;
            if found {
                t = solution.start.param(0)?;
            }
        }
        uv = tangent_drop(
            k,
            eval_surface,
            uv,
            self.curve
                .ok_or(SolidError::null_pointer(OP))?
                .evaluate_point(k, t)?,
        )?;
        if !domain.contains_point(uv, 0.0) {
            if bigger {
                uv = domain.clamp_point(uv);
                let (found, solution) = self
                    .curve
                    .ok_or(SolidError::null_pointer(OP))?
                    .local_point_solve::<O>(
                        k,
                        &interval,
                        SolverOperation::Minimize,
                        eval_surface.evaluate_point(k, uv)?,
                        &PointSolveOptions::default(),
                        interval.evaluate(0.5),
                    )?;
                if found {
                    let t2 = solution.start.param(0)?;
                    let uv2 = domain.clamp_point(tangent_drop(
                        k,
                        eval_surface,
                        uv,
                        self.curve
                            .ok_or(SolidError::null_pointer(OP))?
                            .evaluate_point(k, t2)?,
                    )?);
                    guesses.push((uv2, t2));
                }
            } else {
                uv = domain.evaluate(0.5, 0.5);
                t = interval.min();
                uv = tangent_drop(
                    k,
                    eval_surface,
                    uv,
                    self.curve
                        .ok_or(SolidError::null_pointer(OP))?
                        .evaluate_point(k, t)?,
                )?;
                if !domain.contains_point(uv, 0.0) {
                    uv = domain.evaluate(0.5, 0.5);
                    t = interval.max();
                    uv = tangent_drop(
                        k,
                        eval_surface,
                        uv,
                        self.curve
                            .ok_or(SolidError::null_pointer(OP))?
                            .evaluate_point(k, t)?,
                    )?;
                    if !domain.contains_point(uv, 0.0) {
                        t = interval.evaluate(0.5);
                        uv = domain.evaluate(0.5, 0.5);
                        uv = domain.clamp_point(tangent_drop(
                            k,
                            eval_surface,
                            uv,
                            self.curve
                                .ok_or(SolidError::null_pointer(OP))?
                                .evaluate_point(k, t)?,
                        )?);
                        for fraction in [0.5, 0.0, 1.0] {
                            guesses.push((domain.evaluate(0.5, 0.5), interval.evaluate(fraction)));
                        }
                    }
                }
            }
        }
        if let Some(first) = guesses.first_mut() {
            *first = (uv, t);
        }
        if self.found_coincidence {
            guesses.clear();
        }
        let mut found_intersection = false;
        let mut deviation = 0.0;
        let mut index = 0;
        while let Some(&(guess_uv, guess_t)) = guesses.get(index) {
            let result = intersect_surface_curve(
                k,
                eval_surface,
                &domain,
                self.curve.ok_or(SolidError::null_pointer(OP))?,
                &interval,
                self.base.tolerance_3d,
                guess_uv,
                guess_t,
            )?;
            found_intersection = result.is_some();
            if let Some(hit) = result {
                deviation = hit.deviation;
                let solution = self.solution(branch, hit.parameter, hit.uv, hit.deviation)?;
                if !filter_solution(
                    k,
                    eval_surface,
                    &domain,
                    self.curve.ok_or(SolidError::null_pointer(OP))?,
                    &interval,
                    conts,
                    &solution,
                )? {
                    index += 1;
                    continue;
                }
                if super::p1::is_point_valid_with(
                    k,
                    self.base.tree(1)?.owner(),
                    self.surface_cache.ok_or(SolidError::null_pointer(OP))?,
                    hit.uv,
                    None,
                    self.face_queries,
                )? {
                    self.add_sorted_solution(&solution, SortKey::ByAllParameters, true)?;
                }
                if super::p2::cs_are_perpen_disjoint_in(
                    k,
                    curve_tree,
                    &curve_node,
                    &surface_node,
                    self.surface_cache.ok_or(SolidError::null_pointer(OP))?,
                    surface_id,
                )? {
                    return Ok(false);
                }
                if index < 1 {
                    let curve = self.curve.ok_or(SolidError::null_pointer(OP))?;
                    if let Some(new_guess) = ignored(nearby_intersection(
                        k,
                        eval_surface,
                        &domain,
                        curve,
                        &interval,
                        hit.uv,
                        hit.parameter,
                        self.curve_edge_tolerance(curve)?,
                    ))?
                    .flatten()
                    {
                        if guesses.len() >= 6 {
                            return Err(SolidError::invalid_input(
                                OP,
                                "intersection guess limit exceeded",
                            ));
                        }
                        guesses.push(new_guess);
                    }
                }
            }
            index += 1;
        }
        for j in 0..2 {
            let t_cont = *t_conts.get(j).ok_or(SolidError::null_pointer(OP))?;
            if (t_cont as i32) <= ContinuityType::C0 as i32 && !self.found_coincidence {
                let t = if j == 0 {
                    interval.min()
                } else {
                    interval.max()
                };
                let point = self
                    .curve
                    .ok_or(SolidError::null_pointer(OP))?
                    .evaluate_point(k, t)?;
                if pseudo_box.distance_to_point(point) < EFF_ZERO {
                    let (found, mut solution) = surface_point_solve::<O>(
                        self.kernel,
                        parent_surface,
                        &natural_domain,
                        point,
                        domain.evaluate(0.5, 0.5),
                    )?;
                    if found && solution.start.solution_value < self.base.tolerance_3d {
                        let uv = Point2::new(solution.start.param(0)?, solution.start.param(1)?);
                        let value = solution.start.solution_value;
                        solution = self.solution(branch, t, uv, value)?;
                        self.keep(
                            k,
                            branch,
                            parent_surface,
                            &natural_domain,
                            &interval,
                            conts,
                            &solution,
                        )?;
                    }
                }
            }
            if !self
                .surface_cache
                .ok_or(SolidError::null_pointer(OP))?
                .process_boundary_curves
            {
                continue;
            }
            for (dir, cont, constant, iso_interval) in [
                (
                    SurfParamType::U,
                    u_conts.get(j),
                    if j == 0 {
                        domain.min().x
                    } else {
                        domain.max().x
                    },
                    domain.v_interval(),
                ),
                (
                    SurfParamType::V,
                    v_conts.get(j),
                    if j == 0 {
                        domain.min().y
                    } else {
                        domain.max().y
                    },
                    domain.u_interval(),
                ),
            ] {
                if cont.is_none_or(|c| (*c as i32) > ContinuityType::C0 as i32) {
                    continue;
                }
                let iso = implicit_iso(k, eval_surface, dir, constant, j == 0)?;
                if iso.is_degenerate(k, EFF_ZERO, None)? {
                    continue;
                }
                let coincident = self
                    .curve
                    .ok_or(SolidError::null_pointer(OP))?
                    .simple_coincidence_checker::<O>(
                        self.kernel,
                        &interval,
                        &iso,
                        &iso_interval,
                        self.base.tolerance_3d,
                        Some(&span.pseudo_box),
                        Some(&patch.pseudo_box),
                        [None, None],
                    )?;
                let count = coincident.len();
                let mut max_error = 0.0;
                let mut found_coincidence = false;
                for mut solution in coincident {
                    solution.num_variables = 3;
                    let other_t = solution.start.param(1)?;
                    let uv = if dir == SurfParamType::U {
                        Point2::new(constant, other_t)
                    } else {
                        Point2::new(other_t, constant)
                    };
                    let t = solution.start.param(0)?;
                    let value = solution.start.solution_value;
                    let mut adapted = self.solution(branch, t, uv, value)?;
                    adapted.solution_type = solution.solution_type;
                    if found_intersection
                        && (if dir == SurfParamType::U {
                            value + EFF_ZERO > deviation
                        } else {
                            value > deviation + EFF_ZERO
                        })
                    {
                        continue;
                    }
                    max_error = max2(max_error, value);
                    if solution.solution_type == SolutionType::RangeOfValues {
                        found_coincidence = true;
                        let end_other = solution.end.param(1)?;
                        let end_uv = if dir == SurfParamType::U {
                            Point2::new(constant, end_other)
                        } else {
                            Point2::new(end_other, constant)
                        };
                        let end_t = solution.end.param(0)?;
                        for (slot, value) in solution
                            .end
                            .parameters
                            .iter_mut()
                            .zip([end_t, end_uv.x, end_uv.y])
                        {
                            *slot = value;
                        }
                        adapted.end = solution.end;
                        self.add_sorted_solution(&adapted, SortKey::ByAllParameters, true)?;
                    } else {
                        self.keep(k, branch, eval_surface, &domain, &interval, conts, &adapted)?;
                    }
                }
                if found_coincidence {
                    return Ok(false);
                }
                let zero = if dir == SurfParamType::U {
                    EFF_ZERO
                } else {
                    scaled_zero
                };
                if max_error > zero || count == 0 {
                    if let Some(hit) = local_curve_intersect(
                        k,
                        &iso,
                        &iso_interval,
                        self.curve.ok_or(SolidError::null_pointer(OP))?,
                        &interval,
                        self.base.tolerance_3d,
                        iso_interval.evaluate(0.5),
                        interval.evaluate(0.5),
                    )? {
                        let uv = if dir == SurfParamType::U {
                            Point2::new(constant, hit.my_param)
                        } else {
                            Point2::new(hit.my_param, constant)
                        };
                        let solution = self.solution(branch, hit.other_param, uv, hit.deviation)?;
                        self.keep(
                            k,
                            branch,
                            eval_surface,
                            &domain,
                            &interval,
                            conts,
                            &solution,
                        )?;
                    }
                }
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plane() -> Surface {
        let net = (0..3)
            .map(|i| {
                (0..3)
                    .map(|j| CPoint::new(f64::from(i) / 2.0, f64::from(j) / 2.0, 0.0, NOW))
                    .collect()
            })
            .collect();
        let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let n = crate::nurbs::srf_basic::Surface::new(net, 2, 2, knots.clone(), knots).unwrap();
        Surface::from(BSplineSurface::from_nurb(n, false))
    }
    #[test]
    fn plane_splits_and_natural_boundaries() {
        let s = plane();
        let k = Kernel::default();
        let domain = s.natural_uv_domain_if_built();
        for direction in [SurfParamType::U, SurfParamType::V] {
            assert_eq!(
                s.find_tessellation_splits(&k, direction, &domain, 0.01, 10.0, 0.0, false)
                    .unwrap(),
                vec![0.0, 1.0]
            );
            let (a, b) = s
                .create_iso_boundaries(&k, direction, 0.01)
                .unwrap()
                .unwrap();
            let a = a.evaluate_point(&k, 0.5).unwrap();
            let b = b.evaluate_point(&k, 0.5).unwrap();
            if direction == SurfParamType::U {
                assert_eq!((a.x, b.x), (0.0, 1.0));
            } else {
                assert_eq!((a.y, b.y), (0.0, 1.0));
            }
        }
        assert!(s
            .create_iso_boundaries(&k, SurfParamType::Unknown, 0.01)
            .unwrap()
            .is_none());
    }
    #[test]
    fn excessive_split_count_returns_error_before_allocating() {
        let s = plane();
        let k = Kernel::default();
        assert!(s
            .find_tessellation_splits(
                &k,
                SurfParamType::U,
                &s.natural_uv_domain_if_built(),
                0.01,
                0.0,
                1e-100,
                false
            )
            .is_err());
    }
}
