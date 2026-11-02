//! Surface-point solving, part 3: boundary refinement and the local iteration driver.
//!
//! Upstream: `source/SMLib/src/SmSurfacePointSolve.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`Surface::global_point_solve_1`] | `SmSurface::GlobalPointSolve_1` |
//! | [`Surface::local_point_solve_1`] | `SmSurface::LocalPointSolve_1` |
//! | [`SurfPtSolveStepper::find_boundary_solution`] | `FindBoundarySolution` |
//! | [`SurfPtSolveStepper::check_crease`] | `CheckCrease` |
//! | [`SurfPtSolveStepper::find_next_step`] | `FindNextStep` |
//!
//! Deviations: out-parameters become result records; object pointers become caller
//! handles. Direct local solutions retain the default variable count; the existing
//! `LocalPointSolve` dispatcher sets it to two, as upstream does. Out-of-bounds
//! evaluation is enabled in place on the surface and restored after, as upstream's
//! `SmTemporaryChangeValue`. The nesting check of the former evaluation copy still
//! runs: it checks every owned geometry branch against the `MAX_GEOMETRY_NESTING` (32)
//! depth bound. `CheckCrease` returns immediately because upstream disables it with
//! `constexpr sbDoCreaseSubdomains = FALSE`. The local driver is iterative with
//! upstream's fixed bound of 35.

use super::p1::ErrorNormType;
use super::p2::with_out_of_bounds_enabled_in_place;
use super::{SurfPtGlobalSolver, SurfPtSolveStepper};
use crate::curve::curve::p2::CurvePropertyType;
use crate::curve::curve::p3::PointSolveOptions;
use crate::curve::curve::p5::local_property_analysis;
use crate::curve::curve::Curve;
use crate::curve::iso_curve::IsoCurve;
use crate::error::{Result, SolidError};
use crate::geom::extent2d::Extent2d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::local_solve_nd::TerminationReason;
use crate::geom::solution_array::{Solution, SolutionArray, SolutionObject, SolutionType};
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::Point3;
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::surface::surface::p1::zone_tol_3d;
use crate::surface::surface::Surface;

/// The iteration's status and termination reason (`FindNextStep`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NextStep {
    /// Zero for success, otherwise the upstream `SmStatus` code.
    pub status: i32,
    /// The reason, including when `status` reports a failure.
    pub reason: TerminationReason,
}

impl NextStep {
    const fn success(reason: TerminationReason) -> Self {
        Self { status: 0, reason }
    }

    const fn failure(status: i32, reason: TerminationReason) -> Self {
        Self { status, reason }
    }
}

/// A local point solve, including the best iterate when convergence was not attained.
#[derive(Debug, Clone)]
pub struct LocalPointSolve<O: Copy> {
    /// Upstream's `rbFoundSolution`, independent of the termination reason.
    pub found: bool,
    /// The best solution, with the caller's surface handle.
    pub solution: Solution<O>,
    /// Upstream's `cbi_Reason`.
    pub reason: TerminationReason,
    /// Upstream's `cbi_IterCount` (zero for the starting-guess shortcut).
    pub iterations: i32,
}

/// Ignore ordinary upstream statuses while always propagating missing dependencies.
fn boundary_or_false(value: Result<bool>) -> Result<bool> {
    match value {
        Ok(v) => Ok(v),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(false),
    }
}

impl SurfPtSolveStepper<'_> {
    /// Refines along an outward-pushed boundary (`FindBoundarySolution`).
    ///
    /// Returns whether the boundary point satisfies the outward optimality test.
    /// The updated UV and evaluation remain in this stepper even without a solution.
    ///
    /// # Errors
    ///
    /// Missing dependencies and surface evaluation failures.
    pub fn find_boundary_solution(&mut self, k: &Kernel) -> Result<bool> {
        let tight = self.base.tight_tol();
        let loose = self.base.loose_tol();
        let (pushing, mut bound) = self.pushing_boundary(-tight)?;
        if !pushing {
            return Ok(false);
        }
        self.snap_to_boundaries(loose)?;
        let domain = *self.base.domain();
        if bound == SurfParamType::Both {
            let (pushing, _) = self.pushing_boundary(tight)?;
            if pushing {
                return Ok(true);
            }
            bound = if self.uv().x < domain.u_interval().mid() {
                SurfParamType::UMin
            } else {
                SurfParamType::UMax
            };
        }

        let natural = self.base.surface().natural_uv_domain_with_nurb(k)?;
        let (param, iso, guess, iso_domain, cross_domain) = match bound {
            SurfParamType::UMin => (
                SurfParamType::U,
                natural.u_interval().clamp_value(domain.min().x),
                self.uv().y,
                domain.v_interval(),
                domain.u_interval(),
            ),
            SurfParamType::UMax => (
                SurfParamType::U,
                natural.u_interval().clamp_value(domain.max().x),
                self.uv().y,
                domain.v_interval(),
                domain.u_interval(),
            ),
            SurfParamType::VMin => (
                SurfParamType::V,
                natural.v_interval().clamp_value(domain.min().y),
                self.uv().x,
                domain.u_interval(),
                domain.v_interval(),
            ),
            SurfParamType::VMax => (
                SurfParamType::V,
                natural.v_interval().clamp_value(domain.max().y),
                self.uv().x,
                domain.u_interval(),
                domain.v_interval(),
            ),
            _ => return Ok(false),
        };
        let curve = Curve::from(IsoCurve::from_surface(
            k,
            self.base.surface().clone_for_evaluation()?,
            param,
            iso,
            iso < cross_domain.mid(),
            true,
        )?);
        let mut sol: Solution<()> = Solution::new();
        let found = if self.base.solver_op() == SolverOperation::Signed3dDirectedMaximize {
            boundary_or_false(local_property_analysis(
                k,
                &curve,
                &iso_domain,
                CurvePropertyType::PerpendicularToVector,
                guess,
                None,
                &[self.base.dir_vec()],
                &mut sol,
            ))?
        } else {
            match curve.local_point_solve(
                k,
                &iso_domain,
                SolverOperation::Minimize,
                self.base.test_point(),
                &PointSolveOptions::default(),
                guess,
            ) {
                Ok((found, solution)) => {
                    sol = solution;
                    found
                }
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => false,
            }
        };
        if !found {
            return Ok(false);
        }
        let new_param = sol.start.param(0)?;
        let mut new_uv = self.uv();
        let mut boundary = Self::copy_of(k, self);
        if param == SurfParamType::U {
            new_uv.y = new_param;
        } else {
            new_uv.x = new_param;
        }
        boundary.set_uv(new_uv);
        let (deriv1, deriv2, norm) = if param == SurfParamType::U {
            (boundary.su()?, boundary.suu()?, ErrorNormType::GAbs)
        } else {
            (boundary.sv()?, boundary.svv()?, ErrorNormType::FAbs)
        };
        if boundary.error_norm(norm)? < self.error_norm(norm)? {
            self.assign(&boundary);
        }
        let flip = if self.base.solver_op() == SolverOperation::Maximize {
            true
        } else if self.base.solver_op() == SolverOperation::Normalize {
            let term1 = deriv1.dot(deriv1);
            let term2 = self.gap()?.dot(deriv2);
            term1 + term2 < 0.0
        } else {
            false
        };
        let mut error = self.gap()?.dot(deriv1);
        if flip {
            error = -error;
        }
        Ok(if iso < cross_domain.mid() {
            error > -tight
        } else {
            error < tight
        })
    }

    /// The disabled crease check (`CheckCrease`): `(processed, found)` is `(false, false)`.
    pub fn check_crease(&mut self, _previous: &mut Self) -> (bool, bool) {
        (false, false)
    }

    /// Chooses the next Newton, tangent-plane or line-search step (`FindNextStep`).
    ///
    /// # Errors
    ///
    /// Missing dependencies and evaluation errors. Ordinary iteration failure is
    /// represented by [`NextStep::status`] so its reason is retained.
    pub fn find_next_step(&mut self, k: &Kernel, previous: &mut Self) -> Result<NextStep> {
        use TerminationReason::{
            FoundAnswerConverged as Converged, OutOfBounds, StillIterating, UnableToConverge,
        };
        let tight = self.base.tight_tol();
        let (_, bound) = self.pushing_boundary(tight)?;
        if self.iter_count() > 0 || bound != SurfParamType::Neither {
            if bound == SurfParamType::Both {
                return Ok(NextStep::success(OutOfBounds));
            }
            if boundary_or_false(self.find_boundary_solution(k))? {
                return Ok(NextStep::success(if self.pushing_boundary(tight)?.0 {
                    OutOfBounds
                } else {
                    Converged
                }));
            }
        }
        self.set_nr_suspect(false);
        let mut nr = Self::copy_of(k, previous);
        let mut reason = nr.step(k, previous, false)?;
        if reason == Converged {
            self.assign(&nr);
            return Ok(NextStep::success(reason));
        }
        let mut got_nr = reason == StillIterating;
        if got_nr {
            match nr.adjust_step(k, previous) {
                Ok((adjusted, scale)) => {
                    if nr.converged_with(k, true)? {
                        self.assign(&nr);
                        return Ok(NextStep::success(reason));
                    }
                    if adjusted && scale < EFF_ZERO_SQ {
                        self.set_nr_suspect(true);
                    }
                }
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => {
                    self.set_nr_suspect(true);
                    got_nr = false;
                }
            }
        } else {
            self.set_nr_suspect(true);
        }
        let mut suspect = self.nr_step_suspect() || nr.nr_step_suspect();
        if !suspect {
            let step = nr.uv() - self.uv();
            let factor = self.max_step() / 2.0;
            let domain = self.base.domain();
            if step.x.abs() > factor * domain.u_interval().length()
                || step.y.abs() > factor * domain.v_interval().length()
            {
                suspect = true;
            }
        }
        if !suspect
            && nr.error_norm(ErrorNormType::SumAbs)?
                > previous.error_norm(ErrorNormType::SumAbs)? * 0.75
            && nr.error_norm(ErrorNormType::SumAbs)? > self.base.loose_tol()
        {
            suspect = true;
        }
        self.set_nr_suspect(suspect);
        if got_nr && !self.nr_step_suspect() {
            self.assign(&nr);
            return Ok(NextStep::success(reason));
        }
        self.set_nr_suspect(true);
        let mut plane = Self::copy_of(k, previous);
        reason = plane.step(k, previous, true)?;
        if reason == Converged {
            // Upstream deliberately assigns sNREval here, even after the plane step.
            self.assign(&nr);
            return Ok(NextStep::success(reason));
        }
        let mut got_plane = reason == StillIterating;
        if got_plane {
            match plane.adjust_step(k, previous) {
                Ok(_) => {
                    if plane.converged_with(k, true)? {
                        self.assign(&plane);
                        return Ok(NextStep::success(reason));
                    }
                }
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => got_plane = false,
            }
        }
        if !got_nr && !got_plane {
            self.assign(previous);
            return Ok(if nr.bound_hits_u() != 0 || nr.bound_hits_v() != 0 {
                NextStep::success(OutOfBounds)
            } else {
                NextStep::failure(1022, UnableToConverge)
            });
        }
        let norm = if self.base.solver_op() == SolverOperation::Normalize {
            ErrorNormType::SumAbs
        } else {
            ErrorNormType::Gap
        };
        if got_plane
            && boundary_or_false(plane.find_boundary_solution(k))?
            && plane.error_norm(norm)? <= previous.error_norm(norm)?
            && (!got_nr || plane.error_norm(norm)? <= nr.error_norm(norm)?)
        {
            self.assign(&plane);
            return Ok(if self.pushing_boundary(tight)?.0 {
                NextStep::failure(1020, OutOfBounds)
            } else {
                NextStep::success(Converged)
            });
        }
        // Both CheckCrease calls in upstream are disabled unconditionally.
        if got_nr
            && boundary_or_false(nr.find_boundary_solution(k))?
            && nr.error_norm(norm)? < previous.error_norm(norm)? + EFF_ZERO
            && (!got_plane || nr.error_norm(norm)? < plane.error_norm(norm)? + EFF_ZERO)
        {
            self.assign(&nr);
            return Ok(if self.pushing_boundary(tight)?.0 {
                NextStep::failure(1020, OutOfBounds)
            } else {
                NextStep::success(Converged)
            });
        }
        if got_nr {
            match nr.line_search(k, previous) {
                Err(e) if e.is_unported() => return Err(e),
                _ => {}
            }
        }
        if got_plane {
            match plane.line_search(k, previous) {
                Err(e) if e.is_unported() => return Err(e),
                _ => {}
            }
        }
        if got_nr && (!got_plane || nr.error_norm(norm)? < plane.error_norm(norm)?) {
            self.assign(&nr);
        } else {
            self.assign(&plane);
        }
        if self.bound_hits_u().unsigned_abs() > 3 || self.bound_hits_v().unsigned_abs() > 3 {
            return Ok(NextStep::success(OutOfBounds));
        }
        if self.error_norm(norm)? >= previous.error_norm(norm)?
            || (self.uv() - previous.uv()).length() < EFF_ZERO_SQ
        {
            return Ok(
                if self.bound_hits_u().unsigned_abs() > 1 || self.bound_hits_v().unsigned_abs() > 1
                {
                    NextStep::success(OutOfBounds)
                } else {
                    NextStep::failure(1022, UnableToConverge)
                },
            );
        }
        Ok(NextStep::success(reason))
    }
}

impl Surface {
    /// The new global surface-point solve (`GlobalPointSolve_1`).
    ///
    /// # Errors
    ///
    /// Those of [`SurfPtGlobalSolver::new`] and [`SurfPtGlobalSolver::solve`].
    pub fn global_point_solve_1<O: SolutionObject>(
        &self,
        k: &Kernel,
        domain: &Extent2d,
        op: SolverOperation,
        point: Point3,
        distance_tolerance: f64,
        target_distance: Option<f64>,
        requested: SolutionRequested,
        surface_object: Option<O>,
    ) -> Result<SolutionArray<O>> {
        SurfPtGlobalSolver::new(
            k,
            self,
            domain,
            op,
            point,
            distance_tolerance,
            target_distance,
            requested,
        )?
        .solve(k, surface_object)
    }

    /// The new bounded local surface-point solve (`LocalPointSolve_1`).
    ///
    /// The initial guess is clamped and at most 35 steps are taken. Upstream reports
    /// a best point as found for minimization/maximization even without convergence.
    ///
    /// # Errors
    ///
    /// Missing callees and evaluation failures.
    pub fn local_point_solve_1<O: Copy>(
        &self,
        k: &Kernel,
        domain: &Extent2d,
        op: SolverOperation,
        point: Point3,
        guess: Point2,
        surface_object: Option<O>,
    ) -> Result<LocalPointSolve<O>> {
        with_out_of_bounds_enabled_in_place(self, || {
            self.local_point_solve_1_body(k, domain, op, point, guess, surface_object)
        })
    }

    fn local_point_solve_1_body<O: Copy>(
        &self,
        k: &Kernel,
        domain: &Extent2d,
        op: SolverOperation,
        point: Point3,
        guess: Point2,
        surface_object: Option<O>,
    ) -> Result<LocalPointSolve<O>> {
        use TerminationReason::{
            FoundAnswerConverged as Converged, OutOfBounds, StillIterating, UnableToConverge,
        };
        let guess = domain.clamp_point(guess);
        let tight = EFF_ZERO * (1.0 + point.max_dimension());
        let loose = if self.owner().is_some() {
            zone_tol_3d(k, self)?
        } else if op == SolverOperation::Intersect {
            0.01
        } else {
            EFF_ZERO_SQRT * 100.0
        };
        let mut curr = SurfPtSolveStepper::new(k, self, domain, point, guess, tight, loose, op)?;
        let mut solution = Solution::new();
        solution.solution_type = SolutionType::SingleValue;
        solution.num_objects = 1;
        if let Some(slot) = solution.objects.first_mut() {
            *slot = surface_object;
        }
        *solution.start.param_mut(0)? = guess.x;
        *solution.start.param_mut(1)? = guess.y;
        solution.start.solution_value = curr.gap_length()?;
        if curr.converged(k, EFF_ZERO / 1000.0)?
            && (!curr.is_singular()?.0 || curr.gap_length()? < tight)
        {
            return Ok(LocalPointSolve {
                found: true,
                solution,
                reason: Converged,
                iterations: 0,
            });
        }
        let mut next = SurfPtSolveStepper::copy_of(k, &curr);
        let mut reason = StillIterating;
        let mut iterations = 0;
        while iterations < 35 {
            let step = next.find_next_step(k, &mut curr)?;
            reason = step.reason;
            if next.converged_with(k, true)? {
                reason = Converged;
                break;
            }
            let (boundary, mut which) = next.converged_on_boundary(tight)?;
            if boundary {
                next.snap_to_boundaries(loose)?;
                if next.gap_length()? > loose {
                    let uv = next.uv();
                    let seam = self.is_on_seam(k, uv, None)?;
                    which = match which {
                        SurfParamType::VMin | SurfParamType::VMax => SurfParamType::V,
                        SurfParamType::UMin | SurfParamType::UMax => SurfParamType::U,
                        _ => which,
                    };
                    let mut new_uv = uv;
                    if seam.on_seam {
                        let index = if seam.seam_dir != SurfParamType::Both
                            && (seam.seam_dir == which || which == SurfParamType::Both)
                        {
                            Some(0)
                        } else if seam.seam_dir != SurfParamType::Both && seam.seam_dir != which {
                            None
                        } else if which != SurfParamType::Both {
                            Some(which as usize)
                        } else {
                            Some(2)
                        };
                        if let Some(index) = index {
                            new_uv = seam.cross_seam_uvs.get(index).copied().ok_or(
                                SolidError::invalid_input(
                                    "surface::surface_point_solve::p3::local_point_solve_1",
                                    "missing across-seam point",
                                ),
                            )?;
                        }
                    }
                    // SmVector2d::operator!= scales its tolerance from the left operand.
                    if !new_uv.approx_ne(uv) {
                        reason = OutOfBounds;
                        break;
                    }
                    next.set_uv(new_uv);
                } else {
                    reason = OutOfBounds;
                    break;
                }
            }

            if step.status != 0 || reason != StillIterating {
                break;
            }
            curr.assign(&next);
            curr.set_iter_count(iterations + 1);
            iterations += 1;
        }
        *solution.start.param_mut(0)? = next.uv().x;
        *solution.start.param_mut(1)? = next.uv().y;
        solution.start.solution_value =
            next.error_norm(if op == SolverOperation::Signed3dDirectedMaximize {
                ErrorNormType::SumAbs
            } else {
                ErrorNormType::Gap
            })?;
        let mut found = next.converged_with(k, false)?;
        if !found {
            if matches!(
                op,
                SolverOperation::Minimize
                    | SolverOperation::Maximize
                    | SolverOperation::Signed3dDirectedMaximize
            ) {
                found = true;
            }
            if iterations >= 35 && reason != OutOfBounds {
                reason = UnableToConverge;
            }
        }
        Ok(LocalPointSolve {
            found,
            solution,
            reason,
            iterations,
        })
    }
}
