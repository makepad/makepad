//! `SmSurface`, part 6: the surface continuity across its knots, the natural UV trimming
//! loop, the derivative-surface sample values along a boundary curve, the local
//! curve/surface intersection, the simple curve/surface coincidence check, the
//! classification of a range intersection as a coincident stretch or a tangent point,
//! the surface/surface global solver's identical-solution test and the file-static
//! curve/surface local solve of the global solvers.
//!
//! Upstream: `source/SMLib/src/SmSurface.cpp`, `source/SMLib/inc/SmSurface.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`Surface::calculate_continuities`]               | `SmSurface::CalculateContinuities` (virtual; no surface kind overrides it) |
//! | [`Surface::create_natural_uv_trim_curves`], [`NaturalUvTrimCurves`] | `SmSurface::CreateNaturalUVTrimCurves` |
//! | [`DerivSurfDefinition::sample_values`], [`SampleOptions`], [`DerivSampleValues`] | `SmDerivSurfDefinition::GetSampleValues` (from a surface boundary curve) |
//! | [`Surface::local_curve_intersect`], [`CurveIntersection`] | `SmSurface::LocalCurveIntersect` (virtual; not overridden) |
//! | [`Surface::simple_coincidence_checker`]           | `SmSurface::SimpleCoincidenceChecker` (virtual; not overridden) |
//! | [`Surface::classify_intersecting_range_solution`], [`RangeTangentPoint`] | `SmSurface::ClassifyIntersectingRangeSolution` |
//! | [`SsGlobalSolver::solutions_identical`]           | `SmSSGlobalSolver::IdenticalSolutions`           |
//! | [`sc_local_solve`], [`ScLocalSolveNodes`]         | `sm_SCLocalSolve` (static)                       |
//!
//! Deviations:
//! - **Out-params** become return values: `CalculateContinuities` returns the minimum
//!   and the per-knot continuities, `CreateNaturalUVTrimCurves` a
//!   [`NaturalUvTrimCurves`] (the optional gaps are always returned),
//!   `GetSampleValues` a [`DerivSampleValues`] (the optional second and third
//!   derivatives are asked for with flags), `LocalCurveIntersect` `None` for "no
//!   intersection", `SimpleCoincidenceChecker` `None` for "no coincidence" (the
//!   solution is only written upstream when one is found), and
//!   `ClassifyIntersectingRangeSolution` the degenerate flag with the tangent point it
//!   writes (`None` when `rSolPoint` is left unmodified). Allocation-only `SmContext` arguments are omitted; evaluation paths retain
//!   the caller's Kernel, including boundary iso curves. `ClassifyIntersectingRangeSolution`'s
//!   `dTol3d` and `sm_SCLocalSolve`'s `rdBestAnswerSoFarSq` are `NotUsed` upstream and
//!   dropped.
//! - **`sm_SCLocalSolve`** takes the data it reads from its tree nodes and Bezier patch
//!   instead of the nodes ([`ScLocalSolveNodes`]): the original surface of the surface
//!   node's cache (`sm_GetSurfaceToEvaluate`), the patch surface (`mBA_pSurface`) and
//!   the nodes' boxes. Its boundary iso curves (`SmIsoCurve` over the evaluated
//!   surface) retain the concrete Surface kind.
//! - **`SmSSGlobalSolver::IdenticalSolutions`** is the inherent
//!   [`SsGlobalSolver::solutions_identical`] (the trait method of
//!   [`GlobalSolve`](crate::geom::global_solver::GlobalSolve) calls it); a missing
//!   surface (upstream dereferences `NULL`) is `SM_ERR_NULL_POINTER`.
//! - **Ignored statuses.** Where upstream ignores an evaluation's status, its outputs
//!   keep their previous values (their initial values: zero vectors for
//!   `SmVector2d`/`SmVector3d` outputs it clears first, [`Vec3::UNDEF`] for default
//!   constructed points); [`SolidError::Unported`] is still propagated.
//! - **Iso curves.** `CreateIsoParametricCurve` is the B-spline, plane and surface of
//!   revolution and cone overrides (units `nurbs.b_spline_surface_p2`, `surface.plane_p1`,
//!   `surface.surf_of_revolution_p1`, `surface.cone_p1`); the base body (other kinds) belongs to
//!   `surface.surface_p5`. The plane's iso curve is its `SmLine`, the revolution's sweep
//!   iso curve its `SmCircle`, so their own `IsDegenerate`, `GetKnots` and
//!   `CalculateContinuities` overrides run.
//! - **Debug-only work** (drawing, dumps, `SM_ASSERT`, `WARN`) is not ported.
//! - **Back-edges.** Point drops, local solves, singularities, patch point solves and
//!   generic iso curves call `surface.surface_p5`; seam tests, global point solves and
//!   `SmCSIntersectENFO` call `surface.surface_p4`. Their missing downstream callees
//!   propagate [`SolidError::Unported`]. Callers retain the kernel; [`SsGlobalSolver`]
//!   holds it. Virtual plane overrides preserve its nonsingular, seam-free geometry.
//! - **Partial samples and bounds.** The status-aware boundary sampler retains
//!   written values and pads unwritten slots with zeros, matching `SmTArray::SetSize`
//!   (`calloc`/`memset`, not the vector default constructor). This lets the blend
//!   builder honor upstream's ignored sampling status without inventing values.
//!   Counts are checked and capped at [`MAX_BOUNDARY_SAMPLES`] before any allocation;
//!   outputs otherwise grow as samples are computed. No function here recurses.

use crate::curve::curve::p3::PointSolveOptions;
use crate::curve::curve::Curve;
use crate::curve::iso_curve::IsoCurve;
use crate::curve::line::Line;
use crate::error::{ignored, Result, SolidError};
use crate::geom::assert_array::ObjectKey;
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::{Extent1d, ExtentNd};
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{
    PreferredSolution, SolutionPairType, SolutionRequested, SolverOperation,
};
use crate::geom::local_solve_nd::{BoundaryHandler, LocalSolveNd};
use crate::geom::solution_array::{
    Solution, SolutionArray, SolutionEnd, SolutionObject, SolutionType,
};
use crate::geom::tol::are_same;
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, PI};
use crate::nurbs::b_spline_curve::p2::BoundingBoxRequest as CurveBoxRequest;
use crate::nurbs::b_spline_surface::{IsoCurveOptions, SurfParamType};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;

use super::p1::CsIntersectEnfo;
use super::p2::{DerivSurfDefinition, TangencyType};
use super::p4::{global_point_solve_starts, NoObject};
use super::p5::DropPointOptions;
use super::p7::SsGlobalSolver;
use super::{Surface, SurfaceKind};

const OP_CONT: &str = "surface::surface::p6::Surface::calculate_continuities";
const OP_SAMPLE: &str = "surface::surface::p6::DerivSurfDefinition::sample_values";
/// Maximum boundary-derivative sample count (plus one endpoint), bounding the
/// zero-filled partial arrays retained after a sampling failure. Upstream fixtures
/// use at most hundreds of samples; this allows 65,536 intervals.
pub const MAX_BOUNDARY_SAMPLES: u64 = 65_536;
const OP_SS: &str = "surface::surface::p6::SsGlobalSolver::solutions_identical";
const OP_CS_INTERSECT: &str = "surface::surface::p6::cs_intersect_local_solve";
const OP_SC: &str = "surface::surface::p6::sc_local_solve";

/// `SM_CONTINUITY_ANGLE` (`SM_ANG_TOL_DEG`, degrees): the default angle tolerance of
/// `SmCurve::CalculateContinuities`.
const CONTINUITY_ANGLE: f64 = 0.5;

/// `SM_DEG2RAD`.
fn deg2rad(adeg: f64) -> f64 {
    adeg * (PI / 180.0)
}

/// The continuity code (`SmContinuityType` values are ordered by strength).
const fn code(c: ContinuityType) -> i32 {
    c as i32
}

/// The 3D trimming loop of a UV domain (`CreateNaturalUVTrimCurves`' outputs), one
/// entry per side kept.
#[derive(Debug, Clone)]
pub struct NaturalUvTrimCurves {
    /// `r3DTrimmingCurves`: the iso-parametric curve of each side.
    pub curves_3d: Vec<Curve>,
    /// `rUVTrimmingCurves`: the 2D `SmLine` of each side between its domain corners.
    pub uv_curves: Vec<Line>,
    /// `rOrients`: the orientation of each curve in the counter-clockwise loop.
    pub orients: Vec<OrientType>,
    /// `pOptMaxGap3d`: the achieved maximum gap of each iso curve.
    pub max_gaps_3d: Vec<f64>,
}

/// The cross-derivative samples along a surface boundary curve
/// ([`DerivSurfDefinition::sample_values`]): one entry per sample.
#[derive(Debug, Clone, Default)]
pub struct DerivSampleValues {
    /// `rParams`: the boundary curve parameters.
    pub params: Vec<f64>,
    /// `rPos`: the boundary curve points.
    pub positions: Vec<Vec3>,
    /// `r1stDerivs`: the first cross derivatives.
    pub first_derivs: Vec<Vec3>,
    /// `pOpt2ndDerivs` (when asked).
    pub second_derivs: Option<Vec<Vec3>>,
    /// `pOpt3rdDerivs` (when asked).
    pub third_derivs: Option<Vec<Vec3>>,
}

/// The flags of [`DerivSurfDefinition::sample_values`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SampleOptions {
    /// `bOffToTheRight`: walking along the curve with the head along the surface
    /// normal, the cross tangents point to the right (else to the left).
    pub off_to_the_right: bool,
    /// `bStartAtBoundary` (`DirToReferenceCurve` only): the directions go from the
    /// boundary curve to the reference curve (else from the reference curve).
    pub start_at_boundary: bool,
    /// `[pOpt2ndDerivs != NULL, pOpt3rdDerivs != NULL]`: also return the second /
    /// third derivatives.
    pub want_higher: [bool; 2],
}

/// A curve/surface intersection found by [`Surface::local_curve_intersect`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveIntersection {
    /// `rUVFound`: the surface parameters.
    pub uv: Point2,
    /// `rdTFound`: the curve parameter.
    pub t: f64,
    /// `rdDeviation`: the distance between the curve and surface points.
    pub deviation: f64,
}

/// The tangent point [`Surface::classify_intersecting_range_solution`] writes to its
/// `rSolPoint` (the better end of the range).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RangeTangentPoint {
    /// `m_dSolutionValue`: the deviation at the better end.
    pub solution_value: f64,
    /// `m_adParameters[0..3]`: the curve parameter and the surface `u`, `v` there.
    pub parameters: [f64; 3],
}

/// What [`sc_local_solve`] reads from its tree nodes and Bezier patch.
#[derive(Debug, Clone, Copy)]
pub struct ScLocalSolveNodes<'a> {
    /// `sm_GetSurfaceToEvaluate(pSurfaceNode, ..)`: the original surface of the
    /// surface node's cache.
    pub eval_surface: &'a Surface,
    /// `pBezPatch->mBA_pSurface`: the surface of the node's Bezier patch.
    pub patch_surface: &'a Surface,
    /// `pSurfaceNode->m_sBBox`.
    pub surface_box: Extent3d,
    /// `pCurveNode->m_sBBox` (`None`: no curve node).
    pub curve_box: Option<Extent3d>,
}

// ---- SmSurface members -------------------------------------------------------------------

impl Surface {
    /// The continuity of the surface across its unique knots in `surf_param`'s
    /// direction (`CalculateContinuities`): the iso-parametric curves in that direction
    /// at up to five sample values of the other direction's knots (a single span is
    /// also sampled at its middle; a degenerate first curve past the first sample is
    /// retried between the samples) are classified at their knots; an iso-curve kink
    /// that the surface normals show to be smooth (within one degree) counts as
    /// [`ContinuityType::G1`]. Returns the minimum interior continuity and the
    /// minimum continuity at every unique knot (the end knots
    /// [`ContinuityType::Discontinuous`] until a curve is classified).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for fewer than two unique knots; those of `GetKnots`, the iso curves
    /// and their continuities; [`SolidError::Unported`] for kinds without a ported
    /// `CreateIsoParametricCurve` (the base body: `surface.surface_p5`).
    pub fn calculate_continuities(
        &self,
        k: &Kernel,
        surf_param: SurfParamType,
    ) -> Result<(ContinuityType, Vec<ContinuityType>)> {
        self.calculate_continuities_reached(k, surf_param, &mut None)
    }

    /// [`Surface::calculate_continuities`], also storing in `reached` the minimum
    /// continuity output (`reMinContinuityInSurfaceKnot`) as it stands whenever upstream
    /// writes it, so that a caller ignoring a failed status reads what upstream leaves
    /// there (`None`: never written, upstream's uninitialised value).
    ///
    /// # Errors
    ///
    /// Those of [`Surface::calculate_continuities`].
    pub(crate) fn calculate_continuities_reached(
        &self,
        k: &Kernel,
        surf_param: SurfParamType,
        reached: &mut Option<ContinuityType>,
    ) -> Result<(ContinuityType, Vec<ContinuityType>)> {
        // Get unique knot values for eSurfParam direction
        let (other_knots, _) = self.knots_with_nurb(k, surf_param, None)?;

        // check state
        if other_knots.len() <= 1 {
            return Err(SolidError::sm_err(OP_CONT));
        }

        // init output continuities to [Discontinuous, G1, ... G1, Discontinuous]
        let mut conts_at_knots = vec![ContinuityType::G1; other_knots.len()];
        let mut min_continuity = ContinuityType::G1;
        *reached = Some(min_continuity);
        if let Some(c) = conts_at_knots.first_mut() {
            *c = ContinuityType::Discontinuous;
        }
        if let Some(c) = conts_at_knots.last_mut() {
            *c = ContinuityType::Discontinuous;
        }

        // Get the knots going in the other (u/v) direction.
        // We'll make iso-curves going in our direction at those values.
        let get_sp = if surf_param == SurfParamType::U {
            SurfParamType::V
        } else {
            SurfParamType::U
        };
        let (mut knots, _) = self.knots_with_nurb(k, get_sp, None)?;

        // single span surfaces get checked in the middle in case the iso curves at both
        // end knots are degenerate.
        if let [k0, k1] = knots[..] {
            knots.push(0.5 * (k0 + k1));
        }

        // when reduce knot count to 5 sample points (don't need more)
        if knots.len() > 5 {
            let (Some(&first), Some(&last)) = (knots.first(), knots.last()) else {
                return Err(SolidError::sm_err(OP_CONT));
            };
            let k4 = last;
            let k3 = first + 0.845 * (k4 - first);
            let k2 = 0.5 * (first + k4);
            let k1 = first + 0.345 * (k4 - first);
            knots = vec![first, k1, k2, k3, k4];
        }

        // for every knot value to sample
        let mut first_curve = true;
        for ii in 0..knots.len() {
            let mut parameter = knot(&knots, ii)?;

            // make temp isoParameter curve for this knot sample value
            let mut iso = create_iso_parametric_curve(k, self, get_sp, parameter, None)?.0;
            // (status ignored)
            let iso_knots = iso_knots_of(k, &iso)?;

            // When curve is degenerate - try another knot sample value
            if iso.is_degenerate(k, EFF_ZERO, None)? {
                // If we have not been able to compute a continuity and we are past the
                // first knot.  Then take a middle value between the knots and try it.
                if ii > 0 && first_curve {
                    parameter = (knot(&knots, ii - 1)? + knot(&knots, ii)?) / 2.0;
                    iso = create_iso_parametric_curve(k, self, get_sp, parameter, None)?.0;
                    // (upstream keeps the knots of the replaced curve)
                }
                if iso.is_degenerate(k, EFF_ZERO, None)? {
                    continue;
                }
            }

            // calculate the iso-curve continuities
            let (_, mut tmp_conts) = iso.calculate_continuities(k, CONTINUITY_ANGLE)?;

            // Check the herringbone case: an iso-curve can change direction within the
            // surface, without causing a crease in the surface. The true check is the
            // surface normal.  If we find a kink in the isocurve, check the normals there.
            let mut tmp_min = ContinuityType::CInfinity;

            // For each interior knot:
            for jj in 1..tmp_conts.len().saturating_sub(1) {
                let cont = tmp_conts
                    .get_mut(jj)
                    .ok_or_else(|| SolidError::sm_err(OP_CONT))?;
                if code(*cont) < code(ContinuityType::G1) {
                    // A C0 or worse discontinuity in the isocurve.
                    let iso_knot = iso_knots
                        .get(jj)
                        .copied()
                        .ok_or_else(|| SolidError::sm_err(OP_CONT))?;
                    let uv = if surf_param == SurfParamType::U {
                        Point2::new(iso_knot, parameter)
                    } else {
                        Point2::new(parameter, iso_knot)
                    };
                    let norm1 = ignored(self.evaluate_normal(k, uv, Side::Right, Side::Right))?;
                    let norm2 = ignored(self.evaluate_normal(k, uv, Side::Left, Side::Left))?;
                    if let (Some(norm1), Some(norm2)) = (norm1, norm2) {
                        let dot = norm1.dot(norm2);
                        if dot > 0.9998476951563913 {
                            // cosine of 1 deg, our standard angle limit
                            *cont = ContinuityType::G1;
                        }
                    }
                }

                // Reset the min cont for this isocurve with the possibly-modified value.
                if code(*cont) < code(tmp_min) {
                    tmp_min = *cont;
                }
            }

            // Just check the continuity across the iso curve knots that are in the
            // surface knot vector.
            let surface_knot_cnt = conts_at_knots.len();
            let same_knots = tmp_conts.len() == surface_knot_cnt;

            if first_curve {
                // if this is the first curve to classify
                first_curve = false;

                // store computed values
                if same_knots {
                    conts_at_knots.clone_from(&tmp_conts);
                } else {
                    // save continuity values at original knot values (the others stay
                    // SM_CT_UNDEFINED, the zeroed memory of SetSize)
                    conts_at_knots = vec![ContinuityType::Undefined; surface_knot_cnt];
                    for (jj, cont) in tmp_conts.iter().enumerate() {
                        let iso_knot = iso_knots
                            .get(jj)
                            .copied()
                            .ok_or_else(|| SolidError::sm_err(OP_CONT))?;
                        if let Some(index) = find_element(&other_knots, iso_knot) {
                            if let Some(c) = conts_at_knots.get_mut(index) {
                                *c = *cont;
                            }
                        }
                    }
                }
                min_continuity = tmp_min;
                *reached = Some(min_continuity);
            } else {
                // save minimum continuity values
                if code(tmp_min) < code(min_continuity) {
                    min_continuity = tmp_min;
                    *reached = Some(min_continuity);
                }

                // for every knot
                for (jj, cont) in tmp_conts.iter().enumerate() {
                    let index = if same_knots {
                        Some(jj)
                    } else {
                        let iso_knot = iso_knots
                            .get(jj)
                            .copied()
                            .ok_or_else(|| SolidError::sm_err(OP_CONT))?;
                        find_element(&other_knots, iso_knot)
                    };
                    if let Some(c) = index.and_then(|i| conts_at_knots.get_mut(i)) {
                        if code(*cont) < code(*c) {
                            *c = *cont;
                        }
                    }
                }
            }
        }

        Ok((min_continuity, conts_at_knots))
    }

    /// The trimming loop of `uv_domain` (`CreateNaturalUVTrimCurves`), counter-clockwise
    /// about the surface normal: starting at the minimum `u` side for
    /// [`SurfParamType::U`] (`(0,1) -> (0,0) -> (1,0) -> (1,1) -> (0,1)`), else at the
    /// minimum `v` side (`(0,0) -> (1,0) -> (1,1) -> (0,1) -> (0,0)`). Each side is the
    /// iso-parametric curve trimmed to the domain (orientation opposite, same, same,
    /// opposite for the minimum `u`, minimum `v`, maximum `u` and maximum `v` sides)
    /// with its UV line; with `remove_degenerate_curves` a degenerate side is dropped.
    ///
    /// # Errors
    ///
    /// Those of the iso curves and of [`Line::create_line_segment`];
    /// [`SolidError::Unported`] for kinds without a ported `CreateIsoParametricCurve`
    /// (`surface.surface_p5`).
    pub fn create_natural_uv_trim_curves(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        tgt_domain_side: SurfParamType,
        remove_degenerate_curves: bool,
    ) -> Result<NaturalUvTrimCurves> {
        let mut out = NaturalUvTrimCurves {
            curves_3d: Vec::new(),
            uv_curves: Vec::new(),
            orients: Vec::new(),
            max_gaps_3d: Vec::new(),
        };

        // locals
        let start_index = usize::from(tgt_domain_side != SurfParamType::U);
        let surf_domain = *uv_domain;

        for side in 0..4 {
            let current = (start_index + side) % 4;
            // side: iso direction, iso value, UV line ends, orientation
            let (param, iso, uv_start, uv_end, orient) = match current {
                // minimum U Curve [(0,0),(0,1)]
                0 => (
                    SurfParamType::U,
                    surf_domain.min().x,
                    surf_domain.evaluate(0.0, 0.0),
                    surf_domain.evaluate(0.0, 1.0),
                    OrientType::Opposite,
                ),
                // minimum V Curve [(0,0),(1,0)]
                1 => (
                    SurfParamType::V,
                    surf_domain.min().y,
                    surf_domain.evaluate(0.0, 0.0),
                    surf_domain.evaluate(1.0, 0.0),
                    OrientType::Same,
                ),
                // maximum U Curve [(1,0),(1,1)]
                2 => (
                    SurfParamType::U,
                    surf_domain.max().x,
                    surf_domain.evaluate(1.0, 0.0),
                    surf_domain.evaluate(1.0, 1.0),
                    OrientType::Same,
                ),
                // maximum V Curve [(0,1),(1,1)]
                _ => (
                    SurfParamType::V,
                    surf_domain.max().y,
                    surf_domain.evaluate(0.0, 1.0),
                    surf_domain.evaluate(1.0, 1.0),
                    OrientType::Opposite,
                ),
            };

            // create the 3D curve (approximation tolerance 0: set by the system)
            let (curve, max_gap_3d) =
                create_iso_parametric_curve(k, self, param, iso, Some(&surf_domain))?;

            // cull degenerate curves when asked
            if remove_degenerate_curves && curve.is_degenerate(k, EFF_ZERO, None)? {
                continue;
            }

            // create the UV curve
            let uv_curve =
                Line::create_line_segment(2, Vec3::from(uv_start), Vec3::from(uv_end), None)?;

            out.curves_3d.push(curve);
            out.orients.push(orient);
            out.uv_curves.push(uv_curve);
            out.max_gaps_3d.push(max_gap_3d);
        }

        Ok(out)
    }

    /// The intersection of `curve` over `interval` with the surface over `uv_domain`
    /// near the guesses (`LocalCurveIntersect`): a Newton solve of the gap equations
    /// (`SmCSIntersectENFO`, clamped to the intervals, at most 20 steps), else the
    /// guesses; where the curve lies within 5 degrees of the tangent plane a closest
    /// point solve that does better replaces it; at a singularity the degenerate
    /// parameter moves to the end of `uv_domain` shared with the natural domain (or
    /// nearer the solution). `None` unless the curve and surface points lie closer
    /// than `distance_tolerance`.
    ///
    /// # Errors
    ///
    /// Those of the evaluations, the Newton solve and the closest point solve.
    pub fn local_curve_intersect(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        distance_tolerance: f64,
        uv_guess: Point2,
        t_guess: f64,
    ) -> Result<Option<CurveIntersection>> {
        // locals for Solver interval and periodicity values (local solvers should not
        // jump over seams)
        let intervals = [
            *interval,
            Extent1d::from_min_max(uv_domain.min().x, uv_domain.max().x),
            Extent1d::from_min_max(uv_domain.min().y, uv_domain.max().y),
        ];
        let guess = [t_guess, uv_guess.x, uv_guess.y];

        // Solve it
        let solution = cs_intersect_local_solve(
            k,
            self,
            curve,
            EFF_ZERO * EFF_ZERO,
            &intervals,
            &guess,
            distance_tolerance * 4.0,
        )?;
        let (mut t, mut uv) = match solution {
            Some([s0, s1, s2]) => (s0, Point2::new(s1, s2)),
            None => (t_guess, uv_guess),
        };

        // get 3D solution Points and gap vector (nonZeroTangent values)
        let pv1 = curve.evaluate(k, t, 1, true, true)?;
        let pv1_0 = vec_at(&pv1, 0)?;
        let pv1_1 = vec_at(&pv1, 1)?;
        let p2 = self.evaluate_point(k, uv)?;
        let mut dist = pv1_0.distance(p2);

        // When curve is near Surface tangentPlane (5 deg) -
        let normal = self.evaluate_normal(k, uv, Side::Left, Side::Left)?;
        let scaled_zero = EFF_ZERO * (1.0 + p2.max_dimension());
        if normal.is_perpendicular_to(pv1_1, 5.0) {
            // Find Curve/Surface closest point pair
            let found = self.local_curve_solve(
                k,
                uv_domain,
                curve,
                interval,
                SolverOperation::Minimize,
                distance_tolerance,
                &[],
                uv,
                t,
            )?;

            // when a better solution was found or we have two good solutions
            if let Some(s) = found {
                if s.distance < dist || s.distance < scaled_zero {
                    // save the better solution
                    dist = s.distance;
                    t = s.t;
                    uv = s.uv;
                }
            }
        }

        // If we're at a singularity, any value of the degenerate parameter will be a
        // solution: return a parameter value on one end of the degenerate domain,
        // whichever end is closer to the guess point.
        let (singular, sing_dir) = is_singularity(k, self, uv, distance_tolerance)?;
        if singular {
            // If one end of the subdomain coincides with an end of the whole domain,
            // use that.
            let all_domain = self.natural_uv_domain_with_nurb(k)?;

            if sing_dir == SurfParamType::U || sing_dir == SurfParamType::Both {
                uv.x = singular_end(&uv_domain.u_interval(), &all_domain.u_interval(), uv.x);
            }
            if sing_dir == SurfParamType::V || sing_dir == SurfParamType::Both {
                uv.y = singular_end(&uv_domain.v_interval(), &all_domain.v_interval(), uv.y);
            }
        }

        // when gap vector is less than tolerance
        if dist < distance_tolerance {
            return Ok(Some(CurveIntersection {
                uv,
                t,
                deviation: dist,
            }));
        }
        Ok(None)
    }

    /// The simple coincidence check of `curve` over `interval` with the surface over
    /// `uv_domain` (`SimpleCoincidenceChecker`): disjoint boxes (within
    /// `tol_3d`) are no coincidence; else the curve midpoint is dropped to the surface
    /// (a global solve), then points stepping from the middle out to both ends
    /// (`2 * (degree + 5)` of them, each solve starting from the last result). A point
    /// is coincident when it lies within `tol_3d` and the curve is tangent to the
    /// surface there (within about one degree). Without `partial_coincidence_ok` every
    /// point must be coincident; with it three hits end the search. Returns the
    /// coincident range (curve parameters from the sampled points, the farthest
    /// apart UVs and the end deviations) with `objects` (`[curve, this surface]`), or
    /// `None`.
    ///
    /// # Errors
    ///
    /// Those of the evaluations and of `GlobalPointSolve`;
    /// [`SolidError::Unported`]: `LocalPointSolve` (`surface.surface_p5`).
    pub fn simple_coincidence_checker<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        tol_3d: f64,
        partial_coincidence_ok: bool,
        objects: [Option<O>; 2],
    ) -> Result<Option<Solution<O>>> {
        self.simple_coincidence_checker_from(
            k,
            uv_domain,
            curve,
            interval,
            tol_3d,
            partial_coincidence_ok,
            objects,
            None,
            None,
        )
    }

    /// [`Surface::simple_coincidence_checker`] with lazy cache preparation for
    /// its initial global point solve. All boxing and sample tests are shared.
    ///
    /// # Errors
    /// Those of the coincidence check and prepared trimmed point classification.
    #[allow(clippy::too_many_arguments)]
    pub fn simple_coincidence_checker_with_context<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        tol_3d: f64,
        partial_coincidence_ok: bool,
        objects: [Option<O>; 2],
        cache: Option<&crate::surface::surface_cache::SurfaceCache<O>>,
        context: &mut dyn super::p1::SurfaceSolveContext<O>,
    ) -> Result<Option<Solution<O>>> {
        self.simple_coincidence_checker_from(
            k,
            uv_domain,
            curve,
            interval,
            tol_3d,
            partial_coincidence_ok,
            objects,
            cache,
            Some(context),
        )
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn simple_coincidence_checker_from<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        tol_3d: f64,
        partial_coincidence_ok: bool,
        objects: [Option<O>; 2],
        cache: Option<&crate::surface::surface_cache::SurfaceCache<O>>,
        context: Option<&mut dyn super::p1::SurfaceSolveContext<O>>,
    ) -> Result<Option<Solution<O>>> {
        // First try boxing (statuses ignored).
        let this_box =
            ignored(self.calculate_bounding_box(k, uv_domain, [true, false, false], true))?
                .and_then(|b| b.normal_box)
                .unwrap_or_default();
        let req = CurveBoxRequest {
            normal: true,
            pseudo: false,
            polar: false,
        };
        let curv_box = ignored(curve.calculate_bounding_box(k, interval, req, true))?
            .and_then(|b| b.normal)
            .unwrap_or_default();
        if this_box.are_disjoint(&curv_box, tol_3d) {
            return Ok(None);
        }

        // locals: every sample parameter value found to be coincident (DOES NOT mark
        // the complete range of coincidence)
        let mut sampled_ivl = Extent1d::new();
        let mut num_hits: u64 = 0;
        // sNorm: one variable for every (status ignored) normal evaluation
        let mut norm = Vec3::UNDEF;

        // First drop the Curve MidPoint to Surface. Use a Global point solve, because
        // we have no guess uv.
        let mid_param = interval.evaluate(0.5);
        let mid_pt = curve.evaluate(k, mid_param, 1, true, true)?;
        let mid_pt0 = vec_at(&mid_pt, 0)?;
        let mut mid_pt1 = vec_at(&mid_pt, 1)?;
        let solutions = if let Some(context) = context {
            let mut found = SolutionArray::<O>::new();
            self.global_point_solve_into_with_context(
                k,
                objects[1],
                cache,
                context,
                uv_domain,
                SolverOperation::Minimize,
                mid_pt0,
                tol_3d,
                None,
                SolutionRequested::Single,
                &mut found,
            )?;
            found.as_slice().iter().map(|s| s.start).collect()
        } else {
            global_point_solve_single(k, self, uv_domain, mid_pt0, tol_3d)?
        };

        // If GlobalSolve, using Minimize, couldn't find any solutions, we're not likely
        // to have much success in finding coincidence.
        let Some(first) = solutions.first() else {
            return Ok(None);
        };

        // curve natural interval
        let crv_ivl = curve.natural_interval();

        // save Surf DropPoint
        let mid_uv = Point2::new(first.param(0)?, first.param(1)?);

        // Check whether this is a coincident solution: position and tangent.
        let mut found = first.solution_value <= tol_3d;
        if found {
            // Check tangency.
            let mag = mid_pt1.length();
            if mag > EFF_ZERO {
                mid_pt1 /= mag;
            }
            norm = normal_ignored(k, self, mid_uv, norm)?;
            let dot = mid_pt1.dot(norm);

            found = dot.abs() < 0.017; // about 1 degree
        }

        if found {
            sampled_ivl.add_value(mid_param);
            num_hits += 1;
        } else if !partial_coincidence_ok {
            // no coincidence when not looking for partial coincidence
            return Ok(None);
        }

        // Now test a sequence of points from the midpoint moving progressively closer
        // to the two ends, each time using the previous solution for the new guess.
        let mut uv_guess_start = mid_uv;
        let mut uv_guess_end = mid_uv;
        let mut uv0 = mid_uv; // Farthest-apart start and end uv's.
        let mut uv1 = mid_uv;
        // Errors at start and end of coincident portion:
        let mut st_dist = first.solution_value;
        let mut en_dist = first.solution_value;

        // use the degree of the curve to select the number of test points
        let count = curve.degree()? + 5;

        let mut have_start_uv = false;
        let mut have_end_uv = false;
        let mut end_pts_only = true;

        for ii in 0..count {
            // Move current Start and End Points out towards the ends.
            let frac = ((ii as f64 + 1.0) / count as f64) / 2.0;
            let (st_frac, en_frac) = if ii == count - 1 {
                (0.0, 1.0)
            } else {
                (0.5 - frac, 0.5 + frac)
            };

            // 1. Drop current StartPoint to Surface and check for coincidence
            let st_t = interval.evaluate(st_frac);
            let st_pnt = curve.evaluate(k, st_t, 1, true, true)?;
            let st_pnt0 = vec_at(&st_pnt, 0)?;
            let mut st_pnt1 = vec_at(&st_pnt, 1)?;
            let (mut found_start, sol) = local_point_solve::<O>(
                k,
                self,
                uv_domain,
                SolverOperation::Minimize,
                st_pnt0,
                uv_guess_start,
            )?;

            // Check close enough.
            if found_start {
                found_start = sol.start.solution_value <= tol_3d;
            }

            // Also check tangency.
            if found_start {
                let mag = st_pnt1.length();
                if mag > EFF_ZERO {
                    st_pnt1 /= mag;
                }
                let uv = Point2::new(sol.start.param(0)?, sol.start.param(1)?);
                norm = normal_ignored(k, self, uv, norm)?;
                let dot = st_pnt1.dot(norm);

                found_start = dot.abs() < 0.017; // about 1 degree
            }

            // this CrvStartPt is coincident and tangent to surface
            if found_start {
                // remember when midPoints were coincident
                if !crv_ivl.is_value_on_boundary(st_t, EFF_ZERO).0 {
                    end_pts_only = false;
                }

                // add mid point to interval known to be coincident
                sampled_ivl.add_value(st_t);
                st_dist = sol.start.solution_value;
                uv_guess_start = Point2::new(sol.start.param(0)?, sol.start.param(1)?);

                // Three coincident/tangent results is enough.
                let hits = num_hits;
                num_hits += 1;
                if hits >= 3 && partial_coincidence_ok {
                    break;
                }
            } else if !partial_coincidence_ok {
                // No full coincidence -- drop was unsuccessful
                return Ok(None);
            }

            // 2. Drop current EndPoint to Surface and check for coincidence
            let en_t = interval.evaluate(en_frac);
            let en_pnt = curve.evaluate(k, en_t, 1, false, true)?;
            let en_pnt0 = vec_at(&en_pnt, 0)?;
            let mut en_pnt1 = vec_at(&en_pnt, 1)?;
            let (mut found_end, sol) = local_point_solve::<O>(
                k,
                self,
                uv_domain,
                SolverOperation::Minimize,
                en_pnt0,
                uv_guess_end,
            )?;

            // Check close enough.
            if found_end {
                found_end = sol.start.solution_value <= tol_3d;
            }

            // Also check tangency.
            if found_end {
                let mag = en_pnt1.length();
                if mag > EFF_ZERO {
                    en_pnt1 /= mag;
                }
                let uv = Point2::new(sol.start.param(0)?, sol.start.param(1)?);
                norm = normal_ignored(k, self, uv, norm)?;
                let dot = en_pnt1.dot(norm);

                found_end = dot.abs() < 0.017; // about 1 degree
            }

            // this CrvEndPt is coincident and tangent to surface
            if found_end {
                // remember when midPoints were coincident
                if !crv_ivl.is_value_on_boundary(en_t, EFF_ZERO).0 {
                    end_pts_only = false;
                }

                // add mid point to interval known to be coincident
                sampled_ivl.add_value(en_t);
                en_dist = sol.start.solution_value;
                uv_guess_end = Point2::new(sol.start.param(0)?, sol.start.param(1)?);

                let hits = num_hits;
                num_hits += 1;
                if hits >= 3 && partial_coincidence_ok {
                    break;
                }
            } else if !partial_coincidence_ok {
                // No full coincidence -- drop was unsuccessful
                return Ok(None);
            }

            // Record the farthest-apart uv values.
            if found_start {
                uv0 = uv_guess_start;
                have_start_uv = true;

                // If we have no End uv yet, set it to this start uv.
                if !have_end_uv {
                    uv1 = uv0;
                    have_end_uv = true;
                }
            }
            if found_end {
                uv1 = uv_guess_end;
                have_end_uv = true;

                // Again, don't let sUV0 be unset.
                if !have_start_uv {
                    uv0 = uv1;
                    have_start_uv = true;
                }
            }
        }

        // No coincidence when no sample points were found to be coincident and tangent.
        if sampled_ivl.is_init() {
            return Ok(None);
        }

        // Also no coincidence when only one sample point was found to be coincident and
        // tangent, or only end points (a curve tangent to the surface at a seam point).
        if sampled_ivl.length() < EFF_ZERO || end_pts_only {
            return Ok(None);
        }

        // If we haven't returned yet, then we have found a coincidence.
        let mut solution = Solution::new();
        solution.solution_type = SolutionType::RangeOfValues;
        solution.num_objects = 2;
        solution.objects[0] = objects[0];
        solution.objects[1] = objects[1];
        solution.start.solution_value = st_dist;
        *solution.start.param_mut(0)? = sampled_ivl.min();
        *solution.start.param_mut(1)? = uv0.x;
        *solution.start.param_mut(2)? = uv0.y;

        solution.end.solution_value = en_dist;
        *solution.end.param_mut(0)? = sampled_ivl.max();
        *solution.end.param_mut(1)? = uv1.x;
        *solution.end.param_mut(2)? = uv1.y;

        Ok(Some(solution))
    }

    /// Whether the surface/`curve` range intersection `sol` is degenerate, a tangent
    /// point instead of a coincident stretch (`ClassifyIntersectingRangeSolution`). A
    /// point solution is degenerate (the point is not written). Otherwise the gap is
    /// sampled from the better end at geometrically growing steps along the range,
    /// each curve point dropped to the surface (a normalizing solve): a non-zero,
    /// monotonic, faster than linearly growing gap (or a point that does not drop) is
    /// a tangent point; a gap that never left zero over a short range (a tenth of the
    /// curve interval and of the domain diagonal) also is. For a tangent point the
    /// better end is returned.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`]: `LocalPointSolve` (`surface.surface_p5`); upstream
    /// ignores every other status.
    pub fn classify_intersecting_range_solution<O: SolutionObject>(
        &self,
        k: &Kernel,
        curve: &Curve,
        sol: &Solution<O>,
    ) -> Result<(bool, Option<RangeTangentPoint>)> {
        // low work - Solution already a point solution
        if sol.solution_type == SolutionType::SingleValue {
            return Ok((true, None));
        }

        // locals to run from best solution end to other end
        let ivl = curve.natural_interval();
        let reg = self.natural_uv_domain_with_nurb(k)?;
        let (start_dev, start_par, delta_par) = if sol.start.solution_value < sol.end.solution_value
        {
            let s = sol.start.parameters;
            let e = sol.end.parameters;
            (
                sol.start.solution_value,
                [par(&s, 0)?, par(&s, 1)?, par(&s, 2)?],
                [
                    par(&e, 0)? - par(&s, 0)?,
                    par(&e, 1)? - par(&s, 1)?,
                    par(&e, 2)? - par(&s, 2)?,
                ],
            )
        } else {
            let s = sol.end.parameters;
            let e = sol.start.parameters;
            (
                sol.end.solution_value,
                [par(&s, 0)?, par(&s, 1)?, par(&s, 2)?],
                [
                    par(&e, 0)? - par(&s, 0)?,
                    par(&e, 1)? - par(&s, 1)?,
                    par(&e, 2)? - par(&s, 2)?,
                ],
            )
        };
        let [sp0, sp1, sp2] = start_par;
        let [dp0, dp1, dp2] = delta_par;

        // get Solution segment start points and gapVector (statuses ignored)
        let point0 = curve_point_ignored(k, curve, sp0, Vec3::UNDEF)?;
        let point1 = point_ignored(k, self, Point2::new(sp1, sp2), Vec3::UNDEF)?;
        let xsect_gap = point1 - point0;

        // scale zero to current intersection point location
        let scaled_zero = EFF_ZERO * (1.0 + point0.max_dimension());

        // locals - gap function properties
        let mut surface_point = [Vec3::UNDEF; 3];
        let mut curve_point = [Vec3::UNDEF; 3];
        let mut gap = [Vec3::UNDEF; 3];
        let mut gap_length = [0.0_f64; 3];

        // init classification booleans
        let mut zero = true;
        let mut mono = true;
        let mut g_div = true;

        // init first SamplePoint array entries with the xSect StartPoint
        curve_point[0] = point0;
        surface_point[0] = point1;
        gap[0] = Vec3::new(0.0, 0.0, 0.0); // set gap to exactly zero
        gap_length[0] = 0.0;

        // init walk parameter (a little smaller than a fraction of a power of two)
        let mut delta = 1.0 / 16.0001;

        // init solution classification
        let mut is_degenerate = false;

        // until solution segment is classified
        let mut done = false;
        let mut ii: usize = 1;
        while !done && delta < 1.0 {
            // get the gapFunction sample array index
            let tgt = if ii > 1 { 2 } else { 1 };

            // get the samplePoint Parameter Values
            let t1 = sp0 + delta * dp0;
            let t2 = sp1 + delta * dp1;
            let t3 = sp2 + delta * dp2;

            // get Curve sample point (status ignored)
            let prev = slot(&curve_point, tgt)?;
            set_slot(
                &mut curve_point,
                tgt,
                curve_point_ignored(k, curve, t1, prev)?,
            )?;

            // Find the surface point nearest the curve point (status ignored)
            let (found, solution) = ignored(local_point_solve::<O>(
                k,
                self,
                &reg,
                SolverOperation::Normalize,
                slot(&curve_point, tgt)?,
                Point2::new(t2, t3),
            ))?
            .unwrap_or_default();

            if found {
                // evaluate the nearest surface point
                let drop_point = Point2::new(solution.start.param(0)?, solution.start.param(1)?);
                let prev = slot(&surface_point, tgt)?;
                set_slot(
                    &mut surface_point,
                    tgt,
                    point_ignored(k, self, drop_point, prev)?,
                )?;

                // get the Curve/Surface gapVector: subtract the StartPoint gap from
                // every gap SamplePoint to keep tolerance problems from creating
                // oppositely directed gap vectors
                let g = (slot(&surface_point, tgt)? - slot(&curve_point, tgt)?) - xsect_gap;
                set_slot(&mut gap, tgt, g)?;
                set_slot(&mut gap_length, tgt, g.length())?;
            } else {
                // we've run into problems: return a point solution
                is_degenerate = true;

                // get out of this classification loop
                done = true;
                ii += 1;
                delta *= 2.0;
                continue;
            }

            // once we have 3 sample points
            if ii >= 2 {
                let tgt1 = ((tgt + 3) - 1) % 3;
                let tgt2 = ((tgt + 3) - 2) % 3;
                let len = slot(&gap_length, tgt)?;
                let len1 = slot(&gap_length, tgt1)?;
                let len2 = slot(&gap_length, tgt2)?;

                // 1st: is xSectCurve gapSize zero (coincident curves)
                if len > scaled_zero {
                    zero = false;
                }

                // 2nd: is xSectCurve gap function monotonic
                if slot(&gap, tgt)?.dot(slot(&gap, tgt1)?) <= 0.0 {
                    mono = false;
                }

                // 3rd: is xSectCurve gap function geometrically diverging (growing
                // faster than linear pace)
                if len == 0.0 || len < len1 + 1.0001 * (len1 - len2) {
                    g_div = false;
                }

                // now classify this Curve/Surface XSect GapFunction. The GapFunction is
                // degenerate when it has a nonZero OutsideThisSurface Gap that is
                // monotonically diverging
                if !zero && mono && g_div {
                    is_degenerate = true;
                    done = true;
                } else if zero && mono && g_div {
                    // try another point
                    done = false;

                    // move current guess to last guess
                    surface_point[1] = surface_point[2];
                    curve_point[1] = curve_point[2];
                    gap[1] = gap[2];
                    gap_length[1] = gap_length[2];
                } else {
                    // this side of the curve can have a coincident region
                    is_degenerate = false;
                    done = true;
                }
            }
            ii += 1;
            delta *= 2.0;
        }

        // check to see if interval was classified
        if !done && g_div {
            // range diverged but never got larger than Scaled Zero: let's try throwing
            // away short intervals
            is_degenerate = dp0.abs() < ivl.length() / 10.0
                && dp1.abs() < reg.size().length() / 10.0
                && dp2.abs() < reg.size().length() / 10.0;
        }

        // when soluton is classified as degenerate
        let point = is_degenerate.then_some(RangeTangentPoint {
            solution_value: start_dev,
            parameters: start_par,
        });
        Ok((is_degenerate, point))
    }
}

// ---- SmDerivSurfDefinition ----------------------------------------------------------------

impl DerivSurfDefinition<'_> {
    /// The cross-derivative samples along `boundary_curve` over `boundary_interval` on
    /// `surface` (`GetSampleValues` from a surface boundary curve), for the tangency
    /// types of a surface boundary (`NoTangency`, `TanToEdges`, `MatchNeighbors`,
    /// `DirToReferenceCurve`): at `num_samples + 1` evenly spaced parameters, each
    /// curve point dropped to the surface, the cross direction chosen in the tangent
    /// plane (the binormal to the right with `off_to_the_right`, else to the left;
    /// rotated by the interpolated direction angle; or towards the tangent reference
    /// curve, mirrored per `start_at_boundary`), the surface's directional derivatives
    /// taken along it, the first derivative rescaled by the interpolated tangent
    /// length (times the reference distance) and rotated about the curve tangent by
    /// the interpolated angle. `want_2nd`/`want_3rd` also return the second/third
    /// derivatives (asking for the third evaluates three derivatives).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for another tangency type, an invalid definition, a degenerate curve
    /// tangent, a curve point that does not drop, a degenerate surface normal or
    /// tangent, a curve perpendicular to the surface, a reference curve touching the
    /// boundary or a failed UV direction; those of the evaluations;
    /// `InvalidInput` above [`MAX_BOUNDARY_SAMPLES`] intervals;
    /// [`SolidError::Unported`]: `SmSurface::DropPoint` (`surface.surface_p5`).
    pub fn sample_values(
        &self,
        k: &Kernel,
        surface: &Surface,
        boundary_curve: &Curve,
        boundary_interval: &Extent1d,
        options: SampleOptions,
    ) -> Result<DerivSampleValues> {
        let (values, status) =
            self.sample_values_with_status(k, surface, boundary_curve, boundary_interval, options)?;
        status?;
        Ok(values)
    }

    /// The same sampler with its native partial outputs and status kept separately.
    /// The outer error is a count-safety failure; an ordinary sample
    /// failure is the inner status. `CreateBlendSurface` ignores that status upstream.
    pub(super) fn sample_values_with_status(
        &self,
        k: &Kernel,
        surface: &Surface,
        boundary_curve: &Curve,
        boundary_interval: &Extent1d,
        options: SampleOptions,
    ) -> Result<(DerivSampleValues, Result<()>)> {
        if self.num_samples > MAX_BOUNDARY_SAMPLES {
            return Err(SolidError::invalid_input(
                OP_SAMPLE,
                "too many boundary samples",
            ));
        }
        let count = usize::try_from(self.num_samples)
            .ok()
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| {
                SolidError::invalid_input(OP_SAMPLE, "boundary sample count overflow")
            })?;
        let mut out = DerivSampleValues::default();
        let mut initialized = false;
        let status = (|| {
            type T = TangencyType;
            let SampleOptions {
                off_to_the_right,
                start_at_boundary,
                want_higher: [want_2nd, want_3rd],
            } = options;
            let tt = self.tangency_type;

            // no work - Tangency types for function SmEdgeuse::CreateDerivativeSurface
            if !(tt == T::NoTangency
                || tt == T::TanToEdges
                || tt == T::MatchNeighbors
                || tt == T::DirToReferenceCurve)
            {
                return Err(SolidError::sm_err(OP_SAMPLE));
            }

            // check state
            if !self.assert_valid(None, ObjectKey(0))? {
                return Err(SolidError::sm_err(OP_SAMPLE));
            }

            // the outputs grow sample by sample (upstream sizes them first)
            out = DerivSampleValues {
                second_derivs: want_2nd.then(Vec::new),
                third_derivs: want_3rd.then(Vec::new),
                ..DerivSampleValues::default()
            };
            initialized = true;

            // locals
            let mut uv_guess = Vec2::UNDEF;
            let mut reference_gap_len = 0.0;

            // get number of cross-derivatives to compute
            let num_derivatives: usize = if want_3rd {
                3
            } else if want_2nd {
                2
            } else {
                1
            };

            // for every sample point (+ 1 for the ends)
            let num_samples = self.num_samples;
            let mut ii: u64 = 0;
            while ii <= num_samples {
                let param = if ii < num_samples {
                    ii as f64 / num_samples as f64
                } else {
                    1.0
                };
                let crv_param = boundary_interval.evaluate(param);
                out.params.push(crv_param);

                // BoundaryCurve pos and tan (exact tangent values)
                let crv_pv = boundary_curve.evaluate(k, crv_param, 1, true, false)?;
                let crv_pos = vec_at(&crv_pv, 0)?;
                let mut crv_tan = vec_at(&crv_pv, 1)?;

                // insist on non-degenerate curves
                let scaled_zero = EFF_ZERO * (1.0 + boundary_interval.length());
                if crv_tan.length_squared() <= scaled_zero {
                    return Err(SolidError::sm_err(OP_SAMPLE));
                }

                // project CurvePoint to Surface
                let guess = (ii != 0).then_some(uv_guess);
                let (success, surface_uv0) = drop_point(
                    k,
                    surface,
                    crv_pos,
                    &surface.natural_uv_domain_with_nurb(k)?,
                    guess,
                )?;
                if !success {
                    return Err(SolidError::sm_err(OP_SAMPLE));
                }
                uv_guess = surface_uv0;

                // Surface Normal
                let mut normal = surface.evaluate_normal(k, surface_uv0, Side::Left, Side::Left)?;
                if normal.length_squared() <= EFF_ZERO {
                    return Err(SolidError::sm_err(OP_SAMPLE));
                }
                normal = normal.unitize_or_z();

                // Surface Boundary Binormal
                let mut binormal = if off_to_the_right {
                    crv_tan.cross(normal)
                } else {
                    normal.cross(crv_tan)
                };
                let bi_length = binormal.length();
                let scaled_zero = EFF_ZERO * (1.0 + crv_tan.max_dimension());
                if bi_length <= scaled_zero {
                    return Err(SolidError::sm_err(OP_SAMPLE));
                }
                binormal /= bi_length;

                // switch on the tangency type to pick the directional derivative direction
                let surface_uv1 = match tt {
                    T::NoTangency => {
                        // 2d dir = projection of 3d binormal into surface UV space (status
                        // ignored)
                        let dir = param_direction_ignored(k, surface, surface_uv0, binormal)?;
                        dir.unitize().unwrap_or(dir)
                    }
                    T::TanToEdges | T::MatchNeighbors => {
                        // dir = linear interp between the start and end angles
                        if off_to_the_right {
                            binormal = -binormal;
                        }

                        // Project BoundaryBinormal to SurfaceUV
                        let uv_binormal = surface
                            .compute_param_direction(k, surface_uv0, binormal)
                            .map_err(|e| {
                                if e.is_unported() {
                                    e
                                } else {
                                    SolidError::sm_err(OP_SAMPLE)
                                }
                            })?;
                        let uv_binormal = uv_binormal.unitize().unwrap_or(uv_binormal);

                        // linearly interp the dir rotation angle
                        let ang_rad = deg2rad(
                            (1.0 - param) * self.start_deriv_dir_angle_deg
                                + param * self.end_deriv_dir_angle_deg,
                        );

                        // set directional derivative direction
                        uv_binormal.rotate(ang_rad)
                    }
                    T::DirToReferenceCurve => {
                        // reference point
                        let reference = self
                            .tangent_reference
                            .ok_or_else(|| SolidError::null_pointer(OP_SAMPLE))?;
                        let mut reference_pnt = reference.evaluate_point(k, crv_param)?;
                        let mut reference_gap = reference_pnt - crv_pos;
                        reference_gap_len = reference_gap.length();
                        let scaled_zero = EFF_ZERO * (1.0 + reference_pnt.max_dimension());
                        if reference_gap_len <= scaled_zero {
                            return Err(SolidError::sm_err(OP_SAMPLE));
                        }

                        // Positive Dist = reference gap in same direction as 1st cross-deriv
                        let dist = reference_gap.dot(binormal);

                        if start_at_boundary {
                            // reference pnt behind the curve: mirror it about the plane
                            // perpendicular to the binormal
                            if dist < 0.0 {
                                reference_pnt += binormal * (2.0 * -dist);
                            }
                            reference_gap = reference_pnt - crv_pos;
                        } else {
                            // reference pnt in front of the curve: move in the
                            // (Mirror(RefPnt)-BoundaryCrv) direction
                            if dist > 0.0 {
                                reference_pnt += binormal * (2.0 * -dist);
                            }
                            reference_gap = crv_pos - reference_pnt;
                        }

                        // Project sGap into Surface UV plane (status ignored)
                        let dir = param_direction_ignored(k, surface, surface_uv0, reference_gap)?;
                        dir.unitize().unwrap_or(dir)
                    }
                    _ => return Err(SolidError::sm_err(OP_SAMPLE)),
                };

                // Surface directional derivatives up to the num of derivs being stored
                // (status ignored: the outputs stay zero)
                let mut tgt_values = [Vec3::new(0.0, 0.0, 0.0); 4];
                if let Some(d) = ignored(surface.evaluate_directional_derivs(
                    k,
                    surface_uv0,
                    surface_uv1,
                    num_derivatives,
                    true,
                    true,
                ))? {
                    for (dst, src) in tgt_values.iter_mut().zip(d) {
                        *dst = src;
                    }
                }
                let [tgt0, mut tgt1, tgt2, tgt3] = tgt_values;

                // Do not support degenerate surfaces (upstream scales by 1.0 * max)
                let tgt_1st_len = tgt1.length();
                let scaled_zero = EFF_ZERO * (1.0 * tgt0.max_dimension());
                if tgt_1st_len <= scaled_zero {
                    return Err(SolidError::sm_err(OP_SAMPLE));
                }

                // let positions = Curve Positions
                let position = crv_pos;

                // tangent length - 1. default = mag(Directional 1st-Derivativate)
                //                  2. when asked, linearly interp two given end lengths
                //                  3. when asked, scale by dist(BoundaryCurve, TangentReference)
                let mut tan_length = -1.0;
                if self.start_tan_length > 0.0 {
                    tan_length = (1.0 - param) * self.start_tan_length
                        + param * self.end_tan_scale * self.start_tan_length;
                }
                if self.tangent_reference.is_some() {
                    tan_length *= reference_gap_len;
                }

                // Change tangent length when not using the current length
                if tan_length > 0.0 {
                    tgt1 = tgt1 * tan_length / tgt_1st_len;
                }

                // When asked Rotate Tangent about BaseCurve by Ang
                let rot_angle_deg =
                    self.start_angle_deg + param * (self.end_angle_deg - self.start_angle_deg);
                let unitized = if let Ok(v) = crv_tan.unitize() {
                    crv_tan = v;
                    true
                } else {
                    crv_tan = crv_tan.unitize_or_z();
                    false
                };
                if unitized && rot_angle_deg.abs() > EFF_ZERO {
                    let mut rot = Axis2Placement::default();
                    rot.rotate_about_axis(deg2rad(rot_angle_deg), crv_tan);
                    tgt1 = rot.transform_point(tgt1);
                }

                // set outputs
                out.positions.push(position);
                out.first_derivs.push(tgt1);
                if let Some(v) = out.second_derivs.as_mut() {
                    v.push(tgt2);
                }
                if let Some(v) = out.third_derivs.as_mut() {
                    v.push(tgt3);
                }
                ii += 1;
            }

            Ok(())
        })();
        if initialized {
            // GetSampleValues calls ReSet before SetSize: even the shared blend
            // parameter buffer is zeroed, not inherited from the first sampler.
            // Each unwritten vector slot also remains zero after a sample fails.
            out.params.resize(count, 0.0);
            out.positions.resize(count, Vec3::ZERO);
            out.first_derivs.resize(count, Vec3::ZERO);
            if let Some(v) = out.second_derivs.as_mut() {
                v.resize(count, Vec3::ZERO);
            }
            if let Some(v) = out.third_derivs.as_mut() {
                v.resize(count, Vec3::ZERO);
            }
        }
        Ok((out, status))
    }
}

// ---- SmSSGlobalSolver ---------------------------------------------------------------------

impl<O: SolutionObject> SsGlobalSolver<'_, '_, O> {
    /// `SmSSGlobalSolver::IdenticalSolutions`: the base test, except that two point
    /// solutions differing only in their parameters are the same when their four
    /// surface points coincide within the 3D tolerance and both surfaces are singular
    /// at both solutions, unless both solutions lie on a seam (of either surface).
    /// When only one does, it is preferred; when neither does, the one with the
    /// smaller solution value.
    ///
    /// # Errors
    ///
    /// Those of the base test; `SM_ERR_NULL_POINTER` without surfaces;
    /// [`SolidError::Unported`]: `SmSurface::IsSingularity` (`surface.surface_p5`) for
    /// kinds without an override; those of `IsOnSeam`.
    pub fn solutions_identical(
        &self,
        k: &Kernel,
        sol1: &Solution<O>,
        sol2: &Solution<O>,
    ) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
        // If the parent thinks they're identical, then they are.
        // (the solutions name the two surfaces, `this` and `&crOtherSurface`)
        let (sp_type, preferred) = if let Some(operands) = &self.operands {
            self.base.identical_solutions_in(sol1, sol2, operands)?
        } else {
            let domains = super::p1::SolveOperands {
                operands: [
                    super::p5::surface_operand(&self.base, 0, self.caches[0], self.surface1),
                    super::p5::surface_operand(&self.base, 1, self.caches[1], self.surface2),
                ],
            };
            self.base.identical_solutions_in(sol1, sol2, &domains)?
        };
        if sp_type == SolutionPairType::Identical {
            return Ok((SolutionPairType::Identical, preferred));
        }

        if sp_type == SolutionPairType::DiffParameters
            && sol1.solution_type == SolutionType::SingleValue
            && sol2.solution_type == SolutionType::SingleValue
        {
            let surf1 = self
                .surface1
                .ok_or_else(|| SolidError::null_pointer(OP_SS))?;
            let surf2 = self
                .surface2
                .ok_or_else(|| SolidError::null_pointer(OP_SS))?;

            // At a singularity, the same 3d point can have different uv values.
            let uv1a = Point2::new(sol1.start.param(1)?, sol1.start.param(2)?);
            let uv1b = Point2::new(sol2.start.param(1)?, sol2.start.param(2)?);
            let uv2a = Point2::new(sol1.start.param(3)?, sol1.start.param(4)?);
            let uv2b = Point2::new(sol2.start.param(3)?, sol2.start.param(4)?);

            // Check positions (statuses ignored).
            let pt1a = point_ignored(k, surf1, uv1a, Vec3::UNDEF)?;
            let pt1b = point_ignored(k, surf1, uv1b, Vec3::UNDEF)?;
            let pt2a = point_ignored(k, surf2, uv2a, Vec3::UNDEF)?;
            let pt2b = point_ignored(k, surf2, uv2b, Vec3::UNDEF)?;

            let tol = self.base.tolerance_3d;

            if !pt1a.closer_than(tol, pt2a)
                || !pt1a.closer_than(tol, pt2b)
                || !pt1b.closer_than(tol, pt2a)
                || !pt1b.closer_than(tol, pt2b)
            {
                return Ok((SolutionPairType::DiffParameters, None));
            }

            // 3d points are the same.  Need to check seams and poles. If either is
            // nonsingular, they're distinct.
            let surf1_singular = is_singularity(self.kernel, surf1, uv1a, EFF_ZERO)?.0
                && is_singularity(self.kernel, surf1, uv1b, EFF_ZERO)?.0;
            let surf2_singular = is_singularity(self.kernel, surf2, uv2a, EFF_ZERO)?.0
                && is_singularity(self.kernel, surf2, uv2b, EFF_ZERO)?.0;

            // when neither Surf1 nor Surf2 solutions are on a singularity
            if !surf1_singular && !surf2_singular {
                return Ok((SolutionPairType::DiffParameters, None));
            }

            // We're at a pole.  Distinct only if they're both on a seam. When one is on
            // a seam prefer that. Else prefer the best solution.
            let k = self.kernel;
            let sol1_on_seam = is_on_seam(k, surf1, uv1a)? || is_on_seam(k, surf2, uv2a)?;
            let sol2_on_seam = is_on_seam(k, surf1, uv1b)? || is_on_seam(k, surf2, uv2b)?;

            if sol1_on_seam && !sol2_on_seam {
                return Ok((SolutionPairType::Identical, Some(PreferredSolution::First)));
            }
            if !sol1_on_seam && sol2_on_seam {
                return Ok((SolutionPairType::Identical, Some(PreferredSolution::Second)));
            }
            if !sol1_on_seam && !sol2_on_seam {
                let preferred = if sol1.start.solution_value < sol2.start.solution_value {
                    PreferredSolution::First
                } else {
                    PreferredSolution::Second
                };
                return Ok((SolutionPairType::Identical, Some(preferred)));
            }
        }

        // Identical positions, at a pole on a seam: call them distinct points.
        Ok((sp_type, None))
    }
}

// ---- sm_SCLocalSolve -----------------------------------------------------------------------

/// `sm_SCLocalSolve`: the local curve/surface solves of a curve span and a surface
/// patch for the global solvers. For minimize and maximize, first the end points of
/// the curve interval with position discontinuities (`t_conts[0] <= C0` at the start,
/// `<= DISCONTINUOUS` at the end; a point/surface solve, `sm_SpLocalSolve`) and, with
/// `process_boundary_curves`, the patch boundaries with position discontinuities (a
/// curve/curve solve against the boundary iso curve, `sm_CCLocalSolve`); then, unless
/// `skip_normalize`, curve/surface solves from three guesses (the domain middle with
/// the interval middle and ends; the first guess improved by dropping points between
/// the curve and the surface when one node's box is over three times the other's),
/// each kept when it passes the convergence test (not checked for signed directed
/// minimizes). Each solution has three variables `[t, u, v]`.
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` for an operation other than minimize, maximize, normalize
/// or signed directed minimize (an `SM_ASSERT` upstream); those of the evaluations
/// and solves, including [`SolidError::Unported`] from their missing downstream callees.
pub fn sc_local_solve<O: SolutionObject>(
    k: &Kernel,
    nodes: &ScLocalSolveNodes<'_>,
    uv_domain: &Extent2d,
    u_conts: [ContinuityType; 2],
    v_conts: [ContinuityType; 2],
    curve: &Curve,
    interval: &Extent1d,
    t_conts: [ContinuityType; 2],
    tol_3d: f64,
    opt_vectors: &[Vec3],
    op: SolverOperation,
    skip_normalize: bool,
    process_boundary_curves: bool,
) -> Result<SolutionArray<O>> {
    use SolverOperation as S;

    // check input
    if !(op == S::Minimize
        || op == S::Maximize
        || op == S::Signed3dDirectedMinimize
        || op == S::Normalize)
    {
        return Err(SolidError::invalid_input(
            OP_SC,
            "operation must be minimize, maximize, normalize or signed directed minimize",
        ));
    }

    let mut solutions = SolutionArray::new();
    let bss = nodes.patch_surface;
    let srf_eval = nodes.eval_surface;
    let c0 = code(ContinuityType::C0);
    let discontinuous = ContinuityType::Discontinuous;
    let set = |s: &mut Solution<O>, i: usize, v: f64| -> Result<()> {
        *s.start.param_mut(i)? = v;
        Ok(())
    };

    // for two passes
    for j in 0..2 {
        // Handle curve discontinuities only for min and max
        if op != S::Maximize && op != S::Minimize {
            continue;
        }

        let t_cont = cont_at(t_conts, j)?;
        if code(t_cont) <= c0 {
            // following line prevents multiple solves for points inside curve.
            if j == 1 && t_cont != discontinuous {
                continue;
            }

            let t = if j == 0 {
                interval.min()
            } else {
                interval.max()
            };

            let pnt = curve.evaluate_point(k, t)?;
            let point_sols = sp_local_solve::<O>(
                k,
                nodes,
                uv_domain,
                u_conts,
                v_conts,
                pnt,
                tol_3d,
                op,
                process_boundary_curves,
            )?;

            for sol in point_sols.as_slice() {
                let mut sol = *sol;
                sol.num_variables = 3;
                let s1 = sol.start.param(1)?;
                let s0 = sol.start.param(0)?;
                set(&mut sol, 2, s1)?;
                set(&mut sol, 1, s0)?;
                set(&mut sol, 0, t)?;
                sol.solution_type = SolutionType::SingleValue;
                solutions.add(&sol);
            }
        }

        // Handle surface boundary discontinuities (boundaries coincident with the
        // curve are taken care of by the point on surface testing above).
        if !process_boundary_curves {
            continue;
        }

        let u_cont = cont_at(u_conts, j)?;
        if code(u_cont) <= c0 {
            // Solve for internal curves only once.
            if j == 1 && u_cont != discontinuous {
                continue;
            }

            let (u, from_left) = if j == 1 {
                (uv_domain.max().x, false)
            } else {
                (uv_domain.min().x, true)
            };
            let ic = boundary_iso_curve(k, srf_eval, SurfParamType::U, u, from_left)?;

            if !ic.is_degenerate(k, EFF_ZERO, None)? {
                let v_ivl = Extent1d::from_min_max(uv_domain.min().y, uv_domain.max().y);
                // point/surface is already taken care of in above section.
                let tmp_conts = [ContinuityType::C1C2, ContinuityType::C1C2];
                let cc = super::p2::cc_local_solve::<O>(
                    k, curve, interval, tmp_conts, &ic, &v_ivl, v_conts, op, tol_3d,
                )?;
                for sol in cc.as_slice() {
                    let mut sol = *sol;
                    sol.num_variables = 3;
                    let s1 = sol.start.param(1)?;
                    set(&mut sol, 2, s1)?;
                    set(&mut sol, 1, u)?;
                    sol.solution_type = SolutionType::SingleValue;
                    solutions.add(&sol);
                }
            }
        }

        let v_cont = cont_at(v_conts, j)?;
        if code(v_cont) <= c0 {
            // Solve for internal curves only once.
            if j == 1 && v_cont != discontinuous {
                continue;
            }

            let (v, from_left) = if j == 1 {
                (uv_domain.max().y, false)
            } else {
                (uv_domain.min().y, true)
            };
            let ic = boundary_iso_curve(k, srf_eval, SurfParamType::V, v, from_left)?;

            if !ic.is_degenerate(k, EFF_ZERO, None)? {
                let u_ivl = Extent1d::from_min_max(uv_domain.min().x, uv_domain.max().x);
                // point/surface is already taken care of in above section.
                let tmp_conts = [ContinuityType::C1C2, ContinuityType::C1C2];
                let cc = super::p2::cc_local_solve::<O>(
                    k, curve, interval, tmp_conts, &ic, &u_ivl, u_conts, op, tol_3d,
                )?;
                for sol in cc.as_slice() {
                    let mut sol = *sol;
                    sol.num_variables = 3;
                    set(&mut sol, 2, v)?;
                    sol.solution_type = SolutionType::SingleValue;
                    solutions.add(&sol);
                }
            }
        }
    }

    // If we don't need to do normalize just return here
    if skip_normalize {
        return Ok(solutions);
    }

    // Take three guesses to try to solve for intersection.
    let mid_uv = uv_domain.evaluate(0.5, 0.5);
    let mut uv_guesses = [mid_uv, mid_uv, mid_uv];
    let mut t_guesses = [
        interval.evaluate(0.5),
        interval.evaluate(0.0),
        interval.evaluate(1.0),
    ];

    if let Some(curve_box) = nodes.curve_box {
        let surf_box_size = nodes.surface_box.size().length();
        let curve_box_size = curve_box.size().length();
        let no_opts = PointSolveOptions::default();

        // better start/guess point when one bounding box is much larger than the other
        if surf_box_size > 3.0 * curve_box_size {
            // Drop point from center of curve to surface
            let curve_pnt = curve.evaluate_point(k, interval.evaluate(0.5))?;
            let (found, sol) =
                local_point_solve::<O>(k, bss, uv_domain, S::Minimize, curve_pnt, uv_guesses[0])?;

            if found {
                // Drop point back to curve & replace the first guesses
                uv_guesses[0] = Point2::new(sol.start.param(0)?, sol.start.param(1)?);
                let surf_pnt = bss.evaluate_point(k, uv_guesses[0])?;

                let (found, sol) = curve.local_point_solve::<O>(
                    k,
                    interval,
                    S::Minimize,
                    surf_pnt,
                    &no_opts,
                    t_guesses[0],
                )?;

                if found {
                    t_guesses[0] = sol.start.param(0)?;
                }
            }
        }

        if curve_box_size > 3.0 * surf_box_size {
            // Drop point from center of surface to curve
            let surf_pnt = bss.evaluate_point(k, uv_domain.evaluate(0.5, 0.5))?;
            let (found, sol) = curve.local_point_solve::<O>(
                k,
                interval,
                S::Minimize,
                surf_pnt,
                &no_opts,
                t_guesses[0],
            )?;

            if found {
                // Drop point back to surface
                t_guesses[0] = sol.start.param(0)?;
                let curve_pnt = curve.evaluate_point(k, t_guesses[0])?;
                let (found, sol) = local_point_solve::<O>(
                    k,
                    bss,
                    uv_domain,
                    S::Minimize,
                    curve_pnt,
                    uv_guesses[0],
                )?;
                if found {
                    uv_guesses[0] = Point2::new(sol.start.param(0)?, sol.start.param(1)?);
                }
            }
        }
    }

    for (uv_guess, t_guess) in uv_guesses.into_iter().zip(t_guesses) {
        let found = srf_eval.local_curve_solve(
            k,
            uv_domain,
            curve,
            interval,
            op,
            tol_3d,
            opt_vectors,
            uv_guess,
            t_guess,
        )?;

        let Some(found) = found else {
            continue;
        };
        if op != S::Signed3dDirectedMinimize
            && !super::p7::pass_sc_convergence_test(k, srf_eval, found.uv, curve, found.t)?
        {
            continue;
        }

        let mut sol = Solution::new();
        sol.solution_type = SolutionType::SingleValue;
        sol.start.solution_value = found.distance;
        set(&mut sol, 0, found.t)?;
        set(&mut sol, 1, found.uv.x)?;
        set(&mut sol, 2, found.uv.y)?;
        sol.num_variables = 3;
        solutions.add(&sol);
    }

    Ok(solutions)
}

// ---- helpers -------------------------------------------------------------------------------

/// `rSubIvl` end for a singular parameter (`LocalCurveIntersect`): the end shared
/// with the whole domain when exactly one is, else the end on the side of `value`.
fn singular_end(sub_ivl: &Extent1d, all_ivl: &Extent1d, value: f64) -> f64 {
    let lo_end = are_same(sub_ivl.min(), all_ivl.min());
    let hi_end = are_same(sub_ivl.max(), all_ivl.max());
    if lo_end == hi_end {
        // Either both the same or both different (out in the middle).
        let mid = sub_ivl.mid();
        if value < mid {
            sub_ivl.min()
        } else {
            sub_ivl.max()
        }
    } else if lo_end {
        // One but not both are at the end of the whole domain.  Use that.
        sub_ivl.min()
    } else {
        sub_ivl.max()
    }
}

/// `sKnots[i]`.
fn knot(knots: &[f64], i: usize) -> Result<f64> {
    knots
        .get(i)
        .copied()
        .ok_or_else(|| SolidError::sm_err(OP_CONT))
}

/// `SmTArray::FindElement`: the first index of an exactly equal value.
fn find_element(values: &[f64], target: f64) -> Option<usize> {
    values.iter().position(|&v| v == target)
}

/// `pIsoCrv->GetKnots(sIsoKnots)` with its status ignored (no knots on failure).
fn iso_knots_of(k: &Kernel, curve: &Curve) -> Result<Vec<f64>> {
    Ok(ignored(curve.knots(k, None))?
        .map(|(k, _)| k)
        .unwrap_or_default())
}

/// `aeConts[j]`.
fn cont_at(conts: [ContinuityType; 2], j: usize) -> Result<ContinuityType> {
    conts
        .get(j)
        .copied()
        .ok_or_else(|| SolidError::sm_err(OP_SC))
}

/// Entry `i` of an evaluation result.
fn vec_at(v: &[Vec3], i: usize) -> Result<Vec3> {
    v.get(i).copied().ok_or_else(|| {
        SolidError::invalid_input("surface::surface::p6::evaluate", "evaluation too short")
    })
}

/// `m_adParameters[i]`.
fn par(p: &[f64], i: usize) -> Result<f64> {
    p.get(i).copied().ok_or_else(|| {
        SolidError::invalid_input("surface::surface::p6::parameters", "parameter index")
    })
}

/// Sample slot `i` of a three-entry gap-function array.
fn slot<T: Copy>(a: &[T; 3], i: usize) -> Result<T> {
    a.get(i)
        .copied()
        .ok_or_else(|| SolidError::invalid_input("surface::surface::p6::classify", "sample index"))
}

fn set_slot<T>(a: &mut [T; 3], i: usize, v: T) -> Result<()> {
    let s = a.get_mut(i).ok_or_else(|| {
        SolidError::invalid_input("surface::surface::p6::classify", "sample index")
    })?;
    *s = v;
    Ok(())
}

/// `EvaluatePoint(uv, pt)` with its status ignored: `prev` on failure.
fn point_ignored(k: &Kernel, surface: &Surface, uv: Point2, prev: Point3) -> Result<Point3> {
    Ok(ignored(surface.evaluate_point(k, uv))?.unwrap_or(prev))
}

/// `crCurve.EvaluatePoint(t, pt)` with its status ignored: `prev` on failure.
fn curve_point_ignored(k: &Kernel, curve: &Curve, t: f64, prev: Point3) -> Result<Point3> {
    Ok(ignored(curve.evaluate_point(k, t))?.unwrap_or(prev))
}

/// `EvaluateNormal(uv, FALSE, FALSE, n)` with its status ignored: `prev` on failure.
fn normal_ignored(k: &Kernel, surface: &Surface, uv: Point2, prev: Vec3) -> Result<Vec3> {
    Ok(ignored(surface.evaluate_normal(k, uv, Side::Right, Side::Right))?.unwrap_or(prev))
}

/// `ComputeParamDirection(uv, vec, dir)` with its status ignored: the cleared output
/// `(0, 0)` on failure.
fn param_direction_ignored(k: &Kernel, surface: &Surface, uv: Point2, vec: Vec3) -> Result<Vec2> {
    Ok(ignored(surface.compute_param_direction(k, uv, vec))?.unwrap_or(Vec2::new(0.0, 0.0)))
}

/// `SmIsoCurve(*pSrfEval, dir, value, bFromLeft)` (input clamped) as a curve.
pub(super) fn boundary_iso_curve(
    k: &Kernel,
    surface: &Surface,
    dir: SurfParamType,
    value: f64,
    from_left: bool,
) -> Result<Curve> {
    Ok(Curve::from(IsoCurve::from_surface(
        k,
        surface.clone_for_evaluation()?,
        dir,
        value,
        from_left,
        true,
    )?))
}

/// `CreateIsoParametricCurve(ctx, dir, value, 0.0, pCurve, pOptDomain, &maxGap)`
/// (virtual): the curve and its achieved maximum gap.
pub(super) fn create_iso_parametric_curve(
    k: &Kernel,
    surface: &Surface,
    dir: SurfParamType,
    value: f64,
    domain: Option<&Extent2d>,
) -> Result<(Curve, f64)> {
    match surface.kind_with_nurb(k)? {
        SurfaceKind::BSpline(b) => {
            let iso = b.create_iso_parametric_curve(
                k,
                dir,
                value,
                IsoCurveOptions {
                    domain: domain.copied(),
                    uv_iso_curve: false,
                },
            )?;
            Ok((Curve::from(iso.curve), iso.max_gap_3d))
        }
        // (SmSurfOfExtrusion inherits the B-spline override, whose
        // `GetOrCreateGwNurbPointer` makes a missing NURB: a recognised extrusion has
        // none until then)
        SurfaceKind::SurfOfExtrusion(s) => {
            let iso = s.base_with_nurb(k)?.create_iso_parametric_curve(
                k,
                dir,
                value,
                IsoCurveOptions {
                    domain: domain.copied(),
                    uv_iso_curve: false,
                },
            )?;
            Ok((Curve::from(iso.curve), iso.max_gap_3d))
        }
        SurfaceKind::Plane(p) => {
            let iso = p.create_iso_parametric_curve(dir, value, domain, false)?;
            Ok((Curve::from(iso.curve), iso.max_gap_3d))
        }
        SurfaceKind::Cone(s) => {
            let iso = s.create_iso_parametric_curve(k, dir, value, domain, false)?;
            Ok((iso.curve, iso.max_gap_3d))
        }
        SurfaceKind::Sphere(s) => {
            let iso = s.create_iso_parametric_curve(k, dir, value, domain, false)?;
            Ok((iso.curve, iso.max_gap_3d))
        }
        SurfaceKind::Torus(s) => {
            let iso = s.create_iso_parametric_curve(k, dir, value, domain, false)?;
            Ok((iso.curve, iso.max_gap_3d))
        }
        SurfaceKind::SurfOfRevolution(s) => {
            let iso = s.create_iso_parametric_curve(k, dir, value, domain)?;
            Ok((iso.curve, iso.max_gap_3d))
        }
        SurfaceKind::OffsetSurface(_)
        | SurfaceKind::CurveBounded(_)
        | SurfaceKind::SrfInVolume(_)
        | SurfaceKind::Step(_) => {
            // the SmSurface body
            let iso = surface.create_iso_parametric_curve(k, dir, value, 0.0, domain, false)?;
            Ok((iso.curve, iso.max_gap_3d))
        }
    }
}

/// `IsSingularity(uv, eSingDir, tol)` (virtual): the plane override (a plane has no
/// singularity), the base body otherwise.
fn is_singularity(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    tol: f64,
) -> Result<(bool, SurfParamType)> {
    match surface.kind() {
        SurfaceKind::Plane(p) => Ok(p.is_singularity()),

        SurfaceKind::Cone(_)
        | SurfaceKind::Sphere(_)
        | SurfaceKind::Torus(_)
        | SurfaceKind::BSpline(_)
        | SurfaceKind::CurveBounded(_)
        | SurfaceKind::SrfInVolume(_)
        | SurfaceKind::SurfOfRevolution(_)
        | SurfaceKind::SurfOfExtrusion(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_) => {
            let r = surface.is_singularity(k, uv, tol, false)?;
            Ok((r.singular, r.direction))
        }
    }
}

/// `IsOnSeam(uv)` (virtual, default arguments): the plane override (a plane has no
/// seam), else the base body.
fn is_on_seam(k: &Kernel, surface: &Surface, uv: Point2) -> Result<bool> {
    match surface.kind() {
        SurfaceKind::Plane(p) => Ok(p.is_on_seam().0),

        SurfaceKind::Cone(_)
        | SurfaceKind::Sphere(_)
        | SurfaceKind::Torus(_)
        | SurfaceKind::BSpline(_)
        | SurfaceKind::CurveBounded(_)
        | SurfaceKind::SrfInVolume(_)
        | SurfaceKind::SurfOfRevolution(_)
        | SurfaceKind::SurfOfExtrusion(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_) => Ok(surface.is_on_seam(k, uv, None)?.on_seam),
    }
}

/// `DropPoint(pt, domain, guess, bSuccess, uv, gap, bIsMulti)` (default
/// `SM_SO_MINIMIZE`): success and the UV point.
fn drop_point(
    k: &Kernel,
    surface: &Surface,
    pt: Point3,
    domain: &Extent2d,
    guess: Option<Point2>,
) -> Result<(bool, Point2)> {
    let opts = DropPointOptions {
        guess_uv: guess,
        ..Default::default()
    };
    let d = surface.drop_point::<NoObject>(k, pt, domain, &opts)?;
    Ok((d.success, d.result_uv))
}

/// `LocalPointSolve(domain, op, point, guess, bFoundAnswer, sSolution)` (virtual; no
/// ported kind overrides it): whether an answer was found and the solution.
fn local_point_solve<O: SolutionObject>(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    op: SolverOperation,
    point: Point3,
    guess: Point2,
) -> Result<(bool, Solution<O>)> {
    let r = surface.local_point_solve::<O>(k, domain, op, point, guess)?;
    Ok((r.found, r.solution))
}

/// `GlobalPointSolve(domain, SM_SO_MINIMIZE, pt, tol, NULL, SM_SR_SINGLE, sols)`
/// (virtual; no ported kind overrides it): the solutions' start ends.
fn global_point_solve_single(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    pt: Point3,
    tol: f64,
) -> Result<Vec<SolutionEnd>> {
    global_point_solve_starts(
        k,
        surface,
        domain,
        SolverOperation::Minimize,
        pt,
        tol,
        SolutionRequested::Single,
    )
}

/// `sm_SpLocalSolve(pSurfaceNode, pBezPatch, domain, uConts, vConts, pnt, tol, op,
/// TRUE, bProcessBoundaryCurves, sols)`.
fn sp_local_solve<O: SolutionObject>(
    k: &Kernel,
    nodes: &ScLocalSolveNodes<'_>,
    domain: &Extent2d,
    u_conts: [ContinuityType; 2],
    v_conts: [ContinuityType; 2],
    pnt: Point3,
    tol: f64,
    op: SolverOperation,
    process_boundary_curves: bool,
) -> Result<SolutionArray<O>> {
    crate::surface::surface::p5::sp_local_solve::<O>(
        k,
        nodes.eval_surface,
        domain,
        u_conts,
        v_conts,
        pnt,
        tol,
        op,
        true,
        process_boundary_curves,
    )
}

/// The Newton solve of `LocalCurveIntersect`: `SmLocalSolveNd` over
/// `SmCSIntersectENFO(curve, surface, squared_tol)` with the clamped (non-periodic)
/// `intervals`, from `guess`, to `SolveIt`'s tolerance `solve_tol` (desired accuracy
/// `SM_EFF_ZERO / 10`, step cutting, 6 total boundary hits, 20 iterations): the
/// solution `[t, u, v]`, `None` when not found.
fn cs_intersect_local_solve(
    k: &Kernel,
    surface: &Surface,
    curve: &Curve,
    squared_tol: f64,
    intervals: &[Extent1d; 3],
    guess: &[f64; 3],
    solve_tol: f64,
) -> Result<Option<[f64; 3]>> {
    let mut bounds = ExtentNd::new(3);
    for (i, ivl) in intervals.iter().enumerate() {
        if let Some(slot) = bounds.get_mut(i) {
            *slot = *ivl;
        }
    }
    // Local solvers should not jump over seams.
    let periodicities = [false; 3];
    let mut equations = CsIntersectEnfo::new(k, curve, surface, squared_tol);
    let mut solver = LocalSolveNd::new(&mut equations, Some(&bounds), Some(&periodicities));
    solver.set_desired_accuracy(EFF_ZERO / 10.0);
    solver.set_step_cutting_enabled(true);
    solver.set_boundary_handler(BoundaryHandler::TotalBoundaryHits, 6);
    solver.set_maximum_iterations(20);

    // Solve it
    let solution = solver.solve_it(guess, solve_tol)?;
    if !solution.found {
        return Ok(None);
    }
    match solution.x.as_slice() {
        [t, u, v, ..] => Ok(Some([*t, *u, *v])),
        _ => Err(SolidError::sm_err(OP_CS_INTERSECT)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::b_spline_surface::BSplineSurface;
    use crate::nurbs::math::{CPoint, NOW};
    use crate::nurbs::srf_basic::Surface as NlSurface;
    use crate::surface::plane::Plane;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Obj(u8);

    impl SolutionObject for Obj {
        fn kind(&self) -> crate::geom::solution_array::ObjectKind {
            crate::geom::solution_array::ObjectKind::Surface
        }
        fn natural_uv_domain(&self) -> Option<Extent2d> {
            Some(Extent2d::from_points(
                Point2::new(0.0, 0.0),
                Point2::new(1.0, 1.0),
            ))
        }
    }

    fn plane() -> Surface {
        Surface::from(Plane::from_normal(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        ))
    }

    /// A bicubic patch with a kink-free interior knot in each direction.
    fn bspline() -> Surface {
        let net = (0..5)
            .map(|i| {
                (0..4)
                    .map(|j| {
                        let x = f64::from(i);
                        let y = f64::from(j);
                        CPoint::new(x, y, 0.1 * x * y, NOW)
                    })
                    .collect()
            })
            .collect();
        let nl = NlSurface::new(
            net,
            3,
            3,
            vec![0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0],
            vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        )
        .unwrap();
        Surface::from(BSplineSurface::from_nurb(nl, false))
    }

    #[test]
    fn continuities_of_smooth_bspline() {
        let k = Kernel::default();
        let s = bspline();
        let (min, conts) = s.calculate_continuities(&k, SurfParamType::U).unwrap();
        assert_eq!(conts.len(), 3);
        assert_eq!(conts[0], ContinuityType::Discontinuous);
        assert_eq!(conts[2], ContinuityType::Discontinuous);
        assert!(code(min) >= code(ContinuityType::G1));
    }

    #[test]
    fn plane_trim_loop_has_four_sides() {
        let k = Kernel::default();
        let s = plane();
        let dom = Extent2d::from_points(Point2::new(0.0, 0.0), Point2::new(2.0, 1.0));
        let loop_u = s
            .create_natural_uv_trim_curves(&k, &dom, SurfParamType::U, false)
            .unwrap();
        assert_eq!(loop_u.curves_3d.len(), 4);
        assert_eq!(
            loop_u.orients,
            vec![
                OrientType::Opposite,
                OrientType::Same,
                OrientType::Same,
                OrientType::Opposite
            ]
        );
        let loop_v = s
            .create_natural_uv_trim_curves(&k, &dom, SurfParamType::V, true)
            .unwrap();
        assert_eq!(
            loop_v.orients,
            vec![
                OrientType::Same,
                OrientType::Same,
                OrientType::Opposite,
                OrientType::Opposite
            ]
        );
    }

    #[test]
    fn point_solution_is_degenerate() {
        let s = plane();
        let c = Curve::from(
            Line::create_line_segment(3, Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), None)
                .unwrap(),
        );
        let mut sol: Solution<Obj> = Solution::new();
        sol.solution_type = SolutionType::SingleValue;
        let (degenerate, point) = s
            .classify_intersecting_range_solution(&Kernel::default(), &c, &sol)
            .unwrap();
        assert!(degenerate);
        assert!(point.is_none());
    }

    #[test]
    fn plane_pairs_with_distinct_points_differ() {
        let k = Kernel::default();
        let s1 = plane();
        let s2 = plane();
        let mut base = crate::geom::global_solver::GlobalSolver::new();
        base.tolerance_3d = 1e-6;
        let solver = SsGlobalSolver {
            base,
            surface1: Some(&s1),
            surface2: Some(&s2),
            uv_domain1: s1.natural_uv_domain_if_built(),
            uv_domain2: s2.natural_uv_domain_if_built(),
            kernel: &k,
            caches: [None, None],
            face_queries: None,
            operands: None,
        };
        let mk = |a: f64| {
            let mut s: Solution<Obj> = Solution::new();
            s.solution_type = SolutionType::SingleValue;
            s.num_variables = 4;
            s.num_objects = 2;
            s.objects[0] = Some(Obj(0));
            s.objects[1] = Some(Obj(1));
            s.start.parameters[0] = a;
            s.start.parameters[1] = a;
            s.start.parameters[2] = a;
            s.start.parameters[3] = a;
            s
        };
        let (t, p) = solver.solutions_identical(&k, &mk(0.1), &mk(0.9)).unwrap();
        assert_eq!(t, SolutionPairType::DiffParameters);
        assert!(p.is_none());
    }

    #[test]
    fn sample_values_rejects_base_curve_types() {
        let s = plane();
        let c = Curve::from(
            Line::create_line_segment(3, Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), None)
                .unwrap(),
        );
        let d = DerivSurfDefinition {
            tangency_type: TangencyType::AlongVector,
            ..DerivSurfDefinition::default()
        };
        let e = d
            .sample_values(
                &Kernel::default(),
                &s,
                &c,
                &c.natural_interval(),
                SampleOptions {
                    off_to_the_right: true,
                    start_at_boundary: true,
                    ..SampleOptions::default()
                },
            )
            .unwrap_err();
        assert!(!e.is_unported());
    }

    #[test]
    fn sample_status_retains_native_zero_filled_tail() {
        let k = Kernel::default();
        let surface = plane();
        let boundary =
            Curve::from(Line::create_line_segment(3, Vec3::ZERO, Vec3::X, None).unwrap());
        let options = SampleOptions {
            off_to_the_right: true,
            start_at_boundary: true,
            want_higher: [true, true],
        };
        // Touch the boundary at the first, an interior, or the last sample.
        for fail_at in [0, 2, 4] {
            let t = f64::from(fail_at) / 4.0;
            let reference = Curve::from(
                Line::create_line_segment(
                    3,
                    Vec3::new(0.0, -t, 0.0),
                    Vec3::new(1.0, 1.0 - t, 0.0),
                    None,
                )
                .unwrap(),
            );
            let def = DerivSurfDefinition {
                num_samples: 4,
                tangency_type: TangencyType::DirToReferenceCurve,
                tangent_reference: Some(&reference),
                ..Default::default()
            };
            let (values, status) = def
                .sample_values_with_status(
                    &k,
                    &surface,
                    &boundary,
                    &boundary.natural_interval(),
                    options,
                )
                .unwrap();
            assert!(status.is_err());
            assert!(
                def.sample_values(
                    &k,
                    &surface,
                    &boundary,
                    &boundary.natural_interval(),
                    options,
                )
                .is_err(),
                "public sampler must still report failure"
            );
            assert_eq!(values.params.len(), 5);
            assert_eq!(values.positions.len(), 5);
            assert_eq!(values.first_derivs.len(), 5);
            let second = values.second_derivs.as_ref().unwrap();
            let third = values.third_derivs.as_ref().unwrap();
            assert_eq!((second.len(), third.len()), (5, 5));
            for i in 0..5 {
                if i <= fail_at as usize {
                    assert_eq!(values.params[i], i as f64 / 4.0);
                } else {
                    assert_eq!(values.params[i], 0.0);
                }
                if i < fail_at as usize {
                    assert_eq!(values.positions[i], Vec3::new(i as f64 / 4.0, 0.0, 0.0));
                    assert!(values.first_derivs[i].length_squared() > 0.0);
                } else {
                    assert_eq!(values.positions[i], Vec3::ZERO);
                    assert_eq!(values.first_derivs[i], Vec3::ZERO);
                }
                assert_eq!(second[i], Vec3::ZERO);
                assert_eq!(third[i], Vec3::ZERO);
            }
        }
    }

    #[test]
    fn boundary_sample_count_is_bounded_before_padding() {
        let surface = plane();
        let boundary =
            Curve::from(Line::create_line_segment(3, Vec3::ZERO, Vec3::X, None).unwrap());
        for num_samples in [MAX_BOUNDARY_SAMPLES + 1, u64::MAX] {
            let def = DerivSurfDefinition {
                num_samples,
                ..Default::default()
            };
            assert!(matches!(
                def.sample_values(
                    &Kernel::default(),
                    &surface,
                    &boundary,
                    &boundary.natural_interval(),
                    SampleOptions::default()
                ),
                Err(SolidError::InvalidInput {
                    reason: "too many boundary samples",
                    ..
                })
            ));
        }
    }

    #[test]
    fn singular_end_prefers_shared_domain_end() {
        let all = Extent1d::from_min_max(0.0, 1.0);
        assert_eq!(
            singular_end(&Extent1d::from_min_max(0.0, 0.5), &all, 0.4),
            0.0
        );
        assert_eq!(
            singular_end(&Extent1d::from_min_max(0.5, 1.0), &all, 0.6),
            1.0
        );
        assert_eq!(
            singular_end(&Extent1d::from_min_max(0.2, 0.6), &all, 0.5),
            0.6
        );
    }
}
