//! The surface silhouette tracer: the curve definition (point values, local
//! refinement, boundary start points, interior branch culling) that the surface
//! tracer marches to find the silhouette curves of a surface for a parallel view
//! direction or a perspective eye point.
//!
//! Upstream: `source/SMLib/src/SmSurfaceSilhouette.cpp`, `source/SMLib/inc/SmSurfaceSilhouette.h`.
//!
//! | Rust                                                      | Upstream                                           |
//! |-----------------------------------------------------------|----------------------------------------------------|
//! | [`SurfaceSilhouette`], [`SurfaceSilhouette::new`]         | `SmSurfaceSilhouette`, `SmSurfaceSilhouette(cpSurface, crUVDomain, bPerspective, crEye)` |
//! | `Drop` (implicit)                                         | `~SmSurfaceSilhouette`                             |
//! | [`SurfaceSilhouette::does_point_lie_on_curve`]            | `SmSurfaceSilhouette::DoesPointLieOnCurve`         |
//! | [`SurfaceSilhouette::find_solutions_on_boundary_curve`]   | `SmSurfaceSilhouette::FindSolutionsOnBoundaryCurve` |
//! | [`FindSilSingEfo`], [`FindSilSingEfo::new`]               | `SmFindSilSingEFO`, its constructor (file-local class) |
//! | [`FindSilSingEfo::evaluate`] ([`EvalFunction`])           | `SmFindSilSingEFO::Evaluate`                       |
//! | [`SurfaceSilhouette::compute_point_values`]               | `SmSurfaceSilhouette::ComputePointValues`          |
//! | [`SurfaceSilhouette::local_point_solve`]                  | `SmSurfaceSilhouette::LocalPointSolve`             |
//! | [`SurfaceSilhouette::branch_may_contain_answers`], [`SurfaceSilhouette::patch_may_contain_answers`] | `SmSurfaceSilhouette::BranchMayContainAnswers` |
//! | [`TYPE`], [`TYPE_STRING`], [`is_kind_of`]                 | `GetType`, `GetTypeString`, `IsKindOf` (`SM_COMMON`) |
//! | [`SurfaceTrace`] for [`SurfaceSilhouette`]                | the virtual overrides of `SmSurfaceTracer`         |
//!
//! Deviations:
//! - **Virtual methods.** The silhouette owns a [`SurfaceTracer`] (the base members)
//!   and implements [`SurfaceTrace`], overriding `ComputePointValues`,
//!   `LocalPointSolve`, `FindSolutionsOnBoundaryCurve`, `DoesPointLieOnCurve` and the
//!   global solver's `BranchMayContainAnswers`; the bodies are inherent methods.
//! - **Out-params** become return values: `LocalPointSolve`'s found flag and point
//!   are an `Option<Point2>`; the unused `crNextPnt`, `pdStepSize`, `lSide` and
//!   `eSurfParam` stay in the trait signatures only.
//! - **Statuses as booleans.** `DoesPointLieOnCurve` and `BranchMayContainAnswers`
//!   return `SmBoolean` but leave through `SER`/`NER` with a failure status, which
//!   reads as `TRUE`; the port answers `true` there (an [`SolidError::Unported`] is
//!   propagated instead). The ignored statuses (`SE`, `Unitize`) stay ignored;
//!   `Unitize` of a zero vector gives `(0, 0, 1)`.
//! - **Branch data.** `BranchMayContainAnswers` reads the `SmBezierPatch` of the
//!   surface node from the cache that owns the tree (passed to
//!   [`SurfaceTrace::branch_may_contain_answers`]); `GetPolarBox` builds a missing
//!   polar box from its nearest stored ancestor without storing the chain (the
//!   global solve only borrows the cache): the box values are the same. A node
//!   without patch slot is upstream's null `m_pData` (`NER`: `true`).
//! - **Evaluations.** `SmSurface::Evaluate` writes only the upper half of the
//!   `[3][3]` matrix; the other entries keep what the trace point held, as upstream.
//!   `smsurf_EvaluateGeometric` repairs a vanishing `S_u`/`S_v` of its matrix in
//!   place: upstream that also writes the previous point (`pPrevPnt`), the port
//!   evaluates a copy of it (the trait passes `&TracePoint`); nothing here reads the
//!   previous point's derivatives afterwards.
//! - **Boundary solutions** name the boundary curve as
//!   [`BoundaryCurve`] (with its natural interval) where upstream stores the curve
//!   pointer.
//! - **Unported callees** return [`SolidError::Unported`] (see the `UNPORTED`
//!   markers): none left for `surface.surface_p3`.
//! - The `SM_DEBUG_CODE` drawing blocks and `Dump` are not ported. Recursion:
//!   `ComputePointValues` calls itself only without a previous point, which does not
//!   recurse again (depth at most 1); the Brent loop of `LocalPointSolve` stops after
//!   101 iterations as upstream.

use crate::curve::curve::p2::CurvePropertyType;
use crate::curve::curve::p7::PropertyAnalysisOptions;
use crate::curve::curve::Curve;
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::circle_find_silhouette_points;
use crate::geom::global_solver::{Branch, GlobalSolver};
use crate::geom::local_solve1d::{EvalFunction, FunctionValue, LocalSolve1d};
use crate::geom::polar_box::PolarBox;
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::solution_array::{SolutionArray, SolutionObject};
use crate::geom::tol::are_same;
use crate::geom::tree::{AuxDataType, NodeData, SurfaceAux};
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::intersect::surface_tracer::{
    BoundaryCurve, SurfaceTrace, SurfaceTracer, TracePoint, TracePointType,
};
use crate::kernel::Kernel;
use crate::math::{BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::nurbs::knots::Side;
use crate::surface::curve_bounded_surface::NormalSection;
use crate::surface::surface::p2::{drop_vectors, evaluate_geometric, GeometricProperties};
use crate::surface::surface::p7::drop_vectors_at;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::surface::surface_cache::SurfaceCache;

/// `SmSurfaceSilhouette_TYPE` (`SOLV_BASE_TYPE + 58`).
pub const TYPE: i64 = 19_058;
/// `"SmSurfaceSilhouette_TYPE"`.
pub const TYPE_STRING: &str = "SmSurfaceSilhouette_TYPE";

const OP_CPV: &str = "intersect::surface_silhouette::SurfaceSilhouette::compute_point_values";
const OP_DROP: &str = "intersect::surface_silhouette::SurfaceSilhouette::local_point_solve";
const OP_EFO: &str = "intersect::surface_silhouette::FindSilSingEfo::evaluate";

/// `SmSurfaceSilhouette::IsKindOf`: the silhouette type or a base type
/// (`SmSurfaceTracer::IsKindOf`).
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    t == TYPE || crate::intersect::surface_tracer::is_kind_of(t)
}

/// `Evaluate(uv, 2, 2, TRUE, TRUE, TRUE, ..)` into the upper half of `eval`; the
/// other entries are left as they are.
fn evaluate_upper_2(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    eval: &mut [[Vec3; 3]; 3],
) -> Result<()> {
    let flags = EvaluateFlags {
        u_side: Side::Left,
        v_side: Side::Left,
        only_upper_half: true,
        non_zero_tangents: true,
    };
    let d = surface.evaluate(k, uv, 2, 2, flags)?;
    for (i, row) in eval.iter_mut().enumerate() {
        for (j, slot) in row.iter_mut().enumerate() {
            if i + j <= 2 {
                if let Some(v) = d.get(i).and_then(|r| r.get(j)) {
                    *slot = *v;
                }
            }
        }
    }
    Ok(())
}

/// `SmSurface::EvaluateNormal(uv, TRUE, TRUE, ..)` (virtual).
fn evaluate_normal(k: &Kernel, surface: &Surface, uv: Point2) -> Result<Vec3> {
    surface.evaluate_normal(k, uv, Side::Left, Side::Left)
}

/// `SmSurface::EvaluateGeometric(uv, TRUE, TRUE, ..)` (no plane override).
fn evaluate_geometric_at(k: &Kernel, surface: &Surface, uv: Point2) -> Result<GeometricProperties> {
    match surface.kind() {
        SurfaceKind::CurveBounded(s) => s.evaluate_geometric(k, uv, Side::Left, Side::Left),

        SurfaceKind::Cone(_)
        | SurfaceKind::Sphere(_)
        | SurfaceKind::Torus(_)
        | SurfaceKind::BSpline(_)
        | SurfaceKind::Plane(_)
        | SurfaceKind::SurfOfRevolution(_)
        | SurfaceKind::SurfOfExtrusion(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => surface.evaluate_geometric(k, uv, Side::Left, Side::Left),
    }
}

/// `SmSurface::EvaluateNormalSection(uv, TRUE, TRUE, direction, ..)` (no plane
/// override).
fn evaluate_normal_section(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    direction: Vec3,
) -> Result<NormalSection> {
    match surface.kind() {
        SurfaceKind::CurveBounded(s) => {
            s.evaluate_normal_section(k, uv, Side::Left, Side::Left, direction)
        }

        SurfaceKind::Cone(_)
        | SurfaceKind::Sphere(_)
        | SurfaceKind::Torus(_)
        | SurfaceKind::BSpline(_)
        | SurfaceKind::Plane(_)
        | SurfaceKind::SurfOfRevolution(_)
        | SurfaceKind::SurfOfExtrusion(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => {
            let n = surface.evaluate_normal_section(k, uv, Side::Left, Side::Left, direction)?;
            Ok(NormalSection {
                tangent_plane_direction: n.tangent,
                curvature: n.curvature,
                normal: n.normal,
            })
        }
    }
}

/// `SmSurface::DropVectors(uv, TRUE, TRUE, 1, &vector, &uv_vector)` (no override).
fn surface_drop_vector(k: &Kernel, surface: &Surface, uv: Point2, vector: Vec3) -> Result<Vec2> {
    drop_vectors_at(k, surface, uv, Side::Left, Side::Left, &[vector])?
        .first()
        .copied()
        .ok_or(SolidError::sm_err(OP_DROP))
}

/// `SM_PI - a` for angles past a right angle: parallel and anti-parallel are alike.
fn fold_angle(a: f64) -> f64 {
    if a > PI / 2.0 {
        PI - a
    } else {
        a
    }
}

/// The silhouette tracer (`SmSurfaceSilhouette`): the curves of a surface where the
/// normal is perpendicular to the view direction (parallel projection) or to the
/// line to the eye point (perspective projection).
#[derive(Debug, Clone)]
pub struct SurfaceSilhouette<'s> {
    /// Caller context for virtual evaluations.
    pub kernel: &'s Kernel,

    /// The `SmSurfaceTracer` base members.
    pub base: SurfaceTracer<'s>,
    /// `m_bPerspective`: `eye` is an eye point (else a view direction).
    pub perspective: bool,
    /// `m_vEye`: the eye point, or the unit view direction.
    pub eye: Point3,
}

impl<'s> SurfaceSilhouette<'s> {
    /// `SmSurfaceSilhouette(cpSurface, crUVDomain, bPerspective, crEye)`: the base
    /// tracer over `uv_domain`; a view direction is unitized (a zero one becomes
    /// `(0, 0, 1)`; upstream only asserts it is non-zero).
    ///
    /// # Errors
    ///
    /// Those of [`SurfaceTracer::new`].
    pub fn new(
        k: &'s Kernel,
        surface: &'s Surface,
        uv_domain: &Extent2d,
        perspective: bool,
        eye: Vec3,
    ) -> Result<Self> {
        let base = SurfaceTracer::new(k, surface, uv_domain)?;
        let mut eye = eye;
        if !perspective {
            eye = eye.unitize_or_z();
        }
        Ok(Self {
            kernel: k,
            base,
            perspective,
            eye,
        })
    }

    /// `m_cpSurface`.
    fn surface(&self) -> &'s Surface {
        self.base.surface
    }

    /// `DoesPointLieOnCurve`: without `refine`, whether the normal at `uv` is within
    /// `sqrt(SM_EFF_ZERO)` of perpendicular to the view vector; with `refine`,
    /// whether [`Self::local_point_solve`] finds a silhouette point from `uv`, which
    /// replaces `uv` when its function value is smaller.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the surface evaluations; their other failures
    /// answer `true` (upstream returns the failure status as its boolean).
    pub fn does_point_lie_on_curve(
        &self,
        k: &Kernel,
        uv: &mut Point2,
        refine: bool,
    ) -> Result<bool> {
        let surface = self.surface();
        let normal = match evaluate_normal(k, surface, *uv) {
            Ok(n) => n,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Ok(true),
        };
        let vec = if self.perspective {
            let pnt = match surface.evaluate_point(k, *uv) {
                Ok(p) => p,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => return Ok(true),
            };
            (pnt - self.eye).unitize_or_z()
        } else {
            self.eye
        };
        if !refine {
            let dot = vec.dot(normal);
            return Ok(dot.abs() < EFF_ZERO_SQRT);
        }

        // Refine the point some
        let pnt1 = TracePoint::new();
        let found = ignored(self.local_point_solve(k, &pnt1, *uv))?.flatten();
        if let Some(found_uv) = found {
            let normal2 = match evaluate_normal(k, surface, found_uv) {
                Ok(n) => n,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => return Ok(true),
            };
            if self.perspective {
                let pnt = match surface.evaluate_point(k, found_uv) {
                    Ok(p) => p,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => return Ok(true),
                };
                let vec2 = pnt - self.eye;
                if vec2.dot(normal2).abs() < vec.dot(normal).abs() {
                    *uv = found_uv;
                }
            } else if vec.dot(normal2).abs() < vec.dot(normal).abs() {
                *uv = found_uv;
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// `FindSolutionsOnBoundaryCurve`: approximate start points on the boundary iso
    /// curve over `interval`, the silhouette-vector (parallel) or silhouette-point
    /// (perspective) solutions of `GlobalPropertyAnalysis` within the approximation
    /// tolerance.
    ///
    /// # Errors
    ///
    /// Those of [`Curve::global_property_analysis`].
    pub fn find_solutions_on_boundary_curve(
        &self,
        k: &Kernel,
        boundary: &Curve,
        interval: &Extent1d,
    ) -> Result<SolutionArray<BoundaryCurve>> {
        let curve_prop = if self.perspective {
            CurvePropertyType::SilhouettePoint
        } else {
            CurvePropertyType::SilhouetteVector
        };
        let eye = [self.eye];
        let mut opts = PropertyAnalysisOptions {
            property_value: None,
            opt_vectors: &eye,
            want_comb: false,
            handle: None,
        };
        let curve = boundary;
        // the solutions name the curve (its cache tree's owner)
        opts.handle = Some(BoundaryCurve {
            natural_interval: Some(curve.natural_interval()),
        });
        let analysis = curve.global_property_analysis(
            k,
            interval,
            curve_prop,
            self.base.approx_tol_3d,
            &opts,
        )?;
        Ok(analysis.solutions)
    }

    /// `ComputePointValues`: fills `pnt` with the surface point and derivatives at
    /// `uv` (the unit normal in `[2][2]`), the 3D position and the unit silhouette
    /// tangent (the zero-gradient direction of `R . (S_u x S_v)`, flipped to follow
    /// `prev`), and their UV counterparts. Near a singularity (a principal direction
    /// along the view vector with zero curvature) the point is moved onto it, marked
    /// [`TracePointType::Singularity`] and given the previous (or the perpendicular
    /// principal) direction.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for an eye point on the surface, a normal that cannot be found or a
    /// zero tangent without previous point; those of the evaluations, of
    /// `smsurf_EvaluateGeometric`, `AngleBetween` and `smsurf_DropVectors`.
    pub fn compute_point_values(
        &self,
        k: &Kernel,
        uv: Point2,
        pnt: &mut TracePoint,
        prev: Option<&TracePoint>,
    ) -> Result<()> {
        let surface = self.surface();
        let ang_tol = self.base.ang_tol_rad;

        // Evaluate the surface, 2 derivs, into the surface matrix.
        evaluate_upper_2(k, surface, uv, &mut pnt.surface)?;

        // Store a unitized surface normal in [2][2].
        let mut norm = pnt.surface[1][0].cross(pnt.surface[0][1]);
        if norm.length_squared() < EFF_ZERO_SQ {
            norm = evaluate_normal(k, surface, uv)?;
            if norm.length_squared() < EFF_ZERO_SQ {
                return Err(SolidError::sm_err(OP_CPV));
            }
            if pnt.surface[1][0].length_squared() < EFF_ZERO_SQ {
                pnt.surface[1][0] = pnt.surface[0][1].cross(norm);
            }
            if pnt.surface[0][1].length_squared() < EFF_ZERO_SQ {
                pnt.surface[0][1] = norm.cross(pnt.surface[1][0]);
            }
        }
        let norm = norm.unitize_or_z();
        pnt.surface[2][2] = norm;

        pnt.curve_3d[0] = pnt.surface[0][0];

        // The silhouette curve is the locus where F(u,v) = R . N(u,v) = 0 (N = Su x Sv,
        // not normalized); its direction is the zero-gradient direction
        // (du,dv) = ( R . dN/dv, - R . dN/du ).
        let mut r = self.eye;
        if self.perspective {
            r = pnt.surface[0][0] - self.eye;
        }

        if r.length_squared() < EFF_ZERO {
            return Err(SolidError::sm_err(OP_CPV)); // point on surface must be at the eye point
        }

        let du = pnt.surface[1][0];
        let duu = pnt.surface[2][0];
        let dv = pnt.surface[0][1];
        let dvv = pnt.surface[0][2];
        let duv = pnt.surface[1][1];

        // Check for being near a singularity in tangency: one of the principal
        // directions lines up with the view vector, with a zero principal curvature.
        let geom = evaluate_geometric(&mut pnt.surface)?;
        let gauss_k = geom.gaussian_curvature;
        let prin_k1 = geom.principle_curvature1;
        let prin_k2 = geom.principle_curvature2;
        let prin_dir1 = geom.principle_curvature_vector1;
        let prin_dir2 = geom.principle_curvature_vector2;

        // We don't care about parallel vs. anti-parallel:
        let ang1 = fold_angle(r.angle_between(prin_dir1)?);
        let ang2 = fold_angle(r.angle_between(prin_dir2)?);

        // Check for singularity.
        // If we have a prevPoint, check it.
        if let Some(prev) = prev {
            let mut prev_eval = prev.surface;
            let prev_geom = evaluate_geometric(&mut prev_eval)?;
            let prev_gauss_k = prev_geom.gaussian_curvature;
            // (upstream overwrites the principal curvatures with the previous point's)
            let prin_k1 = prev_geom.principle_curvature1;
            let prin_k2 = prev_geom.principle_curvature2;
            let prev_prin_dir1 = prev_geom.principle_curvature_vector1;
            let prev_prin_dir2 = prev_geom.principle_curvature_vector2;

            // 1: singularity in 1st principle dir, 2 in 2nd.
            let mut singularity = 0;
            if prin_k1.abs() < EFF_ZERO_SQRT && ang1.abs() < EFF_ZERO_SQRT {
                singularity = 1;
            } else if prin_k2.abs() < EFF_ZERO_SQRT && ang2.abs() < EFF_ZERO_SQRT {
                singularity = 2;
            }

            if singularity > 0 {
                // if the gaussian curvature changes sign and the smaller angle is the
                // one which has the vanishing curvature.
                if gauss_k.abs() > EFF_ZERO_SQRT
                    && prev_gauss_k.abs() > EFF_ZERO_SQRT
                    && gauss_k * prev_gauss_k < 0.0
                    && ((ang1 < ang2 && prin_k1.abs() < prin_k2.abs())
                        || (ang1 > ang2 && prin_k1.abs() > prin_k2.abs()))
                {
                    // Here we need to minimize GAUSS between surface points.
                    let prev_uv = prev.curve_uv[0];
                    let mut efo = FindSilSingEfo::new(
                        k,
                        surface,
                        prev_uv,
                        uv,
                        true,
                        self.perspective,
                        self.eye,
                    );
                    let ivl = Extent1d::from_min_max(0.0, 1.0);
                    let mut ls = LocalSolve1d::with_interval(ivl, false);
                    match ls.solve_it(&mut efo, 0.5, 1.0e-8) {
                        Err(e) if e.is_unported() => return Err(e),
                        Err(_) => {}
                        Ok(outcome) => {
                            // Found an answer in the solver -- use that new parameter
                            // and recompute the point values.
                            let s_uv = prev_uv + outcome.t * (uv - prev_uv);
                            let g2 = evaluate_geometric_at(k, surface, s_uv)?;

                            let mut r2 = r;
                            if self.perspective {
                                let srf_pnt = surface.evaluate_point(k, s_uv)?;
                                r2 = srf_pnt - self.eye;
                            }

                            // If the angle between the two vectors is very small or
                            // very large then they are parallel.
                            let angle1 =
                                fold_angle(r2.angle_between(g2.principle_curvature_vector1)?);
                            let angle2 =
                                fold_angle(r2.angle_between(g2.principle_curvature_vector2)?);
                            // one curvature vector exactly parallel to the view vector
                            // and the other exactly perpendicular
                            if angle1 < ang_tol / 100.0 || angle2 < ang_tol / 100.0 {
                                pnt.curve_3d[1] = prev.curve_3d[1];
                                pnt.point_type = TracePointType::Singularity;
                                self.compute_point_values(k, s_uv, pnt, None)?;
                                return Ok(());
                            }
                        }
                    }
                }
                // Parabolic point: find where one of the principal directions lines
                // up with R, by solving for the other one being perpendicular to R.
                else if gauss_k.abs() < EFF_ZERO_SQRT
                    && ((PI / 2.0 - ang1).abs() < ang_tol * 4.0
                        || (PI / 2.0 - ang2).abs() < ang_tol * 4.0)
                {
                    let curr_dot = if singularity == 1 {
                        r.dot(prin_dir2)
                    } else {
                        r.dot(prin_dir1)
                    };

                    // Eye vec for previous point could be different, if perspective.
                    let mut r_prev = self.eye;
                    if self.perspective {
                        r_prev = prev.surface[0][0] - self.eye;
                    }
                    let prev_dot = if singularity == 1 {
                        r_prev.dot(prev_prin_dir2)
                    } else {
                        r_prev.dot(prev_prin_dir1)
                    };

                    if curr_dot.abs() < EFF_ZERO {
                        // Hit the singularity directly - no need to use solver
                        pnt.point_type = TracePointType::Singularity;
                        pnt.curve_3d[1] = prev.curve_3d[1];
                        // simple evaluation without singularity checking
                        self.compute_point_values(k, uv, pnt, None)?;
                        return Ok(());
                    }

                    // Minimize the dot between R and one of the principal directions,
                    // for T with (T==0) the previous uv and (T==1) `uv`, allowing
                    // convergence outside of [0,1].
                    let prev_uv = prev.curve_uv[0];
                    let mut efo = FindSilSingEfo::new(
                        k,
                        surface,
                        prev_uv,
                        uv,
                        false,
                        self.perspective,
                        self.eye,
                    );
                    let mut ivl = Extent1d::from_min_max(-5.0, 5.0);
                    let mut ls = LocalSolve1d::with_interval(ivl, false);
                    // Guess param: linearly interpolate between PrevDot (at t==0) and
                    // CurrDot (at t==1) to where Dot would be zero.
                    let mut guess_t = 0.5; // in case we can't get a guess
                    let denom = prev_dot - curr_dot;
                    if denom.abs() > prev_dot.abs() * EFF_ZERO_SQRT {
                        guess_t = prev_dot / denom;
                        // Make sure it's within the interval so SolveIt doesn't complain
                        // (the solver keeps its own copy of the interval).
                        if guess_t.abs() < 10.0 {
                            ivl.add_value(guess_t);
                        }
                        guess_t = ivl.clamp_value(guess_t);
                    }

                    match ls.solve_it(&mut efo, guess_t, 1.0e-8) {
                        Err(e) if e.is_unported() => return Err(e),
                        Err(_) => {}
                        Ok(outcome) => {
                            if ls.found_accuracy().abs() > EFF_ZERO_SQRT {
                                // Didn't find a good solution: temporarily indicate a
                                // singularity, which disables the setting of the curve
                                // tangent in the recursive evaluation.
                                pnt.point_type = TracePointType::Singularity;
                                pnt.curve_3d[1] = prev.curve_3d[1];
                                self.compute_point_values(k, uv, pnt, None)?;
                                pnt.point_type = TracePointType::Normal;
                            } else {
                                // Found the singular point: interpolate from the
                                // previous uv to `uv`.
                                let s_uv = prev_uv + outcome.t * (uv - prev_uv);
                                pnt.point_type = TracePointType::Singularity;

                                // Set the tangent to the view direction if it is pretty
                                // close already, keeping the previous tangent's
                                // magnitude.
                                let prev_tan = prev.curve_3d[1];
                                let tan_mag = prev_tan.length();

                                pnt.curve_3d[1] = prev_tan; // default

                                // Update the view vector, in case the point has moved.
                                let mut r = self.eye;
                                if self.perspective {
                                    r = pnt.surface[0][0] - self.eye;
                                }
                                let dot = prev_tan.dot(r);
                                if dot > 0.995 {
                                    pnt.curve_3d[1] = r * tan_mag;
                                } else if dot < -0.995 {
                                    pnt.curve_3d[1] = -r * tan_mag;
                                }
                                self.compute_point_values(k, s_uv, pnt, None)?;
                            }
                            return Ok(());
                        }
                    }
                }
            }
        } else {
            // No previous point. Check for singularity: there would be two
            // solutions, one parallel to the view direction and one perpendicular.
            // Use the one perpendicular. [B100]
            if (ang1 < ang_tol / 100.0 && prin_k1.abs() < EFF_ZERO_SQRT)
                || (ang2 < ang_tol / 100.0 && prin_k2.abs() < EFF_ZERO_SQRT)
            {
                pnt.point_type = TracePointType::Singularity; // so we don't overwrite this:
                pnt.curve_3d[1] = if ang1 > ang2 { prin_dir1 } else { prin_dir2 };
            }
        }

        if pnt.point_type != TracePointType::Singularity {
            // The normal (nonsingular) case: the 3d direction from the gradient
            // formula. Derivatives of the Normal:
            let dndu = duu.cross(dv) + du.cross(duv);
            let dndv = duv.cross(dv) + du.cross(dvv);

            // The zero-gradient direction:
            let uv_vec = Vec2::new(r.dot(dndv), -r.dot(dndu));

            // The 3D tangent vector from the 2D tangent and the cross derivatives.
            let mut tan_vec = du * uv_vec.x + dv * uv_vec.y;
            tan_vec *= self.base.curve_trace_direction;

            // Flip to align (roughly) with the previous point's tangent.
            if let Some(prev) = prev {
                if tan_vec.dot(prev.curve_3d[1]) < 0.0 {
                    tan_vec = -tan_vec;
                }
            }

            // Set curve tangent to tan_vec, if it's not zero.
            if tan_vec.length_squared() > EFF_ZERO_SQ {
                pnt.curve_3d[1] = tan_vec.unitize_or_z();
            } else if let Some(prev) = prev {
                // If we have a previous point use it otherwise we are in trouble
                pnt.curve_3d[1] = prev.curve_3d[1];
            } else {
                let tan_len = tan_vec.length();
                if tan_len > EFF_ZERO_SQ {
                    // (operator/, not the guarded operator/=)
                    pnt.curve_3d[1] = (tan_vec / tan_len).unitize_or_z();
                } else {
                    return Err(SolidError::sm_err(OP_CPV));
                }
            }
        }

        // Now compute parameter space point and vector
        pnt.curve_uv[0] = uv;
        let dropped = drop_vectors(pnt.surface[1][0], pnt.surface[0][1], &[pnt.curve_3d[1]])?;
        if let Some(v) = dropped.first() {
            pnt.curve_uv[1] = *v;
        }
        Ok(())
    }

    /// `LocalPointSolve`: the silhouette point near `guess` (clamped to the domain),
    /// `None` when none was found: the guess itself when its function value
    /// `R . N` is below `100 SM_EFF_ZERO`; else a first step towards the closest
    /// silhouette point of the osculating circle of the normal section across the
    /// silhouette direction, refined by an inverse quadratic (Brent) iteration along
    /// that step. Steps on a domain boundary keep to the boundary.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when no normal can be found at the guess; those of the surface
    /// evaluations and of `smgu_CircleFindSilhouettePoints`.
    pub fn local_point_solve(
        &self,
        k: &Kernel,
        curr: &TracePoint,
        guess: Point2,
    ) -> Result<Option<Point2>> {
        const OP: &str = "intersect::surface_silhouette::SurfaceSilhouette::local_point_solve";
        let _ = (k, curr);
        let surface = self.surface();
        let domain = self.base.uv_domain;
        let approx_tol = self.base.approx_tol_3d;

        // If the guess point is outside of our domain, clamp it.
        let s_uv = domain.clamp_point(guess);

        // First check serendipity: evaluate the function at the guess (clamped), with
        // the two derivatives the main algorithm needs.
        let mut eval = [[Vec3::ZERO; 3]; 3];
        evaluate_upper_2(k, surface, s_uv, &mut eval)?;
        let mut normal = eval[1][0].cross(eval[0][1]);

        // If cross product is zero, try to get the normal another way, and then use
        // that to make sure that the derivatives are not zero.
        if normal.length_squared() < EFF_ZERO_SQ {
            normal = evaluate_normal(k, surface, s_uv)?;
            if normal.length_squared() < EFF_ZERO_SQ {
                return Err(SolidError::sm_err(OP));
            }
            if eval[1][0].length_squared() < EFF_ZERO_SQ {
                eval[1][0] = eval[0][1].cross(normal);
            }
            if eval[0][1].length_squared() < EFF_ZERO_SQ {
                eval[0][1] = normal.cross(eval[1][0]);
            }
        } else {
            normal = normal.unitize()?;
        }
        let mut r = self.eye;
        if self.perspective {
            r = eval[0][0] - self.eye;
        }
        let fcn_val = r.dot(normal);
        // Don't compare to the approximation tolerance: that can be big.
        if fcn_val.abs() < EFF_ZERO * 100.0 {
            return Ok(Some(s_uv));
        }
        // end serendipity check.

        // First check for case where Guess is on boundary
        let clamp_u = are_same(s_uv.x, domain.min().x) || are_same(s_uv.x, domain.max().x);
        let clamp_v = are_same(s_uv.y, domain.min().y) || are_same(s_uv.y, domain.max().y);

        // A plane perpendicular to the last tangent finds a point on the surface
        // where the silhouette exists.
        let du = eval[1][0];
        let duu = eval[2][0];
        let dv = eval[0][1];
        let dvv = eval[0][2];
        let duv = eval[1][1];

        // Derivatives of du*dv w.r.t. u and v:
        let dndu = duu.cross(dv) + du.cross(duv);
        let dndv = duv.cross(dv) + du.cross(dvv);

        // uv direction perpendicular to gradient of function
        let uv_vec = Vec2::new(r.dot(dndv), -r.dot(dndu));

        // Check to see if we hit the point directly
        if normal.dot(self.eye).abs() < EFF_ZERO && uv_vec.length_squared() > EFF_ZERO_SQ {
            return Ok(Some(s_uv));
        }

        // The 3D tangent vector from the 2D tangent and the cross derivatives.
        let tan_vec = du * uv_vec.x + dv * uv_vec.y;
        if tan_vec.length_squared() < EFF_ZERO_SQ {
            return Ok(None);
        }

        // The normal section of the surface in the plane perpendicular to the curve;
        // Newton on the theoretical curve it generates approaches the silhouette.
        let pnt = eval[0][0];

        let mut normal_plane_vec = tan_vec.cross(normal);
        let section = evaluate_normal_section(k, surface, s_uv, normal_plane_vec)?;
        let tangent_plane_dir = section.tangent_plane_direction;
        let mut curvature = section.curvature;
        normal = section.normal;

        // Find center and normal of circle
        if curvature.abs() < EFF_ZERO {
            // [B22 and 091023; cf. B100, B155]
            curvature = if curvature > 0.0 { EFF_ZERO } else { -EFF_ZERO };
        }
        let center = pnt + normal / curvature;
        let radius = (1.0 / curvature).abs();
        let circle_normal = tangent_plane_dir.cross(normal);

        // Find the closest silhouette point on the normal circle
        let Some((sil_point1, sil_point2)) = circle_find_silhouette_points(
            center,
            circle_normal,
            radius,
            self.perspective,
            self.eye,
        )?
        else {
            return Ok(None);
        };
        let mut found_point = sil_point1;
        if pnt.distance_squared(sil_point1) > pnt.distance_squared(sil_point2) {
            found_point = sil_point2;
        }

        // Now find the parametric vector corresponding to found_point.
        let mut step_3d = found_point - pnt;
        normal_plane_vec = normal_plane_vec.unitize()?;
        step_3d = normal_plane_vec * step_3d.length();

        let step_leng = step_3d.length_squared();
        if step_leng < (approx_tol / 10000.0) * (approx_tol / 10000.0) {
            return Ok(Some(s_uv));
        }
        // A small first step better controls Brent.
        if step_leng > approx_tol * approx_tol {
            step_3d = step_3d.unitize()?;
            step_3d *= approx_tol;
        }
        let mut uv_step = surface_drop_vector(k, surface, s_uv, step_3d)?;
        if clamp_u {
            uv_step.x = 0.0;
        }
        if clamp_v {
            uv_step.y = 0.0;
        }

        // Make sure we don't step too far on that first step or we might miss the
        // curve.
        let size = domain.size();
        if uv_step.x.abs() > size.x / 100.0 {
            uv_step = uv_step * (size.x / 100.0) / uv_step.x;
        }
        if uv_step.y.abs() > size.y / 100.0 {
            uv_step = uv_step * (size.y / 100.0) / uv_step.y;
        }

        // Brent's method along the step towards the root where the normal section
        // is a silhouette point.
        let mut a = 0.0;
        let mut b = 1.0;
        let mut c = 2.0;
        let mut fa = r.dot(normal);
        let mut uv_new1 = s_uv + uv_step;
        let mut uv_new2 = s_uv + uv_step * 2.0;
        if !domain.contains_point(uv_new1, EFF_ZERO) {
            c = domain.clip_line(s_uv, uv_step, 1.0);
            // BIG_DOUBLE in case of error.
            if c == BIG_DOUBLE || c.abs() < EFF_ZERO {
                return Ok(None);
            }

            uv_new2 = s_uv + uv_step * c;
            b = c / 2.0;
            uv_new1 = s_uv + uv_step * b;
        } else if !domain.contains_point(uv_new2, EFF_ZERO) {
            c = domain.clip_line(s_uv, uv_step, 2.0);
            if c == BIG_DOUBLE || c.abs() < EFF_ZERO {
                return Ok(None);
            }

            uv_new2 = s_uv + uv_step * c;
        }
        uv_new1 = domain.clamp_point(uv_new1);
        uv_new2 = domain.clamp_point(uv_new2);
        let normal1 = evaluate_normal(k, surface, uv_new1)?;
        let mut fb = r.dot(normal1);
        let normal2 = evaluate_normal(k, surface, uv_new2)?;
        let mut fc = r.dot(normal2);

        // First check to see if we found answer without any iteration.
        if fb.abs() < EFF_ZERO {
            return Ok(Some(uv_new1));
        }
        if fa.abs() < EFF_ZERO {
            return Ok(Some(s_uv));
        }
        if fc.abs() < EFF_ZERO {
            return Ok(Some(uv_new2));
        }

        let mut count: u64 = 0;
        loop {
            count += 1;
            // Use Brent's method of stepping - quadratic root
            let dr = fb / fc;
            let s = fb / fa;
            let t = fa / fc;
            let p = s * (t * (dr - t) * (c - b) - (1.0 - dr) * (b - a));
            let q = (t - 1.0) * (dr - 1.0) * (s - 1.0);
            // The following will occur if we hit the boundary during the iteration.
            if q == 0.0 || (fb - fc).abs() < EFF_ZERO_SQ {
                if fc.abs() < EFF_ZERO_SQRT {
                    return Ok(Some(s_uv + uv_step * c));
                }
                break;
            }
            let mut x = b + p / q;
            let dom_size = domain.size();
            // Keep silhouette settled down by damping it by domain size
            if uv_step.x > dom_size.x / 30.0 {
                uv_step = uv_step * ((dom_size.x / 30.0) / uv_step.x);
            }
            if uv_step.y > dom_size.y / 30.0 {
                uv_step = uv_step * ((dom_size.y / 30.0) / uv_step.y);
            }
            let mut uv_new = s_uv + uv_step * x;
            // Make sure our step sizes are not too large
            if !domain.contains_point(uv_new, EFF_ZERO) {
                x = domain.clip_line(s_uv, uv_step, 1.0);
                uv_new = s_uv + uv_step * x;
            }
            uv_new = domain.clamp_point(uv_new);
            let norm = evaluate_normal(k, surface, uv_new)?;
            if self.perspective {
                let pt = surface.evaluate_point(k, uv_new)?;
                r = pt - self.eye;
            }
            let fx = r.dot(norm);
            // If we have a very good hit then stop
            if fx.abs() < EFF_ZERO {
                return Ok(Some(uv_new));
            }
            // If we hit a limit of some sort see if we are near to a solution.
            if count > 100 || (b - c).abs() < EFF_ZERO {
                if fx.abs() < EFF_ZERO_SQRT {
                    return Ok(Some(uv_new));
                }
                break;
            }
            // Move to next iteration in loop
            a = b;
            b = c;
            c = x;
            fa = fb;
            fb = fc;
            fc = fx;
        }

        // If we didn't return within the loop, then we didn't find a solution.
        Ok(None)
    }

    /// `BranchMayContainAnswers` for a surface node of type `aux_type` whose
    /// `SmBezierPatch` boxes `patch` returns (`None`: a null patch pointer); only
    /// Bezier patch nodes are culled. Parallel projection: whether the patch's normal
    /// box has a vector perpendicular to the view direction. Perspective: false when
    /// the directions from the patch's pseudo box corners to the eye, crossed twice
    /// with the normal box, are disjoint from those directions.
    ///
    /// # Errors
    ///
    /// Those of `patch` ([`SolidError::Unported`] only; its other failures answer
    /// `true`, as upstream's statuses read as booleans).
    pub fn patch_may_contain_answers<F>(&self, aux_type: AuxDataType, patch: F) -> Result<bool>
    where
        F: FnOnce() -> Result<Option<(PolarBox, PseudoBox)>>,
    {
        // Use the Gauss map of the normal of the surface to do an angular test to
        // see if we can ignore this node.
        if aux_type != AuxDataType::BezierSurface {
            return Ok(true);
        }
        let (polar_box, pseudo_box) = match patch() {
            Ok(Some(b)) => b,
            Err(e) if e.is_unported() => return Err(e),
            // NER: SM_ERR_NULL_POINTER reads as TRUE, as does any failure status
            Ok(None) | Err(_) => return Ok(true),
        };
        if !self.perspective {
            return Ok(polar_box.has_perpendicular_to_vector(self.eye));
        }

        // A polar box from the point to the surface patch, tested for disjointness
        // with the normal polar box of the surface (through Cross).
        let corners = pseudo_box.corners();
        let mut tmp_polar_box = PolarBox::new();
        for corner in corners {
            let vec = self.eye - corner;
            if vec.length_squared() > EFF_ZERO {
                tmp_polar_box.add_vector(vec, None);
            }
        }
        let cross1 = polar_box.cross(&tmp_polar_box);
        let cross2 = cross1.cross(&polar_box);
        Ok(!cross2.are_disjoint(&tmp_polar_box))
    }

    /// `BranchMayContainAnswers` of the interior global solve: the surface node of
    /// `branch` with the boxes of its patch in `cache`
    /// ([`Self::patch_may_contain_answers`]).
    ///
    /// # Errors
    ///
    /// Those of the branch lookup; [`SolidError::Unported`] from the polar box build.
    pub fn branch_may_contain_answers<O: SolutionObject>(
        &self,
        k: &Kernel,
        solver: &GlobalSolver<'_, O>,
        cache: &SurfaceCache<O>,
        branch: &Branch,
    ) -> Result<bool> {
        let surface_node = solver.branch_node(branch, 0)?;
        let node_id = branch.first().copied().flatten();
        self.patch_may_contain_answers(surface_node.aux_type, || {
            let (Some(id), NodeData::Surface(SurfaceAux { slot: Some(_), .. })) =
                (node_id, surface_node.data)
            else {
                return Ok(None);
            };
            let pseudo_box = *cache.patch(id)?.pseudo_box();
            let polar_box = cache.polar_box_unstored(k, id)?;
            Ok(Some((polar_box, pseudo_box)))
        })
    }
}

impl<'s> SurfaceTrace<'s> for SurfaceSilhouette<'s> {
    fn tracer(&self) -> &SurfaceTracer<'s> {
        &self.base
    }

    fn tracer_mut(&mut self) -> &mut SurfaceTracer<'s> {
        &mut self.base
    }

    fn compute_point_values(
        &mut self,
        k: &Kernel,
        uv: Point2,
        pnt: &mut TracePoint,
        prev: Option<&TracePoint>,
        step_size: Option<f64>,
    ) -> Result<()> {
        let _ = step_size;
        let _ = k;
        SurfaceSilhouette::compute_point_values(self, k, uv, pnt, prev)
    }

    fn local_point_solve(
        &self,
        k: &Kernel,
        curr: &TracePoint,
        guess: Point2,
        next: &mut TracePoint,
    ) -> Result<Option<Point2>> {
        let _ = next;
        SurfaceSilhouette::local_point_solve(self, k, curr, guess)
    }

    fn find_solutions_on_boundary_curve(
        &self,
        k: &Kernel,
        boundary: &Curve,
        interval: &Extent1d,
        side: usize,
        param: SurfParamType,
    ) -> Result<SolutionArray<BoundaryCurve>> {
        let _ = (side, param);
        SurfaceSilhouette::find_solutions_on_boundary_curve(self, k, boundary, interval)
    }

    fn does_point_lie_on_curve(&self, k: &Kernel, uv: &mut Point2, refine: bool) -> Result<bool> {
        SurfaceSilhouette::does_point_lie_on_curve(self, k, uv, refine)
    }

    fn branch_may_contain_answers<O: SolutionObject>(
        &mut self,
        solver: &GlobalSolver<'_, O>,
        cache: &SurfaceCache<O>,
        branch: &Branch,
    ) -> Result<bool> {
        SurfaceSilhouette::branch_may_contain_answers(self, self.kernel, solver, cache, branch)
    }
}

/// The function of the silhouette singularity search (`SmFindSilSingEFO`), over
/// the segment `t -> uv_start + t (uv_end - uv_start)`: the Gaussian curvature
/// (`zero_gauss`), or the smaller dot product of the view vector with a principal
/// direction; the derivative is a forward difference with step `1e-8`.
#[derive(Debug, Clone, Copy)]
pub struct FindSilSingEfo<'a> {
    /// Caller context for virtual evaluations.
    pub kernel: &'a Kernel,

    /// `m_crSurface`.
    pub surface: &'a Surface,
    /// `m_crUVStart`.
    pub uv_start: Point2,
    /// `m_crUVEnd`.
    pub uv_end: Point2,
    /// `m_bZeroGauss`.
    pub zero_gauss: bool,
    /// `m_bPerspective`.
    pub perspective: bool,
    /// `m_crEye`: eye point or view direction.
    pub eye: Point3,
}

impl<'a> FindSilSingEfo<'a> {
    /// `SmFindSilSingEFO(crSurface, crUVStart, crUVEnd, bZeroGauss, bPerspective, crEye)`.
    #[must_use]
    pub const fn new(
        k: &'a Kernel,
        surface: &'a Surface,
        uv_start: Point2,
        uv_end: Point2,
        zero_gauss: bool,
        perspective: bool,
        eye: Point3,
    ) -> Self {
        Self {
            kernel: k,
            surface,
            uv_start,
            uv_end,
            zero_gauss,
            perspective,
            eye,
        }
    }

    /// `Evaluate`: `F(t)`, `F'(t)` and whether `|F(t)| < SM_EFF_ZERO_SQ`, written to
    /// `out` as upstream writes its out-params (partial values stay on failure).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for an eye point on the surface; those of the evaluations.
    pub fn evaluate_into(&self, k: &Kernel, t: f64, out: &mut FunctionValue) -> Result<()> {
        // init output
        out.f = 0.0;
        out.f_prime = 0.0;
        out.found_answer = false;

        let uv = self.uv_start + t * (self.uv_end - self.uv_start);
        let g1 = evaluate_geometric_at(k, self.surface, uv)?;

        if self.zero_gauss {
            out.f = g1.gaussian_curvature;
            let uv2 = self.uv_start + (t + 1.0e-8) * (self.uv_end - self.uv_start);
            let g2 = evaluate_geometric_at(k, self.surface, uv2)?;
            out.f_prime = (g2.gaussian_curvature - g1.gaussian_curvature) / 1.0e-8;
            if out.f.abs() < EFF_ZERO_SQ {
                out.found_answer = true;
            }
        } else {
            let mut r = self.eye;
            if self.perspective {
                let pnt = self.surface.evaluate_point(k, uv)?;
                r = pnt - self.eye;
            }
            if r.length_squared() < EFF_ZERO {
                return Err(SolidError::sm_err(OP_EFO)); // point on surface must be at the eye point
            }
            let mut dot1 = r.dot(g1.principle_curvature_vector1);
            if r.dot(g1.principle_curvature_vector2).abs() < dot1.abs() {
                dot1 = r.dot(g1.principle_curvature_vector2);
            }
            out.f = dot1;
            let uv2 = self.uv_start + (t + 1.0e-8) * (self.uv_end - self.uv_start);
            let g2 = evaluate_geometric_at(k, self.surface, uv2)?;
            if self.perspective {
                let pnt = self.surface.evaluate_point(k, uv2)?;
                r = pnt - self.eye;
            }
            if r.length_squared() < EFF_ZERO {
                return Err(SolidError::sm_err(OP_EFO)); // point on surface must be at the eye point
            }
            let mut dot2 = r.dot(g2.principle_curvature_vector1);
            if r.dot(g2.principle_curvature_vector2).abs() < dot2.abs() {
                dot2 = r.dot(g2.principle_curvature_vector2);
            }
            out.f_prime = (dot2 - dot1) / 1.0e-8;
            if out.f.abs() < EFF_ZERO_SQ {
                out.found_answer = true;
            }
        }

        Ok(())
    }
}

impl EvalFunction for FindSilSingEfo<'_> {
    fn evaluate(&mut self, t: f64, out: &mut FunctionValue, signal_errors: bool) -> Result<()> {
        let k = self.kernel;
        let _ = signal_errors;
        self.evaluate_into(k, t, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::plane::Plane;

    fn plane() -> Surface {
        Surface::from(Plane::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0),
        ))
    }

    #[test]
    fn constructor_unitizes_view_direction() {
        let k = &Kernel::default();
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0);
        let t = SurfaceSilhouette::new(k, &s, &d, false, Vec3::new(0.0, 3.0, 4.0)).unwrap();
        assert!((t.eye.length() - 1.0).abs() < 1e-15);
        let z = SurfaceSilhouette::new(k, &s, &d, false, Vec3::ZERO).unwrap();
        assert_eq!(z.eye, Vec3::Z);
        let p = SurfaceSilhouette::new(k, &s, &d, true, Vec3::new(0.0, 3.0, 4.0)).unwrap();
        assert_eq!(p.eye, Vec3::new(0.0, 3.0, 4.0));
        assert!(is_kind_of(TYPE));
        assert!(is_kind_of(crate::intersect::surface_tracer::TYPE));
        assert!(!is_kind_of(0));
    }

    #[test]
    fn plane_point_on_curve() {
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0);
        let k = Kernel::default();
        // viewed edge-on: every point is a silhouette point
        let t = SurfaceSilhouette::new(&k, &s, &d, false, Vec3::new(1.0, 0.0, 0.0)).unwrap();
        let mut uv = Vec2::new(2.0, 3.0);
        assert!(t.does_point_lie_on_curve(&k, &mut uv, false).unwrap());
        // viewed from above: none is
        let t = SurfaceSilhouette::new(&k, &s, &d, false, Vec3::new(0.0, 0.0, 1.0)).unwrap();
        assert!(!t.does_point_lie_on_curve(&k, &mut uv, false).unwrap());
        // perspective eye on the plane's line of sight
        let t = SurfaceSilhouette::new(&k, &s, &d, true, Vec3::new(2.0, 3.0, 5.0)).unwrap();
        assert!(!t.does_point_lie_on_curve(&k, &mut uv, false).unwrap());
    }

    #[test]
    fn plane_point_values_without_tangent_fail() {
        let k = &Kernel::default();
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0);
        // the silhouette function is constant: no direction, no previous point
        let t = SurfaceSilhouette::new(k, &s, &d, false, Vec3::new(0.3, 0.2, 1.0)).unwrap();
        let mut p = TracePoint::new();
        let r = t.compute_point_values(k, Vec2::new(1.0, 1.0), &mut p, None);
        assert!(r.is_err());
        // eye point on the surface
        let t = SurfaceSilhouette::new(k, &s, &d, true, Vec3::new(1.0, 1.0, 0.0)).unwrap();
        let mut p = TracePoint::new();
        let r = t.compute_point_values(k, Vec2::new(1.0, 1.0), &mut p, None);
        assert!(r.is_err());
    }

    #[test]
    fn plane_local_point_solve_serendipity() {
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0);
        let k = Kernel::default();
        let t = SurfaceSilhouette::new(&k, &s, &d, false, Vec3::new(0.0, 1.0, 0.0)).unwrap();
        let r = t
            .local_point_solve(&k, &TracePoint::new(), Vec2::new(12.0, -1.0))
            .unwrap();
        assert_eq!(r, Some(Vec2::new(10.0, 0.0)));
        // off the silhouette: the uv direction vanishes on a plane
        let t = SurfaceSilhouette::new(&k, &s, &d, false, Vec3::new(0.0, 1.0, 1.0)).unwrap();
        let r = t
            .local_point_solve(&k, &TracePoint::new(), Vec2::new(1.0, 1.0))
            .unwrap();
        assert_eq!(r, None);
    }

    #[test]
    fn patch_culling() {
        let k = &Kernel::default();
        let s = plane();
        let d = Extent2d::from_bounds(0.0, 0.0, 10.0, 10.0);
        let t = SurfaceSilhouette::new(k, &s, &d, false, Vec3::new(0.0, 0.0, 1.0)).unwrap();
        let mut pb = PolarBox::new();
        pb.add_vector(Vec3::new(0.0, 0.0, 1.0), None);
        let ps = PseudoBox::new();
        // a normal box around +z has no vector perpendicular to +z
        assert!(!t
            .patch_may_contain_answers(AuxDataType::BezierSurface, || Ok(Some((pb, ps))))
            .unwrap());
        // other node kinds and null patches are never culled
        assert!(t
            .patch_may_contain_answers(AuxDataType::AuxData, || Ok(Some((pb, ps))))
            .unwrap());
        assert!(t
            .patch_may_contain_answers(AuxDataType::BezierSurface, || Ok(None))
            .unwrap());
        let mut pb2 = PolarBox::new();
        pb2.add_vector(Vec3::new(1.0, 0.0, 0.0), None);
        assert!(t
            .patch_may_contain_answers(AuxDataType::BezierSurface, || Ok(Some((pb2, ps))))
            .unwrap());
    }

    #[test]
    fn wired_callees_do_not_defer() {
        let k = &Kernel::default();
        let s = plane();
        let efo = FindSilSingEfo::new(k, &s, Vec2::ZERO, Vec2::new(1.0, 1.0), true, false, Vec3::Z);
        let mut out = FunctionValue::default();
        assert!(!efo
            .evaluate_into(k, 0.5, &mut out)
            .err()
            .is_some_and(|e| e.is_unported()));
    }
}
