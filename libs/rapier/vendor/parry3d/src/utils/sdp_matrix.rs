use crate::math::{Matrix2, Matrix3, Real, Vector2, Vector3};
use core::ops::{Add, Div, Mul, Neg, Sub};
use num_traits::{One, Zero};

/// A 2x2 symmetric-definite-positive matrix.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
pub struct SdpMatrix2<N> {
    /// The component at the first row and first column of this matrix.
    pub m11: N,
    /// The component at the first row and second column of this matrix.
    pub m12: N,
    /// The component at the second row and second column of this matrix.
    pub m22: N,
}

impl<
        N: Copy
            + Zero
            + One
            + Add<Output = N>
            + Sub<Output = N>
            + Mul<Output = N>
            + Div<Output = N>
            + Neg<Output = N>,
    > SdpMatrix2<N>
{
    /// A new SDP 2x2 matrix with the given components.
    ///
    /// Because the matrix is symmetric, only the lower off-diagonal component is required.
    pub fn new(m11: N, m12: N, m22: N) -> Self {
        Self { m11, m12, m22 }
    }

    /// Create a new SDP matrix filled with zeros.
    pub fn zero() -> Self {
        Self {
            m11: N::zero(),
            m12: N::zero(),
            m22: N::zero(),
        }
    }

    /// Create a new SDP matrix with its diagonal filled with `val`, and its off-diagonal elements set to zero.
    pub fn diagonal(val: N) -> Self {
        Self {
            m11: val,
            m12: N::zero(),
            m22: val,
        }
    }

    /// Adds `val` to the diagonal components of `self`.
    pub fn add_diagonal(&mut self, elt: N) -> Self {
        Self {
            m11: self.m11 + elt,
            m12: self.m12,
            m22: self.m22 + elt,
        }
    }

    /// Compute the inverse of this SDP matrix without performing any inversibility check.
    pub fn inverse_unchecked(&self) -> Self {
        self.inverse_and_get_determinant_unchecked().0
    }

    /// Compute the inverse of this SDP matrix without performing any inversibility check.
    pub fn inverse_and_get_determinant_unchecked(&self) -> (Self, N) {
        let determinant = self.m11 * self.m22 - self.m12 * self.m12;
        let m11 = self.m22 / determinant;
        let m12 = -self.m12 / determinant;
        let m22 = self.m11 / determinant;

        (Self { m11, m12, m22 }, determinant)
    }
}

impl SdpMatrix2<Real> {
    /// Build an `SdpMatrix2` structure from a plain matrix, assuming it is SDP.
    pub fn from_sdp_matrix(mat: Matrix2) -> Self {
        let cols = mat.to_cols_array_2d();
        Self {
            m11: cols[0][0],
            m12: cols[1][0],
            m22: cols[1][1],
        }
    }

    /// Convert this SDP matrix to a regular matrix representation.
    pub fn into_matrix(self) -> Matrix2 {
        Matrix2::from_cols(
            Vector2::new(self.m11, self.m12),
            Vector2::new(self.m12, self.m22),
        )
    }

    /// Multiply this matrix by a vector.
    pub fn mul_vec(&self, rhs: Vector2) -> Vector2 {
        Vector2::new(
            self.m11 * rhs.x + self.m12 * rhs.y,
            self.m12 * rhs.x + self.m22 * rhs.y,
        )
    }
}

impl<N: Add<Output = N>> Add<SdpMatrix2<N>> for SdpMatrix2<N> {
    type Output = Self;

    fn add(self, rhs: SdpMatrix2<N>) -> Self {
        Self {
            m11: self.m11 + rhs.m11,
            m12: self.m12 + rhs.m12,
            m22: self.m22 + rhs.m22,
        }
    }
}

impl Mul<Vector2> for SdpMatrix2<Real> {
    type Output = Vector2;

    fn mul(self, rhs: Vector2) -> Self::Output {
        self.mul_vec(rhs)
    }
}

impl Mul<Real> for SdpMatrix2<Real> {
    type Output = SdpMatrix2<Real>;

    fn mul(self, rhs: Real) -> Self::Output {
        SdpMatrix2 {
            m11: self.m11 * rhs,
            m12: self.m12 * rhs,
            m22: self.m22 * rhs,
        }
    }
}

/// A 3x3 symmetric-definite-positive matrix.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
pub struct SdpMatrix3<N> {
    /// The component at the first row and first column of this matrix.
    pub m11: N,
    /// The component at the first row and second column of this matrix.
    pub m12: N,
    /// The component at the first row and third column of this matrix.
    pub m13: N,
    /// The component at the second row and second column of this matrix.
    pub m22: N,
    /// The component at the second row and third column of this matrix.
    pub m23: N,
    /// The component at the third row and third column of this matrix.
    pub m33: N,
}

impl<
        N: Copy
            + Zero
            + One
            + Add<Output = N>
            + Sub<Output = N>
            + Mul<Output = N>
            + Div<Output = N>
            + Neg<Output = N>
            + PartialEq,
    > SdpMatrix3<N>
{
    /// A new SDP 3x3 matrix with the given components.
    ///
    /// Because the matrix is symmetric, only the lower off-diagonal components is required.
    pub fn new(m11: N, m12: N, m13: N, m22: N, m23: N, m33: N) -> Self {
        Self {
            m11,
            m12,
            m13,
            m22,
            m23,
            m33,
        }
    }

    /// Create a new SDP matrix filled with zeros.
    pub fn zero() -> Self {
        Self {
            m11: N::zero(),
            m12: N::zero(),
            m13: N::zero(),
            m22: N::zero(),
            m23: N::zero(),
            m33: N::zero(),
        }
    }

    /// Create a new SDP matrix with its diagonal filled with `val`, and its off-diagonal elements set to zero.
    pub fn diagonal(val: N) -> Self {
        Self {
            m11: val,
            m12: N::zero(),
            m13: N::zero(),
            m22: val,
            m23: N::zero(),
            m33: val,
        }
    }

    /// Are all components of this matrix equal to zero?
    pub fn is_zero(&self) -> bool {
        self.m11 == N::zero()
            && self.m12 == N::zero()
            && self.m13 == N::zero()
            && self.m22 == N::zero()
            && self.m23 == N::zero()
            && self.m33 == N::zero()
    }

    /// Compute the inverse of this SDP matrix without performing any inversibility check.
    pub fn inverse_unchecked(&self) -> Self {
        let minor_m12_m23 = self.m22 * self.m33 - self.m23 * self.m23;
        let minor_m11_m23 = self.m12 * self.m33 - self.m13 * self.m23;
        let minor_m11_m22 = self.m12 * self.m23 - self.m13 * self.m22;

        let determinant =
            self.m11 * minor_m12_m23 - self.m12 * minor_m11_m23 + self.m13 * minor_m11_m22;
        let inv_det = N::one() / determinant;

        SdpMatrix3 {
            m11: minor_m12_m23 * inv_det,
            m12: -minor_m11_m23 * inv_det,
            m13: minor_m11_m22 * inv_det,
            m22: (self.m11 * self.m33 - self.m13 * self.m13) * inv_det,
            m23: (self.m13 * self.m12 - self.m23 * self.m11) * inv_det,
            m33: (self.m11 * self.m22 - self.m12 * self.m12) * inv_det,
        }
    }

    /// Adds `elt` to the diagonal components of `self`.
    pub fn add_diagonal(&self, elt: N) -> Self {
        Self {
            m11: self.m11 + elt,
            m12: self.m12,
            m13: self.m13,
            m22: self.m22 + elt,
            m23: self.m23,
            m33: self.m33 + elt,
        }
    }
}

impl SdpMatrix3<Real> {
    /// Build an `SdpMatrix3` structure from a plain matrix, assuming it is SDP.
    pub fn from_sdp_matrix(mat: Matrix3) -> Self {
        let cols = mat.to_cols_array_2d();
        Self {
            m11: cols[0][0],
            m12: cols[1][0],
            m13: cols[2][0],
            m22: cols[1][1],
            m23: cols[2][1],
            m33: cols[2][2],
        }
    }

    /// Multiply this matrix by a vector.
    pub fn mul_vec(&self, rhs: Vector3) -> Vector3 {
        let x = self.m11 * rhs.x + self.m12 * rhs.y + self.m13 * rhs.z;
        let y = self.m12 * rhs.x + self.m22 * rhs.y + self.m23 * rhs.z;
        let z = self.m13 * rhs.x + self.m23 * rhs.y + self.m33 * rhs.z;
        Vector3::new(x, y, z)
    }

    /// Multiply this matrix by a 3x3 matrix.
    pub fn mul_mat(&self, rhs: Matrix3) -> Matrix3 {
        let cols = rhs.to_cols_array_2d();
        let col0 = self.mul_vec(Vector3::new(cols[0][0], cols[0][1], cols[0][2]));
        let col1 = self.mul_vec(Vector3::new(cols[1][0], cols[1][1], cols[1][2]));
        let col2 = self.mul_vec(Vector3::new(cols[2][0], cols[2][1], cols[2][2]));
        Matrix3::from_cols(col0, col1, col2)
    }

    /// Compute the quadratic form `m.transpose() * self * m`.
    pub fn quadform(&self, m: &Matrix3) -> Self {
        let sm = self.mul_mat(*m);
        let result = m.transpose() * sm;
        Self::from_sdp_matrix(result)
    }

    /// Compute the quadratic form `m.transpose() * self * m` for a 3x2 matrix.
    pub fn quadform3x2(
        &self,
        m11: Real,
        m12: Real,
        m21: Real,
        m22: Real,
        m31: Real,
        m32: Real,
    ) -> SdpMatrix2<Real> {
        let x0 = self.m11 * m11 + self.m12 * m21 + self.m13 * m31;
        let y0 = self.m12 * m11 + self.m22 * m21 + self.m23 * m31;
        let z0 = self.m13 * m11 + self.m23 * m21 + self.m33 * m31;

        let x1 = self.m11 * m12 + self.m12 * m22 + self.m13 * m32;
        let y1 = self.m12 * m12 + self.m22 * m22 + self.m23 * m32;
        let z1 = self.m13 * m12 + self.m23 * m22 + self.m33 * m32;

        let r11 = m11 * x0 + m21 * y0 + m31 * z0;
        let r12 = m11 * x1 + m21 * y1 + m31 * z1;
        let r22 = m12 * x1 + m22 * y1 + m32 * z1;

        SdpMatrix2 {
            m11: r11,
            m12: r12,
            m22: r22,
        }
    }
}

impl<N: Add<Output = N>> Add<SdpMatrix3<N>> for SdpMatrix3<N> {
    type Output = SdpMatrix3<N>;

    fn add(self, rhs: SdpMatrix3<N>) -> Self::Output {
        SdpMatrix3 {
            m11: self.m11 + rhs.m11,
            m12: self.m12 + rhs.m12,
            m13: self.m13 + rhs.m13,
            m22: self.m22 + rhs.m22,
            m23: self.m23 + rhs.m23,
            m33: self.m33 + rhs.m33,
        }
    }
}

impl Mul<Real> for SdpMatrix3<Real> {
    type Output = SdpMatrix3<Real>;

    fn mul(self, rhs: Real) -> Self::Output {
        SdpMatrix3 {
            m11: self.m11 * rhs,
            m12: self.m12 * rhs,
            m13: self.m13 * rhs,
            m22: self.m22 * rhs,
            m23: self.m23 * rhs,
            m33: self.m33 * rhs,
        }
    }
}

impl Mul<Vector3> for SdpMatrix3<Real> {
    type Output = Vector3;

    fn mul(self, rhs: Vector3) -> Self::Output {
        self.mul_vec(rhs)
    }
}

impl Mul<Matrix3> for SdpMatrix3<Real> {
    type Output = Matrix3;

    fn mul(self, rhs: Matrix3) -> Self::Output {
        self.mul_mat(rhs)
    }
}

impl<T> From<[SdpMatrix3<Real>; 4]> for SdpMatrix3<T>
where
    T: From<[Real; 4]>,
{
    fn from(data: [SdpMatrix3<Real>; 4]) -> Self {
        SdpMatrix3 {
            m11: T::from([data[0].m11, data[1].m11, data[2].m11, data[3].m11]),
            m12: T::from([data[0].m12, data[1].m12, data[2].m12, data[3].m12]),
            m13: T::from([data[0].m13, data[1].m13, data[2].m13, data[3].m13]),
            m22: T::from([data[0].m22, data[1].m22, data[2].m22, data[3].m22]),
            m23: T::from([data[0].m23, data[1].m23, data[2].m23, data[3].m23]),
            m33: T::from([data[0].m33, data[1].m33, data[2].m33, data[3].m33]),
        }
    }
}
