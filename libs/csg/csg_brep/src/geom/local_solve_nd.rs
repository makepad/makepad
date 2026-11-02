//! Multi-variable Newton-Raphson solver for square systems of equations near a good
//! guess, with step cutting, interval clamping and boundary-hit exit criteria, plus
//! the finite-difference Jacobian helper used by equation sets.
//!
//! Upstream: `source/SMLib/inc/SmLocalSolveNd.h`, `source/SMLib/src/SmLocalSolveNd.cpp`
//! (enums `SmLocalSolverAlgorithmType` / `SmTerminationReasonType` from
//! `source/SMLib/inc/SmLocalSolve1d.h`).
//!
//! | Rust                                        | Upstream                                         |
//! |---------------------------------------------|--------------------------------------------------|
//! | [`EvalNFunctions`]                          | `SmEvalNFunctionsObject`                         |
//! | [`EvalNFunctions::solution_size`]           | `GetSolutionSize` / `m_dSolutionSize`            |
//! | [`EvalNFunctions::scaled_zero`]             | `GetScaledZero` / `m_dScaledZero`                |
//! | [`EvalNFunctions::is_converged`]            | `IsConverged`                                    |
//! | [`EvalNFunctions::has_intervals`]           | `HasIntervals`                                   |
//! | [`EvalNFunctions::set_intervals`]           | `SetIntervals`                                   |
//! | [`EvalNFunctions::evaluate`]                | `Evaluate`                                       |
//! | [`compute_jacobian`]                        | `SmEvalNFunctionsObject::ComputeJacobian`        |
//! | [`LocalSolveNd`]                            | `SmLocalSolveNd`                                 |
//! | [`LocalSolveNd::new`]                       | `SmLocalSolveNd(SmEvalNFunctionsObject&, const SmExtentNd*, const SmTArray<SmBoolean>*)` |
//! | [`Drop`] (implicit)                         | `~SmLocalSolveNd`                                |
//! | [`LocalSolveNd::intervals`]                 | `GetIntervals`                                   |
//! | [`LocalSolveNd::termination_reason`]        | `GetTerminationReason`                           |
//! | [`LocalSolveNd::current_iteration`]         | `m_lCurrentIteration`                            |
//! | [`LocalSolveNd::evaluator`]                 | `m_rFunctionEvaluator`                           |
//! | [`LocalSolveNd::set_step_cutting_enabled`]  | `SetStepCuttingEnabled`                          |
//! | [`LocalSolveNd::set_desired_accuracy`]      | `SetDesiredAccuracy`                             |
//! | [`LocalSolveNd::set_maximum_iterations`]    | `SetMaximumIterations`                           |
//! | [`LocalSolveNd::set_boundary_handler`]      | `SetBoundaryHandler`                             |
//! | [`LocalSolveNd::solve_it`]                  | `SolveIt`                                        |
//! | [`BoundaryHandler`]                         | `SmBoundaryHandlerType`                          |
//! | [`TerminationReason`]                       | `SmTerminationReasonType`                        |
//! | [`NdSolution`]                              | `SolveIt` out-params `rbFoundSolution`, `rSolutionVector` |
//!
//! Deviations:
//! - `SmEvalNFunctionsObject` is a trait; its data members `m_dSolutionSize` and
//!   `m_dScaledZero` become the accessors [`EvalNFunctions::solution_size`] (default
//!   `0.0`) and [`EvalNFunctions::scaled_zero`] (default `1.0`) that an equation set
//!   overrides when it tracks them. The base `Evaluate` / `SetIntervals` (which only
//!   return `SM_ERR`) are a required method and an `SM_ERR` default.
//! - `Evaluate` returns `Ok(found_answer)` instead of `SM_SUCCESS` plus the
//!   `rbFoundAnswer` out-param; any `Err` counts as a failed evaluation exactly where
//!   upstream tests `!= SM_SUCCESS`, except [`SolidError::Unported`], which always
//!   propagates (the solver cannot reproduce upstream past an unported callee). A
//!   failed evaluation leaves the solver's "found answer" flag unchanged (upstream: an
//!   evaluator that fails may or may not have written it). The function values are a
//!   fixed-size slice, so an evaluator cannot resize them.
//! - `SolveIt` returns [`NdSolution`] (found flag and solution vector) instead of
//!   out-params, and `SmStatus` errors as [`SolidError`]. It rejects interval /
//!   periodicity arrays whose size differs from the guess (`InvalidInput`; upstream
//!   documents "must be presized" and resizes its clamped vectors to the interval
//!   dimension). Only a linear solve failing with `SM_ERR` ends the iteration with
//!   [`TerminationReason::BadJacobian`] as upstream; its other errors (unreachable
//!   with an `N x N` Jacobian) are returned.
//! - `ComputeJacobian` is a free function over any [`EvalNFunctions`]; Jacobian
//!   writes outside the matrix return `InvalidInput` (upstream writes out of bounds).
//!   As upstream, on failure the evaluated parameter is left perturbed in `x`.
//! - `m_eLocalSolverType` (always `SM_SA_NEWTON`, never read), `m_pSolutionVector`
//!   (a pointer to the caller's output) and `m_dFoundAccuracy` (never written by
//!   `SolveIt`) are not kept. `SetAcceptableAccuracy` is not ported (`SolveIt`
//!   overwrites the value; no caller uses it).
//! - The `SM_DEBUG_CODE` trace output is not ported.

use crate::error::{Result, SolidError};
use crate::geom::extent1d::{Extent1d, ExtentNd};
use crate::geom::matrix::Matrix;
use crate::math::{BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT};

/// Upstream status `SM_ERR`.
const SM_ERR: i32 = 1001;

fn sm_err(operation: &'static str) -> SolidError {
    SolidError::Upstream {
        operation,
        code: SM_ERR,
        entity: None,
    }
}

/// How [`LocalSolveNd::solve_it`] treats next guesses that leave the parameter
/// intervals (`SmBoundaryHandlerType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryHandler {
    /// Stop when the parameters have stepped out of bounds (any combination, counting
    /// only large steps) more than a maximum number of times
    /// (`SM_BH_TOTAL_BOUNDARY_HITS`).
    TotalBoundaryHits,
    /// Stop when the next guess was out of every interval at once more than twice
    /// (`SM_BH_HITS_ALL_BOUNDARIES`).
    HitsAllBoundaries,
    /// Never stop because of boundary hits (`SM_BH_IGNORE_BOUNDARY_HITS`).
    IgnoreBoundaryHits,
}

/// Why a local solver stopped (`SmTerminationReasonType`, shared with the 1-D
/// solver of unit `geom.local_solve1d`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminationReason {
    /// Not set (`SM_TR_UNDEFINED`).
    Undefined,
    /// Converged to the desired accuracy (`SM_TR_FOUND_ANSWER_CONVERGED`).
    FoundAnswerConverged,
    /// Converged to the acceptable accuracy only (`SM_TR_FOUND_ANSWER_CLOSE`).
    FoundAnswerClose,
    /// Stopped by the boundary handler (`SM_TR_OUT_OF_BOUNDS`).
    OutOfBounds,
    /// The Jacobian system could not be solved (`SM_TR_BAD_JACOBIAN`).
    BadJacobian,
    /// No convergence (`SM_TR_UNABLE_TO_CONVERGE`).
    UnableToConverge,
    /// Still iterating (`SM_TR_STILL_ITERATING`).
    StillIterating,
}

impl TerminationReason {
    /// The upstream enum value (`SM_TR_UNDEFINED` = 0, ...).
    #[must_use]
    pub fn code(self) -> i32 {
        match self {
            Self::Undefined => 0,
            Self::FoundAnswerConverged => 1,
            Self::FoundAnswerClose => 2,
            Self::OutOfBounds => 3,
            Self::BadJacobian => 4,
            Self::UnableToConverge => 5,
            Self::StillIterating => 6,
        }
    }
}

/// A square system of equations `F(x) = 0` solved by [`LocalSolveNd`]
/// (`SmEvalNFunctionsObject`). The number of unknowns is the length of the guess
/// passed to [`LocalSolveNd::solve_it`].
pub trait EvalNFunctions {
    /// Quality of the last evaluated solution (`m_dSolutionSize`, default `0.0`).
    fn solution_size(&self) -> f64 {
        0.0
    }

    /// Convergence threshold for [`EvalNFunctions::solution_size`] (`m_dScaledZero`,
    /// default `1.0`).
    fn scaled_zero(&self) -> f64 {
        1.0
    }

    /// `solution_size < scaled_zero` (`IsConverged`).
    fn is_converged(&self) -> bool {
        self.solution_size() < self.scaled_zero()
    }

    /// True when the equation set wants the solver's intervals through
    /// [`EvalNFunctions::set_intervals`] (`HasIntervals`, default false).
    fn has_intervals(&self) -> bool {
        false
    }

    /// Receives the solver's intervals and periodicities before solving
    /// (`SetIntervals`; the default fails with `SM_ERR`).
    fn set_intervals(
        &mut self,
        _intervals: Option<&ExtentNd>,
        _periodicities: Option<&[bool]>,
    ) -> Result<()> {
        Err(sm_err(
            "geom::local_solve_nd::EvalNFunctions::set_intervals",
        ))
    }

    /// Evaluates the equations at `x` into `f` (same length as `x`) and, when
    /// `jacobian` is given, their partial derivatives (`jacobian[i][j] = dF_i/dx_j`).
    /// Returns true when the equation set itself decides `x` is a solution
    /// (`Evaluate`'s `rbFoundAnswer`). An `Err` is a failed evaluation.
    fn evaluate(&mut self, x: &[f64], f: &mut [f64], jacobian: Option<&mut Matrix>)
        -> Result<bool>;
}

/// Finite-difference Jacobian of `eval` at `x` with current values `f`, by central
/// differences of step `SM_EFF_ZERO_SQRT * (1 + |x_j|)`, clamped to the optional
/// per-parameter `intervals` (`SmEvalNFunctionsObject::ComputeJacobian`).
///
/// `x` is perturbed one parameter at a time and restored; the number of equations
/// is `f.len()`.
///
/// # Errors
///
/// A failed evaluation (returned as is, leaving that parameter perturbed), `SM_ERR`
/// when a clamped step collapses below `SM_EFF_ZERO`, and `InvalidInput` when `x`,
/// `intervals` or `jacobian` is smaller than `f`.
pub fn compute_jacobian<E: EvalNFunctions + ?Sized>(
    eval: &mut E,
    x: &mut [f64],
    f: &[f64],
    jacobian: &mut Matrix,
    intervals: Option<&[Extent1d]>,
) -> Result<()> {
    const OP: &str = "geom::local_solve_nd::compute_jacobian";
    let short = || SolidError::InvalidInput {
        operation: OP,
        reason: "parameter array shorter than the equation count",
    };
    let num_eqns = f.len();
    let mut f_lo = vec![0.0; num_eqns];
    let mut f_hi = vec![0.0; num_eqns];

    for jj in 0..num_eqns {
        let base_param = *x.get(jj).ok_or_else(short)?;
        let mut delta = EFF_ZERO_SQRT * (1.0 + base_param.abs());

        let mut lo_t = base_param - delta;
        let mut hi_t = base_param + delta;

        // Clamp to the domain if present.
        if let Some(ivls) = intervals {
            let ivl = ivls.get(jj).ok_or_else(short)?;
            lo_t = ivl.clamp_value(lo_t);
            hi_t = ivl.clamp_value(hi_t);
        }

        // evaluate all equations at low and high t -- no Jacobian
        set(x, jj, lo_t, OP)?;
        eval.evaluate(x, &mut f_lo, None)?;

        set(x, jj, hi_t, OP)?;
        eval.evaluate(x, &mut f_hi, None)?;

        delta = hi_t - lo_t; // use the actual difference, whatever happened

        if delta.abs() < EFF_ZERO {
            return Err(sm_err(OP));
        }

        // jj-th Jacobian column by first order central differences
        for (ii, (hi, lo)) in f_hi.iter().zip(&f_lo).enumerate() {
            jacobian.set_at(ii, jj, (hi - lo) / delta)?;
        }

        // restore the original value
        set(x, jj, base_param, OP)?;
    }
    Ok(())
}

fn set(v: &mut [f64], i: usize, value: f64, operation: &'static str) -> Result<()> {
    *v.get_mut(i).ok_or(SolidError::InvalidInput {
        operation,
        reason: "index out of range",
    })? = value;
    Ok(())
}

/// Result of [`LocalSolveNd::solve_it`].
#[derive(Debug, Clone, PartialEq)]
pub struct NdSolution {
    /// The solver accepted the solution (`rbFoundSolution`). See
    /// [`LocalSolveNd::termination_reason`] for why it stopped.
    pub found: bool,
    /// The final parameters (`rSolutionVector`), same length as the guess.
    pub x: Vec<f64>,
}

/// One evaluation outcome inside the solver: `Some(found)` on success, `None` on a
/// failed evaluation; `Unported` propagates.
fn eval_outcome(r: Result<bool>) -> Result<Option<bool>> {
    match r {
        Ok(found) => Ok(Some(found)),
        Err(e @ SolidError::Unported { .. }) => Err(e),
        Err(_) => Ok(None),
    }
}

fn sum_abs(v: &[f64]) -> f64 {
    let mut s = 0.0;
    for x in v {
        s += x.abs();
    }
    s
}

/// Newton-Raphson solver for an [`EvalNFunctions`] equation set (`SmLocalSolveNd`).
///
/// Construct with the equation set and optional bounds, adjust the behaviour with
/// the setters, then call [`LocalSolveNd::solve_it`] with a nearby guess.
pub struct LocalSolveNd<'a, E: EvalNFunctions + ?Sized> {
    evaluator: &'a mut E,
    desired_accuracy: f64,
    acceptable_accuracy: f64,
    maximum_iterations: u64,
    intervals: Option<&'a ExtentNd>,
    periodicities: Option<&'a [bool]>,
    boundary_handler: BoundaryHandler,
    max_boundary_hits: u64,
    step_cutting_enabled: bool,
    current_iteration: u64,
    termination_reason: TerminationReason,
}

impl<'a, E: EvalNFunctions + ?Sized> LocalSolveNd<'a, E> {
    /// A solver for `evaluator`, with optional parameter `intervals` (unbounded
    /// intervals do not clamp) and `periodicities` (`true`: the parameter wraps in
    /// its interval, else it is clamped), both sized like the guess. Defaults:
    /// desired accuracy `SM_EFF_ZERO / 10`, 100 iterations,
    /// [`BoundaryHandler::HitsAllBoundaries`] with 3 hits, no step cutting
    /// (`SmLocalSolveNd::SmLocalSolveNd`).
    pub fn new(
        evaluator: &'a mut E,
        intervals: Option<&'a ExtentNd>,
        periodicities: Option<&'a [bool]>,
    ) -> Self {
        Self {
            evaluator,
            desired_accuracy: EFF_ZERO / 10.0,
            acceptable_accuracy: EFF_ZERO,
            maximum_iterations: 100,
            intervals,
            periodicities,
            boundary_handler: BoundaryHandler::HitsAllBoundaries,
            max_boundary_hits: 3,
            step_cutting_enabled: false,
            current_iteration: 0,
            termination_reason: TerminationReason::Undefined,
        }
    }

    /// The parameter intervals (`GetIntervals`).
    #[must_use]
    pub fn intervals(&self) -> Option<&'a ExtentNd> {
        self.intervals
    }

    /// Why the last [`LocalSolveNd::solve_it`] stopped (`GetTerminationReason`).
    #[must_use]
    pub fn termination_reason(&self) -> TerminationReason {
        self.termination_reason
    }

    /// The iteration counter after the last solve (`m_lCurrentIteration`; equals
    /// the maximum when the iteration ran out, `0` when it never started).
    #[must_use]
    pub fn current_iteration(&self) -> u64 {
        self.current_iteration
    }

    /// The equation set (`m_rFunctionEvaluator`), e.g. to read its state after a
    /// solve.
    pub fn evaluator(&mut self) -> &mut E {
        self.evaluator
    }

    /// Enables cutting (and finally negating) the step when it makes the error
    /// worse (`SetStepCuttingEnabled`).
    pub fn set_step_cutting_enabled(&mut self, enabled: bool) {
        self.step_cutting_enabled = enabled;
    }

    /// Sets the tight stopping tolerance on the sum of `|F_i|`
    /// (`SetDesiredAccuracy`).
    pub fn set_desired_accuracy(&mut self, accuracy: f64) {
        self.desired_accuracy = accuracy;
    }

    /// Sets the iteration limit (`SetMaximumIterations`).
    pub fn set_maximum_iterations(&mut self, iterations: u64) {
        self.maximum_iterations = iterations;
    }

    /// Sets the out-of-bounds exit rule and its hit limit (`SetBoundaryHandler`;
    /// upstream's default limit is 3).
    pub fn set_boundary_handler(&mut self, handler: BoundaryHandler, max_boundary_hits: u64) {
        self.boundary_handler = handler;
        self.max_boundary_hits = max_boundary_hits;
    }

    fn clamp(&self, values: &[f64], out: &mut Vec<f64>, counts: &mut Vec<u64>) -> Result<bool> {
        match self.intervals {
            Some(ivls) => ivls.clamp_vector(values, self.periodicities, out, Some(counts)),
            None => Ok(false),
        }
    }

    fn done(&mut self, found: bool, reason: TerminationReason, x: Vec<f64>) -> NdSolution {
        self.termination_reason = reason;
        NdSolution { found, x }
    }

    /// Solves the equations starting at `guess` (its length is the problem size),
    /// accepting a solution whose sum of `|F_i|` is below `acceptable_accuracy` when
    /// the desired accuracy cannot be reached (`SmLocalSolveNd::SolveIt`).
    ///
    /// Not finding a solution is not an error: see [`NdSolution::found`] and
    /// [`LocalSolveNd::termination_reason`].
    ///
    /// # Errors
    ///
    /// A failing [`EvalNFunctions::set_intervals`], interval or periodicity arrays
    /// not sized like `guess` (`InvalidInput`), and `Unported` from the equation set.
    /// [`SolidError::InvalidInput`] when the Jacobian element count overflows,
    /// storage exceeds [`crate::limits::MAX_ALLOC_BYTES`], or the elements cannot
    /// be allocated (see [`Matrix::try_new`]).
    pub fn solve_it(&mut self, guess: &[f64], acceptable_accuracy: f64) -> Result<NdSolution> {
        const OP: &str = "geom::local_solve_nd::LocalSolveNd::solve_it";
        let invalid = |reason| SolidError::InvalidInput {
            operation: OP,
            reason,
        };
        // init output
        self.termination_reason = TerminationReason::UnableToConverge;

        let n = guess.len();
        if self.intervals.is_some_and(|iv| iv.dimension() != n) {
            return Err(invalid("interval dimension differs from the guess size"));
        }
        if self.periodicities.is_some_and(|p| p.len() != n) {
            return Err(invalid("periodicity count differs from the guess size"));
        }

        let mut jacobian = Matrix::try_new(n, n)?;
        let mut equation_values = vec![0.0; n];
        let mut next_guess = vec![0.0; n];
        let mut step_unclamped = vec![0.0; n];

        // for function evaluators that want to look at the current intervals
        if self.evaluator.has_intervals() {
            self.evaluator
                .set_intervals(self.intervals, self.periodicities)?;
        }

        // additional state for checking two kinds of clamping
        let mut jacobian2 = Matrix::try_new(n, n)?;
        let mut next_guess_clamped = vec![0.0; n];
        let mut next_guess_clamped_scaled = vec![0.0; n];
        let mut equation_values2 = vec![0.0; n];

        self.acceptable_accuracy = acceptable_accuracy;

        // the last guess point starts at the guess
        let mut solution = guess.to_vec();
        equation_values.fill(BIG_DOUBLE);

        // first evaluation: equation values and gradients at the guess
        let Some(mut found_answer) = eval_outcome(self.evaluator.evaluate(
            &solution,
            &mut equation_values,
            Some(&mut jacobian),
        ))?
        else {
            // exit case - the evaluation failed
            return Ok(self.done(true, TerminationReason::UnableToConverge, solution));
        };

        // exit case - the evaluator signalled a converged answer
        if found_answer {
            return Ok(self.done(true, TerminationReason::FoundAnswerConverged, solution));
        }

        // accumulate the errors
        let mut sum_f = sum_abs(&equation_values);

        // exit case - the initial guess was good enough
        if sum_f < self.desired_accuracy && self.evaluator.is_converged() {
            return Ok(self.done(true, TerminationReason::FoundAnswerConverged, solution));
        }

        // remember the best solution (upstream initialises it to SM_BIG_DOUBLE above)
        let mut best_sum_f = sum_f;
        let mut best_guess = guess.to_vec();

        let mut boundary_hits: u64 = 0;
        let mut hit_all_bounds_count: u64 = 0;
        let mut clamped = false;
        let mut out_of_bounds_counts: Vec<u64> = Vec::new();
        let mut next_sum_f = 0.0;

        self.current_iteration = 0;
        while self.current_iteration < self.maximum_iterations {
            // negate the right-hand side of Fn(p0, .., pn) = RHS
            for v in &mut equation_values {
                *v = -*v;
            }

            // parameter steps from the Jacobian system J * deltas = -F
            match jacobian.solve_linear_system(&equation_values) {
                Ok(s) => step_unclamped = s.x,
                Err(SolidError::Upstream { code: SM_ERR, .. }) => {
                    // exit case - the Jacobian system is not solvable
                    self.termination_reason = TerminationReason::BadJacobian;
                    break;
                }
                Err(e) => return Err(e),
            }

            // line search: find a next guess that reduces the sum of |F|
            let mut have_good_step = false;
            let mut step_scale = 1.0;
            while !have_good_step {
                let mut step_size = 0.0;
                have_good_step = true;

                // next guess and step size
                for ((ng, &step), &cur) in next_guess.iter_mut().zip(&step_unclamped).zip(&solution)
                {
                    let this_step = step_scale * step;
                    step_size += this_step.abs();
                    *ng = cur + this_step;
                }

                // exit condition - step size below the tight tolerance
                if step_size < self.desired_accuracy * self.desired_accuracy {
                    if sum_f < self.acceptable_accuracy {
                        // success - steps and function values are small
                        return Ok(self.done(true, TerminationReason::FoundAnswerClose, solution));
                    }
                    // failure - function values are not small
                    let reason = if clamped {
                        TerminationReason::OutOfBounds
                    } else {
                        TerminationReason::UnableToConverge
                    };
                    return Ok(self.done(false, reason, solution));
                }

                // clamp the next guess to the parameter bounds
                next_guess_clamped.clone_from(&next_guess);
                if self.intervals.is_some() {
                    clamped = self.clamp(
                        &next_guess,
                        &mut next_guess_clamped,
                        &mut out_of_bounds_counts,
                    )?;
                }

                // evaluate the equations at the next guess
                let mut first_failed = false;
                if let Some(found) = eval_outcome(self.evaluator.evaluate(
                    &next_guess_clamped,
                    &mut equation_values,
                    Some(&mut jacobian),
                ))? {
                    found_answer = found;
                } else {
                    // exit case - the evaluation failed
                    self.termination_reason = TerminationReason::UnableToConverge;
                    first_failed = true;
                    if !clamped {
                        break; // leave the line search; if clamped, try again
                    }
                }

                // accumulate the errors
                next_sum_f = sum_abs(&equation_values);

                // if clamped, compare the unscaled and scaled locations
                if clamped {
                    // scale the step by the most severe clamping
                    let mut scale_factor = 1.0;
                    for ((&ng, &ngc), &cur) in
                        next_guess.iter().zip(&next_guess_clamped).zip(&solution)
                    {
                        let base_step = ng - cur;
                        if base_step.abs() > EFF_ZERO {
                            let this_scale = (ngc - cur) / base_step;
                            if this_scale < scale_factor {
                                scale_factor = this_scale;
                            }
                        }
                    }

                    // clamped noticeably (not completely truncated)?
                    if scale_factor < 1.0 - EFF_ZERO && scale_factor > 0.0 {
                        // the clamped-and-scaled location
                        for ((ncs, &step), &cur) in next_guess_clamped_scaled
                            .iter_mut()
                            .zip(&step_unclamped)
                            .zip(&solution)
                        {
                            let this_step = step_scale * step;
                            *ncs = cur + scale_factor * this_step;
                        }

                        // fix round-off over the boundary by clamping again
                        if self.intervals.is_some() {
                            let raw = next_guess_clamped_scaled.clone();
                            self.clamp(
                                &raw,
                                &mut next_guess_clamped_scaled,
                                &mut out_of_bounds_counts,
                            )?;
                        }

                        // evaluate there
                        let second = eval_outcome(self.evaluator.evaluate(
                            &next_guess_clamped_scaled,
                            &mut equation_values2,
                            Some(&mut jacobian2),
                        ))?;
                        if second.is_none() && first_failed {
                            // exit case - both evaluations failed
                            self.termination_reason = TerminationReason::UnableToConverge;
                            break;
                        }

                        // and see how good it is
                        let next_sum_f2 = sum_abs(&equation_values2);

                        // take the scaled step if it is better (or the first failed)
                        if next_sum_f2 < next_sum_f || first_failed {
                            next_guess_clamped.clone_from(&next_guess_clamped_scaled);
                            equation_values.clone_from(&equation_values2);
                            next_sum_f = next_sum_f2;
                        }
                    }
                }

                // record the better clamped value
                next_guess.clone_from(&next_guess_clamped);

                // small enough: call it good
                found_answer |= next_sum_f < self.desired_accuracy;

                if found_answer {
                    // the next guess is the solution
                    solution.clone_from(&next_guess);
                    return Ok(self.done(true, TerminationReason::FoundAnswerConverged, solution));
                }

                // the error grew: try smaller steps, then the opposite direction
                if next_sum_f >= sum_f
                    && self.step_cutting_enabled
                    && sum_f > self.acceptable_accuracy
                    && step_scale.abs() > 0.0001
                {
                    step_scale /= 10.0;
                    // failing miserably: perhaps a delta has the wrong sign
                    if step_scale > 0.0 && step_scale < 0.001 {
                        step_scale = -1.0;
                    }
                    have_good_step = false;
                }
            }

            // stop when the error grows after 2 iterations but the last was good
            // enough (round-off noise after convergence)
            if self.current_iteration > 2
                && next_sum_f >= sum_f
                && sum_f < self.acceptable_accuracy / 100.0
            {
                self.termination_reason = TerminationReason::FoundAnswerClose;
                break;
            }

            // stop when the error stops changing after 2 iterations and might not be
            // good enough
            let change_factor = 0.000001;
            if self.current_iteration > 2
                && (next_sum_f - sum_f).abs() < sum_f * change_factor
                && sum_f > self.acceptable_accuracy / 100.0
            {
                self.termination_reason = TerminationReason::UnableToConverge;
                break;
            }

            // remember the best solution
            if next_sum_f < best_sum_f {
                best_sum_f = next_sum_f;
                best_guess.clone_from(&next_guess);
            }

            // keep the last solution as best when no better next one was found and
            // the last one is good enough
            if next_sum_f > sum_f && sum_f < self.acceptable_accuracy && sum_f < best_sum_f {
                best_sum_f = sum_f;
                best_guess.clone_from(&solution);
            }

            // set up the next iteration
            sum_f = next_sum_f;

            let mut step_size = 0.0;
            for (cur, &ng) in solution.iter_mut().zip(&next_guess) {
                step_size += (*cur - ng).abs();
                *cur = ng;
            }

            // exit when the step size is near zero - fail
            if step_size < EFF_ZERO_SQ {
                self.termination_reason = TerminationReason::UnableToConverge;
                break;
            }

            // out-of-bounds exit criteria
            if matches!(
                self.boundary_handler,
                BoundaryHandler::TotalBoundaryHits | BoundaryHandler::HitsAllBoundaries
            ) {
                let mut hits_all_boundaries = true;

                for (ii, count) in out_of_bounds_counts.iter_mut().enumerate() {
                    if *count != 0 {
                        *count = 0;
                        // count a hit only when the step is large compared to the
                        // parameter's range (.001 of the span)
                        let ext = self
                            .intervals
                            .and_then(|iv| iv.get(ii))
                            .ok_or(invalid("out-of-bounds count without an interval"))?;
                        let step = step_unclamped.get(ii).copied().unwrap_or(0.0);
                        if step.abs() > 1.0e-3 * ext.length() {
                            boundary_hits += 1;
                        }
                    } else {
                        hits_all_boundaries = false;
                        if self.boundary_handler == BoundaryHandler::HitsAllBoundaries {
                            break;
                        }
                    }
                }

                // count the times every parameter went out of bounds at once
                if hits_all_boundaries && !out_of_bounds_counts.is_empty() {
                    hit_all_bounds_count += 1;
                }

                // exit criteria - stepped out of bounds repeatedly
                if (self.boundary_handler == BoundaryHandler::TotalBoundaryHits
                    && boundary_hits > self.max_boundary_hits)
                    || (self.boundary_handler == BoundaryHandler::HitsAllBoundaries
                        && hit_all_bounds_count > 2)
                {
                    self.termination_reason = TerminationReason::OutOfBounds;
                    solution.clone_from(&next_guess);
                    break;
                }
            }

            self.current_iteration += 1;
        }

        // use the best solution seen
        if sum_f > best_sum_f && best_sum_f.abs() < BIG_DOUBLE {
            sum_f = best_sum_f;
            solution = best_guess;
            // upstream ignores this evaluation's status
            eval_outcome(self.evaluator.evaluate(
                &solution,
                &mut equation_values,
                Some(&mut jacobian),
            ))?;
        }

        // converged
        if sum_f < self.desired_accuracy {
            return Ok(self.done(true, TerminationReason::FoundAnswerConverged, solution));
        }

        // nearly converged
        if sum_f < self.acceptable_accuracy {
            return Ok(self.done(true, TerminationReason::FoundAnswerClose, solution));
        }

        // failed to converge; the termination reason was already set
        Ok(NdSolution {
            found: false,
            x: solution,
        })
    }
}

#[cfg(test)]
#[allow(clippy::needless_range_loop)]
mod tests {
    use super::*;

    /// `F_i = sum_j A_ij x_j + c_i x_i^3 - b_i`.
    struct Cubic {
        a: Vec<f64>,
        c: Vec<f64>,
        b: Vec<f64>,
        calls: usize,
        fd: bool,
    }

    impl EvalNFunctions for Cubic {
        fn evaluate(
            &mut self,
            x: &[f64],
            f: &mut [f64],
            jacobian: Option<&mut Matrix>,
        ) -> Result<bool> {
            self.calls += 1;
            let n = x.len();
            for i in 0..n {
                let mut v = -self.b[i];
                for j in 0..n {
                    v += self.a[i * n + j] * x[j];
                }
                v += self.c[i] * x[i] * x[i] * x[i];
                f[i] = v;
            }
            if let Some(jac) = jacobian {
                if self.fd {
                    let mut xx = x.to_vec();
                    let ff = f.to_vec();
                    compute_jacobian(self, &mut xx, &ff, jac, None)?;
                } else {
                    for i in 0..n {
                        for j in 0..n {
                            let mut d = self.a[i * n + j];
                            if i == j {
                                d += 3.0 * self.c[i] * x[i] * x[i];
                            }
                            jac.set_at(i, j, d)?;
                        }
                    }
                }
            }
            Ok(false)
        }
    }

    fn cubic(fd: bool) -> Cubic {
        Cubic {
            a: vec![4.0, 1.0, 1.0, 3.0],
            c: vec![0.5, 0.25],
            b: vec![1.0, 2.0],
            calls: 0,
            fd,
        }
    }

    #[test]
    fn oversized_jacobian_fails_before_evaluation() {
        let mut e = cubic(false);
        let mut s = LocalSolveNd::new(&mut e, None, None);
        let guess = vec![0.0; 65_536];
        assert!(matches!(
            s.solve_it(&guess, EFF_ZERO_SQRT),
            Err(SolidError::InvalidInput {
                operation: "geom::matrix::Matrix::try_new",
                reason: "matrix too large to allocate",
            })
        ));
        assert_eq!(s.evaluator().calls, 0);
    }

    #[test]
    fn converges_on_cubic_system() {
        for fd in [false, true] {
            let mut e = cubic(fd);
            let mut s = LocalSolveNd::new(&mut e, None, None);
            let r = s.solve_it(&[0.0, 0.0], EFF_ZERO_SQRT).unwrap();
            assert!(r.found);
            assert_eq!(
                s.termination_reason(),
                TerminationReason::FoundAnswerConverged
            );
            let mut f = [0.0; 2];
            cubic(false).evaluate(&r.x, &mut f, None).unwrap();
            assert!(f[0].abs() + f[1].abs() < 1e-9);
        }
    }

    #[test]
    fn tol_scaled_zero_of_equations() {
        assert_eq!(
            crate::geom::tol::scaled_zero_of_equations(&cubic(false)),
            1.0
        );
    }

    #[test]
    fn singular_jacobian_is_bad_jacobian() {
        let mut e = Cubic {
            a: vec![1.0, 1.0, 0.0, 0.0],
            c: vec![0.0, 0.0],
            b: vec![1.0, 2.0],
            calls: 0,
            fd: false,
        };
        let mut s = LocalSolveNd::new(&mut e, None, None);
        let r = s.solve_it(&[0.0, 0.0], EFF_ZERO_SQRT).unwrap();
        assert!(!r.found);
        assert_eq!(s.termination_reason(), TerminationReason::BadJacobian);
        assert_eq!(r.x, vec![0.0, 0.0]);
        assert_eq!(s.evaluator().calls, 1);
    }

    #[test]
    fn exact_guess_returns_immediately() {
        let mut e = Cubic {
            a: vec![1.0],
            c: vec![0.0],
            b: vec![0.0],
            calls: 0,
            fd: false,
        };
        let mut s = LocalSolveNd::new(&mut e, None, None);
        let r = s.solve_it(&[0.0], EFF_ZERO).unwrap();
        assert!(r.found);
        assert_eq!(
            s.termination_reason(),
            TerminationReason::FoundAnswerConverged
        );
        assert_eq!(s.current_iteration(), 0);
    }

    struct Failing;
    impl EvalNFunctions for Failing {
        fn evaluate(&mut self, _: &[f64], _: &mut [f64], _: Option<&mut Matrix>) -> Result<bool> {
            Err(sm_err("test"))
        }
    }

    #[test]
    fn failing_first_evaluation_reports_found() {
        let mut e = Failing;
        let mut s = LocalSolveNd::new(&mut e, None, None);
        let r = s.solve_it(&[1.0, 2.0], EFF_ZERO).unwrap();
        // upstream quirk: a failed first evaluation is reported as found
        assert!(r.found);
        assert_eq!(s.termination_reason(), TerminationReason::UnableToConverge);
        assert_eq!(r.x, vec![1.0, 2.0]);
    }

    struct Unported;
    impl EvalNFunctions for Unported {
        fn evaluate(&mut self, _: &[f64], _: &mut [f64], _: Option<&mut Matrix>) -> Result<bool> {
            Err(SolidError::Unported {
                upstream: "X",
                unit: "y",
            })
        }
    }

    #[test]
    fn unported_propagates() {
        let mut e = Unported;
        let mut s = LocalSolveNd::new(&mut e, None, None);
        assert!(matches!(
            s.solve_it(&[1.0], EFF_ZERO),
            Err(SolidError::Unported { .. })
        ));
    }

    #[test]
    fn set_intervals_default_fails() {
        struct WantsIntervals;
        impl EvalNFunctions for WantsIntervals {
            fn has_intervals(&self) -> bool {
                true
            }
            fn evaluate(
                &mut self,
                _: &[f64],
                f: &mut [f64],
                _: Option<&mut Matrix>,
            ) -> Result<bool> {
                f.fill(0.0);
                Ok(true)
            }
        }
        let mut e = WantsIntervals;
        let mut s = LocalSolveNd::new(&mut e, None, None);
        assert!(matches!(
            s.solve_it(&[1.0], EFF_ZERO),
            Err(SolidError::Upstream { code: SM_ERR, .. })
        ));
    }

    #[test]
    fn jacobian_by_central_differences() {
        let mut e = cubic(false);
        let x0 = [0.5, -1.0];
        let mut f = [0.0; 2];
        e.evaluate(&x0, &mut f, None).unwrap();
        let mut x = x0;
        let mut j = Matrix::new(2, 2);
        compute_jacobian(&mut e, &mut x, &f, &mut j, None).unwrap();
        assert_eq!(x, x0);
        assert!((j.at(0, 0).unwrap() - (4.0 + 1.5 * 0.25)).abs() < 1e-6);
        assert!((j.at(1, 1).unwrap() - (3.0 + 0.75)).abs() < 1e-6);
        assert!((j.at(0, 1).unwrap() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn jacobian_on_degenerate_interval_fails() {
        let mut e = cubic(false);
        let mut x = [0.5, -1.0];
        let f = [0.0; 2];
        let mut j = Matrix::new(2, 2);
        let ivls = [Extent1d::from_value(0.5), Extent1d::from_min_max(-2.0, 2.0)];
        let r = compute_jacobian(&mut e, &mut x, &f, &mut j, Some(&ivls));
        assert!(matches!(r, Err(SolidError::Upstream { code: SM_ERR, .. })));
        // as upstream, the parameter stays perturbed
        assert_eq!(x, [0.5, -1.0]);
    }
}
