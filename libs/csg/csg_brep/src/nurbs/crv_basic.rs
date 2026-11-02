//! NURBS curve object: structure and accessors, storage sizing, copying, validity
//! checks, rationality and dimension conversions, closedness, degeneracy, straightness
//! and planarity tests, equality and compatibility of curve sets, numerator and
//! denominator split, conic segment counts and cross-boundary derivative curves of
//! non-rational surfaces.
//!
//! Upstream: `source/SMLib/src/NL_CrvBasic.cpp`, `source/SMLib/src/NL_SrfSymbol.cpp`
//! (`N_CrossBoundDerivCrvNonRatSrf` only); declarations in
//! `source/SMLib/inc/NL_CrvBasic.h`, `NL_SrfSymbol.h`, `NL_CURVE` in
//! `source/SMLib/inc/NL_Nurbsdef.h`.
//!
//! | Rust                                          | Upstream                          |
//! |-----------------------------------------------|-----------------------------------|
//! | [`Curve`]                                     | `NL_CURVE` (with its `NL_CPOLYGON` and `NL_KNOTVECTOR`) |
//! | [`Curve::new`], [`Curve::with_indices`]       | `N_CrvFromCPolygonAndKnotVector`  |
//! | [`Curve::n`], [`Curve::m`], [`Curve::array_sizes`] | `N_CrvGetArraySizes`         |
//! | [`Curve::degree`]                             | `N_CrvGetDegree`                  |
//! | [`Curve::cpts`], [`Curve::cpts_mut`]          | `N_CrvGetCPts`                    |
//! | [`Curve::knots`]                              | `N_CrvGetKnots`                   |
//! | [`Curve::knot_vector`]                        | `N_CrvGetKnotVector`              |
//! | the accessors above                           | `N_CrvGetCPtsDegreeAndKnots`, `N_CrvGetCPtsAndKnots`, `N_CrvGetCPtsKnotVectorAndKnots`, `N_CrvDetachPolygonKnot` |
//! | [`Curve::set_cpts_and_knots`]                 | `N_CrvSetCPtsAndKnots`            |
//! | [`Curve::param_bounds`]                       | `N_CrvGetParamBounds`             |
//! | [`Curve::set_size_indices`]                   | `N_CrvSetSizeIndices`             |
//! | [`Curve::compress`]                           | `N_CrvCompress`                   |
//! | `None` / `Option::is_none` on `Option<Curve>` | `N_CrvInitArrays` / `N_CrvAreArraysNULL` |
//! | [`size_arrays`]                               | `N_CrvSizeArrays`                 |
//! | [`Curve::copy_into`], `Clone`                 | `N_CrvCopy`                       |
//! | [`Curve::is_3d`]                              | `N_CrvIs3d`                       |
//! | [`Curve::weights_are_valid`]                  | `N_CrvWeightsAreValid`            |
//! | [`Curve::is_sized`]                           | `N_CrvIsSized`                    |
//! | [`Curve::is_rational`]                        | `N_IsCrvRat`                      |
//! | [`Curve::is_closed`], [`Curve::is_closed_tol`] | `N_CrvIsClosed`                  |
//! | [`Curve::is_degen`], [`Curve::is_degen_tol`]  | `N_CrvIsDegen`                    |
//! | [`Curve::is_line`]                            | `N_CrvIsLine`                     |
//! | [`curves_are_coincident`]                     | `N_CrvsAreCoincident`             |
//! | [`Curve::equals`]                             | `N_CrvsAreEqual`                  |
//! | [`curves_are_compatible`]                     | `N_CrvsAreCombatible`             |
//! | [`Curve::num_and_denom`]                      | `N_CrvGetNumAndDenom`             |
//! | [`curve_from_num_and_denom`]                  | `N_CreateCrvFromNumAndDenom`      |
//! | [`Curve::denom_func`]                         | `N_CrvGetDenomCrvFunc`            |
//! | [`Curve::epolygon`]                           | `N_CrvGetEPolygon`                |
//! | [`Curve::to_2d`], [`Curve::to_3d`]            | `N_Crv3dTo2d`, `N_Crv2dTo3d`      |
//! | [`Curve::rat_to_non_rat`], [`Curve::non_rat_to_rat`] | `N_CrvRatToNonRat`, `N_CrvNonRatToRat` |
//! | [`make_rat_compatible`]                       | `N_CrvsMakeRatCompatible`         |
//! | [`Curve::reparam_to_interval`]                | `N_CrvReparamToInterval`          |
//! | [`Curve::are_weights_equal`]                  | `N_CrvAreWeightsEqual`            |
//! | [`Curve::make_non_rat`]                       | `N_CrvMakeNonRat`                 |
//! | [`Curve::prune_rat`]                          | `N_CrvPruneRat`                   |
//! | [`Curve::is_4d_degen`]                        | `N_Crv4dIsDegen`                  |
//! | [`Curve::is_in_z0_plane`]                     | `N_CrvIsInZ0Plane`                |
//! | [`Curve::is_planar`], [`Planarity`]           | `N_CrvIsPlanar`                   |
//! | [`conic_data`], [`ConicType`], [`ConicData`]  | `N_CrvGetConicData`, `NL_QUADRATIC` / `NL_QUARTIC` / `NL_QUINTIC` |
//! | [`cross_bound_deriv_crv_non_rat_srf`], [`Boundary`] | `N_CrossBoundDerivCrvNonRatSrf`, `NL_LEFT` / `NL_RIGHT` / `NL_BOTTOM` / `NL_TOP` |
//!
//! Deviations:
//! - [`Curve`] owns its control points and knots (upstream points at caller- or
//!   stack-owned `NL_CPOLYGON` / `NL_KNOTVECTOR` objects). As upstream, the highest
//!   indices in use (`n`, `m`) are kept apart from the storage, which may be larger;
//!   every index is checked against the storage, so accesses upstream would make
//!   outside its arrays are `InvalidInput` errors instead.
//! - The NULL curve of `N_CrvInitArrays` (null pointers, degree `-1`) is `None` in an
//!   `Option<Curve>`; partly NULL curves cannot be expressed. Storage that upstream
//!   allocates uninitialised (`malloc`) is zero-filled.
//! - [`size_arrays`] rejects index overflow, failed reservations and arrays above
//!   [`crate::limits::MAX_ALLOC_BYTES`] with [`NlibCode::Memory`], leaving a `None`
//!   slot unchanged even where upstream's allocator might accept the request.
//! - The `NL_STACKS` arguments have no counterpart and allocation cannot fail, so
//!   [`Curve::compress`] and [`Curve::epolygon`] have no memory error path.
//! - Out-params become return values; errors are [`SolidError::Nlib`] values with the
//!   upstream `NL_*_ERR` number instead of a flag plus the global `NL_ERROR` record.
//!   The routine-name argument (`rname`) is dropped.
//! - `N_CrvIsClosed` and `N_CrvIsDegen` read the global `NL_MTOL`:
//!   [`Curve::is_closed`] / [`Curve::is_degen`] use [`MODEL_TOL`], the `_tol` forms
//!   take it as an argument.
//! - `N_CrvReparamToInterval` writes `U[m-p..]` for `m < p` (outside the array);
//!   [`Curve::reparam_to_interval`] returns `InvalidInput` instead, before changing
//!   anything.
//! - `N_CrvGetConicData` loops forever for non-finite angles or angles so large that
//!   adding 360 no longer changes them; [`conic_data`] returns `InvalidInput` for
//!   non-finite angles and after [`CONIC_MAX_TURNS`] turns. The flag of an unknown
//!   conic type (which leaves `n` and `p` unwritten upstream) cannot be expressed
//!   with [`ConicType`].
//! - `N_CrvIsPlanar` returns the error flag while still writing its point and normal;
//!   [`Curve::is_planar`] returns only the error then. When `N_VectorsAngle` fails
//!   while the cross product still normalises (upstream then reads the angle of an
//!   earlier vertex, or an uninitialised one), the port uses the angle of the
//!   previous vertex, NaN at the first.
//! - `N_CrvsAreCombatible` reads `cur[0]` even for an empty set; [`curves_are_compatible`]
//!   returns `true` for an empty slice.
//! - `N_CrossBoundDerivCrvNonRatSrf` with a side flag other than the four boundaries
//!   fails with `NL_INP_ERR`; [`Boundary`] cannot express such a flag. Surfaces too
//!   small for the requested side (fewer than two rows / columns, too few knots) are
//!   `InvalidInput` (upstream reads outside its arrays).
//! - `N_CrvsAreCoincident` maps a failure of `N_CrvsMakeCompatible` to `false`, as
//!   upstream; `InvalidInput` (where upstream would access memory outside its arrays)
//!   propagates.

// Branches keep upstream's `if( x NEQ NL_NOZ ) ... else ...` order for traceability.
#![allow(clippy::if_not_else)]

use std::sync::Arc;

use crate::error::{NlibCode, Result, SolidError};
use crate::limits;
use crate::math::Vec3;
use crate::nurbs::basis_basic::PTOL;
use crate::nurbs::crv_tool::p2::make_compatible;
use crate::nurbs::funcs_basic::CurveFunction;
use crate::nurbs::geom::{
    dist_cpt_cpt_homo, dist_perp_pt_line_seg, dist_pt_pt, dist_signed_pt_plane, normalize,
    plane_pt_normal_to_implicit, pt_along_vector, vectors_angle, Interval,
};
use crate::nurbs::knots::KnotVector;
use crate::nurbs::math::{CPoint, MODEL_TOL, NOW, NOZ};
use crate::nurbs::srf_basic::{Surface, WEIGHT_MAX, WEIGHT_MIN, WEIGHT_TOL};

/// Most turns of 360 degrees [`conic_data`] adds or removes while normalising the end
/// angle before giving up.
pub const CONIC_MAX_TURNS: usize = 1 << 20;

fn nlib(operation: &'static str, code: NlibCode) -> SolidError {
    SolidError::Nlib { operation, code }
}

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

/// A NURBS curve (upstream `NL_CURVE`): control points `Pw[0..=n]`, degree `p`,
/// knots `U[0..=m]`.
///
/// The indices in use are kept apart from the storage (which may be larger), as
/// upstream; the constructors guarantee that the storage holds them.
///
/// The control point and knot storage are shared copy-on-write (`Arc`, written
/// through [`Arc::make_mut`]): a clone shares them until one side writes.
#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    /// Control point storage, at least `n + 1` entries.
    pw: Arc<Vec<CPoint>>,
    n: usize,
    p: usize,
    /// Knot storage, at least `m + 1` entries.
    u: Arc<Vec<f64>>,
    m: usize,
}

impl Curve {
    /// Curve over whole arrays (`n = pw.len() - 1`, `m = u.len() - 1`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an empty control point or knot array. Nothing
    /// else is checked.
    pub fn new(pw: Vec<CPoint>, p: usize, u: Vec<f64>) -> Result<Self> {
        let n = pw
            .len()
            .checked_sub(1)
            .ok_or_else(|| invalid("nurbs::crv_basic::Curve::new", "empty control polygon"))?;
        let m = u
            .len()
            .checked_sub(1)
            .ok_or_else(|| invalid("nurbs::crv_basic::Curve::new", "empty knot vector"))?;
        Ok(Self {
            pw: Arc::new(pw),
            n,
            p,
            u: Arc::new(u),
            m,
        })
    }

    /// Curve over the first `n + 1` control points of `pw` and the first `m + 1`
    /// knots of `u` (upstream `N_CrvFromCPolygonAndKnotVector` with a polygon and
    /// knot vector whose indices may be below their storage).
    ///
    /// # Errors
    ///
    /// `InvalidInput` when the storage does not hold the indices.
    pub fn with_indices(
        pw: Vec<CPoint>,
        n: usize,
        p: usize,
        u: Vec<f64>,
        m: usize,
    ) -> Result<Self> {
        if pw.len() <= n || u.len() <= m {
            return Err(invalid(
                "nurbs::crv_basic::Curve::with_indices",
                "storage smaller than (n, m)",
            ));
        }
        Ok(Self {
            pw: Arc::new(pw),
            n,
            p,
            u: Arc::new(u),
            m,
        })
    }

    /// A zero-filled curve with indices `(n, m)` and degree `p`.
    fn zeroed(n: usize, p: usize, m: usize) -> Result<Self> {
        let mem = || nlib("nurbs::crv_basic::size_arrays", NlibCode::Memory);
        let np = n.checked_add(1).ok_or_else(mem)?;
        let nk = m.checked_add(1).ok_or_else(mem)?;
        Ok(Self {
            pw: Arc::new(limits::try_filled(np, CPoint::default()).ok_or_else(mem)?),
            n,
            p,
            u: Arc::new(limits::try_filled(nk, 0.0).ok_or_else(mem)?),
            m,
        })
    }

    /// Highest index of the control points in use.
    #[must_use]
    pub fn n(&self) -> usize {
        self.n
    }

    /// Highest index of the knots in use.
    #[must_use]
    pub fn m(&self) -> usize {
        self.m
    }

    /// The degree `p` (`N_CrvGetDegree`).
    #[must_use]
    pub fn degree(&self) -> usize {
        self.p
    }

    /// `(n, m)` (`N_CrvGetArraySizes`).
    #[must_use]
    pub fn array_sizes(&self) -> (usize, usize) {
        (self.n, self.m)
    }

    /// The control points in use, `Pw[0..=n]` (`N_CrvGetCPts`).
    #[must_use]
    pub fn cpts(&self) -> &[CPoint] {
        self.pw.get(..=self.n).unwrap_or(&[])
    }

    /// Mutable control points in use, `Pw[0..=n]`.
    pub fn cpts_mut(&mut self) -> &mut [CPoint] {
        Arc::make_mut(&mut self.pw)
            .get_mut(..=self.n)
            .unwrap_or(&mut [])
    }

    /// The knots in use, `U[0..=m]` (`N_CrvGetKnots`).
    #[must_use]
    pub fn knots(&self) -> &[f64] {
        self.u.get(..=self.m).unwrap_or(&[])
    }

    /// Mutable knots in use, `U[0..=m]`.
    pub fn knots_mut(&mut self) -> &mut [f64] {
        Arc::make_mut(&mut self.u)
            .get_mut(..=self.m)
            .unwrap_or(&mut [])
    }

    /// Sizes of the control point and knot storage (highest storable indices plus
    /// one).
    #[must_use]
    pub fn storage(&self) -> (usize, usize) {
        (self.pw.len(), self.u.len())
    }

    /// The knot vector `U[0..=m]` over the knot storage (`N_CrvGetKnotVector`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` when the knot storage does not hold `m + 1` knots (never
    /// for a curve built by the constructors).
    pub fn knot_vector(&self) -> Result<KnotVector<'_>> {
        KnotVector::new(&self.u, self.m)
    }

    /// Replaces the control point and knot storage, keeping the indices
    /// (`N_CrvSetCPtsAndKnots`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` (and changes nothing) when the new storage does not
    /// hold the current indices.
    pub fn set_cpts_and_knots(&mut self, pw: Vec<CPoint>, u: Vec<f64>) -> Result<()> {
        if pw.len() <= self.n || u.len() <= self.m {
            return Err(invalid(
                "nurbs::crv_basic::Curve::set_cpts_and_knots",
                "storage smaller than the indices",
            ));
        }
        self.pw = Arc::new(pw);
        self.u = Arc::new(u);
        Ok(())
    }

    /// `(U[0], U[m])` (`N_CrvGetParamBounds`).
    #[must_use]
    pub fn param_bounds(&self) -> (f64, f64) {
        let u = self.knots();
        (
            u.first().copied().unwrap_or(f64::NAN),
            u.last().copied().unwrap_or(f64::NAN),
        )
    }

    /// Sets the indices and the degree (`N_CrvSetSizeIndices`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` (and changes nothing) when the storage does not hold
    /// the new indices (upstream would index past its arrays later).
    pub fn set_size_indices(&mut self, n: usize, p: usize, m: usize) -> Result<()> {
        if self.pw.len() <= n || self.u.len() <= m {
            return Err(invalid(
                "nurbs::crv_basic::Curve::set_size_indices",
                "storage smaller than the indices",
            ));
        }
        self.n = n;
        self.p = p;
        self.m = m;
        Ok(())
    }

    /// Shrinks the storage to exactly the indices in use (`N_CrvCompress`).
    pub fn compress(&mut self) {
        if self.pw.len() == self.n.saturating_add(1) && self.u.len() == self.m.saturating_add(1) {
            // (already exactly the indices in use: the same storage values, kept
            // shared)
            return;
        }
        let pw = self.cpts().to_vec();
        let u = self.knots().to_vec();
        self.pw = Arc::new(pw);
        self.u = Arc::new(u);
    }

    /// Copies this curve into `dst` (`N_CrvCopy`): a NULL (`None`) destination is
    /// allocated, an existing one is resized with [`size_arrays`], then the control
    /// points and knots in use are copied.
    ///
    /// # Errors
    ///
    /// [`NlibCode::Storage`] (leaving `dst` unchanged) when an existing
    /// destination is smaller than this curve.
    pub fn copy_into(&self, dst: &mut Option<Curve>) -> Result<()> {
        let cur = size_arrays(dst, self.n, self.p, self.m)?;
        for (q, p) in cur.cpts_mut().iter_mut().zip(self.cpts()) {
            *q = *p;
        }
        for (q, p) in cur.knots_mut().iter_mut().zip(self.knots()) {
            *q = *p;
        }
        Ok(())
    }

    // ---- error checks ---------------------------------------------------------------

    /// For a rational curve, checks that every weight is in
    /// `[WEIGHT_MIN, WEIGHT_MAX]` (`N_CrvWeightsAreValid`; NaN weights pass).
    ///
    /// # Errors
    ///
    /// [`NlibCode::Weight`] (`NL_WEI_ERR`).
    pub fn weights_are_valid(&self) -> Result<()> {
        if self.is_rational() {
            for pw in self.cpts() {
                let w = pw.w;
                if w < WEIGHT_MIN || w > WEIGHT_MAX {
                    return Err(nlib(
                        "nurbs::crv_basic::Curve::weights_are_valid",
                        NlibCode::Weight,
                    ));
                }
            }
        }
        Ok(())
    }

    /// Checks that the indices in use are at least `np` and `mk` (`N_CrvIsSized`).
    ///
    /// # Errors
    ///
    /// [`NlibCode::Storage`] (`NL_STO_ERR`).
    pub fn is_sized(&self, np: usize, mk: usize) -> Result<()> {
        if self.n < np || self.m < mk {
            return Err(nlib("nurbs::crv_basic::Curve::is_sized", NlibCode::Storage));
        }
        Ok(())
    }

    // ---- predicates -----------------------------------------------------------------

    /// `true` when `Pw[0]` has a `z` coordinate (`N_CrvIs3d`).
    #[must_use]
    pub fn is_3d(&self) -> bool {
        self.first().z != NOZ
    }

    /// `true` when `Pw[0]` carries a weight (`N_IsCrvRat`).
    #[must_use]
    pub fn is_rational(&self) -> bool {
        self.first().w != NOW
    }

    /// `Pw[0]` (always in the storage by construction; NaN otherwise).
    fn first(&self) -> CPoint {
        self.cpts()
            .first()
            .copied()
            .unwrap_or(CPoint::new(f64::NAN, f64::NAN, f64::NAN, f64::NAN))
    }

    /// `Pw[n]` (always in the storage by construction; NaN otherwise).
    fn last(&self) -> CPoint {
        self.cpts()
            .last()
            .copied()
            .unwrap_or(CPoint::new(f64::NAN, f64::NAN, f64::NAN, f64::NAN))
    }

    /// Closedness with `NL_MTOL` = [`MODEL_TOL`] (`N_CrvIsClosed`).
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.is_closed_tol(MODEL_TOL)
    }

    /// `true` when the Euclidean images of `Pw[0]` and `Pw[n]` are closer than `mtol`
    /// (`N_CrvIsClosed` with `NL_MTOL = mtol`).
    #[must_use]
    pub fn is_closed_tol(&self, mtol: f64) -> bool {
        let p0 = self.first().to_euclid();
        let pn = self.last().to_euclid();
        let d = dist_pt_pt(p0, pn);
        d < mtol
    }

    /// Degeneracy to a point with `NL_MTOL` = [`MODEL_TOL`] (`N_CrvIsDegen`).
    #[must_use]
    pub fn is_degen(&self) -> bool {
        self.is_degen_tol(MODEL_TOL)
    }

    /// `true` when every Euclidean control point is within `mtol` of their centroid
    /// (`N_CrvIsDegen` with `NL_MTOL = mtol`).
    #[must_use]
    pub fn is_degen_tol(&self, mtol: f64) -> bool {
        let fac = 1.0 / (self.n as f64 + 1.0);
        let mut m = Vec3::ZERO;
        for pw in self.cpts() {
            let q = pw.to_euclid();
            m += q;
        }
        m = scale(m, fac);
        for pw in self.cpts() {
            let q = pw.to_euclid();
            let d = dist_pt_pt(q, m);
            if d > mtol {
                return false;
            }
        }
        true
    }

    /// `true` when the curve is a straight line within `tol` (`N_CrvIsLine`): every
    /// interior Euclidean control point has a perpendicular foot on the chord
    /// `Pw[0] Pw[n]` (extended by `tol` at both ends) at most `tol` away.
    ///
    /// When the chord is shorter than `tol`, an interior point farther than `tol` from
    /// `Pw[0]` makes the curve not straight before the chord test.
    #[must_use]
    pub fn is_line(&self, tol: f64) -> bool {
        let pts = self.cpts();
        let interior = pts.get(1..self.n).unwrap_or(&[]);

        // Check straightness.
        let a = self.first().to_euclid();
        let b = self.last().to_euclid();
        let d = dist_pt_pt(a, b);
        if d <= tol {
            for pw in interior {
                let q = pw.to_euclid();
                let d = dist_pt_pt(a, q);
                if d > tol {
                    return false;
                }
            }
        }

        // Extend A and B out a little, so that even small hooks at the ends project
        // onto the line.
        let mut line = b - a;
        let length = line.length();
        // `N_VectorNormalizeRef` leaves the vector as it is when it fails.
        if let Ok((unit, _)) = normalize(line) {
            line = unit;
        }
        let bprime = pt_along_vector(a, length + tol, line);
        line = Vec3::new(-line.x, -line.y, -line.z);
        let aprime = pt_along_vector(b, length + tol, line);

        for pw in interior {
            let q = pw.to_euclid();
            match dist_perp_pt_line_seg(q, aprime, bprime) {
                Ok(Some(d)) => {
                    if d < 0.0 || d > tol {
                        return false;
                    }
                }
                // The foot is outside the segment (upstream `d = -1`), or the
                // distance failed.
                Ok(None) | Err(_) => return false,
            }
        }
        true
    }

    /// `true` when both curves have the same degree and index counts, every pair of
    /// control points is within `tol3d` in homogeneous space and every pair of knots
    /// within `tol1d` (`N_CrvsAreEqual`).
    #[must_use]
    pub fn equals(&self, other: &Curve, tol3d: f64, tol1d: f64) -> bool {
        // Check counts, then control points, then knots.
        self.p == other.p
            && self.n == other.n
            && self.m == other.m
            && self
                .cpts()
                .iter()
                .zip(other.cpts())
                .all(|(pw, qw)| CPoint::diff(*pw, *qw).magnitude() <= tol3d)
            && self
                .knots()
                .iter()
                .zip(other.knots())
                .all(|(a, b)| (a - b).abs() <= tol1d)
    }

    /// `true` when `|max w - min w| < WEIGHT_TOL` (`N_CrvAreWeightsEqual`).
    #[must_use]
    pub fn are_weights_equal(&self) -> bool {
        let w = self.first().w;
        let (mut wmin, mut wmax) = (w, w);
        for pw in self.cpts().iter().skip(1) {
            let w = pw.w;
            if w < wmin {
                wmin = w;
            }
            if w > wmax {
                wmax = w;
            }
        }
        (wmax - wmin).abs() < WEIGHT_TOL
    }

    /// `true` when every control point is within [`MODEL_TOL`] of their centroid in
    /// homogeneous space (`N_Crv4dIsDegen`).
    #[must_use]
    pub fn is_4d_degen(&self) -> bool {
        let fac = 1.0 / (self.n as f64 + 1.0);
        let mut mw = CPoint::ZERO;
        for pw in self.cpts() {
            mw = CPoint::sum(mw, *pw);
        }
        mw = mw.scale(fac);
        for pw in self.cpts() {
            let d = dist_cpt_cpt_homo(*pw, mw);
            if d > MODEL_TOL {
                return false;
            }
        }
        true
    }

    /// `true` when every `z` is `0` or missing (`N_CrvIsInZ0Plane`).
    #[must_use]
    pub fn is_in_z0_plane(&self) -> bool {
        self.cpts().iter().all(|pw| pw.z == 0.0 || pw.z == NOZ)
    }

    /// Planarity test (`N_CrvIsPlanar`): a curve with at least three control points
    /// that is neither degenerate ([`Curve::is_degen`]) nor a line within `tol`
    /// ([`Curve::is_line`]) gets the plane through the centroid of its Euclidean
    /// control points, with the averaged normal of the control-polygon corners
    /// (corners with an angle in `[1, 179]` degrees preferred); it is planar when
    /// every control point is within `tol` of that plane.
    ///
    /// # Errors
    ///
    /// [`NlibCode::Geometry`] when the averaged normal (or the plane)
    /// cannot be normalised.
    pub fn is_planar(&self, tol: f64) -> Result<Planarity> {
        let not_planar = Planarity {
            flat: false,
            plane: None,
        };
        let n = self.n;

        // Check special cases.
        if n < 2 || self.is_degen() || self.is_line(tol) {
            return Ok(not_planar);
        }

        // Project all control points to Euclidean space.
        let p: Vec<Vec3> = self.cpts().iter().map(|pw| pw.to_euclid()).collect();

        // Compute candidate point on plane and normal vector.
        let (mut count1, mut count2) = (0usize, 0usize);
        let mut cg = Vec3::ZERO;
        let mut nn1 = Vec3::ZERO;
        let mut nn2 = Vec3::ZERO;
        let mut ang = f64::NAN;
        for (ii, pi) in p.iter().enumerate() {
            cg += *pi;
            if ii > 0 && ii < n {
                let (Some(prev), Some(next)) = (p.get(ii - 1), p.get(ii + 1)) else {
                    return Err(invalid(
                        "nurbs::crv_basic::Curve::is_planar",
                        "control point index out of range",
                    ));
                };
                let v1 = *pi - *prev;
                let v2 = *next - *pi;
                if let Ok(a) = vectors_angle(v1, v2) {
                    ang = a;
                }
                // A corner whose normal does not normalise adds nothing.
                let Ok((mut vn, _)) = normalize(v1.cross(v2)) else {
                    continue;
                };
                // Orient along the normals summed so far.
                let reference = if count1 > 0 {
                    Some(nn1)
                } else if count2 > 0 {
                    Some(nn2)
                } else {
                    None
                };
                if reference.is_some_and(|r| vn.dot(r) < 0.0) {
                    vn = scale(vn, -1.0);
                }
                if ang < 1.0 || ang > 179. {
                    nn2 += vn;
                    count2 += 1;
                } else {
                    nn1 += vn;
                    count1 += 1;
                }
            }
        }
        if count1 + count2 == 0 {
            return Ok(not_planar);
        }

        let pp = scale(cg, 1.0 / (n as f64 + 1.0));
        let nn = if count1 > 0 {
            scale(nn1, 1.0 / count1 as f64)
        } else {
            scale(nn2, 1.0 / count2 as f64)
        };
        let (nn, _) = normalize(nn)
            .map_err(|_| nlib("nurbs::crv_basic::Curve::is_planar", NlibCode::Geometry))?;

        // Check flatness.
        let [a, b, c, d] = plane_pt_normal_to_implicit(pp, nn)
            .map_err(|_| nlib("nurbs::crv_basic::Curve::is_planar", NlibCode::Geometry))?;
        let plane = Some((pp, nn));
        for pi in &p {
            let fac = dist_signed_pt_plane(a, b, c, d, *pi);
            if fac.abs() > tol {
                return Ok(Planarity { flat: false, plane });
            }
        }
        Ok(Planarity { flat: true, plane })
    }

    // ---- data extraction ------------------------------------------------------------

    /// Splits the curve into a non-rational numerator curve (the weighted
    /// coordinates, `w = NOW`) and, for a rational curve, a denominator function (the
    /// weights) over the same knots (`N_CrvGetNumAndDenom`). `num` and `den` are
    /// sized with [`size_arrays`] / [`CurveFunction::size_arrays`]; `den` is left
    /// alone for a non-rational curve.
    ///
    /// # Errors
    ///
    /// [`NlibCode::Storage`] when an existing `num` or `den` is too small
    /// (`num` is already resized when `den` fails, as upstream).
    pub fn num_and_denom(
        &self,
        num: &mut Option<Curve>,
        den: &mut Option<CurveFunction>,
    ) -> Result<()> {
        let rat = self.is_rational();
        let (n, p, m) = (self.n, self.p, self.m);
        let numc = size_arrays(num, n, p, m)?;
        let mut denf = if rat {
            Some(CurveFunction::size_arrays(den, n, p, m)?)
        } else {
            None
        };

        // Define output entities.
        for (i, pw) in self.cpts().iter().enumerate() {
            if let Some(nw) = numc.cpts_mut().get_mut(i) {
                *nw = CPoint::new(pw.x, pw.y, pw.z, NOW);
            }
            if let Some(f) = denf.as_mut().and_then(|d| d.values_mut().get_mut(i)) {
                *f = pw.w;
            }
        }
        for (i, uc) in self.knots().iter().enumerate() {
            if let Some(un) = numc.knots_mut().get_mut(i) {
                *un = *uc;
            }
            if let Some(ud) = denf.as_mut().and_then(|d| d.knots_mut().get_mut(i)) {
                *ud = *uc;
            }
        }
        Ok(())
    }

    /// The weights as a curve function over the same knots (`N_CrvGetDenomCrvFunc`);
    /// a non-rational curve gives [`NOW`] values. `cfn` is sized with
    /// [`CurveFunction::size_arrays`].
    ///
    /// # Errors
    ///
    /// [`NlibCode::Storage`] when an existing `cfn` is too small.
    pub fn denom_func(&self, cfn: &mut Option<CurveFunction>) -> Result<()> {
        let f = CurveFunction::size_arrays(cfn, self.n, self.p, self.m)?;
        for (fu, pw) in f.values_mut().iter_mut().zip(self.cpts()) {
            *fu = pw.w;
        }
        for (uf, u) in f.knots_mut().iter_mut().zip(self.knots()) {
            *uf = *u;
        }
        Ok(())
    }

    /// Euclidean images of `Pw[k..=l]` (`N_CrvGetEPolygon`).
    ///
    /// # Errors
    ///
    /// [`NlibCode::Index`] (`NL_IND_ERR`) when `k > l` or `l > n`.
    pub fn epolygon(&self, k: usize, l: usize) -> Result<Vec<Vec3>> {
        Ok(self.epolygon_iter(k, l)?.collect())
    }

    /// [`Curve::epolygon`] without the allocation: the Euclidean images of
    /// `Pw[k..=l]`, in order.
    ///
    /// # Errors
    ///
    /// As [`Curve::epolygon`].
    pub fn epolygon_iter(&self, k: usize, l: usize) -> Result<impl Iterator<Item = Vec3> + '_> {
        if k > l || l > self.n {
            return Err(nlib("nurbs::crv_basic::Curve::epolygon", NlibCode::Index));
        }
        let part = self
            .cpts()
            .get(k..=l)
            .ok_or_else(|| nlib("nurbs::crv_basic::Curve::epolygon", NlibCode::Index))?;
        Ok(part.iter().map(|pw| pw.to_euclid()))
    }

    // ---- conversions ----------------------------------------------------------------

    /// Sets every `z` to [`NOZ`] (`N_Crv3dTo2d`).
    pub fn to_2d(&mut self) {
        for pw in self.cpts_mut() {
            pw.z = NOZ;
        }
    }

    /// Sets every `z` to `0.0` (`N_Crv2dTo3d`).
    pub fn to_3d(&mut self) {
        for pw in self.cpts_mut() {
            pw.z = 0.0;
        }
    }

    /// Sets every weight to [`NOW`] without dividing (`N_CrvRatToNonRat`).
    pub fn rat_to_non_rat(&mut self) {
        for pw in self.cpts_mut() {
            pw.w = NOW;
        }
    }

    /// Sets every weight to `1.0` (`N_CrvNonRatToRat`); the other coordinates are
    /// kept as they are.
    pub fn non_rat_to_rat(&mut self) {
        for pw in self.cpts_mut() {
            pw.w = 1.0;
        }
    }

    /// Maps every control point to its Euclidean image `(x, y, z, NOW)`
    /// (`N_CrvMakeNonRat`).
    pub fn make_non_rat(&mut self) {
        for pw in self.cpts_mut() {
            *pw = pw.unweighted();
        }
    }

    /// Converts a rational curve with equal weights to non-rational form and, when
    /// `to_2d_if_flat`, a curve in the `z = 0` plane to 2D (`N_CrvPruneRat`).
    pub fn prune_rat(&mut self, to_2d_if_flat: bool) {
        if self.is_rational() && self.are_weights_equal() {
            self.make_non_rat();
        }
        if to_2d_if_flat && self.is_in_z0_plane() {
            self.to_2d();
        }
    }

    /// Rescales the knots in place to `[I.ul, I.ur]` (`N_CrvReparamToInterval`): the
    /// first `p + 1` knots become the new start, the last `p + 1` the new end, and the
    /// interior knots are mapped affinely. Nothing changes when the bounds already
    /// match.
    ///
    /// # Errors
    ///
    /// `InvalidInput` (changing nothing) when `m < p` and the knots would
    /// be rescaled, where upstream writes outside the array.
    pub fn reparam_to_interval(&mut self, interval: Interval) -> Result<()> {
        let oob = || {
            invalid(
                "nurbs::crv_basic::Curve::reparam_to_interval",
                "knot vector shorter than the degree",
            )
        };
        let (a, b) = (interval.ul, interval.ur);
        let (p, m) = (self.p, self.m);
        let u = self.knots_mut();
        let (Some(&u0), Some(&um)) = (u.first(), u.last()) else {
            return Err(oob());
        };
        if a != u0 || b != um {
            if m < p {
                return Err(oob());
            }
            let fac = (b - a) / (um - u0);
            for x in u.get_mut(..=p).ok_or_else(oob)? {
                *x = a;
            }
            // `U[0]` was overwritten above, so the interior uses the saved `u0`.
            if let Some(end) = (m - p).checked_sub(1) {
                if p < end {
                    for x in u.get_mut(p + 1..=end).ok_or_else(oob)? {
                        *x = fac * (*x - u0) + a;
                    }
                }
            }
            for x in u.get_mut(m - p..=m).ok_or_else(oob)? {
                *x = b;
            }
        }
        Ok(())
    }
}

/// `alpha * V` with upstream's operand order (`N_VectorScale`).
fn scale(v: Vec3, alpha: f64) -> Vec3 {
    Vec3::new(alpha * v.x, alpha * v.y, alpha * v.z)
}

/// Result of [`Curve::is_planar`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Planarity {
    /// Every control point lies within the tolerance of the plane.
    pub flat: bool,
    /// The candidate plane `(point, unit normal)`; `None` when the curve has fewer
    /// than three control points, is degenerate, is a line, or no polygon corner gives
    /// a normal (upstream leaves `PP` and `NN` unwritten then).
    pub plane: Option<(Vec3, Vec3)>,
}

/// Makes `slot` able to hold a curve with indices `(n, m)` and degree `p`, and sets
/// them (`N_CrvSizeArrays`).
///
/// A NULL (`None`) slot gets new zero-filled storage. An existing curve must have
/// indices in use of at least `n` and `m` ([`Curve::is_sized`]); its storage is kept
/// and the indices and degree are reset to the requested values. Returns the sized
/// curve.
///
/// # Errors
///
/// [`NlibCode::Storage`] (leaving the slot unchanged) when the existing
/// curve is too small. [`NlibCode::Memory`] (also leaving the slot unchanged)
/// when new storage overflows, exceeds [`limits::MAX_ALLOC_BYTES`] per array,
/// or cannot be allocated.
pub fn size_arrays(slot: &mut Option<Curve>, n: usize, p: usize, m: usize) -> Result<&mut Curve> {
    match slot {
        Some(cur) => {
            cur.is_sized(n, m)?;
            cur.set_size_indices(n, p, m)?;
            Ok(cur)
        }
        None => Ok(slot.insert(Curve::zeroed(n, p, m)?)),
    }
}

/// Builds `cur` from a non-rational numerator curve and an optional denominator
/// function (`N_CreateCrvFromNumAndDenom`): `Pw[i] = (Nw[i].x, Nw[i].y, Nw[i].z,
/// fu[i])` (`w = NOW` without a denominator), knots from the numerator. `cur` is
/// sized with [`size_arrays`].
///
/// # Errors
///
/// [`NlibCode::Input`] when the denominator's indices or degree differ
/// from the numerator's, and [`NlibCode::Storage`] when an existing `cur` is too
/// small.
pub fn curve_from_num_and_denom(
    num: &Curve,
    den: Option<&CurveFunction>,
    cur: &mut Option<Curve>,
) -> Result<()> {
    let (nn, pn, mn) = (num.n, num.p, num.m);
    if let Some(d) = den {
        let (nd, md) = d.array_sizes();
        if nn != nd || mn != md || pn != d.degree() {
            return Err(nlib(
                "nurbs::crv_basic::curve_from_num_and_denom",
                NlibCode::Input,
            ));
        }
    }
    let c = size_arrays(cur, nn, pn, mn)?;

    // Define curve.
    for (i, nw) in num.cpts().iter().enumerate() {
        let w = match den {
            Some(d) => d.values().get(i).copied().unwrap_or(f64::NAN),
            None => NOW,
        };
        if let Some(pw) = c.cpts_mut().get_mut(i) {
            *pw = CPoint::new(nw.x, nw.y, nw.z, w);
        }
    }
    for (uc, un) in c.knots_mut().iter_mut().zip(num.knots()) {
        *uc = *un;
    }
    Ok(())
}

/// `true` when all curves are rational or all non-rational, all 2D or all 3D, all
/// have the same indices and all knots agree with the first curve's within
/// `NL_PTOL` (`N_CrvsAreCombatible`). An empty set is compatible.
#[must_use]
pub fn curves_are_compatible(curves: &[Curve]) -> bool {
    let Some(first) = curves.first() else {
        return true;
    };

    // All must be rational or non-rational.
    let allra = curves.iter().all(Curve::is_rational);
    let allnra = !curves.iter().any(Curve::is_rational);
    if !allra && !allnra {
        return false;
    }

    // All must be 2-D or 3-D.
    let all2d = !curves.iter().any(Curve::is_3d);
    let all3d = curves.iter().all(Curve::is_3d);
    if !all2d && !all3d {
        return false;
    }

    // All must have the same indexes.
    let (n0, m0) = first.array_sizes();
    if curves.iter().skip(1).any(|c| c.array_sizes() != (n0, m0)) {
        return false;
    }

    // All must have the same knots.
    let u0 = first.knots();
    for c in curves.iter().skip(1) {
        for (u, v) in c.knots().iter().zip(u0) {
            if (u - v).abs() > PTOL {
                return false;
            }
        }
    }
    true
}

/// Makes the curves rationality- and dimension-compatible
/// (`N_CrvsMakeRatCompatible`): when any is rational, every non-rational one gets
/// weights `1.0`; when any is 3D, every 2D one gets `z = 0`.
pub fn make_rat_compatible(curves: &mut [Curve]) {
    let allnra = !curves.iter().any(Curve::is_rational);
    let all2d = !curves.iter().any(Curve::is_3d);
    if !allnra {
        for cur in curves.iter_mut() {
            if !cur.is_rational() {
                cur.non_rat_to_rat();
            }
        }
    }
    if !all2d {
        for cur in curves.iter_mut() {
            if !cur.is_3d() {
                cur.to_3d();
            }
        }
    }
}

/// `true` when the two curves, made compatible in place (common degree and knots),
/// have every pair of control points closer than `tol` in homogeneous space
/// (`N_CrvsAreCoincident`). Both curves are modified as upstream. When making them
/// compatible fails with an NLib error the curves keep the changes made so far and
/// the result is `false`, as upstream.
///
/// # Errors
///
/// `InvalidInput` where upstream would read or write outside its arrays.
pub fn curves_are_coincident(p: &mut Curve, q: &mut Curve, tol: f64) -> Result<bool> {
    // Make input curves compatible.
    let mut pair = [p.clone(), q.clone()];
    let res = make_compatible(&mut pair);
    let [cp, cq] = pair;
    *p = cp;
    *q = cq;
    match res {
        Ok(()) => {}
        Err(SolidError::Nlib { .. }) => return Ok(false),
        Err(e) => return Err(e),
    }

    // Check coincidence.
    let n = q.n;
    let mut dwmax = -1.0;
    for i in 0..=n {
        let (Some(pw), Some(qw)) = (p.cpts().get(i), q.cpts().get(i)) else {
            return Err(invalid(
                "nurbs::crv_basic::curves_are_coincident",
                "control point index out of range",
            ));
        };
        let dw = dist_cpt_cpt_homo(*pw, *qw);
        if dw > dwmax {
            dwmax = dw;
        }
    }
    Ok(dwmax < tol)
}

/// Conic type of [`conic_data`] (`NL_QUADRATIC = 2`, `NL_QUARTIC = 4`,
/// `NL_QUINTIC = 5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConicType {
    /// `NL_QUADRATIC`: degree 2, up to four segments.
    Quadratic,
    /// `NL_QUARTIC`: degree 4, up to three segments.
    Quartic,
    /// `NL_QUINTIC`: degree 5, one segment.
    Quintic,
}

impl ConicType {
    /// The upstream flag value.
    #[must_use]
    pub const fn flag(self) -> i32 {
        match self {
            ConicType::Quadratic => 2,
            ConicType::Quartic => 4,
            ConicType::Quintic => 5,
        }
    }

    /// The conic type for an upstream flag value, `None` for any other value.
    #[must_use]
    pub const fn from_flag(flag: i32) -> Option<Self> {
        match flag {
            2 => Some(ConicType::Quadratic),
            4 => Some(ConicType::Quartic),
            5 => Some(ConicType::Quintic),
            _ => None,
        }
    }
}

/// Result of [`conic_data`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConicData {
    /// The end angle in degrees, moved into `(as, as + 360]`.
    pub ae: f64,
    /// Highest control point index.
    pub n: usize,
    /// Degree.
    pub p: usize,
    /// Highest knot index (`n + p + 1`).
    pub m: usize,
}

/// Index counts of a conic arc from `as_deg` to `ae_deg` degrees
/// (`N_CrvGetConicData`): the end angle is moved into `(as, as + 360]` (an arc
/// shorter than `1e-6` degrees becomes a full turn), then the number of segments
/// follows from the sweep and the conic type.
///
/// # Errors
///
/// `InvalidInput` for non-finite angles, or when the end angle needs more
/// than [`CONIC_MAX_TURNS`] turns (upstream loops forever when adding 360 no longer
/// changes it).
pub fn conic_data(ctp: ConicType, as_deg: f64, ae_deg: f64) -> Result<ConicData> {
    let angle_tol_deg = 1.0e-6; // Don't make this too small.
    if !as_deg.is_finite() || !ae_deg.is_finite() {
        return Err(invalid("nurbs::crv_basic::conic_data", "non-finite angle"));
    }
    let mut ae = ae_deg;

    // Get number of arcs.
    let mut turns = 0usize;
    while ae < (as_deg + angle_tol_deg) {
        ae += 360.0;
        turns += 1;
        if turns > CONIC_MAX_TURNS {
            return Err(invalid(
                "nurbs::crv_basic::conic_data",
                "end angle too far from the start angle",
            ));
        }
    }
    while (ae - as_deg) > 360.0 - angle_tol_deg {
        ae -= 360.0;
        turns += 1;
        if turns > CONIC_MAX_TURNS {
            return Err(invalid(
                "nurbs::crv_basic::conic_data",
                "end angle too far from the start angle",
            ));
        }
    }
    if (ae - as_deg) < angle_tol_deg {
        ae = as_deg + 360.0;
    }
    let theta = ae - as_deg;

    let (p, n) = match ctp {
        ConicType::Quadratic => {
            let n = if theta < 91.0 {
                2
            } else if theta < 181.0 {
                4
            } else if theta < 271.0 {
                6
            } else {
                8
            };
            (2, n)
        }
        ConicType::Quartic => {
            let n = if theta < 121.0 {
                4
            } else if theta < 241.0 {
                8
            } else {
                12
            };
            (4, n)
        }
        ConicType::Quintic => (5, 5),
    };
    Ok(ConicData {
        ae,
        n,
        p,
        m: n + p + 1,
    })
}

/// A boundary of a surface's parameter rectangle (`NL_LEFT = 1`: `u = umin`,
/// `NL_RIGHT = 2`: `u = umax`, `NL_BOTTOM = 3`: `v = vmin`, `NL_TOP = 4`:
/// `v = vmax`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Boundary {
    /// `NL_LEFT`: across `u = umin`.
    Left,
    /// `NL_RIGHT`: across `u = umax`.
    Right,
    /// `NL_BOTTOM`: across `v = vmin`.
    Bottom,
    /// `NL_TOP`: across `v = vmax`.
    Top,
}

impl Boundary {
    /// The upstream flag value.
    #[must_use]
    pub const fn flag(self) -> i32 {
        match self {
            Boundary::Left => 1,
            Boundary::Right => 2,
            Boundary::Bottom => 3,
            Boundary::Top => 4,
        }
    }

    /// The boundary for an upstream flag value, `None` for any other value.
    #[must_use]
    pub const fn from_flag(flag: i32) -> Option<Self> {
        match flag {
            1 => Some(Boundary::Left),
            2 => Some(Boundary::Right),
            3 => Some(Boundary::Bottom),
            4 => Some(Boundary::Top),
            _ => None,
        }
    }
}

/// The cross-boundary derivative curve of a non-rational surface (or of a rational
/// one in 4D) across `side` (`N_CrossBoundDerivCrvNonRatSrf`): across `u = umin`
/// it is `p / (U[p+1] - U[1]) * (Pw[1][j] - Pw[0][j])` over the v knots, and
/// likewise for the other sides. `cur` is sized with [`size_arrays`].
///
/// # Errors
///
/// [`NlibCode::Storage`] when an existing `cur` is too small, and
/// `InvalidInput` (before touching `cur`) when the surface has fewer than two rows
/// (`Left` / `Right`) or columns (`Bottom` / `Top`), or its knots in use do not
/// reach the indices read.
pub fn cross_bound_deriv_crv_non_rat_srf(
    sur: &Surface,
    side: Boundary,
    cur: &mut Option<Curve>,
) -> Result<()> {
    let oob = || {
        invalid(
            "nurbs::crv_basic::cross_bound_deriv_crv_non_rat_srf",
            "surface too small for the boundary",
        )
    };
    let (n, m, r, s) = sur.array_sizes();
    let (p, q) = sur.degrees();
    let us = sur.u_knots();
    let vs = sur.v_knots();
    let knot = |k: &[f64], i: Option<usize>| -> Result<f64> {
        i.and_then(|i| k.get(i)).copied().ok_or_else(oob)
    };

    match side {
        Boundary::Left | Boundary::Right => {
            let (i0, i1, fac) = if side == Boundary::Left {
                let den = knot(us, p.checked_add(1))? - knot(us, Some(1))?;
                (0, 1, p as f64 / den)
            } else {
                let n1 = n.checked_sub(1).ok_or_else(oob)?;
                let den = knot(us, n.checked_add(p))? - knot(us, Some(n))?;
                (n1, n, p as f64 / den)
            };
            let row0 = sur.row(i0).map_err(|_| oob())?;
            let row1 = sur.row(i1).map_err(|_| oob())?;
            let c = size_arrays(cur, m, q, s)?;
            for (cw, (a, b)) in c.cpts_mut().iter_mut().zip(row1.iter().zip(row0)) {
                *cw = CPoint::diff(*a, *b).scale(fac);
            }
            for (uc, v) in c.knots_mut().iter_mut().zip(vs) {
                *uc = *v;
            }
        }
        Boundary::Bottom | Boundary::Top => {
            let (j0, j1, fac) = if side == Boundary::Bottom {
                let den = knot(vs, q.checked_add(1))? - knot(vs, Some(1))?;
                if m < 1 {
                    return Err(oob());
                }
                (0, 1, q as f64 / den)
            } else {
                let m1 = m.checked_sub(1).ok_or_else(oob)?;
                let den = knot(vs, m.checked_add(q))? - knot(vs, Some(m))?;
                (m1, m, q as f64 / den)
            };
            let col: Vec<CPoint> = sur
                .rows()
                .map(|row| match (row.get(j1), row.get(j0)) {
                    (Some(a), Some(b)) => Ok(CPoint::diff(*a, *b).scale(fac)),
                    _ => Err(oob()),
                })
                .collect::<Result<_>>()?;
            let c = size_arrays(cur, n, p, r)?;
            for (cw, d) in c.cpts_mut().iter_mut().zip(col) {
                *cw = d;
            }
            for (uc, u) in c.knots_mut().iter_mut().zip(us) {
                *uc = *u;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_storage_until_written() {
        let a = line2();
        let mut b = a.clone();
        assert!(Arc::ptr_eq(&a.pw, &b.pw) && Arc::ptr_eq(&a.u, &b.u));
        b.compress();
        assert!(Arc::ptr_eq(&a.pw, &b.pw));
        b.knots_mut()[0] = -1.0;
        assert!(!Arc::ptr_eq(&a.u, &b.u) && Arc::ptr_eq(&a.pw, &b.pw));
        b.cpts_mut()[0] = CPoint::ZERO;
        assert!(!Arc::ptr_eq(&a.pw, &b.pw));
        assert_eq!(a, line2());
        assert_ne!(a, b);
    }

    fn line2() -> Curve {
        Curve::new(
            vec![
                CPoint::new(0.0, 0.0, 0.0, NOW),
                CPoint::new(1.0, 0.0, 0.0, NOW),
                CPoint::new(2.0, 0.0, 0.0, NOW),
            ],
            2,
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        )
        .unwrap()
    }

    fn arc() -> Curve {
        let w = std::f64::consts::FRAC_1_SQRT_2;
        Curve::new(
            vec![
                CPoint::new(1.0, 0.0, 0.0, 1.0),
                CPoint::new(w, w, 0.0, w),
                CPoint::new(0.0, 1.0, 0.0, 1.0),
            ],
            2,
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        )
        .unwrap()
    }

    #[test]
    fn size_arrays_allocates_zeroed_exact_storage() {
        for (n, p, m) in [(0, 0, 0), (2, 2, 5), (1, usize::MAX, 3)] {
            let mut slot = None;
            let curve = size_arrays(&mut slot, n, p, m).unwrap();
            assert_eq!(curve.array_sizes(), (n, m));
            assert_eq!(curve.degree(), p);
            assert_eq!(curve.storage(), (n + 1, m + 1));
            assert_eq!(curve.cpts(), vec![CPoint::ZERO; n + 1]);
            assert_eq!(curve.knots(), vec![0.0; m + 1]);
            assert!(slot.is_some());
        }
    }

    #[test]
    fn size_arrays_rejects_overflow_and_over_budget_without_inserting() {
        let oversized_n = limits::MAX_ALLOC_BYTES / size_of::<CPoint>();
        let oversized_m = limits::MAX_ALLOC_BYTES / size_of::<f64>();
        for (n, m) in [
            (usize::MAX, 0),
            (0, usize::MAX),
            (oversized_n, 0),
            (0, oversized_m),
        ] {
            let mut slot = None;
            assert!(matches!(
                size_arrays(&mut slot, n, 2, m),
                Err(SolidError::Nlib {
                    operation: "nurbs::crv_basic::size_arrays",
                    code: NlibCode::Memory,
                })
            ));
            assert!(slot.is_none());
        }
    }

    #[test]
    fn size_arrays_reuses_existing_storage_and_preserves_storage_errors() {
        let original = line2();
        let mut slot = Some(original.clone());
        let pw = slot.as_ref().unwrap().pw.as_ptr();
        let u = slot.as_ref().unwrap().u.as_ptr();
        let curve = size_arrays(&mut slot, 1, usize::MAX, 3).unwrap();
        assert_eq!(curve.array_sizes(), (1, 3));
        assert_eq!(curve.degree(), usize::MAX);
        assert_eq!(curve.storage(), original.storage());
        assert_eq!(curve.pw, original.pw);
        assert_eq!(curve.u, original.u);
        assert_eq!(curve.pw.as_ptr(), pw);
        assert_eq!(curve.u.as_ptr(), u);

        let unchanged = slot.clone();
        for (n, m) in [(2, 3), (1, 4), (usize::MAX, 3), (1, usize::MAX)] {
            assert!(matches!(
                size_arrays(&mut slot, n, 0, m),
                Err(SolidError::Nlib {
                    code: NlibCode::Storage,
                    ..
                })
            ));
            assert_eq!(slot, unchanged);
        }
    }

    #[test]
    fn accessors() {
        let c = line2();
        assert_eq!(c.array_sizes(), (2, 5));
        assert_eq!(c.degree(), 2);
        assert_eq!(c.param_bounds(), (0.0, 1.0));
        assert!(c.is_3d());
        assert!(!c.is_rational());
        assert!(c.is_sized(2, 5).is_ok());
        assert!(c.is_sized(3, 5).is_err());
        assert_eq!(c.knot_vector().unwrap().last_index(), 5);
    }

    #[test]
    fn predicates() {
        let c = line2();
        assert!(c.is_line(1e-6));
        assert!(!c.is_closed());
        assert!(!c.is_degen());
        assert!(c.is_in_z0_plane());
        let a = arc();
        assert!(a.is_rational());
        assert!(!a.is_line(1e-6));
        assert!(!a.are_weights_equal());
        assert!(a.weights_are_valid().is_ok());
        let pl = a.is_planar(1e-6).unwrap();
        assert!(pl.flat);
        let (pp, nn) = pl.plane.unwrap();
        assert!(pp.z.abs() < 1e-12);
        assert!((nn.z.abs() - 1.0).abs() < 1e-12);
        assert!(!line2().is_planar(1e-6).unwrap().flat);
    }

    #[test]
    fn conversions_and_copy() {
        let mut c = arc();
        let mut dst = None;
        c.copy_into(&mut dst).unwrap();
        assert_eq!(dst.as_ref().unwrap().cpts(), c.cpts());
        c.make_non_rat();
        assert!(!c.is_rational());
        c.prune_rat(true);
        assert!(!c.is_3d());
        let mut small = Some(Curve::new(vec![CPoint::ZERO], 0, vec![0.0]).unwrap());
        assert!(arc().copy_into(&mut small).is_err());
    }

    #[test]
    fn num_denom_round_trip() {
        let a = arc();
        let (mut num, mut den) = (None, None);
        a.num_and_denom(&mut num, &mut den).unwrap();
        let mut back = None;
        curve_from_num_and_denom(num.as_ref().unwrap(), den.as_ref(), &mut back).unwrap();
        assert_eq!(back.unwrap(), a);
    }

    #[test]
    fn reparam() {
        let mut c = line2();
        c.reparam_to_interval(Interval::new(2.0, 4.0)).unwrap();
        assert_eq!(c.knots(), &[2.0, 2.0, 2.0, 4.0, 4.0, 4.0]);
    }

    #[test]
    fn conic() {
        let d = conic_data(ConicType::Quadratic, 0.0, 90.0).unwrap();
        assert_eq!((d.n, d.p, d.m), (2, 2, 5));
        let d = conic_data(ConicType::Quadratic, 0.0, 0.0).unwrap();
        assert_eq!(d.ae, 360.0);
        assert_eq!(d.n, 8);
        assert!(conic_data(ConicType::Quartic, f64::NAN, 1.0).is_err());
    }

    #[test]
    fn compatible_and_coincident() {
        let mut a = line2();
        let mut b = line2();
        assert!(curves_are_compatible(&[a.clone(), b.clone()]));
        assert!(!curves_are_compatible(&[a.clone(), arc()]));
        assert!(curves_are_coincident(&mut a, &mut b, 1e-6).unwrap());
        let mut c = arc();
        let mut d = line2();
        assert!(!curves_are_coincident(&mut c, &mut d, 1e-6).unwrap());
    }
}
