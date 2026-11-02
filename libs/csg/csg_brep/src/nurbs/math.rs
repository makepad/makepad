//! NLib arithmetic and linear algebra: homogeneous control points, point
//! combinations, the quadratic solver and floating-point range checks, real matrices
//! (multiply, transpose, LU decomposition with and without pivoting, forward/backward
//! substitution, inverse, least squares), power-basis surface evaluation and array
//! reallocation.
//!
//! Upstream: `source/SMLib/src/NL_Math.cpp`, `source/SMLib/src/NL_SrfPoly.cpp`,
//! `source/SMLib/src/NL_Util.cpp` (declarations in `source/SMLib/inc/NL_Math.h`,
//! `NL_SrfPoly.h`, `NL_Util.h`; `NL_CPOINT` in `NL_Geometry.h`, `NL_RMATRIX` in
//! `NL_DataStruct.h`, tolerances in `src/NL_Globals.cpp`).
//!
//! | Rust                                  | Upstream                         |
//! |---------------------------------------|----------------------------------|
//! | [`CPoint`]                            | `NL_CPOINT`                      |
//! | [`NOZ`] / [`NOW`]                     | `NL_NOZ` / `NL_NOW`              |
//! | [`CPoint::ZERO`]                      | `NL_CZERO`                       |
//! | [`CPoint::combine2`]                  | `N_Combine2CPts`                 |
//! | [`CPoint::translate_sum2`]            | `N_TranslateSum2CPts`            |
//! | [`CPoint::combine4`]                  | `N_Combine4CPts`                 |
//! | `Copy` of [`CPoint`] / [`Vec3`]       | `N_CopyCPt` / `N_CopyPt`         |
//! | [`CPoint::to_euclid`]                 | `N_CPtToPtEuclid`                |
//! | [`CPoint::to_euclid_and_w`]           | `N_CPtToPtAndW`                  |
//! | [`CPoint::weighted`]                  | `N_Weight`                       |
//! | [`CPoint::blend`]                     | `N_VectorBlendCPt`               |
//! | [`CPoint::scale`]                     | `N_ScaleCPt`                     |
//! | [`CPoint::scale_xyz`]                 | `N_ScaleCPtXYZ`                  |
//! | [`CPoint::transform`]                 | `N_TransformCPt`                 |
//! | [`CPoint::translate`]                 | `N_TranslateCPt`                 |
//! | [`CPoint::scale_about`]               | `N_ScaleCPtWithPtAndVector`      |
//! | [`CPoint::from_point`]                | `N_PtToCPt`                      |
//! | field `w` / `z`                       | `N_CPtGetW` / `N_CPtGetZ`        |
//! | [`CPoint::new`], [`CPoint::to_array`] | `N_CPtFromWxWyWz`, `N_CPtToWxWyWz` |
//! | fields `x`, `y`, `z`, `w`             | `N_CPtSetX/Y/Z/W`                |
//! | [`CPoint::sum`] / [`CPoint::diff`]    | `N_Sum2CPts` / `N_Diff2CPts`     |
//! | [`CPoint::dot_4d`]                    | `N_Dot2CPtsIn4D`                 |
//! | [`CPoint::magnitude`]                 | `N_CPtMagnitude`                 |
//! | [`CPoint::rat_deriv`]                 | `N_CPtRatDeriv`                  |
//! | [`CPoint::unweighted`]                | `N_CPtToPtNoW`                   |
//! | [`CPoint::drop_w`]                    | `N_CPtToPt`                      |
//! | [`CPoint::approx_eq`]                 | `N_CPtsAreEqual` (`NL_MTOL`)     |
//! | [`combine2_pts`] / [`combine4_pts`]   | `N_Combine2Pts` / `N_Combine4Pts` |
//! | [`translate_sum2_pts`]                | `N_TranslateSum2Pts`             |
//! | [`blend_pt`]                          | `N_VectorBlendPt`                |
//! | `Vec3 * f64`, `+`, `-`                | `N_ScalePt`, `N_Sum2Pts`, `N_Diff2Pts` |
//! | [`Vec3::dot`] / [`Vec3::length`]      | `N_Dot2Pts` / `N_PtMagnitude`    |
//! | [`Vec3::new`], [`Vec3::to_array`]     | `N_PtFromXYZ`, `N_PtToXYZ`       |
//! | [`solve_quadratic`], [`QuadraticRoots`] | `N_SolveQuadraticEq`           |
//! | [`float_op_is_bad`], [`FloatOp`]      | `N_FloatOpIsBad`                 |
//! | `core::mem::swap`                     | `N_SwapIntegers`, `N_SwapReals`, `N_SwapCPts` |
//! | [`pascal_triangle`]                   | `N_PascalTriRow`                 |
//! | `PASCAL_TRI`, `pascal_entry` (crate)  | `NL_PascalTri` (`NL_Globals.cpp`) |
//! | [`RealMatrix`], [`MatrixType`]        | `NL_RMATRIX`, `NL_MATRIXTYPE`    |
//! | [`RealMatrix::null`]                  | `N_InitRealMatrix`               |
//! | [`RealMatrix::from_rows`]             | `N_CreateRealMatrix`             |
//! | [`RealMatrix::define`]                | `N_RealMatrixDefine`             |
//! | [`RealMatrix::n`] etc., [`RealMatrix::get`] | `N_GetRealMatrixData`, `N_GetRealMatrixPtr` |
//! | [`RealMatrix::max_index`]             | `N_GetMaxIndexRealMatrix`        |
//! | [`RealMatrix::is_null`]               | `N_RealMatrixIsNULL`             |
//! | [`RealMatrix::new`]                   | `N_SetRealMatrix`                |
//! | [`RealMatrix::ensure`]                | `N_CheckMemRealMatrix`           |
//! | [`RealMatrix::check_storage`]         | `N_CheckRealMatStorage`          |
//! | [`RealMatrix::multiply_into`] / [`RealMatrix::multiply`] | `N_RealMatrixMultiply` |
//! | [`RealMatrix::transpose_into`], [`RealMatrix::transpose`], [`RealMatrix::transpose_in_place`] | `N_RealMatrixTranspose` |
//! | [`RealMatrix::multiply_points`]       | `N_RealMatrixMultiplyPtArray`    |
//! | [`RealMatrix::multiply_cpoints`]      | `N_RealMatrixMultiplyCPtArray`   |
//! | [`RealMatrix::lu_decompose`]          | `N_RealMatrixLuDecompose`        |
//! | [`RealMatrix::lu_decompose_pivot`]    | `N_RealMatrixLuDecomposePivot`   |
//! | [`RealMatrix::for_back_points`] / [`RealMatrix::for_back_cpoints`] | `N_RealMatrixForBack` (`NL_EPOINT` / `NL_HPOINT`) |
//! | [`RealMatrix::right_for_back`]        | `N_RealMatrixRightForBack`       |
//! | [`RealMatrix::for_back_pivot`]        | `N_RealMatrixForBackPivot`       |
//! | [`RealMatrix::inverse_into`] / [`RealMatrix::inverse`] | `N_RealMatrixInverse` |
//! | [`RealMatrix::lst_sq_solve`]          | `N_RealMatrixLstSqSolve`         |
//! | [`power_basis_srf_eval_pts`]          | `N_PowerBasisSrfEvalPts`         |
//! | [`srf_power_basis_eval_pt`]           | `N_SrfPowerBasisEvalPt`          |
//! | [`srf_power_basis_eval_derivs`]       | `N_SrfPowerBasisEvalDerivs`      |
//! | [`realloc_1d`]                        | `N_Realloc1dFlagArray`, `N_Realloc1dRealArray`, `N_Realloc1dPtArray`, `N_Realloc1dCPtArray` |
//! | [`realloc_2d`]                        | `N_Realloc2dRealArray`, `N_Realloc2dCPtArray` |
//! | (none: errors are values)             | `N_ErrClear`                     |
//! | [`LU_TOL`], [`ZERO_DISCRIMINANT_TOL`], [`ZERO_COEFFICIENT_TOL`], [`MODEL_TOL`] | `NL_LUDT`, `NL_ZDTL`, `NL_ZCTL`, `NL_MTOL` |
//! | [`MAX_DERIVATIVE`]                    | `NL_MAXDER`                      |
//!
//! Deviations:
//! - Out-params become return values. The matrix routines that write into a caller
//!   matrix (`N_RealMatrixMultiply`, `N_RealMatrixTranspose`, `N_RealMatrixInverse`)
//!   keep that form as `*_into(.., out: &mut RealMatrix)` because upstream reuses the
//!   output's storage when it is not null (and fails with `NL_STO_ERR` when it is too
//!   small); the plain forms start from [`RealMatrix::null`]. Transposing a matrix
//!   onto itself is [`RealMatrix::transpose_in_place`].
//! - Errors are [`SolidError::Nlib`] values carrying the upstream `NL_*_ERR` number
//!   instead of a flag plus the global `NL_ERROR` record, so `N_ErrClear` has no
//!   counterpart. On error the outputs upstream leaves partly written are not
//!   returned; matrices decomposed in place are left exactly as upstream leaves them.
//! - Every array and matrix access is bounds checked against the storage actually
//!   allocated. Where upstream would read or write outside a row of its storage
//!   (non-square triangular operands, banded matrices with an even bandwidth, power
//!   basis degrees whose scratch rows overflow the fixed `NL_MAXDEG + 1` arrays, too
//!   short point arrays, ...) the Rust functions return [`SolidError::InvalidInput`]
//!   instead of undefined behaviour. For every input upstream handles inside its
//!   arrays the results are the same, bit for bit (same operations in the same order).
//! - Storage that upstream allocates with `malloc` (and so leaves uninitialised) is
//!   zero-filled; upstream never reads those entries in the paths ported here.
//! - Operations on a matrix without storage ([`RealMatrix::null`]) return
//!   `InvalidInput` (upstream reads index `-1` bounds and a null pointer).
//! - `N_FloatOpIsBad` reads the global `NLIBCHKBYPASS`, which only `M_NoErrChk` sets
//!   and nothing calls; the port uses its default (checks enabled).
//! - `N_CPtsAreEqual` reads the global `NL_MTOL`, which `N_SrfIsClosedSmooth`
//!   overrides temporarily; [`CPoint::approx_eq`] takes the tolerance as an argument
//!   ([`MODEL_TOL`] is the default).
//! - `N_SrfPowerBasisEvalPt` and `N_SrfPowerBasisEvalDerivs` take an `NL_SURFACE`;
//!   the Rust functions take the control net (and degrees) directly. Upstream only
//!   reads the net and degrees (`N_SrfGetCPts`; the knot vectors fetched by
//!   `N_SrfGetKnotVectors` are unused), so no call into `nlib.srf_basic` remains.
//! - The `NL_STACKS` memory-stack arguments have no counterpart (Rust owns memory).

// Branches keep upstream's `if( x NEQ NL_NOZ ) ... else ...` order for traceability.
#![allow(clippy::if_not_else)]

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::knots::MAX_DEGREE;

/// `NL_NOZ`: the `z` value marking a 2D control point (`+DBL_MAX`).
pub const NOZ: f64 = f64::MAX;
/// `NL_NOW`: the `w` value marking a non-rational control point (`-DBL_MAX`).
pub const NOW: f64 = -f64::MAX;
/// `NL_MAXDER`: the largest derivative order NLib evaluates.
pub const MAX_DERIVATIVE: usize = 4;
/// `NL_LUDT`: LU-decomposition (pivot) tolerance.
pub const LU_TOL: f64 = 1.0e-09;
/// `NL_ZDTL`: zero-discriminant tolerance of [`solve_quadratic`].
pub const ZERO_DISCRIMINANT_TOL: f64 = 1.0e-08;
/// `NL_ZCTL`: zero-coefficient tolerance of [`solve_quadratic`].
pub const ZERO_COEFFICIENT_TOL: f64 = 1.0e-12;
/// `NL_MTOL`: model-space tolerance (default for [`CPoint::approx_eq`]).
pub const MODEL_TOL: f64 = 1.0e-07;

/// `NL_BIGD` (`DBL_MAX`).
pub(crate) const BIGD: f64 = f64::MAX;
/// `NL_SMAD` (`DBL_MIN`, the smallest positive normal double).
const SMAD: f64 = f64::MIN_POSITIVE;

const OP_TRANSFORM: &str = "nurbs::math::CPoint::transform";
const OP_QUADRATIC: &str = "nurbs::math::solve_quadratic";
const OP_PASCAL: &str = "nurbs::math::pascal_triangle";
const OP_MATRIX: &str = "nurbs::math::RealMatrix";
const OP_NEW: &str = "nurbs::math::RealMatrix::new";
const OP_FROM_ROWS: &str = "nurbs::math::RealMatrix::from_rows";
const OP_ENSURE: &str = "nurbs::math::RealMatrix::ensure";
const OP_CHECK_STORAGE: &str = "nurbs::math::RealMatrix::check_storage";
const OP_MULTIPLY: &str = "nurbs::math::RealMatrix::multiply";
const OP_TRANSPOSE: &str = "nurbs::math::RealMatrix::transpose";
const OP_MUL_PTS: &str = "nurbs::math::RealMatrix::multiply_points";
const OP_MUL_CPTS: &str = "nurbs::math::RealMatrix::multiply_cpoints";
const OP_LU: &str = "nurbs::math::RealMatrix::lu_decompose";
const OP_LU_PIVOT: &str = "nurbs::math::RealMatrix::lu_decompose_pivot";
const OP_FOR_BACK: &str = "nurbs::math::RealMatrix::for_back";
const OP_RIGHT_FOR_BACK: &str = "nurbs::math::RealMatrix::right_for_back";
const OP_FOR_BACK_PIVOT: &str = "nurbs::math::RealMatrix::for_back_pivot";
const OP_INVERSE: &str = "nurbs::math::RealMatrix::inverse";
const OP_LST_SQ: &str = "nurbs::math::RealMatrix::lst_sq_solve";
const OP_SRF_EVAL: &str = "nurbs::math::power_basis_srf_eval_pts";
const OP_SRF_DERIVS: &str = "nurbs::math::srf_power_basis_eval_derivs";
const OP_REALLOC: &str = "nurbs::math::realloc";

fn nlib(operation: &'static str, code: NlibCode) -> SolidError {
    SolidError::Nlib { operation, code }
}

fn out_of_range(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "index out of range (upstream would access outside its arrays)",
    }
}

/// `a[i]` (copied), or `InvalidInput` when out of range.
#[inline]
fn at<T: Copy>(a: &[T], i: usize, op: &'static str) -> Result<T> {
    a.get(i).copied().ok_or_else(|| out_of_range(op))
}

/// `&mut a[i]`, or `InvalidInput` when out of range.
#[inline]
fn at_mut<'a, T>(a: &'a mut [T], i: usize, op: &'static str) -> Result<&'a mut T> {
    a.get_mut(i).ok_or_else(|| out_of_range(op))
}

/// `a + b - c` on indices, or `InvalidInput` where upstream's signed index would go
/// negative.
#[inline]
fn band_index(a: usize, b: usize, c: usize, op: &'static str) -> Result<usize> {
    a.checked_add(b)
        .and_then(|s| s.checked_sub(c))
        .ok_or_else(|| out_of_range(op))
}

/// `len` copies of `value`, or `InvalidInput` instead of an allocation abort.
fn filled<T: Clone>(len: Option<usize>, value: T, op: &'static str) -> Result<Vec<T>> {
    let too_big = || SolidError::InvalidInput {
        operation: op,
        reason: "array size overflow",
    };
    let len = len.ok_or_else(too_big)?;
    let mut v = Vec::new();
    v.try_reserve_exact(len).map_err(|_| too_big())?;
    v.resize(len, value);
    Ok(v)
}

// ---- control points ---------------------------------------------------------------

/// A homogeneous control point `(wx, wy, wz, w)` (upstream `NL_CPOINT`).
///
/// `z == `[`NOZ`] marks a 2D point and `w == `[`NOW`] a non-rational one; the routines
/// below propagate those markers exactly as upstream does.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CPoint {
    /// Weighted x (`w * x`, or `x` when non-rational).
    pub x: f64,
    /// Weighted y.
    pub y: f64,
    /// Weighted z, or [`NOZ`].
    pub z: f64,
    /// Weight, or [`NOW`].
    pub w: f64,
}

impl CPoint {
    /// `NL_CZERO`: all four coordinates zero (a rational 3D point).
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0, 0.0);

    /// Control point from its coordinates (`N_CPtFromWxWyWz`).
    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64, w: f64) -> Self {
        Self { x, y, z, w }
    }

    /// `[x, y, z, w]` (`N_CPtToWxWyWz`).
    #[must_use]
    pub const fn to_array(self) -> [f64; 4] {
        [self.x, self.y, self.z, self.w]
    }

    /// Non-rational control point of a Euclidean point (`N_PtToCPt`).
    #[must_use]
    pub const fn from_point(p: Vec3) -> Self {
        Self::new(p.x, p.y, p.z, NOW)
    }

    /// Weighted control point `(w*P, w)`, or `(P, NOW)` when `w == NOW` (`N_Weight`).
    #[must_use]
    pub fn weighted(p: Vec3, w: f64) -> Self {
        if w == NOW {
            Self::new(p.x, p.y, p.z, NOW)
        } else {
            Self::new(w * p.x, w * p.y, w * p.z, w)
        }
    }

    /// `alpha*A + beta*B` (`N_Combine2CPts`); a missing `z` in one operand drops that
    /// term, a missing `w` in either makes the result non-rational.
    #[must_use]
    pub fn combine2(alpha: f64, a: Self, beta: f64, b: Self) -> Self {
        let z = if a.z != NOZ && b.z != NOZ {
            alpha * a.z + beta * b.z
        } else if a.z != NOZ {
            alpha * a.z
        } else if b.z != NOZ {
            beta * b.z
        } else {
            NOZ
        };
        let w = if a.w != NOW && b.w != NOW {
            alpha * a.w + beta * b.w
        } else {
            NOW
        };
        Self::new(alpha * a.x + beta * b.x, alpha * a.y + beta * b.y, z, w)
    }

    /// `C + alpha*A + beta*B` (`N_TranslateSum2CPts`).
    #[must_use]
    pub fn translate_sum2(c: Self, alpha: f64, a: Self, beta: f64, b: Self) -> Self {
        let z = if a.z != NOZ && b.z != NOZ && c.z != NOZ {
            c.z + alpha * a.z + beta * b.z
        } else {
            NOZ
        };
        let w = if a.w != NOW && b.w != NOW && c.w != NOW {
            c.w + alpha * a.w + beta * b.w
        } else {
            NOW
        };
        Self::new(
            c.x + alpha * a.x + beta * b.x,
            c.y + alpha * a.y + beta * b.y,
            z,
            w,
        )
    }

    /// `alf*A + bet*B + gam*C + del*D` (`N_Combine4CPts`).
    #[must_use]
    pub fn combine4(
        alf: f64,
        a: Self,
        bet: f64,
        b: Self,
        gam: f64,
        c: Self,
        del: f64,
        d: Self,
    ) -> Self {
        let z = if a.z != NOZ && b.z != NOZ && c.z != NOZ && d.z != NOZ {
            alf * a.z + bet * b.z + gam * c.z + del * d.z
        } else {
            NOZ
        };
        let w = if a.w != NOW && b.w != NOW && c.w != NOW && d.w != NOW {
            alf * a.w + bet * b.w + gam * c.w + del * d.w
        } else {
            NOW
        };
        Self::new(
            alf * a.x + bet * b.x + gam * c.x + del * d.x,
            alf * a.y + bet * b.y + gam * c.y + del * d.y,
            z,
            w,
        )
    }

    /// Euclidean point: divides by `w` unless non-rational; a missing `z` becomes `0`
    /// (`N_CPtToPtEuclid`; multiplies by the reciprocal of `w`, as upstream).
    #[must_use]
    pub fn to_euclid(self) -> Vec3 {
        if self.w != NOW {
            let w_reciprocal = 1.0 / self.w;
            let z = if self.z != NOZ {
                self.z * w_reciprocal
            } else {
                0.0
            };
            Vec3::new(self.x * w_reciprocal, self.y * w_reciprocal, z)
        } else {
            let z = if self.z != NOZ { self.z } else { 0.0 };
            Vec3::new(self.x, self.y, z)
        }
    }

    /// Euclidean point and weight (`N_CPtToPtAndW`).
    #[must_use]
    pub fn to_euclid_and_w(self) -> (Vec3, f64) {
        (self.to_euclid(), self.w)
    }

    /// `self + alpha*A` (`N_VectorBlendCPt`); `z`/`w` become missing when `A`'s are.
    pub fn blend(&mut self, alpha: f64, a: Self) {
        self.x += alpha * a.x;
        self.y += alpha * a.y;
        self.z = if a.z != NOZ {
            self.z + alpha * a.z
        } else {
            NOZ
        };
        self.w = if a.w != NOW {
            self.w + alpha * a.w
        } else {
            NOW
        };
    }

    /// `alpha * self`, keeping missing `z`/`w` (`N_ScaleCPt`).
    #[must_use]
    pub fn scale(self, alpha: f64) -> Self {
        let z = if self.z != NOZ { alpha * self.z } else { NOZ };
        let w = if self.w != NOW { alpha * self.w } else { NOW };
        Self::new(alpha * self.x, alpha * self.y, z, w)
    }

    /// Scales the first three coordinates only (`N_ScaleCPtXYZ`).
    #[must_use]
    pub fn scale_xyz(self, alpha: f64) -> Self {
        let z = if self.z != NOZ { alpha * self.z } else { NOZ };
        Self::new(alpha * self.x, alpha * self.y, z, self.w)
    }

    /// Applies a 4x4 matrix `M` from the left to the Euclidean point (with projective
    /// division by row 3) and re-weights the result (`N_TransformCPt`).
    ///
    /// Errors with `InvalidInput` when `m`'s storage is smaller than 4x4.
    pub fn transform(self, m: &RealMatrix) -> Result<Self> {
        let r = |i: usize, j: usize| m.get(i, j).map_err(|_| out_of_range(OP_TRANSFORM));
        let (r00, r01, r02, r03) = (r(0, 0)?, r(0, 1)?, r(0, 2)?, r(0, 3)?);
        let (r10, r11, r12, r13) = (r(1, 0)?, r(1, 1)?, r(1, 2)?, r(1, 3)?);
        let (r20, r21, r22, r23) = (r(2, 0)?, r(2, 1)?, r(2, 2)?, r(2, 3)?);
        let (r30, r31, r32, r33) = (r(3, 0)?, r(3, 1)?, r(3, 2)?, r(3, 3)?);
        let mut p = self;
        if p.w != NOW {
            p.x /= p.w;
            p.y /= p.w;
            if p.z != NOZ {
                p.z /= p.w;
            }
        }
        let mut q = if p.z != NOZ {
            let f = 1.0 / (p.x * r30 + p.y * r31 + p.z * r32 + r33);
            Self::new(
                f * (p.x * r00 + p.y * r01 + p.z * r02 + r03),
                f * (p.x * r10 + p.y * r11 + p.z * r12 + r13),
                f * (p.x * r20 + p.y * r21 + p.z * r22 + r23),
                0.0,
            )
        } else {
            let f = 1.0 / (p.x * r30 + p.y * r31 + r33);
            Self::new(
                f * (p.x * r00 + p.y * r01 + r03),
                f * (p.x * r10 + p.y * r11 + r13),
                NOZ,
                0.0,
            )
        };
        if p.w != NOW {
            q.x *= p.w;
            q.y *= p.w;
            if p.z != NOZ {
                q.z *= p.w;
            }
            q.w = p.w;
        } else {
            q.w = NOW;
        }
        Ok(q)
    }

    /// Translates the Euclidean point by `t` and re-weights it (`N_TranslateCPt`).
    #[must_use]
    pub fn translate(self, t: Vec3) -> Self {
        let mut p = self;
        if p.w != NOW {
            p.x /= p.w;
            p.y /= p.w;
            if p.z != NOZ {
                p.z /= p.w;
            }
        }
        let mut q = Self::new(
            p.x + t.x,
            p.y + t.y,
            if p.z != NOZ { p.z + t.z } else { NOZ },
            0.0,
        );
        if p.w != NOW {
            q.x *= p.w;
            q.y *= p.w;
            if p.z != NOZ {
                q.z *= p.w;
            }
            q.w = p.w;
        } else {
            q.w = NOW;
        }
        q
    }

    /// Scales by the per-axis factors `f` about the point `c`
    /// (`N_ScaleCPtWithPtAndVector`).
    #[must_use]
    pub fn scale_about(self, c: Vec3, f: Vec3) -> Self {
        let g = Vec3::new(1.0 - f.x, 1.0 - f.y, 1.0 - f.z);
        let w = if self.w != NOW { self.w } else { 1.0 };
        let cw = Vec3::new(w * c.x, w * c.y, w * c.z);
        let z = if self.z != NOZ {
            f.z * self.z + g.z * cw.z
        } else {
            NOZ
        };
        let qw = if self.w != NOW { self.w } else { NOW };
        Self::new(f.x * self.x + g.x * cw.x, f.y * self.y + g.y * cw.y, z, qw)
    }

    /// `A + B` (`N_Sum2CPts`); a missing `z` in one operand is ignored, a missing `w`
    /// in either makes the result non-rational.
    #[must_use]
    pub fn sum(a: Self, b: Self) -> Self {
        let z = if a.z != NOZ && b.z != NOZ {
            a.z + b.z
        } else if a.z != NOZ {
            a.z
        } else if b.z != NOZ {
            b.z
        } else {
            NOZ
        };
        let w = if a.w != NOW && b.w != NOW {
            a.w + b.w
        } else {
            NOW
        };
        Self::new(a.x + b.x, a.y + b.y, z, w)
    }

    /// `A - B` (`N_Diff2CPts`), with the same `z`/`w` rules as [`CPoint::sum`].
    #[must_use]
    pub fn diff(a: Self, b: Self) -> Self {
        let z = if a.z != NOZ && b.z != NOZ {
            a.z - b.z
        } else if a.z != NOZ {
            a.z
        } else if b.z != NOZ {
            -b.z
        } else {
            NOZ
        };
        let w = if a.w != NOW && b.w != NOW {
            a.w - b.w
        } else {
            NOW
        };
        Self::new(a.x - b.x, a.y - b.y, z, w)
    }

    /// Dot product in homogeneous space (`N_Dot2CPtsIn4D`). A weight present in only
    /// one operand is added as is.
    #[must_use]
    pub fn dot_4d(a: Self, b: Self) -> f64 {
        let mut dw = a.x * b.x + a.y * b.y;
        if a.z != NOZ && b.z != NOZ {
            dw += a.z * b.z;
        }
        if a.w != NOW && b.w != NOW {
            dw += a.w * b.w;
        } else if a.w != NOW {
            dw += a.w;
        } else if b.w != NOW {
            dw += b.w;
        }
        dw
    }

    /// `sqrt(dot_4d(self, self))` (`N_CPtMagnitude`).
    #[must_use]
    pub fn magnitude(self) -> f64 {
        Self::dot_4d(self, self).sqrt()
    }

    /// Homogeneous derivative `(wd*P + w*D, wd)` from the point `pw`, the Euclidean
    /// derivative `d` and the weight derivative `wd` (`N_CPtRatDeriv`).
    #[must_use]
    pub fn rat_deriv(pw: Self, d: Vec3, wd: f64) -> Self {
        let p = pw.to_euclid();
        let w = pw.w;
        let z = if pw.z != NOZ { wd * p.z + d.z * w } else { NOZ };
        Self::new(wd * p.x + d.x * w, wd * p.y + d.y * w, z, wd)
    }

    /// Euclidean control point `(x, y, z, NOW)` (`N_CPtToPtNoW`).
    #[must_use]
    pub fn unweighted(self) -> Self {
        if self.w != NOW {
            let w_reciprocal = 1.0 / self.w;
            let z = if self.z != NOZ {
                self.z * w_reciprocal
            } else {
                NOZ
            };
            Self::new(self.x * w_reciprocal, self.y * w_reciprocal, z, NOW)
        } else {
            let z = if self.z != NOZ { self.z } else { NOZ };
            Self::new(self.x, self.y, z, self.w)
        }
    }

    /// Drops the weight without dividing; a missing `z` becomes `0` (`N_CPtToPt`).
    #[must_use]
    pub fn drop_w(self) -> Vec3 {
        Vec3::new(self.x, self.y, if self.z != NOZ { self.z } else { 0.0 })
    }

    /// True when `|self - other| < tol` in homogeneous space (`N_CPtsAreEqual` with
    /// `NL_MTOL = tol`; upstream's default is [`MODEL_TOL`]).
    #[must_use]
    pub fn approx_eq(self, other: Self, tol: f64) -> bool {
        Self::diff(self, other).magnitude() < tol
    }
}

/// `alpha*A + beta*B` (`N_Combine2Pts`).
#[must_use]
pub fn combine2_pts(alpha: f64, a: Vec3, beta: f64, b: Vec3) -> Vec3 {
    Vec3::new(
        alpha * a.x + beta * b.x,
        alpha * a.y + beta * b.y,
        alpha * a.z + beta * b.z,
    )
}

/// `C + alpha*A + beta*B` (`N_TranslateSum2Pts`).
#[must_use]
pub fn translate_sum2_pts(c: Vec3, alpha: f64, a: Vec3, beta: f64, b: Vec3) -> Vec3 {
    Vec3::new(
        c.x + alpha * a.x + beta * b.x,
        c.y + alpha * a.y + beta * b.y,
        c.z + alpha * a.z + beta * b.z,
    )
}

/// `alf*A + bet*B + gam*C + del*D` (`N_Combine4Pts`).
#[must_use]
pub fn combine4_pts(
    alf: f64,
    a: Vec3,
    bet: f64,
    b: Vec3,
    gam: f64,
    c: Vec3,
    del: f64,
    d: Vec3,
) -> Vec3 {
    Vec3::new(
        alf * a.x + bet * b.x + gam * c.x + del * d.x,
        alf * a.y + bet * b.y + gam * c.y + del * d.y,
        alf * a.z + bet * b.z + gam * c.z + del * d.z,
    )
}

/// `B += alpha*A` (`N_VectorBlendPt`).
pub fn blend_pt(b: &mut Vec3, alpha: f64, a: Vec3) {
    b.x += alpha * a.x;
    b.y += alpha * a.y;
    b.z += alpha * a.z;
}

// ---- scalar math ------------------------------------------------------------------

/// Real roots of `a*x^2 + b*x + c = 0` (`N_SolveQuadraticEq`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QuadraticRoots {
    /// No real root (or the degenerate `a = b = 0`); upstream `n = 0`.
    None,
    /// One root (upstream `n = 1`, `r1 = r2`).
    One(f64),
    /// Two roots `r1 <= r2` (upstream `n = 2`).
    Two(f64, f64),
}

impl QuadraticRoots {
    /// Number of roots (upstream `n`).
    #[must_use]
    pub fn count(self) -> usize {
        match self {
            Self::None => 0,
            Self::One(_) => 1,
            Self::Two(..) => 2,
        }
    }
}

/// Solves `a*x^2 + b*x + c = 0` (`N_SolveQuadraticEq`).
///
/// Coefficients below [`ZERO_COEFFICIENT_TOL`] and discriminants within
/// [`ZERO_DISCRIMINANT_TOL`] of zero are treated as zero, exactly as upstream; two
/// roots use `q = -(b + sgn(b)*sqrt(disc))/2`, `x1 = q/a`, `x2 = c/q`. Fails with
/// `NL_NUM_ERR` when a division would overflow or underflow ([`float_op_is_bad`]).
pub fn solve_quadratic(a: f64, b: f64, c: f64) -> Result<QuadraticRoots> {
    let num_err = || nlib(OP_QUADRATIC, NlibCode::Numerical);
    let ordered = |r1: f64, r2: f64| {
        if r1 > r2 {
            QuadraticRoots::Two(r2, r1)
        } else {
            QuadraticRoots::Two(r1, r2)
        }
    };
    // a=b=0 --> c=0
    if a.abs() < ZERO_COEFFICIENT_TOL && b.abs() < ZERO_COEFFICIENT_TOL {
        return Ok(QuadraticRoots::None);
    }
    // a=c=0 --> b*x=0
    if a.abs() < ZERO_COEFFICIENT_TOL && c.abs() < ZERO_COEFFICIENT_TOL {
        return Ok(QuadraticRoots::One(0.0));
    }
    // b=c=0 --> a*x*x=0
    if b.abs() < ZERO_COEFFICIENT_TOL && c.abs() < ZERO_COEFFICIENT_TOL {
        return Ok(QuadraticRoots::One(0.0));
    }
    // a=0 --> b*x+c=0
    if a.abs() < ZERO_COEFFICIENT_TOL {
        if float_op_is_bad(-c, b, FloatOp::Division) {
            return Err(num_err());
        }
        return Ok(QuadraticRoots::One(-c / b));
    }
    // b=0 --> a*x*x+c=0
    if b.abs() < ZERO_COEFFICIENT_TOL {
        if float_op_is_bad(-c, a, FloatOp::Division) {
            return Err(num_err());
        }
        let dis = -c / a;
        if dis < 0.0 {
            return Ok(QuadraticRoots::None);
        }
        let dis = dis.sqrt();
        return Ok(QuadraticRoots::Two(-dis, dis));
    }
    // c=0 --> a*x*x+b*x=0
    if c.abs() < ZERO_COEFFICIENT_TOL {
        if float_op_is_bad(-b, a, FloatOp::Division) {
            return Err(num_err());
        }
        return Ok(ordered(0.0, -b / a));
    }
    // Regular case.
    let dis = b * b - 4.0 * a * c;
    if dis < -ZERO_DISCRIMINANT_TOL {
        return Ok(QuadraticRoots::None);
    }
    if dis.abs() < ZERO_DISCRIMINANT_TOL {
        if float_op_is_bad(-b, 2.0 * a, FloatOp::Division) {
            return Err(num_err());
        }
        return Ok(QuadraticRoots::One(-b / (2.0 * a)));
    }
    let dis = dis.sqrt();
    let q = -0.5 * (b + sign(b) * dis);
    if float_op_is_bad(q, a, FloatOp::Division) {
        return Err(num_err());
    }
    if float_op_is_bad(c, q, FloatOp::Division) {
        return Err(num_err());
    }
    Ok(ordered(q / a, c / q))
}

/// `NL_SIGN(x)`: `1.0` for `x > 0`, else `-1.0` (including `0` and NaN).
fn sign(x: f64) -> f64 {
    if x > 0.0 {
        1.0
    } else {
        -1.0
    }
}

/// The arithmetic operation checked by [`float_op_is_bad`] (`NL_ADDITION`,
/// `NL_SUBTRACTION`, `NL_MULTIPLICATION`, `NL_DIVISION`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FloatOp {
    /// `x + y` (`NL_ADDITION` = 1).
    Addition,
    /// `x - y` (`NL_SUBTRACTION` = 2).
    Subtraction,
    /// `x * y` (`NL_MULTIPLICATION` = 3).
    Multiplication,
    /// `x / y` (`NL_DIVISION` = 4).
    Division,
}

impl FloatOp {
    /// The upstream flag value.
    #[must_use]
    pub fn flag(self) -> i32 {
        match self {
            Self::Addition => 1,
            Self::Subtraction => 2,
            Self::Multiplication => 3,
            Self::Division => 4,
        }
    }
}

/// True when `x op y` would overflow (or, for products and quotients, underflow
/// below the smallest normal double) (`N_FloatOpIsBad`). Upstream's comparison
/// sequence is kept, so NaN inputs give upstream's answers.
#[must_use]
pub fn float_op_is_bad(x: f64, y: f64, op: FloatOp) -> bool {
    let (mut x, mut y) = (x, y);
    match op {
        FloatOp::Multiplication => {
            if x == 0.0 || y == 0.0 {
                return false;
            }
            if x < 0.0 {
                x = -x;
            }
            if y < 0.0 {
                y = -y;
            }
            // Opposite sides of 1.0 cannot overflow or underflow.
            if x >= 1.0 && y <= 1.0 {
                return false;
            }
            if x <= 1.0 && y >= 1.0 {
                return false;
            }
            if x >= 1.0 {
                if y <= BIGD / x {
                    return false;
                }
                if x <= BIGD / y {
                    return false;
                }
            } else if x <= 1.0 && x > SMAD / y {
                return false;
            }
            true
        }
        FloatOp::Division => {
            if y == 0.0 {
                return true;
            }
            if x == 0.0 {
                return false;
            }
            if x < 0.0 {
                x = -x;
            }
            if y < 0.0 {
                y = -y;
            }
            if y <= 1.0 && x < BIGD * y {
                return false;
            }
            if y >= 1.0 && x > SMAD * y {
                return false;
            }
            true
        }
        FloatOp::Addition => {
            if sign(x) != sign(y) {
                return false;
            }
            if x < 0.0 {
                x = -x;
            }
            if y < 0.0 {
                y = -y;
            }
            // Overflow check.
            if y < BIGD - x {
                return false;
            }
            true
        }
        FloatOp::Subtraction => {
            if sign(x) * sign(y) > 0.0 {
                return false;
            }
            if x < 0.0 {
                x = -x;
            }
            if y > 0.0 {
                y = -y;
            }
            if y > x - BIGD {
                return false;
            }
            true
        }
    }
}

/// Rows `0..=MAX_DEGREE` of Pascal's triangle as a constant table (upstream's static
/// `NL_INTEGER NL_PascalTri[18][18]`, `NL_Globals.cpp`, written out the same way);
/// entries right of the diagonal are zero. Equal to [`pascal_triangle`]`(MAX_DEGREE)`
/// (checked by a unit test), without the per-call allocation.
#[rustfmt::skip]
pub(crate) const PASCAL_TRI: [[i64; MAX_DEGREE + 1]; MAX_DEGREE + 1] = [
    [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 3, 3, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 4, 6, 4, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 5, 10, 10, 5, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 6, 15, 20, 15, 6, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 7, 21, 35, 35, 21, 7, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 8, 28, 56, 70, 56, 28, 8, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 9, 36, 84, 126, 126, 84, 36, 9, 1, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 10, 45, 120, 210, 252, 210, 120, 45, 10, 1, 0, 0, 0, 0, 0, 0, 0],
    [1, 11, 55, 165, 330, 462, 462, 330, 165, 55, 11, 1, 0, 0, 0, 0, 0, 0],
    [1, 12, 66, 220, 495, 792, 924, 792, 495, 220, 66, 12, 1, 0, 0, 0, 0, 0],
    [1, 13, 78, 286, 715, 1287, 1716, 1716, 1287, 715, 286, 78, 13, 1, 0, 0, 0, 0],
    [1, 14, 91, 364, 1001, 2002, 3003, 3432, 3003, 2002, 1001, 364, 91, 14, 1, 0, 0, 0],
    [1, 15, 105, 455, 1365, 3003, 5005, 6435, 6435, 5005, 3003, 1365, 455, 105, 15, 1, 0, 0],
    [1, 16, 120, 560, 1820, 4368, 8008, 11440, 12870, 11440, 8008, 4368, 1820, 560, 120, 16, 1, 0],
    [1, 17, 136, 680, 2380, 6188, 12376, 19448, 24310, 24310, 19448, 12376, 6188, 2380, 680, 136, 17, 1],
];

/// `NL_PascalTri[k][j]` as `f64`; `None` outside the triangle (`j > k`) or the table.
#[inline]
pub(crate) fn pascal_entry(k: usize, j: usize) -> Option<f64> {
    if j > k {
        return None;
    }
    PASCAL_TRI
        .get(k)
        .and_then(|row| row.get(j))
        .map(|&b| b as f64)
}

/// Rows `0..=row` of Pascal's triangle; row `k` has `k + 1` entries
/// (`N_PascalTriRow`, same symmetric fill).
///
/// Errors with `InvalidInput` when an entry overflows `i64` (row > 66), where
/// upstream's signed `long` arithmetic would overflow.
pub fn pascal_triangle(row: usize) -> Result<Vec<Vec<i64>>> {
    let mut tri: Vec<Vec<i64>> = vec![vec![1]];
    if row == 0 {
        return Ok(tri);
    }
    tri.push(vec![1, 1]);
    if row == 1 {
        return Ok(tri);
    }
    for k in 2..=row {
        let prev = tri.last().ok_or_else(|| out_of_range(OP_PASCAL))?;
        let mut cur = filled(k.checked_add(1), 0i64, OP_PASCAL)?;
        *at_mut(&mut cur, 0, OP_PASCAL)? = 1;
        let r = k / 2;
        let mut tmp2: i64 = 1;
        for j in 1..=r {
            let tmp1 = at(prev, j, OP_PASCAL)?;
            let v = tmp1.checked_add(tmp2).ok_or(SolidError::InvalidInput {
                operation: OP_PASCAL,
                reason: "integer overflow",
            })?;
            *at_mut(&mut cur, j, OP_PASCAL)? = v;
            *at_mut(&mut cur, k - j, OP_PASCAL)? = v;
            tmp2 = tmp1;
        }
        *at_mut(&mut cur, k, OP_PASCAL)? = 1;
        tri.push(cur);
    }
    Ok(tri)
}

// ---- real matrices ----------------------------------------------------------------

/// Storage layout of a [`RealMatrix`] (upstream `NL_MATRIXTYPE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MatrixType {
    /// `NL_MT_FULL` (0): every entry stored.
    #[default]
    Full,
    /// `NL_MT_LOWERLEFT` (1): entries with `j <= i` are meaningful.
    LowerLeft,
    /// `NL_MT_UPPERRIGHT` (2): entries with `j >= i` are meaningful.
    UpperRight,
    /// `NL_MT_BANDED` (3): row `i` stores `A[i][j]` at column `j - i + bw/2`.
    Banded,
}

impl MatrixType {
    /// The upstream enum value.
    #[must_use]
    pub fn code(self) -> i32 {
        match self {
            Self::Full => 0,
            Self::LowerLeft => 1,
            Self::UpperRight => 2,
            Self::Banded => 3,
        }
    }

    /// The type with upstream value `code`, if any.
    #[must_use]
    pub fn from_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::Full),
            1 => Some(Self::LowerLeft),
            2 => Some(Self::UpperRight),
            3 => Some(Self::Banded),
            _ => None,
        }
    }
}

/// Row-major element storage of a [`RealMatrix`] (upstream's `RM` row pointers into
/// one `(rows) x (cols)` block).
#[derive(Debug, Clone, PartialEq)]
struct Storage {
    rows: usize,
    cols: usize,
    data: Vec<f64>,
}

impl Storage {
    fn zeros(rows: usize, cols: usize, op: &'static str) -> Result<Self> {
        let data = filled(rows.checked_mul(cols), 0.0, op)?;
        Ok(Self { rows, cols, data })
    }

    fn index(&self, i: usize, j: usize) -> Option<usize> {
        if i < self.rows && j < self.cols {
            Some(i * self.cols + j)
        } else {
            None
        }
    }
}

/// A real matrix with highest row index `n` and highest column index `m` (upstream
/// `NL_RMATRIX`).
///
/// Elements live in a storage block whose shape is fixed when it is allocated;
/// [`RealMatrix::define`] and [`RealMatrix::ensure`] may change the logical shape
/// within it, as upstream does. Banded matrices store `A[i][j]` at row `i`, column
/// `j - i + bw/2` of a `(n+1) x bw` block.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RealMatrix {
    n: usize,
    m: usize,
    mtp: MatrixType,
    bw: usize,
    storage: Option<Storage>,
}

/// Number of storage columns upstream allocates for a matrix (`N_SetRealMatrix`).
fn storage_cols(m: usize, mtp: MatrixType, bw: usize) -> Option<usize> {
    if mtp == MatrixType::Banded {
        // N_AllocReal2dArray(n, bw) for bw == 1, else N_AllocReal2dArray(n, bw - 1).
        Some(if bw == 1 { 2 } else { bw })
    } else {
        m.checked_add(1)
    }
}

impl RealMatrix {
    /// The null matrix: no storage (`N_InitRealMatrix`).
    #[must_use]
    pub fn null() -> Self {
        Self::default()
    }

    /// A zero-filled matrix with highest indexes `n`, `m` (`N_SetRealMatrix`): the
    /// storage is `(n+1) x (m+1)`, or `(n+1) x bw` for banded matrices (`(n+1) x 2`
    /// when `bw == 1`).
    pub fn new(n: usize, m: usize, mtp: MatrixType, bw: usize) -> Result<Self> {
        let too_big = || SolidError::InvalidInput {
            operation: OP_NEW,
            reason: "array size overflow",
        };
        let rows = n.checked_add(1).ok_or_else(too_big)?;
        let cols = storage_cols(m, mtp, bw).ok_or_else(too_big)?;
        Ok(Self {
            n,
            m,
            mtp,
            bw,
            storage: Some(Storage::zeros(rows, cols, OP_NEW)?),
        })
    }

    /// A matrix over caller storage `rows` (each row the same length)
    /// (`N_CreateRealMatrix`). The logical shape `n`, `m` is independent of the
    /// storage shape, as upstream.
    pub fn from_rows(
        n: usize,
        m: usize,
        rows: Vec<Vec<f64>>,
        mtp: MatrixType,
        bw: usize,
    ) -> Result<Self> {
        let cols = rows.first().map_or(0, Vec::len);
        if rows.iter().any(|r| r.len() != cols) {
            return Err(SolidError::InvalidInput {
                operation: OP_FROM_ROWS,
                reason: "storage rows differ in length",
            });
        }
        let nrows = rows.len();
        let data: Vec<f64> = rows.into_iter().flatten().collect();
        Ok(Self {
            n,
            m,
            mtp,
            bw,
            storage: Some(Storage {
                rows: nrows,
                cols,
                data,
            }),
        })
    }

    /// Sets the logical shape, type and bandwidth, keeping the storage
    /// (`N_RealMatrixDefine`).
    pub fn define(&mut self, n: usize, m: usize, mtp: MatrixType, bw: usize) {
        self.n = n;
        self.m = m;
        self.mtp = mtp;
        self.bw = bw;
    }

    /// Highest row index `n`.
    #[must_use]
    pub fn n(&self) -> usize {
        self.n
    }

    /// Highest column index `m`.
    #[must_use]
    pub fn m(&self) -> usize {
        self.m
    }

    /// `(n, m)` (`N_GetMaxIndexRealMatrix`).
    #[must_use]
    pub fn max_index(&self) -> (usize, usize) {
        (self.n, self.m)
    }

    /// Matrix type.
    #[must_use]
    pub fn matrix_type(&self) -> MatrixType {
        self.mtp
    }

    /// Bandwidth `bw`.
    #[must_use]
    pub fn bandwidth(&self) -> usize {
        self.bw
    }

    /// `(rows, cols)` of the storage block, `None` without storage.
    #[must_use]
    pub fn storage_shape(&self) -> Option<(usize, usize)> {
        self.storage.as_ref().map(|s| (s.rows, s.cols))
    }

    /// True when the matrix needs storage before use: no storage or zero bandwidth
    /// (`N_RealMatrixIsNULL`).
    #[must_use]
    pub fn is_null(&self) -> bool {
        self.storage.is_none() || self.bw == 0
    }

    /// Storage element `[i][j]` (raw storage indexing, i.e. banded column offsets are
    /// not applied), or `InvalidInput` outside the storage.
    pub fn get(&self, i: usize, j: usize) -> Result<f64> {
        let s = self
            .storage
            .as_ref()
            .ok_or_else(|| out_of_range(OP_MATRIX))?;
        s.index(i, j)
            .and_then(|k| s.data.get(k).copied())
            .ok_or_else(|| out_of_range(OP_MATRIX))
    }

    /// Sets storage element `[i][j]`, or `InvalidInput` outside the storage.
    pub fn set(&mut self, i: usize, j: usize, v: f64) -> Result<()> {
        *self.slot(i, j, OP_MATRIX)? = v;
        Ok(())
    }

    fn rd(&self, i: usize, j: usize, op: &'static str) -> Result<f64> {
        self.get(i, j).map_err(|_| out_of_range(op))
    }

    fn slot(&mut self, i: usize, j: usize, op: &'static str) -> Result<&mut f64> {
        let s = self.storage.as_mut().ok_or_else(|| out_of_range(op))?;
        let k = s.index(i, j).ok_or_else(|| out_of_range(op))?;
        s.data.get_mut(k).ok_or_else(|| out_of_range(op))
    }

    fn wr(&mut self, i: usize, j: usize, v: f64, op: &'static str) -> Result<()> {
        *self.slot(i, j, op)? = v;
        Ok(())
    }

    fn require_storage(&self, op: &'static str) -> Result<()> {
        if self.storage.is_none() {
            return Err(SolidError::InvalidInput {
                operation: op,
                reason: "matrix has no storage",
            });
        }
        Ok(())
    }

    /// Fails with `NL_STO_ERR` when the logical shape is smaller than `nr x mc`
    /// (`N_CheckRealMatStorage`).
    pub fn check_storage(&self, nr: usize, mc: usize) -> Result<()> {
        self.check_storage_for(nr, mc, OP_CHECK_STORAGE)
    }

    fn check_storage_for(&self, nr: usize, mc: usize, op: &'static str) -> Result<()> {
        if self.n < nr || self.m < mc {
            return Err(nlib(op, NlibCode::Storage));
        }
        Ok(())
    }

    /// Makes the matrix ready to hold an `n x m` result: allocates when null,
    /// otherwise checks the logical shape and redefines it (`N_CheckMemRealMatrix`).
    pub fn ensure(&mut self, n: usize, m: usize, mtp: MatrixType, bw: usize) -> Result<()> {
        self.ensure_for(n, m, mtp, bw, OP_ENSURE)
    }

    fn ensure_for(
        &mut self,
        n: usize,
        m: usize,
        mtp: MatrixType,
        bw: usize,
        op: &'static str,
    ) -> Result<()> {
        if self.is_null() {
            *self = Self::new(n, m, mtp, bw)?;
        } else {
            self.check_storage_for(n, m, op)?;
            self.define(n, m, mtp, bw);
        }
        Ok(())
    }

    /// `C = self * b` into `c` (`N_RealMatrixMultiply`): `c` is allocated when null,
    /// else must be at least `na x mb`. The result is lower-left (upper-right) when
    /// both operands are, else full, with bandwidth `na`. Triangular operands only
    /// touch their triangles.
    ///
    /// Errors: `NL_IND_ERR` when `self.m != b.n`; `NL_INP_ERR` for banded operands;
    /// `NL_STO_ERR` when `c` is too small.
    pub fn multiply_into(&self, b: &RealMatrix, c: &mut RealMatrix) -> Result<()> {
        use MatrixType::{Full, LowerLeft as Ll, UpperRight as Ur};
        const OP: &str = OP_MULTIPLY;
        let (na, ma, mta) = (self.n, self.m, self.mtp);
        let (nb, mb, mtb) = (b.n, b.m, b.mtp);
        if ma != nb {
            return Err(nlib(OP, NlibCode::Index));
        }
        if mta == MatrixType::Banded || mtb == MatrixType::Banded {
            return Err(nlib(OP, NlibCode::Input));
        }
        self.require_storage(OP)?;
        b.require_storage(OP)?;
        let mtc = if mta == Ll && mtb == Ll {
            Ll
        } else if mta == Ur && mtb == Ur {
            Ur
        } else {
            Full
        };
        c.ensure_for(na, mb, mtc, na, OP)?;
        // One routine per type pair, with upstream's loop bounds.
        let cell =
            |c: &mut RealMatrix, i: usize, j: usize, ks: core::ops::RangeInclusive<usize>| {
                let mut acc = 0.0;
                for k in ks {
                    acc += self.rd(i, k, OP)? * b.rd(k, j, OP)?;
                }
                c.wr(i, j, acc, OP)
            };
        for i in 0..=na {
            match (mta, mtb) {
                (Full, Full) => {
                    for j in 0..=mb {
                        cell(c, i, j, 0..=ma)?;
                    }
                }
                (Full, Ll) => {
                    for j in 0..=mb {
                        cell(c, i, j, j..=ma)?;
                    }
                }
                (Full, Ur) => {
                    for j in 0..=mb {
                        cell(c, i, j, 0..=j)?;
                    }
                }
                (Ll, Full) => {
                    for j in 0..=mb {
                        cell(c, i, j, 0..=i)?;
                    }
                }
                (Ll, Ll) => {
                    for j in 0..=i {
                        cell(c, i, j, j..=i)?;
                    }
                }
                (Ll, Ur) => {
                    for j in 0..=mb {
                        cell(c, i, j, 0..=i.min(j))?;
                    }
                }
                (Ur, Full) => {
                    for j in 0..=mb {
                        cell(c, i, j, i..=ma)?;
                    }
                }
                (Ur, Ll) => {
                    for j in 0..=mb {
                        cell(c, i, j, i.max(j)..=ma)?;
                    }
                }
                (Ur, Ur) => {
                    for j in i..=mb {
                        cell(c, i, j, i..=j)?;
                    }
                }
                (MatrixType::Banded, _) | (_, MatrixType::Banded) => {
                    return Err(nlib(OP, NlibCode::Input));
                }
            }
        }
        Ok(())
    }

    /// `self * b` as a new matrix ([`RealMatrix::multiply_into`] with a null output).
    pub fn multiply(&self, b: &RealMatrix) -> Result<RealMatrix> {
        let mut c = Self::null();
        self.multiply_into(b, &mut c)?;
        Ok(c)
    }

    /// Writes the transpose of `self` (with storage `a`, shape `n x m`) into `b`.
    fn transpose_fill(
        a: &RealMatrix,
        b: &mut RealMatrix,
        n: usize,
        m: usize,
        mta: MatrixType,
        bw: usize,
    ) -> Result<()> {
        const OP: &str = OP_TRANSPOSE;
        let sbw = bw / 2;
        match mta {
            MatrixType::LowerLeft => {
                // Rows past `m` have no entries (empty inner loop upstream).
                for i in 0..=n.min(m) {
                    for j in i..=m {
                        b.wr(i, j, a.rd(j, i, OP)?, OP)?;
                    }
                }
            }
            MatrixType::UpperRight => {
                // Columns past `n` have no entries (empty inner loop upstream).
                for j in 0..=m.min(n) {
                    for i in j..=n {
                        b.wr(i, j, a.rd(j, i, OP)?, OP)?;
                    }
                }
            }
            MatrixType::Banded => {
                for i in 0..=n {
                    let jl = i.saturating_sub(sbw);
                    let jh = n.min(i + sbw);
                    for j in jl..=jh {
                        let v = a.rd(i, band_index(j, sbw, i, OP)?, OP)?;
                        b.wr(j, band_index(i, sbw, j, OP)?, v, OP)?;
                    }
                }
            }
            MatrixType::Full => {
                for i in 0..=n {
                    for j in 0..=m {
                        b.wr(j, i, a.rd(i, j, OP)?, OP)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn transposed_type(mta: MatrixType) -> MatrixType {
        match mta {
            MatrixType::LowerLeft => MatrixType::UpperRight,
            MatrixType::UpperRight => MatrixType::LowerLeft,
            MatrixType::Banded => MatrixType::Banded,
            MatrixType::Full => MatrixType::Full,
        }
    }

    /// Transpose of `self` into a different matrix `b` (`N_RealMatrixTranspose` with
    /// `rma != rmb`): `b` is allocated when null, else must be at least `m x n`.
    /// Lower-left and upper-right swap; only the meaningful triangle (or band) of
    /// `b` is written.
    pub fn transpose_into(&self, b: &mut RealMatrix) -> Result<()> {
        self.require_storage(OP_TRANSPOSE)?;
        let (n, m, mta, bw) = (self.n, self.m, self.mtp, self.bw);
        b.ensure_for(m, n, Self::transposed_type(mta), bw, OP_TRANSPOSE)?;
        Self::transpose_fill(self, b, n, m, mta, bw)
    }

    /// Transpose as a new matrix ([`RealMatrix::transpose_into`] with a null output).
    pub fn transpose(&self) -> Result<RealMatrix> {
        let mut b = Self::null();
        self.transpose_into(&mut b)?;
        Ok(b)
    }

    /// In-place transpose (`N_RealMatrixTranspose(rma, rma, ..)`): triangular
    /// matrices are mirrored within their storage (the old triangle stays in place);
    /// full and banded matrices get new `(m+1) x (n+1)` storage.
    pub fn transpose_in_place(&mut self) -> Result<()> {
        self.require_storage(OP_TRANSPOSE)?;
        let (n, m, mta, bw) = (self.n, self.m, self.mtp, self.bw);
        let mtb = Self::transposed_type(mta);
        match mta {
            MatrixType::LowerLeft | MatrixType::UpperRight => {
                self.define(m, n, mtb, bw);
                // B and A share storage: B[i][j] = A[j][i] element by element, in
                // upstream's order.
                // Outer indices past the other bound have empty inner loops
                // upstream; they are skipped.
                if mta == MatrixType::LowerLeft {
                    for i in 0..=n.min(m) {
                        for j in i..=m {
                            let v = self.rd(j, i, OP_TRANSPOSE)?;
                            self.wr(i, j, v, OP_TRANSPOSE)?;
                        }
                    }
                } else {
                    for j in 0..=m.min(n) {
                        for i in j..=n {
                            let v = self.rd(j, i, OP_TRANSPOSE)?;
                            self.wr(i, j, v, OP_TRANSPOSE)?;
                        }
                    }
                }
                Ok(())
            }
            MatrixType::Full | MatrixType::Banded => {
                let rows = m.checked_add(1).ok_or_else(|| out_of_range(OP_TRANSPOSE))?;
                let cols = n.checked_add(1).ok_or_else(|| out_of_range(OP_TRANSPOSE))?;
                let mut b = RealMatrix {
                    n: m,
                    m: n,
                    mtp: mtb,
                    bw,
                    storage: Some(Storage::zeros(rows, cols, OP_TRANSPOSE)?),
                };
                Self::transpose_fill(self, &mut b, n, m, mta, bw)?;
                *self = b;
                Ok(())
            }
        }
    }

    /// `Q = self * P` for a point array (`N_RealMatrixMultiplyPtArray`); returns
    /// `n + 1` points. Triangular and banded matrices only use their stored entries.
    pub fn multiply_points(&self, p: &[Vec3]) -> Result<Vec<Vec3>> {
        const OP: &str = OP_MUL_PTS;
        self.require_storage(OP)?;
        let (n, m, bw) = (self.n, self.m, self.bw);
        let sbw = bw / 2;
        let mut q = filled(n.checked_add(1), Vec3::ZERO, OP)?;
        for (i, qi) in q.iter_mut().enumerate() {
            let mut acc = Vec3::ZERO;
            match self.mtp {
                MatrixType::LowerLeft => {
                    for j in 0..=i {
                        blend_pt(&mut acc, self.rd(i, j, OP)?, at(p, j, OP)?);
                    }
                }
                MatrixType::UpperRight => {
                    for j in i..=m {
                        blend_pt(&mut acc, self.rd(i, j, OP)?, at(p, j, OP)?);
                    }
                }
                MatrixType::Full => {
                    for j in 0..=m {
                        blend_pt(&mut acc, self.rd(i, j, OP)?, at(p, j, OP)?);
                    }
                }
                MatrixType::Banded => {
                    for j in i.saturating_sub(sbw)..=n.min(i + sbw) {
                        let a = self.rd(i, band_index(j, sbw, i, OP)?, OP)?;
                        blend_pt(&mut acc, a, at(p, j, OP)?);
                    }
                }
            }
            *qi = acc;
        }
        Ok(q)
    }

    /// `Qw = self * Pw` for a control point array (`N_RealMatrixMultiplyCPtArray`);
    /// returns `n + 1` control points, accumulated from [`CPoint::ZERO`] with
    /// [`CPoint::blend`].
    pub fn multiply_cpoints(&self, pw: &[CPoint]) -> Result<Vec<CPoint>> {
        const OP: &str = OP_MUL_CPTS;
        self.require_storage(OP)?;
        let (n, m, bw) = (self.n, self.m, self.bw);
        let sbw = bw / 2;
        let mut q = filled(n.checked_add(1), CPoint::ZERO, OP)?;
        for (i, qi) in q.iter_mut().enumerate() {
            let mut acc = CPoint::ZERO;
            match self.mtp {
                MatrixType::LowerLeft => {
                    for j in 0..=i {
                        acc.blend(self.rd(i, j, OP)?, at(pw, j, OP)?);
                    }
                }
                MatrixType::UpperRight => {
                    for j in i..=m {
                        acc.blend(self.rd(i, j, OP)?, at(pw, j, OP)?);
                    }
                }
                MatrixType::Full => {
                    for j in 0..=m {
                        acc.blend(self.rd(i, j, OP)?, at(pw, j, OP)?);
                    }
                }
                MatrixType::Banded => {
                    for j in i.saturating_sub(sbw)..=n.min(i + sbw) {
                        let a = self.rd(i, band_index(j, sbw, i, OP)?, OP)?;
                        acc.blend(a, at(pw, j, OP)?);
                    }
                }
            }
            *qi = acc;
        }
        Ok(q)
    }

    /// In-place LU decomposition without pivoting (Doolittle: unit lower triangle
    /// below the diagonal, upper triangle on and above it) of a full or banded square
    /// matrix (`N_RealMatrixLuDecompose`).
    ///
    /// Errors: `NL_IND_ERR` (not square), `NL_INP_ERR` (triangular type),
    /// `NL_NUM_ERR` (pivot below [`LU_TOL`]; the matrix is left partly decomposed).
    pub fn lu_decompose(&mut self) -> Result<()> {
        const OP: &str = OP_LU;
        let (n, m, mtp, bw) = (self.n, self.m, self.mtp, self.bw);
        if m != n {
            return Err(nlib(OP, NlibCode::Index));
        }
        if mtp != MatrixType::Full && mtp != MatrixType::Banded {
            return Err(nlib(OP, NlibCode::Input));
        }
        self.require_storage(OP)?;
        let sbw = bw / 2;
        if mtp == MatrixType::Full {
            for i in 0..=n {
                for j in i..=n {
                    let mut sum = 0.0;
                    for k in 0..i {
                        sum += self.rd(i, k, OP)? * self.rd(k, j, OP)?;
                    }
                    *self.slot(i, j, OP)? -= sum;
                }
                for j in i + 1..=n {
                    let mut sum = 0.0;
                    for k in 0..i {
                        sum += self.rd(j, k, OP)? * self.rd(k, i, OP)?;
                    }
                    let aii = self.rd(i, i, OP)?;
                    if aii.abs() < LU_TOL {
                        return Err(nlib(OP, NlibCode::Numerical));
                    }
                    let v = (self.rd(j, i, OP)? - sum) / aii;
                    self.wr(j, i, v, OP)?;
                }
            }
        } else {
            for i in 0..=n {
                let jh = n.min(i + sbw);
                for j in i..=jh {
                    let kl = j.saturating_sub(sbw);
                    let mut sum = 0.0;
                    for k in kl..i {
                        sum += self.rd(i, band_index(k, sbw, i, OP)?, OP)?
                            * self.rd(k, band_index(j, sbw, k, OP)?, OP)?;
                    }
                    *self.slot(i, band_index(j, sbw, i, OP)?, OP)? -= sum;
                }
                for j in i + 1..=jh {
                    let kl = j.saturating_sub(sbw);
                    let mut sum = 0.0;
                    for k in kl..i {
                        sum += self.rd(j, band_index(k, sbw, j, OP)?, OP)?
                            * self.rd(k, band_index(i, sbw, k, OP)?, OP)?;
                    }
                    let pivot = self.rd(i, sbw, OP)?;
                    if pivot.abs() < LU_TOL {
                        return Err(nlib(OP, NlibCode::Numerical));
                    }
                    let col = band_index(i, sbw, j, OP)?;
                    let v = (self.rd(j, col, OP)? - sum) / pivot;
                    self.wr(j, col, v, OP)?;
                }
            }
        }
        Ok(())
    }

    /// In-place LU decomposition with partial pivoting (Crout, implicit row scaling)
    /// of a full square matrix (`N_RealMatrixLuDecomposePivot`); returns the row
    /// permutation `ind` (`ind[j]` = row swapped with row `j`).
    ///
    /// Errors: `NL_IND_ERR` (not square), `NL_INP_ERR` (not full), `NL_NUM_ERR` (a row
    /// maximum or pivot below [`LU_TOL`]; the matrix is left partly decomposed).
    pub fn lu_decompose_pivot(&mut self) -> Result<Vec<usize>> {
        const OP: &str = OP_LU_PIVOT;
        let (n, m, mtp) = (self.n, self.m, self.mtp);
        if m != n {
            return Err(nlib(OP, NlibCode::Index));
        }
        if mtp != MatrixType::Full {
            return Err(nlib(OP, NlibCode::Input));
        }
        self.require_storage(OP)?;
        let mut v = filled(n.checked_add(1), 0.0, OP)?;
        let mut ind = filled(n.checked_add(1), 0usize, OP)?;
        // Scaling info.
        for i in 0..=n {
            let mut big = 0.0;
            for j in 0..=n {
                let temp = self.rd(i, j, OP)?.abs();
                if temp > big {
                    big = temp;
                }
            }
            if big < LU_TOL {
                return Err(nlib(OP, NlibCode::Numerical));
            }
            *at_mut(&mut v, i, OP)? = 1.0 / big;
        }
        for j in 0..=n {
            let mut imax = j;
            for i in 0..j {
                let mut sum = self.rd(i, j, OP)?;
                for k in 0..i {
                    sum -= self.rd(i, k, OP)? * self.rd(k, j, OP)?;
                }
                self.wr(i, j, sum, OP)?;
            }
            let mut big = 0.0;
            for i in j..=n {
                let mut sum = self.rd(i, j, OP)?;
                for k in 0..j {
                    sum -= self.rd(i, k, OP)? * self.rd(k, j, OP)?;
                }
                self.wr(i, j, sum, OP)?;
                let dum = at(&v, i, OP)? * sum.abs();
                if dum >= big {
                    big = dum;
                    imax = i;
                }
            }
            if j != imax {
                for k in 0..=n {
                    let a = self.rd(imax, k, OP)?;
                    let b = self.rd(j, k, OP)?;
                    self.wr(imax, k, b, OP)?;
                    self.wr(j, k, a, OP)?;
                }
                *at_mut(&mut v, imax, OP)? = at(&v, j, OP)?;
            }
            *at_mut(&mut ind, j, OP)? = imax;
            let ajj = self.rd(j, j, OP)?;
            if ajj.abs() < LU_TOL {
                return Err(nlib(OP, NlibCode::Numerical));
            }
            if j < n {
                let dum = 1.0 / ajj;
                for i in j + 1..=n {
                    *self.slot(i, j, OP)? *= dum;
                }
            }
        }
        Ok(ind)
    }

    /// Checks the type of an LU-decomposed matrix for the substitution routines.
    fn lu_kind(&self, op: &'static str) -> Result<()> {
        if self.m != self.n {
            return Err(nlib(op, NlibCode::Index));
        }
        if self.mtp != MatrixType::Full && self.mtp != MatrixType::Banded {
            return Err(nlib(op, NlibCode::Input));
        }
        self.require_storage(op)
    }

    /// Column of `B[i][j]` for full or banded storage.
    fn lu_col(&self, i: usize, j: usize, sbw: usize, op: &'static str) -> Result<usize> {
        if self.mtp == MatrixType::Banded {
            band_index(j, sbw, i, op)
        } else {
            Ok(j)
        }
    }

    /// Range of `j < i` used by the forward pass for row `i`.
    fn lower_range(&self, i: usize, sbw: usize) -> core::ops::Range<usize> {
        if self.mtp == MatrixType::Banded {
            i.saturating_sub(sbw)..i
        } else {
            0..i
        }
    }

    /// Range of `j > i` used by the backward pass for row `i`.
    fn upper_range(&self, i: usize, sbw: usize) -> core::ops::RangeInclusive<usize> {
        if self.mtp == MatrixType::Banded {
            i + 1..=self.n.min(i + sbw)
        } else {
            i + 1..=self.n
        }
    }

    /// Solves `LU * Q = P` for Euclidean right-hand sides (`N_RealMatrixForBack` with
    /// `NL_EPOINT`); `self` must be LU-decomposed ([`RealMatrix::lu_decompose`]). The
    /// solution points are returned as non-rational control points.
    ///
    /// Errors: `NL_IND_ERR` (not square), `NL_INP_ERR` (triangular type),
    /// `NL_NUM_ERR` (a diagonal entry cannot be inverted).
    pub fn for_back_points(&self, p: &[Vec3]) -> Result<Vec<CPoint>> {
        const OP: &str = OP_FOR_BACK;
        self.lu_kind(OP)?;
        let n = self.n;
        let sbw = self.bw / 2;
        let len = n.checked_add(1);
        let mut z = filled(len, Vec3::ZERO, OP)?;
        let mut q = filled(len, Vec3::ZERO, OP)?;
        let mut qw = filled(len, CPoint::ZERO, OP)?;
        for i in 0..=n {
            let mut s = Vec3::ZERO;
            for j in self.lower_range(i, sbw) {
                blend_pt(
                    &mut s,
                    self.rd(i, self.lu_col(i, j, sbw, OP)?, OP)?,
                    at(&z, j, OP)?,
                );
            }
            *at_mut(&mut z, i, OP)? = at(p, i, OP)? - s;
        }
        for i in (0..=n).rev() {
            let diag = self.rd(i, self.lu_col(i, i, sbw, OP)?, OP)?;
            if float_op_is_bad(1.0, diag, FloatOp::Division) {
                return Err(nlib(OP, NlibCode::Numerical));
            }
            let fact = 1.0 / diag;
            let mut s = Vec3::ZERO;
            for j in self.upper_range(i, sbw) {
                blend_pt(
                    &mut s,
                    self.rd(i, self.lu_col(i, j, sbw, OP)?, OP)?,
                    at(&q, j, OP)?,
                );
            }
            let qi = combine2_pts(fact, at(&z, i, OP)?, -fact, s);
            *at_mut(&mut q, i, OP)? = qi;
            *at_mut(&mut qw, i, OP)? = CPoint::from_point(qi);
        }
        Ok(qw)
    }

    /// Solves `LU * Qw = Pw` for homogeneous right-hand sides (`N_RealMatrixForBack`
    /// with `NL_HPOINT`); `self` must be LU-decomposed. Sums start from
    /// [`CPoint::ZERO`] and use the `NOZ`/`NOW` rules of [`CPoint::blend`],
    /// [`CPoint::diff`] and [`CPoint::combine2`].
    pub fn for_back_cpoints(&self, pw: &[CPoint]) -> Result<Vec<CPoint>> {
        const OP: &str = OP_FOR_BACK;
        self.lu_kind(OP)?;
        let n = self.n;
        let sbw = self.bw / 2;
        let len = n.checked_add(1);
        let mut zw = filled(len, CPoint::ZERO, OP)?;
        let mut qw = filled(len, CPoint::ZERO, OP)?;
        for i in 0..=n {
            let mut sw = CPoint::ZERO;
            for j in self.lower_range(i, sbw) {
                sw.blend(
                    self.rd(i, self.lu_col(i, j, sbw, OP)?, OP)?,
                    at(&zw, j, OP)?,
                );
            }
            *at_mut(&mut zw, i, OP)? = CPoint::diff(at(pw, i, OP)?, sw);
        }
        for i in (0..=n).rev() {
            let diag = self.rd(i, self.lu_col(i, i, sbw, OP)?, OP)?;
            if float_op_is_bad(1.0, diag, FloatOp::Division) {
                return Err(nlib(OP, NlibCode::Numerical));
            }
            let fact = 1.0 / diag;
            let mut sw = CPoint::ZERO;
            for j in self.upper_range(i, sbw) {
                sw.blend(
                    self.rd(i, self.lu_col(i, j, sbw, OP)?, OP)?,
                    at(&qw, j, OP)?,
                );
            }
            *at_mut(&mut qw, i, OP)? = CPoint::combine2(fact, at(&zw, i, OP)?, -fact, sw);
        }
        Ok(qw)
    }

    /// Solves `LU * x = y` for a real right-hand side (`N_RealMatrixRightForBack`);
    /// `self` must be LU-decomposed (full or banded).
    pub fn right_for_back(&self, y: &[f64]) -> Result<Vec<f64>> {
        const OP: &str = OP_RIGHT_FOR_BACK;
        self.lu_kind(OP)?;
        let n = self.n;
        let sbw = self.bw / 2;
        let len = n.checked_add(1);
        let mut z = filled(len, 0.0, OP)?;
        let mut x = filled(len, 0.0, OP)?;
        for i in 0..=n {
            let mut sum = 0.0;
            for j in self.lower_range(i, sbw) {
                sum += self.rd(i, self.lu_col(i, j, sbw, OP)?, OP)? * at(&z, j, OP)?;
            }
            *at_mut(&mut z, i, OP)? = at(y, i, OP)? - sum;
        }
        for i in (0..=n).rev() {
            let mut sum = 0.0;
            for j in self.upper_range(i, sbw) {
                sum += self.rd(i, self.lu_col(i, j, sbw, OP)?, OP)? * at(&x, j, OP)?;
            }
            let diag = self.rd(i, self.lu_col(i, i, sbw, OP)?, OP)?;
            let zi = at(&z, i, OP)?;
            if float_op_is_bad(zi - sum, diag, FloatOp::Division) {
                return Err(nlib(OP, NlibCode::Numerical));
            }
            *at_mut(&mut x, i, OP)? = (zi - sum) / diag;
        }
        Ok(x)
    }

    /// Solves `A x = b` in place given the pivoted decomposition of `A`
    /// ([`RealMatrix::lu_decompose_pivot`] and its `ind`) (`N_RealMatrixForBackPivot`).
    /// Leading zero right-hand-side entries are skipped (`|b| <= LU_TOL`), as upstream.
    pub fn for_back_pivot(&self, ind: &[usize], b: &mut [f64]) -> Result<()> {
        const OP: &str = OP_FOR_BACK_PIVOT;
        let n = self.n;
        if self.m != n {
            return Err(nlib(OP, NlibCode::Index));
        }
        if self.mtp != MatrixType::Full {
            return Err(nlib(OP, NlibCode::Input));
        }
        self.require_storage(OP)?;
        let mut ii: Option<usize> = None;
        for i in 0..=n {
            let ip = at(ind, i, OP)?;
            let mut sum = at(b, ip, OP)?;
            *at_mut(b, ip, OP)? = at(b, i, OP)?;
            if let Some(first) = ii {
                for j in first..i {
                    sum -= self.rd(i, j, OP)? * at(b, j, OP)?;
                }
            } else if sum.abs() > LU_TOL {
                ii = Some(i);
            }
            *at_mut(b, i, OP)? = sum;
        }
        for i in (0..=n).rev() {
            let mut sum = at(b, i, OP)?;
            for j in i + 1..=n {
                sum -= self.rd(i, j, OP)? * at(b, j, OP)?;
            }
            let aii = self.rd(i, i, OP)?;
            if float_op_is_bad(sum, aii, FloatOp::Division) {
                return Err(nlib(OP, NlibCode::Numerical));
            }
            *at_mut(b, i, OP)? = sum / aii;
        }
        Ok(())
    }

    /// Inverse of a full or banded square matrix into `b` (`N_RealMatrixInverse`):
    /// `b` (full, bandwidth `n`) is allocated when null, else must be at least
    /// `n x n`. Unless `lu_done`, `self` is first LU-decomposed in place (no
    /// pivoting); each column is one [`RealMatrix::right_for_back`].
    pub fn inverse_into(&mut self, lu_done: bool, b: &mut RealMatrix) -> Result<()> {
        const OP: &str = OP_INVERSE;
        let (n, m, mta) = (self.n, self.m, self.mtp);
        if n != m {
            return Err(nlib(OP, NlibCode::Index));
        }
        if mta != MatrixType::Full && mta != MatrixType::Banded {
            return Err(nlib(OP, NlibCode::Input));
        }
        self.require_storage(OP)?;
        b.ensure_for(n, m, MatrixType::Full, n, OP)?;
        if !lu_done {
            self.lu_decompose()?;
        }
        let mut y = filled(n.checked_add(1), 0.0, OP)?;
        *at_mut(&mut y, 0, OP)? = 1.0;
        let x = self.right_for_back(&y)?;
        for (i, xi) in x.iter().enumerate() {
            b.wr(i, 0, *xi, OP)?;
        }
        for i in 1..=n {
            *at_mut(&mut y, i - 1, OP)? = 0.0;
            *at_mut(&mut y, i, OP)? = 1.0;
            let x = self.right_for_back(&y)?;
            for (j, xj) in x.iter().enumerate() {
                b.wr(j, i, *xj, OP)?;
            }
        }
        Ok(())
    }

    /// Inverse as a new matrix ([`RealMatrix::inverse_into`] with a null output).
    pub fn inverse(&mut self, lu_done: bool) -> Result<RealMatrix> {
        let mut b = Self::null();
        self.inverse_into(lu_done, &mut b)?;
        Ok(b)
    }

    /// Least-squares solution of the overdetermined system `self * x = rhs`
    /// (`self` has `rows - 1 = n > m = cols - 1`) via the normal equations
    /// `AᵀA x = Aᵀ rhs`, solved with [`RealMatrix::lu_decompose_pivot`] and
    /// [`RealMatrix::for_back_pivot`] (`N_RealMatrixLstSqSolve`).
    ///
    /// Errors: `NL_IND_ERR` unless there are more rows than columns; errors of the
    /// called routines.
    pub fn lst_sq_solve(&self, rhs: &[f64]) -> Result<Vec<f64>> {
        const OP: &str = OP_LST_SQ;
        // Upstream names the row index m and the column index n here.
        let (m, n) = (self.n, self.m);
        if m <= n {
            return Err(nlib(OP, NlibCode::Index));
        }
        let rmt = self.transpose()?;
        let mut rmta = rmt.multiply(self)?;
        let mut sol = filled(n.checked_add(1), 0.0, OP)?;
        for (ii, s) in sol.iter_mut().enumerate() {
            let mut acc = 0.0;
            for kk in 0..=m {
                acc += self.rd(kk, ii, OP)? * at(rhs, kk, OP)?;
            }
            *s = acc;
        }
        let ind = rmta.lu_decompose_pivot()?;
        rmta.for_back_pivot(&ind, &mut sol)?;
        Ok(sol)
    }
}

// ---- power basis surfaces ---------------------------------------------------------

/// Point of the power-basis surface `S(u,v) = sum aw[i][j] u^i v^j`, `i <= n`,
/// `j <= m`, by Horner's method in homogeneous space, then projected
/// (`N_PowerBasisSrfEvalPts`).
pub fn power_basis_srf_eval_pts(
    aw: &[Vec<CPoint>],
    n: usize,
    m: usize,
    u: f64,
    v: f64,
) -> Result<Vec3> {
    const OP: &str = OP_SRF_EVAL;
    let row = |i: usize| aw.get(i).map(Vec::as_slice).ok_or_else(|| out_of_range(OP));
    let horner = |r: &[CPoint]| -> Result<CPoint> {
        let mut s = at(r, m, OP)?;
        for j in (0..m).rev() {
            s = CPoint::combine2(v, s, 1.0, at(r, j, OP)?);
        }
        Ok(s)
    };
    let mut sw = horner(row(n)?)?;
    for i in (0..n).rev() {
        let sa = horner(row(i)?)?;
        sw = CPoint::combine2(u, sw, 1.0, sa);
    }
    Ok(sw.to_euclid())
}

/// Point of a power-basis surface patch given by its coefficient net `pw`
/// (`N_SrfPowerBasisEvalPt`; upstream takes the `NL_SURFACE` holding that net and
/// evaluates outside the parameter rectangle too).
pub fn srf_power_basis_eval_pt(pw: &[Vec<CPoint>], u: f64, v: f64) -> Result<Vec3> {
    let n = pw
        .len()
        .checked_sub(1)
        .ok_or_else(|| out_of_range(OP_SRF_EVAL))?;
    let m = pw
        .first()
        .and_then(|r| r.len().checked_sub(1))
        .ok_or_else(|| out_of_range(OP_SRF_EVAL))?;
    power_basis_srf_eval_pts(pw, n, m, u, v)
}

/// Partial derivatives `SD[k][l] = d^(k+l) S / du^k dv^l`, `k <= udr`, `l <= vdr`, of
/// the power-basis patch with coefficients `pw[0..=p][0..=q]` (rational when
/// `pw[0][0].w != NOW`) (`N_SrfPowerBasisEvalDerivs`).
///
/// With `upper_half` and `udr == vdr` only `k + l <= udr` is computed (the rest stays
/// zero), with upstream's quirks: in the rational branch the final restriction tests
/// the loop variable `k1` against `vdr` instead of `udr == vdr`.
///
/// Errors: `NL_INP_ERR` when `udr` or `vdr` exceed [`MAX_DERIVATIVE`]; `NL_CON_ERR`
/// when the rational weight cannot be inverted; `InvalidInput` where upstream would
/// overrun its fixed `NL_MAXDEG + 1` scratch array (it stores the v-power rows after
/// the `p + 1` u-powers), the net, or `SD`.
pub fn srf_power_basis_eval_derivs(
    pw: &[Vec<CPoint>],
    p: usize,
    q: usize,
    u: f64,
    v: f64,
    upper_half: bool,
    udr: usize,
    vdr: usize,
) -> Result<Vec<Vec<Vec3>>> {
    const OP: &str = OP_SRF_DERIVS;
    if udr > MAX_DERIVATIVE || vdr > MAX_DERIVATIVE {
        return Err(nlib(OP, NlibCode::Input));
    }
    let net = |i: usize, j: usize| -> Result<CPoint> {
        pw.get(i)
            .and_then(|r| r.get(j))
            .copied()
            .ok_or_else(|| out_of_range(OP))
    };
    // up[0..=p] holds u-powers, up[p+1..] ("vp") the rows of v-power derivatives.
    let mut up = [0.0f64; MAX_DEGREE + 1];
    let vb = p + 1;
    *at_mut(&mut up, vb, OP)? = 1.0;
    for ii in 1..=q {
        let prev = at(&up, vb + ii - 1, OP)?;
        *at_mut(&mut up, vb + ii, OP)? = v * prev;
    }
    let maxvd = vdr.min(q);
    let mut k1 = 0usize;
    let mut k2 = 0usize;
    for ii in 1..=maxvd {
        k1 = k2;
        k2 += q + 1;
        for jj in ii..=q {
            let prev = at(&up, vb + k1 + jj - 1, OP)?;
            *at_mut(&mut up, vb + k2 + jj, OP)? = jj as f64 * prev;
        }
    }
    let mut sd = filled(
        udr.checked_add(1),
        filled(vdr.checked_add(1), Vec3::ZERO, OP)?,
        OP,
    )?;
    let mut temp = [CPoint::ZERO; MAX_DEGREE + 1];
    let rational = net(0, 0)?.w != NOW;
    let mut cw_ders = filled(
        udr.checked_add(1),
        filled(vdr.checked_add(1), CPoint::ZERO, OP)?,
        OP,
    )?;
    // Derivatives of the polynomial part (all four coordinates when rational).
    *at_mut(&mut up, 0, OP)? = 1.0;
    for ii in 1..=p {
        let prev = at(&up, ii - 1, OP)?;
        *at_mut(&mut up, ii, OP)? = u * prev;
    }
    let maxud = udr.min(p);
    for ii in 0..=maxud {
        if ii > 0 {
            for jj in (ii..=p).rev() {
                let prev = at(&up, jj - 1, OP)?;
                *at_mut(&mut up, jj, OP)? = jj as f64 * prev;
            }
        }
        for jj in 0..=q {
            let mut t = CPoint::ZERO;
            for kk in ii..=p {
                let c = at(&up, kk, OP)?;
                let pk = net(kk, jj)?;
                t.x += c * pk.x;
                t.y += c * pk.y;
                t.z += c * pk.z;
                if rational {
                    t.w += c * pk.w;
                }
            }
            *at_mut(&mut temp, jj, OP)? = t;
        }
        k2 = maxvd;
        if udr == vdr && upper_half {
            k2 = k2.min(udr - ii);
        }
        k1 = 0;
        for jj in 0..=k2 {
            for kk in jj..=q {
                let c = at(&up, vb + k1 + kk, OP)?;
                let t = at(&temp, kk, OP)?;
                if rational {
                    let d = at_mut(at_mut(&mut cw_ders, ii, OP)?, jj, OP)?;
                    d.x += c * t.x;
                    d.y += c * t.y;
                    d.z += c * t.z;
                    d.w += c * t.w;
                } else {
                    let d = at_mut(at_mut(&mut sd, ii, OP)?, jj, OP)?;
                    d.x += c * t.x;
                    d.y += c * t.y;
                    d.z += c * t.z;
                }
            }
            k1 += q + 1;
        }
    }
    if !rational {
        return Ok(sd);
    }
    // Binomial coefficients up to max(udr, vdr) (upstream inlines N_PascalTriRow).
    let bin = pascal_triangle(udr.max(vdr))?;
    let binom = |i: usize, j: usize| -> Result<f64> {
        bin.get(i)
            .and_then(|r| r.get(j))
            .copied()
            .map(|b| b as f64)
            .ok_or_else(|| out_of_range(OP))
    };
    let cwd = |c: &Vec<Vec<CPoint>>, i: usize, j: usize| -> Result<CPoint> {
        c.get(i)
            .and_then(|r| r.get(j))
            .copied()
            .ok_or_else(|| out_of_range(OP))
    };
    let sdg = |s: &Vec<Vec<Vec3>>, i: usize, j: usize| -> Result<Vec3> {
        s.get(i)
            .and_then(|r| r.get(j))
            .copied()
            .ok_or_else(|| out_of_range(OP))
    };
    let w00 = cwd(&cw_ders, 0, 0)?.w;
    if float_op_is_bad(1.0, w00, FloatOp::Division) {
        return Err(nlib(OP, NlibCode::Convergence));
    }
    for kk in 0..=udr {
        let mut k2 = vdr;
        if upper_half && k1 == k2 {
            k2 = udr - kk;
        }
        for ll in 0..=k2 {
            let c = cwd(&cw_ders, kk, ll)?;
            let mut v1 = Vec3::new(c.x, c.y, c.z);
            for jj in 1..=ll {
                let alpha = -(binom(ll, jj)? * cwd(&cw_ders, 0, jj)?.w);
                blend_pt(&mut v1, alpha, sdg(&sd, kk, ll - jj)?);
            }
            for ii in 1..=kk {
                let alpha = -(binom(kk, ii)? * cwd(&cw_ders, ii, 0)?.w);
                blend_pt(&mut v1, alpha, sdg(&sd, kk - ii, ll)?);
                let mut v2 = Vec3::ZERO;
                for jj in 1..=ll {
                    let beta = binom(ll, jj)? * cwd(&cw_ders, ii, jj)?.w;
                    blend_pt(&mut v2, beta, sdg(&sd, kk - ii, ll - jj)?);
                }
                blend_pt(&mut v1, -binom(kk, ii)?, v2);
            }
            let out = at_mut(at_mut(&mut sd, kk, OP)?, ll, OP)?;
            out.x = v1.x / w00;
            out.y = v1.y / w00;
            out.z = v1.z / w00;
        }
    }
    Ok(sd)
}

// ---- reallocation -----------------------------------------------------------------

/// Resizes `a` to highest index `m`, keeping entries `0..=min(n, m)` and filling the
/// rest with `T::default()` (`N_Realloc1dFlagArray`, `N_Realloc1dRealArray`,
/// `N_Realloc1dPtArray`, `N_Realloc1dCPtArray`; upstream leaves new entries
/// uninitialised).
///
/// Errors with `InvalidInput` when `a` has fewer than `min(n, m) + 1` entries.
pub fn realloc_1d<T: Copy + Default>(a: &mut Vec<T>, n: usize, m: usize) -> Result<()> {
    let mut b = filled(m.checked_add(1), T::default(), OP_REALLOC)?;
    let kl = n.min(m);
    for k in 0..=kl {
        *at_mut(&mut b, k, OP_REALLOC)? = at(a, k, OP_REALLOC)?;
    }
    *a = b;
    Ok(())
}

/// Resizes the 2D array `a` from highest indexes `(n, m)` to `(k, l)`, keeping the
/// overlapping block (`N_Realloc2dRealArray`, `N_Realloc2dCPtArray`).
///
/// Errors with `InvalidInput` when `a` is smaller than the block to copy.
pub fn realloc_2d<T: Copy + Default>(
    a: &mut Vec<Vec<T>>,
    n: usize,
    m: usize,
    k: usize,
    l: usize,
) -> Result<()> {
    let row = filled(l.checked_add(1), T::default(), OP_REALLOC)?;
    let mut b = filled(k.checked_add(1), row, OP_REALLOC)?;
    let rl = k.min(n);
    let sl = l.min(m);
    for r in 0..=rl {
        let src = a.get(r).ok_or_else(|| out_of_range(OP_REALLOC))?;
        let dst = at_mut(&mut b, r, OP_REALLOC)?;
        for s in 0..=sl {
            *at_mut(dst, s, OP_REALLOC)? = at(src, s, OP_REALLOC)?;
        }
    }
    *a = b;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full(rows: &[&[f64]]) -> RealMatrix {
        let n = rows.len() - 1;
        let m = rows[0].len() - 1;
        RealMatrix::from_rows(
            n,
            m,
            rows.iter().map(|r| r.to_vec()).collect(),
            MatrixType::Full,
            n,
        )
        .unwrap()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-12 * (1.0 + b.abs())
    }

    #[test]
    fn null_matrix_has_a_defined_full_type() {
        // source fork a05e288 (#370): N_InitRealMatrix left the type
        // indeterminate, and N_RealMatrixIsNULL loads it; the fork sets NL_MT_FULL.
        // The port's null matrix has that type, and the first N_CheckMemRealMatrix
        // allocates it with the caller's type.
        let mut a = RealMatrix::null();
        assert_eq!(a.matrix_type(), MatrixType::Full);
        assert!(a.is_null());
        a.ensure(2, 3, MatrixType::Full, 1).unwrap();
        assert!(!a.is_null());
        assert_eq!(a.matrix_type(), MatrixType::Full);
        assert_eq!(a.get(2, 3).unwrap(), 0.0);
    }

    #[test]
    fn cpoint_markers_propagate() {
        let a = CPoint::new(1.0, 2.0, NOZ, NOW);
        let b = CPoint::new(3.0, 4.0, 5.0, 2.0);
        let c = CPoint::combine2(2.0, a, 1.0, b);
        assert_eq!(c, CPoint::new(5.0, 8.0, 5.0, NOW));
        assert_eq!(CPoint::sum(a, b).z, 5.0);
        assert_eq!(CPoint::diff(a, b).z, -5.0);
        assert_eq!(a.to_euclid(), Vec3::new(1.0, 2.0, 0.0));
        assert_eq!(b.to_euclid(), Vec3::new(1.5, 2.0, 2.5));
        assert_eq!(b.unweighted(), CPoint::new(1.5, 2.0, 2.5, NOW));
        assert_eq!(a.drop_w(), Vec3::new(1.0, 2.0, 0.0));
        assert_eq!(CPoint::weighted(Vec3::new(1.0, 2.0, 3.0), NOW).w, NOW);
        assert_eq!(
            CPoint::weighted(Vec3::new(1.0, 2.0, 3.0), 2.0),
            CPoint::new(2.0, 4.0, 6.0, 2.0)
        );
        assert_eq!(CPoint::dot_4d(a, b), 3.0 + 8.0 + 2.0);
        let mut d = b;
        d.blend(1.0, a);
        assert_eq!(d, CPoint::new(4.0, 6.0, NOZ, NOW));
        assert!(b.approx_eq(CPoint::new(3.0, 4.0, 5.0, 2.0 + 1e-9), MODEL_TOL));
        assert!(!b.approx_eq(CPoint::new(3.0, 4.0, 5.0, 2.1), MODEL_TOL));
        // Translation acts on the Euclidean point and keeps the weight.
        let t = b.translate(Vec3::new(1.0, 1.0, 1.0));
        assert_eq!(t, CPoint::new(5.0, 6.0, 7.0, 2.0));
        let s = b.scale_about(Vec3::ZERO, Vec3::new(2.0, 2.0, 2.0));
        assert_eq!(s, CPoint::new(6.0, 8.0, 10.0, 2.0));
    }

    #[test]
    fn transform_with_translation_matrix() {
        // Column-vector convention: translation in the last column.
        let m = full(&[
            &[1.0, 0.0, 0.0, 2.0],
            &[0.0, 1.0, 0.0, -1.0],
            &[0.0, 0.0, 1.0, 3.0],
            &[0.0, 0.0, 0.0, 1.0],
        ]);
        let p = CPoint::new(2.0, 4.0, 6.0, 2.0);
        assert_eq!(p.transform(&m).unwrap(), CPoint::new(6.0, 2.0, 12.0, 2.0));
        let small = full(&[&[1.0, 0.0], &[0.0, 1.0]]);
        assert!(matches!(
            p.transform(&small),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn quadratic_cases() {
        assert_eq!(
            solve_quadratic(1.0, -3.0, 2.0).unwrap(),
            QuadraticRoots::Two(1.0, 2.0)
        );
        assert_eq!(
            solve_quadratic(1.0, 2.0, 1.0).unwrap(),
            QuadraticRoots::One(-1.0)
        );
        assert_eq!(
            solve_quadratic(1.0, 1.0, 1.0).unwrap(),
            QuadraticRoots::None
        );
        assert_eq!(
            solve_quadratic(0.0, 0.0, 1.0).unwrap(),
            QuadraticRoots::None
        );
        assert_eq!(
            solve_quadratic(0.0, 2.0, -4.0).unwrap(),
            QuadraticRoots::One(2.0)
        );
        assert_eq!(
            solve_quadratic(-1.0, 5.0, 0.0).unwrap(),
            QuadraticRoots::Two(0.0, 5.0)
        );
        assert_eq!(solve_quadratic(1.0, 0.0, 4.0).unwrap().count(), 0);
        // Overflowing division reports NL_NUM_ERR.
        assert!(matches!(
            solve_quadratic(0.0, 1e-11, -1e300),
            Err(SolidError::Nlib {
                code: NlibCode::Numerical,
                ..
            })
        ));
        // NaN coefficients fail every tolerance test; the final division check rejects them.
        assert!(matches!(
            solve_quadratic(f64::NAN, 1.0, 1.0),
            Err(SolidError::Nlib {
                code: NlibCode::Numerical,
                ..
            })
        ));
    }

    #[test]
    fn float_op_edges() {
        use FloatOp::{Addition, Division, Multiplication, Subtraction};
        assert!(float_op_is_bad(f64::MAX, 2.0, Multiplication));
        assert!(!float_op_is_bad(f64::MAX, 0.5, Multiplication));
        assert!(float_op_is_bad(1e-200, 1e-200, Multiplication));
        assert!(float_op_is_bad(1.0, 0.0, Division));
        assert!(!float_op_is_bad(0.0, 3.0, Division));
        assert!(float_op_is_bad(f64::MAX, 0.5, Division));
        assert!(float_op_is_bad(f64::MAX, f64::MAX, Addition));
        assert!(!float_op_is_bad(f64::MAX, -f64::MAX, Addition));
        assert!(float_op_is_bad(f64::MAX, -f64::MAX, Subtraction));
        assert!(!float_op_is_bad(1.0, 2.0, Subtraction));
        // NaN: every comparison fails, so the final verdict is "bad".
        assert!(float_op_is_bad(f64::NAN, 2.0, Multiplication));
        assert!(float_op_is_bad(1.0, f64::NAN, Division));
        assert_eq!(Division.flag(), 4);
    }

    #[test]
    fn pascal_const_table_matches_rows() {
        let t = pascal_triangle(MAX_DEGREE).unwrap();
        for (k, row) in t.iter().enumerate() {
            for (j, &b) in row.iter().enumerate() {
                assert_eq!(PASCAL_TRI[k][j], b);
                assert_eq!(pascal_entry(k, j), Some(b as f64));
            }
            assert_eq!(pascal_entry(k, k + 1), None);
        }
        assert_eq!(pascal_entry(MAX_DEGREE + 1, 0), None);
    }

    #[test]
    fn pascal_rows() {
        let t = pascal_triangle(5).unwrap();
        assert_eq!(t[5], vec![1, 5, 10, 10, 5, 1]);
        assert_eq!(pascal_triangle(0).unwrap(), vec![vec![1]]);
        let big = pascal_triangle(66).unwrap();
        assert_eq!(big[66][33], 7_219_428_434_016_265_740);
        assert!(matches!(
            pascal_triangle(67),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn solve_with_pivoting_matches_upstream_test() {
        // prog_test my_test_matrix: x = (1, 2, 3).
        let mut a = full(&[&[1.0, 1.0, 2.0], &[2.0, 4.0, -3.0], &[3.0, 6.0, -5.0]]);
        let ind = a.lu_decompose_pivot().unwrap();
        let mut b = vec![9.0, 1.0, 0.0];
        a.for_back_pivot(&ind, &mut b).unwrap();
        for (x, e) in b.iter().zip([1.0, 2.0, 3.0]) {
            assert!(close(*x, e), "{b:?}");
        }
        let mut s = full(&[&[1.0, 1.0], &[2.0, 2.0]]);
        assert!(matches!(
            s.lu_decompose_pivot(),
            Err(SolidError::Nlib {
                code: NlibCode::Numerical,
                ..
            })
        ));
    }

    #[test]
    fn lu_inverse_and_least_squares() {
        let a0 = full(&[&[4.0, 1.0, 0.0], &[1.0, 4.0, 1.0], &[0.0, 1.0, 4.0]]);
        let mut a = a0.clone();
        let inv = a.inverse(false).unwrap();
        let id = a0.multiply(&inv).unwrap();
        for i in 0..3 {
            for j in 0..3 {
                let e = if i == j { 1.0 } else { 0.0 };
                assert!(close(id.get(i, j).unwrap(), e));
            }
        }
        // Same system banded (bw 3) gives the same LU factors.
        let mut band = RealMatrix::from_rows(
            2,
            2,
            vec![
                vec![0.0, 4.0, 1.0],
                vec![1.0, 4.0, 1.0],
                vec![1.0, 4.0, 0.0],
            ],
            MatrixType::Banded,
            3,
        )
        .unwrap();
        band.lu_decompose().unwrap();
        let mut f = a0.clone();
        f.lu_decompose().unwrap();
        for i in 0..3usize {
            for j in i.saturating_sub(1)..=(i + 1).min(2) {
                assert_eq!(band.get(i, j + 1 - i).unwrap(), f.get(i, j).unwrap());
            }
        }
        let x = band.right_for_back(&[5.0, 6.0, 5.0]).unwrap();
        for v in &x {
            assert!(close(*v, 1.0));
        }
        let pts = band
            .for_back_points(&[
                Vec3::new(5.0, 0.0, 0.0),
                Vec3::new(6.0, 0.0, 0.0),
                Vec3::new(5.0, 0.0, 0.0),
            ])
            .unwrap();
        assert!(pts.iter().all(|p| close(p.x, 1.0) && p.w == NOW));
        // Least squares line fit through exact data.
        let a = full(&[&[1.0, 0.0], &[1.0, 1.0], &[1.0, 2.0]]);
        let x = a.lst_sq_solve(&[1.0, 3.0, 5.0]).unwrap();
        assert!(close(x[0], 1.0) && close(x[1], 2.0));
        assert!(matches!(
            full(&[&[1.0, 0.0], &[0.0, 1.0]]).lst_sq_solve(&[1.0, 1.0]),
            Err(SolidError::Nlib {
                code: NlibCode::Index,
                ..
            })
        ));
    }

    #[test]
    fn multiply_transpose_and_storage() {
        let ll = RealMatrix::from_rows(
            1,
            1,
            vec![vec![2.0, 99.0], vec![1.0, 3.0]],
            MatrixType::LowerLeft,
            1,
        )
        .unwrap();
        let p = ll.multiply(&ll).unwrap();
        assert_eq!(p.matrix_type(), MatrixType::LowerLeft);
        assert_eq!(
            (
                p.get(0, 0).unwrap(),
                p.get(1, 0).unwrap(),
                p.get(1, 1).unwrap()
            ),
            (4.0, 5.0, 9.0)
        );
        let mut t = ll.clone();
        t.transpose_in_place().unwrap();
        assert_eq!(t.matrix_type(), MatrixType::UpperRight);
        assert_eq!(t.get(0, 1).unwrap(), 1.0);
        let a = full(&[&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]]);
        let at = a.transpose().unwrap();
        assert_eq!(at.max_index(), (2, 1));
        assert_eq!(at.get(2, 1).unwrap(), 6.0);
        // Too-small output: NL_STO_ERR; bandwidth 0 counts as null and is reallocated.
        let mut out = RealMatrix::new(0, 0, MatrixType::Full, 1).unwrap();
        assert!(matches!(
            a.multiply_into(&at, &mut out),
            Err(SolidError::Nlib {
                code: NlibCode::Storage,
                ..
            })
        ));
        let mut out = RealMatrix::new(0, 0, MatrixType::Full, 0).unwrap();
        assert!(out.is_null());
        a.multiply_into(&at, &mut out).unwrap();
        assert_eq!(out.get(1, 1).unwrap(), 16.0 + 25.0 + 36.0);
        assert!(RealMatrix::null().is_null());
        assert_eq!(
            RealMatrix::new(3, 3, MatrixType::Banded, 1)
                .unwrap()
                .storage_shape(),
            Some((4, 2))
        );
        assert!(matches!(
            a.multiply(&a),
            Err(SolidError::Nlib {
                code: NlibCode::Index,
                ..
            })
        ));
        let q = a.multiply_points(&[Vec3::X, Vec3::Y, Vec3::Z]).unwrap();
        assert_eq!(q, vec![Vec3::new(1.0, 2.0, 3.0), Vec3::new(4.0, 5.0, 6.0)]);
        assert!(matches!(
            a.multiply_points(&[Vec3::X]),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn power_basis_surface() {
        // S(u,v) = (u, v, u*v) as power-basis coefficients.
        let c = |x: f64, y: f64, z: f64| CPoint::new(x, y, z, NOW);
        let net = vec![
            vec![c(0.0, 0.0, 0.0), c(0.0, 1.0, 0.0)],
            vec![c(1.0, 0.0, 0.0), c(0.0, 0.0, 1.0)],
        ];
        let s = srf_power_basis_eval_pt(&net, 0.25, 0.5).unwrap();
        assert_eq!(s, Vec3::new(0.25, 0.5, 0.125));
        let sd = srf_power_basis_eval_derivs(&net, 1, 1, 0.25, 0.5, false, 1, 1).unwrap();
        assert_eq!(sd[0][0], s);
        assert_eq!(sd[1][0], Vec3::new(1.0, 0.0, 0.5));
        assert_eq!(sd[0][1], Vec3::new(0.0, 1.0, 0.25));
        assert_eq!(sd[1][1], Vec3::new(0.0, 0.0, 1.0));
        // Rational with constant weight 2 (all coefficients doubled, w only at [0][0]).
        let r = |x: f64, y: f64, z: f64, w: f64| CPoint::new(x, y, z, w);
        let rnet = vec![
            vec![r(0.0, 0.0, 0.0, 2.0), r(0.0, 2.0, 0.0, 0.0)],
            vec![r(2.0, 0.0, 0.0, 0.0), r(0.0, 0.0, 2.0, 0.0)],
        ];
        let rd = srf_power_basis_eval_derivs(&rnet, 1, 1, 0.25, 0.5, false, 1, 1).unwrap();
        assert_eq!(rd, sd);
        assert!(matches!(
            srf_power_basis_eval_derivs(&net, 1, 1, 0.0, 0.0, false, 5, 0),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
        // Scratch-array overflow upstream: InvalidInput here.
        let big = vec![vec![c(0.0, 0.0, 0.0); 6]; 6];
        assert!(matches!(
            srf_power_basis_eval_derivs(&big, 5, 5, 0.5, 0.5, false, 3, 3),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn realloc_keeps_prefix() {
        let mut a = vec![1.0, 2.0, 3.0];
        realloc_1d(&mut a, 2, 4).unwrap();
        assert_eq!(a, vec![1.0, 2.0, 3.0, 0.0, 0.0]);
        realloc_1d(&mut a, 4, 1).unwrap();
        assert_eq!(a, vec![1.0, 2.0]);
        let mut b = vec![vec![1i16, 2], vec![3, 4]];
        realloc_2d(&mut b, 1, 1, 2, 0).unwrap();
        assert_eq!(b, vec![vec![1], vec![3], vec![0]]);
        assert!(realloc_1d(&mut vec![1.0], 3, 3).is_err());
    }

    /// Fuzz regression: a triangular matrix with a huge logical column count over
    /// tiny storage is transposed without walking the empty outer iterations.
    #[test]
    fn transpose_triangular_huge_logical_size() {
        let mut a = RealMatrix::from_rows(
            0,
            usize::MAX >> 1,
            vec![vec![2.0]],
            MatrixType::UpperRight,
            0,
        )
        .unwrap();
        a.transpose_in_place().unwrap();
        assert_eq!(a.get(0, 0).unwrap(), 2.0);
    }
}
