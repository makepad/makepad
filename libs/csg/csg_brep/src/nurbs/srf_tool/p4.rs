//! Surface knot removal that keeps a given set of knots, and compatibility of a set
//! of surfaces in both directions.
//!
//! Upstream: `source/SMLib/src/NL_SrfTool.cpp`, `source/SMLib/inc/NL_SrfTool.h`.
//!
//! | Rust                         | Upstream                      |
//! |------------------------------|-------------------------------|
//! | [`remove_knots_constrained`] | `N_SrfRemoveKnotsConstraints` |
//! | [`make_compatible_uv`]       | `N_MakeSrfsCompatibleUV`      |
//!
//! Deviations:
//! - `N_SrfRemoveKnotsConstraints` is `N_SrfRemoveAllKnots` with one change: a
//!   distinct interior knot listed in `UK` / `VK` starts with the "not removable"
//!   bound instead of its one-step removal bound. Both share one implementation
//!   (`nurbs::srf_tool::p3`), so every deviation listed there applies here too (new
//!   surface instead of an out-param, the handling of a failed basis-maximum search,
//!   bounds-checked arrays).
//! - The constraint arrays with their highest indices (`UK, mu`, `VK, mv`, `NULL` for
//!   none) become `Option<&[f64]>`; the highest index is the slice length minus one.
//!   Upstream skips the leading entries equal to the first knot without a bound
//!   check; a list that is empty or holds nothing but that value (upstream reads
//!   past the array) returns [`SolidError::InvalidInput`]. A list is only read for a
//!   direction that is removed.
//! - `N_MakeSrfsCompatibleUV` passes `k` (the highest index); [`make_compatible_uv`]
//!   takes the slice of surfaces (see `nurbs::srf_tool::p3::make_compatible`).

use crate::error::Result;
#[cfg(doc)]
use crate::error::SolidError;
use crate::nurbs::srf_basic::{Direction, Surface};

use super::p3::{make_compatible, remove_knots_kept};

/// Removes every knot of `sur` (in u, v or both) whose removal keeps the surface
/// within `tol`, except the knots listed in `keep_u` / `keep_v`
/// (`N_SrfRemoveKnotsConstraints`). Knots are removed one at a time, always the one
/// with the smallest error bound, accumulating the bounds per span pair; weights must
/// stay within the allowed range, and the tolerance is scaled for rational surfaces
/// by `wmin / (1 + pmax)`.
///
/// Each list is sorted ascending and may hold the end knots and repeated values; it
/// is matched against the distinct interior knots in order, as upstream: the list
/// only advances on a match, so an entry that is not one of the knots keeps every
/// later entry from matching.
///
/// Errors: as `nurbs::srf_tool::p3::remove_all_knots`; [`SolidError::InvalidInput`]
/// for a list that is empty or contains only the first knot.
pub fn remove_knots_constrained(
    sur: &Surface,
    keep_u: Option<&[f64]>,
    keep_v: Option<&[f64]>,
    tol: f64,
    dir: Direction,
) -> Result<Surface> {
    remove_knots_kept(
        sur,
        [keep_u, keep_v],
        tol,
        dir,
        "nurbs::srf_tool::p4::remove_knots_constrained",
    )
}

/// Makes the surfaces compatible in both directions (`N_MakeSrfsCompatibleUV`):
/// `nurbs::srf_tool::p3::make_compatible` with [`Direction::UV`].
///
/// Errors: as `make_compatible`.
pub fn make_compatible_uv(sur: &mut [Surface]) -> Result<()> {
    make_compatible(sur, Direction::UV)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SolidError;
    use crate::nurbs::math::{CPoint, NOW};
    use crate::nurbs::srf_tool::p1::insert_knots;
    use crate::nurbs::srf_tool::p3::remove_all_knots;

    fn sample() -> Surface {
        let net: Vec<Vec<CPoint>> = (0..5)
            .map(|i| {
                (0..4)
                    .map(|j| {
                        let (x, y) = (f64::from(i), f64::from(j));
                        CPoint::new(x, y, (x * 0.7 - y * 0.3).sin(), NOW)
                    })
                    .collect()
            })
            .collect();
        let u = vec![0.0, 0.0, 0.0, 0.0, 0.4, 1.0, 1.0, 1.0, 1.0];
        let v = vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0];
        Surface::new(net, 3, 2, u, v).unwrap()
    }

    fn refined() -> Surface {
        let a = insert_knots(&sample(), &[0.2, 0.7, 0.7], Direction::U).unwrap();
        insert_knots(&a, &[0.25, 0.8], Direction::V).unwrap()
    }

    #[test]
    fn no_constraints_is_remove_all() {
        let a = refined();
        for d in [Direction::U, Direction::V, Direction::UV] {
            assert_eq!(
                remove_knots_constrained(&a, None, None, 1e-9, d).unwrap(),
                remove_all_knots(&a, 1e-9, d).unwrap()
            );
        }
    }

    #[test]
    fn kept_knots_survive() {
        let a = refined();
        let ku = [0.0, 0.0, 0.2, 0.7];
        let kv = [0.0, 0.8, 1.0];
        let r = remove_knots_constrained(&a, Some(&ku), Some(&kv), 1e-9, Direction::UV).unwrap();
        assert!(r.u_knots().contains(&0.2));
        assert!(r.u_knots().contains(&0.7));
        assert!(r.v_knots().contains(&0.8));
        assert!(!r.v_knots().contains(&0.25));
        // A list for a direction that is not removed is never read.
        let bad: [f64; 2] = [0.0, 0.0];
        assert!(remove_knots_constrained(&a, Some(&ku), Some(&bad), 1e-9, Direction::U).is_ok());
        assert!(matches!(
            remove_knots_constrained(&a, Some(&bad), None, 1e-9, Direction::U),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(remove_knots_constrained(&a, Some(&[]), None, 1e-9, Direction::UV).is_err());
    }

    #[test]
    fn compatible_uv() {
        let s = sample();
        let t = insert_knots(&s, &[0.6], Direction::U).unwrap();
        let mut v = vec![s, t];
        make_compatible_uv(&mut v).unwrap();
        assert_eq!(v[0].u_knots(), v[1].u_knots());
        assert!(make_compatible_uv(&mut []).is_err());
    }
}
