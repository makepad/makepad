//! `SmBSplineSurface` with NLib: the least-squares approximation of an ordered grid of
//! points.
//!
//! Upstream: `source/SMLib/src/SmBSplineSurfaceNLib.cpp`, `source/SMLib/inc/SmBSplineSurface.h`.
//!
//! | Rust                                   | Upstream                               |
//! |----------------------------------------|----------------------------------------|
//! | [`BSplineSurface::approximate_points`] | `SmBSplineSurface::ApproximatePoints`  |
//!
//! Owned by no plan unit (`SmBSplineSurface::ApproximatePoints` is outside
//! `harness/callgraph/units.json`); completion for `SmApiCreateSurfaceFromOrderedPoints`
//! (`primitive.api_surface_ops`).
//!
//! Deviations:
//! - **Memory and contexts.** `SmNLibStackHandler`, `NL_STACKS` and the `SmContext` have
//!   no counterpart; the new surface is the return value (upstream out-param).
//! - **Tolerance form not ported.** [`BSplineSurface::approximate_points`] is the
//!   `pdOptTolerance == NULL` form (the only one `SmApiCreateSurfaceFromOrderedPoints`
//!   takes): it has no tolerance or `bParamOption` argument. The tolerance form
//!   (`N_FitSrfApproxTol`, `N_FitSrfLstSqKnots`, `N_FitSrfApproxRemoveKnots`,
//!   `N_FitSrfRemovalBoundary`, `N_GetClosestPtOnSrf`, `N_CFuncFromKnots`,
//!   `N_Pts2dGetMaxExtent`; reached upstream only from `SmBSplineSurface::RebuildSurface`,
//!   in `harness/callgraph/unused_functions.txt`, and `prog_test`) is a follow-up.
//! - **Short input.** Fewer points than `num_rows * num_columns`, or a size product that
//!   overflows, is `InvalidInput` here. `SM_ASSERT(lUDegree < lNumRows && lVDegree <
//!   lNumCols)` is debug-only upstream and is not ported: `N_FitSrfLstSqApprox` rejects
//!   those grids.
//! - **Empty grid.** No rows or columns gives upstream a highest index of `-1`: it first
//!   allocates the grid (`N_AllocPt2dArray(rows - 1, cols - 1)`, zero-sized blocks; with
//!   one dimension `0` it still makes and fills a row-pointer array as long as the other),
//!   copies nothing, and `N_FitSrfLstSqApprox` then rejects `n < 1` (`SM_ERR`). The port
//!   returns the same `SM_ERR` without allocating or calling it.
//! - **Recursion.** None.

use crate::error::{Result, SolidError};
use crate::math::{Point3, Vec3};
use crate::nurbs::b_spline_surface::BSplineSurface;
use crate::nurbs::crv_fit::p1::ParamMethod;
use crate::nurbs::srf_fit::lst_sq_approx;

const OP: &str = "nurbs::b_spline_surface_n_lib::approx::approximate_points";

impl BSplineSurface {
    /// The surface of degrees `u_degree` x `v_degree` approximating the grid `points`
    /// (`num_rows` rows of `num_columns` points, row by row: `P[row][col] =
    /// points[row * num_columns + col]`) (`SmBSplineSurface::ApproximatePoints`).
    ///
    /// This is the form without a tolerance (`pdOptTolerance == NULL`): the least-squares
    /// fit `N_FitSrfLstSqApprox` with one control point per sample (chord-length
    /// parameters), through the corner rows and columns. The tolerance form is not ported
    /// (see the module docs).
    ///
    /// # Errors
    ///
    /// `InvalidInput` when `points` holds fewer than `num_rows * num_columns` points;
    /// `SM_ERR` when the fit fails (no rows or columns, fewer rows or columns than the
    /// degree plus one, a degenerate grid).
    pub fn approximate_points(
        points: &[Point3],
        num_rows: u64,
        num_columns: u64,
        u_degree: u64,
        v_degree: u64,
    ) -> Result<Self> {
        let rows = usize::try_from(num_rows)
            .map_err(|_| SolidError::invalid_input(OP, "too many rows"))?;
        let cols = usize::try_from(num_columns)
            .map_err(|_| SolidError::invalid_input(OP, "too many columns"))?;
        let count = rows
            .checked_mul(cols)
            .ok_or_else(|| SolidError::invalid_input(OP, "rows times columns overflows"))?;
        if points.len() < count {
            return Err(SolidError::invalid_input(OP, "fewer points than the grid"));
        }

        // set N_FitSrfLstSqApprox() input arguments: P[row][col] = p[row*lNumCols + col]
        let mut grid: Vec<Vec<Vec3>> = Vec::new();
        for row in points.chunks(cols.max(1)).take(rows) {
            grid.push(row.to_vec());
        }

        let p = usize::try_from(u_degree).map_err(|_| SolidError::sm_err(OP))?;
        let q = usize::try_from(v_degree).map_err(|_| SolidError::sm_err(OP))?;

        // Least-squares NURBS approximation for a sampled grid of points: one control point
        // per sample, chord-length parameters. No rows or columns is a highest index of -1,
        // which N_FitSrfLstSqApprox rejects (NL_SER: SM_ERR).
        let (Some(n), Some(m)) = (rows.checked_sub(1), cols.checked_sub(1)) else {
            return Err(SolidError::sm_err(OP));
        };
        let surface = lst_sq_approx(&grid, n, m, p, q, ParamMethod::ChordLength)
            .map_err(SolidError::nl_ser(OP))?;

        // set output
        Ok(Self::from_gw_nurb(&surface))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: usize, cols: usize) -> Vec<Point3> {
        let mut v = Vec::new();
        for i in 0..rows {
            for j in 0..cols {
                let (x, y) = (i as f64, j as f64);
                v.push(Vec3::new(x, y, (x * 0.3).sin() + (y * 0.2).cos()));
            }
        }
        v
    }

    #[test]
    fn a_grid_is_fitted_with_one_control_point_per_sample() {
        let s = BSplineSurface::approximate_points(&grid(5, 7), 5, 7, 3, 3).unwrap();
        let nurb = s.gw_nurb().unwrap();
        let ix = nurb.indices();
        assert_eq!((ix.n, ix.m, ix.p, ix.q), (4, 6, 3, 3));
    }

    #[test]
    fn short_and_empty_grids_fail() {
        assert!(matches!(
            BSplineSurface::approximate_points(&grid(3, 3), 4, 4, 3, 3),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            BSplineSurface::approximate_points(&[], 0, 4, 3, 3),
            Err(SolidError::Upstream { .. })
        ));
        assert!(matches!(
            BSplineSurface::approximate_points(&grid(3, 7), 3, 7, 3, 3),
            Err(SolidError::Upstream { .. })
        ));
    }
}
