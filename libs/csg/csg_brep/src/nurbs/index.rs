//! Bounds-checked access with upstream's signed `NL_INDEX` arithmetic, shared by the
//! NLib ports that keep upstream's index loops (`nurbs::funcs_adv`,
//! `nurbs::crv_nurb`, `nurbs::srf_adv`, ...).
//!
//! Upstream: no counterpart. NLib indexes raw C arrays with signed `NL_INDEX` values
//! (`a[i]`, `a[i][j]`) and allocates them with `N_AllocReal1dArray(hi)` and friends
//! (`hi + 1` entries). The ported loops keep that arithmetic in `isize`; every access
//! goes through these helpers, which reject negative or too-large indices with
//! [`SolidError::InvalidInput`] where upstream would read or write outside its arrays.
//!
//! | Rust                  | Upstream                                  |
//! |-----------------------|-------------------------------------------|
//! | `get`, `set`          | `a[i]`, `a[i] = v`                        |
//! | `get2`, `set2`        | `a[i][j]`, `a[i][j] = v` (fixed-size rows) |
//! | `storage`             | `N_AllocReal1dArray(hi)` and friends      |
//! | `sx`, `ux`            | `NL_INDEX` <-> array length conversions   |
//! | `at`, `put`           | `a[i]`, `a[i] = v` with a `usize` index    |
//! | `at2`, `put2`         | `a[i][j]`, `a[i][j] = v` (`Vec` rows, `usize`) |

use crate::error::{Result, SolidError};

/// The error of an access outside the arrays upstream would use.
pub(crate) const fn oob(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "index out of range (malformed input or arguments)",
    }
}

/// `usize` -> signed index.
pub(crate) fn sx(u: usize, op: &'static str) -> Result<isize> {
    isize::try_from(u).map_err(|_| oob(op))
}

/// Signed index -> `usize`, rejecting negative values.
pub(crate) fn ux(i: isize, op: &'static str) -> Result<usize> {
    usize::try_from(i).map_err(|_| oob(op))
}

/// `a[i]`.
#[inline]
pub(crate) fn get<T: Copy>(a: &[T], i: isize, op: &'static str) -> Result<T> {
    usize::try_from(i)
        .ok()
        .and_then(|i| a.get(i))
        .copied()
        .ok_or_else(|| oob(op))
}

/// `a[i] = v`.
#[inline]
pub(crate) fn set<T>(a: &mut [T], i: isize, v: T, op: &'static str) -> Result<()> {
    let slot = usize::try_from(i)
        .ok()
        .and_then(|i| a.get_mut(i))
        .ok_or_else(|| oob(op))?;
    *slot = v;
    Ok(())
}

/// `a[i][j]` of fixed-size rows.
#[inline]
pub(crate) fn get2<T: Copy, const N: usize>(
    a: &[[T; N]],
    i: isize,
    j: isize,
    op: &'static str,
) -> Result<T> {
    let row = usize::try_from(i)
        .ok()
        .and_then(|i| a.get(i))
        .ok_or_else(|| oob(op))?;
    get(row, j, op)
}

/// `a[i][j] = v` of fixed-size rows.
#[inline]
pub(crate) fn set2<T, const N: usize>(
    a: &mut [[T; N]],
    i: isize,
    j: isize,
    v: T,
    op: &'static str,
) -> Result<()> {
    let row = usize::try_from(i)
        .ok()
        .and_then(|i| a.get_mut(i))
        .ok_or_else(|| oob(op))?;
    set(row, j, v, op)
}

/// `a[i]` with an unsigned index.
#[inline]
pub(crate) fn at<T: Copy>(a: &[T], i: usize, op: &'static str) -> Result<T> {
    a.get(i).copied().ok_or_else(|| oob(op))
}

/// `a[i] = v` with an unsigned index.
#[inline]
pub(crate) fn put<T>(a: &mut [T], i: usize, v: T, op: &'static str) -> Result<()> {
    *a.get_mut(i).ok_or_else(|| oob(op))? = v;
    Ok(())
}

/// `a[i][j]` of `Vec` rows with unsigned indices.
#[inline]
pub(crate) fn at2<T: Copy>(a: &[Vec<T>], i: usize, j: usize, op: &'static str) -> Result<T> {
    at(a.get(i).ok_or_else(|| oob(op))?, j, op)
}

/// `a[i][j] = v` of `Vec` rows with unsigned indices.
#[inline]
pub(crate) fn put2<T>(a: &mut [Vec<T>], i: usize, j: usize, v: T, op: &'static str) -> Result<()> {
    put(a.get_mut(i).ok_or_else(|| oob(op))?, j, v, op)
}

/// Zero-filled storage for `hi + 1` entries (`N_AllocReal1dArray(hi)` and friends).
pub(crate) fn storage<T: Clone + Default>(hi: isize, op: &'static str) -> Result<Vec<T>> {
    let len = ux(hi, op)?.checked_add(1).ok_or_else(|| oob(op))?;
    let mut v = Vec::new();
    v.try_reserve_exact(len).map_err(|_| oob(op))?;
    v.resize(len, T::default());
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_access() {
        let mut a = [1.0, 2.0, 3.0];
        assert_eq!(get(&a, 2, "t").unwrap(), 3.0);
        assert!(get(&a, -1, "t").is_err());
        assert!(get(&a, 3, "t").is_err());
        set(&mut a, 0, 5.0, "t").unwrap();
        assert_eq!(a[0], 5.0);
        assert!(set(&mut a, -1, 0.0, "t").is_err());
        let mut b = [[0_i64; 2]; 2];
        set2(&mut b, 1, 1, 7, "t").unwrap();
        assert_eq!(get2(&b, 1, 1, "t").unwrap(), 7);
        assert!(get2(&b, 2, 0, "t").is_err());
        assert!(set2(&mut b, 0, 2, 1, "t").is_err());
    }

    #[test]
    fn storage_sizes() {
        assert_eq!(storage::<f64>(2, "t").unwrap(), vec![0.0; 3]);
        assert!(storage::<f64>(-1, "t").is_err());
        assert_eq!(sx(4, "t").unwrap(), 4);
        assert!(ux(-1, "t").is_err());
    }
}
