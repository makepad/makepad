//! Closed-form real roots of quadratic, cubic and quartic polynomials.
//!
//! Upstream: `source/SMLib/src/SmPolynomial.cpp`, `source/SMLib/inc/SmPolynomial.h`.
//!
//! | Rust                  | Upstream                          |
//! |-----------------------|-----------------------------------|
//! | [`Roots`]             | `ULONG rlNumSolutions` + `double adSolutions[]` |
//! | [`solve_quadratic`]   | `SmPolynomial::SolveQuadraticEqn` |
//! | [`solve_cubic`]       | `SmPolynomial::SolveCubicEqn`     |
//! | [`solve_quartic`]     | `SmPolynomial::SolveQuarticEqn`   |
//! | `add_solution` (private) | `sm_AddSolution` (static)      |
//!
//! Coefficients are ordered by ascending power, as upstream: `c[i]` multiplies
//! `x^i`, so `[C, B, A]` is `A x^2 + B x + C`.
//!
//! Deviations:
//! - The out-params (`rlNumSolutions`, `adSolutions`) become a returned [`Roots`];
//!   the `SmStatus` return is dropped because every path returns `SM_SUCCESS`.
//!   Upstream's unused tail slots (zeroed or left uninitialised) are not exposed.
//! - In [`solve_quartic`] the resolvent cubic `z^3 + ...` has leading coefficient
//!   `1.0`, so it always yields a root unless `zero_tol > 1`; then upstream reads an
//!   uninitialised `adCSol[0]` (undefined behaviour). The port uses `0.0` there
//!   (see `plan/findings.md`, `geom.polynomial`).
//! - The `SM_DEBUG_CODE` self-checks (disabled upstream) are not ported.

use core::ops::Deref;

use crate::geom::vector3d::{acos, cbrt, sqrt};
use crate::math::{EFF_ZERO_SQRT, PI};

/// Up to four real roots, in the order upstream produces them.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Roots {
    values: [f64; 4],
    len: usize,
}

impl Roots {
    /// No roots.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            values: [0.0; 4],
            len: 0,
        }
    }

    /// The roots as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[f64] {
        self.values.get(..self.len).unwrap_or(&[])
    }

    /// Appends a root (ignored once four roots are stored, which upstream
    /// never reaches).
    fn push(&mut self, x: f64) {
        if let Some(slot) = self.values.get_mut(self.len) {
            *slot = x;
            self.len += 1;
        }
    }

    /// Subtracts `sub` from every root (upstream's back-substitution).
    fn shift(&mut self, sub: f64) {
        let len = self.len;
        for x in self.values.iter_mut().take(len) {
            *x -= sub;
        }
    }
}

impl Deref for Roots {
    type Target = [f64];

    fn deref(&self) -> &[f64] {
        self.as_slice()
    }
}

/// Solves `A x^2 + B x + C = 0` with `coefficients = [C, B, A]`.
///
/// Returns no roots for imaginary roots or a constant equation, one root for a
/// linear equation or a (near-)double root, and two otherwise (`sqrt(disc) - p`
/// first). `zero_tol` is the largest value treated as zero (typically `1e-8` to
/// `1e-12`). From Graphics Gems I.
#[must_use]
pub fn solve_quadratic(coefficients: &[f64; 3], zero_tol: f64) -> Roots {
    let [c0, c1, c2] = *coefficients;
    let mut roots = Roots::new();

    if c2.abs() < zero_tol {
        // No quadratic term: constant (no meaningful solution) or linear.
        if c1.abs() < zero_tol {
            // Independent of x.
        } else {
            roots.push(-c0 / c1);
        }
    } else if c0.abs() < zero_tol {
        // x * (Ax + B) = 0: x = 0, and -B/A unless that is also zero.
        roots.push(0.0);
        if c1.abs() < zero_tol {
            // Second root is also zero.
        } else {
            roots.push(-c1 / c2);
        }
    } else {
        let p = c1 / (2.0 * c2);
        let q = c0 / c2;

        let disc = p * p - q;

        if disc.abs() < zero_tol || (disc < 0.0 && disc.abs() < EFF_ZERO_SQRT) {
            // One 'grazing' solution.
            roots.push(-p);
        } else if disc < 0.0 {
            // Imaginary roots.
        } else {
            let sqrt_disc = sqrt(disc);
            roots.push(sqrt_disc - p);
            roots.push(-sqrt_disc - p);
        }
    }
    roots
}

/// Solves `A x^3 + B x^2 + C x + D = 0` with `coefficients = [D, C, B, A]`.
///
/// Falls back to [`solve_quadratic`] when `|A| < zero_tol`. Multiple roots are
/// reported once (a triple root once, a single plus double root as two roots).
/// From Graphics Gems I.
#[must_use]
pub fn solve_cubic(coefficients: &[f64; 4], zero_tol: f64) -> Roots {
    let [c0, c1, c2, c3] = *coefficients;
    if c3.abs() < zero_tol {
        return solve_quadratic(&[c0, c1, c2], zero_tol);
    }

    let a = c2 / c3;
    let b = c1 / c3;
    let c = c0 / c3;

    let sq_a = a * a;
    let p = 1.0 / 3.0 * (-1.0 / 3.0 * sq_a + b);
    let q = 1.0 / 2.0 * (2.0 / 27.0 * a * sq_a - 1.0 / 3.0 * a * b + c);
    let cb_p = p * p * p;
    let d = q * q + cb_p;

    let mut roots = Roots::new();
    if d.abs() < zero_tol {
        if q.abs() < zero_tol {
            // One triple root.
            roots.push(0.0);
        } else {
            // One single and one double root.
            let u = cbrt(-q);
            roots.push(2.0 * u);
            roots.push(-u);
        }
    } else if d < 0.0 {
        // Three roots. Upstream uses the plain `sqrt` here, `smos_Sqrt` below.
        let phi = 1.0 / 3.0 * acos(-q / (-cb_p).sqrt());
        let t = 2.0 * sqrt(-p);

        roots.push(t * phi.cos());
        roots.push(-t * (phi + PI / 3.0).cos());
        roots.push(-t * (phi - PI / 3.0).cos());
    } else {
        // One real root.
        let sqrt_d = sqrt(d);
        let u = cbrt(sqrt_d - q);
        let v = -cbrt(sqrt_d + q);
        roots.push(u + v);
    }

    roots.shift(1.0 / 3.0 * a);
    roots
}

/// `sm_AddSolution`: appends `value` unless it is within `zero_tol` of a root
/// already stored.
fn add_solution(roots: &mut Roots, value: f64, zero_tol: f64) {
    let duplicate = roots.iter().any(|&x| (value - x).abs() < zero_tol);
    if !duplicate {
        roots.push(value);
    }
}

/// Solves `A x^4 + B x^3 + C x^2 + D x + E = 0` with
/// `coefficients = [E, D, C, B, A]`.
///
/// Falls back to [`solve_cubic`] when `|A| < zero_tol`. Roots within `zero_tol`
/// of each other are reported once. From Graphics Gems I (Ferrari's method via
/// the resolvent cubic).
#[must_use]
pub fn solve_quartic(coefficients: &[f64; 5], zero_tol: f64) -> Roots {
    let [c0, c1, c2, c3, c4] = *coefficients;
    let mut roots = Roots::new();

    if c4.abs() < zero_tol {
        // Not a quartic: cubic, quadratic or linear solutions.
        for &x in solve_cubic(&[c0, c1, c2, c3], zero_tol).iter() {
            add_solution(&mut roots, x, zero_tol);
        }
        return roots;
    }

    // Normal form x^4 + A x^3 + B x^2 + C x + D = 0.
    let a = c3 / c4;
    let b = c2 / c4;
    let c = c1 / c4;
    let d = c0 / c4;

    // Substitute x = y - A/4: y^4 + p y^2 + q y + r = 0.
    let sq_a = a * a;
    let p = -3.0 / 8.0 * sq_a + b;
    let q = 1.0 / 8.0 * sq_a * a - 1.0 / 2.0 * a * b + c;
    let r = (-3.0 / 256.0 * sq_a * sq_a) + (1.0 / 16.0 * sq_a * b) - (1.0 / 4.0 * a * c) + d;

    if r.abs() < zero_tol {
        // No absolute term: y (y^3 + p y + q) = 0.
        for &x in solve_cubic(&[q, p, 0.0, 1.0], zero_tol).iter() {
            add_solution(&mut roots, x, zero_tol);
        }
        add_solution(&mut roots, 0.0, zero_tol);
    } else {
        // One real root of the resolvent cubic builds two quadratics.
        let resolvent = [
            1.0 / 2.0 * r * p - 1.0 / 8.0 * q * q,
            -r,
            -1.0 / 2.0 * p,
            1.0,
        ];
        let z = solve_cubic(&resolvent, zero_tol)
            .first()
            .copied()
            .unwrap_or(0.0);
        let mut u = z * z - r;
        let mut v = 2.0 * z - p;

        if u.abs() < zero_tol {
            u = 0.0;
        } else if u > 0.0 {
            u = sqrt(u);
        } else {
            return Roots::new();
        }

        if v.abs() < zero_tol {
            v = 0.0;
        } else if v > 0.0 {
            v = sqrt(v);
        } else {
            return Roots::new();
        }

        let first = [z - u, if q < 0.0 { -v } else { v }, 1.0];
        for &x in solve_quadratic(&first, zero_tol).iter() {
            add_solution(&mut roots, x, zero_tol);
        }

        let second = [z + u, if q < 0.0 { v } else { -v }, 1.0];
        for &x in solve_quadratic(&second, zero_tol).iter() {
            add_solution(&mut roots, x, zero_tol);
        }
    }

    // Back-substitute x = y - A/4.
    roots.shift(1.0 / 4.0 * a);
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::EFF_ZERO;

    fn sorted(r: &Roots) -> Vec<f64> {
        let mut v = r.to_vec();
        v.sort_by(f64::total_cmp);
        v
    }

    fn close(a: &[f64], b: &[f64], tol: f64) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < tol)
    }

    // prog_test my_test_polynomial (smCurveTest.cpp).
    #[test]
    fn quadratic_upstream_cases() {
        let r = solve_quadratic(&[2.0, 3.0, 1.0], EFF_ZERO);
        assert_eq!(r.len(), 2);
        assert!((r[0] + r[1] + 3.0).abs() < EFF_ZERO);
        assert_eq!(solve_quadratic(&[2.0, 7.0, 6.0], EFF_ZERO).len(), 2);
        let r = solve_quadratic(&[1.0, 2.0, 1.0], EFF_ZERO);
        assert_eq!(r.len(), 1);
        assert!((r[0] + 1.0).abs() < EFF_ZERO);
        assert!(solve_quadratic(&[1.0, -1.0, 1.0], EFF_ZERO).is_empty());
    }

    #[test]
    fn quadratic_degenerate() {
        assert!(solve_quadratic(&[1.0, 0.0, 0.0], EFF_ZERO).is_empty());
        assert_eq!(
            solve_quadratic(&[2.0, 4.0, 0.0], EFF_ZERO).as_slice(),
            &[-0.5]
        );
        assert_eq!(
            solve_quadratic(&[0.0, 0.0, 3.0], EFF_ZERO).as_slice(),
            &[0.0]
        );
        assert_eq!(
            solve_quadratic(&[0.0, 2.0, 1.0], EFF_ZERO).as_slice(),
            &[0.0, -2.0]
        );
        // Slightly negative discriminant above the tolerance but below sqrt(eff zero).
        assert_eq!(solve_quadratic(&[1.0 + 1e-8, 2.0, 1.0], EFF_ZERO).len(), 1);
    }

    #[test]
    fn cubic_upstream_cases() {
        let r = solve_cubic(&[6.0, 11.0, 6.0, 1.0], EFF_ZERO);
        assert!(close(&sorted(&r), &[-3.0, -2.0, -1.0], 1e-9));
        let r = solve_cubic(&[1.0, 3.0, 3.0, 1.0], EFF_ZERO);
        assert!(close(&r, &[-1.0], 1e-9));
        let r = solve_cubic(&[2.0, 5.0, 4.0, 1.0], EFF_ZERO);
        assert!(close(&sorted(&r), &[-2.0, -1.0], 1e-9));
        let r = solve_cubic(&[1.0, 0.0, 0.0, 1.0], EFF_ZERO);
        assert!(close(&r, &[-1.0], 1e-12));
    }

    #[test]
    fn cubic_falls_back_to_quadratic() {
        let r = solve_cubic(&[2.0, 3.0, 1.0, 0.0], EFF_ZERO);
        assert!(close(&sorted(&r), &[-2.0, -1.0], 1e-12));
    }

    #[test]
    fn quartic_upstream_cases() {
        let r = solve_quartic(&[-24.0, -38.0, -13.0, 2.0, 1.0], EFF_ZERO);
        assert!(close(&sorted(&r), &[-3.0, -2.0, -1.0, 4.0], 1e-9));
        let r = solve_quartic(&[1.0, 4.0, 6.0, 4.0, 1.0], EFF_ZERO);
        assert!(close(&r, &[-1.0], 1e-9));
        let r = solve_quartic(&[2.0, 7.0, 9.0, 5.0, 1.0], EFF_ZERO);
        assert_eq!(r.len(), 2);
        let r = solve_quartic(&[1.0, 0.0, -2.0, 0.0, 1.0], EFF_ZERO);
        assert!(close(&sorted(&r), &[-1.0, 1.0], 1e-9));
    }

    #[test]
    fn quartic_degenerate() {
        // Falls back to the cubic, duplicates removed.
        let r = solve_quartic(&[6.0, 11.0, 6.0, 1.0, 0.0], EFF_ZERO);
        assert_eq!(r.len(), 3);
        // x^4 + 1 = 0 has no real roots.
        assert!(solve_quartic(&[1.0, 0.0, 0.0, 0.0, 1.0], EFF_ZERO).is_empty());
        // x^4 - x = 0: r == 0 branch, roots 0 and 1.
        let r = solve_quartic(&[0.0, -1.0, 0.0, 0.0, 1.0], EFF_ZERO);
        assert!(close(&sorted(&r), &[0.0, 1.0], 1e-9));
        // Tolerance above one: resolvent cubic has no root, port uses z = 0.
        let _ = solve_quartic(&[1.0, 2.0, 3.0, 4.0, 5.0], 2.0);
    }
}
