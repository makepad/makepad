//! Dense row-major matrices of `f64` used to solve small equation systems: LU
//! decomposition with partial pivoting, linear solves, and the Jacobi eigen solver
//! for real symmetric matrices.
//!
//! Upstream: `source/SMLib/inc/SmMatrix.h`, `source/SMLib/src/SmMatrix.cpp`.
//!
//! | Rust                                   | Upstream                                   |
//! |----------------------------------------|--------------------------------------------|
//! | [`Matrix`]                             | `SmMatrix`                                 |
//! | [`Matrix::new`], [`Matrix::try_new`]   | `SmMatrix(ULONG, ULONG)`                   |
//! | [`Clone`] / [`Drop`] (derived)         | `SmMatrix(const SmMatrix&)`, `~SmMatrix`   |
//! | [`Matrix::from_row_major`]             | (none: constructor from values, for callers and tests) |
//! | [`Matrix::num_rows`], [`Matrix::num_columns`] | `GetNumRows`, `GetNumColumns`       |
//! | [`Matrix::at`]                         | `GetAt`                                    |
//! | [`Matrix::set_at`]                     | `SetAt`                                    |
//! | [`Matrix::add_to`]                     | `AddTo`                                    |
//! | [`Matrix::row`], [`Matrix::row_mut`]   | `operator[]` (const / non-const)           |
//! | [`Matrix::as_slice`]                   | (none: the row-major element array `m_pdM`) |
//! | [`Matrix::set_size`]                   | `SetSize`                                  |
//! | [`Matrix::is_symmetric`]               | `IsSymmetric`                              |
//! | [`Matrix::clear`]                      | `Clear`                                    |
//! | [`Matrix::make_identity`]              | `MakeIdentity`                             |
//! | [`Matrix::lu_decompose`]               | `LUDecompose`                              |
//! | [`Matrix::solve_with_lu`]              | `SolveWithLUMatrix`                        |
//! | [`Matrix::solve_linear_system`]        | `SolveLinearSystem`                        |
//! | [`Matrix::eigen_system_jacobi`]        | `EigenSystemJacobiSolver`                  |
//! | [`Matrix::compute_3x3_eigen_vectors`]  | `Compute3x3EigenVectors`                   |
//! | `sort_eigen_system` (private)          | `sm_SortEigenSystem`                       |
//!
//! The other `SmMatrix` members (`Add`, `Multiply`, `Invert`, `Transpose`, SVD,
//! `Rank`, `MakeRowEchelon`, `SwapRows`, 3x3 determinant / eigenvalues, ...) are not
//! reached by any upstream test entry point and are not ported (see
//! `harness/callgraph/units.json`).
//!
//! Deviations:
//! - Storage is one `Vec<f64>` (row-major, as upstream `m_pdM`); there are no row
//!   pointers and no 6x6 inline buffer. [`Matrix::new`] and [`Matrix::set_size`]
//!   zero the elements where upstream leaves small matrices uninitialized (large
//!   ones are `calloc`ed); `set_size` to the current size keeps the values, as
//!   upstream.
//! - [`Matrix::try_new`] (caller-chosen sizes) and [`Matrix::set_size`] return
//!   `InvalidInput` when `rows * cols` overflows, storage exceeds
//!   [`crate::limits::MAX_ALLOC_BYTES`], or cannot be allocated, leaving the
//!   matrix unchanged (upstream's `smos_Calloc` returns `NULL` and the row-pointer
//!   loop writes through it). [`Matrix::new`] is for small sizes fixed at the call
//!   site.
//! - Element access outside the matrix returns `InvalidInput` (upstream asserts).
//! - Status out-params become return values: `LUDecompose`'s `SM_ERR_WARNING` (a
//!   pivot below `SM_EFF_ZERO` was replaced by `±SM_EFF_ZERO`) is
//!   [`LuDecomposition::near_singular`] / [`LinearSolution::near_singular`]; the
//!   row-swap index array and the solution vector are returned. Other statuses are
//!   [`SolidError::InvalidInput`] (`SM_ERR_INVALID_INPUT`) or
//!   [`SolidError::Upstream`] with code `1001` (`SM_ERR`).
//! - `LUDecompose` uses a fixed `MAX_MATRIX_SIZE` (1000) scale buffer on the stack
//!   (larger matrices overflow it); here it is a `Vec`. `SolveLinearSystem` keeps
//!   its explicit size check.
//! - [`Matrix::solve_with_lu`] checks the index array and right-hand side lengths
//!   (upstream reads past them) and requires every index to be a row of the matrix.
//! - `Compute3x3EigenVectors` is `const` upstream but casts `this` to non-const and
//!   runs the Jacobi solver on it, so [`Matrix::compute_3x3_eigen_vectors`] takes
//!   `&mut self`. Upstream ignores the solver's status: when the matrix is not
//!   symmetric it returns `SM_SUCCESS` with the caller's eigenvalue array untouched
//!   and eigenvectors read from an uninitialized matrix; the port returns `SM_ERR`
//!   there, and `InvalidInput` for a matrix that is not 3x3 (upstream reads out of
//!   bounds). When the solver does not converge, the unconverged values are
//!   returned with success, as upstream.

use crate::error::{Result, SolidError};
use crate::geom::tol::is_zero;
use crate::geom::vector3d::{sqrt, Vec3};
use crate::math::EFF_ZERO;

/// Upstream status `SM_ERR`.
const SM_ERR: i32 = 1001;

/// `MAX_MATRIX_SIZE`: the largest system [`Matrix::solve_linear_system`] accepts.
pub const MAX_MATRIX_SIZE: usize = 1000;

/// Sweeps of the Jacobi eigen solver before it gives up.
const JACOBI_MAX_SWEEPS: usize = 50;

fn out_of_range(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "matrix index out of range",
    }
}

fn sm_err(operation: &'static str) -> SolidError {
    SolidError::Upstream {
        operation,
        code: SM_ERR,
        entity: None,
    }
}

/// A dense `rows x cols` matrix stored row-major (`SmMatrix`).
#[derive(Debug, Clone, PartialEq)]
pub struct Matrix {
    rows: usize,
    cols: usize,
    data: Vec<f64>,
}

/// Result of [`Matrix::lu_decompose`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuDecomposition {
    /// Row interchanged with each column's pivot row (`plIndx`), one per column.
    pub index: Vec<usize>,
    /// A pivot was below `SM_EFF_ZERO` and replaced by `±SM_EFF_ZERO`
    /// (upstream status `SM_ERR_WARNING`).
    pub near_singular: bool,
}

/// Result of [`Matrix::solve_linear_system`].
#[derive(Debug, Clone, PartialEq)]
pub struct LinearSolution {
    /// The solution `x` of `Ax = b`.
    pub x: Vec<f64>,
    /// The decomposition hit a near-zero pivot (upstream status `SM_ERR_WARNING`);
    /// `x` is still computed.
    pub near_singular: bool,
}

/// Result of [`Matrix::eigen_system_jacobi`].
#[derive(Debug, Clone, PartialEq)]
pub struct EigenSystem {
    /// Eigenvalues in ascending order.
    pub values: Vec<f64>,
    /// Eigenvectors: column `i` belongs to `values[i]`.
    pub vectors: Matrix,
    /// Number of Jacobi rotations performed.
    pub rotations: u64,
}

/// Raw Jacobi solver state (converged or not).
struct JacobiState {
    values: Vec<f64>,
    vectors: Matrix,
    rotations: u64,
    converged: bool,
}

impl Matrix {
    /// A `rows x cols` matrix of zeros (`SmMatrix(ULONG, ULONG)`; upstream leaves
    /// small matrices uninitialized).
    ///
    /// For small sizes known at the call site. Like any `Vec`, it aborts when the
    /// elements cannot be allocated; use [`Matrix::try_new`] for sizes that come
    /// from caller input.
    #[must_use]
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            data: vec![0.0; rows.saturating_mul(cols)],
        }
    }

    /// A `rows x cols` matrix of zeros (`SmMatrix(ULONG, ULONG)`) for sizes that
    /// come from caller input.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when `rows * cols` overflows, storage exceeds
    /// [`crate::limits::MAX_ALLOC_BYTES`], or the elements cannot be allocated
    /// (upstream's `smos_Calloc` then returns `NULL` and the constructor writes
    /// through it).
    pub fn try_new(rows: usize, cols: usize) -> Result<Self> {
        const OP: &str = "geom::matrix::Matrix::try_new";
        let len = rows.checked_mul(cols).ok_or(SolidError::InvalidInput {
            operation: OP,
            reason: "element count overflows",
        })?;
        let data = crate::limits::try_filled(len, 0.0).ok_or(SolidError::InvalidInput {
            operation: OP,
            reason: "matrix too large to allocate",
        })?;
        Ok(Self { rows, cols, data })
    }

    /// A `rows x cols` matrix holding `data` in row-major order, or `InvalidInput`
    /// when `data.len() != rows * cols`.
    pub fn from_row_major(rows: usize, cols: usize, data: Vec<f64>) -> Result<Self> {
        if rows.checked_mul(cols) != Some(data.len()) {
            return Err(SolidError::InvalidInput {
                operation: "geom::matrix::Matrix::from_row_major",
                reason: "element count is not rows * cols",
            });
        }
        Ok(Self { rows, cols, data })
    }

    /// Row count (`GetNumRows`).
    #[must_use]
    pub const fn num_rows(&self) -> usize {
        self.rows
    }

    /// Column count (`GetNumColumns`).
    #[must_use]
    pub const fn num_columns(&self) -> usize {
        self.cols
    }

    /// The elements in row-major order (upstream `m_pdM`).
    #[must_use]
    pub fn as_slice(&self) -> &[f64] {
        &self.data
    }

    fn offset(&self, row: usize, col: usize) -> Option<usize> {
        if row < self.rows && col < self.cols {
            Some(row * self.cols + col)
        } else {
            None
        }
    }

    fn slot(&mut self, row: usize, col: usize, op: &'static str) -> Result<&mut f64> {
        let k = self.offset(row, col).ok_or_else(|| out_of_range(op))?;
        self.data.get_mut(k).ok_or_else(|| out_of_range(op))
    }

    fn get(&self, row: usize, col: usize, op: &'static str) -> Result<f64> {
        self.offset(row, col)
            .and_then(|k| self.data.get(k).copied())
            .ok_or_else(|| out_of_range(op))
    }

    fn put(&mut self, row: usize, col: usize, v: f64, op: &'static str) -> Result<()> {
        *self.slot(row, col, op)? = v;
        Ok(())
    }

    /// Element `[row][col]` (`GetAt`).
    pub fn at(&self, row: usize, col: usize) -> Result<f64> {
        self.get(row, col, "geom::matrix::Matrix::at")
    }

    /// Sets element `[row][col]` (`SetAt`).
    pub fn set_at(&mut self, row: usize, col: usize, value: f64) -> Result<()> {
        self.put(row, col, value, "geom::matrix::Matrix::set_at")
    }

    /// Adds `value` to element `[row][col]` (`AddTo`).
    pub fn add_to(&mut self, row: usize, col: usize, value: f64) -> Result<()> {
        *self.slot(row, col, "geom::matrix::Matrix::add_to")? += value;
        Ok(())
    }

    /// Row `row` (`operator[]` const), `None` out of range.
    #[must_use]
    pub fn row(&self, row: usize) -> Option<&[f64]> {
        if row >= self.rows {
            return None;
        }
        let start = row * self.cols;
        self.data.get(start..start + self.cols)
    }

    /// Row `row`, mutable (`operator[]`), `None` out of range.
    pub fn row_mut(&mut self, row: usize) -> Option<&mut [f64]> {
        if row >= self.rows {
            return None;
        }
        let start = row * self.cols;
        let cols = self.cols;
        self.data.get_mut(start..start + cols)
    }

    /// Resizes to `rows x cols` (`SetSize`). The same size keeps the values;
    /// otherwise every element is zero (upstream: uninitialized).
    ///
    /// # Errors
    ///
    /// As [`Matrix::try_new`]; the matrix is then unchanged.
    pub fn set_size(&mut self, rows: usize, cols: usize) -> Result<()> {
        if rows == self.rows && cols == self.cols {
            return Ok(());
        }
        *self = Self::try_new(rows, cols)?;
        Ok(())
    }

    /// True when the matrix is square, non-empty and every off-diagonal pair agrees
    /// within `SM_EFF_ZERO` (`IsSymmetric`).
    #[must_use]
    pub fn is_symmetric(&self) -> bool {
        const OP: &str = "geom::matrix::Matrix::is_symmetric";
        // row by row, stopping at the first pair that differs (NaN differs)
        self.rows == self.cols
            && self.rows > 0
            && (0..self.rows).all(|ii| {
                (ii + 1..self.cols).all(|jj| {
                    matches!(
                        (self.get(ii, jj, OP), self.get(jj, ii, OP)),
                        (Ok(a), Ok(b)) if is_zero(a - b)
                    )
                })
            })
    }

    /// Sets every element to zero (`Clear`).
    pub fn clear(&mut self) {
        self.data.fill(0.0);
    }

    /// Makes the matrix the identity (`MakeIdentity`); `SM_ERR` when not square.
    pub fn make_identity(&mut self) -> Result<()> {
        if self.rows != self.cols {
            return Err(sm_err("geom::matrix::Matrix::make_identity"));
        }
        self.clear();
        let step = self.rows + 1;
        for v in self.data.iter_mut().step_by(step) {
            *v = 1.0;
        }
        Ok(())
    }

    /// Crout LU decomposition with scaled partial pivoting, in place
    /// (`LUDecompose`, Numerical Recipes `ludcmp`).
    ///
    /// Errors: `InvalidInput` when not square; `SM_ERR` when a row's largest
    /// magnitude is below `SM_EFF_ZERO` (the matrix is then unchanged). A pivot below
    /// `SM_EFF_ZERO` is replaced by `±SM_EFF_ZERO` (sign kept, `-` for exactly zero)
    /// and reported through [`LuDecomposition::near_singular`].
    pub fn lu_decompose(&mut self) -> Result<LuDecomposition> {
        const OP: &str = "geom::matrix::Matrix::lu_decompose";
        if self.rows != self.cols {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "matrix is not square",
            });
        }
        let n = self.rows;
        let zero_tol = EFF_ZERO;
        let mut near_singular = false;
        let mut scale = vec![0.0; n];
        let mut index = vec![0usize; n];

        // for every row - get 1.0/MaxTerm value (SM_ERR for any zero row)
        for (row, sc) in scale.iter_mut().enumerate() {
            let mut big = 0.0;
            for col in 0..n {
                let tmp = self.get(row, col, OP)?.abs();
                if tmp > big {
                    big = tmp;
                }
            }
            if big < zero_tol {
                return Err(sm_err(OP));
            }
            *sc = 1.0 / big;
        }

        for col in 0..n {
            for row in 0..col {
                let mut sum = self.get(row, col, OP)?;
                for kk in 0..row {
                    sum -= self.get(row, kk, OP)? * self.get(kk, col, OP)?;
                }
                self.put(row, col, sum, OP)?;
            }

            // search for largest pivot element
            let mut big = 0.0;
            let mut l_max = 0usize;
            for row in col..n {
                let mut sum = self.get(row, col, OP)?;
                for kk in 0..col {
                    sum -= self.get(row, kk, OP)? * self.get(kk, col, OP)?;
                }
                self.put(row, col, sum, OP)?;

                let dum = scale.get(row).copied().ok_or_else(|| out_of_range(OP))? * sum.abs();
                if dum >= big {
                    big = dum;
                    l_max = row;
                }
            }

            // interchange rows
            if col != l_max {
                for kk in 0..n {
                    let tmp = self.get(l_max, kk, OP)?;
                    let other = self.get(col, kk, OP)?;
                    self.put(l_max, kk, other, OP)?;
                    self.put(col, kk, tmp, OP)?;
                }
                let sc = scale.get(col).copied().ok_or_else(|| out_of_range(OP))?;
                *scale.get_mut(l_max).ok_or_else(|| out_of_range(OP))? = sc;
            }

            *index.get_mut(col).ok_or_else(|| out_of_range(OP))? = l_max;

            // Replace a tiny pivot by +-SM_EFF_ZERO, preserving its sign.
            let pivot = self.get(col, col, OP)?;
            if pivot.abs() < zero_tol {
                let v = if pivot > 0.0 { EFF_ZERO } else { -EFF_ZERO };
                self.put(col, col, v, OP)?;
                near_singular = true;
            }

            // scale rows
            if col != n - 1 {
                let tmp = 1.0 / self.get(col, col, OP)?;
                for kk in col + 1..n {
                    *self.slot(kk, col, OP)? *= tmp;
                }
            }
        }

        Ok(LuDecomposition {
            index,
            near_singular,
        })
    }

    /// Solves `Ax = b` with this matrix holding the LU decomposition from
    /// [`Matrix::lu_decompose`] and its row-swap `index` (`SolveWithLUMatrix`).
    ///
    /// The solution has `rhs.len()` entries; entries past the matrix size are copied
    /// from `rhs`. Errors: `InvalidInput` when `index` or `rhs` is shorter than the
    /// matrix or an index is not a row; `SM_ERR` when a diagonal element is below
    /// `SM_EFF_ZERO`.
    pub fn solve_with_lu(&self, index: &[usize], rhs: &[f64]) -> Result<Vec<f64>> {
        const OP: &str = "geom::matrix::Matrix::solve_with_lu";
        let n = self.rows;
        if index.len() < n || rhs.len() < n || index.iter().take(n).any(|&i| i >= n) {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "index or right-hand side does not match the matrix",
            });
        }
        let zero_tol = EFF_ZERO;
        let mut x = rhs.to_vec();
        let xi = |x: &[f64], i: usize| x.get(i).copied().ok_or_else(|| out_of_range(OP));

        // forward substitution
        let mut mm: Option<usize> = None;
        for (row, &row_per) in index.iter().enumerate().take(n) {
            let mut sum = xi(&x, row_per)?;
            let at_row = xi(&x, row)?;
            *x.get_mut(row_per).ok_or_else(|| out_of_range(OP))? = at_row;
            if let Some(m) = mm {
                for jj in m..row {
                    sum -= self.get(row, jj, OP)? * xi(&x, jj)?;
                }
            } else if sum != 0.0 {
                mm = Some(row);
            }
            *x.get_mut(row).ok_or_else(|| out_of_range(OP))? = sum;
        }

        // backward elimination
        for row in (0..n).rev() {
            let mut sum = xi(&x, row)?;
            for col in row + 1..self.cols {
                sum -= self.get(row, col, OP)? * xi(&x, col)?;
            }
            let denom = self.get(row, row, OP)?;
            if denom.abs() < zero_tol {
                return Err(sm_err(OP));
            }
            *x.get_mut(row).ok_or_else(|| out_of_range(OP))? = sum / denom;
        }
        Ok(x)
    }

    /// Solves `Ax = b` by LU decomposition of a copy of the matrix
    /// (`SolveLinearSystem`).
    ///
    /// Errors: `InvalidInput` when the matrix is not square, `rhs` does not match it,
    /// or it has more than [`MAX_MATRIX_SIZE`] rows; `SM_ERR` when the decomposition
    /// or the back substitution fails. A near-zero pivot is not an error (see
    /// [`LinearSolution::near_singular`]).
    pub fn solve_linear_system(&self, rhs: &[f64]) -> Result<LinearSolution> {
        const OP: &str = "geom::matrix::Matrix::solve_linear_system";
        if self.rows != self.cols {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "matrix is not square",
            });
        }
        if rhs.len() != self.rows {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "right-hand side size differs from the matrix size",
            });
        }
        if self.rows > MAX_MATRIX_SIZE {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "matrix larger than MAX_MATRIX_SIZE",
            });
        }
        let mut tmp = self.clone();
        let lu = tmp.lu_decompose().map_err(|_| sm_err(OP))?;
        let x = tmp.solve_with_lu(&lu.index, rhs)?;
        Ok(LinearSolution {
            x,
            near_singular: lu.near_singular,
        })
    }

    /// Eigenvalues (ascending) and eigenvectors of a real symmetric matrix by
    /// cyclic Jacobi rotations (`EigenSystemJacobiSolver`, Numerical Recipes
    /// `jacobi`).
    ///
    /// The matrix is used as work space: on success its upper triangle is restored
    /// from the lower triangle; when the solver fails to converge it is left rotated.
    /// Errors: `SM_ERR` when the matrix is not symmetric ([`Matrix::is_symmetric`])
    /// or the off-diagonal sum is not below `SM_EFF_ZERO` after 50 sweeps.
    pub fn eigen_system_jacobi(&mut self) -> Result<EigenSystem> {
        const OP: &str = "geom::matrix::Matrix::eigen_system_jacobi";
        let st = self.jacobi()?.ok_or_else(|| sm_err(OP))?;
        if !st.converged {
            return Err(sm_err(OP));
        }
        Ok(EigenSystem {
            values: st.values,
            vectors: st.vectors,
            rotations: st.rotations,
        })
    }

    /// The Jacobi solver; `None` when the matrix is not symmetric, else its final
    /// state (sorted and with `self` restored only when converged).
    fn jacobi(&mut self) -> Result<Option<JacobiState>> {
        const OP: &str = "geom::matrix::Matrix::eigen_system_jacobi";
        if !self.is_symmetric() {
            return Ok(None);
        }
        let n = self.rows;
        let mut d = vec![0.0; n];
        let mut v = Self::new(n, n);
        v.make_identity()?;
        let mut rotations: u64 = 0;
        let mut b = vec![0.0; n];
        let mut z = vec![0.0; n];

        for ip in 0..n {
            let a_pp = self.get(ip, ip, OP)?;
            *b.get_mut(ip).ok_or_else(|| out_of_range(OP))? = a_pp;
            *d.get_mut(ip).ok_or_else(|| out_of_range(OP))? = a_pp;
        }

        let dv = |d: &[f64], i: usize| d.get(i).copied().ok_or_else(|| out_of_range(OP));

        for i in 0..JACOBI_MAX_SWEEPS {
            // sum of the upper off-diagonal magnitudes
            let mut sm = 0.0;
            for ip in 0..n.saturating_sub(1) {
                for iq in ip + 1..n {
                    sm += self.get(ip, iq, OP)?.abs();
                }
            }

            // normal exit relying on quadratic convergence
            if sm < EFF_ZERO {
                sort_eigen_system(&mut d, &mut v);
                for ip in 0..n.saturating_sub(1) {
                    for iq in ip + 1..n {
                        let lower = self.get(iq, ip, OP)?;
                        self.put(ip, iq, lower, OP)?;
                    }
                }
                return Ok(Some(JacobiState {
                    values: d,
                    vectors: v,
                    rotations,
                    converged: true,
                }));
            }

            let tresh = if i < 3 {
                0.2 * sm / ((n * n) as f64)
            } else {
                0.0
            };

            for ip in 0..n {
                for iq in ip + 1..n {
                    let a_pq = self.get(ip, iq, OP)?;
                    let g = 100.0 * a_pq.abs();
                    let d_p = dv(&d, ip)?;
                    let d_q = dv(&d, iq)?;

                    // after four sweeps, skip the rotation if the off-diagonal element is small
                    if i >= 3
                        && is_zero((d_p.abs() + g) - d_p.abs())
                        && is_zero((d_q.abs() + g) - d_q.abs())
                    {
                        self.put(ip, iq, 0.0, OP)?;
                    } else if a_pq.abs() > tresh {
                        let mut h = d_q - d_p;
                        let mut t;
                        if is_zero((h.abs() + g) - h.abs()) {
                            t = a_pq / h;
                        } else {
                            let theta = h / 2.0 / a_pq;
                            t = 1.0 / (theta.abs() + sqrt(1.0 + theta * theta));
                            if theta < 0.0 {
                                t = -t;
                            }
                        }
                        let c = 1.0 / sqrt(1.0 + t * t);
                        let s = t * c;
                        let tau = s / (1.0 + c);
                        h = t * a_pq;
                        *z.get_mut(ip).ok_or_else(|| out_of_range(OP))? -= h;
                        *z.get_mut(iq).ok_or_else(|| out_of_range(OP))? += h;
                        *d.get_mut(ip).ok_or_else(|| out_of_range(OP))? -= h;
                        *d.get_mut(iq).ok_or_else(|| out_of_range(OP))? += h;
                        self.put(ip, iq, 0.0, OP)?;

                        for j in 0..ip {
                            self.rotate(s, tau, (j, ip), (j, iq))?;
                        }
                        for j in ip + 1..iq {
                            self.rotate(s, tau, (ip, j), (j, iq))?;
                        }
                        for j in iq + 1..n {
                            self.rotate(s, tau, (ip, j), (iq, j))?;
                        }
                        for j in 0..n {
                            v.rotate(s, tau, (j, ip), (j, iq))?;
                        }
                        rotations += 1;
                    }
                }
            }

            // update d with the sum of t*a[p][q] and reinit z
            for ((bi, di), zi) in b.iter_mut().zip(d.iter_mut()).zip(z.iter_mut()) {
                *bi += *zi;
                *di = *bi;
                *zi = 0.0;
            }
        }

        Ok(Some(JacobiState {
            values: d,
            vectors: v,
            rotations,
            converged: false,
        }))
    }

    /// The `ROTATE(a,i,j,k,l)` macro of `EigenSystemJacobiSolver`.
    fn rotate(&mut self, s: f64, tau: f64, ij: (usize, usize), kl: (usize, usize)) -> Result<()> {
        const OP: &str = "geom::matrix::Matrix::eigen_system_jacobi";
        let g = self.get(ij.0, ij.1, OP)?;
        let h = self.get(kl.0, kl.1, OP)?;
        self.put(ij.0, ij.1, g - s * (h + g * tau), OP)?;
        self.put(kl.0, kl.1, h + s * (g - h * tau), OP)
    }

    /// Eigenvalues (ascending) and eigenvectors of a symmetric 3x3 matrix through
    /// the Jacobi solver (`Compute3x3EigenVectors`). Like upstream it modifies the
    /// matrix as [`Matrix::eigen_system_jacobi`] does and returns the unconverged
    /// state when the solver runs out of sweeps.
    ///
    /// Errors: `InvalidInput` when not 3x3; `SM_ERR` when not symmetric (upstream
    /// returns uninitialized vectors there).
    pub fn compute_3x3_eigen_vectors(&mut self) -> Result<([f64; 3], [Vec3; 3])> {
        const OP: &str = "geom::matrix::Matrix::compute_3x3_eigen_vectors";
        if self.rows != 3 || self.cols != 3 {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "matrix is not 3x3",
            });
        }
        let st = self.jacobi()?.ok_or_else(|| sm_err(OP))?;
        let vv = &st.vectors;
        let value = |i: usize| st.values.get(i).copied().ok_or_else(|| out_of_range(OP));
        let values = [value(0)?, value(1)?, value(2)?];
        let col = |c: usize| -> Result<Vec3> {
            Ok(Vec3::new(
                vv.get(0, c, OP)?,
                vv.get(1, c, OP)?,
                vv.get(2, c, OP)?,
            ))
        };
        Ok((values, [col(0)?, col(1)?, col(2)?]))
    }
}

/// Sorts eigenvalues ascending, swapping the matching eigenvector columns
/// (`sm_SortEigenSystem`, a selection sort). Does nothing when the sizes disagree
/// (upstream returns `SM_ERR`, which its only caller ignores).
fn sort_eigen_system(d: &mut [f64], v: &mut Matrix) {
    let n = d.len();
    if n != v.rows || n != v.cols {
        return;
    }
    for i in 0..n.saturating_sub(1) {
        let Some(&d_i) = d.get(i) else { return };
        let mut d_min = d_i;
        let mut j_min = i;
        for (j, &d_j) in d.iter().enumerate().skip(i + 1) {
            if d_j < d_min {
                d_min = d_j;
                j_min = j;
            }
        }
        if d_min != d_i {
            d.swap(i, j_min);
            for j in 0..n {
                let (Some(a), Some(b)) = (v.offset(j, i), v.offset(j, j_min)) else {
                    return;
                };
                v.data.swap(a, b);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(rows: usize, cols: usize, v: &[f64]) -> Matrix {
        Matrix::from_row_major(rows, cols, v.to_vec()).unwrap()
    }

    #[test]
    fn upstream_my_test_matrix() {
        // tests/prog_test/src/smCurveTest.cpp my_test_matrix
        let a = m(2, 2, &[1.0, 1.0, 2.0, 2.0]);
        let s = a.solve_linear_system(&[4.0, 6.0]).unwrap();
        assert!(s.near_singular);
        let a = m(2, 2, &[1.0, 2.0, 2.0, 2.0]);
        let s = a.solve_linear_system(&[4.0, 6.0]).unwrap();
        assert!(!s.near_singular);
        assert!((s.x[0] - 2.0).abs() < 1e-12 && (s.x[1] - 1.0).abs() < 1e-12);
        let a = m(3, 3, &[1.0, 1.0, 2.0, 2.0, 4.0, -3.0, 3.0, 6.0, -5.0]);
        let s = a.solve_linear_system(&[9.0, 1.0, 0.0]).unwrap();
        for (x, e) in s.x.iter().zip([1.0, 2.0, 3.0]) {
            assert!((x - e).abs() < 1e-12);
        }
    }

    #[test]
    fn solve_errors() {
        let a = m(2, 3, &[1.0; 6]);
        assert!(matches!(
            a.solve_linear_system(&[1.0, 2.0]),
            Err(SolidError::InvalidInput { .. })
        ));
        let a = m(2, 2, &[1.0, 0.0, 0.0, 1.0]);
        assert!(matches!(
            a.solve_linear_system(&[1.0]),
            Err(SolidError::InvalidInput { .. })
        ));
        let z = m(2, 2, &[1.0, 2.0, 0.0, 0.0]);
        assert!(matches!(
            z.solve_linear_system(&[1.0, 2.0]),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
        let big = Matrix::new(MAX_MATRIX_SIZE + 1, MAX_MATRIX_SIZE + 1);
        assert!(matches!(
            big.solve_linear_system(&vec![0.0; MAX_MATRIX_SIZE + 1]),
            Err(SolidError::InvalidInput { .. })
        ));
        let empty = Matrix::new(0, 0);
        assert_eq!(empty.solve_linear_system(&[]).unwrap().x, Vec::<f64>::new());
    }

    #[test]
    fn lu_zero_row_leaves_matrix() {
        let mut a = m(2, 2, &[3.0, 1.0, 0.0, 1e-13]);
        let before = a.clone();
        assert!(a.lu_decompose().is_err());
        assert_eq!(a, before);
        let mut r = m(2, 3, &[0.0; 6]);
        assert!(matches!(
            r.lu_decompose(),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn lu_round_trip() {
        let a = m(3, 3, &[0.0, 2.0, 1.0, 4.0, 1.0, 0.0, 1.0, 1.0, 5.0]);
        let mut lu = a.clone();
        let d = lu.lu_decompose().unwrap();
        let x = lu.solve_with_lu(&d.index, &[3.0, 5.0, 7.0]).unwrap();
        for r in 0..3 {
            let row = a.row(r).unwrap();
            let ax: f64 = row.iter().zip(&x).map(|(p, q)| p * q).sum();
            assert!((ax - [3.0, 5.0, 7.0][r]).abs() < 1e-12);
        }
        assert!(lu.solve_with_lu(&[5, 0, 0], &[1.0, 2.0, 3.0]).is_err());
        assert!(lu.solve_with_lu(&d.index, &[1.0]).is_err());
    }

    #[test]
    fn identity_access_resize() {
        let mut a = Matrix::new(3, 3);
        a.make_identity().unwrap();
        assert_eq!(a.as_slice(), &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
        assert!(a.is_symmetric());
        a.add_to(0, 2, 2.5).unwrap();
        assert_eq!(a.at(0, 2).unwrap(), 2.5);
        assert!(!a.is_symmetric());
        a.set_at(2, 0, 2.5).unwrap();
        assert!(a.is_symmetric());
        assert!(a.at(3, 0).is_err());
        assert!(a.set_at(0, 3, 1.0).is_err());
        assert_eq!(a.row(1).unwrap(), &[0.0, 1.0, 0.0]);
        assert!(a.row(3).is_none());
        a.row_mut(1).unwrap()[0] = 7.0;
        assert_eq!(a.at(1, 0).unwrap(), 7.0);
        let keep = a.clone();
        a.set_size(3, 3).unwrap();
        assert_eq!(a, keep);
        a.set_size(2, 4).unwrap();
        assert_eq!((a.num_rows(), a.num_columns()), (2, 4));
        assert!(a.make_identity().is_err());
        a.clear();
        assert!(a.as_slice().iter().all(|&x| x == 0.0));
        assert!(!Matrix::new(0, 0).is_symmetric());
        assert!(Matrix::from_row_major(2, 2, vec![1.0]).is_err());
    }

    /// Fuzz regression: caller-chosen sizes whose element count overflows or cannot
    /// be allocated return `InvalidInput` instead of aborting.
    #[test]
    fn huge_sizes_fail_softly() {
        assert!(matches!(
            Matrix::try_new(1 << 32, 1 << 32),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            Matrix::try_new(usize::MAX / 8, 2),
            Err(SolidError::InvalidInput { .. })
        ));
        let mut a = m(2, 2, &[1.0, 2.0, 3.0, 4.0]);
        let keep = a.clone();
        assert!(a.set_size(usize::MAX, 3).is_err());
        assert!(a.set_size(1 << 40, 1 << 20).is_err());
        assert_eq!(a, keep);
        assert_eq!(Matrix::try_new(2, 3).unwrap(), Matrix::new(2, 3));
    }

    #[test]
    fn allocation_ceiling_rejected() {
        let rows = crate::limits::MAX_ALLOC_BYTES / size_of::<f64>() + 1;
        assert!(matches!(
            Matrix::try_new(rows, 1),
            Err(SolidError::InvalidInput {
                operation: "geom::matrix::Matrix::try_new",
                reason: "matrix too large to allocate",
            })
        ));
    }

    #[test]
    fn element_count_overflow_rejected() {
        assert!(matches!(
            Matrix::try_new(usize::MAX, 2),
            Err(SolidError::InvalidInput {
                operation: "geom::matrix::Matrix::try_new",
                reason: "element count overflows",
            })
        ));
    }

    #[test]
    fn failed_resize_preserves_matrix() {
        let mut a = m(2, 3, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let keep = a.clone();
        let rows = crate::limits::MAX_ALLOC_BYTES / size_of::<f64>() + 1;
        for (rows, cols) in [(rows, 1), (usize::MAX, 2)] {
            assert!(matches!(
                a.set_size(rows, cols),
                Err(SolidError::InvalidInput { .. })
            ));
            assert_eq!(a, keep);
        }
    }

    #[test]
    fn jacobi_symmetric() {
        let mut a = m(3, 3, &[2.0, -1.0, 0.0, -1.0, 2.0, -1.0, 0.0, -1.0, 2.0]);
        let orig = a.clone();
        let e = a.eigen_system_jacobi().unwrap();
        assert_eq!(a, orig);
        let s2 = 2f64.sqrt();
        for (x, ex) in e.values.iter().zip([2.0 - s2, 2.0, 2.0 + s2]) {
            assert!((x - ex).abs() < 1e-12);
        }
        assert!(e.rotations > 0);
        let mut b = orig.clone();
        let (vals, vecs) = b.compute_3x3_eigen_vectors().unwrap();
        assert_eq!(vals.to_vec(), e.values);
        assert_eq!(vecs[1].x, e.vectors.at(0, 1).unwrap());
        let mut ns = m(2, 2, &[1.0, 2.0, 3.0, 4.0]);
        assert!(ns.eigen_system_jacobi().is_err());
        assert!(ns.compute_3x3_eigen_vectors().is_err());
        let mut ns3 = m(3, 3, &[1.0, 2.0, 0.0, 3.0, 4.0, 0.0, 0.0, 0.0, 1.0]);
        assert!(matches!(
            ns3.compute_3x3_eigen_vectors(),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
    }

    #[test]
    fn jacobi_non_finite() {
        let mut a = m(2, 2, &[f64::INFINITY, 1.0, 1.0, 0.0]);
        let e = a.eigen_system_jacobi().unwrap();
        assert_eq!(e.values, vec![0.0, f64::INFINITY]);
        let mut b = m(
            3,
            3,
            &[f64::INFINITY, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        );
        assert!(b.compute_3x3_eigen_vectors().is_ok());
        let mut nan = m(1, 1, &[f64::NAN]);
        let e = nan.eigen_system_jacobi().unwrap();
        assert!(e.values[0].is_nan());
    }
}
