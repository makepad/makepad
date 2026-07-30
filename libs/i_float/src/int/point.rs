use crate::int::number::int::IntNumber;
use crate::int::vector::IntVector;
use core::cmp::Ordering;
use core::{fmt, ops};

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct IntPoint<T: IntNumber = i32> {
    pub x: T,
    pub y: T,
}

impl<T: IntNumber> IntPoint<T> {
    pub const ZERO: Self = Self {
        x: T::ZERO,
        y: T::ZERO,
    };
    pub const EMPTY: Self = Self { x: T::MAX, y: T::MAX };

    #[inline(always)]
    pub fn new(x: T, y: T) -> Self {
        Self { x, y }
    }

    #[inline(always)]
    pub fn cross_product(self, v: Self) -> T::Wide {
        let a = self.x.wide() * v.y.wide();
        let b = self.y.wide() * v.x.wide();

        a - b
    }

    #[inline(always)]
    pub fn dot_product(self, v: Self) -> T::Wide {
        let xx = self.x.wide() * v.x.wide();
        let yy = self.y.wide() * v.y.wide();
        xx + yy
    }

    #[inline(always)]
    pub fn sqr_length(self) -> T::Wide {
        let x = self.x.wide();
        let y = self.y.wide();
        x * x + y * y
    }

    #[inline(always)]
    pub fn sqr_distance(self, other: Self) -> T::Wide {
        (self - other).sqr_length()
    }
}

impl<T: IntNumber> fmt::Display for IntPoint<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "[{}, {}]", self.x, self.y)
    }
}

impl<T: IntNumber> From<[T; 2]> for IntPoint<T> {
    #[inline(always)]
    fn from(value: [T; 2]) -> Self {
        IntPoint::new(value[0], value[1])
    }
}

impl<T: IntNumber> From<(T, T)> for IntPoint<T> {
    #[inline(always)]
    fn from(value: (T, T)) -> Self {
        IntPoint::new(value.0, value.1)
    }
}

impl<T: IntNumber> PartialOrd for IntPoint<T> {
    #[inline(always)]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: IntNumber> Ord for IntPoint<T> {
    #[inline(always)]
    fn cmp(&self, other: &Self) -> Ordering {
        let x = self.x == other.x;
        if x && self.y == other.y {
            Ordering::Equal
        } else if self.x < other.x || x && self.y < other.y {
            Ordering::Less
        } else {
            Ordering::Greater
        }
    }
}

impl<T: IntNumber> ops::Add for IntPoint<T> {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        IntPoint {
            x: self.x + other.x,
            y: self.y + other.y,
        }
    }
}

impl<T: IntNumber> ops::Sub for IntPoint<T> {
    type Output = IntVector<T>;

    #[inline(always)]
    fn sub(self, other: Self) -> Self::Output {
        IntVector {
            x: self.x.wide() - other.x.wide(),
            y: self.y.wide() - other.y.wide(),
        }
    }
}

#[macro_export]
macro_rules! int_pnt {
    ($x:expr, $y:expr) => {
        IntPoint::new($x, $y)
    };
}

#[cfg(test)]
mod tests {
    use crate::int::point::IntPoint;

    #[test]
    fn test_0() {
        let p: IntPoint = (1, 2).into();
        assert_eq!(p.x, 1);
        assert_eq!(p.y, 2);
    }

    #[test]
    fn test_1() {
        let p: IntPoint = [1, 2].into();
        assert_eq!(p.x, 1);
        assert_eq!(p.y, 2);
    }
    #[test]
    fn test_2() {
        assert_eq!(int_pnt![0, 0], int_pnt![0, 0]);
        assert!(int_pnt![0, 0] < int_pnt![0, 4]);
        assert!(int_pnt![1, 0] > int_pnt![0, 4]);
        assert!(int_pnt![0, 4] > int_pnt![0, 0]);
        assert!(int_pnt![0, 4] < int_pnt![1, 0]);
    }

    #[test]
    fn test_generic_i64() {
        let a: IntPoint<i64> = (1, 2).into();
        let b: IntPoint<i64> = [1, 3].into();

        assert_eq!(alloc::format!("{}", a), "[1, 2]");
        assert!(a < b);
    }

    #[test]
    fn test_sub_returns_wide_vector() {
        let a = IntPoint::new(i32::MIN, i32::MIN);
        let b = IntPoint::new(i32::MAX, i32::MAX);
        let v = a - b;

        assert_eq!(v.x, i32::MIN as i64 - i32::MAX as i64);
        assert_eq!(v.y, i32::MIN as i64 - i32::MAX as i64);
    }
}
