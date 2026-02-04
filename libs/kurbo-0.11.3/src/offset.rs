// Copyright 2022 the Kurbo Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Computation of offset curves of cubic Béziers, based on a curve fitting
//! approach.
//!
//! See the [Parallel curves of cubic Béziers] blog post for a discussion of how
//! this algorithm works and what kind of results can be expected. In general, it
//! is expected to perform much better than most published algorithms. The number
//! of curve segments needed to attain a given accuracy scales as O(n^6) with
//! accuracy.
//!
//! In general, to compute the offset curve (also known as parallel curve) of
//! a cubic Bézier segment, create a [`CubicOffset`] struct with the curve
//! segment and offset, then use [`fit_to_bezpath`] or [`fit_to_bezpath_opt`]
//! depending on how much time to spend optimizing the resulting path.
//!
//! [`fit_to_bezpath`]: crate::fit_to_bezpath
//! [`fit_to_bezpath_opt`]: crate::fit_to_bezpath_opt
//! [Parallel curves of cubic Béziers]: https://raphlinus.github.io/curves/2022/09/09/parallel-beziers.html
use core::ops::Range;

#[cfg(not(feature = "std"))]
use crate::common::FloatFuncs;

use crate::{
    common::solve_itp, CubicBez, CurveFitSample, ParamCurve, ParamCurveDeriv, ParamCurveFit, Point,
    QuadBez, Vec2,
};

/// The offset curve of a cubic Bézier.
///
/// This is a representation of the offset curve of a cubic Bézier segment, for
/// purposes of curve fitting.
///
/// See the [module-level documentation] for a bit more discussion of the approach,
/// and how this struct is to be used.
///
/// [module-level documentation]: crate::offset
pub struct CubicOffset {
    /// Source curve.
    c: CubicBez,
    /// Derivative of source curve.
    q: QuadBez,
    /// Offset.
    d: f64,
    // c0 + c1 t + c2 t^2 is the cross product of second and first
    // derivatives of the underlying cubic, multiplied by offset (for
    // computing cusp).
    c0: f64,
    c1: f64,
    c2: f64,
}

impl CubicOffset {
    /// Create a new curve from Bézier segment and offset.
    ///
    /// This method should only be used if the Bézier is smooth. Use
    /// [`new_regularized`] instead to deal with a wider range of inputs.
    ///
    /// [`new_regularized`]: Self::new_regularized
    pub fn new(c: CubicBez, d: f64) -> Self {
        let q = c.deriv();
        let d0 = q.p0.to_vec2();
        let d1 = 2.0 * (q.p1 - q.p0);
        let d2 = q.p0.to_vec2() - 2.0 * q.p1.to_vec2() + q.p2.to_vec2();
        CubicOffset {
            c,
            q,
            d,
            c0: d * d1.cross(d0),
            c1: d * 2.0 * d2.cross(d0),
            c2: d * d2.cross(d1),
        }
    }

    /// Create a new curve from Bézier segment and offset, with numerical robustness tweaks.
    ///
    /// The dimension represents a minimum feature size; the regularization is allowed to
    /// perturb the curve by this amount in order to improve the robustness.
    pub fn new_regularized(c: CubicBez, d: f64, dimension: f64) -> Self {
        Self::new(c.regularize(dimension), d)
    }

    fn eval_offset(&self, t: f64) -> Vec2 {
        let dp = self.q.eval(t).to_vec2();
        let norm = Vec2::new(-dp.y, dp.x);
        // TODO: deal with hypot = 0
        norm * self.d / dp.hypot()
    }

    fn eval(&self, t: f64) -> Point {
        // Point on source curve.
        self.c.eval(t) + self.eval_offset(t)
    }

    /// Evaluate derivative of curve.
    fn eval_deriv(&self, t: f64) -> Vec2 {
        self.cusp_sign(t) * self.q.eval(t).to_vec2()
    }

    // Compute a function which has a zero-crossing at cusps, and is
    // positive at low curvatures on the source curve.
    fn cusp_sign(&self, t: f64) -> f64 {
        let ds2 = self.q.eval(t).to_vec2().hypot2();
        ((self.c2 * t + self.c1) * t + self.c0) / (ds2 * ds2.sqrt()) + 1.0
    }
}

impl ParamCurveFit for CubicOffset {
    fn sample_pt_tangent(&self, t: f64, sign: f64) -> CurveFitSample {
        let p = self.eval(t);
        const CUSP_EPS: f64 = 1e-8;
        let mut cusp = self.cusp_sign(t);
        if cusp.abs() < CUSP_EPS {
            // This is a numerical derivative, which is probably good enough
            // for all practical purposes, but an analytical derivative would
            // be more elegant.
            //
            // Also, we're not dealing with second or higher order cusps.
            cusp = sign * (self.cusp_sign(t + CUSP_EPS) - self.cusp_sign(t - CUSP_EPS));
        }
        let tangent = self.q.eval(t).to_vec2() * cusp.signum();
        CurveFitSample { p, tangent }
    }

    fn sample_pt_deriv(&self, t: f64) -> (Point, Vec2) {
        (self.eval(t), self.eval_deriv(t))
    }

    fn break_cusp(&self, range: Range<f64>) -> Option<f64> {
        const CUSP_EPS: f64 = 1e-8;
        // When an endpoint is on (or very near) a cusp, move just far enough
        // away from the cusp that we're confident we have the right sign.
        let break_cusp_help = |mut x, mut d| {
            let mut cusp = self.cusp_sign(x);
            while cusp.abs() < CUSP_EPS && d < 1.0 {
                x += d;
                let old_cusp = cusp;
                cusp = self.cusp_sign(x);
                if cusp.abs() > old_cusp.abs() {
                    break;
                }
                d *= 2.0;
            }
            (x, cusp)
        };
        let (a, cusp0) = break_cusp_help(range.start, 1e-12);
        let (b, cusp1) = break_cusp_help(range.end, -1e-12);
        if a >= b || cusp0 * cusp1 >= 0.0 {
            // Discussion point: maybe we should search for double cusps in the interior
            // of the range.
            return None;
        }
        let s = cusp1.signum();
        let f = |t| s * self.cusp_sign(t);
        let k1 = 0.2 / (b - a);
        const ITP_EPS: f64 = 1e-12;
        let x = solve_itp(f, a, b, ITP_EPS, 1, k1, s * cusp0, s * cusp1);
        Some(x)
    }
}

#[cfg(test)]
mod tests {
    use super::CubicOffset;
    use crate::{fit_to_bezpath, fit_to_bezpath_opt, CubicBez, PathEl};

    // This test tries combinations of parameters that have caused problems in the past.
    #[test]
    fn pathological_curves() {
        let curve = CubicBez {
            p0: (-1236.3746269978635, 152.17981429574826).into(),
            p1: (-1175.18662093517, 108.04721798590596).into(),
            p2: (-1152.142883879584, 105.76260301083356).into(),
            p3: (-1151.842639804639, 105.73040758939104).into(),
        };
        let offset = 3603.7267536453924;
        let accuracy = 0.1;
        let offset_path = CubicOffset::new(curve, offset);
        let path = fit_to_bezpath_opt(&offset_path, accuracy);
        assert!(matches!(path.iter().next(), Some(PathEl::MoveTo(_))));
        let path_opt = fit_to_bezpath(&offset_path, accuracy);
        assert!(matches!(path_opt.iter().next(), Some(PathEl::MoveTo(_))));
    }

    /// Cubic offset that used to trigger infinite recursion.
    #[test]
    fn infinite_recursion() {
        const DIM_TUNE: f64 = 0.25;
        const TOLERANCE: f64 = 0.1;
        let c = CubicBez::new(
            (1096.2962962962963, 593.90243902439033),
            (1043.6213991769548, 593.90243902439033),
            (1030.4526748971193, 593.90243902439033),
            (1056.7901234567901, 593.90243902439033),
        );
        let co = CubicOffset::new_regularized(c, -0.5, DIM_TUNE * TOLERANCE);
        fit_to_bezpath(&co, TOLERANCE);
    }

    #[test]
    fn test_cubic_offset_simple_line() {
        let cubic = CubicBez::new((0., 0.), (10., 0.), (20., 0.), (30., 0.));
        let offset = CubicOffset::new(cubic, 5.);
        let _optimized = fit_to_bezpath(&offset, 1e-6);
    }
}
