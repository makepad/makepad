//! Advanced NURBS surface routines: grid evaluation of points and normals, surface
//! extension by a distance or a parameter distance, derivatives of the unit normal,
//! the swung, skinned and translational sweep surface constructions, Coons surfaces
//! and cross-boundary derivative tools.
//!
//! Upstream: `source/SMLib/src/NL_SrfAdv.cpp` (declarations in
//! `source/SMLib/inc/NL_SrfAdv.h`). The file is split over several plan units
//! (`nlib.srf_adv_p1`, `_p2`, ...); each lives in its own submodule.

pub mod p1;
pub mod p2;
pub mod p3;
pub mod p4;
pub mod p5;
pub mod p6;

use crate::error::{NlibCode, Result, SolidError};

/// `len` copies of `x` (an upstream `N_Alloc*` array, filled), or `NL_MEM_ERR` where
/// the storage cannot be allocated (as upstream's failed `N_Malloc`).
fn filled<T: Clone>(len: usize, x: T, op: &'static str) -> Result<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(len)
        .map_err(|_| SolidError::nlib(op, NlibCode::Memory))?;
    v.resize(len, x);
    Ok(v)
}

/// A `rows x cols` array of `x` (an upstream `N_Alloc*2dArray`, filled), or
/// `NL_MEM_ERR` where the storage cannot be allocated.
fn grid<T: Clone>(rows: usize, cols: usize, x: T, op: &'static str) -> Result<Vec<Vec<T>>> {
    let mut w = Vec::new();
    w.try_reserve_exact(rows)
        .map_err(|_| SolidError::nlib(op, NlibCode::Memory))?;
    for _ in 0..rows {
        w.push(filled(cols, x.clone(), op)?);
    }
    Ok(w)
}

/// Largest degree upstream can express (`NL_DEGREE` is a `short`).
const DEGREE_RANGE_MAX: usize = 32767;

/// `p` unchanged, or `InvalidInput` above [`DEGREE_RANGE_MAX`] (a degree upstream
/// cannot express; bounds the index arithmetic and scratch sizes derived from it).
fn degree(p: usize, op: &'static str) -> Result<usize> {
    if p > DEGREE_RANGE_MAX {
        return Err(SolidError::InvalidInput {
            operation: op,
            reason: "degree above upstream's NL_DEGREE (short) range",
        });
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_failures_and_degree_range_are_errors() {
        assert!(filled(usize::MAX, 0.0_f64, "t").is_err());
        // Byte counts past isize::MAX, so the reservation fails on every target; sizes
        // the OS merely cannot back (2^40 x 2^40) are granted lazily on macOS and the
        // zero fill then commits terabytes.
        assert!(grid(1 << 61, 1, 0.0_f64, "t").is_err());
        assert!(grid(1, 1 << 61, 0.0_f64, "t").is_err());
        assert_eq!(filled(3, 1.5_f64, "t").unwrap(), vec![1.5; 3]);
        assert_eq!(degree(DEGREE_RANGE_MAX, "t").unwrap(), DEGREE_RANGE_MAX);
        assert!(degree(DEGREE_RANGE_MAX + 1, "t").is_err());
    }
}
