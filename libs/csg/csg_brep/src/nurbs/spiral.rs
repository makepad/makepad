//! Cubic B-spline approximation of a (conical) spiral about the z axis, and the
//! trivariate NURBS volume record with its allocate and copy helpers.
//!
//! Upstream: `source/SMLib/src/NL_Spiral.cpp`, `source/SMLib/inc/NL_Spiral.h`,
//! `source/SMLib/src/SmNurbsVol.cpp`, `source/SMLib/inc/SmNurbsVol.h`.
//!
//! | Rust                                   | Upstream                                  |
//! |----------------------------------------|-------------------------------------------|
//! | [`approx_spiral`], [`SpiralSpec`], [`ApproxSpiral`] | `N_CrvApproxSpiral`          |
//! | [`Hand`]                               | `RorL` (`1` = right hand, else left)      |
//! | `Spiral::radius_at` (private)          | `ST_GetSpiralAngle`                       |
//! | `Spiral::point_at` (private)           | `ST_GetPointAtAngle`                      |
//! | `Spiral::deriv_at` (private)           | `ST_GetPointAtAngleD`                     |
//! | [`CURVE_CPT_LIMIT`]                    | `NL_CCPLIM`                               |
//! | [`Volume`]                             | `NL_VOLUME` / `gw_VOLUME`                 |
//! | [`allocate_nurb_volume`]               | `sm_AllocateNurbVolume`, `sm_InitNurbVolumeMemory` |
//! | [`copy_nurb_volume`]                   | `sm_AllocateAndCopyNurbVolume`            |
//! | (none: Rust owns memory)               | `sm_ComputeNurbVolumeSize`                |
//!
//! Deviations:
//! - Upstream keeps the spiral (`RL`, `h`, `r0`, `r1`, `dTurns`, `Rdfun`) in file
//!   statics shared by the helpers; the port passes a private `Spiral` value. Every
//!   static is set before it is read in each call, so no state leaks between calls.
//! - The in/out `*NumPoints` and `*tol` become fields of [`SpiralSpec`] (in) and
//!   [`ApproxSpiral`] (out); `RorL` is [`Hand`] ([`Hand::from_flag`] maps upstream's
//!   integer). The `NL_STACKS` argument has no counterpart.
//! - Upstream ignores the error flag of `N_CrvEval` in the goodness-of-fit test; it
//!   cannot fail there (the parameters are span midpoints), and the port returns it.
//! - The initial span count `(int)(8 * NumTurns + 0.99)` is undefined in C outside
//!   the `int` range; Rust's `as` saturates. Point counts whose arrays cannot be
//!   allocated return [`SolidError::InvalidInput`].
//! - When the span count reaches `NL_CCPLIM - 6` without meeting the tolerance,
//!   upstream returns the *incremented* span count in `*NumPoints` (one more than the
//!   curve was fitted with); the port returns the same value (see
//!   `plan/findings.md`).
//! - `NL_MTOL` is the default [`MODEL_TOL`] (upstream's global can be reset by other
//!   routines) and `NL_CCPLIM` the default [`CURVE_CPT_LIMIT`].
//! - The volume memory helpers lay out one `calloc` block of C structs;
//!   [`Volume`] owns its arrays instead, so `sm_ComputeNurbVolumeSize` (a byte count)
//!   and the pointer set-up of `sm_InitNurbVolumeMemory` have no counterpart.
//!   [`allocate_nurb_volume`] returns a zero-filled volume (upstream `smos_Calloc`).
//!   `sm_AllocateAndCopyNurbVolume` returns `NULL` for a volume without a control
//!   mesh; a [`Volume`] always has one, so [`copy_nurb_volume`] always succeeds.
//! - Caller-sized interpolation and volume arrays are capped at
//!   [`crate::limits::MAX_ALLOC_BYTES`] per allocation, returning the existing
//!   allocation error above the ceiling even if an overcommitting allocator would
//!   accept the reservation.

use std::f64::consts::TAU;

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::crv_basic::Curve;
use crate::nurbs::crv_fit::p1::ParamMethod;
use crate::nurbs::crv_fit::p3::cubic_spline_interp;
use crate::nurbs::knots::Side;
use crate::nurbs::math::{CPoint, MODEL_TOL};

/// `NL_CCPLIM`: upstream's maximum number of curve control points (default `1000`).
pub const CURVE_CPT_LIMIT: i64 = 1000;

/// `TWOPI` of `NL_Spiral.cpp` (`6.2831853071795864`, the same double as [`TAU`]).
const TWOPI: f64 = TAU;

const OP: &str = "nurbs::spiral::approx_spiral";

/// Direction of rotation of a spiral about +z (upstream `RorL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hand {
    /// Right-handed (`RorL == 1`): counter-clockwise seen from +z.
    Right,
    /// Left-handed (any other `RorL`).
    Left,
}

impl Hand {
    /// Upstream's mapping of `RorL`: `1` is right-handed, anything else left-handed.
    #[must_use]
    pub fn from_flag(rorl: i64) -> Self {
        if rorl == 1 {
            Self::Right
        } else {
            Self::Left
        }
    }

    /// `RL`: `1` for right, `-1` for left.
    fn sign(self) -> f64 {
        match self {
            Self::Right => 1.0,
            Self::Left => -1.0,
        }
    }
}

/// Inputs of [`approx_spiral`] (`N_CrvApproxSpiral`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpiralSpec {
    /// Height (`>= NL_MTOL`) of the spiral along +z.
    pub height: f64,
    /// Radius at `z = 0` (`>= -NL_MTOL`; below `NL_MTOL` it is `0`).
    pub rad0: f64,
    /// Radius at `z = height` (as `rad0`; `rad0 + rad1` must reach `NL_MTOL`).
    pub rad1: f64,
    /// Number of turns (`1` = 360 degrees, `>= NL_MTOL`).
    pub num_turns: f64,
    /// Direction of rotation.
    pub hand: Hand,
    /// Number of points to interpolate; `<= 1` fits to `tol` instead.
    pub num_points: i64,
    /// Fit tolerance (raised to `10 * NL_MTOL`); used only when fitting.
    pub tol: f64,
}

/// Result of [`approx_spiral`].
#[derive(Debug, Clone, PartialEq)]
pub struct ApproxSpiral {
    /// The cubic spiral, interpolating `num_points` points at equal angles.
    pub curve: Curve,
    /// Upstream's `*NumPoints` on exit: span count + 1 (see the module notes for the
    /// case where the span limit was hit).
    pub num_points: i64,
    /// Upstream's `*tol` on exit: the largest radial deviation at the span
    /// midpoints.
    pub tol: f64,
}

/// The spiral being approximated (upstream's file statics).
#[derive(Debug, Clone, Copy)]
struct Spiral {
    rl: f64,
    h: f64,
    r0: f64,
    r1: f64,
    turns: f64,
    rdfun: f64,
}

impl Spiral {
    /// Radius at angle `theta` (`ST_GetSpiralAngle`).
    fn radius_at(&self, theta: f64) -> f64 {
        (self.r0 * (TWOPI * self.turns - theta) + self.r1 * theta) / (TWOPI * self.turns)
    }

    /// Point at angle `theta`, clamped to the end angle (`ST_GetPointAtAngle`).
    fn point_at(&self, theta: f64) -> Vec3 {
        let theta = if theta > TWOPI * self.turns {
            TWOPI * self.turns
        } else {
            theta
        };
        let cos_theta = theta.cos();
        let sin_theta = theta.sin();
        Vec3::new(
            self.radius_at(theta) * cos_theta,
            self.rl * self.radius_at(theta) * sin_theta,
            self.h * theta / (TWOPI * self.turns),
        )
    }

    /// `dP/dtheta` at angle `theta`, clamped to the end angle
    /// (`ST_GetPointAtAngleD`).
    fn deriv_at(&self, theta: f64) -> Vec3 {
        let theta = if theta > TWOPI * self.turns {
            TWOPI * self.turns
        } else {
            theta
        };
        let cos_theta = theta.cos();
        let sin_theta = theta.sin();
        Vec3::new(
            -self.radius_at(theta) * sin_theta + self.rdfun * cos_theta,
            self.rl * (self.radius_at(theta) * cos_theta + self.rdfun * sin_theta),
            self.h / (TWOPI * self.turns),
        )
    }
}

/// Cubic B-spline approximation of a spiral about +z from `(rad0, 0, 0)` to height
/// `height` (radius `rad1`), turning `num_turns` times (`N_CrvApproxSpiral`).
///
/// The spiral is interpolated at equal angles with exact end derivatives. With
/// `num_points <= 1` the number of points starts at `8 * num_turns` spans and grows
/// until the largest radial deviation at the span midpoints is below `tol`, or the
/// span count reaches `NL_CCPLIM - 6`.
///
/// # Errors
///
/// [`NlibCode::Input`] for `height < NL_MTOL`, a radius below `-NL_MTOL`, both radii
/// (near) zero, `num_turns < NL_MTOL`, or a single interpolation point (fewer than
/// `1/8` turn when fitting); errors of the cubic interpolation and evaluation;
/// `InvalidInput` when the point arrays cannot be allocated.
pub fn approx_spiral(spec: &SpiralSpec) -> Result<ApproxSpiral> {
    let SpiralSpec {
        height,
        mut rad0,
        mut rad1,
        num_turns,
        hand,
        num_points,
        mut tol,
    } = *spec;
    let deg: usize = 3;

    // Check for input errors.
    if height < MODEL_TOL {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    if (rad0 < -MODEL_TOL) || (rad1 < -MODEL_TOL) {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    if rad0 < MODEL_TOL {
        rad0 = 0.;
    }
    if rad1 < MODEL_TOL {
        rad1 = 0.;
    }
    if (rad0 + rad1) < MODEL_TOL {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    if num_turns < MODEL_TOL {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    if tol < 10.0 * MODEL_TOL {
        tol = 10.0 * MODEL_TOL;
    }

    let n_spans_max = CURVE_CPT_LIMIT - 6; // max number of knots for cubic

    // Set up the spiral.
    let mut sp = Spiral {
        rl: hand.sign(),
        h: height,
        r0: rad0,
        r1: rad1,
        turns: num_turns,
        rdfun: 0.0,
    };
    let mut n_spans = num_points.saturating_sub(1);

    let mut do_fit = false;
    if n_spans <= 0 {
        do_fit = true;
        // `(int)` conversion; saturating where C is undefined.
        n_spans = i64::from((8.0 * sp.turns + 0.99) as i32);
    }

    loop {
        let count = usize::try_from(n_spans.saturating_add(1))
            .map_err(|_| SolidError::nlib(OP, NlibCode::Input))?;
        let mut pnts: Vec<Vec3> = Vec::new();
        if !crate::limits::try_reserve_total(&mut pnts, count) {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "too many interpolation points",
            });
        }
        let d_theta = TWOPI * sp.turns / n_spans as f64;

        pnts.push(Vec3::new(sp.r0, 0.0, 0.0));
        let mut angle = 0.0;
        for _ in 1..count {
            angle += d_theta;
            pnts.push(sp.point_at(angle));
        }

        sp.rdfun = (-sp.r0 + sp.r1) / (TWOPI * sp.turns);
        let mut dp0 = sp.deriv_at(0.0);
        let mut dp1 = sp.deriv_at(TWOPI * sp.turns);

        // End tangents: dP/dTheta scaled uniformly into the fit's [0, 1]
        // parameterisation (upstream's comment on the previous per-axis scaling).
        dp0.x *= TWOPI * sp.turns;
        dp0.y *= TWOPI * sp.turns;
        dp0.z *= TWOPI * sp.turns;
        dp1.x *= TWOPI * sp.turns;
        dp1.y *= TWOPI * sp.turns;
        dp1.z *= TWOPI * sp.turns;

        // Use cubic interpolation at the knots.
        let crv = cubic_spline_interp(&pnts, dp0, dp1, ParamMethod::Uniform)?;

        // Test goodness of fit: check the midpoint of each span.
        let uu = crv.knots();
        let mut dmax = 0.0;
        for i in 0..count.saturating_sub(1) {
            let (Some(&a), Some(&b)) = (uu.get(deg + i), uu.get(deg + i + 1)) else {
                return Err(SolidError::InvalidInput {
                    operation: OP,
                    reason: "fitted curve has fewer knots than spans",
                });
            };
            let t = 0.5 * (a + b);
            let c = crv.eval(t, Side::Left)?;
            let theta = (c.z * TWOPI * sp.turns) / sp.h;
            let d = sp.point_at(theta);
            let dist_cxy = (c.x * c.x + c.y * c.y).sqrt();
            let dist_dxy = (d.x * d.x + d.y * d.y).sqrt();
            let dist = (dist_cxy - dist_dxy).abs();
            if dist > dmax {
                dmax = dist;
            }
        }

        let mut in_tol = if do_fit { dmax < tol } else { true };

        // If not in tolerance, reset the span count and repeat.
        if !in_tol {
            let n_spans0 = n_spans;
            let mut factor = dmax / tol;
            if factor > 1.0 {
                factor = 1.0;
            }
            let mut delta_spans = i64::from((n_spans as f64 * factor) as i32);
            if delta_spans < 1 {
                delta_spans = 1;
            }
            n_spans = n_spans.saturating_add(delta_spans);

            if n_spans0 < n_spans_max {
                if n_spans > n_spans_max {
                    n_spans = n_spans_max;
                }
            } else {
                // We hit the max number of allowed spans.
                in_tol = true;
            }
        }

        if in_tol {
            // In all cases return the number of points and dmax.
            return Ok(ApproxSpiral {
                curve: crv,
                num_points: n_spans.saturating_add(1),
                tol: dmax,
            });
        }
    }
}

/// A trivariate NURBS volume (upstream `NL_VOLUME` / `gw_VOLUME`): control mesh
/// `Pw[0..=l][0..=m][0..=n]`, degrees `p`, `q`, `r` and knot vectors `U`, `V`, `W`.
#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    /// Control points, `w` fastest, then `v`, then `u`.
    pw: Vec<CPoint>,
    /// Highest control point indices `(l, m, n)` (upstream `mesh->m, n, o`).
    cp_hi: [usize; 3],
    /// Degrees `(p, q, r)`.
    degree: [usize; 3],
    /// Knot vectors `U`, `V`, `W`.
    knots: [Vec<f64>; 3],
}

fn too_big() -> SolidError {
    SolidError::InvalidInput {
        operation: "nurbs::spiral::allocate_nurb_volume",
        reason: "volume too large",
    }
}

fn zeroed<T: Clone>(len: usize, v: T) -> Result<Vec<T>> {
    crate::limits::try_filled(len, v).ok_or_else(too_big)
}

impl Volume {
    /// Highest control point indices `(l, m, n)` in `u`, `v`, `w`.
    #[must_use]
    pub fn cp_hi(&self) -> [usize; 3] {
        self.cp_hi
    }

    /// Degrees `(p, q, r)` in `u`, `v`, `w`.
    #[must_use]
    pub fn degrees(&self) -> [usize; 3] {
        self.degree
    }

    /// Knot vector of direction `dir` (`0` = u, `1` = v, `2` = w; empty otherwise).
    #[must_use]
    pub fn knots(&self, dir: usize) -> &[f64] {
        self.knots.get(dir).map_or(&[], Vec::as_slice)
    }

    /// Mutable knot vector of direction `dir` (empty for `dir > 2`).
    pub fn knots_mut(&mut self, dir: usize) -> &mut [f64] {
        self.knots.get_mut(dir).map_or(&mut [], Vec::as_mut_slice)
    }

    /// All control points, `w` index fastest, then `v`, then `u`.
    #[must_use]
    pub fn cpts(&self) -> &[CPoint] {
        &self.pw
    }

    /// Mutable control points in the order of [`Volume::cpts`].
    pub fn cpts_mut(&mut self) -> &mut [CPoint] {
        &mut self.pw
    }

    fn offset(&self, i: usize, j: usize, k: usize) -> Option<usize> {
        let [l, m, n] = self.cp_hi;
        if i > l || j > m || k > n {
            return None;
        }
        // In range: the sizes were checked at allocation.
        Some((i * (m + 1) + j) * (n + 1) + k)
    }

    /// Control point `Pw[i][j][k]`, `None` outside the mesh.
    #[must_use]
    pub fn cpt(&self, i: usize, j: usize, k: usize) -> Option<CPoint> {
        self.offset(i, j, k).and_then(|o| self.pw.get(o)).copied()
    }

    /// Mutable control point `Pw[i][j][k]`, `None` outside the mesh.
    pub fn cpt_mut(&mut self, i: usize, j: usize, k: usize) -> Option<&mut CPoint> {
        self.offset(i, j, k).and_then(|o| self.pw.get_mut(o))
    }
}

/// A zero-filled volume with highest control point indices `cp_hi`, degrees
/// `degree` and highest knot indices `knot_hi` (`sm_AllocateNurbVolume` with
/// `sm_InitNurbVolumeMemory`; memory from `smos_Calloc`).
///
/// # Errors
///
/// `InvalidInput` when the arrays cannot be sized or allocated.
pub fn allocate_nurb_volume(
    cp_hi: [usize; 3],
    degree: [usize; 3],
    knot_hi: [usize; 3],
) -> Result<Volume> {
    let [l, m, n] = cp_hi;
    let count = l
        .checked_add(1)
        .zip(m.checked_add(1))
        .zip(n.checked_add(1))
        .and_then(|((a, b), c)| a.checked_mul(b).and_then(|ab| ab.checked_mul(c)))
        .ok_or_else(too_big)?;
    let [ku, kv, kw] = knot_hi;
    let knots = [
        zeroed(ku.checked_add(1).ok_or_else(too_big)?, 0.0)?,
        zeroed(kv.checked_add(1).ok_or_else(too_big)?, 0.0)?,
        zeroed(kw.checked_add(1).ok_or_else(too_big)?, 0.0)?,
    ];
    Ok(Volume {
        pw: zeroed(count, CPoint::default())?,
        cp_hi,
        degree,
        knots,
    })
}

/// A copy of `src` (`sm_AllocateAndCopyNurbVolume`).
#[must_use]
pub fn copy_nurb_volume(src: &Volume) -> Volume {
    src.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(num_points: i64, tol: f64) -> SpiralSpec {
        SpiralSpec {
            height: 10.0,
            rad0: 2.0,
            rad1: 2.0,
            num_turns: 5.0,
            hand: Hand::Right,
            num_points,
            tol,
        }
    }

    #[test]
    fn fits_helix_to_tolerance() {
        let r = approx_spiral(&spec(0, 1e-3)).unwrap();
        assert!(r.tol < 1e-3);
        let c = &r.curve;
        assert_eq!(c.degree(), 3);
        assert_eq!(i64::try_from(c.n()).unwrap(), r.num_points + 1);
        let p0 = c.cpts()[0];
        assert_eq!((p0.x, p0.y, p0.z), (2.0, 0.0, 0.0));
        let pe = c.eval(1.0, Side::Right).unwrap();
        assert!((pe.z - 10.0).abs() < 1e-12);
        assert!(((pe.x * pe.x + pe.y * pe.y).sqrt() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn fixed_point_count_and_hand() {
        let r = approx_spiral(&spec(9, 1e-3)).unwrap();
        assert_eq!(r.num_points, 9);
        assert_eq!(r.curve.n(), 10);
        let mut s = spec(9, 1e-3);
        s.hand = Hand::Left;
        let l = approx_spiral(&s).unwrap();
        for (a, b) in r.curve.cpts().iter().zip(l.curve.cpts()) {
            assert_eq!(a.x, b.x);
            assert_eq!(a.y, -b.y);
            assert_eq!(a.z, b.z);
        }
        assert_eq!(Hand::from_flag(1), Hand::Right);
        assert_eq!(Hand::from_flag(0), Hand::Left);
        assert_eq!(Hand::from_flag(2), Hand::Left);
    }

    #[test]
    fn input_errors() {
        let err = |s: SpiralSpec| approx_spiral(&s).unwrap_err();
        let nl = SolidError::nlib(OP, NlibCode::Input);
        assert_eq!(
            err(SpiralSpec {
                height: 0.0,
                ..spec(0, 1e-3)
            }),
            nl
        );
        assert_eq!(
            err(SpiralSpec {
                rad0: -1.0,
                ..spec(0, 1e-3)
            }),
            nl
        );
        assert_eq!(
            err(SpiralSpec {
                rad0: 0.0,
                rad1: 5e-8,
                ..spec(0, 1e-3)
            }),
            nl
        );
        assert_eq!(
            err(SpiralSpec {
                num_turns: 0.0,
                ..spec(0, 1e-3)
            }),
            nl
        );
        // Fewer than 1/8 turn: zero spans, one point.
        assert!(matches!(
            err(SpiralSpec {
                num_turns: 1e-4,
                ..spec(0, 1e-3)
            }),
            SolidError::Nlib {
                code: NlibCode::Input,
                ..
            }
        ));
    }

    #[test]
    fn span_limit_reports_incremented_count() {
        let r = approx_spiral(&SpiralSpec {
            num_turns: 124.0,
            tol: 1e-9,
            ..spec(0, 1e-3)
        })
        .unwrap();
        assert_eq!(
            r.curve.n(),
            usize::try_from(CURVE_CPT_LIMIT - 6 + 2).unwrap()
        );
        assert!(r.num_points > CURVE_CPT_LIMIT - 6 + 1);
    }

    #[test]
    fn volume_rejects_control_points_above_allocation_ceiling() {
        const COUNT: usize = crate::limits::MAX_ALLOC_BYTES / size_of::<CPoint>() + 1;
        assert_eq!(
            allocate_nurb_volume([COUNT - 1, 0, 0], [0; 3], [0; 3]).unwrap_err(),
            too_big()
        );
    }

    #[test]
    fn volume_rejects_knots_above_allocation_ceiling() {
        const COUNT: usize = crate::limits::MAX_ALLOC_BYTES / size_of::<f64>() + 1;
        for dir in 0..3 {
            let mut knot_hi = [0; 3];
            knot_hi[dir] = COUNT - 1;
            assert_eq!(
                allocate_nurb_volume([0; 3], [0; 3], knot_hi).unwrap_err(),
                too_big()
            );
        }
    }

    #[test]
    fn volume_allocate_and_copy() {
        let mut v = allocate_nurb_volume([1, 2, 3], [1, 2, 3], [3, 5, 7]).unwrap();
        assert_eq!(v.cpts().len(), 2 * 3 * 4);
        assert!(v.cpts().iter().all(|c| *c == CPoint::default()));
        assert_eq!(v.knots(2).len(), 8);
        assert!(v.knots(3).is_empty());
        *v.cpt_mut(1, 2, 3).unwrap() = CPoint::new(1.0, 2.0, 3.0, 4.0);
        v.knots_mut(0)[3] = 1.0;
        let c = copy_nurb_volume(&v);
        assert_eq!(c, v);
        assert_eq!(c.cpt(1, 2, 3), Some(CPoint::new(1.0, 2.0, 3.0, 4.0)));
        assert_eq!(c.cpts().last().copied(), c.cpt(1, 2, 3));
        assert_eq!(c.cpt(2, 0, 0), None);
        assert_eq!(c.degrees(), [1, 2, 3]);
        assert_eq!(c.cp_hi(), [1, 2, 3]);
        assert!(allocate_nurb_volume([usize::MAX, 0, 0], [0; 3], [0; 3]).is_err());
    }
}
