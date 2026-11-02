//! `SM_API` surface utilities: cubic B-spline surfaces from a control net or four
//! corners, surface offsets, and dropping curves onto or lifting UV curves off a
//! surface.
//!
//! Upstream: `source/SM_API/src/SmApiSurfaces.cpp`, `source/SM_API/src/SmApiCurves.cpp`,
//! `source/SM_API/inc/SmApiSurfaces.h`, `source/SM_API/inc/SmApiCurves.h`.
//!
//! | Rust                                  | Upstream                              |
//! |---------------------------------------|---------------------------------------|
//! | [`create_surface`]                    | `SmApiCreateSurface`                  |
//! | [`create_surface_from_corner_points`] | `SmApiCreateSurfaceFromCornerPoints`  |
//! | [`create_surface_from_ordered_points`] | `SmApiCreateSurfaceFromOrderedPoints` |
//! | [`create_offset_surface`]             | `SmApiCreateOffsetSurface`            |
//! | [`drop_curve_to_srf`], [`DroppedCurves`] | `SmApiDropCurveToSrf`              |
//! | [`lift_uv_curve_from_srf`]            | `SmApiLiftUVCurveFromSrf`             |
//!
//! Deviations:
//! - **Out-params.** Results are return values. [`drop_curve_to_srf`] returns the UV
//!   curves, and the 3D curves when `lift` is set, instead of appending to the caller's
//!   optional arrays (on failure upstream appends nothing either);
//!   [`lift_uv_curve_from_srf`] returns the one lifted curve.
//! - **Context.** `SmApiGetOrCreateContext()` (and the surface's context in
//!   `SmApiDropCurveToSrf`) is the caller's [`Kernel`].
//! - **Counts.** `SmApiCreateSurface` compares `lNumRows * lNumColumns` (an `unsigned
//!   long` product that wraps) with the point count; a product that overflows `u64` is
//!   `InvalidInput` here (upstream would go on to build knot vectors of the wrapped
//!   sizes).
//! - **Statuses.** `SM_ERR_INVALID_INPUT` is [`SolidError::InvalidInput`]; the called
//!   kernel functions' errors pass through unchanged.

use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::tol::ZONE_TOL_3D;
use crate::geom::vector3d::Point3;
use crate::kernel::Kernel;
use crate::nurbs::b_spline_curve::KnotType;
use crate::nurbs::b_spline_surface::{BSplineSurface, BSplineSurfaceForm, CanonicalSurfaceData};
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::Surface;

const OP_SURFACE: &str = "primitive::api_surface_ops::create_surface";
const OP_OFFSET: &str = "primitive::api_surface_ops::create_offset_surface";
const OP_ORDERED: &str = "primitive::api_surface_ops::create_surface_from_ordered_points";

/// The cubic B-spline surface with the control net `control_points` (`num_rows` rows
/// of `num_columns` points), clamped knots 0 and 1 at the ends and uniform interior
/// knots `i / (k - 1)` (`k = rows - 2` in u, `columns - 2` in v) (`SmApiCreateSurface`,
/// through `SmBSplineSurface::CreateCanonical`).
///
/// # Errors
///
/// `InvalidInput` when the net size is not the point count or fewer than 4 rows or
/// columns are given; those of [`BSplineSurface::create_canonical`].
pub fn create_surface(
    control_points: &[Point3],
    num_rows: u64,
    num_columns: u64,
) -> Result<BSplineSurface> {
    let count = u64::try_from(control_points.len())
        .map_err(|_| SolidError::invalid_input(OP_SURFACE, "too many points"))?;
    let net = num_rows
        .checked_mul(num_columns)
        .ok_or_else(|| SolidError::invalid_input(OP_SURFACE, "net size overflows"))?;
    if net != count {
        return Err(SolidError::invalid_input(
            OP_SURFACE,
            "rows times columns is not the point count",
        ));
    }

    let degree_u: u64 = 3;
    let degree_v: u64 = 3;
    if num_rows < degree_u + 1 || num_columns < degree_v + 1 {
        return Err(SolidError::invalid_input(
            OP_SURFACE,
            "fewer than 4 rows or columns",
        ));
    }

    // (the counts are bounded by the point count above)
    let num_knots_u = usize::try_from(num_rows - degree_u + 1)
        .map_err(|_| SolidError::invalid_input(OP_SURFACE, "too many rows"))?;
    let num_knots_v = usize::try_from(num_columns - degree_v + 1)
        .map_err(|_| SolidError::invalid_input(OP_SURFACE, "too many columns"))?;
    let knots = |n: usize| -> Vec<f64> {
        let mut k = Vec::new();
        k.push(0.0);
        for ii in 1..n - 1 {
            k.push((ii as f64) / ((n - 1) as f64));
        }
        k.push(1.0);
        k
    };
    let multiplicities = |n: usize, degree: usize| -> Vec<usize> {
        let mut m = Vec::new();
        m.push(degree + 1);
        m.extend(core::iter::repeat_n(1, n - 2));
        m.push(degree + 1);
        m
    };
    let u_knots = knots(num_knots_u);
    let v_knots = knots(num_knots_v);
    let u_multiplicities = multiplicities(num_knots_u, 3);
    let v_multiplicities = multiplicities(num_knots_v, 3);

    BSplineSurface::create_canonical(&CanonicalSurfaceData {
        u_degree: 3,
        v_degree: 3,
        control_points,
        form: BSplineSurfaceForm::Unspecified,
        u_multiplicities: &u_multiplicities,
        v_multiplicities: &v_multiplicities,
        u_knots: &u_knots,
        v_knots: &v_knots,
        knot_type: KnotType::Unspecified,
        weights: None,
        uv_domain: None,
    })
}

/// The bilinear B-spline surface through four corners: `S(0,0) = umin_vmin`,
/// `S(1,0) = umax_vmin`, `S(0,1) = umin_vmax`, `S(1,1) = umax_vmax`
/// (`SmApiCreateSurfaceFromCornerPoints`, through
/// `SmBSplineSurface::CreateBilinearSurface`).
///
/// # Errors
///
/// Those of [`BSplineSurface::create_bilinear_surface`].
pub fn create_surface_from_corner_points(
    umin_vmin: Point3,
    umax_vmin: Point3,
    umin_vmax: Point3,
    umax_vmax: Point3,
) -> Result<BSplineSurface> {
    BSplineSurface::create_bilinear_surface(umin_vmin, umax_vmin, umin_vmax, umax_vmax)
}

/// The bicubic B-spline surface approximating the ordered grid `points` (`num_rows`
/// rows of `num_columns` points, row by row) in the least-squares sense, one control
/// point per sample (`SmApiCreateSurfaceFromOrderedPoints`, through
/// [`BSplineSurface::approximate_points`] with degrees 3: upstream's
/// `SmBSplineSurface::ApproximatePoints` with `pdOptTolerance == NULL`).
///
/// # Errors
///
/// `InvalidInput` when the grid size is not the point count; `SM_ERR` when the fit
/// fails (fewer than 4 rows or columns, a degenerate grid).
pub fn create_surface_from_ordered_points(
    points: &[Point3],
    num_rows: u64,
    num_columns: u64,
) -> Result<BSplineSurface> {
    let degree_u = 3;
    let degree_v = 3;

    let count = u64::try_from(points.len())
        .map_err(|_| SolidError::invalid_input(OP_ORDERED, "too many points"))?;
    if num_rows.checked_mul(num_columns) != Some(count) {
        return Err(SolidError::invalid_input(
            OP_ORDERED,
            "rows times columns is not the point count",
        ));
    }

    BSplineSurface::approximate_points(points, num_rows, num_columns, degree_u, degree_v)
}

/// The offset of `surface` by `distance` along its normal (`SmApiCreateOffsetSurface`,
/// through the surface's `CreateOffsetSurface` with approximation tolerance `0.001`).
///
/// # Errors
///
/// `SM_ERR` when the offset makes no surface (a fully collapsed cone); those of
/// [`Surface::create_offset_surface`].
pub fn create_offset_surface(k: &Kernel, surface: &Surface, distance: f64) -> Result<Surface> {
    let approx_tol = 0.001;
    surface
        .create_offset_surface(k, distance, approx_tol)?
        .ok_or_else(|| SolidError::sm_err(OP_OFFSET))
}

/// The results of [`drop_curve_to_srf`].
#[derive(Debug, Clone)]
pub struct DroppedCurves {
    /// `pOptCurves2d`: one UV curve per piece of the curve over the surface.
    pub uv_curves: Vec<Curve>,
    /// `pOptCurves3d`: the 3D curve of each UV curve, when asked for.
    pub curves_3d: Option<Vec<Curve>>,
}

/// Drops `curve` onto `surface` along the surface normals (`SmApiDropCurveToSrf`):
/// `DropAndTrimCurve` over the natural UV domain with approximation tolerance `10`
/// (keeping every piece), then, with `lift`, `LiftCurve` of each UV curve with tolerance
/// `0.001`.
///
/// # Errors
///
/// Those of [`Surface::drop_and_trim_curve`] and [`Surface::lift_curve`].
pub fn drop_curve_to_srf(
    k: &Kernel,
    curve: &Curve,
    surface: &Surface,
    lift: bool,
) -> Result<DroppedCurves> {
    let approx_tol = 10.0;
    let dropped = surface.drop_and_trim_curve::<NoObject>(
        k,
        &surface.natural_uv_domain_with_nurb(k)?,
        curve,
        &curve.natural_interval(),
        approx_tol,
        true,
        false,
        None,
    )?;

    let curves_3d = if lift {
        let mut out = Vec::new();
        for uv in &dropped.uv_curves {
            let lifted = surface.lift_curve(
                k,
                &surface.natural_uv_domain_with_nurb(k)?,
                uv,
                &uv.natural_interval(),
                0.001,
                false,
            )?;
            out.push(lifted.curve);
        }
        Some(out)
    } else {
        None
    };
    Ok(DroppedCurves {
        uv_curves: dropped.uv_curves,
        curves_3d,
    })
}

/// The 3D curve of the UV curve `uv_curve` on `surface` over its natural UV domain
/// (`SmApiLiftUVCurveFromSrf`, through `LiftCurve` with tolerance `SM_ZONE_TOL_3D`).
///
/// # Errors
///
/// Those of [`Surface::lift_curve`].
pub fn lift_uv_curve_from_srf(k: &Kernel, uv_curve: &Curve, surface: &Surface) -> Result<Curve> {
    Ok(surface
        .lift_curve(
            k,
            &surface.natural_uv_domain_with_nurb(k)?,
            uv_curve,
            &uv_curve.natural_interval(),
            ZONE_TOL_3D,
            false,
        )?
        .curve)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64, z: f64) -> Point3 {
        Point3::new(x, y, z)
    }

    fn grid(rows: u32, cols: u32) -> Vec<Point3> {
        let mut v = Vec::new();
        for i in 0..rows {
            for j in 0..cols {
                v.push(p(f64::from(i), f64::from(j), f64::from(i * j) * 0.1));
            }
        }
        v
    }

    #[test]
    fn create_surface_checks_the_net() {
        assert!(matches!(
            create_surface(&grid(4, 4), 4, 5),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            create_surface(&grid(3, 4), 3, 4),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            create_surface(&[], u64::MAX, 2),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(create_surface(&grid(5, 7), 5, 7).is_ok());
    }

    #[test]
    fn corner_surface_and_offset() {
        let k = Kernel::default();
        let s = create_surface_from_corner_points(
            p(0.0, 0.0, 0.0),
            p(10.0, 0.0, 0.0),
            p(0.0, 10.0, 0.0),
            p(10.0, 10.0, 0.0),
        )
        .unwrap();
        let off = create_offset_surface(&k, &Surface::from(s), 2.0).unwrap();
        let pt = off
            .evaluate_point(&k, crate::geom::vector2d::Point2::new(0.5, 0.5))
            .unwrap();
        assert!((pt.z.abs() - 2.0).abs() < 1.0e-6);
    }
}
