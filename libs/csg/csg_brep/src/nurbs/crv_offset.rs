//! Offset of a planar NURBS curve by point sampling, interpolation and knot removal.
//!
//! Upstream: `source/SMLib/src/NL_CrvOffset.cpp` (only `N_CrvOffsetPtSampling`; the
//! rest of the file is compiled out by `NLIB_UNUSED`).
//!
//! | Rust                                         | Upstream                 |
//! |----------------------------------------------|--------------------------|
//! | [`Curve::offset_pt_sampling`], [`OffsetParam`] | `N_CrvOffsetPtSampling` |
//!
//! Deviations:
//! - The output curve `curQ` and the memory stacks become the return value; errors are
//!   [`SolidError::Nlib`] values with the upstream `NL_*_ERR` number instead of a flag
//!   plus the global `NL_ERROR` record.
//! - The `par` flag (`NL_UNIFORM` / `NL_CHORDLENGTH` / `NL_CENTRIPETAL` /
//!   `NL_INHERITED`) and the `tan` flag (`NL_YES` / `NL_NO`) are [`OffsetParam`] and a
//!   `bool`.
//! - Upstream casts the per-segment sample count `ceil(f1 * gro)` to `NL_INDEX`
//!   without a check; when it is not finite or does not fit (a negative or zero
//!   tolerance, an unbounded second derivative) that is undefined behaviour. The port
//!   returns [`SolidError::InvalidInput`] there. Every other array access is bounds
//!   checked the same way (degree `0`, where upstream writes past the parameter array).
//! - The `N_CrvCompress` step only trims storage and is kept for fidelity.
//! - A curve degree or interpolation degree `deg` above
//!   [`MAX_SHORT_DEGREE`](crate::nurbs::bezier::MAX_SHORT_DEGREE) (impossible
//!   upstream, where `NL_DEGREE` is a `short`) is [`SolidError::InvalidInput`], so
//!   index arithmetic and allocations sized from a degree cannot overflow.
//! - HANG-44: more sample points in total than the input-scaled
//!   [`offset_sample_bound`] is [`SolidError::InvalidInput`]. Upstream samples any
//!   count, but the knot removal that follows takes time quadratic in it (practically
//!   forever for the counts a pole or a tiny tolerance produce). The bound is
//!   `min(2^17, max(16 384, 4 * segments * ceil((1/tol)^exp * 2 deg)))`: any count up
//!   to [`MAX_OFFSET_SAMPLES`] (the former fixed cap, so every input it accepted is
//!   unchanged), four times the curvature-free count a curve of that many Bezier
//!   segments needs at `tol` (a swept wall of 127 segments at `1e-5` needs 38 227),
//!   and never more than [`MAX_OFFSET_SAMPLES_ANY`] (about 30 s at opt-level 3).

use crate::error::{NlibCode, Result, SolidError};
use crate::math::{max2, Vec3};
use crate::nurbs::bezier::check_short_degree;
use crate::nurbs::crv_adv::p2::ConstrainedEnds;
use crate::nurbs::crv_basic::{ConicType, Curve};
use crate::nurbs::crv_common::{circ_arc, Arc};
use crate::nurbs::crv_fit::p1::{
    interp_given_params, interp_knot_vector, interp_knots_and_derivs, knot_vector_end_derivs,
    param_values, ParamMethod,
};
use crate::nurbs::crv_nurb::{line_from_pt_and_vector, CurveType};
use crate::nurbs::geom::{dist_polygon, normalize, pt_along_vector, Interval};
use crate::nurbs::index::{get, set, storage, sx};
use crate::nurbs::knots::{KnotVector, Side};
use crate::nurbs::math::{blend_pt, float_op_is_bad, FloatOp, MODEL_TOL};

const OP: &str = "nurbs::crv_offset::Curve::offset_pt_sampling";

/// Sample points [`Curve::offset_pt_sampling`] always allows on a free-form curve (the
/// sum of the per-segment counts; the difftest guard allows 4000). Above it the
/// input-scaled [`offset_sample_bound`] applies.
pub const MAX_OFFSET_SAMPLES: isize = 16_384;

/// How many times its curvature-free count (`ceil(f1 * 2 deg)` per Bezier segment) a
/// curve may sample in [`offset_sample_bound`].
pub const OFFSET_SAMPLE_SLACK: isize = 4;

/// The most sample points [`Curve::offset_pt_sampling`] takes on any curve: the knot
/// removal that follows is quadratic in the count.
pub const MAX_OFFSET_SAMPLES_ANY: isize = 1 << 17;

/// The most sample points [`Curve::offset_pt_sampling`] takes on a free-form curve of
/// `segments` Bezier segments whose per-segment count without the curvature term
/// (`ceil((1/tol)^exp * 2 deg)`, the least upstream samples a segment) is `floor`:
/// `OFFSET_SAMPLE_SLACK * segments * floor`, clamped to
/// `[MAX_OFFSET_SAMPLES, MAX_OFFSET_SAMPLES_ANY]`. A well-formed curve samples at
/// most `OFFSET_SAMPLE_SLACK` times its floor (its offset's second derivative `Muu`
/// stays below `8 (2 deg OFFSET_SAMPLE_SLACK)^2` on average); a pole or a cusp of the
/// offset drives `Muu`, and so the count, far beyond it.
#[must_use]
pub fn offset_sample_bound(segments: usize, floor: isize) -> isize {
    let scaled = isize::try_from(segments)
        .ok()
        .and_then(|n| n.checked_mul(floor))
        .and_then(|n| n.checked_mul(OFFSET_SAMPLE_SLACK))
        .unwrap_or(isize::MAX);
    scaled.clamp(MAX_OFFSET_SAMPLES, MAX_OFFSET_SAMPLES_ANY)
}

/// Upstream `NL_UNITSPAN`.
const UNIT_SPAN: Interval = Interval::new(0.0, 1.0);

/// Parametrisation of the sampled offset points in [`Curve::offset_pt_sampling`]
/// (upstream `par` flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffsetParam {
    /// `NL_UNIFORM`, `NL_CHORDLENGTH` or `NL_CENTRIPETAL`: computed from the points;
    /// the result is rescaled to the input's parameter range.
    Fit(ParamMethod),
    /// `NL_INHERITED`: the sample parameters of the input curve.
    Inherited,
}

impl OffsetParam {
    /// The upstream flag value (`NL_INHERITED = 4`).
    #[must_use]
    pub fn flag(self) -> i32 {
        match self {
            OffsetParam::Fit(m) => m.flag(),
            OffsetParam::Inherited => 4,
        }
    }
}

/// A sample count upstream's `NL_INDEX` arithmetic cannot represent.
const fn unrepresentable() -> SolidError {
    SolidError::InvalidInput {
        operation: OP,
        reason: "sample count not representable",
    }
}

/// `(NL_INDEX)del` of a sample count: errors where upstream's cast is undefined.
fn count(del: f64) -> Result<isize> {
    if !(0.0..1.0e15).contains(&del) {
        return Err(unrepresentable());
    }
    Ok(del as isize)
}

impl Curve {
    /// Offset at distance `d` of this planar, G1 continuous curve
    /// (`N_CrvOffsetPtSampling`): every point moves along `B x T`, `T` the unit
    /// tangent and `B` the plane normal `b`.
    ///
    /// A curve within [`MODEL_TOL`] of a line or circle ([`Curve::get_type`]) is offset
    /// exactly: a line through the shifted start point, or a circular arc of radius
    /// `r + d` (degree 4 or 5 when `deg` is 4 or 5, else degree 2). Any other curve is
    /// sampled on every Bezier segment with a density from
    /// [`Curve::offset_max_2nd_deriv`] and `tol`, the offset points are interpolated
    /// with degree `deg` (with the input's end derivatives when `tangents`), and knots
    /// are removed within `tol` ([`Curve::remove_knots_deriv_constraints`]). Loops,
    /// cusps and self-intersections are not checked. A 2D input gives a 2D result.
    ///
    /// # Errors
    ///
    /// [`NlibCode::Numerical`] when `1 / tol` is not computable; the errors of
    /// [`Curve::get_type`], [`normalize`] (zero `b`, a degenerate line direction or end
    /// tangent), the line and arc constructors, [`Curve::decompose_bez`],
    /// [`Curve::offset_max_2nd_deriv`], [`Curve::eval_tangent`], [`Curve::derivs`], the
    /// interpolation routines and knot removal; [`SolidError::InvalidInput`] where
    /// upstream's behaviour is undefined (see the module docs).
    pub fn offset_pt_sampling(
        &self,
        b: Vec3,
        d: f64,
        deg: usize,
        tangents: bool,
        par: OffsetParam,
        tol: f64,
    ) -> Result<Curve> {
        check_short_degree(deg, OP)?;

        // Get curve type.
        let typ = self.get_type(MODEL_TOL)?;

        let twod = !self.is_3d();

        match typ {
            // Offset line.
            CurveType::Line { start, dir } => {
                let dd = b.cross(dir);
                let (dd, _) = normalize(dd)?;
                let mut q = start;
                blend_pt(&mut q, d, dd);
                let mut cur_q = line_from_pt_and_vector(q, dir)?;
                if twod {
                    cur_q.to_2d();
                }
                return Ok(cur_q);
            }
            // Offset circle.
            CurveType::Circle {
                center,
                x_axis,
                y_axis,
                radius,
                angle,
                ..
            } => {
                let ctp = match deg {
                    4 => ConicType::Quartic,
                    5 => ConicType::Quintic,
                    _ => ConicType::Quadratic,
                };
                let arc = Arc {
                    center,
                    x: x_axis,
                    y: y_axis,
                    r: radius + d,
                    start: 0.0,
                    end: angle,
                };
                let mut cur_q = circ_arc(&arc, ctp)?;
                if twod {
                    cur_q.to_2d();
                }
                return Ok(cur_q);
            }
            CurveType::FreeForm => {}
        }

        // Offset free-form curve.
        let up = self.knots();
        let mp = sx(self.m(), OP)?;
        check_short_degree(self.degree(), OP)?;
        let p = sx(self.degree(), OP)?;

        // Initialize.
        let exp = if deg == 1 { 0.5 } else { 0.34 };

        if float_op_is_bad(1.0, tol, FloatOp::Division) {
            return Err(SolidError::nlib(OP, NlibCode::Numerical));
        }

        let mut f1 = 1.0 / tol;
        f1 = f1.powf(exp);
        let f2 = 2.0 * deg as f64;

        let (b, _) = normalize(b)?;

        // Get Bezier segments.
        let mut bez = self.decompose_bez()?;

        // Get number of sampling points.
        let mut nu: Vec<isize> = Vec::with_capacity(bez.len());
        let mut mu: isize = 0;
        for seg in &mut bez {
            seg.reparam_to_interval(UNIT_SPAN)?;
            let muu = seg.offset_max_2nd_deriv(d, b)?;
            let num = 0.125 * muu;
            let s = num.sqrt();
            let gro = max2(f2, s);
            let mut del = f1 * gro;
            del = del.ceil();
            let c = count(del)?;
            nu.push(c);
            mu = mu.checked_add(c).ok_or_else(unrepresentable)?;
        }
        // HANG-44: the input-scaled bound, from `del` without the curvature term.
        if mu > MAX_OFFSET_SAMPLES && mu > offset_sample_bound(bez.len(), count((f1 * f2).ceil())?)
        {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "too many sample points",
            });
        }

        // Compute parameters.
        let mut u: Vec<f64> = storage(mu, OP)?;
        let mut ul = get(up, p, OP)?;
        let mut i = p + 1;
        let mut j: isize = 0;
        set(&mut u, 0, ul, OP)?;
        let mut l: isize = 0;

        while i < mp {
            while i < mp && get(up, i, OP)? == get(up, i + 1, OP)? {
                i += 1;
            }
            let ur = get(up, i, OP)?;
            let nj = get(&nu, j, OP)?;
            let uinc = (ur - ul) / nj as f64;
            for k in 1..nj {
                l += 1;
                set(&mut u, l, ul + k as f64 * uinc, OP)?;
            }
            l += 1;
            set(&mut u, l, ur, OP)?;
            ul = ur;
            i += 1;
            j += 1;
        }

        // Sample curve.
        let mut pts: Vec<Vec3> = storage(mu, OP)?;
        for i in 0..=mu {
            let (q, ts) = self.eval_tangent(get(&u, i, OP)?, Side::Left)?;
            let n = b.cross(ts);
            set(&mut pts, i, pt_along_vector(q, d, n), OP)?;
        }

        // Get knot vector.
        if let OffsetParam::Fit(m) = par {
            u = param_values(&pts, m)?;
        }
        let knu = if tangents {
            knot_vector_end_derivs(&u, deg)?
        } else {
            interp_knot_vector(&u, deg)?
        };
        let knt = KnotVector::from_slice(&knu)?;

        // Interpolate points.
        let mut cur_q = if tangents {
            // Get end tangents.
            let up0 = get(up, 0, OP)?;
            let upm = get(up, mp, OP)?;
            let cd = self.derivs(up0, Side::Left, 1)?;
            let mut ts = get(&cd, 1, OP)?;
            let cd = self.derivs(upm, Side::Left, 1)?;
            let mut te = get(&cd, 1, OP)?;

            if par != OffsetParam::Inherited {
                // Adjust magnitudes.
                let del = dist_polygon(&pts);
                ts = normalize(ts)?.0;
                te = normalize(te)?.0;
                ts = Vec3::new(del * ts.x, del * ts.y, del * ts.z);
                te = Vec3::new(del * te.x, del * te.y, del * te.z);
            }

            interp_knots_and_derivs(&pts, &u, &knt, deg, ts, te)?
        } else {
            interp_given_params(&pts, &u, &knt, deg)?
        };

        if twod {
            cur_q.to_2d();
        }

        // Remove knots.
        let der = usize::from(tangents && deg == 2);
        cur_q = cur_q.remove_knots_deriv_constraints(tol, ConstrainedEnds::Both, der)?;

        // Compact output curve.
        let (n, _) = cur_q.array_sizes();
        if sx(n, OP)? < mu {
            cur_q.compress();
        }

        // Rescale knot vector if necessary.
        if par != OffsetParam::Inherited {
            let uq = cur_q.knots();
            let mq = sx(cur_q.m(), OP)?;
            let (up0, upm) = (get(up, 0, OP)?, get(up, mp, OP)?);
            if up0 != get(uq, 0, OP)? || upm != get(uq, mq, OP)? {
                cur_q.reparam_to_interval(Interval::new(up0, upm))?;
            }
        }

        Ok(cur_q)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::CPoint;

    fn cubic() -> Curve {
        let pw = vec![
            CPoint::new(0.0, 0.0, 0.0, crate::nurbs::math::NOW),
            CPoint::new(1.0, 2.0, 0.0, crate::nurbs::math::NOW),
            CPoint::new(3.0, -1.0, 0.0, crate::nurbs::math::NOW),
            CPoint::new(4.0, 1.0, 0.0, crate::nurbs::math::NOW),
            CPoint::new(6.0, 0.5, 0.0, crate::nurbs::math::NOW),
        ];
        Curve::new(pw, 3, vec![0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0]).unwrap()
    }

    #[test]
    fn line_offset_is_exact() {
        let pw = vec![
            CPoint::new(0.0, 0.0, 0.0, crate::nurbs::math::NOW),
            CPoint::new(2.0, 0.0, 0.0, crate::nurbs::math::NOW),
        ];
        let c = Curve::new(pw, 1, vec![0.0, 0.0, 1.0, 1.0]).unwrap();
        let q = c
            .offset_pt_sampling(
                Vec3::new(0.0, 0.0, 1.0),
                0.5,
                3,
                false,
                OffsetParam::Inherited,
                1e-4,
            )
            .unwrap();
        let e = q.cpts()[0].to_euclid();
        assert_eq!((e.x, e.y), (0.0, 0.5));
    }

    #[test]
    fn freeform_offset_stays_at_distance() {
        let c = cubic();
        let b = Vec3::new(0.0, 0.0, 1.0);
        for par in [
            OffsetParam::Inherited,
            OffsetParam::Fit(ParamMethod::ChordLength),
            OffsetParam::Fit(ParamMethod::Uniform),
        ] {
            for tan in [false, true] {
                let q = c.offset_pt_sampling(b, 0.1, 3, tan, par, 1e-4).unwrap();
                assert_eq!(q.knots()[0], 0.0);
                assert_eq!(*q.knots().last().unwrap(), 1.0);
                // Start point is the offset of the start point.
                let (p0, t0) = c.eval_tangent(0.0, Side::Left).unwrap();
                let o = pt_along_vector(p0, 0.1, b.cross(t0));
                let s = q.cpts()[0].to_euclid();
                assert!((s - o).length() < 1e-3, "{par:?} {tan}");
            }
        }
    }

    #[test]
    fn bad_tolerance_and_normal() {
        let c = cubic();
        let b = Vec3::new(0.0, 0.0, 1.0);
        let e = c.offset_pt_sampling(b, 0.1, 3, false, OffsetParam::Inherited, 0.0);
        assert!(matches!(
            e,
            Err(SolidError::Nlib {
                code: NlibCode::Numerical,
                ..
            })
        ));
        let e = c.offset_pt_sampling(b, 0.1, 3, false, OffsetParam::Inherited, -1.0);
        assert!(matches!(e, Err(SolidError::InvalidInput { .. })));
        let e = c.offset_pt_sampling(
            Vec3::new(0.0, 0.0, 0.0),
            0.1,
            3,
            false,
            OffsetParam::Inherited,
            1e-3,
        );
        assert!(matches!(
            e,
            Err(SolidError::Nlib {
                code: NlibCode::Geometry,
                ..
            })
        ));
    }

    /// Fuzz regression: a negative weight (a pole) asked for 727 541 samples, and the
    /// quadratic knot removal then ran practically forever; a huge `deg` overflowed.
    #[test]
    fn sample_count_and_degree_are_bounded() {
        // The case of fuzz00366 (sweep `--fuzz 2000`): control point 5 has weight -1.
        let pw = [
            [0.36951137312380355, 1.7382532204389758, 0.5628709619734076],
            [12.952027085779179, -4.973752671925807, 0.7789347719917435],
            [33.302360218733476, -11.741039188433287, 0.9741795453940191],
            [29.8611057374012, -9.406174233113662, 0.6244556071267372],
            [79.77072958426271, -24.682018234069655, 1.2060780576480667],
            [104.66389854976843, -26.779962001231308, -1.0],
            [83.46567157568222, -19.803719016019436, 0.8313196531928522],
            [119.10887201469666, -24.546586356911348, 1.0094490993583305],
        ]
        .iter()
        .map(|&[x, y, w]| CPoint::new(x, y, 0.0, w))
        .collect();
        let u = vec![
            0.0,
            0.0,
            0.0,
            0.0,
            0.28545366853925924,
            0.5532644864792337,
            0.7403247015519675,
            0.8424203210022205,
            1.0,
            1.0,
            1.0,
            1.0,
        ];
        let c = Curve::new(pw, 3, u).unwrap();
        // HANG-44 addendum 2026-10-29: still refused under the input-scaled bound (5
        // segments, floor 13 each, so the bound is 16 384), before any sampling.
        let clock = std::time::Instant::now();
        let r = c.offset_pt_sampling(
            Vec3::new(0.0, 0.0, 1.0),
            0.0,
            3,
            false,
            OffsetParam::Inherited,
            0.11799393559359235,
        );
        let took = clock.elapsed();
        eprintln!("fuzz00366 refused in {took:?}");
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
        let r = cubic().offset_pt_sampling(
            Vec3::new(0.0, 0.0, 1.0),
            0.1,
            1 << 32,
            false,
            OffsetParam::Inherited,
            0.01,
        );
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
    }

    /// HANG-44 addendum 2026-10-29: the bound scales with the segment count and the
    /// tolerance, never drops below the former cap and never exceeds the ceiling.
    #[test]
    fn sample_bound_is_input_scaled() {
        assert_eq!(offset_sample_bound(5, 13), MAX_OFFSET_SAMPLES);
        assert_eq!(offset_sample_bound(60, 301), 4 * 60 * 301);
        assert_eq!(offset_sample_bound(127, 301), MAX_OFFSET_SAMPLES_ANY);
        assert_eq!(
            offset_sample_bound(usize::MAX, isize::MAX),
            MAX_OFFSET_SAMPLES_ANY
        );
        assert_eq!(offset_sample_bound(0, 0), MAX_OFFSET_SAMPLES);
    }

    /// A tiny tolerance asks for about 72 000 samples per segment even on a gentle
    /// curve: refused at once by the ceiling.
    #[test]
    fn tiny_tolerance_is_refused_quickly() {
        let clock = std::time::Instant::now();
        let r = cubic().offset_pt_sampling(
            Vec3::new(0.0, 0.0, 1.0),
            0.1,
            3,
            false,
            OffsetParam::Inherited,
            1e-12,
        );
        let took = clock.elapsed();
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    }
}
