//! NURBS surface object: structure and accessors, validity checks, storage sizing,
//! copying and u/v swapping, rationality conversions, closedness, equality, cheap
//! flatness test and average control-net lengths.
//!
//! Upstream: `source/SMLib/src/NL_SrfBasic.cpp`, `source/SMLib/src/NL_SrfGeom.cpp`
//! (`N_SrfGetAverageLen` only); declarations in `source/SMLib/inc/NL_SrfBasic.h`,
//! `NL_SrfGeom.h`, `NL_SURFACE` in `source/SMLib/inc/NL_Nurbsdef.h`.
//!
//! | Rust                                         | Upstream                          |
//! |----------------------------------------------|-----------------------------------|
//! | [`Surface`]                                  | `NL_SURFACE` (with its `NL_CNET` and `NL_KNOTVECTOR`s) |
//! | [`Surface::new`], [`Surface::with_indices`], [`SurfaceIndices`] | `N_SrfFromCNetAndKnotVectors` |
//! | [`Surface::n`], [`Surface::m`], [`Surface::array_sizes`] | `N_SrfGetArraySizes`  |
//! | [`Surface::p`], [`Surface::q`], [`Surface::degrees`] | `N_SrfGetDegrees`         |
//! | [`Surface::cpt`], [`Surface::row`], [`Surface::rows`], [`Surface::cpts`] | `N_SrfGetCPts` |
//! | [`Surface::u_knots`], [`Surface::v_knots`]   | `N_SrfGetKnots`                   |
//! | [`Surface::knot_vectors`]                    | `N_SrfGetKnotVectors`             |
//! | the accessors above                          | `N_SrfGetNetAndKnotVectors`, `N_SrfGetCPtsDegreesAndKnots`, `N_SrfGetCPtsAndKnots`, `N_SrfGetCPtsKnotVectorAndKnots` |
//! | [`Surface::set_cpts_and_knots`]              | `N_SrfSetCPtsAndKnots`            |
//! | [`Surface::parameter_bounds`]                | `N_SrfGetParameterBounds`         |
//! | [`Surface::set_size_indices`]                | `N_SrfSetSizeIndices`             |
//! | [`Surface::compress`]                        | `N_SrfCompress`                   |
//! | [`Surface::swap_uv`]                         | `N_SwapUV`                        |
//! | `None` / `Option::is_none` on `Option<Surface>` | `N_SrfInitArrays` / `N_SrfAreArraysNULL` |
//! | [`size_arrays`]                              | `N_SrfSizeArrays`                 |
//! | [`Surface::copy_into`], `Clone`              | `N_SrfCopy`                       |
//! | [`Surface::counts_are_valid`]                | `N_SrfCountsAreValid`             |
//! | [`Surface::weights_are_valid`]               | `N_SrfWeightsAreValid`            |
//! | [`Surface::is_valid`]                        | `N_SrfIsValid`                    |
//! | [`Surface::is_sized`]                        | `N_SrfIsSized`                    |
//! | [`Surface::is_rational`]                     | `N_IsSrfRat`                      |
//! | [`Surface::is_closed`], [`Surface::is_closed_tol`] | `N_SrfIsClosed`             |
//! | [`Surface::equals`]                          | `N_SrfsAreEqual`                  |
//! | [`Surface::enet`]                            | `N_SrfGetENet`                    |
//! | [`Surface::non_rat_to_rat`]                  | `N_SrfNonRatToRat`                |
//! | [`make_rat_compatible`]                      | `N_MakeSrfsRatCompatible`         |
//! | [`Surface::reparam_to_interval`]             | `N_SrfReparamToInterval`          |
//! | [`Surface::are_weights_equal`]               | `N_SrfAreWeightsEqual`            |
//! | [`Surface::make_non_rat`]                    | `N_SrfMakeNonRat`                 |
//! | [`Surface::prune_rat`]                       | `N_SrfPruneRat`                   |
//! | [`Surface::is_flat_cheap`], [`Flatness`]     | `N_SrfIsFlatCheap`                |
//! | [`Surface::average_len`], [`AverageLen`]     | `N_SrfGetAverageLen`              |
//! | [`Direction`]                                | `NL_UDIR` / `NL_VDIR` / `NL_UVDIR` |
//! | [`WEIGHT_MIN`], [`WEIGHT_MAX`], [`WEIGHT_TOL`] | `NL_WMIN`, `NL_WMAX`, `NL_WTOL` |
//!
//! Deviations:
//! - [`Surface`] owns its control net and knot arrays (upstream points at caller- or
//!   stack-owned `NL_CNET` / `NL_KNOTVECTOR` objects). As upstream, the highest
//!   indices in use (`n`, `m`, `r`, `s`) are kept apart from the storage, which may be
//!   larger; every index is checked against the storage, so the out-of-bounds accesses
//!   upstream allows (e.g. `N_SrfSetSizeIndices` past the arrays) are `InvalidInput`.
//! - The NULL surface of `N_SrfInitArrays` (null pointers, degrees `-1`) is `None`
//!   in an `Option<Surface>`; partly NULL surfaces cannot be expressed. Storage that
//!   upstream allocates uninitialised (`malloc`) is zero-filled.
//! - The `NL_STACKS` arguments have no counterpart and allocation cannot fail, so
//!   [`Surface::compress`], [`Surface::swap_uv`] and [`Surface::average_len`] have no
//!   memory error path. [`size_arrays`] sizes new storage from caller indices and so
//!   reserves it fallibly: [`NlibCode::Memory`] (upstream `NL_MEM_ERR`) instead of an
//!   allocation abort, also for a net or knot array above
//!   [`crate::limits::MAX_ALLOC_BYTES`].
//! - Out-params become return values; errors are [`SolidError::Nlib`] values with the
//!   upstream `NL_*_ERR` number instead of a flag plus the global `NL_ERROR` record.
//!   The routine-name argument (`rname`) is dropped.
//! - Index arguments are `usize`, so `N_SrfGetENet`'s `ku < 0` / `kv < 0` tests cannot
//!   fail.
//! - `N_SrfIsClosed` reads the global `NL_MTOL`, which `N_SrfIsClosedSmooth`
//!   overrides temporarily: [`Surface::is_closed`] uses [`MODEL_TOL`],
//!   [`Surface::is_closed_tol`] takes it as an argument.
//! - `N_SrfReparamToInterval` writes `U[r-p..]` for `r < p` (outside the array);
//!   [`Surface::reparam_to_interval`] returns `InvalidInput` instead, before changing
//!   anything.
//! - `N_SrfIsFlatCheap` leaves its outputs unwritten when `N_PlanePtNormalToImplicit`
//!   fails; [`Surface::is_flat_cheap`] returns only the error then.

use std::sync::Arc;

use crate::error::{NlibCode, Result, SolidError};
use crate::limits;
use crate::math::Vec3;
use crate::nurbs::geom::{
    dist_pt_pt, dist_signed_pt_plane, plane_pt_normal_to_implicit, Rectangle,
};
use crate::nurbs::knots::{KnotVector, MAX_DEGREE};
use crate::nurbs::math::{combine2_pts, combine4_pts, CPoint, MODEL_TOL, NOW};

/// Minimum valid weight (`NL_WMIN`).
pub const WEIGHT_MIN: f64 = 1.0e-03;
/// Maximum valid weight (`NL_WMAX`).
pub const WEIGHT_MAX: f64 = 100.0;
/// Weight equality tolerance (`NL_WTOL`).
pub const WEIGHT_TOL: f64 = 1.0e-03;

const OP_NEW: &str = "nurbs::srf_basic::Surface::new";
const OP_WITH_INDICES: &str = "nurbs::srf_basic::Surface::with_indices";
const OP_CPT: &str = "nurbs::srf_basic::Surface::cpt";
const OP_SET_CPTS: &str = "nurbs::srf_basic::Surface::set_cpts_and_knots";
const OP_SET_SIZE: &str = "nurbs::srf_basic::Surface::set_size_indices";
const OP_COUNTS: &str = "nurbs::srf_basic::Surface::counts_are_valid";
const OP_WEIGHTS: &str = "nurbs::srf_basic::Surface::weights_are_valid";
const OP_SIZED: &str = "nurbs::srf_basic::Surface::is_sized";
const OP_ENET: &str = "nurbs::srf_basic::Surface::enet";
const OP_REPARAM: &str = "nurbs::srf_basic::Surface::reparam_to_interval";
const OP_FLAT: &str = "nurbs::srf_basic::Surface::is_flat_cheap";

fn nlib(operation: &'static str, code: NlibCode) -> SolidError {
    SolidError::Nlib { operation, code }
}

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

/// A parametric direction flag (`NL_UDIR = 1`, `NL_VDIR = 2`, `NL_UVDIR = 3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// `NL_UDIR`: the u direction (first net index).
    U,
    /// `NL_VDIR`: the v direction (second net index).
    V,
    /// `NL_UVDIR`: both directions.
    UV,
}

impl Direction {
    /// The upstream flag value.
    #[must_use]
    pub const fn flag(self) -> i32 {
        match self {
            Direction::U => 1,
            Direction::V => 2,
            Direction::UV => 3,
        }
    }

    /// The direction for an upstream flag value, `None` for any other value.
    #[must_use]
    pub const fn from_flag(flag: i32) -> Option<Self> {
        match flag {
            1 => Some(Direction::U),
            2 => Some(Direction::V),
            3 => Some(Direction::UV),
            _ => None,
        }
    }
}

/// Result of [`Surface::is_flat_cheap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flatness {
    /// The control net lies within the tolerance of the approximating plane.
    pub flat: bool,
    /// Suggested subdivision direction ([`Direction::U`]: split at a v value,
    /// [`Direction::V`]: split at a u value).
    pub dir: Direction,
}

/// Result of [`Surface::average_len`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AverageLen {
    /// Area bound: longest u polygon times longest v polygon.
    pub area: f64,
    /// Average length of the u-direction control polygons.
    pub lu: f64,
    /// Average length of the v-direction control polygons.
    pub lv: f64,
}

/// The highest indices and the degrees of a [`Surface`] (upstream `NL_SURFACE`'s
/// `n`, `m`, `p`, `q` and its knot vectors' `r`, `s`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SurfaceIndices {
    /// Highest u index of the control net.
    pub n: usize,
    /// Highest v index of the control net.
    pub m: usize,
    /// Degree in u.
    pub p: usize,
    /// Degree in v.
    pub q: usize,
    /// Highest index of the u knot vector.
    pub r: usize,
    /// Highest index of the v knot vector.
    pub s: usize,
}

/// A NURBS surface (upstream `NL_SURFACE`): control net `Pw[0..=n][0..=m]`, degrees
/// `p`, `q`, knot vectors `U[0..=r]`, `V[0..=s]`.
///
/// The indices in use are kept apart from the storage (which may be larger), as
/// upstream; the constructors guarantee that the storage holds them.
///
/// The net and knot storage are shared copy-on-write (`Arc`, written through
/// [`Arc::make_mut`]): a clone shares them until one side writes, so the many
/// evaluation copies of a surface do not copy its control net.
#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    /// Net storage; rows `0..=n` hold at least `m + 1` points.
    net: Arc<Vec<Vec<CPoint>>>,
    n: usize,
    m: usize,
    p: usize,
    q: usize,
    /// U knot storage, at least `r + 1` entries.
    u: Arc<Vec<f64>>,
    r: usize,
    /// V knot storage, at least `s + 1` entries.
    v: Arc<Vec<f64>>,
    s: usize,
}

/// `true` when `net` holds rows `0..=n` of at least `m + 1` points.
fn net_holds(net: &[Vec<CPoint>], n: usize, m: usize) -> bool {
    match n.checked_add(1).and_then(|k| net.get(..k)) {
        Some(rows) => rows.iter().all(|row| row.len() > m),
        None => false,
    }
}

/// A zero-filled `(n+1) x (m+1)` net.
fn zero_net(n: usize, m: usize) -> Vec<Vec<CPoint>> {
    vec![vec![CPoint::default(); m.saturating_add(1)]; n.saturating_add(1)]
}

impl Surface {
    /// Surface over a whole rectangular net and whole knot arrays
    /// (`n = net.len() - 1`, `m = net[0].len() - 1`, `r = u.len() - 1`,
    /// `s = v.len() - 1`).
    ///
    /// Errors with `InvalidInput` for an empty net, rows of different lengths, or an
    /// empty knot array. Nothing else is checked (see [`Surface::is_valid`]).
    pub fn new(
        net: Vec<Vec<CPoint>>,
        p: usize,
        q: usize,
        u: Vec<f64>,
        v: Vec<f64>,
    ) -> Result<Self> {
        let n = net
            .len()
            .checked_sub(1)
            .ok_or_else(|| invalid(OP_NEW, "empty control net"))?;
        let cols = net.first().map_or(0, Vec::len);
        let m = cols
            .checked_sub(1)
            .ok_or_else(|| invalid(OP_NEW, "empty control net row"))?;
        if net.iter().any(|row| row.len() != cols) {
            return Err(invalid(OP_NEW, "control net rows differ in length"));
        }
        let r = u
            .len()
            .checked_sub(1)
            .ok_or_else(|| invalid(OP_NEW, "empty u knot vector"))?;
        let s = v
            .len()
            .checked_sub(1)
            .ok_or_else(|| invalid(OP_NEW, "empty v knot vector"))?;
        Ok(Self {
            net: Arc::new(net),
            n,
            m,
            p,
            q,
            u: Arc::new(u),
            r,
            v: Arc::new(v),
            s,
        })
    }

    /// Surface over the first `n + 1` rows / `m + 1` columns of `net` and the first
    /// `r + 1` / `s + 1` knots of `u` / `v` (upstream `N_SrfFromCNetAndKnotVectors`
    /// with a net and knot vectors whose indices may be below their storage).
    ///
    /// Errors with `InvalidInput` when the storage does not hold the indices.
    pub fn with_indices(
        net: Vec<Vec<CPoint>>,
        u: Vec<f64>,
        v: Vec<f64>,
        idx: SurfaceIndices,
    ) -> Result<Self> {
        let SurfaceIndices { n, m, p, q, r, s } = idx;
        if !net_holds(&net, n, m) {
            return Err(invalid(OP_WITH_INDICES, "control net smaller than (n, m)"));
        }
        if u.len() <= r || v.len() <= s {
            return Err(invalid(OP_WITH_INDICES, "knot array smaller than (r, s)"));
        }
        Ok(Self {
            net: Arc::new(net),
            n,
            m,
            p,
            q,
            u: Arc::new(u),
            r,
            v: Arc::new(v),
            s,
        })
    }

    /// All indices and degrees.
    #[must_use]
    pub fn indices(&self) -> SurfaceIndices {
        SurfaceIndices {
            n: self.n,
            m: self.m,
            p: self.p,
            q: self.q,
            r: self.r,
            s: self.s,
        }
    }

    /// Highest u index of the control net.
    #[must_use]
    pub fn n(&self) -> usize {
        self.n
    }

    /// Highest v index of the control net.
    #[must_use]
    pub fn m(&self) -> usize {
        self.m
    }

    /// Degree in u.
    #[must_use]
    pub fn p(&self) -> usize {
        self.p
    }

    /// Degree in v.
    #[must_use]
    pub fn q(&self) -> usize {
        self.q
    }

    /// Highest index of the u knot vector.
    #[must_use]
    pub fn r(&self) -> usize {
        self.r
    }

    /// Highest index of the v knot vector.
    #[must_use]
    pub fn s(&self) -> usize {
        self.s
    }

    /// `(p, q)` (`N_SrfGetDegrees`).
    #[must_use]
    pub fn degrees(&self) -> (usize, usize) {
        (self.p, self.q)
    }

    /// `(n, m, r, s)` (`N_SrfGetArraySizes`).
    #[must_use]
    pub fn array_sizes(&self) -> (usize, usize, usize, usize) {
        (self.n, self.m, self.r, self.s)
    }

    /// Control point `Pw[i][j]`; `InvalidInput` for `i > n` or `j > m`.
    pub fn cpt(&self, i: usize, j: usize) -> Result<CPoint> {
        if i > self.n || j > self.m {
            return Err(invalid(OP_CPT, "control point index out of range"));
        }
        self.net
            .get(i)
            .and_then(|row| row.get(j))
            .copied()
            .ok_or_else(|| invalid(OP_CPT, "control point index out of range"))
    }

    /// Mutable control point `Pw[i][j]`; `InvalidInput` for `i > n` or `j > m`.
    pub fn cpt_mut(&mut self, i: usize, j: usize) -> Result<&mut CPoint> {
        if i > self.n || j > self.m {
            return Err(invalid(OP_CPT, "control point index out of range"));
        }
        Arc::make_mut(&mut self.net)
            .get_mut(i)
            .and_then(|row| row.get_mut(j))
            .ok_or_else(|| invalid(OP_CPT, "control point index out of range"))
    }

    /// Row `Pw[i][0..=m]`; `InvalidInput` for `i > n`.
    pub fn row(&self, i: usize) -> Result<&[CPoint]> {
        if i > self.n {
            return Err(invalid(OP_CPT, "control net row out of range"));
        }
        self.net
            .get(i)
            .and_then(|row| row.get(..=self.m))
            .ok_or_else(|| invalid(OP_CPT, "control net row out of range"))
    }

    /// The rows `Pw[0..=n][0..=m]` in use.
    pub fn rows(&self) -> impl Iterator<Item = &[CPoint]> + '_ {
        let m = self.m;
        self.net
            .iter()
            .take(self.n.saturating_add(1))
            .map(move |row| row.get(..=m).unwrap_or(&[]))
    }

    /// Mutable rows `Pw[0..=n][0..=m]` in use.
    fn rows_mut(&mut self) -> impl Iterator<Item = &mut [CPoint]> + '_ {
        let m = self.m;
        Arc::make_mut(&mut self.net)
            .iter_mut()
            .take(self.n.saturating_add(1))
            .map(move |row| row.get_mut(..=m).unwrap_or(&mut []))
    }

    /// Copy of the control net in use (`(n+1) x (m+1)`).
    #[must_use]
    pub fn cpts(&self) -> Vec<Vec<CPoint>> {
        self.rows().map(<[CPoint]>::to_vec).collect()
    }

    /// The u knots `U[0..=r]` (`N_SrfGetKnots`).
    #[must_use]
    pub fn u_knots(&self) -> &[f64] {
        self.u.get(..=self.r).unwrap_or(&[])
    }

    /// The v knots `V[0..=s]` (`N_SrfGetKnots`).
    #[must_use]
    pub fn v_knots(&self) -> &[f64] {
        self.v.get(..=self.s).unwrap_or(&[])
    }

    /// Mutable u knots `U[0..=r]`.
    pub fn u_knots_mut(&mut self) -> &mut [f64] {
        Arc::make_mut(&mut self.u)
            .get_mut(..=self.r)
            .unwrap_or(&mut [])
    }

    /// Mutable v knots `V[0..=s]`.
    pub fn v_knots_mut(&mut self) -> &mut [f64] {
        Arc::make_mut(&mut self.v)
            .get_mut(..=self.s)
            .unwrap_or(&mut [])
    }

    /// The knot vectors over the knot storage (`N_SrfGetKnotVectors`).
    pub fn knot_vectors(&self) -> Result<(KnotVector<'_>, KnotVector<'_>)> {
        Ok((
            KnotVector::new(&self.u, self.r)?,
            KnotVector::new(&self.v, self.s)?,
        ))
    }

    /// Replaces the net and knot storage, keeping the indices
    /// (`N_SrfSetCPtsAndKnots`).
    ///
    /// Errors with `InvalidInput` (and changes nothing) when the new storage does not
    /// hold the current indices.
    pub fn set_cpts_and_knots(
        &mut self,
        net: Vec<Vec<CPoint>>,
        u: Vec<f64>,
        v: Vec<f64>,
    ) -> Result<()> {
        if !net_holds(&net, self.n, self.m) || u.len() <= self.r || v.len() <= self.s {
            return Err(invalid(OP_SET_CPTS, "storage smaller than the indices"));
        }
        self.net = Arc::new(net);
        self.u = Arc::new(u);
        self.v = Arc::new(v);
        Ok(())
    }

    /// `(U[0], U[r], V[0], V[s])` as a rectangle (`N_SrfGetParameterBounds`).
    #[must_use]
    pub fn parameter_bounds(&self) -> Rectangle {
        let u = self.u_knots();
        let v = self.v_knots();
        Rectangle::new(
            u.first().copied().unwrap_or(f64::NAN),
            u.last().copied().unwrap_or(f64::NAN),
            v.first().copied().unwrap_or(f64::NAN),
            v.last().copied().unwrap_or(f64::NAN),
        )
    }

    /// Sets the indices and degrees (`N_SrfSetSizeIndices`).
    ///
    /// Errors with `InvalidInput` (and changes nothing) when the storage does not hold
    /// the new indices (upstream would index past its arrays later).
    pub fn set_size_indices(&mut self, idx: SurfaceIndices) -> Result<()> {
        let SurfaceIndices { n, m, p, q, r, s } = idx;
        if !net_holds(&self.net, n, m) || self.u.len() <= r || self.v.len() <= s {
            return Err(invalid(OP_SET_SIZE, "storage smaller than the indices"));
        }
        self.n = n;
        self.m = m;
        self.p = p;
        self.q = q;
        self.r = r;
        self.s = s;
        Ok(())
    }

    /// Shrinks the storage to exactly the indices in use (`N_SrfCompress`).
    pub fn compress(&mut self) {
        if self.is_compact() {
            // (already exactly the indices in use: the same storage values, kept
            // shared)
            return;
        }
        let net = self.cpts();
        let u = self.u_knots().to_vec();
        let v = self.v_knots().to_vec();
        self.net = Arc::new(net);
        self.u = Arc::new(u);
        self.v = Arc::new(v);
    }

    /// Whether the storage holds exactly the indices in use (what
    /// [`Surface::compress`] leaves).
    fn is_compact(&self) -> bool {
        self.net.len() == self.n.saturating_add(1)
            && self
                .net
                .iter()
                .all(|row| row.len() == self.m.saturating_add(1))
            && self.u.len() == self.r.saturating_add(1)
            && self.v.len() == self.s.saturating_add(1)
    }

    /// A copy with the storage shrunk to the indices in use: the same surface as a
    /// clone followed by [`Surface::compress`], without copying the unused storage
    /// first.
    #[must_use]
    pub fn compressed_copy(&self) -> Self {
        if self.is_compact() {
            // (the same values: share the storage)
            return self.clone();
        }
        Self {
            net: Arc::new(self.cpts()),
            n: self.n,
            m: self.m,
            p: self.p,
            q: self.q,
            u: Arc::new(self.u_knots().to_vec()),
            r: self.r,
            v: Arc::new(self.v_knots().to_vec()),
            s: self.s,
        }
    }

    /// Swaps the roles of u and v, `S_new(v, u) = S_old(u, v)` (`N_SwapUV`): the net
    /// is transposed into new `(m+1) x (n+1)` storage, degrees and knot vectors are
    /// exchanged.
    pub fn swap_uv(&mut self) {
        let mut qw = zero_net(self.m, self.n);
        for (i, row) in self.rows().enumerate() {
            for (j, pw) in row.iter().enumerate() {
                if let Some(slot) = qw.get_mut(j).and_then(|r| r.get_mut(i)) {
                    *slot = *pw;
                }
            }
        }
        self.net = Arc::new(qw);
        core::mem::swap(&mut self.n, &mut self.m);
        core::mem::swap(&mut self.p, &mut self.q);
        core::mem::swap(&mut self.u, &mut self.v);
        core::mem::swap(&mut self.r, &mut self.s);
    }

    /// Copies this surface into `dst` (`N_SrfCopy`): a NULL (`None`) destination is
    /// allocated, an existing one is resized with [`size_arrays`] (keeping storage
    /// that is large enough), then the net and knots in use are copied.
    pub fn copy_into(&self, dst: &mut Option<Surface>) {
        // `size_arrays` with this surface's own (already allocated) sizes; new
        // storage is allocated infallibly here, as for any copy.
        let idx = self.indices();
        let SurfaceIndices { n, m, p, q, r, s } = idx;
        let reuse = match dst {
            Some(sur) => sur.is_sized(n, m, r, s).is_ok() && sur.set_size_indices(idx).is_ok(),
            None => false,
        };
        let sur = if let (true, Some(sur)) = (reuse, dst.as_mut()) {
            sur
        } else {
            dst.insert(Surface {
                net: Arc::new(zero_net(n, m)),
                n,
                m,
                p,
                q,
                u: Arc::new(vec![0.0; r.saturating_add(1)]),
                r,
                v: Arc::new(vec![0.0; s.saturating_add(1)]),
                s,
            })
        };
        for (qrow, prow) in Arc::make_mut(&mut sur.net).iter_mut().zip(self.rows()) {
            for (q, p) in qrow.iter_mut().zip(prow) {
                *q = *p;
            }
        }
        for (q, p) in Arc::make_mut(&mut sur.u).iter_mut().zip(self.u_knots()) {
            *q = *p;
        }
        for (q, p) in Arc::make_mut(&mut sur.v).iter_mut().zip(self.v_knots()) {
            *q = *p;
        }
    }

    // ---- error checks ---------------------------------------------------------------

    /// Checks `n + p + 1 == r` and `m + q + 1 == s`, the degree limit and that all
    /// control points share the rationality of `Pw[0][0]` (`N_SrfCountsAreValid`).
    ///
    /// Errors with [`NlibCode::Surface`] (`NL_SUR_ERR`) or [`NlibCode::Degree`]
    /// (`NL_DEG_ERR`, degree above [`MAX_DEGREE`]).
    pub fn counts_are_valid(&self) -> Result<()> {
        let (n, m, p, q, r, s) = (self.n, self.m, self.p, self.q, self.r, self.s);

        // Check definition and degrees.
        if n.checked_add(p).and_then(|k| k.checked_add(1)) != Some(r)
            || m.checked_add(q).and_then(|k| k.checked_add(1)) != Some(s)
        {
            return Err(nlib(OP_COUNTS, NlibCode::Surface));
        }
        if p > MAX_DEGREE || q > MAX_DEGREE {
            return Err(nlib(OP_COUNTS, NlibCode::Degree));
        }

        // Check for consistency.
        let oldrat = self.cpt(0, 0)?.w != NOW;
        for row in self.rows() {
            for pw in row {
                let newrat = pw.w != NOW;
                if newrat != oldrat {
                    return Err(nlib(OP_COUNTS, NlibCode::Surface));
                }
            }
        }
        Ok(())
    }

    /// For a rational surface, checks that every weight is in
    /// `[WEIGHT_MIN, WEIGHT_MAX]` (`N_SrfWeightsAreValid`; NaN weights pass).
    ///
    /// Errors with [`NlibCode::Weight`] (`NL_WEI_ERR`).
    pub fn weights_are_valid(&self) -> Result<()> {
        if self.is_rational() {
            for row in self.rows() {
                for pw in row {
                    let w = pw.w;
                    if w < WEIGHT_MIN || w > WEIGHT_MAX {
                        return Err(nlib(OP_WEIGHTS, NlibCode::Weight));
                    }
                }
            }
        }
        Ok(())
    }

    /// Complete check: counts, weights, then both knot vectors (`N_SrfIsValid`).
    ///
    /// Errors with the first failing check's code ([`NlibCode::Surface`],
    /// [`NlibCode::Degree`], [`NlibCode::Weight`] or [`NlibCode::Knot`]).
    pub fn is_valid(&self) -> Result<()> {
        self.counts_are_valid()?;
        self.weights_are_valid()?;
        let (knu, knv) = self.knot_vectors()?;
        knu.validate(self.p)?;
        knv.validate(self.q)?;
        Ok(())
    }

    /// Checks that the indices in use are at least `(np, mp, rk, sk)`
    /// (`N_SrfIsSized`).
    ///
    /// Errors with [`NlibCode::Storage`] (`NL_STO_ERR`).
    pub fn is_sized(&self, np: usize, mp: usize, rk: usize, sk: usize) -> Result<()> {
        if self.n < np || self.m < mp || self.r < rk || self.s < sk {
            return Err(nlib(OP_SIZED, NlibCode::Storage));
        }
        Ok(())
    }

    // ---- predicates -----------------------------------------------------------------

    /// `true` when `Pw[0][0]` carries a weight (`N_IsSrfRat`).
    #[must_use]
    pub fn is_rational(&self) -> bool {
        self.cpt(0, 0).is_ok_and(|pw| pw.w != NOW)
    }

    /// Closedness in `dir` with `NL_MTOL` = [`MODEL_TOL`] (`N_SrfIsClosed`).
    #[must_use]
    pub fn is_closed(&self, dir: Direction) -> bool {
        self.is_closed_tol(dir, MODEL_TOL)
    }

    /// `true` when the first and last Euclidean control-point rows (u) or columns (v)
    /// coincide within `mtol` (`N_SrfIsClosed` with `NL_MTOL = mtol`).
    /// [`Direction::UV`] gives `false`, as any other flag upstream.
    #[must_use]
    pub fn is_closed_tol(&self, dir: Direction, mtol: f64) -> bool {
        match dir {
            Direction::U => {
                let (Ok(first), Ok(last)) = (self.row(0), self.row(self.n)) else {
                    return false;
                };
                for (ps, pe) in first.iter().zip(last) {
                    let d = dist_pt_pt(ps.to_euclid(), pe.to_euclid());
                    if d > mtol {
                        return false;
                    }
                }
                true
            }
            Direction::V => {
                for row in self.rows() {
                    let (Some(ps), Some(pe)) = (row.first(), row.last()) else {
                        return false;
                    };
                    let d = dist_pt_pt(ps.to_euclid(), pe.to_euclid());
                    if d > mtol {
                        return false;
                    }
                }
                true
            }
            Direction::UV => false,
        }
    }

    /// `true` when both surfaces have the same degrees and index counts, every pair of
    /// control points is within `tol3d` in homogeneous space and every pair of knots
    /// within `tol1d` (`N_SrfsAreEqual`).
    #[must_use]
    pub fn equals(&self, other: &Surface, tol3d: f64, tol1d: f64) -> bool {
        let mut b_rtn = self.p == other.p
            && self.q == other.q
            && self.n == other.n
            && self.m == other.m
            && self.r == other.r
            && self.s == other.s;
        if b_rtn {
            'net: for (prow, qrow) in self.rows().zip(other.rows()) {
                for (pw, qw) in prow.iter().zip(qrow) {
                    let d = CPoint::diff(*pw, *qw).magnitude();
                    b_rtn &= d <= tol3d;
                    if !b_rtn {
                        break 'net;
                    }
                }
            }
        }
        if b_rtn {
            for (a, b) in self.u_knots().iter().zip(other.u_knots()) {
                b_rtn &= (a - b).abs() <= tol1d;
                if !b_rtn {
                    break;
                }
            }
        }
        if b_rtn {
            for (a, b) in self.v_knots().iter().zip(other.v_knots()) {
                b_rtn &= (a - b).abs() <= tol1d;
                if !b_rtn {
                    break;
                }
            }
        }
        b_rtn
    }

    /// `true` when `max w - min w < WEIGHT_TOL` (`N_SrfAreWeightsEqual`).
    #[must_use]
    pub fn are_weights_equal(&self) -> bool {
        let w0 = self.cpt(0, 0).map_or(f64::NAN, |pw| pw.w);
        let (mut wmin, mut wmax) = (w0, w0);
        for row in self.rows() {
            for pw in row {
                let w = pw.w;
                if w < wmin {
                    wmin = w;
                }
                if w > wmax {
                    wmax = w;
                }
            }
        }
        (wmax - wmin).abs() < WEIGHT_TOL
    }

    // ---- data extraction ------------------------------------------------------------

    /// Euclidean images of `Pw[ku..=lu][kv..=lv]`, as `P[i-ku][j-kv]`
    /// (`N_SrfGetENet`).
    ///
    /// Errors with [`NlibCode::Index`] (`NL_IND_ERR`) when `ku > lu`, `lu > n`,
    /// `kv > lv` or `lv > m`.
    pub fn enet(&self, ku: usize, lu: usize, kv: usize, lv: usize) -> Result<Vec<Vec<Vec3>>> {
        if ku > lu || lu > self.n {
            return Err(nlib(OP_ENET, NlibCode::Index));
        }
        if kv > lv || lv > self.m {
            return Err(nlib(OP_ENET, NlibCode::Index));
        }
        (ku..=lu)
            .map(|i| {
                let row = self.row(i)?;
                let part = row
                    .get(kv..=lv)
                    .ok_or_else(|| nlib(OP_ENET, NlibCode::Index))?;
                Ok(part.iter().map(|pw| pw.to_euclid()).collect())
            })
            .collect()
    }

    // ---- rationality ----------------------------------------------------------------

    /// Sets every weight to `1.0` (`N_SrfNonRatToRat`); the other coordinates are
    /// kept as they are.
    pub fn non_rat_to_rat(&mut self) {
        for row in self.rows_mut() {
            for pw in row {
                pw.w = 1.0;
            }
        }
    }

    /// Maps every control point to its Euclidean image `(x, y, z, NOW)`
    /// (`N_SrfMakeNonRat`).
    pub fn make_non_rat(&mut self) {
        for row in self.rows_mut() {
            for pw in row {
                *pw = pw.unweighted();
            }
        }
    }

    /// Converts a rational surface with equal weights to non-rational form
    /// (`N_SrfPruneRat`).
    pub fn prune_rat(&mut self) {
        if self.is_rational() && self.are_weights_equal() {
            self.make_non_rat();
        }
    }

    // ---- modification ---------------------------------------------------------------

    /// Rescales the u and/or v knots in place to `[R.ul, R.ur]` / `[R.vb, R.vt]`
    /// (`N_SrfReparamToInterval`): the first `p + 1` knots become the new start, the
    /// last `p + 1` the new end, and the interior knots are mapped affinely. A
    /// direction whose bounds already match is left alone.
    ///
    /// Errors with `InvalidInput` (changing nothing) when `r < p` (or `s < q`) in a
    /// direction that would be rescaled, where upstream writes outside the array.
    pub fn reparam_to_interval(&mut self, rect: Rectangle, dir: Direction) -> Result<()> {
        let (a, b, c, d) = (rect.ul, rect.ur, rect.vb, rect.vt);
        let do_u = matches!(dir, Direction::U | Direction::UV);
        let do_v = matches!(dir, Direction::V | Direction::UV);
        let needs_u = do_u && (Some(&a) != self.u.first() || Some(&b) != self.u.get(self.r));
        let needs_v = do_v && (Some(&c) != self.v.first() || Some(&d) != self.v.get(self.s));
        if (needs_u && self.r < self.p) || (needs_v && self.s < self.q) {
            return Err(invalid(OP_REPARAM, "knot vector shorter than the degree"));
        }
        if needs_u {
            rescale(
                Arc::make_mut(&mut self.u).as_mut_slice(),
                self.r,
                self.p,
                a,
                b,
            )?;
        }
        if needs_v {
            rescale(
                Arc::make_mut(&mut self.v).as_mut_slice(),
                self.s,
                self.q,
                c,
                d,
            )?;
        }
        Ok(())
    }

    // ---- geometry -------------------------------------------------------------------

    /// Cheap flatness test against the plane through the centre of the four corner
    /// points (`N_SrfIsFlatCheap`), with a suggested subdivision direction. A surface
    /// closed in u is not flat and should be split in v ([`Direction::V`]); closed in
    /// v gives [`Direction::U`].
    ///
    /// Errors with [`NlibCode::Geometry`] when the plane normal cannot be normalised.
    pub fn is_flat_cheap(&self, eps: f64) -> Result<Flatness> {
        // Check if surface is closed.
        if self.is_closed(Direction::U) {
            return Ok(Flatness {
                flat: false,
                dir: Direction::V,
            });
        }
        if self.is_closed(Direction::V) {
            return Ok(Flatness {
                flat: false,
                dir: Direction::U,
            });
        }

        // Get approximating plane.
        let (n, m) = (self.n, self.m);
        let p00 = self.cpt(0, 0)?.to_euclid();
        let p10 = self.cpt(n, 0)?.to_euclid();
        let p01 = self.cpt(0, m)?.to_euclid();
        let p11 = self.cpt(n, m)?.to_euclid();
        let p = combine4_pts(0.25, p00, 0.25, p10, 0.25, p01, 0.25, p11);
        let a = combine2_pts(0.5, p00, 0.5, p10);
        let b = combine2_pts(0.5, p10, 0.5, p11);
        let c = a - p;
        let d = b - p;
        let lv = c.length();
        let lu = d.length();
        let nrm = c.cross(d);
        let ln = nrm.length();
        let split = if lu > lv { Direction::V } else { Direction::U };
        if ln < MODEL_TOL {
            return Ok(Flatness {
                flat: false,
                dir: split,
            });
        }
        let [pa, pb, pc, pd] =
            plane_pt_normal_to_implicit(p, nrm).map_err(|_| nlib(OP_FLAT, NlibCode::Geometry))?;

        // Check flatness.
        let mut flat = true;
        'net: for row in self.rows() {
            for pw in row {
                let dis = dist_signed_pt_plane(pa, pb, pc, pd, pw.to_euclid());
                if dis.abs() > eps {
                    flat = false;
                    break 'net;
                }
            }
        }
        Ok(Flatness { flat, dir: split })
    }

    /// Average control-polygon lengths in u and v and an area bound
    /// (`N_SrfGetAverageLen`).
    #[must_use]
    pub fn average_len(&self) -> AverageLen {
        let (n, m) = (self.n, self.m);

        // Get maximum polygon lengths in u- and v-directions.
        let mut maxu = 0.0;
        let mut sumu = 0.0;
        for j in 0..=m {
            let mut prev = self.euclid(0, j);
            let mut len = 0.0;
            for i in 1..=n {
                let cur = self.euclid(i, j);
                len += dist_pt_pt(cur, prev);
                prev = cur;
            }
            sumu += len;
            if len > maxu {
                maxu = len;
            }
        }
        let mut maxv = 0.0;
        let mut sumv = 0.0;
        for row in self.rows() {
            let mut it = row.iter().map(|pw| pw.to_euclid());
            let mut prev = it.next().unwrap_or(Vec3::ZERO);
            let mut len = 0.0;
            for cur in it {
                len += dist_pt_pt(cur, prev);
                prev = cur;
            }
            sumv += len;
            if len > maxv {
                maxv = len;
            }
        }

        // Estimate area and length.
        AverageLen {
            area: maxu * maxv,
            lu: sumu / (m as f64 + 1.0),
            lv: sumv / (n as f64 + 1.0),
        }
    }

    /// Euclidean image of `Pw[i][j]` (in range by construction; NaN otherwise).
    fn euclid(&self, i: usize, j: usize) -> Vec3 {
        self.cpt(i, j)
            .map_or(Vec3::new(f64::NAN, f64::NAN, f64::NAN), CPoint::to_euclid)
    }
}

/// One direction of `N_SrfReparamToInterval`: `U[0..=p] = lo`,
/// `U[p+1..=r-p-1]` mapped affinely, `U[r-p..=r] = hi`. Requires `p <= r`.
fn rescale(u: &mut [f64], r: usize, p: usize, lo: f64, hi: f64) -> Result<()> {
    let oob = || invalid(OP_REPARAM, "knot index out of range");
    let u0 = *u.first().ok_or_else(oob)?;
    let ur = *u.get(r).ok_or_else(oob)?;
    let fac = (hi - lo) / (ur - u0);
    let tail = r.checked_sub(p).ok_or_else(oob)?;
    for x in u.get_mut(..=p.min(r)).ok_or_else(oob)? {
        *x = lo;
    }
    // The first loop may have overwritten U[0] (it always does), so the interior
    // uses the saved `u0`, as upstream.
    if let Some(end) = tail.checked_sub(1) {
        if p < end {
            for x in u.get_mut(p + 1..=end).ok_or_else(oob)? {
                *x = fac * (*x - u0) + lo;
            }
        }
    }
    for x in u.get_mut(tail..=r).ok_or_else(oob)? {
        *x = hi;
    }
    Ok(())
}

/// Makes the surfaces rationality-compatible (`N_MakeSrfsRatCompatible`): when any
/// is rational, every non-rational one gets weights `1.0`.
pub fn make_rat_compatible(surfaces: &mut [Surface]) {
    let allnra = !surfaces.iter().any(Surface::is_rational);
    if !allnra {
        for sur in surfaces.iter_mut() {
            if !sur.is_rational() {
                sur.non_rat_to_rat();
            }
        }
    }
}

/// `len_minus_1 + 1` copies of `v`, or [`NlibCode::Memory`] (upstream `NL_MEM_ERR`
/// from a failed allocation) when the storage cannot be reserved, would take more
/// than [`limits::MAX_ALLOC_BYTES`] or the length overflows.
fn try_filled<T: Clone>(len_minus_1: usize, v: T) -> Result<Vec<T>> {
    let mem = || SolidError::nlib(OP_SIZE_ARRAYS, NlibCode::Memory);
    let len = len_minus_1.checked_add(1).ok_or_else(mem)?;
    limits::try_filled(len, v).ok_or_else(mem)
}

/// Operation name of [`size_arrays`].
const OP_SIZE_ARRAYS: &str = "nurbs::srf_basic::size_arrays";

/// Makes `slot` able to hold a surface with indices `(n, m, r, s)` and degrees
/// `(p, q)`, and sets them (`N_SrfSizeArrays`).
///
/// A NULL (`None`) slot gets new zero-filled storage. An existing surface whose
/// indices in use are at least the requested ones ([`Surface::is_sized`]) keeps its
/// storage; otherwise new zero-filled storage replaces it. In every case the indices
/// and degrees are then set to the requested values. Returns the sized surface.
///
/// # Errors
///
/// [`NlibCode::Memory`] (upstream's failed `N_Malloc`, `NL_MEM_ERR`) when the new
/// storage cannot be allocated or an array of it would take more than
/// [`limits::MAX_ALLOC_BYTES`]; the slot is then unchanged.
pub fn size_arrays(slot: &mut Option<Surface>, idx: SurfaceIndices) -> Result<&mut Surface> {
    let SurfaceIndices { n, m, p, q, r, s } = idx;
    let fresh = || -> Result<Surface> {
        // The whole net counts against the bound, not just one row of it.
        let points = n
            .checked_add(1)
            .zip(m.checked_add(1))
            .and_then(|(rows, cols)| rows.checked_mul(cols));
        if !points.is_some_and(limits::fits::<CPoint>) {
            return Err(SolidError::nlib(OP_SIZE_ARRAYS, NlibCode::Memory));
        }
        let row = try_filled(m, CPoint::default())?;
        let mut net = try_filled(n, Vec::new())?;
        for r in &mut net {
            let mut copy = Vec::new();
            copy.try_reserve_exact(row.len())
                .map_err(|_| SolidError::nlib(OP_SIZE_ARRAYS, NlibCode::Memory))?;
            copy.extend_from_slice(&row);
            *r = copy;
        }
        Ok(Surface {
            net: Arc::new(net),
            n,
            m,
            p,
            q,
            u: Arc::new(try_filled(r, 0.0)?),
            r,
            v: Arc::new(try_filled(s, 0.0)?),
            s,
        })
    };
    let sur = match slot {
        Some(sur) => {
            // The storage always holds the indices in use, so `set_size_indices`
            // cannot fail once `is_sized` passed.
            if sur.is_sized(n, m, r, s).is_err() || sur.set_size_indices(idx).is_err() {
                *sur = fresh()?;
            }
            sur
        }
        None => slot.insert(fresh()?),
    };
    Ok(sur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_storage_until_written() {
        let a = bilinear();
        let mut b = a.clone();
        assert!(Arc::ptr_eq(&a.net, &b.net) && Arc::ptr_eq(&a.u, &b.u));
        // compress of compact storage keeps it shared
        b.compress();
        assert!(Arc::ptr_eq(&a.net, &b.net));
        let c = a.compressed_copy();
        assert!(Arc::ptr_eq(&a.net, &c.net) && Arc::ptr_eq(&a.v, &c.v));
        // a write copies only what it writes
        b.u_knots_mut()[0] = -1.0;
        assert!(!Arc::ptr_eq(&a.u, &b.u) && Arc::ptr_eq(&a.net, &b.net));
        *b.cpt_mut(1, 1).unwrap() = CPoint::ZERO;
        assert!(!Arc::ptr_eq(&a.net, &b.net));
        assert_eq!(a, bilinear());
        assert_ne!(a, b);
    }

    fn bilinear() -> Surface {
        let net = vec![
            vec![
                CPoint::new(0.0, 0.0, 0.0, NOW),
                CPoint::new(0.0, 1.0, 0.0, NOW),
            ],
            vec![
                CPoint::new(1.0, 0.0, 0.0, NOW),
                CPoint::new(1.0, 1.0, 0.0, NOW),
            ],
        ];
        Surface::new(
            net,
            1,
            1,
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap()
    }

    #[test]
    fn construct_and_access() {
        let s = bilinear();
        assert_eq!(s.array_sizes(), (1, 1, 3, 3));
        assert_eq!(s.degrees(), (1, 1));
        assert!(s.is_valid().is_ok());
        assert!(!s.is_rational());
        assert_eq!(s.parameter_bounds(), Rectangle::new(0.0, 1.0, 0.0, 1.0));
        assert!(s.cpt(2, 0).is_err());
        assert!(Surface::new(vec![], 1, 1, vec![0.0], vec![0.0]).is_err());
        assert!(
            Surface::new(vec![vec![CPoint::ZERO], vec![]], 1, 1, vec![0.0], vec![0.0]).is_err()
        );
    }

    #[test]
    fn counts_and_weights() {
        let mut s = bilinear();
        s.set_size_indices(SurfaceIndices {
            n: 1,
            m: 1,
            p: 2,
            q: 1,
            r: 3,
            s: 3,
        })
        .unwrap();
        assert!(matches!(
            s.counts_are_valid(),
            Err(SolidError::Nlib {
                code: NlibCode::Surface,
                ..
            })
        ));
        let mut s = bilinear();
        s.cpt_mut(1, 1).unwrap().w = 2.0;
        assert!(matches!(
            s.counts_are_valid(),
            Err(SolidError::Nlib {
                code: NlibCode::Surface,
                ..
            })
        ));
        let mut s = bilinear();
        s.non_rat_to_rat();
        assert!(s.is_rational());
        assert!(s.weights_are_valid().is_ok());
        s.cpt_mut(0, 1).unwrap().w = 1.0e-4;
        assert!(matches!(
            s.weights_are_valid(),
            Err(SolidError::Nlib {
                code: NlibCode::Weight,
                ..
            })
        ));
    }

    #[test]
    fn swap_and_closed_and_flat() {
        let mut s = bilinear();
        s.cpt_mut(0, 1).unwrap().z = 5.0;
        let before = s.clone();
        s.swap_uv();
        assert_eq!(s.cpt(1, 0).unwrap(), before.cpt(0, 1).unwrap());
        s.swap_uv();
        assert_eq!(s, before);
        assert!(!s.is_closed(Direction::U));
        assert!(!s.is_closed(Direction::UV));
        let f = bilinear().is_flat_cheap(1e-6).unwrap();
        assert!(f.flat);
        let f = before.is_flat_cheap(1e-6).unwrap();
        assert!(!f.flat);
    }

    #[test]
    fn reparam_and_size_arrays() {
        let mut s = bilinear();
        s.reparam_to_interval(Rectangle::new(2.0, 4.0, -1.0, 1.0), Direction::UV)
            .unwrap();
        assert_eq!(s.u_knots(), &[2.0, 2.0, 4.0, 4.0]);
        assert_eq!(s.v_knots(), &[-1.0, -1.0, 1.0, 1.0]);
        let mut slot = None;
        s.copy_into(&mut slot);
        assert_eq!(slot.as_ref(), Some(&s));
        let mut short = Surface::new(vec![vec![CPoint::ZERO]], 1, 1, vec![0.0], vec![0.0]).unwrap();
        assert!(short
            .reparam_to_interval(Rectangle::new(2.0, 4.0, 0.0, 0.0), Direction::U)
            .is_err());
        let len = s.average_len();
        assert_eq!((len.area, len.lu, len.lv), (1.0, 1.0, 1.0));
        assert!(s.equals(&slot.unwrap(), 0.0, 0.0));
    }

    /// Fuzz regression (`nlib.srf_basic` fuzz00933, fuzz00159): indices sized from
    /// `i64::MAX` / `2^32` are a memory error, not an allocation abort or panic.
    #[test]
    fn size_arrays_huge_is_a_memory_error() {
        let idx = |n| SurfaceIndices {
            n,
            m: 1,
            p: 1,
            q: 1,
            r: 3,
            s: 3,
        };
        let mut slot = None;
        assert!(size_arrays(&mut slot, idx(usize::MAX)).is_err());
        assert!(slot.is_none());
        let mut slot = Some(bilinear());
        assert!(size_arrays(&mut slot, idx(1)).is_ok());
    }

    #[test]
    fn closed_cylinder_is_not_flat() {
        let w = 0.5f64.sqrt();
        let circ = [
            (1.0, 0.0, 1.0),
            (1.0, 1.0, w),
            (0.0, 1.0, 1.0),
            (-1.0, 1.0, w),
            (-1.0, 0.0, 1.0),
            (-1.0, -1.0, w),
            (0.0, -1.0, 1.0),
            (1.0, -1.0, w),
            (1.0, 0.0, 1.0),
        ];
        let net = circ
            .iter()
            .map(|&(x, y, w)| {
                vec![
                    CPoint::new(x * w, y * w, 0.0, w),
                    CPoint::new(x * w, y * w, 3.0 * w, w),
                ]
            })
            .collect();
        let u = vec![
            0.0, 0.0, 0.0, 0.25, 0.25, 0.5, 0.5, 0.75, 0.75, 1.0, 1.0, 1.0,
        ];
        let s = Surface::new(net, 2, 1, u, vec![0.0, 0.0, 1.0, 1.0]).unwrap();
        assert!(s.is_valid().is_ok());
        assert!(s.is_closed(Direction::U));
        assert!(!s.is_closed(Direction::V));
        assert!(!s.are_weights_equal());
        let f = s.is_flat_cheap(1e-3).unwrap();
        assert_eq!((f.flat, f.dir), (false, Direction::V));
        let e = s.enet(0, 1, 1, 1).unwrap();
        assert_eq!(e[1][0], Vec3::new(1.0, 1.0, 3.0));
        assert!(matches!(
            s.enet(0, 9, 0, 0),
            Err(SolidError::Nlib {
                code: NlibCode::Index,
                ..
            })
        ));
    }
}
