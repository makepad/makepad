use crate::float::number::FloatNumber;

pub trait FloatPointCompatible: Copy {
    type Scalar: FloatNumber;
    fn from_xy(x: Self::Scalar, y: Self::Scalar) -> Self;
    fn x(&self) -> Self::Scalar;
    fn y(&self) -> Self::Scalar;
}

impl<T: FloatNumber> FloatPointCompatible for [T; 2] {
    type Scalar = T;

    #[inline(always)]
    fn from_xy(x: T, y: T) -> Self {
        [x, y]
    }

    #[inline(always)]
    fn x(&self) -> T {
        self[0]
    }

    #[inline(always)]
    fn y(&self) -> T {
        self[1]
    }
}

#[cfg(test)]
mod tests {
    use crate::float::compatible::FloatPointCompatible;

    #[test]
    fn test_0() {
        let a0 = [2.0, 5.0];
        let x = a0.x();
        let y = a0.y();
        let a1 = <[f64; 2]>::from_xy(x, y);

        assert_eq!(a0, a1);
    }
}
