use crate::float::compatible::FloatPointCompatible;
use crate::float::number::FloatNumber;
use core::fmt;
use core::ops::{Add, AddAssign, Mul, Neg, Sub};

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy)]
pub struct FloatPoint<T: FloatNumber> {
    pub x: T,
    pub y: T,
}

impl<T: FloatNumber> FloatPoint<T> {
    #[inline(always)]
    pub fn zero() -> Self {
        Self {
            x: FloatNumber::from_float(0.0),
            y: FloatNumber::from_float(0.0),
        }
    }

    #[inline(always)]
    pub fn new(x: T, y: T) -> Self {
        Self { x, y }
    }

    #[inline(always)]
    pub fn dot_product(self, v: Self) -> T {
        let xx: T = self.x * v.x;
        let yy: T = self.y * v.y;
        xx + yy
    }

    #[inline(always)]
    pub fn cross_product(self, v: Self) -> T {
        let a = self.x * v.y;
        let b = self.y * v.x;

        a - b
    }

    #[inline(always)]
    pub fn sqr_length(&self) -> T {
        self.x * self.x + self.y * self.y
    }

    #[inline(always)]
    pub fn length(&self) -> T {
        self.sqr_length().sqrt()
    }

    #[inline(always)]
    pub fn normalize(&self) -> Self {
        let l = self.length();
        Self {
            x: self.x / l,
            y: self.y / l,
        }
    }
}

impl<T: FloatNumber> Mul<T> for FloatPoint<T> {
    type Output = Self;

    #[inline(always)]
    fn mul(self, scalar: T) -> Self {
        Self {
            x: self.x * scalar,
            y: self.y * scalar,
        }
    }
}

impl<T: FloatNumber> Add for FloatPoint<T> {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
        }
    }
}

impl<T: FloatNumber> Sub for FloatPoint<T> {
    type Output = Self;

    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
        }
    }
}

impl<T: FloatNumber> Neg for FloatPoint<T> {
    type Output = Self;

    #[inline(always)]
    fn neg(self) -> Self::Output {
        Self {
            x: -self.x,
            y: -self.y,
        }
    }
}

impl<T: FloatNumber> AddAssign for FloatPoint<T> {
    #[inline(always)]
    fn add_assign(&mut self, other: Self) {
        self.x = self.x + other.x;
        self.y = self.y + other.y;
    }
}

impl<T: FloatNumber> fmt::Display for FloatPoint<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "[{}, {}]", self.x, self.y)
    }
}

impl<T: FloatNumber> FloatPointCompatible for FloatPoint<T> {
    type Scalar = T;

    #[inline(always)]
    fn from_xy(x: T, y: T) -> Self {
        Self { x, y }
    }

    #[inline(always)]
    fn x(&self) -> T {
        self.x
    }

    #[inline(always)]
    fn y(&self) -> T {
        self.y
    }
}
