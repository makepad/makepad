use crate::int::number::int::IntNumber;
use core::fmt;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IntVector<T: IntNumber> {
    pub x: T::Wide,
    pub y: T::Wide,
}

impl<T: IntNumber> IntVector<T> {
    #[inline(always)]
    pub fn new(x: T::Wide, y: T::Wide) -> Self {
        Self { x, y }
    }

    #[inline(always)]
    pub fn cross_product(self, v: Self) -> T::Wide {
        let a = self.x * v.y;
        let b = self.y * v.x;

        a - b
    }

    #[inline(always)]
    pub fn dot_product(self, v: Self) -> T::Wide {
        let xx = self.x * v.x;
        let yy = self.y * v.y;
        xx + yy
    }

    #[inline(always)]
    pub fn sqr_length(self) -> T::Wide {
        let x = self.x;
        let y = self.y;
        x * x + y * y
    }
}

impl<T: IntNumber> fmt::Display for IntVector<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "[{}, {}]", self.x, self.y)
    }
}
