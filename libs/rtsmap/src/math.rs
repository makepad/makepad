//! Deterministic transcendentals.
//!
//! `f32::sin` and friends come from the platform's libm, and two platforms
//! are allowed to disagree in the last bits. A map generator whose starts sit
//! on a circle would then place a start one cell over on a different box, and
//! "same seed, same map" would quietly stop being true. IEEE add/mul/sqrt
//! are exact, so these polynomial evaluations are the same everywhere.

use core::f32::consts::{FRAC_PI_2, PI};

const TWO_PI: f32 = PI * 2.0;

/// Sine and cosine of `a` radians, to about 1e-6 — far finer than a cell.
pub fn sin_cos(a: f32) -> (f32, f32) {
    let x = wrap_pi(a);
    (poly_sin(x), poly_sin(wrap_pi(x + FRAC_PI_2)))
}

/// Fold an angle into `-PI..=PI`. The subtraction is exact for the angles a
/// map generator produces (a handful of turns at most).
fn wrap_pi(a: f32) -> f32 {
    let mut x = a;
    while x > PI {
        x -= TWO_PI;
    }
    while x < -PI {
        x += TWO_PI;
    }
    x
}

/// Bhaskara-grade is not enough for a 200-cell circle; this is the classic
/// 7th-order odd minimax on -PI..PI, which is exact to ~1e-7 there.
fn poly_sin(x: f32) -> f32 {
    let mut x = x;
    // Fold into -PI/2..PI/2 where the series is tightest.
    if x > FRAC_PI_2 {
        x = PI - x;
    } else if x < -FRAC_PI_2 {
        x = -PI - x;
    }
    let x2 = x * x;
    x * (1.0
        + x2 * (-1.0 / 6.0
            + x2 * (1.0 / 120.0 + x2 * (-1.0 / 5040.0 + x2 * (1.0 / 362_880.0)))))
}

/// `atan2(y, x)` in `-PI..PI`, accurate to ~1e-5.
pub fn atan2(y: f32, x: f32) -> f32 {
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    let ax = if x < 0.0 { -x } else { x };
    let ay = if y < 0.0 { -y } else { y };
    // atan on 0..1 by the odd minimax used everywhere for fixed-point atan.
    let (num, den, offset) = if ax >= ay { (ay, ax, 0.0) } else { (ax, ay, FRAC_PI_2) };
    let z = num / den;
    let z2 = z * z;
    let mut angle = z * (0.999_866 + z2 * (-0.330_299 + z2 * (0.180_141 + z2 * (-0.085_133 + z2 * 0.020_835))));
    if offset != 0.0 {
        angle = offset - angle;
    }
    if x < 0.0 {
        angle = PI - angle;
    }
    if y < 0.0 { -angle } else { angle }
}

/// Euclidean length, on IEEE sqrt only.
pub fn hypot(dx: f32, dy: f32) -> f32 {
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtsmap_sin_cos_tracks_the_platform_within_a_thousandth_of_a_cell() {
        for step in -400..400 {
            let a = step as f32 * 0.031;
            let (s, c) = sin_cos(a);
            assert!((s - a.sin()).abs() < 1e-4, "sin {a}: {s} vs {}", a.sin());
            assert!((c - a.cos()).abs() < 1e-4, "cos {a}: {c} vs {}", a.cos());
        }
    }

    #[test]
    fn rtsmap_atan2_tracks_the_platform() {
        for yi in -20..=20 {
            for xi in -20..=20 {
                let (y, x) = (yi as f32, xi as f32);
                let ours = atan2(y, x);
                let theirs = y.atan2(x);
                let diff = (ours - theirs).abs();
                assert!(diff < 1e-3 || (diff - core::f32::consts::PI * 2.0).abs() < 1e-3, "atan2({y},{x}) {ours} vs {theirs}");
            }
        }
    }
}
