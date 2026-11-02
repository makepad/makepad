//! Surface-point solving, part 1: the problem data of a surface-point solve
//! ([`SurfacePointSolver`]), the global solver's construction, closure cache, objective
//! function, control-net guesses, seam and stride checks, local extremum and boundary
//! tests ([`SurfPtGlobalSolver`]), and the local iteration stepper's construction,
//! accessors, lazy evaluation, clamping, boundary bookkeeping and iso-curve drop
//! ([`SurfPtSolveStepper`]), with the file-static control-net helpers.
//!
//! Upstream: `source/SMLib/src/SmSurfacePointSolve.cpp`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`SurfacePointSolver`], [`SurfacePointSolver::new`] | `SmSurfacePointSolver` (constructor)           |
//! | `Clone`, `Clone::clone_from`, `Drop` (implicit)   | `SmSurfacePointSolver` copy constructor, `operator=`, destructor |
//! | [`SurfacePointSolver::surface`], [`SurfacePointSolver::test_point`], [`SurfacePointSolver::domain`], [`SurfacePointSolver::solver_op`], [`SurfacePointSolver::tight_tol`], [`SurfacePointSolver::loose_tol`] | `SurfacePtr`, `TestPoint`, `Domain`, `SolverOp`, `TightTol`, `LooseTol` |
//! | [`SurfacePointSolver::dir_vec`], [`SurfacePointSolver::b_spline`] | `m_sDirVec`, `m_pBSplSrf` (protected members) |
//! | [`SurfPtGlobalSolver`], [`SurfPtGlobalSolver::new`] | `SmSurfPtGlobalSolver` (constructor)           |
//! | [`SurfPtGlobalSolver::closed_surface`]            | `SmSurfPtGlobalSolver::ClosedSurface`            |
//! | [`SurfPtGlobalSolver::calc_func_val`]             | `SmSurfPtGlobalSolver::CalcFuncVal`              |
//! | [`SurfPtGlobalSolver::find_polygon_triangle_guess`], [`PolygonTriangleGuess`] | `SmSurfPtGlobalSolver::FindPolygonTriangleGuess` |
//! | [`SurfPtGlobalSolver::refine_guess_polygon_leg`]  | `SmSurfPtGlobalSolver::RefineGuessPolygonLeg`    |
//! | [`SurfPtGlobalSolver::check_bad_guess_stride`]    | `SmSurfPtGlobalSolver::CheckBadGuessStride`      |
//! | [`SurfPtGlobalSolver::check_guess_for_seams`]     | `SmSurfPtGlobalSolver::CheckGuessForSeams` (UV point) |
//! | [`SurfPtGlobalSolver::check_guess_for_seams_cpts`] | `SmSurfPtGlobalSolver::CheckGuessForSeams` (control points) |
//! | [`SurfPtGlobalSolver::crease_subdomains`]         | `SmSurfPtGlobalSolver::GetCreaseSubdomains`      |
//! | [`SurfPtGlobalSolver::is_local_min`], [`SurfPtGlobalSolver::is_local_max`] | `SmSurfPtGlobalSolver::IsLocalMin`, `IsLocalMax` |
//! | [`SurfPtGlobalSolver::pushing_boundary`]          | `SmSurfPtGlobalSolver::PushingBoundary`          |
//! | [`triangles_dist`]                                | `GetTrianglesDist` (static)                      |
//! | [`well_behaved_control_net`]                      | `WellBehavedControlNet` (static)                 |
//! | [`ErrorNormType`]                                 | `SmSrfPtErrorNormType`                           |
//! | [`SurfPtSolveStepper`], [`SurfPtSolveStepper::new`] | `SmSurfPtSolveStepper` (constructor)           |
//! | [`SurfPtSolveStepper::copy_of`], [`SurfPtSolveStepper::assign`], `Drop` (implicit) | copy constructor, `operator=`, destructor |
//! | [`SurfPtSolveStepper::set_uv`], [`SurfPtSolveStepper::uv`], [`SurfPtSolveStepper::max_step`] | `SetUV`, `UV`, `MaxStep` |
//! | [`SurfPtSolveStepper::pos`], [`SurfPtSolveStepper::su`], [`SurfPtSolveStepper::sv`], [`SurfPtSolveStepper::suu`], [`SurfPtSolveStepper::suv`], [`SurfPtSolveStepper::svv`], [`SurfPtSolveStepper::normal`] | `Pos`, `Su`, `Sv`, `Suu`, `Suv`, `Svv`, `Normal` |
//! | [`SurfPtSolveStepper::gap`], [`SurfPtSolveStepper::gap_u`], [`SurfPtSolveStepper::gap_v`], [`SurfPtSolveStepper::gap_length`] | `Gap`, `GapU`, `GapV`, `GapLength` |
//! | [`SurfPtSolveStepper::iter_count`], [`SurfPtSolveStepper::set_iter_count`], [`SurfPtSolveStepper::nr_step_suspect`], [`SurfPtSolveStepper::set_nr_suspect`] | `IterCount`, `SetIterCount`, `NRStepSuspect`, `SetNRSuspect` |
//! | [`SurfPtSolveStepper::bound_hits_u`], [`SurfPtSolveStepper::bound_hits_v`] | `BoundHitsU`, `BoundHitsV`  |
//! | [`SurfPtSolveStepper::eval`], [`StepperValues`]   | `SmSurfPtSolveStepper::Eval` (private)           |
//! | [`SurfPtSolveStepper::clamp`]                     | `Clamp`                                          |
//! | [`SurfPtSolveStepper::hit_bound`]                 | `HitBound`                                       |
//! | [`SurfPtSolveStepper::check_bounds`], [`BoundsCheck`] | `CheckBounds`                                |
//! | [`SurfPtSolveStepper::drop_to_iso_curve`]         | `DropToIsoCurve`                                 |
//!
//! Deviations:
//! - **Object model.** The classes are plain structs: the global solver and the
//!   stepper hold a [`SurfacePointSolver`] (`base`) instead of deriving from it. The
//!   surface is borrowed for the life of the solver (`m_pSrf`, not owned upstream
//!   either); `m_pBSplSrf` is the B-spline part of a B-spline or plane with a NURB.
//!   An `SmOffsetSurface` contributes its base's B-spline part (one level, as
//!   upstream).
//! - **Out-params** become return values: [`PolygonTriangleGuess`],
//!   [`BoundsCheck`], `(bool, SurfParamType)` from the boundary test, the new grid
//!   point of the stride check. Upstream's in/out best indices of
//!   `FindPolygonTriangleGuess` (kept when no net point qualifies) are passed in.
//!   The constructor keeps the unused `cpdOptTargetDistance` and
//!   `eSolutionRequested` (`NotUsed` upstream).
//! - **Ignored statuses.** Where upstream ignores an evaluation's status the outputs
//!   keep their previous values (initially undefined, [`Vec3::UNDEF`]), and an
//!   `SmSurfaceEval` accessor reads the values it holds; [`SolidError::Unported`]
//!   still propagates. A failing `CalculateBoundingBox` of the directed-maximise
//!   constructor leaves the box in its initial (inside-out) state, as upstream.
//! - **Unported callees.** The base
//!   `SmSurface::CreateIsoParametricCurve` (`surface.surface_p5`, the iso curve of a
//!   curve-bounded or STEP surface) return [`SolidError::Unported`].
//! - **`DropToIsoCurve`** passes its guess parameter by pointer and its output
//!   parameter by reference to the same variable: `SmCurve::DropPoint` zeroes the
//!   output first, so the guess is always `0`. The port passes `0` explicitly.
//! - **Control points.** An index past the control net (upstream reads past the
//!   array) returns [`SolidError::InvalidInput`].
//! - Debug-only drawing and `SM_ASSERT`s are compiled out of the release build and
//!   omitted. `Dump` has no counterpart.

use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::{Extent2d, SS_UMAX, SS_UMIN, SS_VMAX, SS_VMIN};
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::{line_closest_point, vec_lin_comb_two_vectors};
use crate::geom::geom_utility::p2::{triangle_point_distance, ContinuityType};
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::tol::ANG_TOL_DEG;
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT, UNDEF_DOUBLE};
use crate::nurbs::b_spline_surface::BSplineSurface;
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::math::CPoint;
use crate::surface::surface::p2::SurfaceEval;
use crate::surface::surface::{Surface, SurfaceKind};

const OP_CPTS: &str = "surface::surface_point_solve::p1::control_point";
const OP_TRI: &str =
    "surface::surface_point_solve::p1::SurfPtGlobalSolver::find_polygon_triangle_guess";
const OP_LEG: &str =
    "surface::surface_point_solve::p1::SurfPtGlobalSolver::refine_guess_polygon_leg";

/// `SM_CONTINUITY_ANGLE`, the default angle tolerance of `CalculateContinuities`.
const CONTINUITY_ANGLE: f64 = ANG_TOL_DEG;

// ---- SmSurfacePointSolver ---------------------------------------------------------------

/// The data common to a surface-point solve (`SmSurfacePointSolver`): the surface, its
/// domain, the test point, the operation and the tolerances.
///
/// The allowed operations are [`SolverOperation::Minimize`],
/// [`SolverOperation::Normalize`], [`SolverOperation::Intersect`] and (partly)
/// [`SolverOperation::Maximize`] and [`SolverOperation::Signed3dDirectedMaximize`].
#[derive(Debug, Clone)]
pub struct SurfacePointSolver<'s> {
    /// `m_pSrf` (not owned).
    surface: &'s Surface,
    /// `m_sDomain`.
    domain: Extent2d,
    /// `m_sTestPt`.
    test_pt: Point3,
    /// `m_sDirVec`: for the directed operation.
    dir_vec: Vec3,
    /// `m_eSolverOp`.
    solver_op: SolverOperation,
    /// `m_pBSplSrf`: the surface as a B-spline with a NURB, if it is one.
    b_spline: Option<&'s BSplineSurface>,
    /// `m_dTightTol`.
    tight_tol: f64,
    /// `m_dLooseTol`.
    loose_tol: f64,
}

impl<'s> SurfacePointSolver<'s> {
    /// The problem data (`SmSurfacePointSolver(pSurf, crUVDomain, eSolverOp,
    /// crTestPoint, dDistanceTolerance, cpdOptTargetDistance, eSolutionRequested)`).
    ///
    /// For [`SolverOperation::Signed3dDirectedMaximize`] the test point is the
    /// direction to maximise along; the stored direction is its unitised negation and
    /// the stored test point a corner of the surface's bounding box, expanded by 20%,
    /// on the far side from that direction.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the bounding box (directed operation only; its
    /// other failures are ignored, as upstream).
    pub fn new(
        k: &Kernel,
        surface: &'s Surface,
        uv_domain: &Extent2d,
        solver_op: SolverOperation,
        test_point: Point3,
        distance_tolerance: f64,
        _opt_target_distance: Option<f64>,
        _solution_requested: SolutionRequested,
    ) -> Result<Self> {
        // We need an SmBSplineSurface for some operations.
        let b_spline = b_spline_part(surface);

        let mut solver = Self {
            surface,
            domain: *uv_domain,
            test_pt: test_point,
            dir_vec: Vec3::ZERO,
            solver_op,
            b_spline,
            // Tight tolerance: tight, but it should be achievable.
            tight_tol: EFF_ZERO * (1.0 + test_point.max_dimension()),
            loose_tol: distance_tolerance,
        };

        if solver_op == SolverOperation::Signed3dDirectedMaximize {
            // The direction comes in as the test point; to maximise along it we
            // minimise along its negation.
            solver.dir_vec = (-test_point).unitize_or_z();

            // A point to define a plane: a corner of the expanded bounding box.
            // (status ignored: a failing box is left in its initial, inside-out state)
            let mut bbox =
                match surface.calculate_bounding_box(k, &solver.domain, [true, false, false], true)
                {
                    Ok(boxes) => boxes.normal_box.unwrap_or_else(Extent3d::new),
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => Extent3d::new(),
                };
            bbox.expand_relative(0.2);

            let d = solver.dir_vec;
            solver.test_pt.x = if d.x > 0.0 {
                bbox.min().x
            } else {
                bbox.max().x
            };
            solver.test_pt.y = if d.y > 0.0 {
                bbox.min().y
            } else {
                bbox.max().y
            };
            solver.test_pt.z = if d.z > 0.0 {
                bbox.min().z
            } else {
                bbox.max().z
            };
        }
        Ok(solver)
    }

    /// The surface (`SurfacePtr`).
    #[must_use]
    pub const fn surface(&self) -> &'s Surface {
        self.surface
    }

    /// The test point (`TestPoint`).
    #[must_use]
    pub const fn test_point(&self) -> Point3 {
        self.test_pt
    }

    /// The UV domain (`Domain`).
    #[must_use]
    pub const fn domain(&self) -> &Extent2d {
        &self.domain
    }

    /// The operation (`SolverOp`).
    #[must_use]
    pub const fn solver_op(&self) -> SolverOperation {
        self.solver_op
    }

    /// The tight tolerance (`TightTol`).
    #[must_use]
    pub const fn tight_tol(&self) -> f64 {
        self.tight_tol
    }

    /// The loose tolerance (`LooseTol`).
    #[must_use]
    pub const fn loose_tol(&self) -> f64 {
        self.loose_tol
    }

    /// The direction of the directed operation, zero otherwise (`m_sDirVec`).
    #[must_use]
    pub const fn dir_vec(&self) -> Vec3 {
        self.dir_vec
    }

    /// The surface as a B-spline with a NURB, if it is one (`m_pBSplSrf`).
    #[must_use]
    pub const fn b_spline(&self) -> Option<&'s BSplineSurface> {
        self.b_spline
    }

    /// `crGap` reduced to its component along the direction for the directed
    /// operation (`dDot * m_sDirVec`), else unchanged.
    fn directed(&self, gap: Vec3) -> Vec3 {
        if self.solver_op == SolverOperation::Signed3dDirectedMaximize {
            let dot = gap.dot(self.dir_vec);
            dot * self.dir_vec
        } else {
            gap
        }
    }
}

/// `m_pBSplSrf`: the B-spline part with a NURB, looking through one offset as upstream.
fn b_spline_part(surface: &Surface) -> Option<&BSplineSurface> {
    let base = match surface.kind() {
        SurfaceKind::OffsetSurface(offset) => offset.base_surface(),
        _ => surface,
    };
    match base.kind() {
        SurfaceKind::BSpline(b) => Some(b),
        SurfaceKind::Plane(p) => Some(p.base()),
        SurfaceKind::Cone(s) => Some(s.rev().base()),
        SurfaceKind::Sphere(s) => Some(s.base().base()),
        SurfaceKind::Torus(s) => Some(s.base().base()),
        SurfaceKind::SurfOfRevolution(s) => Some(s.base()),
        SurfaceKind::SurfOfExtrusion(s) => Some(s.base()),
        SurfaceKind::CurveBounded(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => None,
    }
    .filter(|b| b.gw_nurb().is_some())
}

// ---- control-net helpers ------------------------------------------------------------------

/// The control point `(i, j)` of a net stored row by row with `num_v` points per row,
/// divided by its weight when the net is rational.
pub(super) fn control_point(
    pts: &[CPoint],
    num_v: usize,
    i: usize,
    j: usize,
    rational: bool,
) -> Result<Point3> {
    let idx =
        i.checked_mul(num_v)
            .and_then(|x| x.checked_add(j))
            .ok_or(SolidError::invalid_input(
                OP_CPTS,
                "control point index overflows",
            ))?;
    let c = pts.get(idx).ok_or(SolidError::invalid_input(
        OP_CPTS,
        "control point index past the net",
    ))?;
    let mut p = Point3::new(c.x, c.y, c.z);
    if rational {
        p /= c.w;
    }
    Ok(p)
}

/// The closest approach of `test_pt` to the two triangles of the control-net square
/// with lower corner `(i, j)` (`GetTrianglesDist`): the distance and the local `(u, v)`
/// of the closest point in the square (`[i j] [i+1 j] [i j+1]` and
/// `[i+1 j+1] [i j+1] [i+1 j]`).
///
/// The control points are stored row by row (`v` fastest), `num_v` per row;
/// `_num_u` is not used (as upstream).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a square past the net.
pub fn triangles_dist(
    test_pt: Point3,
    cpts: &[CPoint],
    rational: bool,
    i: usize,
    j: usize,
    _num_u: usize,
    num_v: usize,
) -> Result<(f64, f64, f64)> {
    // get the four corners of this 'square'.
    let pt00 = control_point(cpts, num_v, i, j, rational)?;
    let pt10 = control_point(cpts, num_v, i + 1, j, rational)?;
    let pt01 = control_point(cpts, num_v, i, j + 1, rational)?;
    let pt11 = control_point(cpts, num_v, i + 1, j + 1, rational)?;

    let d1 = triangle_point_distance(pt00, pt10, pt01, test_pt)?;
    let d2 = triangle_point_distance(pt11, pt01, pt10, test_pt)?;

    if d1.distance <= d2.distance {
        Ok((d1.distance, d1.u, d1.v))
    } else {
        Ok((d2.distance, 1.0 - d2.u, 1.0 - d2.v))
    }
}

/// Whether the control net is well behaved around control point `(ii, jj)`, so that
/// the control-net triangle guess cannot be fooled (`WellBehavedControlNet`): no sharp
/// turn in u or v (cosine below `-0.61`), no coincident interior points, and the u and
/// v legs neither too acute nor too oblique (cosine within `±0.8`).
///
/// `_test_pt` is not used (as upstream).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a point past the net.
pub fn well_behaved_control_net(
    _test_pt: Point3,
    cpts: &[CPoint],
    rational: bool,
    ii: usize,
    jj: usize,
    num_u: usize,
    num_v: usize,
) -> Result<bool> {
    // Method: check for sharp angles.  Check u- and v-directions.
    const COSINE_LIMIT0: f64 = -0.61; // about 127.6 degrees
                                      // For the u-v angle: arbitrary guess to limit how often `return TRUE` occurs.
    const COSINE_LIMIT1: f64 = -0.80; // about 143.1 degrees

    // Get the mid point.
    let pt1 = control_point(cpts, num_v, ii, jj, rational)?;

    // First check in u-direction.
    if ii > 0 && ii < num_u.wrapping_sub(1) {
        let pt0 = control_point(cpts, num_v, ii - 1, jj, rational)?;
        let pt2 = control_point(cpts, num_v, ii + 1, jj, rational)?;

        let vec1 = pt1 - pt0;
        let vec2 = pt2 - pt1;

        let len1 = vec1.length();
        let len2 = vec2.length();

        if len1 < EFF_ZERO || len2 < EFF_ZERO {
            // Coincident control points: ok only on the low or high end of v.
            return Ok(jj == 0 || jj >= num_v.wrapping_sub(1));
        }

        let cosine = vec1.dot(vec2) / (len1 * len2);
        if cosine < COSINE_LIMIT0 {
            return Ok(false);
        }
    }

    // Repeat check in v-direction.
    if jj > 0 && jj < num_v.wrapping_sub(1) {
        let pt0 = control_point(cpts, num_v, ii, jj - 1, rational)?;
        let pt2 = control_point(cpts, num_v, ii, jj + 1, rational)?;

        let vec1 = pt1 - pt0;
        let vec2 = pt2 - pt1;

        let len1 = vec1.length();
        let len2 = vec2.length();

        if len1 < EFF_ZERO || len2 < EFF_ZERO {
            // Coincident control points: ok only on the low or high end of u.
            return Ok(ii == 0 || ii >= num_u.wrapping_sub(1));
        }

        let cosine = vec1.dot(vec2) / (len1 * len2);
        if cosine < COSINE_LIMIT0 {
            return Ok(false);
        }
    }

    // Check angle between U and V directions
    if ii < num_u.wrapping_sub(1) && jj < num_v.wrapping_sub(1) {
        let pt0 = control_point(cpts, num_v, ii + 1, jj, rational)?;
        let pt2 = control_point(cpts, num_v, ii, jj + 1, rational)?;

        let vec1 = pt0 - pt1;
        let vec2 = pt2 - pt1;

        let len1 = vec1.length();
        let len2 = vec2.length();

        if len1 < EFF_ZERO {
            // ok only on the low or high end of v.
            return Ok(jj == 0 || jj >= num_v.wrapping_sub(1));
        } else if len2 < EFF_ZERO {
            // ok only on the low or high end of u.
            return Ok(ii == 0 || ii >= num_u.wrapping_sub(1));
        }

        let cosine = vec1.dot(vec2) / (len1 * len2);
        if cosine < COSINE_LIMIT1 || cosine > -COSINE_LIMIT1 {
            return Ok(false);
        }
    }

    // No complaints.
    Ok(true)
}

// ---- SmSurfPtGlobalSolver -----------------------------------------------------------------

/// The global surface-point solver (`SmSurfPtGlobalSolver`): the problem data and the
/// lazily computed closure of the surface in u and v.
#[derive(Debug, Clone)]
pub struct SurfPtGlobalSolver<'s> {
    /// The problem data (the `SmSurfacePointSolver` base).
    pub base: SurfacePointSolver<'s>,
    /// `m_bClosedInU` (`None` = `UNSURE`).
    closed_in_u: Option<bool>,
    /// `m_bClosedInV` (`None` = `UNSURE`).
    closed_in_v: Option<bool>,
}

/// The result of [`SurfPtGlobalSolver::find_polygon_triangle_guess`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolygonTriangleGuess {
    /// The UV guess (`sUVGuess`).
    pub uv: Point2,
    /// The best control point's u index (`rlBestIdxU`).
    pub best_idx_u: usize,
    /// The best control point's v index (`rlBestIdxV`).
    pub best_idx_v: usize,
    /// The distance to the closest control-net triangle (`rdBestDist`; negated for
    /// maximise, the signed distance along the direction for the directed operation).
    pub best_dist: f64,
    /// The average distance between neighbouring control points (`rdAveCPtDist`).
    pub ave_cpt_dist: f64,
    /// The net may be far from the surface (`rbSuspicious`): rational, or badly
    /// behaved somewhere.
    pub suspicious: bool,
}

impl<'s> SurfPtGlobalSolver<'s> {
    /// A global solver (`SmSurfPtGlobalSolver(...)`); the closure is computed on
    /// first use.
    ///
    /// # Errors
    ///
    /// Those of [`SurfacePointSolver::new`].
    pub fn new(
        k: &Kernel,
        surface: &'s Surface,
        uv_domain: &Extent2d,
        solver_op: SolverOperation,
        test_point: Point3,
        distance_tolerance: f64,
        opt_target_distance: Option<f64>,
        solution_requested: SolutionRequested,
    ) -> Result<Self> {
        Ok(Self {
            base: SurfacePointSolver::new(
                k,
                surface,
                uv_domain,
                solver_op,
                test_point,
                distance_tolerance,
                opt_target_distance,
                solution_requested,
            )?,
            closed_in_u: None,
            closed_in_v: None,
        })
    }

    /// Whether the surface is closed over the domain in `which_dir` (`SM_SP_U` or
    /// `SM_SP_V`; `false` for any other value), computed once (`ClosedSurface`).
    ///
    /// # Errors
    ///
    /// Those of [`Surface::is_closed`].
    pub fn closed_surface(&mut self, k: &Kernel, which_dir: SurfParamType) -> Result<bool> {
        let b = &self.base;
        if which_dir == SurfParamType::U {
            if self.closed_in_u.is_none() {
                self.closed_in_u = Some(
                    b.surface
                        .is_closed(k, &b.domain, SurfParamType::U, None, false)?
                        .0,
                );
            }
            Ok(self.closed_in_u.unwrap_or(false))
        } else if which_dir == SurfParamType::V {
            if self.closed_in_v.is_none() {
                self.closed_in_v = Some(
                    b.surface
                        .is_closed(k, &b.domain, SurfParamType::V, None, false)?
                        .0,
                );
            }
            Ok(self.closed_in_v.unwrap_or(false))
        } else {
            Ok(false)
        }
    }

    /// The objective function at `uv` (`CalcFuncVal`): for normalise the sum of the
    /// gap's components along the unit tangents, else the distance (along the
    /// direction for the directed operation, negated for maximise);
    /// [`BIG_DOUBLE`] when the surface does not evaluate.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn calc_func_val(&self, k: &Kernel, uv: Point2) -> Result<f64> {
        let b = &self.base;
        let mut ret_val = 0.0;

        if b.solver_op == SolverOperation::Normalize {
            let (srf_pt, su, sv) = match b.surface.evaluate_1st_derivatives(k, uv, false, false) {
                Ok(r) => r,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => return Ok(BIG_DOUBLE),
            };
            if srf_pt.is_undef() {
                return Ok(BIG_DOUBLE);
            }

            let gap = srf_pt - b.test_pt;

            let su = su.unitize_or_z();
            let sv = sv.unitize_or_z();

            ret_val += gap.dot(su).abs();
            ret_val += gap.dot(sv).abs();
        } else {
            let srf_pt = match b.surface.evaluate_point(k, uv) {
                Ok(p) => p,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => return Ok(BIG_DOUBLE),
            };
            if srf_pt.is_undef() {
                return Ok(BIG_DOUBLE);
            }

            let gap = b.directed(srf_pt - b.test_pt);
            ret_val = gap.length();

            if b.solver_op == SolverOperation::Maximize {
                ret_val = -ret_val;
            }
        }

        Ok(ret_val)
    }

    /// A first guess from the control net (`FindPolygonTriangleGuess`): the UV of the
    /// closest point on any control-net triangle (the extreme control point for the
    /// directed operation), from the Greville abscissae of its square, clamped to
    /// the domain. `best_idx` is the caller's best control point, kept when no net
    /// point qualifies (upstream's in/out indices).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a B-spline surface with a NURB; those of the Greville
    /// abscissae and the triangle distances.
    pub fn find_polygon_triangle_guess(
        &self,
        best_idx: (usize, usize),
    ) -> Result<PolygonTriangleGuess> {
        let b = &self.base;
        // We need a SmBSplineSurface to work with control points.
        let Some(bspl) = b.b_spline else {
            return Err(SolidError::sm_err(OP_TRI));
        };

        let (best_idx_u, best_idx_v) = best_idx;
        let mut best_idx_u = best_idx_u;
        let mut best_idx_v = best_idx_v;
        let (num_u, num_v, cpts) = bspl
            .control_points()
            .map_err(|_| SolidError::sm_err(OP_TRI))?;
        let rational = bspl.is_rational();
        let mut best_dist = BIG_DOUBLE;
        let mut this_local_u = -1.0;
        let mut best_local_u = -1.0;
        let mut this_local_v = -1.0;
        let mut best_local_v = -1.0;

        // For collecting average distances:
        let mut accum_dist = 0.0;
        let mut dist_count: u64 = 0;

        // Rational surfaces can have polygons whose shapes are very different from
        // the surface. [B29]
        let mut suspicious = rational;

        // For operation MAXIMIZE - negate the BestDist guess
        if b.solver_op == SolverOperation::Maximize {
            best_dist = -best_dist;
        }

        for ii in 0..num_u {
            for jj in 0..num_v {
                // First, check for a badly-behaved control net. [080718, 080830]
                if !suspicious
                    && !well_behaved_control_net(b.test_pt, &cpts, rational, ii, jj, num_u, num_v)?
                {
                    suspicious = true; // Warn the caller.
                }

                // This control point.
                let pt0 = control_point(&cpts, num_v, ii, jj, rational)?;

                // In all cases, the best is the smallest signed value.
                let this_dist = if b.solver_op == SolverOperation::Signed3dDirectedMaximize {
                    this_local_u = -1.0;
                    this_local_v = -1.0;
                    pt0.dot(b.dir_vec)
                } else {
                    // The triangles of the square 'ahead' of this point: ii+1, jj+1.
                    let mut d = BIG_DOUBLE;
                    if ii < num_u - 1 && jj < num_v - 1 {
                        let (dist, lu, lv) =
                            triangles_dist(b.test_pt, &cpts, rational, ii, jj, num_u, num_v)?;
                        d = dist;
                        this_local_u = lu;
                        this_local_v = lv;

                        if b.solver_op == SolverOperation::Maximize {
                            d = -d;
                        }
                    }
                    d
                };

                // save the best found case
                if this_dist < best_dist {
                    best_dist = this_dist;
                    best_idx_u = ii;
                    best_idx_v = jj;
                    best_local_u = this_local_u;
                    best_local_v = this_local_v;
                }

                // same col control point distances
                if ii > 0 {
                    let pt1 = control_point(&cpts, num_v, ii - 1, jj, rational)?;
                    accum_dist += pt0.distance(pt1);
                    dist_count += 1;
                }

                // same row control point distances
                if jj > 0 {
                    let pt1 = control_point(&cpts, num_v, ii, jj - 1, rational)?;
                    accum_dist += pt0.distance(pt1);
                    dist_count += 1;
                }
            }
        }

        // Other outputs
        let ave_cpt_dist = accum_dist / dist_count as f64;

        // Have to check here for seams.
        let (iu, iv) =
            self.check_guess_for_seams_cpts(best_idx_u, best_idx_v, num_u, num_v, &cpts, rational)?;
        best_idx_u = iu;
        best_idx_v = iv;

        // Use the Greville abscissae for the control point indices.
        let mut uv = Point2::new(
            bspl.greville_abscissa(SurfParamType::U, best_idx_u)?,
            bspl.greville_abscissa(SurfParamType::V, best_idx_v)?,
        );

        // Refine according to the local triangle position.
        if b.solver_op != SolverOperation::Signed3dDirectedMaximize {
            if best_local_u > EFF_ZERO_SQRT && best_idx_u < num_u - 1 {
                let next_greville = bspl.greville_abscissa(SurfParamType::U, best_idx_u + 1)?;
                uv.x = uv.x + best_local_u * (next_greville - uv.x);
            }
            if best_local_v > EFF_ZERO_SQRT && best_idx_v < num_v - 1 {
                let next_greville = bspl.greville_abscissa(SurfParamType::V, best_idx_v + 1)?;
                uv.y = uv.y + best_local_v * (next_greville - uv.y);
            }
        }

        Ok(PolygonTriangleGuess {
            uv: b.domain.clamp_point(uv),
            best_idx_u,
            best_idx_v,
            best_dist,
            ave_cpt_dist,
            suspicious,
        })
    }

    /// Refines the guess of the closest control point `(best_idx_u, best_idx_v)` by
    /// dropping the test point to the four control-polygon legs around it
    /// (`RefineGuessPolygonLeg`): the Greville abscissae of the point, shifted
    /// linearly towards a neighbour by up to half a leg.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a B-spline surface with a NURB or for the directed operation;
    /// those of the Greville abscissae; [`SolidError::InvalidInput`] for indices past
    /// the net.
    pub fn refine_guess_polygon_leg(&self, best_idx_u: usize, best_idx_v: usize) -> Result<Point2> {
        let b = &self.base;
        // This requires an SmBSplineSurface.
        let Some(bspl) = b.b_spline else {
            return Err(SolidError::sm_err(OP_LEG));
        };

        // Also, this won't help in this case:
        if b.solver_op == SolverOperation::Signed3dDirectedMaximize {
            return Err(SolidError::sm_err(OP_LEG));
        }

        let (num_u, num_v, cpts) = bspl
            .control_points()
            .map_err(|_| SolidError::sm_err(OP_LEG))?;
        let rational = bspl.is_rational();

        let best_pt = control_point(&cpts, num_v, best_idx_u, best_idx_v, rational)?;

        let mut uv = Point2::new(
            bspl.greville_abscissa(SurfParamType::U, best_idx_u)?,
            bspl.greville_abscissa(SurfParamType::V, best_idx_v)?,
        );

        let shift_to = |this_pt: Point3| -> f64 {
            let shift = line_closest_point(best_pt, this_pt - best_pt, b.test_pt);
            // (NaN stays NaN, as upstream's two tests)
            shift.clamp(0.0, 0.5)
        };

        // Drop to the two legs running in the u-direction.
        let mut down_shift = 0.0;
        let mut up_shift = 0.0;
        if best_idx_u > 0 {
            down_shift = shift_to(control_point(
                &cpts,
                num_v,
                best_idx_u - 1,
                best_idx_v,
                rational,
            )?);
        }
        if best_idx_u < num_u - 1 {
            up_shift = shift_to(control_point(
                &cpts,
                num_v,
                best_idx_u + 1,
                best_idx_v,
                rational,
            )?);
        }

        // If it dropped interior to either or both, shift the parameter.
        let mut shift = up_shift - down_shift;
        if shift.abs() > EFF_ZERO_SQRT {
            let other_prm = if shift > 0.0 {
                bspl.greville_abscissa(SurfParamType::U, best_idx_u + 1)?
            } else {
                shift = -shift; // make it positive.
                bspl.greville_abscissa(SurfParamType::U, best_idx_u.wrapping_sub(1))?
            };

            // Linearly interpolate
            uv.x = (1.0 - shift) * uv.x + shift * other_prm;
        }

        // Now repeat the whole thing for the v-direction.
        down_shift = 0.0;
        up_shift = 0.0;
        if best_idx_v > 0 {
            down_shift = shift_to(control_point(
                &cpts,
                num_v,
                best_idx_u,
                best_idx_v - 1,
                rational,
            )?);
        }
        if best_idx_v < num_v - 1 {
            up_shift = shift_to(control_point(
                &cpts,
                num_v,
                best_idx_u,
                best_idx_v + 1,
                rational,
            )?);
        }

        shift = up_shift - down_shift;
        if shift.abs() > EFF_ZERO_SQRT {
            let other_prm = if shift > 0.0 {
                bspl.greville_abscissa(SurfParamType::V, best_idx_v + 1)?
            } else {
                shift = -shift; // make it positive.
                bspl.greville_abscissa(SurfParamType::V, best_idx_v.wrapping_sub(1))?
            };

            // Linearly interpolate
            uv.y = (1.0 - shift) * uv.y + shift * other_prm;
        }

        Ok(uv)
    }

    /// Whether a grid point may be on the wrong side of a surface that doubles back
    /// (`CheckBadGuessStride`): takes a plane step from `grid_pt0` towards the test
    /// point (clamped to the domain) and, when that step is longer than 10% of the
    /// domain, a second one if it improves the objective. Returns `None` when the
    /// first plane step cannot be computed (the check is skipped), else the new grid
    /// point and its objective value.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluations.
    pub fn check_bad_guess_stride(
        &self,
        k: &'s Kernel,
        grid_pt0: &mut SurfaceEval<'s>,
    ) -> Result<Option<(SurfaceEval<'s>, f64)>> {
        let b = &self.base;
        let pos0 = eval_value(grid_pt0, 0, 0)?;
        let su0 = eval_value(grid_pt0, 1, 0)?;
        let sv0 = eval_value(grid_pt0, 0, 1)?;
        let gap = b.test_pt - pos0;
        let Ok(uv_step) = vec_lin_comb_two_vectors(su0, sv0, gap) else {
            return Ok(None); // Just forget this check.
        };

        let mut new_grid_pt = SurfaceEval::new(k, Some(b.surface));
        let mut new_uv0 = grid_pt0.uv() + uv_step;
        new_uv0 = b.domain.clamp_point(new_uv0); // ok if outside 'subdomain'.
        new_grid_pt.set_uv(new_uv0);
        let mut new_func_val = self.calc_func_val(k, new_uv0)?;

        // If the step was big, take another one.  [B51]
        let max_step_sq = 0.10 * 0.10; // 10%, squared.

        if uv_step.length_squared() > max_step_sq * b.domain.size().length_squared() {
            // watch out for failing Evaluations
            if fails_eval(&mut new_grid_pt)? {
                return Ok(Some((new_grid_pt, new_func_val))); // Just use the first result.
            }

            let gap = b.test_pt - eval_value(&mut new_grid_pt, 0, 0)?;
            let su = eval_value(&mut new_grid_pt, 1, 0)?;
            let sv = eval_value(&mut new_grid_pt, 0, 1)?;
            let Ok(uv_step) = vec_lin_comb_two_vectors(su, sv, gap) else {
                return Ok(Some((new_grid_pt, new_func_val))); // Just use the first result.
            };

            let mut uv = new_grid_pt.uv() + uv_step;
            uv = b.domain.clamp_point(uv); // ok if outside 'subdomain'.
            let temp_func_val = self.calc_func_val(k, uv)?;
            if temp_func_val < new_func_val {
                new_grid_pt.set_uv(uv);
                new_func_val = temp_func_val;
            }
        }

        Ok(Some((new_grid_pt, new_func_val)))
    }

    /// On a seam, decides which end of the domain the UV guess is better at and may
    /// flip it across (`CheckGuessForSeams`, UV point version).
    ///
    /// # Errors
    ///
    /// Those of the seam and closure tests and of [`Surface::is_on_seam`].
    pub fn check_guess_for_seams(&mut self, k: &Kernel, uv_guess: Point2) -> Result<Point2> {
        let mut uv = uv_guess;
        if !is_on_seam(k, self.base.surface, uv)? {
            return Ok(uv);
        }

        let mut pt = Point3::UNDEF;
        let mut su = Vec3::UNDEF;
        let mut sv = Vec3::UNDEF;

        // Check u-direction.
        let dom = self.base.domain;
        if (uv.x < dom.min().x + EFF_ZERO || uv.x > dom.max().x - EFF_ZERO)
            && self.closed_surface(k, SurfParamType::U)?
        {
            // Check whether low or high bound is better (status ignored upstream).
            eval_1st(k, self.base.surface, uv, &mut pt, &mut su, &mut sv)?;
            let gap = self.base.directed(pt - self.base.test_pt);
            uv.x = if gap.dot(su) > 0.0 {
                dom.max().x
            } else {
                dom.min().x
            };
        }

        // Check v-direction.
        if (uv.y < dom.min().y + EFF_ZERO || uv.y > dom.max().y - EFF_ZERO)
            && self.closed_surface(k, SurfParamType::V)?
        {
            eval_1st(k, self.base.surface, uv, &mut pt, &mut su, &mut sv)?;
            let gap = self.base.directed(pt - self.base.test_pt);
            uv.y = if gap.dot(sv) > 0.0 {
                dom.max().y
            } else {
                dom.min().y
            };
        }
        Ok(uv)
    }

    /// On a seam of the control net, decides which end the best control point
    /// `(idx_u, idx_v)` is better at and may flip it across (`CheckGuessForSeams`,
    /// control point version): a direction with more than two points is a seam when
    /// its first and last points of the row coincide.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for indices past the net.
    pub fn check_guess_for_seams_cpts(
        &self,
        idx_u: usize,
        idx_v: usize,
        num_u: usize,
        num_v: usize,
        cpts: &[CPoint],
        rational: bool,
    ) -> Result<(usize, usize)> {
        let mut idx_u = idx_u;
        let mut idx_v = idx_v;
        if idx_u > 0 && idx_u < num_u.wrapping_sub(1) && idx_v > 0 && idx_v < num_v.wrapping_sub(1)
        {
            return Ok((idx_u, idx_v)); // not on boundary.
        }
        let b = &self.base;
        let shift = |seam_pt: Point3, other_pt: Point3| -> f64 {
            if b.solver_op == SolverOperation::Signed3dDirectedMaximize {
                -b.dir_vec.dot(other_pt - seam_pt)
            } else {
                line_closest_point(seam_pt, other_pt - seam_pt, b.test_pt)
            }
        };

        // Check u-direction.
        if num_u > 2 && (idx_u == 0 || idx_u == num_u - 1) {
            // Check whether it's a seam.
            let seam_pt = control_point(cpts, num_v, 0, idx_v, rational)?;
            let other_pt = control_point(cpts, num_v, num_u - 1, idx_v, rational)?;

            if seam_pt.closer_than(EFF_ZERO, other_pt) {
                let down_shift = shift(
                    seam_pt,
                    control_point(cpts, num_v, num_u - 2, idx_v, rational)?,
                );
                let up_shift = shift(seam_pt, control_point(cpts, num_v, 1, idx_v, rational)?);

                // Lean towards the one that's positive.
                let diff = up_shift - down_shift;
                idx_u = if diff >= 0.0 { 0 } else { num_u - 1 };
            }
        }

        // Repeat for v-direction.
        if num_v > 2 && (idx_v == 0 || idx_v == num_v - 1) {
            // Check whether it's a seam.
            let seam_pt = control_point(cpts, num_v, idx_u, 0, rational)?;
            let other_pt = control_point(cpts, num_v, idx_u, num_v - 1, rational)?;

            if seam_pt.closer_than(EFF_ZERO, other_pt) {
                let down_shift = shift(
                    seam_pt,
                    control_point(cpts, num_v, idx_u, num_v - 2, rational)?,
                );
                let up_shift = shift(seam_pt, control_point(cpts, num_v, idx_u, 1, rational)?);

                let diff = up_shift - down_shift;
                idx_v = if diff >= 0.0 { 0 } else { num_v - 1 };
            }
        }

        Ok((idx_u, idx_v))
    }

    /// The domain broken up along creases (`GetCreaseSubdomains`). Upstream switches
    /// the splitting off (`sbDoCreaseSubdomains = FALSE`): the list is the whole
    /// domain.
    ///
    /// # Errors
    ///
    /// Those of the knots (only with the splitting switched on).
    pub fn crease_subdomains(&self) -> Result<Vec<Extent2d>> {
        // Currently not used.
        const DO_CREASE_SUBDOMAINS: bool = false;

        let b = &self.base;
        let mut list = vec![b.domain];
        if !DO_CREASE_SUBDOMAINS {
            return Ok(list);
        }

        // This requires a B-Spline surface.
        let Some(bspl) = b.b_spline else {
            return Ok(list);
        };

        for dir in [SurfParamType::U, SurfParamType::V] {
            let domain = if dir == SurfParamType::U {
                b.domain.u_interval()
            } else {
                b.domain.v_interval()
            };
            let tol = -(domain.length() / 1000.0); // on-boundary means Outside.
            let (knots, mults) = bspl.knots(dir, None)?;
            let deg = bspl.degree(dir);
            // For each interior knot:
            let num_knots = mults.len();
            for ii in 1..num_knots.saturating_sub(1) {
                let (Some(&knot), Some(&mult)) = (knots.get(ii), mults.get(ii)) else {
                    continue;
                };
                if mult < deg || !domain.contains_value(knot, tol) {
                    continue;
                }
                // Split every subdomain that contains the knot into two pieces.
                let list_size = list.len();
                for jj in 0..list_size {
                    let Some(sub) = list.get(jj).copied() else {
                        continue;
                    };
                    let mut ivl = if dir == SurfParamType::U {
                        sub.u_interval()
                    } else {
                        sub.v_interval()
                    };
                    if !ivl.contains_value(knot, tol) {
                        continue;
                    }
                    let high_end = ivl.max();
                    let mut low = sub;
                    let _ = ivl.set_min_max(ivl.min(), knot);
                    set_interval(&mut low, dir, &ivl);
                    list.push(low);
                    let mut high = low;
                    let _ = ivl.set_min_max(knot, high_end);
                    set_interval(&mut high, dir, &ivl);
                    if let Some(slot) = list.get_mut(jj) {
                        *slot = high;
                    }
                }
            }
        }
        Ok(list)
    }

    /// Whether the evaluated point is a local minimum of the distance to the test
    /// point (`IsLocalMin`): on the surface within `tol`, or on the surface normal
    /// and within the radius of curvature in some direction not along a boundary it
    /// sits on.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn is_local_min(&self, surf_eval: &mut SurfaceEval<'_>, tol: f64) -> Result<bool> {
        self.is_local_extremum(surf_eval, tol, true)
    }

    /// Whether the evaluated point is a local maximum of the distance to the test
    /// point (`IsLocalMax`): off the surface, on its normal and beyond the centre of
    /// curvature in some direction not along a boundary it sits on.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn is_local_max(&self, surf_eval: &mut SurfaceEval<'_>, tol: f64) -> Result<bool> {
        self.is_local_extremum(surf_eval, tol, false)
    }

    /// The common body of `IsLocalMin` (`on_surface_result = TRUE`) and `IsLocalMax`
    /// (`FALSE`): they differ only in the result for a point on the surface.
    fn is_local_extremum(
        &self,
        surf_eval: &mut SurfaceEval<'_>,
        tol: f64,
        on_surface_result: bool,
    ) -> Result<bool> {
        let b = &self.base;
        let gap = b.directed(eval_value(surf_eval, 0, 0)? - b.test_pt);
        let dist = gap.length();
        if dist <= tol {
            return Ok(on_surface_result);
        }

        // We are not right on the surface.
        let su = eval_value(surf_eval, 1, 0)?;
        let sv = eval_value(surf_eval, 0, 1)?;

        if gap.dot(su).abs() > tol {
            return Ok(false);
        }
        if gap.dot(sv).abs() > tol {
            return Ok(false);
        }

        // We are off the surface and on the surface normal: see upstream's notes on
        // the Newton and plane step denominators.
        let noise_lim = EFF_ZERO * 1000.0;

        let suu = eval_value(surf_eval, 2, 0)?;
        let svv = eval_value(surf_eval, 0, 2)?;

        let gap_u = b.directed(su);
        let gap_v = b.directed(sv);

        // If on a boundary, don't check in the direction of the boundary
        // that we're on; like being on the end of a curve [B243].
        let bdry_flags = b.domain.point_boundaries(surf_eval.uv(), EFF_ZERO);

        if !((bdry_flags & SS_UMIN) != 0 || (bdry_flags & SS_UMAX) != 0) {
            let f1 = gap_u.dot(su);
            let f2 = gap.dot(suu);

            // If flat, don't return positive: make sure f1 is not zero.
            if f1.abs() > noise_lim && f1 + f2 < -noise_lim {
                return Ok(true);
            }
        }

        if !((bdry_flags & SS_VMIN) != 0 || (bdry_flags & SS_VMAX) != 0) {
            let f1 = gap_v.dot(sv);
            let f2 = gap.dot(svv);

            if f1.abs() > noise_lim && f1 + f2 < -noise_lim {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Whether the evaluated point is on a domain boundary (within `|tol|`) and the
    /// plane step towards the test point leaves the domain there by more than `tol`
    /// (`PushingBoundary`), and which boundary: [`SurfParamType::UMin`],
    /// [`SurfParamType::UMax`], [`SurfParamType::VMin`], [`SurfParamType::VMax`],
    /// [`SurfParamType::Both`] (a corner) or [`SurfParamType::Neither`].
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn pushing_boundary(
        &self,
        surf_eval: &mut SurfaceEval<'_>,
        tol: f64,
    ) -> Result<(bool, SurfParamType)> {
        let b = &self.base;
        if !b.domain.is_point_on_boundary(surf_eval.uv(), tol.abs()).0 {
            return Ok((false, SurfParamType::Neither));
        }

        let mut gap = b.test_pt - eval_value(surf_eval, 0, 0)?;
        if b.solver_op == SolverOperation::Signed3dDirectedMaximize {
            let dot = -(gap.dot(b.dir_vec));
            gap = dot * b.dir_vec;
        }

        // Method: find the uv direction that the 3d step will take us, and see
        // whether it's heading out of bounds.
        let uv = surf_eval.uv();
        let su = eval_value(surf_eval, 1, 0)?;
        let sv = eval_value(surf_eval, 0, 1)?;

        let Ok(uv_step) = vec_lin_comb_two_vectors(su, sv, gap) else {
            return Ok((false, SurfParamType::Neither));
        };

        let mut ret_val = SurfParamType::Neither;

        let bdry_tol = EFF_ZERO;
        let pushing_tol = tol;

        if uv.x < b.domain.u_interval().min() + bdry_tol && uv_step.x < -pushing_tol {
            ret_val = SurfParamType::UMin;
        } else if uv.x > b.domain.u_interval().max() - bdry_tol && uv_step.x > pushing_tol {
            ret_val = SurfParamType::UMax;
        }

        if uv.y < b.domain.v_interval().min() + bdry_tol && uv_step.y < -pushing_tol {
            ret_val = if ret_val == SurfParamType::Neither {
                SurfParamType::VMin
            } else {
                SurfParamType::Both
            };
        } else if uv.y > b.domain.v_interval().max() - bdry_tol && uv_step.y > pushing_tol {
            ret_val = if ret_val == SurfParamType::Neither {
                SurfParamType::VMax
            } else {
                SurfParamType::Both
            };
        }

        Ok((ret_val != SurfParamType::Neither, ret_val))
    }
}

/// `sDomainList[jj].SetUInterval` / `SetVInterval`.
fn set_interval(d: &mut Extent2d, dir: SurfParamType, ivl: &Extent1d) {
    if dir == SurfParamType::U {
        d.set_u_interval(ivl);
    } else {
        d.set_v_interval(ivl);
    }
}

/// `pSurf->IsOnSeam(crUV)` (virtual, default arguments): a plane has no seam
/// (`SmPlane::IsOnSeam` override); the other kinds run the base body.
fn is_on_seam(k: &Kernel, surface: &Surface, uv: Point2) -> Result<bool> {
    match surface.kind() {
        SurfaceKind::Plane(p) => Ok(p.is_on_seam().0),

        SurfaceKind::Cone(_)
        | SurfaceKind::Sphere(_)
        | SurfaceKind::Torus(_)
        | SurfaceKind::BSpline(_)
        | SurfaceKind::CurveBounded(_)
        | SurfaceKind::SurfOfRevolution(_)
        | SurfaceKind::SurfOfExtrusion(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => Ok(surface.is_on_seam(k, uv, None)?.on_seam),
    }
}

/// `Evaluate1stDerivatives(uv, FALSE, FALSE, ...)` with its status ignored: the
/// outputs keep their previous values on failure.
fn eval_1st(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    pt: &mut Point3,
    su: &mut Vec3,
    sv: &mut Vec3,
) -> Result<()> {
    match surface.evaluate_1st_derivatives(k, uv, false, false) {
        Ok((p, u, v)) => {
            *pt = p;
            *su = u;
            *sv = v;
            Ok(())
        }
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(()),
    }
}

/// `SmSurfaceEval` accessor `m_sPVV[i][j]` after `Eval()` with its status ignored: the
/// values held when the evaluation fails.
fn eval_value(ev: &mut SurfaceEval<'_>, i: usize, j: usize) -> Result<Vec3> {
    if let Err(e) = ev.eval() {
        if e.is_unported() {
            return Err(e);
        }
    }
    Ok(ev
        .values()
        .get(i)
        .and_then(|r| r.get(j))
        .copied()
        .unwrap_or(Vec3::UNDEF))
}

/// `SmSurfaceEval::FailsEval(FALSE)`: evaluates and reports a failure.
fn fails_eval(ev: &mut SurfaceEval<'_>) -> Result<bool> {
    match ev.eval() {
        Ok(()) => Ok(false),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(true),
    }
}

// ---- SmSurfPtSolveStepper -----------------------------------------------------------------

/// The error norms of a stepper evaluation (`SmSrfPtErrorNormType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ErrorNormType {
    /// `SM_SPN_SUM_ABS`: `|F| + |G|`, the L1 norm (the usual one).
    #[default]
    SumAbs = 0,
    /// `SM_SPN_SUM`: `F + G`.
    Sum = 1,
    /// `SM_SPN_F`.
    F = 2,
    /// `SM_SPN_G`.
    G = 3,
    /// `SM_SPN_F_ABS`.
    FAbs = 4,
    /// `SM_SPN_G_ABS`.
    GAbs = 5,
    /// `SM_SPN_MAX`: `F` when `|F| >= |G|`, else `G`.
    Max = 6,
    /// `SM_SPN_MAX_ABS`: the L-infinity norm.
    MaxAbs = 7,
    /// `SM_SPN_GAP`: the 3D distance.
    Gap = 8,
}

/// The numerical values of a stepper at its UV point, valid once evaluated
/// (`m_sGap` ... `m_dSumCheck`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepperValues {
    /// `m_sGap`: `S(u, v) - TestPt` (along the direction for the directed operation).
    pub gap: Vec3,
    /// `m_sGapU`: `Su` (along the direction for the directed operation).
    pub gap_u: Vec3,
    /// `m_sGapV`: `Sv` (along the direction for the directed operation).
    pub gap_v: Vec3,
    /// `m_dGapLen`.
    pub gap_len: f64,
    /// `m_dFVal`: `Gap . Su`.
    pub f_val: f64,
    /// `m_dGVal`: `Gap . Sv`.
    pub g_val: f64,
    /// `m_dFCheck`: `F / |Su|` (`|Su|` at least `0.0001`).
    pub f_check: f64,
    /// `m_dGCheck`: `G / |Sv|` (`|Sv|` at least `0.0001`).
    pub g_check: f64,
    /// `m_dSumCheck`: `|FCheck| + |GCheck|`.
    pub sum_check: f64,
}

impl StepperValues {
    /// The values of a copy-constructed stepper before evaluation (upstream leaves
    /// them uninitialised).
    const UNSET: Self = Self {
        gap: Vec3::UNDEF,
        gap_u: Vec3::UNDEF,
        gap_v: Vec3::UNDEF,
        gap_len: UNDEF_DOUBLE,
        f_val: UNDEF_DOUBLE,
        g_val: UNDEF_DOUBLE,
        f_check: UNDEF_DOUBLE,
        g_check: UNDEF_DOUBLE,
        sum_check: UNDEF_DOUBLE,
    };
}

/// One iteration guess of the local solver (`SmSurfPtSolveStepper`): the problem data,
/// the surface evaluation at the guess, iteration and boundary-hit counters, the step
/// limit, and the lazily computed gap values.
#[derive(Debug)]
pub struct SurfPtSolveStepper<'s> {
    /// The problem data (the `SmSurfacePointSolver` base).
    pub base: SurfacePointSolver<'s>,
    /// `m_sSrfEval`.
    srf_eval: SurfaceEval<'s>,
    /// `m_bSet`.
    set: bool,
    /// `m_iIter`.
    iter: i32,
    /// `m_iBndHitU`.
    bnd_hit_u: i32,
    /// `m_iBndHitV`.
    bnd_hit_v: i32,
    /// `m_dMaxStepFrac`.
    max_step_frac: f64,
    /// `m_bNRSuspect`.
    nr_suspect: bool,
    /// `m_sGap` ... `m_dSumCheck`.
    values: StepperValues,
}

/// The result of [`SurfPtSolveStepper::check_bounds`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundsCheck {
    /// The next UV is outside the domain (the return value).
    pub out_of_bounds: bool,
    /// `-1` past the low u bound, `+1` past the high one, `0` neither (`riHitsU`).
    pub hits_u: i32,
    /// The same for v (`riHitsV`).
    pub hits_v: i32,
    /// The next UV clamped to the domain boundary (`rClampedUV`; `None` when not
    /// clamped: upstream leaves it unset).
    pub clamped_uv: Option<Point2>,
    /// The point where the step from the current UV leaves the domain
    /// (`rClampedUVScaled`; `None` when not clamped).
    pub clamped_uv_scaled: Option<Point2>,
    /// The ratio of the clamped step to the whole step (`rdStepScale`).
    pub step_scale: f64,
}

impl<'s> SurfPtSolveStepper<'s> {
    /// A stepper at `uv` (`SmSurfPtSolveStepper(pSurf, crDomain, crTestPt, rUV,
    /// dTightTol, dLooseTol, eSolverOp)`): the steps are limited to 40% of the domain,
    /// or not at all for a bilinear B-spline (2 x 2 control points).
    ///
    /// # Errors
    ///
    /// Those of [`SurfacePointSolver::new`].
    pub fn new(
        k: &'s Kernel,
        surface: &'s Surface,
        domain: &Extent2d,
        test_pt: Point3,
        uv: Point2,
        tight_tol: f64,
        loose_tol: f64,
        solver_op: SolverOperation,
    ) -> Result<Self> {
        let mut base = SurfacePointSolver::new(
            k,
            surface,
            domain,
            solver_op,
            test_pt,
            loose_tol,
            None,
            SolutionRequested::Single,
        )?;
        base.tight_tol = tight_tol;

        // Maximum single step: say 40% of domain...
        let mut max_step_frac = 0.40;

        // ...unless the surface is bilinear: allow anything.
        if let Some(bspl) = base.b_spline {
            if bspl.number_control_points(SurfParamType::U) == 2
                && bspl.number_control_points(SurfParamType::V) == 2
            {
                max_step_frac = 2.00;
            }
        }

        let mut srf_eval = SurfaceEval::new(k, Some(surface));
        srf_eval.set_uv(uv);
        Ok(Self {
            base,
            srf_eval,
            set: false,
            iter: 0,
            bnd_hit_u: 0,
            bnd_hit_v: 0,
            max_step_frac,
            nr_suspect: false,
            values: StepperValues::UNSET,
        })
    }

    /// The copy constructor: the problem data, counters and step limit of `other` at
    /// its UV, not evaluated (the copy is never expected to stay at the same UV).
    #[must_use]
    pub fn copy_of(_k: &Kernel, other: &Self) -> Self {
        let mut srf_eval = SurfaceEval::new(other.srf_eval.kernel, Some(other.base.surface));
        srf_eval.set_uv(other.uv());
        Self {
            base: other.base.clone(),
            srf_eval,
            set: false, // see note above.
            iter: other.iter,
            bnd_hit_u: other.bnd_hit_u,
            bnd_hit_v: other.bnd_hit_v,
            max_step_frac: other.max_step_frac,
            nr_suspect: other.nr_suspect,
            values: StepperValues::UNSET,
        }
    }

    /// The assignment operator: everything of `other`, including its evaluation when
    /// it has one (`operator=`).
    pub fn assign(&mut self, other: &Self) {
        self.base.clone_from(&other.base);

        self.srf_eval.set_uv(other.uv());
        self.set = other.set;
        self.iter = other.iter;
        self.bnd_hit_u = other.bnd_hit_u;
        self.bnd_hit_v = other.bnd_hit_v;
        self.max_step_frac = other.max_step_frac;
        self.nr_suspect = other.nr_suspect;

        if self.set {
            self.srf_eval.clone_from(&other.srf_eval);
            self.values = other.values;
        }
    }

    /// Moves to `uv` and drops the evaluated values (`SetUV`).
    pub fn set_uv(&mut self, uv: Point2) {
        self.srf_eval.set_uv(uv);
        self.set = false;
    }

    /// The UV point (`UV`).
    #[must_use]
    pub const fn uv(&self) -> Point2 {
        self.srf_eval.uv()
    }

    /// The largest step as a fraction of the domain (`MaxStep`).
    #[must_use]
    pub const fn max_step(&self) -> f64 {
        self.max_step_frac
    }

    /// The surface point (`Pos`).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn pos(&mut self) -> Result<Point3> {
        eval_value(&mut self.srf_eval, 0, 0)
    }

    /// `Su`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn su(&mut self) -> Result<Vec3> {
        eval_value(&mut self.srf_eval, 1, 0)
    }

    /// `Sv`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn sv(&mut self) -> Result<Vec3> {
        eval_value(&mut self.srf_eval, 0, 1)
    }

    /// `Suu`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn suu(&mut self) -> Result<Vec3> {
        eval_value(&mut self.srf_eval, 2, 0)
    }

    /// `Suv`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn suv(&mut self) -> Result<Vec3> {
        eval_value(&mut self.srf_eval, 1, 1)
    }

    /// `Svv`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn svv(&mut self) -> Result<Vec3> {
        eval_value(&mut self.srf_eval, 0, 2)
    }

    /// The unit normal (`Normal`).
    ///
    /// # Errors
    ///
    /// Missing dependencies from [`SurfaceEval::normal`]. Ordinary evaluation
    /// statuses are ignored, retaining the cached value as the C++ accessor does.
    pub fn normal(&mut self, k: &Kernel) -> Result<Vec3> {
        match self.srf_eval.normal(k) {
            Ok(normal) => Ok(normal),
            Err(error) if error.is_unported() => Err(error),
            Err(_) => Ok(self
                .srf_eval
                .values()
                .get(2)
                .and_then(|row| row.get(2))
                .copied()
                .unwrap_or(Vec3::UNDEF)),
        }
    }

    /// The gap `S(u, v) - TestPt` (`Gap`).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn gap(&mut self) -> Result<Vec3> {
        Ok(self.eval()?.gap)
    }

    /// `GapU`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn gap_u(&mut self) -> Result<Vec3> {
        Ok(self.eval()?.gap_u)
    }

    /// `GapV`.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn gap_v(&mut self) -> Result<Vec3> {
        Ok(self.eval()?.gap_v)
    }

    /// The gap length (`GapLength`).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn gap_length(&mut self) -> Result<f64> {
        Ok(self.eval()?.gap_len)
    }

    /// The iteration count (`IterCount`).
    #[must_use]
    pub const fn iter_count(&self) -> i32 {
        self.iter
    }

    /// Whether the Newton step is not to be trusted (`NRStepSuspect`).
    #[must_use]
    pub const fn nr_step_suspect(&self) -> bool {
        self.nr_suspect
    }

    /// Sets the iteration count (`SetIterCount`).
    pub fn set_iter_count(&mut self, new_count: i32) {
        self.iter = new_count;
    }

    /// Sets whether the Newton step is not to be trusted (`SetNRSuspect`).
    pub fn set_nr_suspect(&mut self, is_suspect: bool) {
        self.nr_suspect = is_suspect;
    }

    /// The u boundary-hit count (`BoundHitsU`).
    #[must_use]
    pub const fn bound_hits_u(&self) -> i32 {
        self.bnd_hit_u
    }

    /// The v boundary-hit count (`BoundHitsV`).
    #[must_use]
    pub const fn bound_hits_v(&self) -> i32 {
        self.bnd_hit_v
    }

    /// Whether the gap values are evaluated for the current UV point (`m_bSet`).
    #[must_use]
    pub const fn is_set(&self) -> bool {
        self.set
    }

    /// The evaluation (surface) at the UV point.
    #[must_use]
    pub const fn surface_eval(&self) -> &SurfaceEval<'s> {
        &self.srf_eval
    }

    /// Evaluates the gap values unless done already (`Eval`, private upstream).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn eval(&mut self) -> Result<StepperValues> {
        if self.set {
            return Ok(self.values);
        }

        let pos = self.pos()?;
        let su = self.su()?;
        let sv = self.sv()?;
        let b = &self.base;
        let mut gap = pos - b.test_pt;
        let mut gap_u = su;
        let mut gap_v = sv;
        if b.solver_op == SolverOperation::Signed3dDirectedMaximize {
            gap = b.dir_vec * gap.dot(b.dir_vec);
            gap_u = b.dir_vec * gap_u.dot(b.dir_vec);
            gap_v = b.dir_vec * gap_v.dot(b.dir_vec);
        }

        let gap_len = gap.length();

        let f_val = gap.dot(su);
        let g_val = gap.dot(sv);

        // Convert fcn vals to cosines for convergence test.
        let mut su_len = su.length();
        let mut sv_len = sv.length();
        // Check zero-divide: near a singularity, don't make convergence impossible.
        let min_deriv_length = 0.0001;
        if su_len < min_deriv_length {
            su_len = min_deriv_length;
        }
        if sv_len < min_deriv_length {
            sv_len = min_deriv_length;
        }

        // Note: do not divide by the distance (see upstream).
        let dist = 1.0;

        let f_check = f_val / (dist * su_len);
        let g_check = g_val / (dist * sv_len);
        let sum_check = f_check.abs() + g_check.abs();

        self.values = StepperValues {
            gap,
            gap_u,
            gap_v,
            gap_len,
            f_val,
            g_val,
            f_check,
            g_check,
            sum_check,
        };
        self.set = true;

        Ok(self.values)
    }

    /// Clamps the UV point to the domain (`Clamp`).
    pub fn clamp(&mut self) {
        let uv = self.base.domain.clamp_point(self.srf_eval.uv());
        self.set_uv(uv);
    }

    /// Records a boundary hit in `which` (`SM_SP_U`, else v) (`HitBound`):
    /// `top_or_bottom` `+1` / `-1` pushes outward at the high / low end (counting up
    /// while pushing the same way), `0` resets the count when the point is strictly
    /// inside the domain in that direction.
    pub fn hit_bound(&mut self, which: SurfParamType, top_or_bottom: i32) {
        if top_or_bottom == 0 {
            if which == SurfParamType::U {
                if self.bnd_hit_u != 0
                    && self
                        .base
                        .domain
                        .u_interval()
                        .contains_value(self.srf_eval.uv().x, -EFF_ZERO)
                {
                    self.bnd_hit_u = 0;
                }
            } else if self.bnd_hit_v != 0
                && self
                    .base
                    .domain
                    .v_interval()
                    .contains_value(self.srf_eval.uv().y, -EFF_ZERO)
            {
                self.bnd_hit_v = 0;
            }
        } else if which == SurfParamType::U {
            self.bnd_hit_u = if self.bnd_hit_u.wrapping_mul(top_or_bottom) > 0 {
                self.bnd_hit_u.wrapping_add(top_or_bottom) // pushing same direction
            } else {
                top_or_bottom // no previous bound, or pushing opposite
            };
        } else {
            self.bnd_hit_v = if self.bnd_hit_v.wrapping_mul(top_or_bottom) > 0 {
                self.bnd_hit_v.wrapping_add(top_or_bottom)
            } else {
                top_or_bottom
            };
        }
    }

    /// Whether `next_uv` is outside `domain` (`CheckBounds`), with the boundaries hit,
    /// the next UV clamped to the boundary, and the step from `curr_uv` scaled to end
    /// where it leaves the domain.
    #[must_use]
    pub fn check_bounds(curr_uv: Point2, next_uv: Point2, domain: &Extent2d) -> BoundsCheck {
        let mut hits_u = 0;
        let mut hits_v = 0;

        let mut step_scale = 1.0;

        let u_min = domain.min().x;
        let v_min = domain.min().y;
        let u_max = domain.max().x;
        let v_max = domain.max().y;

        if next_uv.x < u_min {
            hits_u = -1;
            step_scale = if curr_uv.x >= u_min {
                (u_min - curr_uv.x) / (next_uv.x - curr_uv.x)
            } else {
                0.0
            };
        } else if next_uv.x > u_max {
            hits_u = 1;
            step_scale = if curr_uv.x <= u_max {
                (u_max - curr_uv.x) / (next_uv.x - curr_uv.x)
            } else {
                0.0
            };
        }

        if next_uv.y < v_min {
            hits_v = -1;
            let this_scale = if curr_uv.y >= v_min {
                (v_min - curr_uv.y) / (next_uv.y - curr_uv.y)
            } else {
                0.0
            };
            if this_scale < step_scale {
                step_scale = this_scale;
            }
        } else if next_uv.y > v_max {
            hits_v = 1;
            let this_scale = if curr_uv.y <= v_max {
                (v_max - curr_uv.y) / (next_uv.y - curr_uv.y)
            } else {
                0.0
            };
            if this_scale < step_scale {
                step_scale = this_scale;
            }
        }

        let mut out = BoundsCheck {
            out_of_bounds: false,
            hits_u,
            hits_v,
            clamped_uv: None,
            clamped_uv_scaled: None,
            step_scale,
        };

        if hits_u != 0 || hits_v != 0 {
            out.out_of_bounds = true;
            out.clamped_uv_scaled = Some(curr_uv + step_scale * (next_uv - curr_uv));

            let mut clamped = Vec2::new(next_uv.x, next_uv.y);
            if hits_u < 0 {
                clamped = Vec2::new(u_min, clamped.y);
            } else if hits_u > 0 {
                clamped = Vec2::new(u_max, clamped.y);
            }
            if hits_v < 0 {
                clamped = Vec2::new(clamped.x, v_min);
            } else if hits_v > 0 {
                clamped = Vec2::new(clamped.x, v_max);
            }
            out.clamped_uv = Some(clamped);
        }

        out
    }

    /// Drops the test point to the iso curve `which_param = crease_param_val`
    /// (`DropToIsoCurve`, not used upstream): over each G1-or-worse section of the
    /// iso curve within the domain, keeps the closest drop and moves there. Returns
    /// whether a drop was found.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`]: the base `SmSurface::CreateIsoParametricCurve`
    /// (`surface.surface_p5`) of a curve-bounded or STEP surface, and from the drops.
    pub fn drop_to_iso_curve(
        &mut self,
        k: &Kernel,
        which_param: SurfParamType,
        crease_param_val: f64,
        _param_on_crease: f64,
    ) -> Result<bool> {
        let b = &self.base;
        // The iso curve (no curve: FALSE): the surface's CreateIsoParametricCurve, with
        // the loose tolerance as the approximation tolerance of the base body.
        let iso_curve: Curve = match b.surface.create_iso_parametric_curve(
            k,
            which_param,
            crease_param_val,
            b.loose_tol,
            None,
            false,
        ) {
            Ok(r) => r.curve,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Ok(false),
        };

        let domain = if which_param == SurfParamType::U {
            b.domain.v_interval()
        } else {
            b.domain.u_interval()
        };

        // We have to check the iso-curve itself for having kinks (status ignored).
        let conts = match iso_curve.calculate_continuities(k, CONTINUITY_ANGLE) {
            Ok((_, c)) => c,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => Vec::new(),
        };

        let mut got_iso_drop = false;
        let mut min_dist = BIG_DOUBLE;
        let mut min_param = domain.min();

        // Drop the point to each continuous section.
        let knots = match iso_curve.knots(k, None) {
            Ok((k, _)) => k,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => Vec::new(),
        };
        let first_knot = *knots.first().ok_or(SolidError::invalid_input(
            "surface::surface_point_solve::p1::drop_to_iso_curve",
            "iso curve without knots",
        ))?;
        let mut ivl = Extent1d::from_min_max(first_knot, first_knot);
        for ii in 1..conts.len() {
            let Some(&cont) = conts.get(ii) else { continue };
            if (cont as i32) > (ContinuityType::G1 as i32) {
                continue;
            }
            let knot = knots.get(ii).copied().ok_or(SolidError::invalid_input(
                "surface::surface_point_solve::p1::drop_to_iso_curve",
                "fewer knots than continuities",
            ))?;
            let _ = ivl.set_min_max(ivl.min(), knot);

            // in case surface domain is smaller:
            ivl = ivl.intersect(&domain).unwrap_or_else(|_| Extent1d::new());

            // The guess and the output parameter are the same variable upstream:
            // DropPoint zeroes the output first, so the guess is 0.
            let dropped = match iso_curve.drop_point(
                k,
                &ivl,
                b.test_pt,
                None,
                b.tight_tol,
                Some(0.0),
                SolverOperation::Minimize,
            ) {
                Ok(d) => Some(d),
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => None,
            };

            if let Some(d) = dropped {
                if d.success && d.distance < min_dist {
                    got_iso_drop = true;
                    min_dist = d.distance;
                    min_param = d.parameter;
                }
            }

            // Set up for next interval.
            let _ = ivl.set_min_max(ivl.max(), ivl.max());
        }

        if got_iso_drop {
            let crease_uv = if which_param == SurfParamType::U {
                Point2::new(crease_param_val, min_param)
            } else {
                Point2::new(min_param, crease_param_val)
            };
            self.set_uv(crease_uv);
            return Ok(true);
        }

        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::NOW;
    use crate::surface::curve_bounded_surface::CurveBoundedSurface;

    fn bilinear(twist: f64) -> Surface {
        Surface::from(
            BSplineSurface::create_bilinear_surface(
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
                Point3::new(0.0, 1.0, 0.0),
                Point3::new(1.0, 1.0, twist),
            )
            .unwrap(),
        )
    }

    fn unit_dom() -> Extent2d {
        Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0)
    }

    fn global<'s>(
        k: &Kernel,
        s: &'s Surface,
        op: SolverOperation,
        test: Point3,
    ) -> SurfPtGlobalSolver<'s> {
        SurfPtGlobalSolver::new(
            k,
            s,
            &unit_dom(),
            op,
            test,
            1e-6,
            None,
            SolutionRequested::Single,
        )
        .unwrap()
    }

    fn cp(x: f64, y: f64, z: f64) -> CPoint {
        CPoint { x, y, z, w: NOW }
    }

    #[test]
    fn problem_data() {
        let k = Kernel::default();
        let s = bilinear(0.0);
        let g = global(
            &k,
            &s,
            SolverOperation::Minimize,
            Point3::new(0.5, -3.0, 2.0),
        );
        assert!(g.base.b_spline().is_some());
        assert_eq!(g.base.tight_tol(), EFF_ZERO * 4.0);
        assert_eq!(g.base.loose_tol(), 1e-6);
        assert_eq!(g.base.dir_vec(), Vec3::ZERO);
        assert_eq!(g.base.test_point(), Point3::new(0.5, -3.0, 2.0));

        // directed: the direction is negated and unitised, the test point a corner of
        // the expanded box on the far side
        let g = global(
            &k,
            &s,
            SolverOperation::Signed3dDirectedMaximize,
            Point3::new(0.0, 0.0, 2.0),
        );
        assert_eq!(g.base.dir_vec(), Vec3::new(0.0, 0.0, -1.0));
        let t = g.base.test_point();
        assert!(t.z > 0.0 && t.x > 1.0 && t.y > 1.0);

        // a curve-bounded surface has no B-spline part
        let cb = Surface::from(
            CurveBoundedSurface::new(bilinear(0.0), Vec::new(), Vec::new(), true).unwrap(),
        );
        let g = global(&k, &cb, SolverOperation::Minimize, Point3::ZERO);
        assert!(g.base.b_spline().is_none());
        assert!(g.find_polygon_triangle_guess((0, 0)).is_err());
        assert!(g.refine_guess_polygon_leg(0, 0).is_err());
        assert_eq!(g.crease_subdomains().unwrap().len(), 1);
    }

    #[test]
    fn objective_and_guesses() {
        let k = Kernel::default();
        let s = bilinear(0.0);
        let g = global(
            &k,
            &s,
            SolverOperation::Minimize,
            Point3::new(0.25, 0.75, 2.0),
        );
        assert!((g.calc_func_val(&k, Point2::new(0.25, 0.75)).unwrap() - 2.0).abs() < 1e-12);
        let m = global(
            &k,
            &s,
            SolverOperation::Maximize,
            Point3::new(0.25, 0.75, 2.0),
        );
        assert!(m.calc_func_val(&k, Point2::new(0.25, 0.75)).unwrap() < 0.0);

        let r = g.find_polygon_triangle_guess((1, 1)).unwrap();
        assert!((r.uv.x - 0.25).abs() < 1e-12 && (r.uv.y - 0.75).abs() < 1e-12);
        assert!((r.best_dist - 2.0).abs() < 1e-12);
        assert!((r.ave_cpt_dist - 1.0).abs() < 1e-12);
        assert!(!r.suspicious);

        let uv = g.refine_guess_polygon_leg(0, 0).unwrap();
        assert!((uv.x - 0.25).abs() < 1e-12 && (uv.y - 0.5).abs() < 1e-12);
        assert!(g.refine_guess_polygon_leg(5, 0).is_err());
    }

    #[test]
    fn control_net_helpers() {
        // a flat 3 x 3 net
        let flat: Vec<CPoint> = (0..3)
            .flat_map(|i| (0..3).map(move |j| cp(f64::from(i), f64::from(j), 0.0)))
            .collect();
        let t = Point3::new(0.25, 0.25, 1.0);
        let (d, u, v) = triangles_dist(t, &flat, false, 0, 0, 3, 3).unwrap();
        assert!((d - 1.0).abs() < 1e-12 && (u - 0.25).abs() < 1e-12 && (v - 0.25).abs() < 1e-12);
        assert!(triangles_dist(t, &flat, false, 2, 0, 3, 3).is_err());
        for i in 0..3 {
            for j in 0..3 {
                assert!(well_behaved_control_net(t, &flat, false, i, j, 3, 3).unwrap());
            }
        }
        // a fold in u at the middle point
        let mut folded = flat.clone();
        folded[7] = cp(0.0, 1.0, 0.0); // point (2, 1) back on (0, 1)
        assert!(!well_behaved_control_net(t, &folded, false, 1, 1, 3, 3).unwrap());
        // coincident interior points
        let mut coinc = flat;
        coinc[4] = coinc[1];
        assert!(!well_behaved_control_net(t, &coinc, false, 1, 1, 3, 3).unwrap());
    }

    #[test]
    fn seams_from_control_points() {
        let k = Kernel::default();
        let s = bilinear(0.0);
        // a closed ring of 4 points in u (first == last), 2 in v
        let ring = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (1.0, 0.0)];
        let pts: Vec<CPoint> = ring
            .iter()
            .flat_map(|&(x, y)| [cp(x, y, 0.0), cp(x, y, 1.0)])
            .collect();
        // test point near the last leg (towards point 2): prefer the high end
        let g = global(
            &k,
            &s,
            SolverOperation::Minimize,
            Point3::new(0.0, -0.6, 0.0),
        );
        assert_eq!(
            g.check_guess_for_seams_cpts(0, 0, 4, 2, &pts, false)
                .unwrap(),
            (3, 0)
        );
        let g = global(
            &k,
            &s,
            SolverOperation::Minimize,
            Point3::new(0.6, 0.6, 0.0),
        );
        assert_eq!(
            g.check_guess_for_seams_cpts(3, 1, 4, 2, &pts, false)
                .unwrap(),
            (0, 1)
        );
        // interior: unchanged
        assert_eq!(
            g.check_guess_for_seams_cpts(1, 1, 4, 3, &pts, false)
                .unwrap(),
            (1, 1)
        );
    }

    #[test]
    fn extremum_and_boundary_tests() {
        let k = Kernel::default();
        let s = bilinear(0.0);
        let g = global(
            &k,
            &s,
            SolverOperation::Minimize,
            Point3::new(0.5, 0.5, 1.0),
        );
        let mut e = SurfaceEval::new(&k, Some(&s));
        e.set_uv(Point2::new(0.5, 0.5));
        // on the normal of a flat surface: neither (flat is not positive)
        assert!(!g.is_local_min(&mut e, 1e-6).unwrap());
        assert!(!g.is_local_max(&mut e, 1e-6).unwrap());
        // on the surface: a minimum
        let g = global(
            &k,
            &s,
            SolverOperation::Minimize,
            Point3::new(0.5, 0.5, 0.0),
        );
        assert!(g.is_local_min(&mut e, 1e-6).unwrap());
        assert!(!g.is_local_max(&mut e, 1e-6).unwrap());

        // pushing the u = 1 boundary
        let g = global(
            &k,
            &s,
            SolverOperation::Minimize,
            Point3::new(2.0, 0.5, 0.0),
        );
        let mut e = SurfaceEval::new(&k, Some(&s));
        e.set_uv(Point2::new(1.0, 0.5));
        assert_eq!(
            g.pushing_boundary(&mut e, 1e-6).unwrap(),
            (true, SurfParamType::UMax)
        );
        // off a corner
        let g = global(
            &k,
            &s,
            SolverOperation::Minimize,
            Point3::new(-1.0, -1.0, 0.0),
        );
        let mut e = SurfaceEval::new(&k, Some(&s));
        e.set_uv(Point2::new(0.0, 0.0));
        assert_eq!(
            g.pushing_boundary(&mut e, 1e-6).unwrap(),
            (true, SurfParamType::Both)
        );
        // inside: not on a boundary
        let mut e = SurfaceEval::new(&k, Some(&s));
        e.set_uv(Point2::new(0.5, 0.5));
        assert_eq!(
            g.pushing_boundary(&mut e, 1e-6).unwrap(),
            (false, SurfParamType::Neither)
        );
    }

    #[test]
    fn bad_guess_stride() {
        let k = Kernel::default();
        let s = bilinear(0.0);
        let g = global(
            &k,
            &s,
            SolverOperation::Minimize,
            Point3::new(0.8, 0.3, 1.0),
        );
        let mut e = SurfaceEval::new(&k, Some(&s));
        e.set_uv(Point2::new(0.1, 0.9));
        let (pt, f) = g.check_bad_guess_stride(&k, &mut e).unwrap().unwrap();
        assert!((pt.uv().x - 0.8).abs() < 1e-12 && (pt.uv().y - 0.3).abs() < 1e-12);
        assert!((f - 1.0).abs() < 1e-12);
    }

    #[test]
    fn stepper_basics() {
        let k = Kernel::default();
        let s = bilinear(0.0);
        let test = Point3::new(0.5, 0.5, 2.0);
        let mut st = SurfPtSolveStepper::new(
            &k,
            &s,
            &unit_dom(),
            test,
            Point2::new(0.25, 0.5),
            1e-12,
            1e-6,
            SolverOperation::Minimize,
        )
        .unwrap();
        // bilinear: big steps allowed
        assert_eq!(st.max_step(), 2.0);
        assert_eq!(st.base.tight_tol(), 1e-12);
        assert!(!st.is_set());
        let v = st.eval().unwrap();
        assert!(st.is_set());
        assert_eq!(v.gap, Vec3::new(-0.25, 0.0, -2.0));
        assert_eq!(v.f_val, -0.25);
        assert_eq!(v.g_val, 0.0);
        assert_eq!(v.sum_check, 0.25);
        assert!((st.gap_length().unwrap() - (0.0625_f64 + 4.0).sqrt()).abs() < 1e-15);
        assert!(st.normal(&k,).is_ok());

        // the copy is not evaluated, the assignment copies the evaluation
        let c = SurfPtSolveStepper::copy_of(&k, &st);
        assert!(!c.is_set());
        assert_eq!(c.uv(), st.uv());
        let mut a = SurfPtSolveStepper::copy_of(&k, &c);
        a.assign(&st);
        assert!(a.is_set());

        // clamp
        st.set_uv(Point2::new(1.5, -0.5));
        assert!(!st.is_set());
        st.clamp();
        assert_eq!(st.uv(), Point2::new(1.0, 0.0));

        // boundary hits
        st.hit_bound(SurfParamType::U, 1);
        st.hit_bound(SurfParamType::U, 1);
        assert_eq!(st.bound_hits_u(), 2);
        st.hit_bound(SurfParamType::U, -1);
        assert_eq!(st.bound_hits_u(), -1);
        st.hit_bound(SurfParamType::U, 0); // on the boundary: kept
        assert_eq!(st.bound_hits_u(), -1);
        st.set_uv(Point2::new(0.5, 0.5));
        st.hit_bound(SurfParamType::U, 0); // strictly inside: reset
        assert_eq!(st.bound_hits_u(), 0);
        st.hit_bound(SurfParamType::V, -1);
        assert_eq!(st.bound_hits_v(), -1);
        st.set_iter_count(3);
        st.set_nr_suspect(true);
        assert_eq!(st.iter_count(), 3);
        assert!(st.nr_step_suspect());
    }

    #[test]
    fn bounds_check() {
        let d = unit_dom();
        let r = SurfPtSolveStepper::check_bounds(Point2::new(0.5, 0.5), Point2::new(0.7, 0.2), &d);
        assert!(!r.out_of_bounds && r.step_scale == 1.0 && r.clamped_uv.is_none());
        let r = SurfPtSolveStepper::check_bounds(Point2::new(0.5, 0.5), Point2::new(1.5, -0.5), &d);
        assert!(r.out_of_bounds);
        assert_eq!((r.hits_u, r.hits_v), (1, -1));
        assert_eq!(r.step_scale, 0.5);
        assert_eq!(r.clamped_uv, Some(Point2::new(1.0, 0.0)));
        assert_eq!(r.clamped_uv_scaled, Some(Point2::new(1.0, 0.0)));
        // starting outside: no step
        let r =
            SurfPtSolveStepper::check_bounds(Point2::new(-0.5, 0.5), Point2::new(-1.0, 0.5), &d);
        assert_eq!(r.step_scale, 0.0);
    }

    #[test]
    fn drop_to_iso_curve() {
        let k = Kernel::default();
        let s = bilinear(0.0);
        let mut st = SurfPtSolveStepper::new(
            &k,
            &s,
            &unit_dom(),
            Point3::new(0.3, 0.7, 1.0),
            Point2::new(0.5, 0.5),
            1e-12,
            1e-6,
            SolverOperation::Minimize,
        )
        .unwrap();
        assert!(st
            .drop_to_iso_curve(&k, SurfParamType::U, 0.2, 0.5)
            .unwrap());
        assert!((st.uv().x - 0.2).abs() < 1e-12 && (st.uv().y - 0.7).abs() < 1e-9);

        let cb = Surface::from(
            CurveBoundedSurface::new(bilinear(0.0), Vec::new(), Vec::new(), true).unwrap(),
        );
        let mut st = SurfPtSolveStepper::new(
            &k,
            &cb,
            &unit_dom(),
            Point3::ZERO,
            Point2::new(0.5, 0.5),
            1e-12,
            1e-6,
            SolverOperation::Minimize,
        )
        .unwrap();
        // the curve-bounded iso curve (SmIsoCurve) is ported since surface.surface_p3 (#52)
        assert!(st.drop_to_iso_curve(&k, SurfParamType::U, 0.2, 0.5).is_ok());
    }
}
