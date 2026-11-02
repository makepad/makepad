//! One-dimensional local root finding (damped Newton iteration with oscillation
//! detection, regula-falsi bisection fallback) and Brent-like minimisation /
//! maximisation, driven by a caller-supplied function evaluator.
//!
//! Upstream: `source/SMLib/inc/SmLocalSolve1d.h`, `source/SMLib/src/SmLocalSolve1d.cpp`.
//!
//! | Rust                                         | Upstream                                       |
//! |----------------------------------------------|------------------------------------------------|
//! | [`SolverAlgorithm`]                          | `SmLocalSolverAlgorithmType`                   |
//! | [`TerminationReason`] (re-export)            | `SmTerminationReasonType` (defined in `geom::local_solve_nd`) |
//! | [`EvalFunction`], [`FunctionValue`]          | `SmEvalFunctionObject`                         |
//! | [`EvalFunction::evaluate`] (default)         | `SmEvalFunctionObject::Evaluate`               |
//! | [`IterationValue`]                           | `SmIterationValue`                             |
//! | [`IterQueue1d`], [`IterQueue1d::new`], [`IterQueue1d::try_new`] | `SmIterQueue1d`, `SmIterQueue1d(ULONG)` |
//! | [`IterQueue1d::add`]                         | `SmIterQueue1d::Add`                           |
//! | [`IterQueue1d::is_oscillating`]              | `SmIterQueue1d::IsOscillating`                 |
//! | [`LocalSolve1d`]                             | `SmLocalSolve1d`                               |
//! | [`LocalSolve1d::new`]                        | `SmLocalSolve1d(SmEvalFunctionObject&)`        |
//! | [`LocalSolve1d::with_interval`]              | `SmLocalSolve1d(SmEvalFunctionObject&, const SmExtent1d&, SmBoolean)` |
//! | `Drop` (implicit)                            | `~SmLocalSolve1d`                              |
//! | [`LocalSolve1d::found_accuracy`] etc.        | `GetFoundAccuracy`, `GetTerminationReason`, `GetLowerBoundHits`, `GetUpperBoundHits` |
//! | [`LocalSolve1d::set_solution_interval`]      | `SetSolutionInterval`                          |
//! | [`LocalSolve1d::set_solver_algorithm`]       | `SetSolverAlgorithm`                           |
//! | [`LocalSolve1d::set_desired_accuracy`]       | `SetDesiredAccuracy`                           |
//! | [`LocalSolve1d::solve_it`], [`SolveOutcome`] | `SolveIt`                                      |
//! | [`LocalSolve1d::solve_by_bisection`]         | `SolveByBisection`                             |
//! | [`LocalSolve1d::solve_by_brent`]             | `SolveByBrent`                                 |
//! | `solve_oscillating` (private)                | `SolveOscillating`                             |
//!
//! Deviations:
//! - The evaluator is passed to each solve call instead of being stored as a
//!   reference in the solver (Rust borrowing); everything else in the solver state
//!   (brackets, iteration values, counters) is kept and public as upstream.
//! - [`EvalFunction::evaluate`] keeps upstream's out-parameter shape: the solver
//!   hands it the destination values and keeps whatever the evaluator wrote even
//!   when it fails, because upstream reads such values afterwards (e.g. the
//!   interior search of `SolveOscillating` continues after a failed evaluation).
//! - Out-params become return values: [`SolveOutcome`] for `SolveIt` (the found
//!   parameter is `None` where upstream leaves `rdFoundT` unwritten), `(t, f)` for
//!   `SolveByBisection`/`SolveByBrent`. On failure nothing is returned (upstream
//!   callers do not read the out-params after an error); the solver state is still
//!   updated as upstream.
//! - Status codes: upstream returns either the evaluator's status or `SM_ERR`
//!   (1001, [`SolidError::Upstream`]) exactly as in C++. A
//!   [`SolidError::Unported`] from the evaluator is always propagated unchanged,
//!   also where upstream ignores or retries a failed evaluation (so callers that
//!   reach unported code report "deferred" instead of a different result).
//! - `SolveByBisection` recursion is a loop (same arithmetic and order).
//! - [`IterQueue1d::try_new`] (caller-chosen sizes) returns `InvalidInput` when the
//!   slots cannot be allocated or would take more than
//!   [`crate::limits::MAX_ALLOC_BYTES`] (upstream's `SetSize` fails softly and `Add`
//!   then writes out of bounds; upstream's only queue has 8 slots).
//! - `SE(...)`/`SER` error-log side effects and `SM_DEBUG_CODE` tracing are not
//!   ported.

use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
pub use crate::geom::local_solve_nd::TerminationReason;
use crate::geom::tol::{in_tol, scaled_zero};
use crate::limits::try_filled;
use crate::math::{BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, UNDEF_DOUBLE};

/// Upstream status `SM_ERR`.
const SM_ERR: i32 = 1001;

/// Upstream `SM_UNDEF_ULONG`.
pub const UNDEF_ULONG: u64 = 123456789;

/// `ZEPS` of `SolveByBrent`.
const ZEPS: f64 = 1.0e-12;

/// `SM_ERR` for `operation`.
fn sm_err(operation: &'static str) -> SolidError {
    SolidError::Upstream {
        operation,
        code: SM_ERR,
        entity: None,
    }
}

/// Upstream returns `SM_ERR` after a failed evaluation; an `Unported` error is
/// passed through unchanged.
fn eval_failed(e: SolidError, operation: &'static str) -> SolidError {
    if matches!(e, SolidError::Unported { .. }) {
        e
    } else {
        sm_err(operation)
    }
}

/// `SMSIGN(a, b)`: `|a|` with the sign of `b` (`b >= 0` counts as positive).
fn sign(a: f64, b: f64) -> f64 {
    if b >= 0.0 {
        a.abs()
    } else {
        -a.abs()
    }
}

/// Algorithm used by the solver (`SmLocalSolverAlgorithmType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SolverAlgorithm {
    /// `SM_SA_NEWTON` (default).
    #[default]
    Newton,
    /// `SM_SA_BISECTION`.
    Bisection,
    /// `SM_SA_BRENT_MAXIMIZE`.
    BrentMaximize,
    /// `SM_SA_BRENT_MINIMIZE`.
    BrentMinimize,
}

impl SolverAlgorithm {
    /// Upstream enum value (`SM_SA_NEWTON` = 0 .. `SM_SA_BRENT_MINIMIZE` = 3).
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::Newton => 0,
            Self::Bisection => 1,
            Self::BrentMaximize => 2,
            Self::BrentMinimize => 3,
        }
    }

    /// One of the Brent min/max modes.
    #[must_use]
    pub const fn is_brent(self) -> bool {
        matches!(self, Self::BrentMaximize | Self::BrentMinimize)
    }

    /// The algorithm with upstream enum value `code`.
    #[must_use]
    pub const fn from_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::Newton),
            1 => Some(Self::Bisection),
            2 => Some(Self::BrentMaximize),
            3 => Some(Self::BrentMinimize),
            _ => None,
        }
    }
}

/// Values an evaluator writes for one parameter (the out-params of
/// `SmEvalFunctionObject::Evaluate`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FunctionValue {
    /// `F(t)`.
    pub f: f64,
    /// `dF/dt (t)`.
    pub f_prime: f64,
    /// True when the evaluator judges `F(t)` to be zero within its own tolerance.
    pub found_answer: bool,
}

/// A function `F(t)` with derivative, solved by [`LocalSolve1d`]
/// (`SmEvalFunctionObject`).
pub trait EvalFunction {
    /// Evaluates `F` and `F'` at `t` into `out` (`Evaluate`). `signal_errors` is
    /// false where the solver anticipates failures. On failure the solver keeps
    /// whatever was written to `out`, as upstream does.
    ///
    /// The default (upstream base class) fails with `SM_ERR`.
    fn evaluate(&mut self, t: f64, out: &mut FunctionValue, signal_errors: bool) -> Result<()> {
        let _ = (t, out, signal_errors);
        Err(sm_err("geom::local_solve1d::EvalFunction::evaluate"))
    }
}

/// One iteration's values (`SmIterationValue`); all `UNDEF_DOUBLE` initially.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IterationValue {
    /// Parameter `t`.
    pub t: f64,
    /// `F(t)`.
    pub f: f64,
    /// `F'(t)`.
    pub f_prime: f64,
    /// The step that led to this `t`.
    pub prev_step: f64,
}

impl Default for IterationValue {
    fn default() -> Self {
        Self {
            t: UNDEF_DOUBLE,
            f: UNDEF_DOUBLE,
            f_prime: UNDEF_DOUBLE,
            prev_step: UNDEF_DOUBLE,
        }
    }
}

/// Fixed-size ring of the most recent iteration values (`SmIterQueue1d`): once
/// full, each addition overwrites the oldest slot.
#[derive(Debug, Clone, PartialEq)]
pub struct IterQueue1d {
    array: Vec<IterationValue>,
    size: u64,
    count: u64,
    next_idx: u64,
    full: bool,
}

impl IterQueue1d {
    /// An empty queue of `size` slots (`SmIterQueue1d(ULONG)`).
    ///
    /// For small sizes known at the call site. Like any `Vec`, it aborts when the
    /// slots cannot be allocated; use [`IterQueue1d::try_new`] for sizes that come
    /// from caller input.
    #[must_use]
    pub fn new(size: u64) -> Self {
        Self {
            array: vec![IterationValue::default(); size as usize],
            size,
            count: 0,
            next_idx: 0,
            full: false,
        }
    }

    /// An empty queue of `size` slots (`SmIterQueue1d(ULONG)`) for sizes that come
    /// from caller input.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when `size` slots cannot be allocated or would take
    /// more than [`crate::limits::MAX_ALLOC_BYTES`] (upstream's `SetSize` then fails
    /// softly and `Add` writes past the empty array).
    pub fn try_new(size: u64) -> Result<Self> {
        let too_large = || SolidError::InvalidInput {
            operation: "geom::local_solve1d::IterQueue1d::try_new",
            reason: "queue size too large to allocate",
        };
        let len = usize::try_from(size).map_err(|_| too_large())?;
        let array = try_filled(len, IterationValue::default()).ok_or_else(too_large)?;
        Ok(Self {
            array,
            size,
            count: 0,
            next_idx: 0,
            full: false,
        })
    }

    /// Stores `v` in the next slot (`Add`). A zero-size queue (undefined behaviour
    /// upstream) ignores the value.
    pub fn add(&mut self, v: &IterationValue) {
        let Some(slot) = self.array.get_mut(self.next_idx as usize) else {
            return;
        };
        *slot = *v;
        self.next_idx += 1;
        if self.next_idx >= self.size {
            self.next_idx = 0;
            self.full = true;
        }
        if self.count < self.size {
            self.count += 1;
        }
    }

    /// Whether the stored steps alternate (period `step`, only 2 is supported) with
    /// at least three cycles: the alternating sums of `prev_step` (by slot index)
    /// nearly cancel (`IsOscillating`; upstream default `step` is 2).
    #[must_use]
    pub fn is_oscillating(&self, step: u64) -> bool {
        if step != 2 {
            return false;
        }
        if self.count < 3 * step {
            return false;
        }
        let top = if self.full { self.size } else { self.next_idx };
        let mut sum1 = 0.0;
        let mut sum2 = 0.0;
        for (i, v) in self.array.iter().enumerate().take(top as usize) {
            if (i as u64) % step == 0 {
                sum1 += v.prev_step;
            } else {
                sum2 += v.prev_step;
            }
        }
        let sum_all = (sum1 + sum2).abs();
        let sum_abs = sum1.abs() + sum2.abs();
        sum_all < sum_abs / 100.0
    }
}

/// Result of [`LocalSolve1d::solve_it`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolveOutcome {
    /// `rbFoundSolution`: a solution within the acceptable accuracy was found.
    pub found: bool,
    /// `rdFoundT`: the solution, or the best parameter reached when none was found.
    pub t: f64,
}

/// Local solver for one-dimensional equations `F(t) = 0` and min/max problems
/// (`SmLocalSolve1d`).
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // upstream's SmBoolean members, kept public
pub struct LocalSolve1d {
    use_interval: bool,
    interval: Extent1d,
    algorithm: SolverAlgorithm,
    desired_accuracy: f64,
    acceptable_accuracy: f64,
    max_iter: u64,
    max_boundary_hits: u64,
    is_periodic: bool,
    /// A zero crossing's lower (`F < 0`) side is known.
    pub have_lower_bracket: bool,
    /// `F` at the lower bracket.
    pub lower_bracket_f: f64,
    /// Parameter of the lower bracket.
    pub lower_bracket_t: f64,
    /// A zero crossing's upper (`F > 0`) side is known.
    pub have_upper_bracket: bool,
    /// `F` at the upper bracket.
    pub upper_bracket_f: f64,
    /// Parameter of the upper bracket.
    pub upper_bracket_t: f64,
    /// Values two iterations ago (`m_v2Prev`).
    pub prev2: IterationValue,
    /// Values one iteration ago (`m_v1Prev`).
    pub prev1: IterationValue,
    /// Current values (`m_vCurr`); holds the final `t`/`F` after a solve.
    pub curr: IterationValue,
    /// Iteration counter (`m_lIter`).
    pub iter: u64,
    /// `F` at the returned parameter (`m_dFoundAccuracy`).
    pub found_accuracy: f64,
    /// Consecutive clamps at the interval minimum.
    pub lower_bound_hits: u64,
    /// Consecutive clamps at the interval maximum.
    pub upper_bound_hits: u64,
    /// Why the last solve stopped.
    pub termination_reason: TerminationReason,
    /// Not upstream (WR-253, source issue #481): `SolveOscillating` takes a
    /// sign change only from an interior guess that evaluated (`false`, the default:
    /// upstream also reads a failed guess's leftover `F`).
    failed_guess_no_sign_change: bool,
}

impl Default for LocalSolve1d {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalSolve1d {
    /// A Newton solver on the whole real line (`SmLocalSolve1d(rFunctionEvaluator)`).
    #[must_use]
    pub fn new() -> Self {
        Self {
            use_interval: false,
            interval: Extent1d::new(),
            algorithm: SolverAlgorithm::Newton,
            desired_accuracy: EFF_ZERO,
            acceptable_accuracy: UNDEF_DOUBLE,
            max_iter: 100,
            max_boundary_hits: UNDEF_ULONG,
            is_periodic: false,
            have_lower_bracket: false,
            lower_bracket_f: UNDEF_DOUBLE,
            lower_bracket_t: UNDEF_DOUBLE,
            have_upper_bracket: false,
            upper_bracket_f: UNDEF_DOUBLE,
            upper_bracket_t: UNDEF_DOUBLE,
            prev2: IterationValue::default(),
            prev1: IterationValue::default(),
            curr: IterationValue::default(),
            iter: 0,
            found_accuracy: UNDEF_DOUBLE,
            lower_bound_hits: UNDEF_ULONG,
            upper_bound_hits: UNDEF_ULONG,
            termination_reason: TerminationReason::Undefined,
            failed_guess_no_sign_change: false,
        }
    }

    /// A Newton solver restricted to `interval`, wrapping instead of clamping when
    /// `is_periodic` (`SmLocalSolve1d(rFunctionEvaluator, crInterval, bIsPeriodic)`).
    #[must_use]
    pub fn with_interval(interval: Extent1d, is_periodic: bool) -> Self {
        Self {
            use_interval: true,
            interval,
            is_periodic,
            ..Self::new()
        }
    }

    /// `F` at the returned parameter (`GetFoundAccuracy`).
    #[must_use]
    pub fn found_accuracy(&self) -> f64 {
        self.found_accuracy
    }

    /// Why the last solve stopped (`GetTerminationReason`).
    #[must_use]
    pub fn termination_reason(&self) -> TerminationReason {
        self.termination_reason
    }

    /// Consecutive clamps at the interval minimum (`GetLowerBoundHits`).
    #[must_use]
    pub fn lower_bound_hits(&self) -> u64 {
        self.lower_bound_hits
    }

    /// Consecutive clamps at the interval maximum (`GetUpperBoundHits`).
    #[must_use]
    pub fn upper_bound_hits(&self) -> u64 {
        self.upper_bound_hits
    }

    /// The problem interval (without changing whether it is used).
    #[must_use]
    pub fn interval(&self) -> Extent1d {
        self.interval
    }

    /// Replaces the problem interval; whether it limits the solution is unchanged
    /// (`SetSolutionInterval`).
    pub fn set_solution_interval(&mut self, interval: Extent1d) {
        self.interval = interval;
    }

    /// Not upstream (WR-253, source issue #481): whether `SolveOscillating`
    /// ignores an interior guess whose evaluation failed when it looks for a sign change
    /// (`true`), instead of reading the `F` the failed evaluation left behind (`false`,
    /// upstream and the default).
    pub fn set_failed_guess_no_sign_change(&mut self, on: bool) {
        self.failed_guess_no_sign_change = on;
    }

    /// Selects the algorithm (`SetSolverAlgorithm`).
    pub fn set_solver_algorithm(&mut self, algorithm: SolverAlgorithm) {
        self.algorithm = algorithm;
    }

    /// Sets the accuracy at which iteration stops as converged (`SetDesiredAccuracy`;
    /// default `SM_EFF_ZERO`).
    pub fn set_desired_accuracy(&mut self, accuracy: f64) {
        self.desired_accuracy = accuracy;
    }

    /// The accuracy at which iteration stops as converged.
    #[must_use]
    pub fn desired_accuracy(&self) -> f64 {
        self.desired_accuracy
    }

    /// Minimises (or, with `maximize`, maximises) `F` on the bracket `ax`..`cx`
    /// starting from `bx`, by secant steps on `F'` or bisection (`SolveByBrent`).
    /// `F(bx)` should be better than `F(ax)` and `F(cx)`. Returns `(t, F(t))`
    /// (`F` as evaluated, not negated). Fails with `SM_ERR` when an evaluation
    /// fails or the iteration limit is reached.
    pub fn solve_by_brent<E: EvalFunction + ?Sized>(
        &self,
        eval: &mut E,
        ax: f64,
        bx: f64,
        cx: f64,
        tol: f64,
        maximize: bool,
    ) -> Result<(f64, f64)> {
        const OP: &str = "geom::local_solve1d::LocalSolve1d::solve_by_brent";
        let mut bracket_min = if ax < cx { ax } else { cx };
        let mut bracket_max = if ax > cx { ax } else { cx };

        let mut u: f64;
        let mut fu: f64;
        let mut du: f64;
        let mut x = bx;
        let mut v = bx;
        let mut fv = 0.0;
        let mut dv = 0.0;
        let mut w = bx;
        let mut fw = 0.0;
        let mut dw = 0.0;

        let mut delta_x = 0.0;
        let mut e: f64 = 0.0;

        let mut val = FunctionValue::default();
        eval.evaluate(x, &mut val, true)
            .map_err(|err| eval_failed(err, OP))?;
        let mut fx = val.f;
        let mut dx = val.f_prime;
        if maximize {
            fx = -fx;
            dx = -dx;
        }

        let mut iter = 1;
        while iter <= self.max_iter {
            let tol1 = tol * x.abs() + ZEPS;
            let tol2 = 2.0 * tol1;

            // Calculate delta_x by either secant or bisection.
            let mut secant_ok = false;
            if e.abs() > tol1 {
                // Trial secant steps on the derivatives, between x and w and x and v.
                let mut d1 = 2.0 * (bracket_max - bracket_min);
                let mut d2 = d1;
                if dw != dx && w != x {
                    d1 = (w - x) * dx / (dx - dw);
                }
                if dv != dx && v != x {
                    d2 = (v - x) * dx / (dx - dv);
                }
                let u1 = x + d1;
                let u2 = x + d2;

                // ok if: between the bracket ends && slope has opposite sign.
                let ok1 = (bracket_min - u1) * (u1 - bracket_max) > 0.0 && dx * d1 <= 0.0;
                let ok2 = (bracket_min - u2) * (u2 - bracket_max) > 0.0 && dx * d2 <= 0.0;

                let olde = e;
                e = delta_x;

                if ok1 || ok2 {
                    let step = if ok1 && ok2 {
                        if d1.abs() < d2.abs() {
                            d1
                        } else {
                            d2
                        }
                    } else if ok1 {
                        d1
                    } else {
                        d2
                    };

                    // Accept the secant step only if it is less than half the one
                    // before last.
                    if step.abs() <= (0.5 * olde).abs() {
                        secant_ok = true;
                        delta_x = step;
                        // Too close to the bracket ends?
                        let u = x + delta_x;
                        if u - bracket_min < tol2 || bracket_max - u < tol2 {
                            delta_x = sign(tol1, delta_x);
                        }
                    }
                }
            }
            if !secant_ok {
                // Bisect towards the downhill bracket end.
                e = if dx >= 0.0 {
                    bracket_min - x
                } else {
                    bracket_max - x
                };
                delta_x = 0.5 * e;
            }

            // Apply delta_x.
            if delta_x.abs() >= tol1 {
                u = x + delta_x;
                if u == x {
                    // Step is below machine precision.
                    return Ok((x, fx));
                }
                eval.evaluate(u, &mut val, true)
                    .map_err(|err| eval_failed(err, OP))?;
                fu = val.f;
                du = val.f_prime;
                if maximize {
                    fu = -fu;
                    du = -du;
                }
            } else {
                u = x + sign(tol1, delta_x);
                // Step must stay within the bracket.
                if u <= bracket_min {
                    u = (x + bracket_min) / 2.0;
                }
                if u >= bracket_max {
                    u = (x + bracket_max) / 2.0;
                }
                eval.evaluate(u, &mut val, true)
                    .map_err(|err| eval_failed(err, OP))?;
                fu = val.f;
                du = val.f_prime;
                if maximize {
                    fu = -fu;
                    du = -du;
                }
                // Within machine precision, or down in the noise.
                if u == x || fu >= fx {
                    return Ok((x, fx));
                }
            }

            // Update values.
            if fu <= fx {
                if u >= x {
                    bracket_min = x;
                } else {
                    bracket_max = x;
                }
                (v, fv, dv) = (w, fw, dw);
                (w, fw, dw) = (x, fx, dx);
                (x, fx, dx) = (u, fu, du);
            } else {
                if u < x {
                    bracket_min = u;
                } else {
                    bracket_max = u;
                }
                if fu <= fw || w == x {
                    // w == x means w, fw, dw have not been set yet.
                    (v, fv, dv) = (w, fw, dw);
                    (w, fw, dw) = (u, fu, du);
                } else if fu < fv || v == x || v == w {
                    (v, fv, dv) = (u, fu, du);
                }
            }
            iter += 1;
        }

        // Should have returned within the loop.
        Err(sm_err(OP))
    }

    /// Regula-falsi bisection between `lower_t` (`F < 0`) and `upper_t` (`F > 0`)
    /// for at most `max_division_level` subdivisions; the fraction is clamped to
    /// `[0.15, 0.85]` (`SolveByBisection`). Returns `(t, F(t))`. Fails with
    /// `SM_ERR` when an evaluation fails.
    #[allow(clippy::manual_clamp)] // keep upstream's comparisons
    pub fn solve_by_bisection<E: EvalFunction + ?Sized>(
        &self,
        eval: &mut E,
        max_division_level: u64,
        mut lower_t: f64,
        mut upper_t: f64,
        mut lower_f: f64,
        mut upper_f: f64,
    ) -> Result<(f64, f64)> {
        const OP: &str = "geom::local_solve1d::LocalSolve1d::solve_by_bisection";
        let mut level = max_division_level;
        loop {
            if level == 0 {
                // Return the better value (low is negative, high is positive).
                return Ok(if lower_f + upper_f >= 0.0 {
                    (lower_t, lower_f)
                } else {
                    (upper_t, upper_f)
                });
            }

            let mut frac = -lower_f / (upper_f - lower_f);
            // (upstream's two comparisons, not f64::clamp)
            if frac < 0.15 {
                frac = 0.15;
            }
            if frac > 0.85 {
                frac = 0.85;
            }
            let mid_t = lower_t + frac * (upper_t - lower_t);

            let scale = 1.0 * mid_t.abs();
            if (mid_t - upper_t).abs() < EFF_ZERO_SQ * scale
                || (mid_t - lower_t).abs() < EFF_ZERO_SQ * scale
            {
                return Ok(if lower_f.abs() < upper_f.abs() {
                    (lower_t, lower_f)
                } else if upper_f.abs() < lower_f.abs() {
                    (upper_t, upper_f)
                } else {
                    // They can be identical: use the midpoint.
                    ((lower_t + upper_t) / 2.0, (lower_f + upper_f) / 2.0)
                });
            }

            let mut val = FunctionValue::default();
            eval.evaluate(mid_t, &mut val, true)
                .map_err(|err| eval_failed(err, OP))?;
            let mid_f = val.f;
            if val.found_answer || mid_f == 0.0 {
                return Ok((mid_t, mid_f));
            }

            // Continue with whichever interval brackets zero.
            if mid_f > 0.0 {
                upper_t = mid_t;
                upper_f = mid_f;
            } else {
                lower_t = mid_t;
                lower_f = mid_f;
            }
            level -= 1;
        }
    }

    /// Evaluates at `value.t` into `value.f` / `value.f_prime` and `found`, keeping
    /// whatever the evaluator wrote even when it fails.
    fn eval_into<E: EvalFunction + ?Sized>(
        eval: &mut E,
        value: &mut IterationValue,
        found: &mut bool,
        signal_errors: bool,
    ) -> Result<()> {
        let mut val = FunctionValue {
            f: value.f,
            f_prime: value.f_prime,
            found_answer: *found,
        };
        let r = eval.evaluate(value.t, &mut val, signal_errors);
        value.f = val.f;
        value.f_prime = val.f_prime;
        *found = val.found_answer;
        r
    }

    /// Resolves an oscillating Newton iteration between `prev1` and `curr`: look
    /// for a sign change in between and bisect, else Brent (min/max modes) or the
    /// best interior value (`SolveOscillating`). Leaves the result in `curr`.
    fn solve_oscillating<E: EvalFunction + ?Sized>(
        &mut self,
        eval: &mut E,
        guess_t: f64,
    ) -> Result<SolveOutcome> {
        const GUESSES: [f64; 8] = [0.25, 0.75, 0.13, 0.38, 0.82, 0.87, 0.01, 0.99];
        let mut found_answer = false;

        let mut temp = IterationValue {
            t: 0.5 * (self.prev1.t + self.curr.t),
            ..IterationValue::default()
        };
        Self::eval_into(eval, &mut temp, &mut found_answer, true)?;

        let mut best_interior_t = temp.t;
        let mut best_interior_f = temp.f;

        // Not bracketed: look for a sign change between prev1 and curr.
        if !self.have_lower_bracket || !self.have_upper_bracket {
            let curr_t = self.curr.t;
            let prev_t = self.prev1.t;
            let curr_f = self.curr.f;

            let mut evaluated_sign_change = false;
            for g in GUESSES {
                let mid_t = prev_t + g * (curr_t - prev_t);
                temp.t = mid_t;
                let r = Self::eval_into(eval, &mut temp, &mut found_answer, true);
                if ignored(r)?.is_none() {
                    continue; // just keep trying
                }
                if temp.f.abs() < best_interior_f.abs() {
                    best_interior_f = temp.f;
                    best_interior_t = mid_t;
                }
                if temp.f * curr_f <= 0.0 {
                    evaluated_sign_change = true;
                    break; // opposite sign
                }
            }

            // (upstream tests the last guess's `F`, failed or not: WR-253)
            let sign_change = if self.failed_guess_no_sign_change {
                evaluated_sign_change
            } else {
                temp.f * curr_f <= 0.0
            };
            if sign_change {
                // Curr and prev have the same sign, opposite to mid: pick the end on
                // the side of the given guess.
                let pick_curr = if guess_t < temp.t {
                    curr_t < prev_t
                } else {
                    curr_t > prev_t
                };
                let end = if pick_curr {
                    (curr_t, curr_f)
                } else {
                    (prev_t, self.prev1.f)
                };
                let ((mut new_lo_t, mut new_lo_f), (mut new_hi_t, mut new_hi_f)) = if temp.f > 0.0 {
                    (end, (temp.t, temp.f))
                } else {
                    ((temp.t, temp.f), end)
                };

                // Prefer an existing bracket when it is closer.
                if self.have_lower_bracket
                    && (self.lower_bracket_t - new_hi_t).abs() < (new_lo_t - new_hi_t).abs()
                {
                    new_lo_t = self.lower_bracket_t;
                    new_lo_f = self.lower_bracket_f;
                }
                if self.have_upper_bracket
                    && (self.upper_bracket_t - new_lo_t).abs() < (new_hi_t - new_lo_t).abs()
                {
                    new_hi_t = self.upper_bracket_t;
                    new_hi_f = self.upper_bracket_f;
                }

                self.have_lower_bracket = true;
                self.lower_bracket_t = new_lo_t;
                self.lower_bracket_f = new_lo_f;
                self.have_upper_bracket = true;
                self.upper_bracket_t = new_hi_t;
                self.upper_bracket_f = new_hi_f;
            }
        }

        // Bracketed: bisect (deeper, we might not be close). Success even if F is
        // too big: at a discontinuity the caller wants the best solution.
        if self.have_lower_bracket && self.have_upper_bracket {
            let r = self.solve_by_bisection(
                eval,
                40,
                self.lower_bracket_t,
                self.upper_bracket_t,
                self.lower_bracket_f,
                self.upper_bracket_f,
            );
            if let Some((t, f)) = ignored(r)? {
                self.curr.t = t;
                self.curr.f = f;
                self.found_accuracy = self.curr.f;
                self.termination_reason = if self.curr.f.abs() < self.desired_accuracy {
                    TerminationReason::FoundAnswerConverged
                } else {
                    TerminationReason::FoundAnswerClose
                };
                return Ok(SolveOutcome {
                    found: true,
                    t: self.curr.t,
                });
            }
        }

        // No bracket: oscillating between two values of the same sign.
        if self.algorithm.is_brent() {
            let maximize = self.algorithm == SolverAlgorithm::BrentMaximize;
            let (t, _) = self.solve_by_brent(
                eval,
                self.prev1.t,
                best_interior_t,
                self.curr.t,
                self.desired_accuracy,
                maximize,
            )?;
            self.curr.t = t;
        } else if best_interior_f.abs() < self.curr.f.abs() {
            self.curr.t = best_interior_t;
        } else {
            // just try the midpoint
            self.curr.t = 0.5 * (self.prev1.t + self.curr.t);
        }

        // The caller expects the solution in curr.
        Self::eval_into(eval, &mut self.curr, &mut found_answer, true)?;

        let found = if self.algorithm.is_brent() {
            true // for min/max the tolerance does not matter
        } else {
            found_answer || self.curr.f.abs() <= self.acceptable_accuracy
        };
        Ok(SolveOutcome {
            found,
            t: self.curr.t,
        })
    }

    /// Finds a root of `F` near `guess_t` by damped Newton iteration, falling back
    /// to oscillation handling, bisection on a bracket, or Brent min/max for the
    /// `Brent*` algorithms (`SolveIt`). `found` is set when `|F| < desired` or
    /// within `acceptable_accuracy`. Succeeds even when no solution is found; fails
    /// only when an evaluation fails (`SM_ERR` or the evaluator's status). The
    /// final values are also left in [`LocalSolve1d::curr`].
    pub fn solve_it<E: EvalFunction + ?Sized>(
        &mut self,
        eval: &mut E,
        guess_t: f64,
        acceptable_accuracy: f64,
    ) -> Result<SolveOutcome> {
        const OP: &str = "geom::local_solve1d::LocalSolve1d::solve_it";

        self.have_lower_bracket = false;
        self.have_upper_bracket = false;
        self.lower_bracket_f = -BIG_DOUBLE;
        self.upper_bracket_f = BIG_DOUBLE;
        self.iter = 0;
        self.lower_bound_hits = 0;
        self.upper_bound_hits = 0;
        self.max_boundary_hits = 2;
        self.termination_reason = TerminationReason::UnableToConverge;
        self.acceptable_accuracy = acceptable_accuracy;
        self.curr.prev_step = 0.0;

        // The initial guess must be within the problem interval (clamped if not).
        let mut guess_t = guess_t;
        if self.use_interval && !self.interval.contains_value(guess_t, 0.0) {
            // (upstream only logs SM_ERR_INVALID_INPUT beyond EFF_ZERO_SQRT and goes on)
            guess_t = self.interval.clamp_value(guess_t);
        }

        // Evaluate at the guess.
        self.curr.t = guess_t;
        let mut found_answer = false;
        let r = Self::eval_into(eval, &mut self.curr, &mut found_answer, false);
        if let Err(e) = r {
            if matches!(e, SolidError::Unported { .. }) {
                return Err(e);
            }
            // Bad start point: within an interval, tweak it by 1/1000 of the interval.
            if self.use_interval {
                let size = self.interval.length();
                self.curr.t = guess_t + size / 1000.0;
                if !self.interval.contains_value(self.curr.t, 0.0) {
                    self.curr.t = guess_t - size / 1000.0;
                }
                Self::eval_into(eval, &mut self.curr, &mut found_answer, true)
                    .map_err(|err| eval_failed(err, OP))?;
            }
        }

        // Serendipity: the guess is a solution.
        if found_answer {
            self.found_accuracy = self.curr.f;
            self.termination_reason = TerminationReason::FoundAnswerConverged;
            return Ok(SolveOutcome {
                found: true,
                t: self.curr.t,
            });
        }

        let mut cut_factor = 1.0;
        let mut minimum = self.curr.f.abs();
        let mut min_count: u64 = 0;
        let mut queue = IterQueue1d::new(8);

        self.iter = 0;
        while self.iter < self.max_iter {
            // Brackets of zero crossings (used for bisection after Newton).
            if self.curr.f < 0.0 {
                self.have_lower_bracket = true;
                if self.curr.f > self.lower_bracket_f {
                    self.lower_bracket_t = self.curr.t;
                    self.lower_bracket_f = self.curr.f;
                }
            }
            if self.curr.f > 0.0 {
                self.have_upper_bracket = true;
                if self.curr.f < self.upper_bracket_f {
                    self.upper_bracket_t = self.curr.t;
                    self.upper_bracket_f = self.curr.f;
                }
            }

            // Converged.
            if self.curr.f.abs() < self.desired_accuracy {
                self.found_accuracy = self.curr.f;
                self.termination_reason = TerminationReason::FoundAnswerConverged;
                return Ok(SolveOutcome {
                    found: true,
                    t: self.curr.t,
                });
            }

            if queue.is_oscillating(2) {
                return self.solve_oscillating(eval, guess_t);
            }

            // f'(t) near zero: use +/-1 to nudge in the right direction.
            if in_tol(self.curr.f_prime, scaled_zero()) {
                self.curr.f_prime = if self.curr.f_prime < 0.0 { -1.0 } else { 1.0 };
            }

            // Newton step, never huge (even 0.10 can be too big).
            let mut delta_t = -self.curr.f / self.curr.f_prime;
            let mut step_limit = self.interval.length() * 0.08 + EFF_ZERO;
            if delta_t.abs() > step_limit {
                step_limit *= 0.99; // just to avoid exact half
                if delta_t < 0.0 {
                    step_limit = -step_limit;
                }
                delta_t = step_limit;
            }

            // Before giving up, cut the step size and allow 20 more tries.
            if self.iter > 20 && self.iter >= self.max_iter - 1 {
                cut_factor *= 0.5;
                if cut_factor > 0.0001 {
                    self.iter -= 20;
                }
            }

            delta_t *= cut_factor;
            let mut new_t = self.curr.t + delta_t;

            // The next guess leaves the problem domain.
            if self.use_interval && !self.interval.contains_value(new_t, 0.0) {
                if self.is_periodic {
                    new_t = self.interval.periodic_wrap(new_t);
                } else {
                    // Clamp and count boundary hits; reset the other side's count.
                    if new_t < self.interval.min() {
                        new_t = self.interval.min();
                        self.lower_bound_hits += 1;
                        self.upper_bound_hits = 0;
                    } else if new_t > self.interval.max() {
                        new_t = self.interval.max();
                        self.upper_bound_hits += 1;
                        self.lower_bound_hits = 0;
                    }
                    if self.lower_bound_hits > self.max_boundary_hits
                        || self.upper_bound_hits > self.max_boundary_hits
                    {
                        // (upstream also sets rdFoundT = new_t, overwritten below)
                        self.termination_reason = TerminationReason::OutOfBounds;
                        break;
                    }
                }
            } else {
                self.lower_bound_hits = 0;
                self.upper_bound_hits = 0;
            }

            // Save the last two iterations.
            self.curr.prev_step = if self.iter > 0 {
                self.curr.t - self.prev1.t
            } else {
                0.0
            };
            self.prev2 = if self.iter > 1 { self.prev1 } else { self.curr };
            self.prev1 = self.curr;
            queue.add(&self.prev1);

            // Evaluate into curr.
            self.curr.t = new_t;
            Self::eval_into(eval, &mut self.curr, &mut found_answer, true)
                .map_err(|err| eval_failed(err, OP))?;

            if found_answer || self.curr.f.abs() < self.desired_accuracy {
                self.found_accuracy = self.curr.f;
                self.termination_reason = TerminationReason::FoundAnswerConverged;
                return Ok(SolveOutcome {
                    found: true,
                    t: self.curr.t,
                });
            }

            // No decrease for 4 iterations: stuck, try bisection.
            if self.curr.f.abs() >= minimum {
                min_count += 1;
                if min_count >= 4 && self.curr.f.abs() < self.acceptable_accuracy {
                    break;
                }
            } else {
                min_count = 0;
                minimum = self.curr.f.abs();
            }

            // Brent min/max once the extremum is roughly bracketed: curr beats both
            // previous values.
            if self.algorithm.is_brent() && self.iter > 1 {
                let maximize = self.algorithm == SolverAlgorithm::BrentMaximize;
                let beats = |a: f64, b: f64| if maximize { a > b } else { a < b };
                if beats(self.curr.f, self.prev1.f) && beats(self.curr.f, self.prev2.f) {
                    let mut ext = Extent1d::from_value(self.prev1.t);
                    ext.add_value(self.prev2.t);
                    if ext.contains_value(self.curr.t, 0.0) {
                        let (t, _) = self.solve_by_brent(
                            eval,
                            self.prev1.t,
                            self.curr.t,
                            self.prev2.t,
                            self.desired_accuracy,
                            maximize,
                        )?;
                        return Ok(SolveOutcome { found: true, t });
                    }
                }
            }

            // Getting larger on the same side of the answer.
            if self.curr.f.abs() >= self.prev1.f.abs() && self.curr.f * self.prev1.f > 0.0 {
                if self.curr.f != self.prev1.f
                    && self.curr.t != self.prev1.t
                    && self.have_lower_bracket
                    && self.have_upper_bracket
                {
                    break; // bracketed: try bisection
                }

                // The step went to zero: exit with or without a solution.
                if delta_t.abs() < EFF_ZERO {
                    let f_mag = self.curr.f.abs();
                    self.found_accuracy = self.curr.f;
                    self.termination_reason = self.reason_for(f_mag);
                    return Ok(SolveOutcome {
                        found: f_mag < self.acceptable_accuracy,
                        t: self.curr.t,
                    });
                }
            }
            self.iter += 1;
        }

        // Not converged (iteration limit, out of bounds, or diverging): try
        // bisection on a bracket.
        if self.have_lower_bracket && self.have_upper_bracket {
            let r = self.solve_by_bisection(
                eval,
                20,
                self.lower_bracket_t,
                self.upper_bracket_t,
                self.lower_bracket_f,
                self.upper_bracket_f,
            );
            if let Some((t, f)) = ignored(r)? {
                self.curr.t = t;
                self.curr.f = f;
            }
        }

        if self.curr.f.abs() < self.acceptable_accuracy {
            self.found_accuracy = self.curr.f;
            let f_mag = self.curr.f.abs();
            self.termination_reason = self.reason_for(f_mag);
            return Ok(SolveOutcome {
                found: true,
                t: self.curr.t,
            });
        }

        // Best answer anyway, so that min/max works without convergence.
        self.found_accuracy = self.curr.f;
        Ok(SolveOutcome {
            found: false,
            t: self.curr.t,
        })
    }

    /// Termination reason for a final `|F|` of `f_mag`.
    fn reason_for(&self, f_mag: f64) -> TerminationReason {
        if f_mag < self.desired_accuracy {
            TerminationReason::FoundAnswerConverged
        } else if f_mag < self.acceptable_accuracy {
            TerminationReason::FoundAnswerClose
        } else {
            TerminationReason::UnableToConverge
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `F(t) = t^2 - 2`, never failing.
    struct Sq;
    impl EvalFunction for Sq {
        fn evaluate(&mut self, t: f64, out: &mut FunctionValue, _: bool) -> Result<()> {
            out.f = t * t - 2.0;
            out.f_prime = 2.0 * t;
            out.found_answer = false;
            Ok(())
        }
    }

    /// `F(t) = t^3 - 2t + 2`: Newton from 0 cycles 0, 1, 0, 1, ...
    struct Cycle;
    impl EvalFunction for Cycle {
        fn evaluate(&mut self, t: f64, out: &mut FunctionValue, _: bool) -> Result<()> {
            out.f = (t * t - 2.0) * t + 2.0;
            out.f_prime = 3.0 * t * t - 2.0;
            out.found_answer = false;
            Ok(())
        }
    }

    /// The two values `Cycle` settles between from 0 within [-3, 3] (Newton's
    /// steps are capped at 0.08 of the interval) and the last interior guess of
    /// `SolveOscillating` between them, 0.99 of the way.
    const CYCLE_PREV: f64 = 0.475_200_000_000_989_94;
    const CYCLE_CURR: f64 = 0.950_400_000_001_979_9;
    const LAST_GUESS: f64 = CYCLE_PREV + 0.99 * (CYCLE_CURR - CYCLE_PREV);

    /// `Cycle`, except that the evaluation at `LAST_GUESS` fails after zeroing its
    /// outputs, as `SmCurvePropertyEFO::Evaluate` does (`SmCurve.cpp:14918`).
    struct CycleFailingAtLastGuess;
    impl EvalFunction for CycleFailingAtLastGuess {
        fn evaluate(&mut self, t: f64, out: &mut FunctionValue, s: bool) -> Result<()> {
            if t.to_bits() == LAST_GUESS.to_bits() {
                *out = FunctionValue::default();
                return Err(sm_err("test: no value here"));
            }
            Cycle.evaluate(t, out, s)
        }
    }

    struct Unp;
    impl EvalFunction for Unp {
        fn evaluate(&mut self, _: f64, _: &mut FunctionValue, _: bool) -> Result<()> {
            Err(SolidError::Unported {
                upstream: "X",
                unit: "y",
            })
        }
    }

    struct Base;
    impl EvalFunction for Base {}

    #[test]
    fn newton_sqrt2() {
        let mut s = LocalSolve1d::with_interval(Extent1d::from_min_max(0.0, 3.0), false);
        let r = s.solve_it(&mut Sq, 1.0, 1e-9).unwrap();
        assert!(r.found);
        assert!((r.t - 2f64.sqrt()).abs() < 1e-10);
        assert_eq!(
            s.termination_reason(),
            TerminationReason::FoundAnswerConverged
        );
    }

    #[test]
    fn oscillation_is_resolved() {
        let mut s = LocalSolve1d::with_interval(Extent1d::from_min_max(-3.0, 3.0), false);
        let r = s.solve_it(&mut Cycle, 0.0, 1e-9).unwrap();
        assert!(r.t.is_finite());
    }

    /// WR-253 (#481): a failed interior evaluation in `SolveOscillating` leaves
    /// its zeroed `F` behind, and the sign test after the guesses reads it as a
    /// sign change. `F = t^3 - 2t + 2` has no root between the two values Newton
    /// cycles on, so the clean solve finds nothing; when the last guess fails,
    /// upstream brackets that guess with `F = 0`, bisection keeps it, and the solve
    /// reports it converged there. The port does the same.
    #[test]
    fn failed_guess_is_read_as_a_sign_change_as_upstream() {
        let mut s = LocalSolve1d::with_interval(Extent1d::from_min_max(-3.0, 3.0), false);
        let clean = s.solve_it(&mut Cycle, 0.0, 1e-9).unwrap();
        assert!(!clean.found);
        assert_eq!(s.termination_reason(), TerminationReason::UnableToConverge);

        let mut s = LocalSolve1d::with_interval(Extent1d::from_min_max(-3.0, 3.0), false);
        let r = s.solve_it(&mut CycleFailingAtLastGuess, 0.0, 1e-9).unwrap();
        assert!(r.found);
        assert_eq!(r.t.to_bits(), LAST_GUESS.to_bits());
        assert_eq!(
            s.termination_reason(),
            TerminationReason::FoundAnswerConverged
        );
        assert_eq!(
            (s.lower_bracket_t.to_bits(), s.lower_bracket_f),
            (LAST_GUESS.to_bits(), 0.0)
        );
        // F really is about 0.95 there.
        let mut out = FunctionValue::default();
        Cycle.evaluate(LAST_GUESS, &mut out, true).unwrap();
        assert!(out.f > 0.9);
    }

    /// source issue #481 (WR-253): with the opt-in, the failed last guess is
    /// no sign change, and the solve finds nothing, as on the clean function.
    #[test]
    fn issue_481_failed_guess_is_no_sign_change() {
        let mut s = LocalSolve1d::with_interval(Extent1d::from_min_max(-3.0, 3.0), false);
        s.set_failed_guess_no_sign_change(true);
        let r = s.solve_it(&mut CycleFailingAtLastGuess, 0.0, 1e-9).unwrap();
        assert!(!r.found, "{r:?}");
        assert_ne!(r.t.to_bits(), LAST_GUESS.to_bits());
        assert_ne!(s.lower_bracket_t.to_bits(), LAST_GUESS.to_bits());
    }

    #[test]
    fn queue_oscillation() {
        let mut q = IterQueue1d::new(8);
        for i in 0..6 {
            let step = if i % 2 == 0 { 1.0 } else { -1.0 };
            assert!(!q.is_oscillating(2));
            q.add(&IterationValue {
                prev_step: step,
                ..IterationValue::default()
            });
        }
        assert!(q.is_oscillating(2));
        assert!(!q.is_oscillating(3));
        let mut z = IterQueue1d::new(0);
        z.add(&IterationValue::default());
        assert!(!z.is_oscillating(2));
        assert_eq!(IterQueue1d::try_new(8).unwrap(), IterQueue1d::new(8));
        // fuzz regression: a caller-chosen size that cannot be allocated fails softly
        assert!(matches!(
            IterQueue1d::try_new(u64::MAX),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(IterQueue1d::try_new(1 << 60).is_err());
    }

    #[test]
    fn bisection_and_brent() {
        let s = LocalSolve1d::new();
        let (t, f) = s
            .solve_by_bisection(&mut Sq, 40, 1.0, 2.0, -1.0, 2.0)
            .unwrap();
        assert!((t - 2f64.sqrt()).abs() < 1e-9 && f.abs() < 1e-8);
        let (t, _) = s
            .solve_by_brent(&mut Sq, -1.0, 0.3, 2.0, 1e-12, false)
            .unwrap();
        assert!(t.abs() < 1e-6);
        assert_eq!(
            s.solve_by_bisection(&mut Sq, 0, 1.0, 2.0, -1.0, 2.0)
                .unwrap(),
            (1.0, -1.0)
        );
    }

    #[test]
    fn errors() {
        let mut s = LocalSolve1d::with_interval(Extent1d::from_min_max(0.0, 1.0), false);
        assert!(matches!(
            s.solve_it(&mut Unp, 0.5, 1e-9),
            Err(SolidError::Unported { .. })
        ));
        assert!(matches!(
            s.solve_it(&mut Base, 0.5, 1e-9),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
        assert_eq!(
            SolverAlgorithm::from_code(2),
            Some(SolverAlgorithm::BrentMaximize)
        );
        assert_eq!(SolverAlgorithm::BrentMinimize.code(), 3);
    }
}
