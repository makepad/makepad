//! Surface-point solving, part 2: the global solver's rough guess (control net and
//! grid), its refinements (tangent-plane steps, grids), the seam solutions and the
//! main `Solve`; the local stepper's step adjustment, error norms, convergence and
//! singularity tests, Newton / plane step, line searches and boundary tests.
//!
//! Upstream: `source/SMLib/src/SmSurfacePointSolve.cpp`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`SurfPtGlobalSolver::find_rough_guess`], [`RoughGuess`] | `SmSurfPtGlobalSolver::FindRoughGuess`    |
//! | [`SurfPtGlobalSolver::find_polygon_guess`], [`PolygonGuess`] | `SmSurfPtGlobalSolver::FindPolygonGuess` |
//! | [`SurfPtGlobalSolver::refine_guess`]              | `SmSurfPtGlobalSolver::RefineGuess`              |
//! | [`SurfPtGlobalSolver::refine_patch_by_grid`]      | `SmSurfPtGlobalSolver::RefinePatchByGrid`        |
//! | [`SurfPtGlobalSolver::refine_guess_by_grid`]      | `SmSurfPtGlobalSolver::RefineGuessByGrid`        |
//! | [`SurfPtGlobalSolver::check_seams`]               | `SmSurfPtGlobalSolver::CheckSeams`               |
//! | [`SurfPtGlobalSolver::solve`]                     | `SmSurfPtGlobalSolver::Solve`                    |
//! | [`SurfPtSolveStepper::copy_of`] (in `p1`)         | `SmSurfPtSolveStepper` copy constructor          |
//! | [`SurfPtSolveStepper::adjust_step`]               | `SmSurfPtSolveStepper::AdjustStep`               |
//! | [`SurfPtSolveStepper::error_norm`]                | `SmSurfPtSolveStepper::ErrorNorm`                |
//! | [`SurfPtSolveStepper::converged`], [`SurfPtSolveStepper::converged_with`] | `Converged(double)`, `Converged(SmBoolean)` |
//! | [`SurfPtSolveStepper::is_singular`]               | `SmSurfPtSolveStepper::IsSingular`               |
//! | [`SurfPtSolveStepper::step`]                      | `SmSurfPtSolveStepper::Step`                     |
//! | [`SurfPtSolveStepper::line_search`]               | `SmSurfPtSolveStepper::LineSearch`               |
//! | [`SurfPtSolveStepper::line_search_bracketted`]    | `SmSurfPtSolveStepper::LineSearchBracketted`     |
//! | [`SurfPtSolveStepper::converged_on_boundary`]     | `SmSurfPtSolveStepper::ConvergedOnBoundary`      |
//! | [`SurfPtSolveStepper::pushing_boundary`]          | `SmSurfPtSolveStepper::PushingBoundary`          |
//! | [`SurfPtSolveStepper::snap_to_boundaries`]        | `SmSurfPtSolveStepper::SnapToBoundaries`         |
//!
//! Deviations:
//! - **Out-params** become return values ([`RoughGuess`], [`PolygonGuess`],
//!   `(adjusted, scale)` of the step adjustment, `(bool, SurfParamType)` of the
//!   boundary and singularity tests, the solution array). The in/out guesses of
//!   `RefineGuess` and `RefinePatchByGrid` (and the latter's best value) stay `&mut`
//!   arguments: upstream changes them before some of its error returns. The unused
//!   index outputs of `RefineGuess` are dropped.
//! - **Status returns.** `AdjustStep`'s `SM_ERR` (no step and outside the domain)
//!   is [`SolidError`] `SM_ERR`; upstream's outputs are then `adjusted = TRUE`,
//!   `scale = 0`, and the stepper is clamped to the domain. A boundary test that is
//!   not on a boundary reports [`SurfParamType::Neither`] where upstream leaves the
//!   caller's variable unset.
//! - **Ignored statuses** keep upstream's values: an evaluation that fails leaves
//!   the point undefined ([`Point3::UNDEF`], upstream uninitialised) or at its previous
//!   value; `smgu_VecLinCombTwoVectors` writes `(0, 0)` before failing.
//!   [`SolidError::Unported`] always propagates.
//! - **Out-of-bounds evaluation.** `Solve` enables out-of-bounds evaluation of an
//!   `SmBSplineSurface` (a B-spline, plane or surface of revolution) for the duration
//!   of the solve (`SmTemporaryChangeValue`), in place on the borrowed surface (the
//!   flag is a `Cell`), as upstream; the flag is restored after the solve.
//! - **Solutions' objects** are the caller's handle for the surface (`O`, upstream
//!   the `SmSurface*`).
//! - **Unported callees.** `SmSurface::LocalPointSolve` (`surface.surface_p5`) for
//!   every surface and `SmSurfaceEval::Normal` (`surface.surface_p4`, through `p1`) return
//!   [`SolidError::Unported`].
//! - The stepper's `IsLocalMin` / `IsLocalMax` are not used upstream and are not
//!   ported. Debug-only drawing and `SM_ASSERT`s are omitted.

use crate::error::{Result, SolidError};
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::{line_closest_point, vec_lin_comb_two_vectors};
use crate::geom::global_solver::SolverOperation;
use crate::geom::local_solve_nd::TerminationReason;
use crate::geom::solution_array::{Solution, SolutionArray, SolutionObject, SortKey};
use crate::geom::tol::is_zero;
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::Point3;
use crate::kernel::Kernel;
use crate::math::{min2, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_PARAM, EFF_ZERO_SQ, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_surface::{BSplineSurface, SurfParamType};
use crate::surface::surface::p2::SurfaceEval;
use crate::surface::surface::{Surface, SurfaceKind};

use super::p1::{control_point, ErrorNormType, SurfPtGlobalSolver, SurfPtSolveStepper};

const OP_ROUGH: &str = "surface::surface_point_solve::p2::SurfPtGlobalSolver::find_rough_guess";
const OP_POLY: &str = "surface::surface_point_solve::p2::SurfPtGlobalSolver::find_polygon_guess";
const OP_SOLVE: &str = "surface::surface_point_solve::p2::SurfPtGlobalSolver::solve";
const OP_ADJUST: &str = "surface::surface_point_solve::p2::SurfPtSolveStepper::adjust_step";
const OP_LINE_B: &str =
    "surface::surface_point_solve::p2::SurfPtSolveStepper::line_search_bracketted";

/// The result of [`SurfPtGlobalSolver::find_rough_guess`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoughGuess {
    /// The UV guess (`rUVGuess`).
    pub uv: Point2,
    /// The best control point's u index (`rlBestIdxU`; `0` without a control net).
    pub best_idx_u: usize,
    /// The best control point's v index (`rlBestIdxV`).
    pub best_idx_v: usize,
}

/// The result of [`SurfPtGlobalSolver::find_polygon_guess`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolygonGuess {
    /// The UV guess (`rUVGuess`; undefined when the control net could not be read
    /// and the guess is not suspicious, as upstream).
    pub uv: Point2,
    /// The best control point's u index (`rlBestIdxU`).
    pub best_idx_u: usize,
    /// The best control point's v index (`rlBestIdxV`).
    pub best_idx_v: usize,
    /// The control net may be far from the surface (`rbSuspicious`).
    pub suspicious: bool,
}

impl SurfPtGlobalSolver<'_> {
    /// A first rough guess (`FindRoughGuess`): the control-net guess, and when that
    /// fails or is suspicious, the best of a 7 x 7, 5 x 5 and 3 x 3 grid search over
    /// ever smaller sub-domains around the best grid point (fewer points for a net
    /// with fewer control points); with both, the better of the two.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when neither guess has a defined objective value; those of the
    /// objective function and the grid search.
    pub fn find_rough_guess(&mut self, k: &Kernel) -> Result<RoughGuess> {
        // init output
        let mut best_idx_u = 0;
        let mut best_idx_v = 0;
        let mut uv = Point2::UNDEF;

        // locals
        let mut suspicious = false;

        // Find a guess from a BSplineSurface's control polygon
        let poly_ok = match self.find_polygon_guess(k, (best_idx_u, best_idx_v)) {
            Ok(g) => {
                uv = g.uv;
                best_idx_u = g.best_idx_u;
                best_idx_v = g.best_idx_v;
                suspicious = g.suspicious;
                true
            }
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => false,
        };

        // when FindPolygonGuess() worked without setting the bSuspicious flag
        if poly_ok && !suspicious {
            return Ok(RoughGuess {
                uv,
                best_idx_u,
                best_idx_v,
            });
        }

        // Next: Find the best GuessUV out of a Grid of UV samples.
        // Grid 7x7, then 5x5, then 3x3.  [080718; B46; B51; B394]
        let mut grid_size_u: i32 = 7; // 5 is too small [B394]
        let mut grid_size_v: i32 = 7;
        let num_iters = 3;

        // If it's a single linear patch, don't subdivide.
        // Similar for just a few control points.
        let surface = self.base.surface();
        let num_cpts =
            i32::try_from(surface.number_control_points(k, SurfParamType::U)).unwrap_or(i32::MAX);
        if num_cpts < grid_size_u && num_cpts > 0 {
            grid_size_u = num_cpts;
        }
        let num_cpts =
            i32::try_from(surface.number_control_points(k, SurfParamType::V)).unwrap_or(i32::MAX);
        if num_cpts < grid_size_v && num_cpts > 0 {
            grid_size_v = num_cpts;
        }

        let domain = *self.base.domain();
        let mut domain_size = domain.size();

        let mut sub_domain = domain;
        let mut grid_uv_guess = Point2::UNDEF;
        let mut grid_ok = false;

        let mut best_func_val = BIG_DOUBLE;

        // for a sequence of ever smaller sub-domains centered on the last iterations
        // best guess, search a grid of UV samples for the best UV guess point
        for _ in 0..num_iters {
            self.refine_patch_by_grid(
                k,
                &sub_domain,
                grid_size_u,
                grid_size_v,
                &mut grid_uv_guess,
                &mut best_func_val,
            )?;
            grid_ok = true;
            if grid_size_u < 2 || grid_size_v < 2 {
                break; // Just to be safe.
            }

            let fraction_u = 1.0 / f64::from(grid_size_u - 1) + 0.01;
            let fraction_v = 1.0 / f64::from(grid_size_v - 1) + 0.01;
            domain_size.x *= fraction_u;
            domain_size.y *= fraction_v;
            let uv_min = domain.clamp_point(grid_uv_guess - domain_size);
            let uv_max = domain.clamp_point(grid_uv_guess + domain_size);
            // (status ignored, as upstream)
            let _ = sub_domain.set_min_max(uv_min, uv_max);

            // Decrease grid size for the next iteration: 7-5-3.
            if grid_size_u > 4 {
                grid_size_u -= 1;
            }
            if grid_size_v > 4 {
                grid_size_v -= 1;
            }
            if grid_size_u > 2 {
                grid_size_u -= 1;
            }
            if grid_size_v > 2 {
                grid_size_v -= 1;
            }
        }

        // If guesses from both FindPolygonGuess() and RefinePatchByGrid(), pick the
        // better one.
        if poly_ok && grid_ok {
            let poly_dist = self.calc_func_val(k, uv)?;
            let grid_dist = self.calc_func_val(k, grid_uv_guess)?;

            // when either guess is uninitialized - quit
            if poly_dist >= BIG_DOUBLE && grid_dist >= BIG_DOUBLE {
                return Err(SolidError::sm_err(OP_ROUGH));
            }

            // when the GridGuess is better than the PolyGuess, use it
            if grid_dist < poly_dist {
                uv = grid_uv_guess;
            }
        } else if grid_ok {
            uv = grid_uv_guess;
        } else {
            // both failed
            return Err(SolidError::sm_err(OP_ROUGH));
        }

        Ok(RoughGuess {
            uv,
            best_idx_u,
            best_idx_v,
        })
    }

    /// A UV guess from the control net (`FindPolygonGuess`): the control-net
    /// triangle guess, unless it is suspicious (a suspicious guess is accepted when
    /// both the net and the surface come within 1% of the average control-point
    /// spacing); else the closest control point (the extreme one for maximise and
    /// the directed operation), refined along its polygon legs, trying every point of
    /// a row or column of coincident points. `best_idx` is the caller's best control
    /// point, kept when no net point qualifies (upstream's in/out indices).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a B-spline surface with a NURB; those of the seam check, the
    /// leg refinement and the Greville abscissae.
    pub fn find_polygon_guess(&self, k: &Kernel, best_idx: (usize, usize)) -> Result<PolygonGuess> {
        let b = &self.base;
        // We need a SmBSplineSurface to work with control points.
        let Some(bspl) = b.b_spline() else {
            return Err(SolidError::sm_err(OP_POLY));
        };
        let op = b.solver_op();
        let test_pt = b.test_point();

        let (mut best_idx_u, mut best_idx_v) = best_idx;
        let mut uv = Point2::UNDEF;

        // The suspicious triangle guess, kept as a candidate with `fixes.point_solve_keeps_triangle_guess`
        // (WR-444, #1164): the nearest control point found below can be worse.
        let mut triangle: Option<(Point2, usize, usize)> = None;

        // find closest ControlPoint triangle with point closest to target
        let suspicious = match self.find_polygon_triangle_guess((best_idx_u, best_idx_v)) {
            Ok(t) => {
                uv = t.uv;
                best_idx_u = t.best_idx_u;
                best_idx_v = t.best_idx_v;
                let mut suspicious = t.suspicious;

                // When worried ControlPoint Mesh may be far from Surface
                if suspicious && op != SolverOperation::Signed3dDirectedMaximize {
                    // when the BestDist is small compared to the Average spacing
                    if t.best_dist < t.ave_cpt_dist / 100.0 {
                        // Also check distance to the surface. [091002]
                        let srf_pt = eval_point(k, b.surface(), uv, Point3::UNDEF)?;
                        let surf_dist = srf_pt.distance(test_pt);

                        if surf_dist < t.ave_cpt_dist / 100.0 {
                            suspicious = false;
                        }
                    }
                }
                if suspicious
                    && k.fixes.point_solve_keeps_triangle_guess
                    && op != SolverOperation::Signed3dDirectedMaximize
                {
                    triangle = Some((b.domain().clamp_point(uv), best_idx_u, best_idx_v));
                }
                suspicious
            }
            Err(e) if e.is_unported() => return Err(e),
            // (the control net could not be read: only the rational warning is set)
            Err(_) => bspl.is_rational(),
        };

        // If it looks like the UVGuess is good - return
        if !suspicious {
            return Ok(PolygonGuess {
                uv,
                best_idx_u,
                best_idx_v,
                suspicious,
            });
        }

        // Next try: Iter every ControlPoint and find the one closest to the target
        let rational = bspl.is_rational();
        let (num_u, num_v, cpts) = bspl
            .control_points()
            .map_err(|_| SolidError::sm_err(OP_POLY))?;

        let mut tied: Option<(usize, usize)> = None;
        let mut num_ties: usize = 0;
        let mut best_dist = BIG_DOUBLE;
        if op == SolverOperation::Maximize {
            best_dist = -best_dist;
        }

        for ii in 0..num_u {
            for jj in 0..num_v {
                let this_pt = control_point(&cpts, num_v, ii, jj, rational)?;

                // the best dThisDist has the smallest signed value
                let this_dist = if op == SolverOperation::Maximize {
                    -test_pt.distance(this_pt)
                } else if op == SolverOperation::Signed3dDirectedMaximize {
                    this_pt.dot(b.dir_vec())
                } else {
                    test_pt.distance(this_pt)
                };

                // save best found case
                if this_dist < best_dist - EFF_ZERO {
                    best_dist = this_dist;
                    best_idx_u = ii;
                    best_idx_v = jj;
                    // No tie; reset.
                    tied = None;
                    num_ties = 0;
                }
                // else if two equivalent choices are found
                else if this_dist < best_dist + EFF_ZERO {
                    // 'Equal': record a tie.
                    num_ties += 1;
                    tied = Some((best_idx_u, best_idx_v));
                    best_idx_u = ii;
                    best_idx_v = jj;
                }
            }
        }

        // Have to check here for seams (status ignored).
        let (iu, iv) =
            self.check_guess_for_seams_cpts(best_idx_u, best_idx_v, num_u, num_v, &cpts, rational)?;
        best_idx_u = iu;
        best_idx_v = iv;

        // Refine along the legs of the control polygon.
        let mut uv_guess_poly = Point2::UNDEF;
        let mut leg_ok = match self.refine_guess_polygon_leg(best_idx_u, best_idx_v) {
            Ok(g) => {
                uv_guess_poly = g;
                true
            }
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => false,
        };

        // Check for ties -- coincident control points.
        let mut best_dist = BIG_DOUBLE;
        let mut srf_pt = Point3::UNDEF;
        let tied_u = tied.map(|t| t.0);
        let tied_v = tied.map(|t| t.1);

        if num_ties >= 2 && num_ties == num_u.wrapping_sub(1) && tied_v == Some(best_idx_v) {
            // Adjacent u-points with the same value (presumably coincident).
            for ii in 0..num_u {
                if let Some(guess) = self.leg_or_none(ii, best_idx_v)? {
                    srf_pt = eval_point(k, b.surface(), guess, srf_pt)?;
                    let this_dist = srf_pt.distance(test_pt);
                    if this_dist < best_dist {
                        best_dist = this_dist;
                        uv_guess_poly = guess;
                        best_idx_u = ii;

                        // Quit if we get a direct hit.
                        if this_dist < b.loose_tol() {
                            break;
                        }
                    }
                }
            }
            leg_ok = best_dist < BIG_DOUBLE / 2.0;
        } else if num_ties >= 2 && num_ties == num_v.wrapping_sub(1) && tied_u == Some(best_idx_u) {
            // Adjacent v-points.
            for ii in 0..num_v {
                if let Some(guess) = self.leg_or_none(best_idx_u, ii)? {
                    srf_pt = eval_point(k, b.surface(), guess, srf_pt)?;
                    let this_dist = srf_pt.distance(test_pt);
                    if this_dist < best_dist {
                        best_dist = this_dist;
                        uv_guess_poly = guess;
                        best_idx_v = ii;

                        if this_dist < b.loose_tol() {
                            break;
                        }
                    }
                }
            }
            leg_ok = best_dist < BIG_DOUBLE / 2.0;
        }

        if leg_ok {
            uv = uv_guess_poly;
        } else {
            // Use the Greville abscissae for the control point indices.
            uv = Point2::new(
                bspl.greville_abscissa(SurfParamType::U, best_idx_u)?,
                bspl.greville_abscissa(SurfParamType::V, best_idx_v)?,
            );
        }

        let uv = b.domain().clamp_point(uv);

        // Return the better of the triangle guess and the control-point guess, judged on
        // the surface itself (`fixes.point_solve_keeps_triangle_guess`, WR-444, #1164).
        if let Some((tri_uv, tri_u, tri_v)) = triangle {
            let on_surface = |guess: Point2| match self.calc_func_val(k, guess) {
                Ok(v) => Ok(Some(v)),
                Err(e) if e.is_unported() => Err(e),
                Err(_) => Ok(None),
            };
            if let (Some(tri), Some(cpt)) = (on_surface(tri_uv)?, on_surface(uv)?) {
                if tri < cpt {
                    return Ok(PolygonGuess {
                        uv: tri_uv,
                        best_idx_u: tri_u,
                        best_idx_v: tri_v,
                        suspicious,
                    });
                }
            }
        }

        Ok(PolygonGuess {
            uv,
            best_idx_u,
            best_idx_v,
            suspicious,
        })
    }

    /// `RefineGuessPolygonLeg` with its status: the guess, `None` on failure.
    fn leg_or_none(&self, idx_u: usize, idx_v: usize) -> Result<Option<Point2>> {
        match self.refine_guess_polygon_leg(idx_u, idx_v) {
            Ok(g) => Ok(Some(g)),
            Err(e) if e.is_unported() => Err(e),
            Err(_) => Ok(None),
        }
    }

    /// Refines `uv_guess` to where Newton iteration should work (`RefineGuess`): up to
    /// three tangent-plane steps (each at most 10% of the domain; for the directed
    /// operation steps along the descent direction, from 5% of the domain halving),
    /// with seam checks, stopping on a negligible objective value or step; the result
    /// is the better of the last two points.
    ///
    /// On an evaluation error `uv_guess` holds the seam-checked input, as upstream.
    ///
    /// # Errors
    ///
    /// Those of the surface derivatives (upstream's `SER`), the objective function
    /// and the seam checks.
    pub fn refine_guess(&mut self, k: &Kernel, uv_guess: &mut Point2) -> Result<()> {
        // Note: don't use a Newton step here, we're presumably not close enough.
        // Try a couple of plane steps.
        *uv_guess = self.seams_status_ignored(k, *uv_guess)?; // Since we do this on subsequent guesses...

        let mut this_uv = *uv_guess;
        let mut next_uv = *uv_guess;
        let mut this_f_val;
        let mut next_f_val = BIG_DOUBLE;

        let domain = *self.base.domain();
        let u_extent = domain.u_interval().length();
        let v_extent = domain.v_interval().length();

        // Don't ever take very big steps here.
        let max_u_shift = u_extent * 0.10;
        let max_v_shift = v_extent * 0.10;

        // For directed operations, we have to guess a step size.
        // Start at 1/20 of the domain, and shrink each iteration.
        let domain_size = domain.size().length();
        let mut step_size_guess = 0.05 * domain_size;

        let op = self.base.solver_op();

        for _ in 0..3 {
            this_f_val = next_f_val;

            next_f_val = self.calc_func_val(k, next_uv)?;

            if next_f_val.abs() < EFF_ZERO_SQRT {
                *uv_guess = next_uv;
                return Ok(()); // very close, definitely good enough guess.
            }

            if next_f_val >= this_f_val {
                // if no movement, then we're close enough.
                if (next_uv.x - this_uv.x).abs() < u_extent / 1000.0
                    && (next_uv.y - this_uv.y).abs() < v_extent / 1000.0
                {
                    break;
                }

                if op == SolverOperation::Signed3dDirectedMaximize {
                    next_uv = (this_uv + next_uv) / 2.0;
                    continue;
                }
                break; // have to try something different.
            }

            // Take a plane step.
            let (pt, su, sv) = self
                .base
                .surface()
                .evaluate_1st_derivatives(k, next_uv, false, false)?;

            let (u_shift, v_shift);
            if op == SolverOperation::Signed3dDirectedMaximize {
                // Plane step doesn't work here.  All we can get is a direction.
                let mut uv_step = vec_lin_comb_two_vectors(su, sv, -self.base.dir_vec())
                    .unwrap_or(Vec2::new(0.0, 0.0));
                let mag = uv_step.length();
                if mag < EFF_ZERO_SQRT {
                    break;
                }

                uv_step *= step_size_guess / mag;
                step_size_guess /= 2.0; // shrink on each iteration.

                u_shift = uv_step.x;
                v_shift = uv_step.y;
            } else {
                // 'Point' cases.
                let gap = pt - self.base.test_point();

                let f = gap.dot(su); // These are the right-hand side (b-vector)
                let g = gap.dot(sv);

                // Solve 2x2 to get step
                let fu = su.dot(su);
                let fv = su.dot(sv);
                let gu = fv;
                let gv = sv.dot(sv);

                let det = fu * gv - fv * gu;
                let mut us = -(f * gv - fv * g); // numerators
                let mut vs = -(fu * g - f * gu); // to be divided by dDet.

                // For our purposes, if singular, just do what we can.
                if is_zero(det) || is_zero(us) {
                    us = 0.0;
                } else if det.abs() > us.abs() * EFF_ZERO_SQRT {
                    us /= det;
                } else {
                    us = 0.0;
                }

                if is_zero(det) || is_zero(vs) {
                    vs = 0.0;
                } else if det.abs() > vs.abs() * EFF_ZERO_SQRT {
                    vs /= det;
                } else {
                    vs = 0.0;
                }

                // if no movement, then we're close enough.
                if us.abs() < u_extent / 1000.0 && vs.abs() < v_extent / 1000.0 {
                    *uv_guess = next_uv;
                    return Ok(());
                }

                // Don't take big steps.
                if us.abs() > max_u_shift {
                    us = if us > 0.0 { max_u_shift } else { -max_u_shift };
                }
                if vs.abs() > max_v_shift {
                    vs = if vs > 0.0 { max_v_shift } else { -max_v_shift };
                }
                u_shift = us;
                v_shift = vs;
            }

            // Set up for next iter.
            this_uv = next_uv;

            next_uv = Point2::new(this_uv.x + u_shift, this_uv.y + v_shift);

            next_uv = domain.clamp_point(next_uv);

            next_uv = self.seams_status_ignored(k, next_uv)?;
        }

        // We didn't return with success during the loop.
        this_f_val = next_f_val;
        next_f_val = self.calc_func_val(k, next_uv)?;

        *uv_guess = if next_f_val < this_f_val {
            next_uv
        } else {
            this_uv
        };
        Ok(())
    }

    /// `CheckGuessForSeams(rUVGuess)` with its status ignored: the input on failure.
    fn seams_status_ignored(&mut self, k: &Kernel, uv: Point2) -> Result<Point2> {
        match self.check_guess_for_seams(k, uv) {
            Ok(u) => Ok(u),
            Err(e) if e.is_unported() => Err(e),
            Err(_) => Ok(uv),
        }
    }

    /// Grids `sub_domain` with `num_u` x `num_v` points (at least 2 x 2) and keeps the
    /// best point to the test point (`RefinePatchByGrid`), trying a plane step from
    /// each grid point too ([`SurfPtGlobalSolver::check_bad_guess_stride`]).
    /// `best_func_val` carries the best objective value over sequential calls:
    /// `uv_guess` changes only when a point beats it.
    ///
    /// # Errors
    ///
    /// Those of the objective function and the stride check.
    pub fn refine_patch_by_grid(
        &self,
        k: &Kernel,
        sub_domain: &Extent2d,
        num_u: i32,
        num_v: i32,
        uv_guess: &mut Point2,
        best_func_val: &mut f64,
    ) -> Result<()> {
        let surface = self.base.surface();
        let mut this_grid_pt = SurfaceEval::new(k, Some(surface));

        let num_u = num_u.max(2);
        let num_v = num_v.max(2);

        for ii in 0..num_u {
            let frac_u = f64::from(ii) / f64::from(num_u - 1);

            for jj in 0..num_v {
                let frac_v = f64::from(jj) / f64::from(num_v - 1);

                let this_uv = sub_domain.evaluate(frac_u, frac_v);

                this_grid_pt.set_uv(this_uv);

                let mut this_val = self.calc_func_val(k, this_uv)?;

                let mut best_uv = this_uv; // Best of: this uv, or one of the refined ones.

                // Check for too-big strides.  [B46, B51]
                if let Some((temp_srf_pt, temp_val)) =
                    self.check_bad_guess_stride(k, &mut this_grid_pt)?
                {
                    if temp_val < this_val {
                        this_val = temp_val;
                        best_uv = temp_srf_pt.uv();
                    }
                }

                if this_val < *best_func_val {
                    *uv_guess = best_uv;
                    *best_func_val = this_val;
                }
            }
        }

        Ok(())
    }

    /// A third refinement of `uv_guess` by brute-force gridding (`RefineGuessByGrid`):
    /// 6 x 6 grids over the control-polygon neighbourhood of the best control point
    /// `(best_idx_u, best_idx_v)` (a quarter of the domain each way around the guess
    /// without a control net), then twice over a grid cell around the best point;
    /// keeps the input when no grid point is better.
    ///
    /// # Errors
    ///
    /// Those of the objective function, the grid search and the Greville abscissae.
    pub fn refine_guess_by_grid(
        &self,
        k: &Kernel,
        uv_guess: Point2,
        best_idx_u: usize,
        best_idx_v: usize,
    ) -> Result<Point2> {
        let mut uv_guess = uv_guess;
        let domain = *self.base.domain();

        // Sometimes the original guess is better than any gridded value.
        let mut best_uv_guess = uv_guess;
        let mut best_val = self.calc_func_val(k, best_uv_guess)?;

        // Find a subdomain around rUVGuess.
        let (mut min_uv, mut max_uv);

        if let Some(bspl) = self.base.b_spline() {
            let num_u = bspl.number_control_points(SurfParamType::U);
            let num_v = bspl.number_control_points(SurfParamType::V);

            let lo_u = if best_idx_u > 0 { best_idx_u - 1 } else { 0 };
            let hi_u = if best_idx_u < num_u.wrapping_sub(1) {
                best_idx_u + 1
            } else {
                num_u.wrapping_sub(1)
            };
            let lo_v = if best_idx_v > 0 { best_idx_v - 1 } else { 0 };
            let hi_v = if best_idx_v < num_v.wrapping_sub(1) {
                best_idx_v + 1
            } else {
                num_v.wrapping_sub(1)
            };

            min_uv = Point2::new(
                bspl.greville_abscissa(SurfParamType::U, lo_u)?,
                bspl.greville_abscissa(SurfParamType::V, lo_v)?,
            );
            max_uv = Point2::new(
                bspl.greville_abscissa(SurfParamType::U, hi_u)?,
                bspl.greville_abscissa(SurfParamType::V, hi_v)?,
            );
        } else {
            // No B-Spline surface.  Use 1/4 of the domain.
            let domain_size = domain.size();
            min_uv = uv_guess - domain_size / 4.0;
            max_uv = uv_guess + domain_size / 4.0;
        }

        min_uv = domain.clamp_point(min_uv);
        max_uv = domain.clamp_point(max_uv);

        let mut sub_domain = Extent2d::from_points(min_uv, max_uv);

        let grid_size: i32 = 6;

        let mut this_val = BIG_DOUBLE;

        self.refine_patch_by_grid(
            k,
            &sub_domain,
            grid_size,
            grid_size,
            &mut uv_guess,
            &mut this_val,
        )?;

        // Check whether it improved.
        if this_val < best_val {
            best_val = this_val;
            best_uv_guess = uv_guess;
        }

        // Smaller grids, twice.
        for last in [false, true] {
            let delta_u = max_uv.x - min_uv.x;
            let delta_v = max_uv.y - min_uv.y;

            min_uv = Point2::new(
                uv_guess.x - delta_u / f64::from(grid_size - 1),
                uv_guess.y - delta_v / f64::from(grid_size - 1),
            );
            max_uv = Point2::new(
                uv_guess.x + delta_u / f64::from(grid_size - 1),
                uv_guess.y + delta_v / f64::from(grid_size - 1),
            );

            min_uv = domain.clamp_point(min_uv);
            max_uv = domain.clamp_point(max_uv);

            // (status ignored, as upstream)
            let _ = sub_domain.set_min_max(min_uv, max_uv);

            self.refine_patch_by_grid(
                k,
                &sub_domain,
                grid_size,
                grid_size,
                &mut uv_guess,
                &mut this_val,
            )?;

            if last {
                // Check whether it improved.
                if this_val > best_val {
                    uv_guess = best_uv_guess;
                }
            } else if this_val < best_val {
                best_val = this_val;
                best_uv_guess = uv_guess;
            }
        }

        Ok(uv_guess)
    }

    /// The solutions for `sol` across the seams it is on (`CheckSeams`): `sol` (moved
    /// onto a seam when that is no worse) and, on a closed surface, the point on the
    /// other side of each seam (and of both, for a torus) when it is a solution too,
    /// sorted by parameters. `surface_obj` is the caller's handle for the surface,
    /// stored in the added solutions. `_tol` is not used (as upstream).
    ///
    /// # Errors
    ///
    /// Those of the boundary and closure tests, the evaluations and the sorted
    /// insertion.
    pub fn check_seams<O: SolutionObject>(
        &mut self,
        k: &Kernel,
        _tol: f64,
        sol: &Solution<O>,
        surface_obj: Option<O>,
    ) -> Result<SolutionArray<O>> {
        let mut solutions = SolutionArray::new();
        solutions.add(sol);

        let mut uv = Point2::new(sol.start.param(0)?, sol.start.param(1)?);

        let b = &self.base;
        let domain = *b.domain();
        let surface = b.surface();

        // Check with a looser tol, because we will check whether the cross-seam
        // point is a solution, and reject it if not.
        let domain_tol = 0.001 * domain.size().length();
        let (on_boundary, which_bound, _, _) =
            surface.is_on_boundary(k, &domain, uv, Some(domain_tol))?;
        if !on_boundary {
            return Ok(solutions); // can't be on a seam.
        }

        let mut other_sol = *sol;

        // Make two steppers, to evaluate and compare.
        let mut this_eval = SurfPtSolveStepper::new(
            k,
            surface,
            &domain,
            b.test_point(),
            uv,
            b.tight_tol(),
            b.loose_tol(),
            b.solver_op(),
        )?;
        let mut other_eval = SurfPtSolveStepper::copy_of(k, &this_eval);
        let tight_tol = b.tight_tol();

        // For comparison:
        let norm_type = if b.solver_op() == SolverOperation::Normalize {
            ErrorNormType::SumAbs
        } else {
            ErrorNormType::Gap
        };

        // Do U direction.
        if self.closed_surface(k, SurfParamType::U)?
            && (which_bound == SurfParamType::U || which_bound == SurfParamType::Both)
        {
            let dom = domain.u_interval();
            let (near_seam_u, far_seam_u) = if uv.x < dom.mid() {
                (dom.min(), dom.max())
            } else {
                (dom.max(), dom.min())
            };
            // If not quite on the seam, see whether right on is better.
            if (uv.x - near_seam_u).abs() > tight_tol {
                other_eval.set_uv(Point2::new(near_seam_u, uv.y));
                if other_eval.error_norm(norm_type)? <= this_eval.error_norm(norm_type)? {
                    uv.x = near_seam_u;
                    let gap_len = other_eval.gap_length()?;
                    if let Some(first) = solutions.get_mut(0) {
                        *first.start.param_mut(0)? = near_seam_u;
                        first.start.solution_value = gap_len;
                    }
                }
            }

            // Now check across the seam: is it a solution too?
            other_eval.set_uv(Point2::new(far_seam_u, uv.y));
            if other_eval.error_norm(norm_type)? < this_eval.error_norm(norm_type)? + EFF_ZERO_SQRT
            {
                *other_sol.start.param_mut(0)? = far_seam_u;
                *other_sol.start.param_mut(1)? = uv.y;
                other_sol.start.solution_value = other_eval.gap_length()?;
                other_sol.num_objects = 1;
                set_first_object(&mut other_sol, surface_obj);
                solutions.add_sorted_solution(&other_sol, SortKey::ByFirstTwoParameters)?;
            }
        }

        // Now do V direction.
        if self.closed_surface(k, SurfParamType::V)?
            && (which_bound == SurfParamType::V || which_bound == SurfParamType::Both)
        {
            let dom = domain.v_interval();
            let (near_seam_v, far_seam_v) = if uv.y < dom.mid() {
                (dom.min(), dom.max())
            } else {
                (dom.max(), dom.min())
            };
            // If not quite on the seam, see whether right on is better.
            if (uv.y - near_seam_v).abs() > tight_tol {
                other_eval.set_uv(Point2::new(uv.x, near_seam_v));
                if other_eval.error_norm(norm_type)? <= this_eval.error_norm(norm_type)? {
                    uv.y = near_seam_v;
                    let gap_len = other_eval.gap_length()?;
                    if let Some(first) = solutions.get_mut(0) {
                        *first.start.param_mut(1)? = near_seam_v;
                        first.start.solution_value = gap_len;
                    }
                }
            }

            // Now check across the seam: is it a solution too?
            other_eval.set_uv(Point2::new(uv.x, far_seam_v));
            if other_eval.error_norm(norm_type)? < this_eval.error_norm(norm_type)? + EFF_ZERO_SQRT
            {
                *other_sol.start.param_mut(0)? = uv.x;
                *other_sol.start.param_mut(1)? = far_seam_v;
                other_sol.start.solution_value = other_eval.gap_length()?;
                other_sol.num_objects = 1;
                set_first_object(&mut other_sol, surface_obj);
                solutions.add_sorted_solution(&other_sol, SortKey::ByFirstTwoParameters)?;
            }
        }

        // Finally, check both directions (torus).
        if self.closed_surface(k, SurfParamType::U)?
            && self.closed_surface(k, SurfParamType::V)?
            && which_bound == SurfParamType::Both
        {
            let dom = domain.u_interval();
            let far_seam_u = if uv.x < dom.mid() {
                dom.max()
            } else {
                dom.min()
            };
            let dom = domain.v_interval();
            let far_seam_v = if uv.y < dom.mid() {
                dom.max()
            } else {
                dom.min()
            };

            // Now check across both seams: is it a solution too?
            other_eval.set_uv(Point2::new(far_seam_u, far_seam_v));
            if other_eval.error_norm(norm_type)? < this_eval.error_norm(norm_type)? + EFF_ZERO_SQRT
            {
                *other_sol.start.param_mut(0)? = far_seam_u;
                *other_sol.start.param_mut(1)? = far_seam_v;
                other_sol.start.solution_value = other_eval.gap_length()?;
                solutions.add_sorted_solution(&other_sol, SortKey::ByFirstTwoParameters)?;
            }
        }

        Ok(solutions)
    }

    /// The main global solve (`Solve`): a rough guess, refined and handed to the
    /// local solver over each crease sub-domain (the whole domain: crease splitting is
    /// off upstream); a result that is the wrong kind of extremum (or not on the
    /// surface for intersect, or pushing a boundary for normalise) is retried from a
    /// grid-refined guess; the best solution is returned with its across-seam
    /// partners. Out-of-bounds evaluation is enabled for the solve. `surface_obj` is
    /// the caller's handle for the surface.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a rough guess; [`SolidError::Unported`]:
    /// `SmSurface::LocalPointSolve` (`surface.surface_p5`); those of the guesses and
    /// the tests.
    pub fn solve<O: SolutionObject>(
        &mut self,
        k: &Kernel,
        surface_obj: Option<O>,
    ) -> Result<SolutionArray<O>> {
        // We have to enable out-of-bounds evaluations.  Otherwise, the evaluators
        // will silently clamp, leading to wrong answers.
        with_out_of_bounds_enabled_in_place(self.base.surface(), || self.solve_body(k, surface_obj))
    }

    /// The body of [`Self::solve`], once out-of-bounds evaluation is enabled.
    fn solve_body<O: SolutionObject>(
        &mut self,
        k: &Kernel,
        surface_obj: Option<O>,
    ) -> Result<SolutionArray<O>> {
        let mut solutions = SolutionArray::new();

        // First find a rough guess.
        let mut sol: Solution<O> = Solution::new();
        sol.num_variables = 2;
        let mut found_answer = false;

        let rough = match self.find_rough_guess(k) {
            Ok(r) => r,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Err(SolidError::sm_err(OP_SOLVE)),
        };
        let (best_idx_u, best_idx_v) = (rough.best_idx_u, rough.best_idx_v);

        // Check for creases.  Note: this is currently not used.
        let domain_list = self.crease_subdomains()?;

        let mut sol_array: SolutionArray<O> = SolutionArray::new();
        let mut stat_ok = true;
        let op = self.base.solver_op();
        let loose_tol = self.base.loose_tol();
        let surface = self.base.surface();

        for this_domain in &domain_list {
            // Use only those guesses that are in or near this subdomain.
            if !this_domain.contains_point(rough.uv, EFF_ZERO_SQRT) {
                continue;
            }

            // Refine the guess (status ignored, as upstream).
            let mut uv_guess = rough.uv;
            if let Err(e) = self.refine_guess(k, &mut uv_guess) {
                if e.is_unported() {
                    return Err(e);
                }
            }

            // We have to check here for seams: start on the proper side.
            uv_guess = self.seams_status_ignored(k, uv_guess)?;

            // Do the local solve.
            let pt_for_local = if op == SolverOperation::Signed3dDirectedMaximize {
                -self.base.dir_vec()
            } else {
                self.base.test_point()
            };
            match local_point_solve(
                k,
                surface,
                surface_obj,
                this_domain,
                op,
                pt_for_local,
                uv_guess,
                &sol,
            ) {
                Ok((found, s)) => {
                    found_answer = found;
                    sol = s;
                    stat_ok = true;
                }
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => stat_ok = false,
            }

            // Double-check this solution; see whether we should try it again with a
            // more refined guess.
            let mut try_again = false;
            let mut surf_eval =
                SurfaceEval::with_uv(k, Some(surface), sol.start.param(0)?, sol.start.param(1)?);

            match op {
                SolverOperation::Minimize | SolverOperation::Signed3dDirectedMaximize => {
                    if found_answer && self.is_local_max(&mut surf_eval, loose_tol)? {
                        found_answer = false;
                    }
                    if !found_answer {
                        try_again = true;
                    }
                }
                SolverOperation::Maximize => {
                    if found_answer && self.is_local_min(&mut surf_eval, loose_tol)? {
                        found_answer = false;
                    }
                    if !found_answer {
                        try_again = true;
                    }
                }
                SolverOperation::Intersect => {
                    if found_answer && sol.start.solution_value > loose_tol {
                        found_answer = false;
                    }
                    if !found_answer
                        && !self.is_local_min(&mut surf_eval, loose_tol)?
                        && !self.pushing_boundary(&mut surf_eval, loose_tol)?.0
                    {
                        try_again = true;
                    }
                }
                // Note: Even if it's pushing a boundary, try again anyway.
                SolverOperation::Normalize if !found_answer => try_again = true,
                _ => {}
            }

            if try_again {
                let save_guess = uv_guess;
                match self.refine_guess_by_grid(k, uv_guess, best_idx_u, best_idx_v) {
                    Ok(refined) => {
                        uv_guess = refined;
                        // eStat is now RefineGuessByGrid's status, even if the
                        // guess did not move and the first local solve failed.
                        stat_ok = true;
                    }
                    Err(error) if error.is_unported() => return Err(error),
                    Err(_) => stat_ok = false,
                }

                if stat_ok && !uv_guess.closer_than(EFF_ZERO_SQRT, save_guess) {
                    match local_point_solve(
                        k,
                        surface,
                        surface_obj,
                        this_domain,
                        op,
                        pt_for_local,
                        uv_guess,
                        &sol,
                    ) {
                        Ok((found, s)) => {
                            found_answer = found;
                            sol = s;
                            stat_ok = true;
                            if found_answer {
                                surf_eval
                                    .set_uv(Point2::new(sol.start.param(0)?, sol.start.param(1)?));
                            }
                        }
                        Err(e) if e.is_unported() => return Err(e),
                        Err(_) => stat_ok = false,
                    }
                }
            }

            // check the final result to see whether it's really a solution.
            if found_answer {
                match op {
                    SolverOperation::Minimize | SolverOperation::Signed3dDirectedMaximize => {
                        if self.is_local_max(&mut surf_eval, loose_tol)? {
                            found_answer = false;
                        }
                    }
                    SolverOperation::Maximize => {
                        if self.is_local_min(&mut surf_eval, loose_tol)? {
                            found_answer = false;
                        }
                    }
                    SolverOperation::Intersect => {
                        if sol.start.solution_value > loose_tol {
                            found_answer = false;
                        }
                    }
                    SolverOperation::Normalize
                        if self.pushing_boundary(&mut surf_eval, loose_tol)?.0 =>
                    {
                        found_answer = false;
                    }
                    _ => {}
                }
            }

            if stat_ok && found_answer {
                if sol.num_variables == 0 {
                    sol.num_variables = 2;
                }
                sol_array.add(&sol);
            }
        }

        // Sort out multiple solutions.
        let mut best_sol_value = BIG_DOUBLE;
        for s in sol_array.as_slice() {
            if s.start.solution_value < best_sol_value {
                sol = *s;
                best_sol_value = sol.start.solution_value;
                found_answer = true;
            }
        }

        // If on a seam, add across-seam solutions.
        if stat_ok && found_answer {
            // TightTol: [080520]
            solutions = self.check_seams(k, self.base.tight_tol(), &sol, surface_obj)?;
        }

        Ok(solutions)
    }
}

/// `sOtherSol.m_apObjects[0] = m_pSrf`.
fn set_first_object<O: SolutionObject>(sol: &mut Solution<O>, surface_obj: Option<O>) {
    if let Some(slot) = sol.objects.first_mut() {
        *slot = surface_obj;
    }
}

/// A copy of `surface` with out-of-bounds evaluation enabled, when it is an
/// `SmBSplineSurface` (B-spline, plane, surface of revolution) without it; `None`
/// when nothing changes (`SmTemporaryChangeValue` on `GetOutOfBoundsEnabled()`).
pub(crate) fn with_out_of_bounds_enabled(surface: &Surface) -> Result<Option<Surface>> {
    let enabled = match surface.kind() {
        SurfaceKind::BSpline(b) => b.is_out_of_bounds_enabled(),
        SurfaceKind::Plane(p) => p.base().is_out_of_bounds_enabled(),
        SurfaceKind::Cone(s) => s.rev().base().is_out_of_bounds_enabled(),
        SurfaceKind::Sphere(s) => s.base().base().is_out_of_bounds_enabled(),
        SurfaceKind::Torus(s) => s.base().base().is_out_of_bounds_enabled(),
        SurfaceKind::SurfOfRevolution(s) => s.base().is_out_of_bounds_enabled(),
        SurfaceKind::SurfOfExtrusion(s) => s.base().is_out_of_bounds_enabled(),
        SurfaceKind::CurveBounded(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => {
            return Ok(None);
        }
    };
    if enabled {
        return Ok(None);
    }
    let mut copy = surface.clone_for_evaluation()?;
    match copy.kind_mut() {
        SurfaceKind::BSpline(b) => *b.out_of_bounds_enabled_mut() = true,
        SurfaceKind::Plane(p) => *p.out_of_bounds_enabled_mut() = true,
        SurfaceKind::Cone(s) => *s.rev.base_mut().out_of_bounds_enabled_mut() = true,
        SurfaceKind::Sphere(s) => *s.base.base_mut().out_of_bounds_enabled_mut() = true,
        SurfaceKind::Torus(s) => *s.base.base_mut().out_of_bounds_enabled_mut() = true,
        SurfaceKind::SurfOfRevolution(s) => *s.base_mut().out_of_bounds_enabled_mut() = true,
        SurfaceKind::SurfOfExtrusion(s) => *s.base_mut().out_of_bounds_enabled_mut() = true,
        SurfaceKind::CurveBounded(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => {}
    }
    Ok(Some(copy))
}

/// Runs `f` with out-of-bounds evaluation enabled on `surface` itself when it is an
/// `SmBSplineSurface` (B-spline, plane, surface of revolution) without it, and restores
/// the flag after (`SmTemporaryChangeValue` on `GetOutOfBoundsEnabled()`, in place as
/// upstream). The nesting check of [`with_out_of_bounds_enabled`]'s copy still runs, so
/// errors stay the same.
///
/// # Errors
///
/// Those of [`Surface::check_evaluation_copy_depth`] and of `f`.
pub(crate) fn with_out_of_bounds_enabled_in_place<R>(
    surface: &Surface,
    f: impl FnOnce() -> Result<R>,
) -> Result<R> {
    let Some(base) = out_of_bounds_base(surface) else {
        return f();
    };
    if base.is_out_of_bounds_enabled() {
        return f();
    }
    surface.check_evaluation_copy_depth(0)?;
    base.set_out_of_bounds_enabled_in_place(true);
    let result = f();
    base.set_out_of_bounds_enabled_in_place(false);
    // a NURB a cone, sphere, torus or surface of revolution made inside `f` is kept as
    // a made kind copied while the flag was set (`Surface::kind_with_nurb`); upstream
    // restores the flag on that one object, so restore it on the made kind too
    if let Some(made) = out_of_bounds_base(surface) {
        made.set_out_of_bounds_enabled_in_place(false);
    }
    result
}

/// The `SmBSplineSurface` part whose `m_bOutOfBoundsEnabled` an evaluation of `surface`
/// reads, `None` for the kinds not derived from it.
fn out_of_bounds_base(surface: &Surface) -> Option<&BSplineSurface> {
    match surface.kind() {
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
}

/// `pSurf->EvaluatePoint(uv, rPt)` with its status ignored: `prev` on failure.
fn eval_point(k: &Kernel, surface: &Surface, uv: Point2, prev: Point3) -> Result<Point3> {
    match surface.evaluate_point(k, uv) {
        Ok(p) => Ok(p),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(prev),
    }
}

/// `m_pSrf->LocalPointSolve(domain, op, point, guess, bFoundAnswer, sSol)`: whether
/// an answer was found and the solution.
#[allow(clippy::too_many_arguments)] // upstream's arguments plus the surface's object
fn local_point_solve<O: SolutionObject>(
    k: &Kernel,
    surface: &Surface,
    surface_obj: Option<O>,
    domain: &Extent2d,
    op: SolverOperation,
    point: Point3,
    guess: Point2,
    _sol: &Solution<O>,
) -> Result<(bool, Solution<O>)> {
    // (upstream's LocalPointSolve overwrites the solution with its own, default when
    // nothing was found)
    let r = surface.local_point_solve::<O>(k, domain, op, point, guess)?;
    let mut solution = r.solution;
    // LocalPointSolve_1 sets m_lNumObjects = 1 and m_apObjects[0] = this; the
    // LocalPointSolve_0 path (MAXIMIZE) leaves both at their defaults.
    if solution.num_objects > 0 {
        set_first_object(&mut solution, surface_obj);
    }
    Ok((r.found, solution))
}

// ---- SmSurfPtSolveStepper -----------------------------------------------------------------

impl SurfPtSolveStepper<'_> {
    /// Adjusts the step from `prev` to this stepper's UV (`AdjustStep`): limits each
    /// parameter's step to [`SurfPtSolveStepper::max_step`] of the domain, clips at
    /// the domain boundary (the better of the clamped and the proportionally scaled
    /// step), records boundary hits, and moves there. Returns whether the step was
    /// adjusted and the final step length as a fraction of the original.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when there is no step and the UV is outside the domain (it is clamped
    /// to the domain: upstream's outputs are `adjusted`, scale `0`); those of the
    /// evaluations.
    pub fn adjust_step(&mut self, k: &Kernel, prev: &Self) -> Result<(bool, f64)> {
        let mut adjusted = false;
        let mut scale = 1.0;
        let domain = *self.base.domain();

        let mut step = self.uv() - prev.uv();
        let orig_step_size = step.length();

        if orig_step_size < EFF_ZERO_SQ {
            if domain.contains_point(self.uv(), 0.0) {
                return Ok((adjusted, scale));
            }
            self.set_uv(domain.clamp_point(self.uv()));
            return Err(SolidError::sm_err(OP_ADJUST));
        }

        // Never take too big a step.
        // Scale the two parameters individually [ Fillet 2:234 ]
        let mut size_scale = 1.0;

        let max_step = self.max_step();

        let max_u_step = max_step * domain.u_interval().length();
        if step.x.abs() > max_u_step {
            adjusted = true;
            size_scale = max_u_step / step.x.abs();
            step.x *= size_scale;
        }

        let max_v_step = max_step * domain.v_interval().length();
        if step.y.abs() > max_v_step {
            adjusted = true;
            let scale_y = min2(max_v_step / step.y.abs(), size_scale);
            step.y *= scale_y;
            size_scale = min2(size_scale, scale_y);
        }

        // Apply the step and check out of bounds.
        let curr_uv = prev.uv();
        let next_uv = curr_uv + step;

        let bc = Self::check_bounds(curr_uv, next_uv, &domain);

        self.hit_bound(SurfParamType::U, bc.hits_u);
        self.hit_bound(SurfParamType::V, bc.hits_v);

        if bc.out_of_bounds {
            let clamped_uv = bc.clamped_uv.unwrap_or(next_uv);
            // Clamped.  Use the better of scaled and unscaled.
            if bc.step_scale > 1.0 - EFF_ZERO || bc.step_scale < EFF_ZERO {
                // both are the same, or scaled would be zero step.
                self.set_uv(clamped_uv);
            } else {
                // Evaluate both and use the better one.
                self.set_uv(clamped_uv);

                let mut scaled_eval = Self::copy_of(k, self);
                scaled_eval.set_uv(bc.clamped_uv_scaled.unwrap_or(next_uv));

                if scaled_eval.error_norm(ErrorNormType::SumAbs)?
                    < self.error_norm(ErrorNormType::SumAbs)?
                {
                    self.assign(&scaled_eval);
                    // (upstream then resets its local hit flags, which are not read
                    // again)
                }
            }
        } else {
            self.set_uv(next_uv);
        }

        if size_scale < 1.0 || bc.out_of_bounds {
            let final_step_size = (self.uv() - prev.uv()).length();
            scale = final_step_size / orig_step_size;
            adjusted = true;
        }

        Ok((adjusted, scale))
    }

    /// The measure of goodness `norm_type` of the evaluation (`ErrorNorm`): from the
    /// unitised-derivative check values (`F / |Su|`, `G / |Sv|`), or the gap length.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn error_norm(&mut self, norm_type: ErrorNormType) -> Result<f64> {
        let v = self.eval()?; // Make sure we're initialized.

        Ok(match norm_type {
            ErrorNormType::SumAbs => v.sum_check,
            ErrorNormType::Sum => v.f_check + v.g_check,
            ErrorNormType::F => v.f_check,
            ErrorNormType::G => v.g_check,
            ErrorNormType::FAbs => v.f_check.abs(),
            ErrorNormType::GAbs => v.g_check.abs(),
            ErrorNormType::Max => {
                if v.f_check.abs() >= v.g_check.abs() {
                    v.f_check
                } else {
                    v.g_check
                }
            }
            ErrorNormType::MaxAbs => {
                if v.f_check.abs() >= v.g_check.abs() {
                    v.f_check.abs()
                } else {
                    v.g_check.abs()
                }
            }
            ErrorNormType::Gap => v.gap_len,
        })
    }

    /// Whether the evaluation has converged to the tight (`use_tight_tol`) or loose
    /// tolerance (`Converged(SmBoolean)`).
    ///
    /// # Errors
    ///
    /// Those of [`Self::converged`].
    pub fn converged_with(&mut self, k: &Kernel, use_tight_tol: bool) -> Result<bool> {
        let tol = if use_tight_tol {
            self.base.tight_tol()
        } else {
            self.base.loose_tol()
        };
        self.converged(k, tol)
    }

    /// Whether the evaluation has converged within `tol` (`Converged(double)`): on
    /// the test point, or on the surface normal (by the unitised check values away
    /// from a singularity; at one, the normal lined up with the direction, or passing
    /// within `tol` (scaled by the distance) of the test point).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation and, at a singularity,
    /// `SmSurfaceEval::Normal` (`surface.surface_p4`).
    pub fn converged(&mut self, k: &Kernel, tol: f64) -> Result<bool> {
        self.eval()?;

        // If surface point equals test point, we're done.
        if self.error_norm(ErrorNormType::Gap)? < tol {
            return Ok(true);
        }

        // Check on the normal.  Not reliable for singularities however.
        if !self.is_singular()?.0 {
            return Ok(self.error_norm(ErrorNormType::SumAbs)? < tol);
        }

        // We're at a singularity.
        let norm = self.normal(k)?;

        if norm.length_squared() < EFF_ZERO_SQ {
            return Ok(false);
        }

        // For this case, converged is when the normals line up.
        if self.base.solver_op() == SolverOperation::Signed3dDirectedMaximize {
            let dir = self.base.dir_vec();
            let diff = if norm.dot(dir) > 0.0 {
                norm - dir
            } else {
                norm + dir
            };
            return Ok(diff.length_squared() <= tol * tol);
        }

        // All other cases: check distance from test pt to surface normal.
        let test_pt = self.base.test_point();
        let pos = self.pos()?;
        let line_t = line_closest_point(pos, norm, test_pt);

        let line_pnt = pos + line_t * norm;
        let diff = line_pnt - test_pt;

        // Scale by how far the point is from the surface.
        let mut dist_to_surf = line_pnt.distance(self.pos()?);
        if dist_to_surf < 1.0 {
            dist_to_surf = 1.0;
        }

        Ok(diff.length() < tol * dist_to_surf)
    }

    /// Whether the evaluation is near a singularity (`IsSingular`): one first
    /// derivative much smaller (by a factor 10000) than the other and below
    /// `SM_EFF_ZERO_PARAM` in squared length; which one ([`SurfParamType::U`],
    /// [`SurfParamType::V`] or [`SurfParamType::Neither`]).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn is_singular(&mut self) -> Result<(bool, SurfParamType)> {
        self.eval()?;

        let ratio_limit_squared = 10000.0 * 10000.0;
        let abs_mag_limit = EFF_ZERO_PARAM;

        let mut local_which = SurfParamType::Neither;

        let su = self.su()?;
        let sv = self.sv()?;
        let mag_su_sq = su.dot(su);
        let mag_sv_sq = sv.dot(sv);

        if mag_su_sq < mag_sv_sq / ratio_limit_squared && mag_su_sq < abs_mag_limit {
            local_which = SurfParamType::U;
        }

        if mag_sv_sq < mag_su_sq / ratio_limit_squared && mag_sv_sq < abs_mag_limit {
            local_which = SurfParamType::V;
        }

        Ok((local_which != SurfParamType::Neither, local_which))
    }

    /// Takes a Newton (or, with `do_plane_step`, tangent-plane) step from `prev`
    /// and moves this stepper there (`Step`), using unitised first derivatives and
    /// limiting the second-derivative terms on the concave side (marking the Newton
    /// step suspect). For the directed operation a plane step only gets a direction
    /// (a step of 5% of the domain). Near a singularity the point is inverted there.
    /// Returns [`TerminationReason::StillIterating`] (a step was taken),
    /// [`TerminationReason::FoundAnswerConverged`] (inverted at a singularity) or
    /// [`TerminationReason::UnableToConverge`] (no step).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluations and, near a singularity,
    /// `SmSurface::InvertPointNearSingularity`'s callees.
    pub fn step(
        &mut self,
        k: &Kernel,
        prev: &mut Self,
        do_plane_step: bool,
    ) -> Result<TerminationReason> {
        const SECOND_DERIV_TERM_LIMIT: f64 = 0.90;

        self.eval()?;
        prev.eval()?;

        let op = self.base.solver_op();

        // Special case: Plane step with directed operations:
        // Can get direction but not step size.
        if do_plane_step && op == SolverOperation::Signed3dDirectedMaximize {
            let mut step = vec_lin_comb_two_vectors(prev.su()?, prev.sv()?, -self.base.dir_vec())
                .unwrap_or(Vec2::new(0.0, 0.0));
            let mag = step.length();
            if mag > EFF_ZERO_SQRT / 100.0 {
                const STEP_SIZE_GUESS: f64 = 0.05;
                let domain_size = self.base.domain().size().length();
                step *= STEP_SIZE_GUESS * domain_size / mag;
            }
            self.set_uv(prev.uv() + step);

            return Ok(TerminationReason::StillIterating);
        }

        let mut got_step = false;

        // Check for near singularity: if one deriv is much smaller than the other.
        // Unfortunately, this isn't implemented for signed-directed case.
        if prev.is_singular()?.0 && op != SolverOperation::Signed3dDirectedMaximize {
            // Near singularity, use the surface's own domain. [B50]
            let surface = self.base.surface();
            let srf_domain = surface.natural_uv_domain_with_nurb(k)?;

            match surface.invert_point_near_singularity(
                k,
                self.base.test_point(),
                &srf_domain,
                prev.uv(),
                self.base.tight_tol(),
            ) {
                Ok(inverted) => {
                    self.set_uv(inverted.uv);
                    return Ok(TerminationReason::FoundAnswerConverged); // end iteration.
                }
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => {}
            }
        }

        // We use unitized first derivatives.
        let su = prev.su()?;
        let sv = prev.sv()?;
        let mu = su.length();
        let mv = sv.length();
        // We already checked for singularity, so this is unlikely.
        if mu < EFF_ZERO || mv < EFF_ZERO {
            return Ok(TerminationReason::UnableToConverge);
        }

        let su_n = su / mu;
        let sv_n = sv / mv;

        // Now the function values.
        let pv = prev.eval()?;
        let f = pv.gap.dot(su_n);
        let g = pv.gap.dot(sv_n);

        // The four derivatives of the unitized first derivatives.
        let suu = prev.suu()?;
        let suv = prev.suv()?;
        let svv = prev.svv()?;
        let su_nu = su.unitized_derivative(suu);
        let su_nv = su.unitized_derivative(suv);
        let sv_nu = sv.unitized_derivative(suv);
        let sv_nv = sv.unitized_derivative(svv);

        // Keep the two terms of Fu and Gv separate.
        let fu1 = pv.gap_u.dot(su_n);
        let mut fu2 = pv.gap.dot(su_nu);

        // Limit the magnitude of dFu2, if it's negative (see upstream's notes).

        if fu2 < -fu1 * SECOND_DERIV_TERM_LIMIT
            && (op == SolverOperation::Minimize
                || op == SolverOperation::Intersect
                || op == SolverOperation::Signed3dDirectedMaximize)
        {
            self.set_nr_suspect(true);
            fu2 = -fu1 * SECOND_DERIV_TERM_LIMIT;
        }

        let gv1 = pv.gap_v.dot(sv_n);
        let mut gv2 = pv.gap.dot(sv_nv);

        if gv2 < -gv1 * SECOND_DERIV_TERM_LIMIT
            && (op == SolverOperation::Minimize || op == SolverOperation::Intersect)
        {
            self.set_nr_suspect(true);
            gv2 = -gv1 * SECOND_DERIV_TERM_LIMIT;
        }

        let (fu, gv, fv, gu) = if do_plane_step {
            // planar step: ignore 2nd deriv terms.
            (fu1, gv1, pv.gap_v.dot(su_n), pv.gap_u.dot(sv_n))
        } else {
            // Newton step.
            (
                fu1 + fu2,
                gv1 + gv2,
                pv.gap_v.dot(su_n) + pv.gap.dot(su_nv),
                pv.gap_u.dot(sv_n) + pv.gap.dot(sv_nu),
            )
        };

        // 2x2 for the steps: Cramer's rule
        let det = fu * gv - gu * fv;
        let numer_u = -(f * gv - g * fv);
        let numer_v = -(fu * g - gu * f);

        if det.abs() <= EFF_ZERO * numer_u.abs() || det.abs() <= EFF_ZERO * numer_v.abs() {
            self.set_nr_suspect(true);
        } else {
            // Calculate the step.
            got_step = true;
            let step = Vec2::new(numer_u / det, numer_v / det);
            self.set_uv(prev.uv() + step);
        }

        Ok(if got_step {
            TerminationReason::StillIterating
        } else {
            TerminationReason::UnableToConverge
        })
    }

    /// Looks for a better point between `prev` and this stepper (`LineSearch`): a
    /// bracketed secant search on `F` or `G` when one dominates and changes sign;
    /// else samples at 10%, 20%, 40% and 70% of the way (returning the first that
    /// halves the error), and beyond either end when the error looks monotone, and
    /// moves to the best (clamped to the domain).
    ///
    /// # Errors
    ///
    /// Those of [`Self::line_search_bracketted`]; [`SolidError::Unported`] from the
    /// evaluations.
    pub fn line_search(&mut self, k: &Kernel, prev: &mut Self) -> Result<()> {
        const VALS: [f64; 4] = [0.1, 0.2, 0.4, 0.7]; // was: { 0.5, 0.2, 0.8, 0.4, 0.6, 0.1, 0.9 }

        // If one function value (F or G) is much bigger than the other, and it
        // changes sign, iterate on that one only.
        if prev.error_norm(ErrorNormType::F)? * self.error_norm(ErrorNormType::F)? < 0.0
            && prev.error_norm(ErrorNormType::FAbs)?
                > 10.0 * prev.error_norm(ErrorNormType::GAbs)?
            && self.error_norm(ErrorNormType::FAbs)?
                > 10.0 * self.error_norm(ErrorNormType::GAbs)?
        {
            return self.line_search_bracketted(k, prev, ErrorNormType::F);
        }
        if prev.error_norm(ErrorNormType::G)? * self.error_norm(ErrorNormType::G)? < 0.0
            && prev.error_norm(ErrorNormType::GAbs)?
                > 10.0 * prev.error_norm(ErrorNormType::FAbs)?
            && self.error_norm(ErrorNormType::GAbs)?
                > 10.0 * self.error_norm(ErrorNormType::FAbs)?
        {
            return self.line_search_bracketted(k, prev, ErrorNormType::G);
        }

        // Since we're likely in a problem area, use 3d gap for the error,
        // unless we're Normalizing or doing a directed operation.
        let op = self.base.solver_op();
        let norm_type = if op == SolverOperation::Normalize
            || op == SolverOperation::Signed3dDirectedMaximize
        {
            ErrorNormType::SumAbs
        } else {
            ErrorNormType::Gap
        };

        let mut third_eval = Self::copy_of(k, self); // init to this: bnds counts, etc.

        // Set dBestErr and dBestVal.
        let err0 = prev.error_norm(norm_type)?;
        let err1 = self.error_norm(norm_type)?;

        let mut best_err = err0;
        let mut best_val = 0.0;
        if err1 < best_err {
            best_err = err1;
            best_val = 1.0;
        }
        let best_init_err = best_err;

        for fraction in VALS {
            third_eval.set_uv((1.0 - fraction) * prev.uv() + fraction * self.uv());

            let this_err = third_eval.error_norm(norm_type)?;
            if this_err < best_err {
                // If good enough, return it, else record it and keep looking.
                if this_err < best_init_err * 0.50 {
                    self.assign(&third_eval);
                    return Ok(());
                }

                best_err = this_err;
                best_val = fraction;
            }
        }

        // If no minimum between the two inputs, try outside the range.
        if best_val < 0.01 && err0 < err1 {
            // Appears to be monotonically increasing.
            let fraction = -1.0;
            third_eval.set_uv((1.0 - fraction) * prev.uv() + fraction * self.uv());
            // Need to clamp this.
            third_eval.clamp();

            let this_err = third_eval.error_norm(norm_type)?;
            if this_err < best_err {
                best_err = this_err;
                best_val = fraction;
            }
        }

        if best_val > 0.99 && err1 < err0 {
            // Appears to be monotonically decreasing.
            let fraction = 2.0;
            third_eval.set_uv((1.0 - fraction) * prev.uv() + fraction * self.uv());
            third_eval.clamp();

            // (the best error is not read again)
            if third_eval.error_norm(norm_type)? < best_err {
                best_val = fraction;
            }
        }

        self.set_uv((1.0 - best_val) * prev.uv() + best_val * self.uv());

        self.clamp(); // In case not between the two inputs.

        Ok(())
    }

    /// A secant search for the zero of the signed norm `norm_type` between `prev`
    /// and this stepper, which bracket it (`LineSearchBracketted`): up to 5 secant
    /// steps (clamped to the domain), extended by 5 at a time up to 30 with a
    /// bisection when the value stops improving; moves to the last point.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a norm that cannot be negative, or when the two values do not
    /// bracket zero; [`SolidError::Unported`] from the evaluations.
    pub fn line_search_bracketted(
        &mut self,
        k: &Kernel,
        prev: &mut Self,
        norm_type: ErrorNormType,
    ) -> Result<()> {
        if matches!(
            norm_type,
            ErrorNormType::SumAbs
                | ErrorNormType::FAbs
                | ErrorNormType::GAbs
                | ErrorNormType::MaxAbs
                | ErrorNormType::Gap
        ) {
            return Err(SolidError::sm_err(OP_LINE_B));
        }

        let mut y0 = prev.error_norm(norm_type)?;
        let mut y1 = self.error_norm(norm_type)?;

        if y0 * y1 > 0.0 {
            return Err(SolidError::sm_err(OP_LINE_B));
        }

        let best_input_y = min2(y0.abs(), y1.abs());

        let mut max_iter = 5; // Don't spend too much time on a line search.

        // Call the initial points the unit interval:
        let mut x0 = 0.0;
        let mut x1 = 1.0;

        let mut mid_eval = Self::copy_of(k, prev);
        let domain = *self.base.domain();

        let mut ii = 0;
        while ii < max_iter {
            // The secant step: find the zero on the line between the evals.
            let mut x_mid = x0 - y0 * (x1 - x0) / (y1 - y0);

            mid_eval.set_uv((1.0 - x_mid) * prev.uv() + x_mid * self.uv());

            // Due to numerical noise the interpolation can leave the domain.  So:
            mid_eval.set_uv(domain.clamp_point(mid_eval.uv()));

            let mut y_mid = mid_eval.error_norm(norm_type)?;

            if y_mid.abs() < EFF_ZERO_SQRT {
                self.assign(&mid_eval);
                return Ok(());
            }
            // Another check: the fcn value can get worse with this method.
            // If it's not improving, try something different: a bisection.
            if ii >= max_iter - 1 && y_mid.abs() >= best_input_y {
                max_iter += 5;
                if max_iter > 30 {
                    break;
                }
                x_mid = (x0 + x1) / 2.0;

                mid_eval.set_uv((1.0 - x_mid) * prev.uv() + x_mid * self.uv());
                y_mid = mid_eval.error_norm(norm_type)?;
            }

            if y_mid * y0 < 0.0 {
                x1 = x_mid;
                y1 = y_mid;
            } else {
                x0 = x_mid;
                y0 = y_mid;
            }
            ii += 1;
        }

        self.assign(&mid_eval);

        Ok(())
    }

    /// Whether the evaluation is on a domain boundary and converged there
    /// (`ConvergedOnBoundary`): pushing the boundary (right-on counts), and
    /// perpendicular to the boundary curve within `tol` or pushing off its end at a
    /// corner; which boundary ([`SurfParamType::Neither`] when not pushing one).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn converged_on_boundary(&mut self, tol: f64) -> Result<(bool, SurfParamType)> {
        // Neg tol: if right on, do the check.
        let (pushing, ret_val) = self.pushing_boundary(-EFF_ZERO)?;
        if !pushing {
            return Ok((false, SurfParamType::Neither));
        }

        self.eval()?;
        let domain = *self.base.domain();

        if matches!(
            ret_val,
            SurfParamType::UMin | SurfParamType::UMax | SurfParamType::Both
        ) {
            if self.error_norm(ErrorNormType::GAbs)? < tol {
                return Ok((true, ret_val)); // perpendicular to boundary curve.
            }

            let param = self.uv().y;
            let crv_domain = domain.v_interval();
            if param < crv_domain.min() + tol {
                return Ok((self.error_norm(ErrorNormType::G)? > -tol, ret_val));
            } else if param > crv_domain.max() - tol {
                return Ok((self.error_norm(ErrorNormType::G)? < tol, ret_val));
            }
        }
        if matches!(
            ret_val,
            SurfParamType::VMin | SurfParamType::VMax | SurfParamType::Both
        ) {
            if self.error_norm(ErrorNormType::FAbs)? < tol {
                return Ok((true, ret_val)); // perpendicular to boundary curve.
            }

            let param = self.uv().x;
            let crv_domain = domain.u_interval();
            if param < crv_domain.min() + tol {
                return Ok((self.error_norm(ErrorNormType::F)? > -tol, ret_val));
            } else if param > crv_domain.max() - tol {
                return Ok((self.error_norm(ErrorNormType::F)? < tol, ret_val));
            }
        }

        Ok((false, ret_val))
    }

    /// Whether the UV is on a domain boundary and the plane step towards the test
    /// point leaves the domain there by more than `tol` (`PushingBoundary`; a
    /// negative `tol` makes a right-on point count), and which boundary:
    /// [`SurfParamType::UMin`], [`SurfParamType::UMax`], [`SurfParamType::VMin`],
    /// [`SurfParamType::VMax`], [`SurfParamType::Both`] (a corner) or
    /// [`SurfParamType::Neither`].
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn pushing_boundary(&mut self, tol: f64) -> Result<(bool, SurfParamType)> {
        let mut ret_val = SurfParamType::Neither;

        self.eval()?;

        let b = &self.base;
        let test_pt = b.test_point();
        let dir_vec = b.dir_vec();
        let directed = b.solver_op() == SolverOperation::Signed3dDirectedMaximize;
        let domain = *b.domain();

        let mut gap = test_pt - self.pos()?;
        if directed {
            let dot = gap.dot(dir_vec);
            gap = dot * dir_vec;
        }

        // Method: find the uv direction that the 3d step will take us,
        // and see whether it's heading out of bounds.
        let su = self.su()?;
        let sv = self.sv()?;
        let Ok(uv_step) = vec_lin_comb_two_vectors(su, sv, gap) else {
            return Ok((false, SurfParamType::Neither));
        };

        let bdry_tol = EFF_ZERO;
        let pushing_tol = tol;
        let uv = self.uv();

        if uv.x < domain.u_interval().min() + bdry_tol && uv_step.x < -pushing_tol {
            ret_val = SurfParamType::UMin;
        } else if uv.x > domain.u_interval().max() - bdry_tol && uv_step.x > pushing_tol {
            ret_val = SurfParamType::UMax;
        }

        if uv.y < domain.v_interval().min() + bdry_tol && uv_step.y < -pushing_tol {
            ret_val = if ret_val == SurfParamType::Neither {
                SurfParamType::VMin
            } else {
                SurfParamType::Both
            };
        } else if uv.y > domain.v_interval().max() - bdry_tol && uv_step.y > pushing_tol {
            ret_val = if ret_val == SurfParamType::Neither {
                SurfParamType::VMax
            } else {
                SurfParamType::Both
            };
        }

        Ok((ret_val != SurfParamType::Neither, ret_val))
    }

    /// When the UV is within `tol` of a domain boundary but not on it, snaps to the
    /// boundary (or boundaries) if that is no worse (`SnapToBoundaries`). Returns
    /// whether the stepper moved.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluations.
    pub fn snap_to_boundaries(&mut self, tol: f64) -> Result<bool> {
        let mut uv = self.uv();
        let domain = *self.base.domain();

        if !domain.is_point_on_boundary(uv, tol).0 {
            return Ok(false);
        }

        if domain.is_point_on_boundary(uv, 0.0).0 {
            return Ok(false);
        }

        // Ok, we're within dTol of a boundary but not exactly on it.
        let op = self.base.solver_op();
        let norm_type = if op == SolverOperation::Normalize
            || op == SolverOperation::Signed3dDirectedMaximize
        {
            ErrorNormType::SumAbs
        } else {
            ErrorNormType::Gap
        };
        let current_error = self.error_norm(norm_type)?;

        let save_uv = uv;

        if uv.x <= domain.min().x + tol {
            uv.x = domain.min().x;
        } else if uv.x >= domain.max().x - tol {
            uv.x = domain.max().x;
        }

        if uv.y <= domain.min().y + tol {
            uv.y = domain.min().y;
        } else if uv.y >= domain.max().y - tol {
            uv.y = domain.max().y;
        }

        self.set_uv(uv);

        let bdry_error = self.error_norm(norm_type)?;

        let mut ret_val = true;

        // If not better, restore original uv.
        if bdry_error > current_error {
            // If exactly equal, leave on bdry.
            self.set_uv(save_uv);
            ret_val = false;
        }

        Ok(ret_val)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::global_solver::SolutionRequested;
    use crate::geom::solution_array::ObjectKind;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Srf;

    impl SolutionObject for Srf {
        fn kind(&self) -> ObjectKind {
            ObjectKind::Surface
        }
        fn natural_uv_domain(&self) -> Option<Extent2d> {
            Some(unit_dom())
        }
    }

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

    fn stepper<'s>(
        k: &'s Kernel,
        s: &'s Surface,
        test: Point3,
        uv: Point2,
        op: SolverOperation,
    ) -> SurfPtSolveStepper<'s> {
        SurfPtSolveStepper::new(k, s, &unit_dom(), test, uv, 1e-12, 1e-6, op).unwrap()
    }

    #[test]
    fn rough_and_polygon_guesses() {
        let k = Kernel::default();
        let s = bilinear(0.0);
        let mut g = global(
            &k,
            &s,
            SolverOperation::Minimize,
            Point3::new(0.25, 0.75, 2.0),
        );
        let r = g.find_rough_guess(&k).unwrap();
        assert!((r.uv.x - 0.25).abs() < 1e-12 && (r.uv.y - 0.75).abs() < 1e-12);
        let p = g.find_polygon_guess(&k, (0, 0)).unwrap();
        assert!(!p.suspicious);
        assert_eq!(p.uv, r.uv);

        // a twisted bilinear surface with a test point far off its net: the grid
        // search still lands near the closest point
        let s = bilinear(0.8);
        let mut g = global(
            &k,
            &s,
            SolverOperation::Minimize,
            Point3::new(0.9, 0.9, 0.7),
        );
        let r = g.find_rough_guess(&k).unwrap();
        assert!(r.uv.x > 0.5 && r.uv.y > 0.5);
    }

    /// A rational strip of `runs` straight runs stacked 5 apart in z and joined end to end by
    /// turns (weight 0.7), like the turns of a coil (#1164). Along a run the control points are
    /// 20 to 40 apart, staggered from one run to the next, and the inner ones sit 1.5 above the run, so
    /// the control net is off the surface between knots. Returns the surface and its domain.
    fn stacked_runs_strip(runs: usize) -> (Surface, Extent2d) {
        use crate::nurbs::b_spline_curve::p1::KnotType;
        use crate::nurbs::b_spline_surface::p1::{BSplineSurfaceForm, CanonicalSurfaceData};
        let sag = 1.5;
        let mut path: Vec<(f64, f64, f64)> = Vec::new();
        for r in 0..runs {
            let z = 5.0 * r as f64;
            let xs: Vec<f64> = if r % 2 == 0 {
                vec![0.0, 40.0, 80.0, 120.0, 160.0]
            } else {
                vec![160.0, 140.0, 100.0, 60.0, 20.0, 0.0]
            };
            let n = xs.len();
            for (i, x) in xs.into_iter().enumerate() {
                let zz = if i > 0 && i + 1 < n { z + sag } else { z };
                path.push((x, zz, 1.0));
            }
            if r + 1 < runs {
                let xt = if r % 2 == 0 { 170.0 } else { -10.0 };
                path.push((xt, z + 2.5, 0.7));
            }
        }
        let n = path.len();
        let mut pts = Vec::new();
        let mut w = Vec::new();
        for &(x, z, wt) in &path {
            for y in [0.0, 1.0] {
                pts.push(Point3::new(x, y, z));
                w.push(wt);
            }
        }
        let spans = n - 2;
        let knots: Vec<f64> = (0..=spans).map(|i| i as f64).collect();
        let mut mult = vec![1usize; spans + 1];
        mult[0] = 3;
        mult[spans] = 3;
        let s = BSplineSurface::create_canonical(&CanonicalSurfaceData {
            u_degree: 2,
            v_degree: 1,
            control_points: &pts,
            form: BSplineSurfaceForm::Unspecified,
            u_multiplicities: &mult,
            v_multiplicities: &[2, 2],
            u_knots: &knots,
            v_knots: &[0.0, 1.0],
            knot_type: KnotType::Unspecified,
            weights: Some(&w),
            uv_domain: None,
        })
        .unwrap();
        (
            Surface::from(s),
            Extent2d::from_bounds(0.0, 0.0, spans as f64, 1.0),
        )
    }

    /// #1164: on a rational surface whose control points are farther apart than its neighbouring
    /// parts, the nearest control point is on the wrong run. Upstream drops the control-net
    /// triangle guess for it, and from what is left (that control point's guess and the grid
    /// search's) the solve finds nothing; with the fix the triangle guess is kept and the point is found.
    #[test]
    fn a_point_solve_finds_the_point_on_stacked_runs_with_the_fix() {
        let (s, dom) = stacked_runs_strip(4);
        let solve = |fixed: bool, target: Point3| {
            let mut k = Kernel::default();
            k.fixes.point_solve_keeps_triangle_guess = fixed;
            s.global_point_solve(
                &k,
                Some(Srf),
                &dom,
                SolverOperation::Intersect,
                target,
                1e-6,
                None,
                SolutionRequested::All,
            )
            .unwrap()
        };
        let at = |u: f64| {
            eval_point(&Kernel::default(), &s, Point2::new(u, 0.5), Point3::UNDEF).unwrap()
        };

        // a point on the first run, inside the second span
        let target = at(0.8395);
        assert_eq!(solve(false, target).len(), 0, "upstream finds the point");
        let found = solve(true, target);
        assert_eq!(found.len(), 1);
        let sol = found.get(0).unwrap();
        let uv = Point2::new(sol.start.param(0).unwrap(), sol.start.param(1).unwrap());
        assert!(
            eval_point(&Kernel::default(), &s, uv, Point3::UNDEF)
                .unwrap()
                .distance(target)
                < 1e-6,
            "{uv:?}"
        );

        // over the whole strip the fix never loses a point upstream finds, and finds some it
        // misses
        let mut gained = 0;
        for i in 0..400 {
            let target = at(dom.u_interval().max() * (f64::from(i) + 0.5) / 400.0);
            let (before, after) = (solve(false, target).len(), solve(true, target).len());
            assert!(after >= before, "lost the point at {target:?}");
            gained += usize::from(after > before);
        }
        assert!(gained > 0);
    }

    #[test]
    fn refinements() {
        let k = Kernel::default();
        let s = bilinear(0.0);
        let mut g = global(
            &k,
            &s,
            SolverOperation::Minimize,
            Point3::new(0.3, 0.6, 1.0),
        );
        // an interior point is not on a seam: the guess is refined in place
        let mut uv = Point2::new(0.5, 0.5);
        g.refine_guess(&k, &mut uv).unwrap();

        let mut best = BIG_DOUBLE;
        let mut guess = Point2::new(0.0, 0.0);
        g.refine_patch_by_grid(&k, &unit_dom(), 3, 1, &mut guess, &mut best)
            .unwrap();
        assert!((best - 1.0).abs() < 1e-12);

        let uv = g
            .refine_guess_by_grid(&k, Point2::new(0.9, 0.1), 1, 1)
            .unwrap();
        assert!(
            g.calc_func_val(&k, uv).unwrap() <= g.calc_func_val(&k, Point2::new(0.9, 0.1)).unwrap()
        );
    }

    #[test]
    fn seams_and_solve() {
        let k = Kernel::default();
        let s = bilinear(0.0);
        let mut g = global(
            &k,
            &s,
            SolverOperation::Minimize,
            Point3::new(0.0, 0.5, 0.0),
        );
        let mut sol: Solution<Srf> = Solution::new();
        sol.num_variables = 2;
        sol.num_objects = 1;
        sol.objects[0] = Some(Srf);
        sol.start.parameters[0] = 0.0;
        sol.start.parameters[1] = 0.5;
        // an open surface: just the solution
        let sols = g.check_seams(&k, 1e-12, &sol, Some(Srf)).unwrap();
        assert_eq!(sols.len(), 1);

        let solved = g.solve(&k, Some(Srf)).unwrap();
        assert_eq!(solved.len(), 1);
        let solution = solved.get(0).unwrap();
        assert_eq!(solution.start.parameters[0], 0.0);
        assert_eq!(solution.start.parameters[1], 0.5);
        assert_eq!(solution.start.solution_value, 0.0);
        assert_eq!(solution.objects[0], Some(Srf));
    }

    #[test]
    fn stepper_norms_and_tests() {
        let k = Kernel::default();
        let s = bilinear(0.0);
        let mut st = stepper(
            &k,
            &s,
            Point3::new(0.5, 0.25, 2.0),
            Point2::new(0.25, 0.5),
            SolverOperation::Minimize,
        );
        assert_eq!(st.error_norm(ErrorNormType::F).unwrap(), -0.25);
        assert_eq!(st.error_norm(ErrorNormType::G).unwrap(), 0.25);
        assert_eq!(st.error_norm(ErrorNormType::SumAbs).unwrap(), 0.5);
        assert_eq!(st.error_norm(ErrorNormType::Sum).unwrap(), 0.0);
        assert_eq!(st.error_norm(ErrorNormType::Max).unwrap(), -0.25);
        assert_eq!(st.error_norm(ErrorNormType::MaxAbs).unwrap(), 0.25);
        assert_eq!(st.is_singular().unwrap(), (false, SurfParamType::Neither));
        assert!(!st.converged(&k, 1e-6).unwrap());

        // a Newton step on a plane lands on the foot point
        let mut next = SurfPtSolveStepper::copy_of(&k, &st);
        assert_eq!(
            next.step(&Kernel::default(), &mut st, false).unwrap(),
            TerminationReason::StillIterating
        );
        assert!((next.uv().x - 0.5).abs() < 1e-12 && (next.uv().y - 0.25).abs() < 1e-12);
        assert!(next.converged(&k, 1e-6).unwrap());
        assert!(next.converged_with(&k, false).unwrap());

        // pushing a boundary
        let mut b = stepper(
            &k,
            &s,
            Point3::new(2.0, 0.5, 0.0),
            Point2::new(1.0, 0.5),
            SolverOperation::Minimize,
        );
        assert_eq!(
            b.pushing_boundary(1e-9).unwrap(),
            (true, SurfParamType::UMax)
        );
        assert_eq!(
            b.converged_on_boundary(1e-9).unwrap(),
            (true, SurfParamType::UMax)
        );

        // snapping
        let mut sn = stepper(
            &k,
            &s,
            Point3::new(1.5, 0.5, 0.0),
            Point2::new(1.0 - 1e-4, 0.5),
            SolverOperation::Minimize,
        );
        assert!(sn.snap_to_boundaries(1e-3).unwrap());
        assert_eq!(sn.uv(), Point2::new(1.0, 0.5));
        assert!(!sn.snap_to_boundaries(1e-3).unwrap());
    }

    #[test]
    fn adjust_and_line_search() {
        let k = Kernel::default();
        let s = bilinear(0.3);
        let test = Point3::new(0.5, 0.5, 1.0);
        let prev = stepper(
            &k,
            &s,
            test,
            Point2::new(0.5, 0.5),
            SolverOperation::Minimize,
        );
        let mut this = SurfPtSolveStepper::copy_of(&k, &prev);
        this.set_uv(Point2::new(1.5, 0.5));
        let (adjusted, scale) = this.adjust_step(&k, &prev).unwrap();
        assert!(adjusted);
        assert!((scale - 0.5).abs() < 1e-12);
        assert_eq!(this.uv(), Point2::new(1.0, 0.5));
        assert_eq!(this.bound_hits_u(), 1);

        // no step, outside: clamped, SM_ERR
        let mut out = SurfPtSolveStepper::copy_of(&k, &prev);
        out.set_uv(Point2::new(1.5, 0.5));
        let prev_out = SurfPtSolveStepper::copy_of(&k, &out);
        assert!(out.adjust_step(&k, &prev_out).is_err());
        assert_eq!(out.uv(), Point2::new(1.0, 0.5));

        let mut prev = stepper(
            &k,
            &s,
            test,
            Point2::new(0.1, 0.1),
            SolverOperation::Minimize,
        );
        let mut this = SurfPtSolveStepper::copy_of(&k, &prev);
        this.set_uv(Point2::new(0.9, 0.9));
        let before = this.error_norm(ErrorNormType::Gap).unwrap();
        this.line_search(&k, &mut prev).unwrap();
        assert!(this.error_norm(ErrorNormType::Gap).unwrap() <= before);

        assert!(this
            .line_search_bracketted(&k, &mut prev, ErrorNormType::Gap)
            .is_err());
    }

    fn oob(s: &Surface) -> bool {
        match s.kind() {
            SurfaceKind::BSpline(b) => b.is_out_of_bounds_enabled(),
            _ => panic!("not a B-spline"),
        }
    }

    #[test]
    fn out_of_bounds_in_place_is_scoped_and_restored_on_error() {
        let s = bilinear(0.0);
        assert!(!oob(&s));
        let r = with_out_of_bounds_enabled_in_place(&s, || {
            assert!(oob(&s));
            // a nested scope sees the flag set and leaves it set
            let inner = with_out_of_bounds_enabled_in_place(&s, || Ok(oob(&s)));
            assert_eq!(inner.ok(), Some(true));
            assert!(oob(&s));
            Err::<(), _>(SolidError::sm_err("test"))
        });
        assert!(r.is_err());
        assert!(!oob(&s));
    }
}
