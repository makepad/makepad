//! Curve geometry: lines, conics, B-splines, offset/projected/iso/composite curves,
//! as a concrete `enum` with evaluation, derivatives, closest point and length.
//!
//! Upstream: `SmCurve`, `SmLine`, `SmCircle`, `SmEllipse`, `SmParabola`,
//! `SmHyperbola`, `SmBSplineCurve`, `SmOffsetCurve`, `SmCompositeCurve`, `SmIsoCurve`,
//! `SmProjectedCurve`, `SmCurveClass` in `source/SMLib`, and `SM_API/SmApiCurves`.
//!
//! Deviations:
//! - **Derivative orders are bounded** by [`MAX_EVAL_DERIVS`]. Upstream `Evaluate`
//!   writes `nDerivs + 1` vectors into a caller-supplied array, so any order is
//!   accepted there (analytic kinds zero-fill the orders they cannot compute,
//!   B-splines fail at `NL_MAXDER`). The port returns a `Vec` sized by the order,
//!   so a caller order would size an allocation (`usize::MAX` overflows, `2^31`
//!   exhausts memory). Every allocating evaluator of this module (and the
//!   [`curve::Curve::evaluate`] dispatcher) calls [`check_eval_derivs`] before allocating and
//!   returns [`SolidError::InvalidInput`] above the bound; below it results are
//!   unchanged.

pub mod composite_curve;
pub mod conic;
pub mod crv_in_volume;
pub mod crv_on_surf;
#[allow(clippy::module_inception)]
pub mod curve;
pub mod curve_cache;
pub mod ellipse;
pub mod hermite_curve;
pub mod hyperbola;
pub mod iso_curve;
pub mod line;
pub mod offset_curve;
pub mod parabola;
pub mod projected_curve;
pub mod tangent_field;

use crate::error::{Result, SolidError};

/// Highest derivative order (`num_derivs`) the curve evaluators accept.
///
/// Upstream callers ask for at most 5 (`NL_MAXDER`, `GW_MAX_DERIV`); the bound is
/// well above that and above every FFI guard (16), and only stops a caller order
/// from sizing an unbounded output allocation.
pub const MAX_EVAL_DERIVS: usize = 64;

/// [`SolidError::InvalidInput`] when `num_derivs` exceeds [`MAX_EVAL_DERIVS`].
///
/// # Errors
///
/// [`SolidError::InvalidInput`] with `operation` for an order above the bound.
pub fn check_eval_derivs(num_derivs: usize, operation: &'static str) -> Result<()> {
    if num_derivs > MAX_EVAL_DERIVS {
        return Err(SolidError::invalid_input(
            operation,
            "derivative order above MAX_EVAL_DERIVS",
        ));
    }
    Ok(())
}
