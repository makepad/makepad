//! Implicit offset curves: a base curve moved a constant distance along the unit
//! binormal `C'(t) x N` of a fixed offset-plane normal `N` (`SmOffsetCurve`).
//!
//! Upstream: `source/SMLib/inc/SmOffsetCurve.h`, `source/SMLib/src/SmOffsetCurve.cpp`.
//!
//! | Rust                                         | Upstream                                         |
//! |----------------------------------------------|--------------------------------------------------|
//! | [`OffsetCurve`]                              | `SmOffsetCurve`                                  |
//! | [`OffsetCurve::new`]                         | `SmOffsetCurve(ULONG, const SmCurve&, const SmExtent1d&, const SmVector3d&, double, SmBoolean)` |
//! | [`OffsetCurve::empty`]                       | `SmOffsetCurve()` (the empty I/O constructor)    |
//! | [`OffsetCurve::copy`], `Clone`               | `SmOffsetCurve(const SmOffsetCurve&)`, `Copy`    |
//! | `Drop` (implicit)                            | `~SmOffsetCurve`                                 |
//! | [`OffsetCurve::approx_eq`]                   | `operator==`                                     |
//! | [`OffsetCurve::evaluate`]                    | `Evaluate`                                       |
//! | [`OffsetCurve::evaluate_point`]              | `EvaluatePoint`                                  |
//! | [`OffsetCurve::natural_interval`]            | `GetNaturalInterval`                             |
//! | [`OffsetCurve::knots`]                       | `GetKnots`                                       |
//! | [`OffsetCurve::number_natural_knots`]        | `GetNumberNaturalKnots`                          |
//! | [`OffsetCurve::calculate_continuities`]      | `CalculateContinuities`                          |
//! | [`OffsetCurve::set_context`]                 | `SetContext`                                     |
//! | [`OffsetCurve::reverse_parameterization`]    | `ReverseParameterization`                        |
//! | [`OffsetCurve::trim`]                        | `Trim`                                           |
//! | [`OffsetCurve::read_from_db`]                | `ReadFromDB`                                     |
//! | [`OffsetCurve::TYPE`], [`OffsetCurve::type_code`], [`OffsetCurve::type_string`], [`OffsetCurve::class_string`] | `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`OffsetCurve::is_kind_of`]                  | `IsKindOf`                                       |
//!
//! Deviations:
//! - **Nesting.** `new` returns `InvalidInput` when the base curve would nest
//!   deeper than [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING)
//!   (32; upstream fixtures nest at most 3), which bounds the recursive dispatch,
//!   cloning and dropping of nested geometry (no limit upstream).
//! - **Base curve.** Upstream keeps a pointer to the base curve, owned (deleted with
//!   the offset, deep-copied by the copy constructor) or shared (`bOwnsCurve =
//!   FALSE`). The port always owns its base curve by value ([`Box<Curve>`]): a
//!   shared base becomes a clone, which evaluates identically. `operator==`'s pointer
//!   test (`m_pCurve == rOther.m_pCurve`) therefore becomes the base curves'
//!   `operator==`. The ownership flag is kept ([`OffsetCurve::owns_curve`]) for
//!   callers that mirror upstream; the base curve's owner link back to the offset
//!   curve (`SetOwner(this)`) is not modelled (the offset curve has no object key).
//!   The copy constructor's failure path for a base curve without `Copy` (upstream
//!   then shares the pointer and deletes it twice) is a plain clone.
//! - **`Clone`.** `Clone` is the copy constructor: a borrowed base curve becomes an
//!   exact copy that keeps its raw knots and owner (upstream shares the pointer), an
//!   owned one is copied through [`Curve::clone`] and the ownership flag becomes
//!   `TRUE` (the empty curve's `UNSURE` included). Upstream's copy constructor is
//!   undefined for the empty curve (it dereferences the null base); the port keeps it
//!   a defined empty value instead.
//! - **Context and notifications.** `SmContext` arguments are dropped
//!   (`SetContext` is a no-op), as are the `SM_NO_PRE_EDIT`/`SM_NO_POST_EDIT`
//!   notifications of `Trim` and `ReverseParameterization`: the offset curve keeps
//!   no cache here. The base-class constructors (`SmCurve(ULONG)`, `SmCurve(const
//!   SmCurve&)`, unit `curve.curve_p5`) only set the dimension, kept in
//!   [`OffsetCurve::dim`].
//! - **Booleans.** `m_bIsReversed` is an `SmBoolean` (`int`): it is kept as the
//!   `i32` upstream stores (`0`/`1`, `UNSURE` = `2` in the empty constructor, or
//!   whatever `ReadFromDB` read), so `operator==` and the reversal toggle behave as
//!   upstream. An ASCII `ReadFromDB` whose boolean extraction is skipped (the stream
//!   already failed) leaves upstream's local uninitialised; the port uses `0`.
//! - **Out-params** become return values. `ReverseParameterization` ignores the
//!   status of `SetMinMax` as upstream; when it fails the new interval is the
//!   caller's untouched default ([`Extent1d::new`]). `Evaluate`'s recursive
//!   non-zero-tangent call ignores its status as upstream: a failure leaves the
//!   first derivative uninitialised ([`Vec3::UNDEF`]), except for
//!   [`SolidError::Unported`], which is always propagated.
//! - **`ReadFromDB`** always builds a new curve (upstream's "fill the curve passed
//!   in" branch has no counterpart; its type and dimension arguments are unused
//!   upstream and dropped). The base curve is read through
//!   [`Curve::read_from_db`].
//! - **Not ported here.** `WriteToDB`, `SetOffsetDistance`, `AssertValid`
//!   (unused upstream), `GetMemoryUsed`, `Dump`, `GetBaseCurve`, `GetRootCurve`,
//!   `GetOffsetDistance`, `GetOffsetNormal` and `GetMaxAnalyticDomain` are not part
//!   of this unit; plain getters are provided for the data.
//! - **Derivative order bound.** `Evaluate` returns [`SolidError::InvalidInput`] when
//!   `num_derivs` exceeds [`crate::curve::MAX_EVAL_DERIVS`] (upstream fills a
//!   caller-supplied array of any length; the port's output `Vec` must not be sized by
//!   an unbounded caller value).

use crate::curve::curve::p2::{is_kind_of as curve_is_kind_of, Curve, OFFSET_CURVE_TYPE};
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::tol::is_zero;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, UNDEF_DOUBLE};
use crate::surface::surface::p2::{check_geometry_nesting, NestedGeometry};
use crate::topology::database_io::{DatabaseReader, FileType, InputStream};

const OP_EVAL: &str = "curve::offset_curve::OffsetCurve::evaluate";
const OP_EVAL_POINT: &str = "curve::offset_curve::OffsetCurve::evaluate_point";
const OP_TRIM: &str = "curve::offset_curve::OffsetCurve::trim";
const OP_BASE: &str = "curve::offset_curve::OffsetCurve";

/// Derivatives [`OffsetCurve::evaluate`] computes (`clMaxDerivs`).
const MAX_DERIVS: usize = 3;

/// `UNSURE` (`SmTypes.h`), the initial `SmBoolean` members of the empty constructor.
const UNSURE: i32 = 2;

/// An implicit offset of a base curve (`SmOffsetCurve`):
///
/// ```text
/// O(t) = C(t) + D * B(t) / |B(t)|,   B(t) = C'(t) x N
/// ```
///
/// with `N` the unit offset-plane normal and `D` the offset distance (positive to
/// the right of the curve seen from above the offset plane). With the reversal flag
/// set, `t` is mirrored within the curve interval first.
#[derive(Debug)]
pub struct OffsetCurve {
    /// Image-space dimension (`SmCurve::m_lDim`).
    dim: u64,
    /// The base curve (`m_pCurve`); `None` only for [`OffsetCurve::empty`].
    base: Option<Box<Curve>>,
    /// Unit normal of the offset plane (`m_vOffsetPlaneNorm`).
    normal: Vec3,
    /// Offset distance (`m_dOffsetDistance`).
    distance: f64,
    /// Domain of the base curve used by the offset (`m_vCurveInterval`).
    interval: Extent1d,
    /// Parameterisation reversed (`m_bIsReversed`, an `SmBoolean`).
    reversed: i32,
    /// Whether upstream would own (delete) the base curve (`m_bOwnsCurve`).
    owns_curve: i32,
}

impl Clone for OffsetCurve {
    /// The copy constructor (`SmOffsetCurve(const SmOffsetCurve&)`): a borrowed base
    /// (`m_bOwnsCurve == FALSE`) is shared upstream, so here it is an exact copy that
    /// keeps its raw knots and owner (`Curve::clone` would fix the knots up); an owned
    /// one is copied through the base's copy constructor ([`Curve::clone`]) and the
    /// flag becomes `TRUE`, as upstream sets it after the copy. The exact helper is
    /// infallible here: [`OffsetCurve::new`] bounds the geometry nesting and the
    /// database reader bounds its recursion, so the copied base cannot exceed
    /// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING).
    ///
    /// The empty curve (no base, `UNSURE` ownership) stays a defined empty value: copying
    /// it upstream dereferences the null base (undefined), so it is not modelled. An
    /// owned base without a `Copy` (a Hermite curve) is left sharing the base upstream
    /// and deleted twice; the port clones it instead, as [`OffsetCurve::copy`] documents.
    fn clone(&self) -> Self {
        let base = match &self.base {
            Some(b) if self.owns_curve == 0 => Some(Box::new(b.clone_exact_bounded())),
            b => b.clone(),
        };
        Self {
            dim: self.dim,
            base,
            normal: self.normal,
            distance: self.distance,
            interval: self.interval,
            reversed: self.reversed,
            owns_curve: i32::from(self.owns_curve != 0),
        }
    }
}

impl OffsetCurve {
    /// Exact evaluation copy after the caller validates the geometry depth.
    pub(crate) fn clone_exact_bounded(&self) -> Self {
        Self {
            dim: self.dim,
            base: self
                .base
                .as_deref()
                .map(|c| Box::new(c.clone_exact_bounded())),
            normal: self.normal,
            distance: self.distance,
            interval: self.interval,
            reversed: self.reversed,
            owns_curve: self.owns_curve,
        }
    }

    /// An exact evaluation copy, retaining raw flags and the base curve's owner.
    /// `depth` is the base's depth in the shared `MAX_GEOMETRY_NESTING` (32) geometry traversal.
    pub(crate) fn clone_for_evaluation(&self, depth: usize) -> Result<Self> {
        Ok(Self {
            dim: self.dim,
            base: self
                .base
                .as_deref()
                .map(|c| c.clone_for_evaluation_depth(depth).map(Box::new))
                .transpose()?,
            normal: self.normal,
            distance: self.distance,
            interval: self.interval,
            reversed: self.reversed,
            owns_curve: self.owns_curve,
        })
    }

    /// `SmOffsetCurve_TYPE` (`CURV_BASE_TYPE + 36`).
    pub const TYPE: i64 = OFFSET_CURVE_TYPE;

    /// The offset of `base` over `interval` at `distance` in the plane with normal
    /// `normal` (unitised; a zero vector becomes `(0, 0, 1)` as upstream), of image
    /// dimension `dim`. `owns_curve` is upstream's `bOwnsCurve` (default `FALSE`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` when `base` nests deeper than
    /// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING) - 1.
    pub fn new(
        dim: u64,
        base: Curve,
        interval: Extent1d,
        normal: Vec3,
        distance: f64,
        owns_curve: bool,
    ) -> Result<Self> {
        check_geometry_nesting(NestedGeometry::Curve(&base), 1, OP_BASE)?;
        Ok(Self {
            dim,
            base: Some(Box::new(base)),
            // SE(m_vOffsetPlaneNorm.Unitize()): status ignored
            normal: normal.unitize_or_z(),
            distance,
            interval,
            reversed: 0,
            owns_curve: i32::from(owns_curve),
        })
    }

    /// The empty curve built for reading (`SmOffsetCurve()`): dimension 3, no base
    /// curve, uninitialised normal, distance `SM_UNDEF_DOUBLE`, an "inside out"
    /// interval and `UNSURE` flags.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            dim: 3,
            base: None,
            normal: Vec3::UNDEF,
            distance: UNDEF_DOUBLE,
            interval: Extent1d::new(),
            reversed: UNSURE,
            owns_curve: UNSURE,
        }
    }

    /// `Copy` (the copy constructor): an equal offset curve. An owned base curve
    /// (`TRUE` or `UNSURE`) is copied and owned by the copy; a borrowed one is shared
    /// upstream, so here it is an exact copy that keeps its owner (`Curve::clone`
    /// would fix its knots up).
    ///
    /// # Errors
    ///
    /// Those of [`Curve::clone_for_evaluation`] on a borrowed base curve (geometry
    /// nested too deeply).
    pub fn copy(&self) -> Result<Self> {
        let base = match &self.base {
            Some(b) if self.owns_curve == 0 => Some(Box::new(b.clone_for_evaluation()?)),
            b => b.clone(),
        };
        Ok(Self {
            dim: self.dim,
            base,
            normal: self.normal,
            distance: self.distance,
            interval: self.interval,
            reversed: self.reversed,
            owns_curve: i32::from(self.owns_curve != 0),
        })
    }

    /// Image-space dimension (`GetDim`).
    #[must_use]
    pub const fn dim(&self) -> u64 {
        self.dim
    }

    /// The base curve (`GetBaseCurve`), `None` for an empty curve.
    #[must_use]
    pub fn base_curve(&self) -> Option<&Curve> {
        self.base.as_deref()
    }

    /// The unit offset-plane normal (`GetOffsetNormal`).
    #[must_use]
    pub const fn offset_normal(&self) -> Vec3 {
        self.normal
    }

    /// The offset distance (`GetOffsetDistance`).
    #[must_use]
    pub const fn offset_distance(&self) -> f64 {
        self.distance
    }

    /// Whether the parameterisation is reversed (`m_bIsReversed != FALSE`).
    #[must_use]
    pub const fn is_reversed(&self) -> bool {
        self.reversed != 0
    }

    /// The raw `SmBoolean` value of `m_bIsReversed`.
    #[must_use]
    pub const fn reversed_flag(&self) -> i32 {
        self.reversed
    }

    /// The raw `SmBoolean` value of `m_bOwnsCurve`.
    #[must_use]
    pub const fn owns_curve(&self) -> i32 {
        self.owns_curve
    }

    /// The base curve, or `SM_ERR_NULL_POINTER` for an empty curve (upstream
    /// dereferences a null pointer).
    fn base(&self) -> Result<&Curve> {
        self.base
            .as_deref()
            .ok_or(SolidError::null_pointer(OP_BASE))
    }

    /// `operator==`: same dimension, equal base curves, normals and intervals, offset
    /// distances within the scaled zero and the same reversal flag.
    ///
    /// # Errors
    ///
    /// Those of [`Curve::approx_eq`] on the base curves.
    pub fn approx_eq(&self, k: &Kernel, other: &Self) -> Result<bool> {
        // low work
        if core::ptr::eq(self, other) {
            return Ok(true);
        }

        // first check the base (SmCurve::operator==: same type, same dimension)
        if self.dim != other.dim {
            return Ok(false);
        }

        // check equivalence of these objects
        let bases = match (&self.base, &other.base) {
            (None, None) => true,
            (Some(a), Some(b)) => a.approx_eq(k, b)?,
            _ => false,
        };
        Ok(bases
            && self.normal.approx_eq(other.normal)
            && is_zero(self.distance - other.distance)
            && self.interval.are_same(&other.interval)
            && self.reversed == other.reversed)
    }

    /// The natural interval (`GetNaturalInterval`): the curve interval.
    #[must_use]
    pub const fn natural_interval(&self) -> Extent1d {
        self.interval
    }

    /// The parameter on the base curve (mirrored within the interval when
    /// reversed).
    fn base_parameter(&self, t: f64) -> f64 {
        if self.reversed != 0 {
            self.interval.min() + self.interval.max() - t
        } else {
            t
        }
    }

    /// Position and derivatives `0..=num_derivs` at `t` (`Evaluate`). Up to three
    /// derivatives are computed, higher ones are zero. With `non_zero_tangents` a
    /// tiny first derivative is replaced by a tolerance-sized one along the second
    /// derivative.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the base curve's first derivative stays zero after a
    /// finite-difference retry; `SM_ERR_INVALID_INPUT` when the binormal is zero;
    /// `SM_ERR` from the unit binormal derivatives; those of the base curve's
    /// evaluator.
    pub fn evaluate(
        &self,
        k: &Kernel,
        input_t: f64,
        num_derivs: usize,
        from_left: bool,
        non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        // The output holds `num_derivs + 1` vectors: bound the caller's order.
        crate::curve::check_eval_derivs(num_derivs, OP_EVAL)?;
        let base = self.base()?;
        let mut from_left = from_left;

        // Adjust parameter if reversed: mirror within the interval.
        let mut t = self.base_parameter(input_t);

        // If offset dist is zero, just evaluate base curve (upstream does not return
        // here: the result is overwritten below).
        if self.distance.abs() < EFF_ZERO {
            base.evaluate(k, t, num_derivs, from_left, true)?;
        }

        // init output.
        let mut out = vec![Vec3::new(0.0, 0.0, 0.0); num_derivs + 1];

        // How many can we actually calculate: 3.
        let num_calc = num_derivs.min(MAX_DERIVS);

        // Eval base curve: one more deriv than we need.
        let mut pv = [Vec3::UNDEF; MAX_DERIVS + 2];
        if is_zero(t - self.interval.max()) {
            from_left = false;
        }
        let ev = base.evaluate(k, t, num_calc + 1, from_left, true)?;
        for (dst, src) in pv.iter_mut().zip(ev) {
            *dst = src;
        }

        // Check for zero 1st deriv.
        if pv[1].length_squared() < EFF_ZERO_SQ {
            let mut sign = 1.0;
            if t > self.interval.max() - EFF_ZERO_SQRT * self.interval.length() {
                sign = -1.0;
                t -= EFF_ZERO_SQRT * self.interval.length();
            } else {
                t += EFF_ZERO_SQRT * self.interval.length();
            }
            let mut pv2 = [Vec3::UNDEF; 4];
            let ev2 = base.evaluate(k, t, num_calc, from_left, true)?;
            for (dst, src) in pv2.iter_mut().zip(ev2) {
                *dst = src;
            }
            pv[1] = (pv2[0] - pv[0]) / (sign * EFF_ZERO_SQRT * self.interval.length());
            if num_calc > 1 {
                pv[2] = (pv2[1] - pv[1]) / (sign * EFF_ZERO_SQRT * self.interval.length());
            }
            if num_calc > 2 {
                pv[3] = (pv2[2] - pv[2]) / (sign * EFF_ZERO_SQRT * self.interval.length());
            }

            if pv[1].length_squared() < EFF_ZERO_SQ {
                return Err(SolidError::sm_err(OP_EVAL));
            }
        } // end zero-1st-deriv check.

        // Here we have a usable first derivative.

        // Offset curve is defined as:   O(t) = C(t) + D * UB(t)
        // where UB(t) is unitized binormal of C at t.

        // Position:
        let bin = pv[1].cross(self.normal); // Bin is right hand side of curve
        let ubin = bin.unitize()?;
        let p0 = pv[0] + ubin * self.distance;
        if let Some(o) = out.get_mut(0) {
            *o = p0;
        }

        if num_calc < 1 {
            return Ok(out);
        }

        // Derivatives: call only one of the UnitizedDerivative methods.
        let mut ubin_t = Vec3::UNDEF;
        let mut ubin_tt = Vec3::UNDEF;
        let mut ubin_ttt = Vec3::UNDEF;
        if num_calc == 1 {
            let bin_t = pv[2].cross(self.normal);
            ubin_t = bin.unitized_derivative(bin_t);
        } else if num_calc == 2 {
            let bin_t = pv[2].cross(self.normal);
            let bin_tt = pv[3].cross(self.normal);
            let [a, b] = bin
                .unitized_derivative2(bin_t, bin_tt)
                .map_err(|_| SolidError::sm_err(OP_EVAL))?;
            ubin_t = a;
            ubin_tt = b;
        } else if num_calc == 3 {
            let bin_t = pv[2].cross(self.normal);
            let bin_tt = pv[3].cross(self.normal);
            let bin_ttt = pv[4].cross(self.normal);
            let [a, b, c] = bin
                .unitized_derivative3(bin_t, bin_tt, bin_ttt)
                .map_err(|_| SolidError::sm_err(OP_EVAL))?;
            ubin_t = a;
            ubin_tt = b;
            ubin_ttt = c;
        }

        // 1st deriv.
        let mut d1 = pv[1] + self.distance * ubin_t;
        if self.reversed != 0 {
            d1 = -d1;
        }
        if let Some(o) = out.get_mut(1) {
            *o = d1;
        }

        // 2nd deriv.
        if num_calc >= 2 {
            if let Some(o) = out.get_mut(2) {
                *o = pv[2] + self.distance * ubin_tt;
            }
        }

        // 3rd deriv.
        if num_calc >= 3 {
            let mut d3 = pv[3] + self.distance * ubin_ttt;
            if self.reversed != 0 {
                d3 = -d3;
            }
            if let Some(o) = out.get_mut(3) {
                *o = d3;
            }
        }

        // When asked, try to find a direction for zero-length first derivatives based
        // on the 2nd derivative value.
        if non_zero_tangents {
            let scaled_zero = EFF_ZERO * (1.0 + p0.max_dimension());
            let new_len = 1.1 * scaled_zero;

            // see if the 1st order derivative is smaller than dNewLen
            let d1 = out.get(1).copied().unwrap_or(Vec3::UNDEF);
            if num_derivs >= 1 && d1.length_squared() < new_len * new_len {
                // make sure 2nd order derivatives are available
                if num_derivs < 2 {
                    // recurse back to this function so the next section sets the 1st
                    // derivative (status ignored upstream)
                    let vals = match self.evaluate(k, t, 2, from_left, true) {
                        Ok(v) => v,
                        Err(e @ SolidError::Unported { .. }) => return Err(e),
                        Err(_) => vec![Vec3::UNDEF; 3],
                    };

                    // save the 1st derivative
                    if let Some(o) = out.get_mut(1) {
                        *o = vals.get(1).copied().unwrap_or(Vec3::UNDEF);
                    }
                } else {
                    // fix the zero 1st derivative function here
                    let d2 = out.get(2).copied().unwrap_or(Vec3::UNDEF);
                    let len = d2.length();
                    if len > new_len {
                        let mut nd1 = (new_len / len) * d2;

                        // At the 'top' of the domain the degenerate derivative is
                        // shrinking, so it is opposite to the 2nd derivative.
                        if t > self.natural_interval().mid() {
                            nd1 *= -1.0;
                        }
                        if let Some(o) = out.get_mut(1) {
                            *o = nd1;
                        }
                    }
                }
            }
        }

        Ok(out)
    }

    /// The point at `t` (`EvaluatePoint`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the base curve's first derivative stays zero after a
    /// finite-difference retry or the binormal is zero; those of the base curve's
    /// evaluator.
    pub fn evaluate_point(&self, k: &Kernel, input_t: f64) -> Result<Point3> {
        let base = self.base()?;

        // Adjust parameter if reversed: mirror within the interval.
        let mut t = self.base_parameter(input_t);

        if self.distance.abs() < EFF_ZERO {
            return base.evaluate_point(k, t);
        }
        let mut pv = [Vec3::UNDEF; 2];
        let mut from_left = true;
        if is_zero(t - self.interval.max()) {
            from_left = false;
        }
        let ev = base.evaluate(k, t, 1, from_left, true)?;
        for (dst, src) in pv.iter_mut().zip(ev) {
            *dst = src;
        }
        if pv[1].length_squared() < EFF_ZERO_SQ {
            let mut sign = 1.0;
            if t > self.interval.max() - EFF_ZERO_SQRT * self.interval.length() {
                sign = -1.0;
                t -= EFF_ZERO_SQRT * self.interval.length();
            } else {
                t += EFF_ZERO_SQRT * self.interval.length();
            }
            let mut pv2 = [Vec3::UNDEF; 2];
            let ev2 = base.evaluate(k, t, 1, from_left, true)?;
            for (dst, src) in pv2.iter_mut().zip(ev2) {
                *dst = src;
            }
            pv[1] = (pv2[0] - pv[0]) / (sign * EFF_ZERO_SQRT * self.interval.length());
            if pv[1].length_squared() < EFF_ZERO_SQ {
                return Err(SolidError::sm_err(OP_EVAL_POINT));
            }
        }

        // Here we have a usable first derivative.
        let tan = pv[1];
        let bin = tan.cross(self.normal); // Bin is right hand side of curve
        if bin.length_squared() < EFF_ZERO_SQ {
            return Err(SolidError::sm_err(OP_EVAL_POINT));
        }
        let bin = bin.unitize_or_z();
        Ok(pv[0] + bin * self.distance)
    }

    /// The base curve's unique knots and multiplicities, optionally limited to
    /// `interval` (`GetKnots`).
    ///
    /// # Errors
    ///
    /// Those of the base curve's `GetKnots`.
    pub fn knots(&self, k: &Kernel, interval: Option<&Extent1d>) -> Result<(Vec<f64>, Vec<usize>)> {
        self.base()?.knots(k, interval)
    }

    /// The base curve's natural knot count (`GetNumberNaturalKnots`).
    ///
    /// # Errors
    ///
    /// Those of the base curve.
    pub fn number_natural_knots(&self, k: &Kernel) -> Result<usize> {
        self.base()?.number_natural_knots(k)
    }

    /// The base curve's continuities (`CalculateContinuities`).
    ///
    /// # Errors
    ///
    /// Those of the base curve.
    pub fn calculate_continuities(
        &self,
        k: &Kernel,
        angle_tol: f64,
    ) -> Result<(ContinuityType, Vec<ContinuityType>)> {
        self.base()?.calculate_continuities(k, angle_tol)
    }

    /// `SetContext`: contexts are not modelled, nothing to do.
    pub const fn set_context(&self) {}

    /// Reverses the parameterisation (`ReverseParameterization`) by toggling the
    /// reversal flag, and returns the image of `old_interval` (mirrored within the
    /// curve interval). The offset direction does not change.
    #[must_use]
    pub fn reverse_parameterization(&mut self, old_interval: &Extent1d) -> Extent1d {
        // Calculate the return argument.
        let inv = |v: f64| self.interval.inversion(v).parameter.unwrap_or(UNDEF_DOUBLE);
        let old_t_min = inv(old_interval.min());
        let old_t_max = inv(old_interval.max());

        let new_t_min = self.interval.evaluate(1.0 - old_t_max);
        let new_t_max = self.interval.evaluate(1.0 - old_t_min);

        // (status ignored upstream: the caller's interval stays as it was)
        let mut new_interval = Extent1d::new();
        let _ = new_interval.set_min_max(new_t_min, new_t_max);

        // Do the reversal.
        self.reversed = i32::from(self.reversed == 0);
        new_interval
    }

    /// Trims the curve to `trim_interval` (`Trim`): sets the curve interval (unless
    /// `interval_only`, upstream `bNotify == UNSURE`). The interval is not snapped.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the *current* curve interval is not inside the base curve's
    /// natural interval (within `EFF_ZERO`; upstream checks the old interval, not
    /// the new one).
    pub fn trim(&mut self, trim_interval: &Extent1d, interval_only: bool) -> Result<()> {
        // Check compatibility with base curve.
        let base_ivl = self.base()?.natural_interval();
        if !self.interval.is_contained_by(&base_ivl, EFF_ZERO) {
            return Err(SolidError::sm_err(OP_TRIM));
        }

        // Trim when asked
        if !interval_only {
            self.interval = *trim_interval;
        }
        Ok(())
    }

    /// Reads an offset curve from `db` (`ReadFromDB`): normal, distance, interval,
    /// reversal flag, then the base curve's type header and data. `version` is the
    /// database version passed to the base curve's reader.
    ///
    /// # Errors
    ///
    /// A short binary read; those of [`DatabaseReader::read_type`] and
    /// [`Curve::read_from_db`] for the base curve.
    pub fn read_from_db(k: &Kernel, db: &mut DatabaseReader, version: u64) -> Result<Self> {
        // init output object
        let mut c = Self::empty();

        // locals
        let mut normal = Vec3::UNDEF;
        let mut distance = 0.0;
        let mut ivl_min = 0.0;
        let mut ivl_max = 0.0;
        let mut reversed = 0;

        let ascii = db.file_type() == FileType::Ascii;
        if ascii {
            if let Some(s) = db.in_stream_mut() {
                for coord in [&mut normal.x, &mut normal.y, &mut normal.z] {
                    if let Some(x) = s.extract_double() {
                        *coord = x;
                    }
                }
            }
            db.go_to_next_line();
            if let Some(x) = db.in_stream_mut().and_then(InputStream::extract_double) {
                distance = x;
            }
            db.go_to_next_line();
            if let Some(s) = db.in_stream_mut() {
                for v in [&mut ivl_min, &mut ivl_max] {
                    if let Some(x) = s.extract_double() {
                        *v = x;
                    }
                }
            }
            db.go_to_next_line();
            if let Some(b) = db.in_stream_mut().and_then(InputStream::extract_int) {
                reversed = b;
            }
            db.go_to_next_line();
        } else {
            normal.x = db.read_double()?;
            normal.y = db.read_double()?;
            normal.z = db.read_double()?;
            distance = db.read_double()?;
            ivl_min = db.read_double()?;
            ivl_max = db.read_double()?;
            reversed = db.read_boolean()?;
        }

        // base curve
        if ascii {
            db.go_to_next_line();
        }
        let header = db.read_type(true)?;
        let base_type = header.type_code.unwrap_or(0);
        let base_dim = header.dim.unwrap_or(0);
        let base = Curve::read_from_db(k, base_type, db, base_dim, version)?;

        // load the obj
        c.base = Some(Box::new(base));
        c.normal = normal;
        c.distance = distance;
        // (status ignored upstream)
        let _ = c.interval.set_min_max(ivl_min, ivl_max);
        c.reversed = reversed;
        c.owns_curve = 1;
        Ok(c)
    }

    /// `SmOffsetCurve_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmOffsetCurve_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmOffsetCurve_TYPE"
    }

    /// `"SmOffsetCurve"` (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        "SmOffsetCurve"
    }

    /// True for `SmOffsetCurve_TYPE` and its base types (`IsKindOf`).
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == Self::TYPE || curve_is_kind_of(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::hermite_curve::HermiteCurve;

    fn line() -> Curve {
        Curve::from(HermiteCurve::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            3,
        ))
    }

    fn offset(d: f64) -> OffsetCurve {
        OffsetCurve::new(
            3,
            line(),
            Extent1d::from_min_max(0.0, 1.0),
            Vec3::new(0.0, 0.0, 2.0),
            d,
            true,
        )
        .unwrap()
    }

    #[test]
    fn huge_derivative_orders_are_rejected() {
        // fuzz: `n = 2^31` / `usize::MAX` sized the output allocation (abort)
        let c = offset(0.5);
        for n in [crate::curve::MAX_EVAL_DERIVS + 1, 1 << 31, usize::MAX] {
            assert!(matches!(
                c.evaluate(&Kernel::default(), 0.25, n, true, true),
                Err(SolidError::InvalidInput { .. })
            ));
        }
        let e = c
            .evaluate(
                &Kernel::default(),
                0.25,
                crate::curve::MAX_EVAL_DERIVS,
                true,
                true,
            )
            .unwrap();
        assert_eq!(e.len(), crate::curve::MAX_EVAL_DERIVS + 1);
    }

    #[test]
    fn offsets_a_line_to_the_right() {
        let k = &Kernel::default();
        let c = offset(0.5);
        assert_eq!(c.offset_normal(), Vec3::new(0.0, 0.0, 1.0));
        let p = c.evaluate_point(k, 0.25).unwrap();
        // x cross z = -y: right of the curve seen from +z
        assert!((p.y + 0.5).abs() < 1e-15 && (p.x - 0.25).abs() < 1e-15);
        let e = c.evaluate(k, 0.25, 5, true, true).unwrap();
        assert_eq!(e.len(), 6);
        assert!((e[0] - p).length() < 1e-15);
        assert!((e[1] - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-12);
        assert_eq!(e[4], Vec3::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn reverse_and_trim() {
        let k = &Kernel::default();
        let mut c = offset(0.5);
        let p = c.evaluate_point(k, 0.25).unwrap();
        let ivl = c.reverse_parameterization(&Extent1d::from_min_max(0.0, 0.5));
        assert_eq!((ivl.min(), ivl.max()), (0.5, 1.0));
        assert!(c.is_reversed());
        let q = c.evaluate_point(k, 0.75).unwrap();
        assert!((p - q).length() < 1e-15);
        let d = c.evaluate(k, 0.75, 1, true, true).unwrap();
        assert!(d[1].x < 0.0);
        c.trim(&Extent1d::from_min_max(0.2, 0.4), false).unwrap();
        assert_eq!(c.natural_interval(), Extent1d::from_min_max(0.2, 0.4));
        let mut bad = offset(0.5);
        bad.interval = Extent1d::from_min_max(-1.0, 1.0);
        assert!(bad.trim(&Extent1d::from_min_max(0.0, 1.0), false).is_err());
    }

    #[test]
    fn equality_copy_and_types() {
        let k = &Kernel::default();
        let a = offset(0.5);
        let b = a.copy().unwrap();
        assert!(a.approx_eq(k, &b).unwrap());
        assert!(!a.approx_eq(k, &offset(0.6)).unwrap());
        assert!(a.is_kind_of(OffsetCurve::TYPE));
        assert!(a.is_kind_of(crate::curve::curve::p2::TYPE));
        assert!(!a.is_kind_of(HermiteCurve::TYPE));
        assert_eq!(a.type_string(), "SmOffsetCurve_TYPE");
        assert_eq!(a.class_string(), "SmOffsetCurve");
        assert!(OffsetCurve::empty().evaluate_point(k, 0.0).is_err());
    }

    #[test]
    fn evolute_point_gets_a_tangent_from_the_second_derivative() {
        let k = &Kernel::default();
        // y = x^2 as a quadratic Bezier; at x = 0.5 the radius of curvature is sqrt(2)
        let mut cur = crate::nurbs::nurbs_crv::allocate_nurb_curve(2, 2, 5).unwrap();
        for (d, s) in cur
            .cpts_mut()
            .iter_mut()
            .zip([[-1.0, 1.0], [0.0, -1.0], [1.0, 1.0]])
        {
            d.x = s[0];
            d.y = s[1];
            d.z = 0.0;
            d.w = crate::nurbs::math::NOW;
        }
        cur.knots_mut()
            .copy_from_slice(&[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        let base = Curve::from(crate::nurbs::b_spline_curve::BSplineCurve::from_nurb(
            cur, 3, false,
        ));
        let c = OffsetCurve::new(
            3,
            base,
            Extent1d::from_min_max(0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
            -core::f64::consts::SQRT_2,
            false,
        )
        .unwrap();
        let exact = c.evaluate(k, 0.75, 2, true, false).unwrap();
        assert!(exact[1].length() < 1e-13);
        let fixed = c.evaluate(k, 0.75, 2, true, true).unwrap();
        let new_len = 1.1 * EFF_ZERO * (1.0 + fixed[0].max_dimension());
        assert!((fixed[1].length() - new_len).abs() < 1e-24);
        // past the middle the tangent is opposite to the 2nd derivative
        assert!(fixed[1].dot(fixed[2]) < 0.0);
        // one derivative: recursion with two
        let one = c.evaluate(k, 0.75, 1, true, true).unwrap();
        assert_eq!(one[1], fixed[1]);
    }

    #[test]
    fn parallel_normal_fails() {
        let k = &Kernel::default();
        let c = OffsetCurve::new(
            3,
            line(),
            Extent1d::from_min_max(0.0, 1.0),
            Vec3::new(1.0, 0.0, 0.0),
            0.5,
            false,
        )
        .unwrap();
        assert!(c.evaluate_point(k, 0.5).is_err());
        assert!(c.evaluate(k, 0.5, 2, true, true).is_err());
    }
}
