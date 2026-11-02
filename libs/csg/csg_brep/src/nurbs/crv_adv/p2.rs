//! Advanced NURBS curve routines, part 2: the public curve extension entry points, the
//! second-derivative bound of planar offsets, evaluation outside the parameter range,
//! knot removal with end-derivative constraints, refinement to a knot vector, the
//! symbolic operators (sum/difference, first derivative curves, scalar products) and
//! the rational and arc-length reparametrisations.
//!
//! Upstream: `source/SMLib/src/NL_CrvAdv.cpp` (declarations in
//! `source/SMLib/inc/NL_CrvAdv.h`).
//!
//! | Rust                                         | Upstream                            |
//! |----------------------------------------------|-------------------------------------|
//! | [`Curve::extend_by_dist`]                    | `N_CrvExtendByDist`                 |
//! | [`Curve::extend_by_param_dist`]              | `N_CrvExtendByParamDist`            |
//! | [`Curve::offset_max_2nd_deriv`]              | `N_CrvOffsetGetMax2ndDeriv`         |
//! | [`Curve::eval_unbounded_pts_and_derivs`], [`UnboundedEval`] | `N_CrvEvalUnboundedPtsAndDerivs` |
//! | [`Curve::remove_knots_deriv_constraints`], [`ConstrainedEnds`] | `N_CrvRemoveKnotsDerivConstraints` |
//! | [`Curve::refine_to_knot_vector`]             | `N_CrvRefineToKnotVector`           |
//! | [`Curve::sum_diff`], [`SumDiff`]             | `N_CrvSumDiffCrv`                   |
//! | [`Curve::non_rat_first_deriv`]               | `N_CrvNonRatGetFirstDeriv`          |
//! | [`Curve::rat_first_deriv`]                   | `N_CrvRatGetFirstDeriv`             |
//! | [`Curve::constant_multiply`]                 | `N_ConstantMultiplyCrv`             |
//! | [`Curve::linear_multiply`]                   | `N_LinearMultiplyCrv`               |
//! | [`Curve::constant_multiply_4d`]              | `N_ConstantMultiplyCrv4d`           |
//! | [`Curve::reparam_rat`]                       | `N_CrvReparamRat`                   |
//! | [`Curve::reparam_weights`]                   | `N_CrvReparamWeights`               |
//! | [`Curve::reparam_mult_knots`]                | `N_SrfReparamMultKnots`             |
//! | [`Curve::reparam_end_weights`]               | `N_SrfReparamWeights`               |
//! | [`G1_ANGLE_TOL`]                             | `g1tol` of `N_CrvEvalUnboundedPtsAndDerivs` |
//!
//! Deviations:
//! - Out-params become return values; errors are [`SolidError::Nlib`] values with the
//!   upstream `NL_*_ERR` number instead of a flag plus the global `NL_ERROR` record.
//!   The `NL_STACKS` arguments and the memory error paths have no counterpart.
//! - Routines that write into an output curve that is NULL, an existing curve or the
//!   input itself take `&self` and return a new [`Curve`] (in-place and out-of-place
//!   upstream calls produce the same curve). The routines upstream documents as
//!   in-place only (`N_ConstantMultiplyCrv`, `N_LinearMultiplyCrv`,
//!   `N_ConstantMultiplyCrv4d`, `N_CrvReparamWeights`, `N_SrfReparamWeights`) take
//!   `&mut self` and, on error, leave the curve modified as far as upstream got.
//! - `N_CrvSumDiffCrv` and `N_CrvReparamRat` temporarily (or, for `N_CrvReparamRat`,
//!   permanently) turn a non-rational input curve rational; the Rust methods work on
//!   copies and leave their inputs alone. The operator flag of `N_CrvSumDiffCrv`
//!   (`NL_PLUS` / `NL_MINUS`; anything else fails with `NL_CAL_ERR` after the work for
//!   rational curves) and the constraint flag of `N_CrvRemoveKnotsDerivConstraints`
//!   (`NL_NO` / `NL_START` / `NL_END` / `NL_BOTH`; any other value constrains nothing,
//!   as `NL_NO`) are the enums [`SumDiff`] and [`ConstrainedEnds`]. Upstream signed
//!   counts (`d` of `N_CrvNonRatGetFirstDeriv`, `der` of
//!   `N_CrvRemoveKnotsDerivConstraints`, `ndr`) are `usize`, so their negative-value
//!   paths cannot occur.
//! - `N_CrvEvalUnboundedPtsAndDerivs` keeps its extension curve `ecur` and closedness
//!   flag `gflg` in the caller's variables; [`UnboundedEval`] holds both. When an
//!   extension step fails, upstream leaves `ecur` as far as the in-place extension got;
//!   the port keeps the last complete extension. The periodic parameter adjustment of a
//!   G1-closed curve loops forever upstream when adding the parameter range no longer
//!   changes the parameter; the port fails with [`SolidError::NotConverged`] there.
//! - `N_LinearMultiplyCrv` ignores the error of `N_CrvGetGrevilleAbscissa` (degree 0,
//!   where it then reads an uninitialised value); the port propagates it.
//! - `N_CrvRefineToKnotVector` writes the new control points into a caller array
//!   (`Qw[0..=n+r+1]`); the Rust method returns them. Entries the algorithm does not
//!   write (none for knots strictly inside the knot range) are zero.
//! - Every array access is bounds checked: where upstream would read or write outside
//!   its arrays (malformed curves, knots to insert that are not sorted, results of
//!   different sizes in the sum of numerators), the routines return
//!   [`SolidError::InvalidInput`] instead. For every input upstream handles without
//!   undefined behaviour the results are the same, bit for bit.
//! - A degree above [`MAX_SHORT_DEGREE`](crate::nurbs::bezier::MAX_SHORT_DEGREE)
//!   (impossible upstream, where `NL_DEGREE` is a `short`) is
//!   [`SolidError::InvalidInput`], so index arithmetic and allocations sized from the
//!   degree cannot overflow.

// Index arithmetic follows upstream's signed `NL_INDEX` (`isize` here); every access
// goes through `get`/`set`, which reject negative or too-large indices.

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::basis_adv::{compatible_knot_array_mult, increase_knot_mult};
use crate::nurbs::basis_basic::PTOL;
use crate::nurbs::bezier::check_short_degree;
use crate::nurbs::crv_adv::p1::{Continuity, CurveEnd};
use crate::nurbs::crv_basic::{curve_from_num_and_denom, Curve};
use crate::nurbs::crv_geom::ToleranceKind;
use crate::nurbs::crv_nurb::greville_abscissa;
use crate::nurbs::geom::Interval;
use crate::nurbs::index::{get, oob, set, storage, sx, ux};
use crate::nurbs::knots::Side;
use crate::nurbs::math::{float_op_is_bad, CPoint, FloatOp, MODEL_TOL};

/// Upstream `g1tol` of `N_CrvEvalUnboundedPtsAndDerivs`: angular tolerance of the G1
/// closedness test that decides between periodic wrapping and extension.
pub const G1_ANGLE_TOL: f64 = 0.1;

/// Upstream `NL_UNITSPAN`.
const UNIT_SPAN: Interval = Interval::new(0.0, 1.0);

/// Which ends of a curve keep their end derivatives in
/// [`Curve::remove_knots_deriv_constraints`] (`NL_NO = 0`, `NL_START = 1`,
/// `NL_END = 2`, `NL_BOTH = 3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstrainedEnds {
    /// `NL_NO`: no constraint.
    None,
    /// `NL_START`: constrain the derivatives at the start point.
    Start,
    /// `NL_END`: constrain the derivatives at the end point.
    End,
    /// `NL_BOTH`: constrain both ends.
    Both,
}

impl ConstrainedEnds {
    /// The upstream flag value.
    #[must_use]
    pub const fn flag(self) -> i32 {
        match self {
            ConstrainedEnds::None => 0,
            ConstrainedEnds::Start => 1,
            ConstrainedEnds::End => 2,
            ConstrainedEnds::Both => 3,
        }
    }

    /// The constraint for an upstream flag value (any other value constrains nothing
    /// upstream, as `NL_NO`).
    #[must_use]
    pub const fn from_flag(flag: i32) -> Self {
        match flag {
            1 => ConstrainedEnds::Start,
            2 => ConstrainedEnds::End,
            3 => ConstrainedEnds::Both,
            _ => ConstrainedEnds::None,
        }
    }

    const fn start(self) -> bool {
        matches!(self, ConstrainedEnds::Start | ConstrainedEnds::Both)
    }

    const fn end(self) -> bool {
        matches!(self, ConstrainedEnds::End | ConstrainedEnds::Both)
    }
}

/// The operator of [`Curve::sum_diff`] (`NL_PLUS = 1`, `NL_MINUS = 2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SumDiff {
    /// `NL_PLUS`: the sum.
    Plus,
    /// `NL_MINUS`: the difference.
    Minus,
}

impl SumDiff {
    /// The upstream flag value.
    #[must_use]
    pub const fn flag(self) -> i32 {
        match self {
            SumDiff::Plus => 1,
            SumDiff::Minus => 2,
        }
    }

    /// The operator for an upstream flag value, `None` for any other value.
    #[must_use]
    pub const fn from_flag(flag: i32) -> Option<Self> {
        match flag {
            1 => Some(SumDiff::Plus),
            2 => Some(SumDiff::Minus),
            _ => None,
        }
    }

    fn apply(self, a: CPoint, b: CPoint) -> CPoint {
        match self {
            SumDiff::Plus => CPoint::sum(a, b),
            SumDiff::Minus => CPoint::diff(a, b),
        }
    }
}

/// State of [`Curve::eval_unbounded_pts_and_derivs`] across calls with one curve
/// (upstream's caller-held `ecur` and `gflg`). Start from [`UnboundedEval::default`]
/// (upstream: `N_CrvInitArrays(&ecur)`) and keep it unchanged between the calls.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UnboundedEval {
    /// The extended (or copied, for a G1-closed curve) curve once a parameter outside
    /// the curve's range was evaluated (upstream `ecur`), else `None`.
    pub ext: Option<Curve>,
    /// Whether the curve is G1 closed (upstream `gflg`); set when `ext` is created.
    pub g1_closed: bool,
}

impl Curve {
    /// Extends the curve at `end` by about the arc length `dist`
    /// (`N_CrvExtendByDist`): [`Curve::extend_by_dist_at_depth`] at depth 0.
    ///
    /// # Errors
    ///
    /// The errors of [`Curve::extend_by_dist_at_depth`].
    pub fn extend_by_dist(
        &self,
        dist: f64,
        kind: ToleranceKind,
        end: CurveEnd,
        cont: Continuity,
    ) -> Result<Curve> {
        self.extend_by_dist_at_depth(dist, kind, end, cont, 0)
    }

    /// Extends the curve at `end` to the parameter `par`
    /// (`N_CrvExtendByParamDist`): [`Curve::extend_by_param_dist_at_depth`] at depth 0.
    ///
    /// # Errors
    ///
    /// The errors of [`Curve::extend_by_param_dist_at_depth`].
    pub fn extend_by_param_dist(&self, par: f64, end: CurveEnd, cont: Continuity) -> Result<Curve> {
        self.extend_by_param_dist_at_depth(par, end, cont, 0)
    }

    /// An upper bound on the magnitude of the second derivative of the planar offset
    /// at distance `d` (plane normal `normal`) of this planar curve
    /// (`N_CrvOffsetGetMax2ndDeriv`): the largest `|O''|` of
    /// [`Curve::planar_offset_derivs`] over `10 p + 1` evenly spaced parameters of
    /// every Bezier segment.
    ///
    /// # Errors
    ///
    /// The errors of [`Curve::decompose_bez`] and [`Curve::planar_offset_derivs`].
    pub fn offset_max_2nd_deriv(&self, d: f64, normal: Vec3) -> Result<f64> {
        const OP: &str = "nurbs::crv_adv::p2::Curve::offset_max_2nd_deriv";

        // Get locals and decompose into Bezier segments.
        check_short_degree(self.degree(), OP)?;
        let p = sx(self.degree(), OP)?;
        let bez = self.decompose_bez()?;

        // Compute maximum derivative.
        let mut duu = 0.0;
        let mu = 10 * p;
        let mut u: Vec<f64> = storage(mu, OP)?;
        for (i, b) in bez.iter().enumerate() {
            let al = isize::from(i != 0);
            let bu = b.knots();
            let r = sx(b.m(), OP)?;
            let (b0, br) = (get(bu, 0, OP)?, get(bu, r, OP)?);
            let ui = (br - b0) / mu as f64;
            set(&mut u, 0, b0, OP)?;
            set(&mut u, mu, br, OP)?;
            for a in 1..mu {
                set(&mut u, a, b0 + a as f64 * ui, OP)?;
            }
            for a in al..=mu {
                let (_, od2) = b.planar_offset_derivs(get(&u, a, OP)?, d, normal, Side::Left)?;
                let muu = (od2.x * od2.x + od2.y * od2.y + od2.z * od2.z).sqrt();
                if muu > duu {
                    duu = muu;
                }
            }
        }
        Ok(duu)
    }

    /// The point and derivatives `D[0..=ndr]` at `u`, which may lie outside the
    /// parameter range (`N_CrvEvalUnboundedPtsAndDerivs`, left-sided).
    ///
    /// The first time `u` is out of range, the curve is copied into `state.ext` and
    /// tested for G1 closedness ([`MODEL_TOL`], [`G1_ANGLE_TOL`]). A G1-closed curve is
    /// evaluated periodically (`u` shifted by whole parameter ranges); any other curve
    /// is extended with continuity `cont` ([`Curve::extend_by_param_dist`]) in steps of
    /// at most `0.9` times the current parameter range until it covers `u`. Later
    /// calls evaluate `state.ext` once it exists, extending it further as needed.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotConverged`] when the periodic shift no longer changes `u`
    /// (upstream loops forever), and the errors of [`Curve::is_closed_continuity`],
    /// [`Curve::extend_by_param_dist`], [`Curve::eval`] (`ndr = 0`) and
    /// [`Curve::derivs`].
    pub fn eval_unbounded_pts_and_derivs(
        &self,
        u: f64,
        ndr: usize,
        cont: Continuity,
        state: &mut UnboundedEval,
    ) -> Result<Vec<Vec3>> {
        const OP: &str = "nurbs::crv_adv::p2::Curve::eval_unbounded_pts_and_derivs";
        let stuck = || SolidError::NotConverged {
            operation: OP,
            iterations: 0,
        };
        let mut u = u;

        // Assign pointer to curve to be evaluated and get u - bounds.
        let mut g1_flag = false;
        let (mut us, mut ue) = match &state.ext {
            None => self.param_bounds(),
            Some(e) => {
                g1_flag = state.g1_closed;
                e.param_bounds()
            }
        };

        // Check if u is in bounds.
        if u < us || u > ue {
            // Must extend or adjust periodic parameter.
            if state.ext.is_none() {
                self.copy_into(&mut state.ext)?;
                let tols = [MODEL_TOL, G1_ANGLE_TOL];
                state.g1_closed = self.is_closed_continuity(1, &tols)?;
                g1_flag = state.g1_closed;
            }
            let ext = state.ext.as_mut().ok_or_else(|| oob(OP))?;

            // Now extend or adjust parm.
            if g1_flag {
                while u < us {
                    let v = ue - us + u;
                    if v == u {
                        return Err(stuck());
                    }
                    u = v;
                }
                while u > ue {
                    let v = us - ue + u;
                    if v == u {
                        return Err(stuck());
                    }
                    u = v;
                }
            } else if u < us {
                loop {
                    let uu = if us - u <= 0.9 * (ue - us) {
                        u
                    } else {
                        us - (0.9 * (ue - us))
                    };
                    *ext = ext.extend_by_param_dist(uu, CurveEnd::Start, cont)?;
                    us = uu;
                    if u >= us {
                        break;
                    }
                }
            } else {
                loop {
                    let uu = if u - ue <= 0.9 * (ue - us) {
                        u
                    } else {
                        ue + (0.9 * (ue - us))
                    };
                    *ext = ext.extend_by_param_dist(uu, CurveEnd::End, cont)?;
                    ue = uu;
                    if u <= ue {
                        break;
                    }
                }
            }
        }

        // Now evaluate.
        let cur = state.ext.as_ref().unwrap_or(self);
        if ndr == 0 {
            Ok(vec![cur.eval(u, Side::Left)?])
        } else {
            cur.derivs(u, Side::Left, ndr)
        }
    }

    /// Removes every removable knot within `tol` as [`Curve::remove_knots`], keeping
    /// the 1st to `der`-th derivatives at the ends selected by `whr`
    /// (`N_CrvRemoveKnotsDerivConstraints`): a knot `U[r]` is not removed when
    /// `r <= p + der` (start constrained) or `r >= n - der + 1` (end constrained, `n`
    /// the current highest control point index).
    ///
    /// # Errors
    ///
    /// As [`Curve::remove_knots`].
    pub fn remove_knots_deriv_constraints(
        &self,
        tol: f64,
        whr: ConstrainedEnds,
        der: usize,
    ) -> Result<Curve> {
        const OP: &str = "nurbs::crv_adv::p2::Curve::remove_knots_deriv_constraints";
        check_short_degree(self.degree(), OP)?;
        let p = sx(self.degree(), OP)?;
        let der = sx(der, OP)?;
        self.remove_knots_constrained(
            tol,
            // Upstream's end test is `r >= n - der + 1` (the same for integers). A
            // `p + der` past `isize::MAX` exceeds every index `r`, which the saturating
            // sum keeps.
            |r, n| (whr.start() && r <= p.saturating_add(der)) || (whr.end() && r > n - der),
            OP,
        )
    }

    /// The control points `Qw[0..=n+r+1]` of the curve refined with the knots
    /// `x[0..=r]` (`N_CrvRefineToKnotVector`, knot refinement computing control points
    /// only). The knots must be non-decreasing and strictly inside the knot range.
    ///
    /// # Errors
    ///
    /// [`NlibCode::Input`] for an empty `x`, `x[0] <= U[0]` or `x[r] >= U[m]`, and the
    /// errors of the knot span search.
    pub fn refine_to_knot_vector(&self, x: &[f64]) -> Result<Vec<CPoint>> {
        const OP: &str = "nurbs::crv_adv::p2::Curve::refine_to_knot_vector";

        // Get local notation.
        let pw = self.cpts();
        let uk = self.knots();
        let pu = self.degree();
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let n = sx(self.n(), OP)?;
        let m = sx(self.m(), OP)?;
        let knt = self.knot_vector()?;

        // Check input parameters.
        let r = sx(x.len(), OP)? - 1;
        if r < 0 {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }
        if get(x, 0, OP)? <= get(uk, 0, OP)? {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }
        if get(x, r, OP)? >= get(uk, m, OP)? {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }

        // Find knot spans.
        let a = sx(knt.find_span(pu, get(x, 0, OP)?, Side::Left)?, OP)?;
        let b = sx(knt.find_span(pu, get(x, r, OP)?, Side::Left)?, OP)? + 1;

        // Compute new control points.
        let mut uq: Vec<f64> = storage(m + r + 1, OP)?;
        let mut qw: Vec<CPoint> = storage(n + r + 1, OP)?;
        for j in 0..=a {
            set(&mut uq, j, get(uk, j, OP)?, OP)?;
        }
        for j in b + p..=m {
            set(&mut uq, j + r + 1, get(uk, j, OP)?, OP)?;
        }
        for j in 0..=a - p {
            set(&mut qw, j, get(pw, j, OP)?, OP)?;
        }
        for j in b - 1..=n {
            set(&mut qw, j + r + 1, get(pw, j, OP)?, OP)?;
        }

        let mut i = b + p - 1;
        let mut k = b + p + r;
        for j in (0..=r).rev() {
            let xj = get(x, j, OP)?;
            while xj <= get(uk, i, OP)? && i > a {
                set(&mut qw, k - p - 1, get(pw, i - p - 1, OP)?, OP)?;
                set(&mut uq, k, get(uk, i, OP)?, OP)?;
                k -= 1;
                i -= 1;
            }
            let c = get(&qw, k - p, OP)?;
            set(&mut qw, k - p - 1, c, OP)?;
            for l in 1..=p {
                let t = k - p + l;
                let mut alf = get(&uq, k + l, OP)? - xj;
                if alf.abs() <= 0.0 {
                    let c = get(&qw, t, OP)?;
                    set(&mut qw, t - 1, c, OP)?;
                } else {
                    alf /= get(&uq, k + l, OP)? - get(uk, i - p + l, OP)?;
                    let oma = 1.0 - alf;
                    let c = CPoint::combine2(alf, get(&qw, t - 1, OP)?, oma, get(&qw, t, OP)?);
                    set(&mut qw, t - 1, c, OP)?;
                }
            }
            set(&mut uq, k, xj, OP)?;
            k -= 1;
        }
        Ok(qw)
    }

    /// The sum or difference of this curve and `other` (`N_CrvSumDiffCrv`).
    ///
    /// Non-rational curves are degree elevated to the common degree, their knot
    /// vectors merged (the shorter parameter range is rescaled to the longer one) and
    /// the control points added. When either curve is rational the result is
    /// `(N_p D_q +/- N_q D_p) / (D_p D_q)`, built with the curve function products.
    ///
    /// # Errors
    ///
    /// The errors of [`Curve::elevate_degree`], [`Curve::refine`] and
    /// [`compatible_knot_array_mult`] (non-rational), of the curve function products
    /// and of [`curve_from_num_and_denom`] (rational).
    pub fn sum_diff(&self, other: &Curve, opr: SumDiff) -> Result<Curve> {
        const OP: &str = "nurbs::crv_adv::p2::Curve::sum_diff";

        // Get rational flags.
        let rat_p = self.is_rational();
        let rat_q = other.is_rational();

        if rat_p || rat_q {
            // Make non-rational curve rational.
            let mut cur_p = self.clone();
            let mut cur_q = other.clone();
            if !rat_p {
                cur_p.non_rat_to_rat();
            }
            if !rat_q {
                cur_q.non_rat_to_rat();
            }

            // Extract numerators and denominators.
            let (mut num_p, mut den_p) = (None, None);
            cur_p.num_and_denom(&mut num_p, &mut den_p)?;
            let (mut num_q, mut den_q) = (None, None);
            cur_q.num_and_denom(&mut num_q, &mut den_q)?;
            let mut num_p = num_p.ok_or_else(|| oob(OP))?;
            let mut den_p = den_p.ok_or_else(|| oob(OP))?;
            let mut num_q = num_q.ok_or_else(|| oob(OP))?;
            let mut den_q = den_q.ok_or_else(|| oob(OP))?;

            // Compute product of denominators.
            let den = den_p.multiply_func(&mut den_q)?;

            // Compute terms in the numerator.
            let mut num = den_q.multiply_crv(&mut num_p)?;
            let tmp = den_p.multiply_crv(&mut num_q)?;

            // Add terms in the numerator.
            let n = sx(tmp.n(), OP)?;
            let qw = tmp.cpts();
            let pw = num.cpts_mut();
            for i in 0..=n {
                let c = opr.apply(get(pw, i, OP)?, get(qw, i, OP)?);
                set(pw, i, c, OP)?;
            }

            // Create output curve.
            let mut out = None;
            curve_from_num_and_denom(&num, Some(&den), &mut out)?;
            return out.ok_or_else(|| oob(OP));
        }

        // Create working curves.
        let mut cur_pw = None;
        self.copy_into(&mut cur_pw)?;
        let mut cur_pw = cur_pw.ok_or_else(|| oob(OP))?;
        let mut cur_qw = None;
        other.copy_into(&mut cur_qw)?;
        let mut cur_qw = cur_qw.ok_or_else(|| oob(OP))?;

        // Get local notation (`n`, `m` are those of the curve read last).
        let mut p = cur_pw.degree();
        let q = cur_qw.degree();
        let (mut n, mut m) = (cur_qw.n(), cur_qw.m());

        // Degree elevate.
        if p != q {
            if p < q {
                cur_pw = cur_pw.elevate_degree(q - p)?;
                p = cur_pw.degree();
                (n, m) = (cur_pw.n(), cur_pw.m());
            } else {
                cur_qw = cur_qw.elevate_degree(p - q)?;
                (n, m) = (cur_qw.n(), cur_qw.m());
            }
        }

        // Refine curves.
        let (kxp, kxq) = compatible_knot_array_mult(cur_pw.knots_mut(), cur_qw.knots_mut(), p)?;
        if !kxp.is_empty() {
            cur_pw = cur_pw.refine(&kxp)?;
            (n, m) = (cur_pw.n(), cur_pw.m());
        }
        if !kxq.is_empty() {
            cur_qw = cur_qw.refine(&kxq)?;
            (n, m) = (cur_qw.n(), cur_qw.m());
        }

        // Compute output curve.
        let (n, m) = (sx(n, OP)?, sx(m, OP)?);
        let pw = cur_pw.cpts();
        let qw = cur_qw.cpts();
        let up = cur_pw.knots();
        let mut rw: Vec<CPoint> = storage(n, OP)?;
        let mut ur: Vec<f64> = storage(m, OP)?;
        for i in 0..=n {
            set(&mut rw, i, opr.apply(get(pw, i, OP)?, get(qw, i, OP)?), OP)?;
        }
        for i in 0..=m {
            set(&mut ur, i, get(up, i, OP)?, OP)?;
        }
        Curve::with_indices(rw, ux(n, OP)?, p, ur, ux(m, OP)?)
    }

    /// The `d`-th derivative curve of a non-rational curve, of degree `p - d`
    /// (`N_CrvNonRatGetFirstDeriv`; despite the name any `d <= p`).
    ///
    /// # Errors
    ///
    /// [`NlibCode::Input`] for `d > p` or a rational curve, [`NlibCode::Derivative`]
    /// when a knot difference `U[i+p+1] - U[i+k]` is zero, and
    /// [`NlibCode::Numerical`] when the quotient `(p-k+1) / (U[i+p+1] - U[i+k])` would
    /// overflow or underflow.
    pub fn non_rat_first_deriv(&self, d: usize) -> Result<Curve> {
        const OP: &str = "nurbs::crv_adv::p2::Curve::non_rat_first_deriv";

        // Get locals and check error.
        let pw = self.cpts();
        let uk = self.knots();
        check_short_degree(self.degree(), OP)?;
        let p = sx(self.degree(), OP)?;
        let n = sx(self.n(), OP)?;
        let m = sx(self.m(), OP)?;
        let d = sx(d, OP)?;
        if d > p {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }
        if self.is_rational() {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }

        // See if memory is needed.
        let nd = n - d;
        let pd = p - d;
        let md = nd + pd + 1;
        let mut dw: Vec<CPoint> = storage(nd, OP)?;
        let mut v: Vec<f64> = storage(md, OP)?;

        // Initialize control points.
        let mut tw: Vec<CPoint> = storage(n, OP)?;
        for i in 0..=n {
            set(&mut tw, i, get(pw, i, OP)?, OP)?;
        }

        // Compute control points of derivative curve.
        for k in 1..=d {
            for i in 0..=n - k {
                let (hi, lo) = (get(uk, i + p + 1, OP)?, get(uk, i + k, OP)?);
                if hi == lo {
                    return Err(SolidError::nlib(OP, NlibCode::Derivative));
                }
                let num = p as f64 - k as f64 + 1.0;
                if float_op_is_bad(num, hi - lo, FloatOp::Division) {
                    return Err(SolidError::nlib(OP, NlibCode::Numerical));
                }
                let alf = num / (hi - lo);
                let c = CPoint::combine2(alf, get(&tw, i + 1, OP)?, -alf, get(&tw, i, OP)?);
                set(&mut tw, i, c, OP)?;
            }
        }
        for i in 0..=n - d {
            set(&mut dw, i, get(&tw, i, OP)?, OP)?;
        }

        // Compute the knot vector.
        let mut j: isize = -1;
        let u0 = get(uk, 0, OP)?;
        let um = get(uk, m, OP)?;
        for _ in 0..=p - d {
            j += 1;
            set(&mut v, j, u0, OP)?;
        }
        for i in p + 1..=n {
            j += 1;
            set(&mut v, j, get(uk, i, OP)?, OP)?;
        }
        for _ in 0..=p - d {
            j += 1;
            set(&mut v, j, um, OP)?;
        }
        Curve::with_indices(dw, ux(nd, OP)?, ux(pd, OP)?, v, ux(md, OP)?)
    }

    /// The first derivative curve (`N_CrvRatGetFirstDeriv`): for a rational curve
    /// `(N/D)' = (N' D - N D') / D^2`, the numerator degree elevated once to match the
    /// squared denominator (whose interior knot multiplicities are raised by one);
    /// for a non-rational curve [`Curve::non_rat_first_deriv`] with `d = 1`.
    ///
    /// # Errors
    ///
    /// The errors of the curve function products and derivative, of
    /// [`increase_knot_mult`], [`Curve::non_rat_first_deriv`], [`Curve::sum_diff`],
    /// [`Curve::elevate_degree`] and [`curve_from_num_and_denom`].
    pub fn rat_first_deriv(&self) -> Result<Curve> {
        const OP: &str = "nurbs::crv_adv::p2::Curve::rat_first_deriv";

        // Check if non-rational.
        if !self.is_rational() {
            return self.non_rat_first_deriv(1);
        }

        // Extract numerator and denominator.
        let (mut n, mut d) = (None, None);
        self.num_and_denom(&mut n, &mut d)?;
        let mut n = n.ok_or_else(|| oob(OP))?;
        let mut d = d.ok_or_else(|| oob(OP))?;

        // Compute denominator of output curve (`d * d`: upstream passes the same
        // function twice).
        let mut d_copy = d.clone();
        let mut den = d.multiply_func(&mut d_copy)?;
        let knx = increase_knot_mult(den.knots(), den.degree(), 1)?;
        if !knx.is_empty() {
            den = den.refine(&knx)?;
        }

        // Compute terms in the numerator.
        let mut np = n.non_rat_first_deriv(1)?;
        let mut dp = d.deriv(1)?;
        let npd = d.multiply_crv(&mut np)?;
        let ndp = dp.multiply_crv(&mut n)?;

        // Add terms in the numerator.
        let num = npd.sum_diff(&ndp, SumDiff::Minus)?;

        // Degree elevate numerator.
        let num = num.elevate_degree(1)?;

        // Create output curve.
        let mut out = None;
        curve_from_num_and_denom(&num, Some(&den), &mut out)?;
        out.ok_or_else(|| oob(OP))
    }

    /// Multiplies `x, y, z` of every control point by `alpha`, in place
    /// (`N_ConstantMultiplyCrv`).
    pub fn constant_multiply(&mut self, alpha: f64) {
        for pw in self.cpts_mut() {
            *pw = pw.scale_xyz(alpha);
        }
    }

    /// Multiplies `x, y, z` of every control point by the linear function from `s0`
    /// at the start to `s1` at the end, sampled at the control point's Greville
    /// abscissa, in place (`N_LinearMultiplyCrv`). Scale factors closer than
    /// [`PTOL`] use their mean as a constant.
    ///
    /// # Errors
    ///
    /// The errors of [`greville_abscissa`] (degree 0).
    pub fn linear_multiply(&mut self, s0: f64, s1: f64) -> Result<()> {
        const OP: &str = "nurbs::crv_adv::p2::Curve::linear_multiply";
        if (s0 - s1).abs() < PTOL {
            let s = (s0 + s1) / 2.0;
            self.constant_multiply(s);
            return Ok(());
        }

        // Different scale factors, have to interpolate.
        let p = self.degree();
        let knots = self.knots().to_vec();
        let m = sx(self.m(), OP)?;
        let t0 = get(&knots, 0, OP)?;
        let t1 = get(&knots, m, OP)?;
        for (ii, pw) in self.cpts_mut().iter_mut().enumerate() {
            let t = greville_abscissa(&knots, p, ii)?;
            let frac = (t - t0) / (t1 - t0);
            let s = s0 + frac * (s1 - s0);
            *pw = pw.scale_xyz(s);
        }
        Ok(())
    }

    /// Multiplies all four homogeneous coordinates of every control point by
    /// `alpha`, in place (`N_ConstantMultiplyCrv4d`).
    pub fn constant_multiply_4d(&mut self, alpha: f64) {
        for pw in self.cpts_mut() {
            *pw = pw.scale(alpha);
        }
    }

    /// Reparametrises the curve with the rational linear function
    /// `s = (alf u + bet) / (gam u + del)` (`N_CrvReparamRat`); the result is rational.
    ///
    /// # Errors
    ///
    /// [`NlibCode::Input`] when `alf del - bet gam < PTOL` or a new knot is not
    /// computable.
    pub fn reparam_rat(&self, alf: f64, bet: f64, gam: f64, del: f64) -> Result<Curve> {
        let mut out = None;
        self.copy_into(&mut out)?;
        let mut cur = out.ok_or_else(|| oob("nurbs::crv_adv::p2::Curve::reparam_rat"))?;
        cur.reparam_rat_in_place(alf, bet, gam, del)?;
        Ok(cur)
    }

    /// `N_CrvReparamRat` with the output curve equal to the input.
    fn reparam_rat_in_place(&mut self, alf: f64, bet: f64, gam: f64, del: f64) -> Result<()> {
        const OP: &str = "nurbs::crv_adv::p2::Curve::reparam_rat";

        // Check coefficients.
        if (alf * del - bet * gam) < PTOL {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }

        // Get local notation.
        if !self.is_rational() {
            self.non_rat_to_rat();
        }
        check_short_degree(self.degree(), OP)?;
        let p = sx(self.degree(), OP)?;
        let n = sx(self.n(), OP)?;
        let m = sx(self.m(), OP)?;

        // Get new knots.
        let uq = self.knots_mut();
        for i in 0..=m {
            let ui = get(uq, i, OP)?;
            let num = alf * ui + bet;
            let den = gam * ui + del;
            if float_op_is_bad(num, den, FloatOp::Division) {
                return Err(SolidError::nlib(OP, NlibCode::Input));
            }
            set(uq, i, num / den, OP)?;
        }
        let uq = self.knots().to_vec();

        // Get new weighted control points.
        let qw = self.cpts_mut();
        let mut w: Vec<f64> = storage(n, OP)?;
        for i in 0..=n {
            let c = get(qw, i, OP)?;
            set(&mut w, i, c.w, OP)?;
            let fact = 1.0 / c.w;
            set(qw, i, c.scale(fact), OP)?;
        }
        for i in 0..=n {
            let mut fact = 1.0;
            for j in 1..=p {
                fact *= gam * get(&uq, i + j, OP)? - alf;
            }
            if fact < 0.0 {
                fact = -fact;
            }
            let wi = get(&w, i, OP)? * fact;
            set(&mut w, i, wi, OP)?;
            set(qw, i, get(qw, i, OP)?.scale(wi), OP)?;
        }
        Ok(())
    }

    /// Reparametrises a rational curve onto `[c, d]` so that its end weights become
    /// equal, then scales all control points so that the mean end weight is `w`, in
    /// place (`N_CrvReparamWeights`). A non-rational curve is left alone.
    ///
    /// # Errors
    ///
    /// The errors of [`Curve::reparam_rat`] (the curve is left modified as far as it
    /// got).
    pub fn reparam_weights(&mut self, w: f64, c: f64, d: f64) -> Result<()> {
        const OP: &str = "nurbs::crv_adv::p2::Curve::reparam_weights";

        // Check rationality.
        if !self.is_rational() {
            return Ok(());
        }

        // Get local notation.
        let n = sx(self.n(), OP)?;
        let m = sx(self.m(), OP)?;
        let p = self.degree();

        // Reparametrize curve.
        let (mut w0, mut wn) = (get(self.cpts(), 0, OP)?.w, get(self.cpts(), n, OP)?.w);
        let exp = 1.0 / p as f64;
        w0 = w0.powf(exp);
        wn = wn.powf(exp);
        let (u0, um) = (get(self.knots(), 0, OP)?, get(self.knots(), m, OP)?);
        let alf = wn * d - w0 * c;
        let bet = w0 * c * um - wn * d * u0;
        let gam = wn - w0;
        let del = w0 * um - wn * u0;
        self.reparam_rat_in_place(alf, bet, gam, del)?;

        // Re-weight control points.
        let (w0, wn) = (get(self.cpts(), 0, OP)?.w, get(self.cpts(), n, OP)?.w);
        let f = (2.0 * w) / (w0 + wn);
        self.scale_cpts(f);
        Ok(())
    }

    /// Reparametrises the curve by arc length at its knots of multiplicity at least
    /// the degree (`N_SrfReparamMultKnots`): each such knot moves to the arc length up
    /// to it (computed with the relative tolerance `tol`), the knots between two of
    /// them are rescaled linearly, and the curve runs over `[0, length]`. The control
    /// points are unchanged.
    ///
    /// # Errors
    ///
    /// The errors of [`Curve::arc_length`].
    pub fn reparam_mult_knots(&self, tol: f64) -> Result<Curve> {
        const OP: &str = "nurbs::crv_adv::p2::Curve::reparam_mult_knots";

        // Get local notation.
        let pw = self.cpts();
        let up = self.knots();
        let pu = self.degree();
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let n = sx(self.n(), OP)?;
        let m = sx(self.m(), OP)?;
        let mut uq: Vec<f64> = storage(m, OP)?;

        // Get the total arc length.
        let len = self.arc_length(
            get(up, 0, OP)?,
            get(up, m, OP)?,
            tol,
            ToleranceKind::Relative,
        )?;

        // Place each knot of full multiplicity at the arc length there and rescale the
        // knots between it and the previous one.
        let mut ua = get(up, 0, OP)?;
        let mut ul = 0.0;
        let mut ur = 0.0;
        let mut jj = p;
        let mut kk = p + 1;
        let mut oldmlt: isize = -1;
        for ii in 0..=p {
            set(&mut uq, ii, 0.0, OP)?;
        }
        while kk < m {
            let ii = kk;

            // Check multiplicity of this knot.
            while kk < m && get(up, kk, OP)? == get(up, kk + 1, OP)? {
                kk += 1;
            }
            let mlt = kk - ii + 1;
            if mlt >= p {
                let ub = get(up, kk, OP)?;
                let d = self.arc_length(ua, ub, tol, ToleranceKind::Relative)?;
                ur += d;
                if kk == m {
                    ur = len;
                }
                let fact = (ur - ul) / (ub - ua);
                for ii in jj + 1..=kk - mlt {
                    set(&mut uq, ii, ul + fact * (get(up, ii, OP)? - ua), OP)?;
                }
                if oldmlt > 0 {
                    for ii in jj - oldmlt + 1..=jj {
                        set(&mut uq, ii, ul, OP)?;
                    }
                }
                oldmlt = mlt;
                ua = ub;
                ul = ur;
                jj = kk;
            }
            kk += 1;
        }
        for ii in 0..=p {
            set(&mut uq, n + ii + 1, len, OP)?;
        }
        Curve::with_indices(pw.to_vec(), ux(n, OP)?, pu, uq, ux(m, OP)?)
    }

    /// Reparametrises the curve (rational afterwards) with a rational linear function
    /// so that its end weights become `wt0` and `wtn`, keeping the parameter range, in
    /// place (`N_SrfReparamWeights`).
    ///
    /// # Errors
    ///
    /// [`NlibCode::Input`] when the reparametrisation is degenerate or a new knot is
    /// not computable, [`NlibCode::Weight`] for a weight whose reciprocal is not
    /// computable, [`NlibCode::Numerical`] when a new weight is not computable, and the
    /// errors of [`Curve::reparam_to_interval`]. The curve is left modified as far as
    /// upstream got.
    pub fn reparam_end_weights(&mut self, wt0: f64, wtn: f64) -> Result<()> {
        const OP: &str = "nurbs::crv_adv::p2::Curve::reparam_end_weights";

        // Check rationality.
        if !self.is_rational() {
            self.non_rat_to_rat();
        }

        // Get local notation and rescale span.
        let n = sx(self.n(), OP)?;
        let m = sx(self.m(), OP)?;
        let pu = self.degree();
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let old_span = Interval::new(get(self.knots(), 0, OP)?, get(self.knots(), m, OP)?);
        self.reparam_to_interval(UNIT_SPAN)?;

        // Reparametrize curve.
        let (mut w0, mut wn) = (get(self.cpts(), 0, OP)?.w, get(self.cpts(), n, OP)?.w);
        let exp = 1.0 / pu as f64;
        w0 = (w0 / wt0).powf(exp);
        wn = (wn / wtn).powf(exp);
        let alf = wn;
        let bet = 0.0;
        let gam = wn - w0;
        let del = w0;
        if (alf * del - bet * gam) < PTOL {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }

        let uk = self.knots().to_vec();
        let pw = self.cpts_mut();
        let mut w: Vec<f64> = storage(n, OP)?;
        for i in 0..=n {
            let c = get(pw, i, OP)?;
            set(&mut w, i, c.w, OP)?;
            if float_op_is_bad(1.0, c.w, FloatOp::Division) {
                return Err(SolidError::nlib(OP, NlibCode::Weight));
            }
            let fact = 1.0 / c.w;
            set(pw, i, c.scale(fact), OP)?;
        }
        set(pw, 0, get(pw, 0, OP)?.scale(wt0), OP)?;
        for i in 1..n {
            let mut fact = 1.0;
            for j in 1..=p {
                fact *= gam * get(&uk, i + j, OP)? + del;
            }
            if fact < 0.0 {
                fact = -fact;
            }
            let wi = get(&w, i, OP)?;
            if float_op_is_bad(wi, fact, FloatOp::Division) {
                return Err(SolidError::nlib(OP, NlibCode::Numerical));
            }
            let wi = wi / fact;
            set(&mut w, i, wi, OP)?;
            set(pw, i, get(pw, i, OP)?.scale(wi), OP)?;
        }
        set(pw, n, get(pw, n, OP)?.scale(wtn), OP)?;

        let mut c0 = get(pw, 0, OP)?;
        c0.w = wt0;
        set(pw, 0, c0, OP)?;
        let mut cn = get(pw, n, OP)?;
        cn.w = wtn;
        set(pw, n, cn, OP)?;

        let uq = self.knots_mut();
        for i in 0..=m {
            let ui = get(uq, i, OP)?;
            let num = alf * ui + bet;
            let den = gam * ui + del;
            if float_op_is_bad(num, den, FloatOp::Division) {
                return Err(SolidError::nlib(OP, NlibCode::Input));
            }
            set(uq, i, num / den, OP)?;
        }

        self.reparam_to_interval(old_span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::{NOW, NOZ};

    fn crv(pts: &[[f64; 4]], p: usize, u: &[f64]) -> Curve {
        let pw = pts
            .iter()
            .map(|&[x, y, z, w]| CPoint::new(x, y, z, w))
            .collect();
        Curve::new(pw, p, u.to_vec()).unwrap()
    }

    fn cubic() -> Curve {
        crv(
            &[
                [0.0, 0.0, 0.0, NOW],
                [1.0, 2.0, 0.0, NOW],
                [3.0, 2.0, 1.0, NOW],
                [4.0, 0.0, 1.0, NOW],
                [6.0, -1.0, 0.0, NOW],
            ],
            3,
            &[0.0, 0.0, 0.0, 0.0, 0.4, 1.0, 1.0, 1.0, 1.0],
        )
    }

    fn quarter_circle() -> Curve {
        let w = std::f64::consts::FRAC_1_SQRT_2;
        crv(
            &[[1.0, 0.0, NOZ, 1.0], [w, w, NOZ, w], [0.0, 1.0, NOZ, 1.0]],
            2,
            &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        )
    }

    fn pt(c: &Curve, u: f64) -> Vec3 {
        c.eval(u, Side::Left).unwrap()
    }

    fn close(a: Vec3, b: Vec3, tol: f64) -> bool {
        (a.x - b.x).abs() < tol && (a.y - b.y).abs() < tol && (a.z - b.z).abs() < tol
    }

    #[test]
    fn non_rat_first_deriv_matches_derivs() {
        let c = cubic();
        let d = c.non_rat_first_deriv(1).unwrap();
        assert_eq!(d.degree(), 2);
        for k in 0..=10 {
            let u = f64::from(k) / 10.0;
            let want = c.derivs(u, Side::Left, 1).unwrap()[1];
            assert!(close(pt(&d, u), want, 1e-12));
        }
        assert!(c.non_rat_first_deriv(4).is_err());
        assert!(quarter_circle().non_rat_first_deriv(1).is_err());
    }

    #[test]
    fn rat_first_deriv_matches_derivs() {
        let c = quarter_circle();
        let d = c.rat_first_deriv().unwrap();
        for k in 0..=8 {
            let u = f64::from(k) / 8.0;
            let want = c.derivs(u, Side::Left, 1).unwrap()[1];
            assert!(close(pt(&d, u), want, 1e-9), "{u}");
        }
    }

    #[test]
    fn sum_and_difference() {
        let a = cubic();
        let mut b = cubic();
        b.constant_multiply(2.0);
        let s = a.sum_diff(&b, SumDiff::Plus).unwrap();
        let d = a.sum_diff(&b, SumDiff::Minus).unwrap();
        for k in 0..=5 {
            let u = f64::from(k) / 5.0;
            let (pa, pb) = (pt(&a, u), pt(&b, u));
            assert!(close(pt(&s, u), pa + pb, 1e-12));
            assert!(close(pt(&d, u), pa - pb, 1e-12));
        }
        let r = quarter_circle()
            .sum_diff(&quarter_circle(), SumDiff::Plus)
            .unwrap();
        let p = pt(&r, 0.3);
        let q = pt(&quarter_circle(), 0.3);
        assert!((p.x - 2.0 * q.x).abs() < 1e-12 && (p.y - 2.0 * q.y).abs() < 1e-12);
    }

    #[test]
    fn refine_to_knot_vector_keeps_curve() {
        let c = cubic();
        let x = [0.2, 0.4, 0.7];
        let qw = c.refine_to_knot_vector(&x).unwrap();
        assert_eq!(qw.len(), c.n() + x.len() + 1);
        let r = c.refine(&x).unwrap();
        assert_eq!(qw, r.cpts());
        assert!(c.refine_to_knot_vector(&[]).is_err());
        assert!(c.refine_to_knot_vector(&[0.0]).is_err());
        assert!(c.refine_to_knot_vector(&[1.0]).is_err());
    }

    #[test]
    fn reparam_rat_keeps_shape() {
        let c = cubic();
        let r = c.reparam_rat(2.0, 0.0, 1.0, 1.0).unwrap();
        assert!(r.is_rational());
        // s = 2u / (u + 1): u = 0.5 -> s = 2/3.
        assert!(close(pt(&r, 2.0 / 3.0), pt(&c, 0.5), 1e-12));
        assert!(c.reparam_rat(0.0, 1.0, 1.0, 0.0).is_err());
    }

    #[test]
    fn reparam_weights_equalises_end_weights() {
        let mut c = crv(
            &[
                [0.0, 0.0, 0.0, 1.0],
                [2.0, 4.0, 0.0, 2.0],
                [6.0, 0.0, 0.0, 3.0],
            ],
            2,
            &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        );
        let before = pt(&c, 0.0);
        c.reparam_weights(1.0, 0.0, 1.0).unwrap();
        let (w0, wn) = (c.cpts()[0].w, c.cpts()[2].w);
        assert!((w0 - wn).abs() < 1e-12 && (w0 - 1.0).abs() < 1e-12);
        assert!(close(pt(&c, 0.0), before, 1e-12));

        let mut nr = cubic();
        nr.reparam_weights(1.0, 0.0, 1.0).unwrap();
        assert_eq!(nr, cubic());
    }

    #[test]
    fn reparam_end_weights_sets_weights() {
        let mut c = quarter_circle();
        let mid = pt(&c, 0.5);
        c.reparam_end_weights(2.0, 3.0).unwrap();
        assert_eq!(c.cpts()[0].w, 2.0);
        assert_eq!(c.cpts()[2].w, 3.0);
        assert_eq!(c.param_bounds(), (0.0, 1.0));
        // Same curve, other parametrisation: the old midpoint is still on it.
        let mut best = f64::MAX;
        for k in 0..=1000 {
            let p = pt(&c, f64::from(k) / 1000.0);
            best = best.min((p - mid).length());
        }
        assert!(best < 2e-3);
    }

    #[test]
    fn reparam_mult_knots_uses_arc_length() {
        // Two straight Bezier pieces of lengths 1 and 3 with a double knot at 0.5.
        let c = crv(
            &[
                [0.0, 0.0, 0.0, NOW],
                [0.5, 0.0, 0.0, NOW],
                [1.0, 0.0, 0.0, NOW],
                [2.5, 0.0, 0.0, NOW],
                [4.0, 0.0, 0.0, NOW],
            ],
            2,
            &[0.0, 0.0, 0.0, 0.5, 0.5, 1.0, 1.0, 1.0],
        );
        let r = c.reparam_mult_knots(1e-6).unwrap();
        let u = r.knots();
        assert!((u[3] - 1.0).abs() < 1e-9 && (u[4] - 1.0).abs() < 1e-9);
        assert!((u[7] - 4.0).abs() < 1e-9);
        assert_eq!(r.cpts(), c.cpts());
    }

    #[test]
    fn multiply_routines() {
        let mut c = cubic();
        c.constant_multiply(3.0);
        assert_eq!(c.cpts()[1], CPoint::new(3.0, 6.0, 0.0, NOW));
        let mut c = quarter_circle();
        c.constant_multiply_4d(2.0);
        assert_eq!(c.cpts()[0], CPoint::new(2.0, 0.0, NOZ, 2.0));
        let mut c = cubic();
        c.linear_multiply(1.0, 1.0).unwrap();
        assert_eq!(c, cubic());
        c.linear_multiply(0.0, 2.0).unwrap();
        assert_eq!(c.cpts()[0], CPoint::new(0.0, 0.0, 0.0, NOW));
        assert_eq!(c.cpts()[4], CPoint::new(12.0, -2.0, 0.0, NOW));
    }

    #[test]
    fn unbounded_eval_extends_and_wraps() {
        let c = cubic();
        let mut st = UnboundedEval::default();
        let inside = c
            .eval_unbounded_pts_and_derivs(0.3, 1, Continuity::G1, &mut st)
            .unwrap();
        assert!(st.ext.is_none());
        assert_eq!(inside, c.derivs(0.3, Side::Left, 1).unwrap());
        let out = c
            .eval_unbounded_pts_and_derivs(2.5, 0, Continuity::G1, &mut st)
            .unwrap();
        assert_eq!(out.len(), 1);
        let e = st.ext.as_ref().unwrap();
        assert!(e.param_bounds().1 >= 2.5);
        assert!(!st.g1_closed);

        // A closed square polygon of degree 1 is G1 closed? No: corners. A full
        // circle is.
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let circle = crv(
            &[
                [1.0, 0.0, NOZ, 1.0],
                [w, w, NOZ, w],
                [0.0, 1.0, NOZ, 1.0],
                [-w, w, NOZ, w],
                [-1.0, 0.0, NOZ, 1.0],
                [-w, -w, NOZ, w],
                [0.0, -1.0, NOZ, 1.0],
                [w, -w, NOZ, w],
                [1.0, 0.0, NOZ, 1.0],
            ],
            2,
            &[
                0.0, 0.0, 0.0, 0.25, 0.25, 0.5, 0.5, 0.75, 0.75, 1.0, 1.0, 1.0,
            ],
        );
        let mut st = UnboundedEval::default();
        let p = circle
            .eval_unbounded_pts_and_derivs(1.25, 0, Continuity::G2, &mut st)
            .unwrap();
        assert!(st.g1_closed);
        assert!(close(p[0], pt(&circle, 0.25), 1e-12));
    }

    #[test]
    fn remove_knots_deriv_constraints_keeps_ends() {
        // A cubic with redundant knots: inserting knots then removing them.
        let c = cubic().refine(&[0.2, 0.7]).unwrap();
        let free = c
            .remove_knots_deriv_constraints(1e-9, ConstrainedEnds::None, 1)
            .unwrap();
        assert_eq!(free, c.remove_knots(1e-9).unwrap());
        assert!(free.n() < c.n());
        let both = c
            .remove_knots_deriv_constraints(1e-9, ConstrainedEnds::Both, 3)
            .unwrap();
        assert!(both.n() >= free.n());
        assert_eq!(ConstrainedEnds::from_flag(7), ConstrainedEnds::None);
        assert_eq!(SumDiff::from_flag(2), Some(SumDiff::Minus));
    }

    #[test]
    fn offset_bound_of_circle() {
        let c = quarter_circle();
        let muu = c
            .offset_max_2nd_deriv(0.5, Vec3::new(0.0, 0.0, 1.0))
            .unwrap();
        assert!(muu.is_finite() && muu > 0.0);
    }

    /// Fuzz regressions: a huge degree overflowed `non_rat_first_deriv`, and a huge
    /// derivative order overflowed the start constraint `r <= p + der`.
    #[test]
    fn huge_degree_and_derivative_order_do_not_overflow() {
        let c = cubic();
        for p in [1usize << 31, i64::MAX as usize, usize::MAX] {
            let h = Curve::new(c.cpts().to_vec(), p, c.knots().to_vec()).unwrap();
            let r = h.non_rat_first_deriv(1);
            assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
            let r = h.remove_knots_deriv_constraints(1e-3, ConstrainedEnds::Both, 1);
            assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
        }
        let all = c.remove_knots_deriv_constraints(1e-3, ConstrainedEnds::Both, usize::MAX >> 1);
        let start = c.remove_knots_deriv_constraints(1e-3, ConstrainedEnds::Start, 1 << 40);
        assert!(all.is_ok() && start.is_ok());
    }
}
