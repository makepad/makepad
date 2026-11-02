//! `SM_API` curve utilities: interpolation through points, simple offsets, continuity
//! at a parameter, making B-spline curves compatible and ordering curves into loops.
//!
//! Upstream: `source/SM_API/src/SmApiCurves.cpp`, `source/SM_API/inc/SmApiCurves.h`.
//!
//! | Rust                           | Upstream                          |
//! |--------------------------------|-----------------------------------|
//! | [`create_curve_interp_points`] | `SmApiCreateCurveInterpPoints`    |
//! | [`offset_curve`]               | `SmApiOffsetCurve`                |
//! | [`evaluate_continuity`]        | `SmApiEvaluateContinuity`         |
//! | [`make_curves_compatible`]     | `SmApiMakeCurvesCompatible`       |
//! | [`order_curves`]               | `SmApiOrderCurves`                |
//!
//! Deviations:
//! - **Out-params.** Results are return values. [`order_curves`] returns the new order
//!   as indices into the input instead of reordering the caller's array in place (the
//!   caller applies it); on failure there is none, as upstream leaves the array alone.
//! - **Context.** `SmApiGetOrCreateContext()` is the caller's [`Kernel`].
//! - **Parameterization.** `SmCurveParameterizationType` is [`ParamMethod`]: upstream's
//!   check for a value outside `SM_CP_UNIFORM .. SM_CP_CENTRIPETAL` cannot fail here.
//! - **Curve types.** Upstream's `SmBSplineCurve*` arguments reach the virtual
//!   `CreateSimpleOffset` of the object's class: [`offset_curve`] takes a [`Curve`] and
//!   dispatches to the B-spline, line (`SmLine`) and ellipse / circle (`SmEllipse`,
//!   `SmCircle`) overrides; another curve kind (not an `SmBSplineCurve` upstream) is
//!   `InvalidInput`. [`make_curves_compatible`] takes plain [`BSplineCurve`]s.
//! - **`GetEnds`.** [`offset_curve`] returns the error of the curve's end points (a
//!   curve without NURB), where upstream ignores the status and reads uninitialised
//!   points.
//! - **Statuses.** `SM_ERR_INVALID_INPUT` is [`SolidError::InvalidInput`]; the called
//!   kernel functions' errors pass through unchanged.

use crate::curve::curve::p2::evaluate_continuity as curve_continuity;
use crate::curve::curve::p3::compute_crvs_normal;
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignored, Result, SolidError};
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::tol::ZONE_TOL_3D;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, EFF_ZERO_SQ};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::crv_fit::p1::ParamMethod;
use crate::topology::trimming_tools::order_curves_into_loops;

const OP_INTERP: &str = "primitive::api_curve_ops::create_curve_interp_points";
const OP_OFFSET: &str = "primitive::api_curve_ops::offset_curve";
const OP_CONTINUITY: &str = "primitive::api_curve_ops::evaluate_continuity";
const OP_COMPATIBLE: &str = "primitive::api_curve_ops::make_curves_compatible";
const OP_ORDER: &str = "primitive::api_curve_ops::order_curves";

/// `SM_CONTINUITY_ANGLE` (degrees): the default G1 angle of `SmCurve::EvaluateContinuity`.
const CONTINUITY_ANGLE: f64 = 0.5;

/// `SmBSplineCurve::MakeCurvesCompatible`'s default knot tolerance.
const COMPATIBLE_KNOT_TOL: f64 = 1.0e-8;

/// The cubic B-spline interpolating `points` (at least 4, all defined) with knots by
/// `parameterization` (`SmApiCreateCurveInterpPoints`, through
/// `SmBSplineCurve::CreateInterpolatingCurve` with dimension 3, degree 3 and no
/// vectors). The chord-length and centripetal parameterizations need consecutive
/// points farther apart than `SM_EFF_ZERO`.
///
/// # Errors
///
/// `InvalidInput` for fewer than 4 points, an undefined point, or coincident
/// consecutive points with a distance-based parameterization; those of
/// [`BSplineCurve::create_interpolating_curve`].
pub fn create_curve_interp_points(
    points: &[Point3],
    parameterization: ParamMethod,
) -> Result<BSplineCurve> {
    if points.len() < 4 {
        return Err(SolidError::invalid_input(OP_INTERP, "fewer than 4 points"));
    }
    if points.iter().any(|p| p.is_undef()) {
        return Err(SolidError::invalid_input(OP_INTERP, "undefined point"));
    }
    if matches!(
        parameterization,
        ParamMethod::ChordLength | ParamMethod::Centripetal
    ) && points.windows(2).any(|w| match w {
        [a, b] => b.distance_squared(*a) <= EFF_ZERO_SQ,
        _ => false,
    }) {
        return Err(SolidError::invalid_input(OP_INTERP, "coincident points"));
    }
    BSplineCurve::create_interpolating_curve(parameterization, 3, 3, points, &[], None, false)
}

/// The offset of `curve` by `distance` (positive to the right of the curve) in the
/// plane of the curve (`SmApiOffsetCurve`): the normal is `SmCurve::ComputeCrvsNormal`'s;
/// when that fails, `(0, 0, 1)`, or `(0, 1, 0)` when the chord between the end points is
/// parallel to it. The offset is the curve class's `CreateSimpleOffset` with
/// approximation tolerance `0.001`.
///
/// # Errors
///
/// `InvalidInput` for a curve that is not a B-spline, line, ellipse or circle; the
/// errors of the end points and of the class's `CreateSimpleOffset`.
pub fn offset_curve(k: &Kernel, curve: &Curve, distance: f64) -> Result<Curve> {
    let approx_tol = 0.001;

    // (the curve must be an SmBSplineCurve: checked before any work)
    let base = match curve.kind() {
        CurveKind::BSpline(c) => c,
        CurveKind::Line(l) => l.base(),
        CurveKind::Ellipse(e) => e.base(),
        _ => return Err(SolidError::invalid_input(OP_OFFSET, "not a B-spline curve")),
    };

    let normal = if let Some(n) = ignored(compute_crvs_normal(k, core::slice::from_ref(curve)))? {
        n
    } else {
        let mut n = Vec3::new(0.0, 0.0, 1.0);
        // (the class's virtual GetEnds)
        let ends = curve.ends(k)?;
        let dir = ends.end - ends.start;
        if let Ok(dir) = dir.unitize() {
            if dir.cross(n).length() < EFF_ZERO {
                n = Vec3::new(0.0, 1.0, 0.0);
            }
        }
        n
    };

    match curve.kind() {
        CurveKind::Line(l) => l
            .create_simple_offset(normal, distance)
            .map(|(c, _)| Curve::from(c)),
        CurveKind::Ellipse(e) => e
            .create_simple_offset(k, approx_tol, normal, distance, false)
            .map(|(c, _)| c),
        _ => base
            .create_simple_offset(k, approx_tol, normal, distance, false)
            .map(|(c, _)| Curve::from(c)),
    }
}

/// The continuity of `curve` at `parameter` between its evaluations from the left and
/// from the right (`SmApiEvaluateContinuity`, through `SmCurve::EvaluateContinuity`
/// with the default G1 angle).
///
/// # Errors
///
/// `InvalidInput` for a parameter outside the curve's natural interval (or NaN); those
/// of [`crate::curve::curve::p2::evaluate_continuity`].
pub fn evaluate_continuity(k: &Kernel, curve: &Curve, parameter: f64) -> Result<ContinuityType> {
    let domain = curve.natural_interval();
    // (a NaN parameter is outside)
    if !(domain.min()..=domain.max()).contains(&parameter) {
        return Err(SolidError::invalid_input(
            OP_CONTINUITY,
            "parameter outside the curve's interval",
        ));
    }
    Ok(curve_continuity(
        k,
        curve,
        parameter,
        true,
        curve,
        parameter,
        false,
        CONTINUITY_ANGLE,
    )?
    .continuity)
}

/// Makes the B-spline curves `curves` compatible in place (the same degree and knot
/// vector; `SmApiMakeCurvesCompatible`, through `SmBSplineCurve::MakeCurvesCompatible`
/// with its default knot tolerance `1e-8`). The curves are updated one at a time, so a
/// failure can leave some already modified.
///
/// # Errors
///
/// `InvalidInput` for fewer than 2 curves; those of
/// [`BSplineCurve::make_curves_compatible`].
pub fn make_curves_compatible(curves: &mut [BSplineCurve]) -> Result<()> {
    if curves.len() < 2 {
        return Err(SolidError::invalid_input(
            OP_COMPATIBLE,
            "fewer than 2 curves",
        ));
    }
    BSplineCurve::make_curves_compatible(curves, COMPATIBLE_KNOT_TOL)
}

/// The order of `curves` in loop sequence, as indices into `curves`
/// (`SmApiOrderCurves`, through `SmTrimmingTools::OrderCurvesIntoLoops` with tolerance
/// `SM_ZONE_TOL_3D`). Curves are not reversed and the loop boundaries are not
/// reported, so consecutive curves need not meet head to tail.
///
/// # Errors
///
/// `InvalidInput` for fewer than 2 curves; `SM_ERR` when the curves do not form closed
/// planar loops or none are ordered; those of
/// [`order_curves_into_loops`].
pub fn order_curves(k: &Kernel, curves: &[Curve]) -> Result<Vec<usize>> {
    if curves.len() < 2 {
        return Err(SolidError::invalid_input(OP_ORDER, "fewer than 2 curves"));
    }
    let loops = order_curves_into_loops(k, curves, ZONE_TOL_3D)?;
    loops.status()?;
    if loops.ordered.is_empty() {
        return Err(SolidError::sm_err(OP_ORDER));
    }
    Ok(loops.ordered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitive::api_curves::{create_circle, create_curve, create_rectangle};

    fn p(x: f64, y: f64, z: f64) -> Point3 {
        Point3::new(x, y, z)
    }

    #[test]
    fn interp_checks_inputs() {
        let pts = [p(0.0, 0.0, 0.0), p(1.0, 1.0, 0.0), p(2.0, 0.0, 0.0)];
        assert!(matches!(
            create_curve_interp_points(&pts, ParamMethod::Uniform),
            Err(SolidError::InvalidInput { .. })
        ));
        let dup = [
            p(0.0, 0.0, 0.0),
            p(0.0, 0.0, 0.0),
            p(1.0, 1.0, 0.0),
            p(2.0, 0.0, 0.0),
        ];
        assert!(matches!(
            create_curve_interp_points(&dup, ParamMethod::ChordLength),
            Err(SolidError::InvalidInput { .. })
        ));
        let ok = [
            p(0.0, 0.0, 0.0),
            p(1.0, 1.0, 0.0),
            p(2.0, 0.0, 0.0),
            p(3.0, 1.0, 0.0),
        ];
        assert!(create_curve_interp_points(&ok, ParamMethod::Centripetal).is_ok());
    }

    #[test]
    fn offset_of_a_straight_curve_stays_in_its_plane() {
        let k = Kernel::default();
        let pts: Vec<Point3> = (0..4).map(|i| p(f64::from(i) * 10.0, 0.0, 0.0)).collect();
        let line = Curve::from(create_curve(&pts).unwrap());
        let off = offset_curve(&k, &line, 2.0).unwrap();
        let interval = off.natural_interval();
        let start = off.evaluate_point(&k, interval.min()).unwrap();
        assert!(start.z.abs() < 1.0e-9);
        assert!((start.y.abs() - 2.0).abs() < 1.0e-6);
    }

    #[test]
    fn continuity_rejects_outside_parameters() {
        let k = Kernel::default();
        let c = Curve::from(create_circle(p(0.0, 0.0, 0.0), 5.0).unwrap());
        let ivl = c.natural_interval();
        assert!(evaluate_continuity(&k, &c, ivl.max() + 1.0).is_err());
        assert!(evaluate_continuity(&k, &c, f64::NAN).is_err());
        assert!(evaluate_continuity(&k, &c, ivl.evaluate(0.5)).is_ok());
    }

    #[test]
    fn compatible_and_order_need_two_curves() {
        let k = Kernel::default();
        assert!(make_curves_compatible(&mut []).is_err());
        let mut circles = vec![
            create_circle(p(0.0, 0.0, 0.0), 5.0).unwrap(),
            create_circle(p(0.0, 0.0, 5.0), 3.0).unwrap(),
        ];
        assert!(make_curves_compatible(&mut circles).is_ok());
        assert!(order_curves(&k, &[]).is_err());
        let rect: Vec<Curve> = create_rectangle(p(0.0, 0.0, 0.0), 10.0, 6.0)
            .unwrap()
            .into_iter()
            .map(Curve::from)
            .collect();
        let order = order_curves(&k, &rect).unwrap();
        assert_eq!(order.len(), 4);
    }
}
