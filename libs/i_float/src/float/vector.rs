use crate::float::compatible::FloatPointCompatible;
use crate::float::number::FloatNumber;

pub struct FloatPointMath<P> {
    _phantom: core::marker::PhantomData<P>,
}

impl<P: FloatPointCompatible> FloatPointMath<P> {
    #[inline(always)]
    pub fn add(a: &P, b: &P) -> P {
        P::from_xy(a.x() + b.x(), a.y() + b.y())
    }

    #[inline(always)]
    pub fn sub(a: &P, b: &P) -> P {
        P::from_xy(a.x() - b.x(), a.y() - b.y())
    }

    #[inline(always)]
    pub fn scale(p: &P, s: P::Scalar) -> P {
        P::from_xy(s * p.x(), s * p.y())
    }

    #[inline(always)]
    pub fn sqr_length(p: &P) -> P::Scalar {
        p.x() * p.x() + p.y() * p.y()
    }

    #[inline(always)]
    pub fn length(p: &P) -> P::Scalar {
        Self::sqr_length(p).sqrt()
    }

    #[inline(always)]
    pub fn normalize(p: &P) -> P {
        let inv_len = P::Scalar::from_float(1.0) / Self::length(p);
        P::from_xy(p.x() * inv_len, p.y() * inv_len)
    }

    #[inline(always)]
    pub fn dot_product(a: &P, b: &P) -> P::Scalar {
        a.x() * b.x() + a.y() * b.y()
    }

    #[inline(always)]
    pub fn cross_product(a: &P, b: &P) -> P::Scalar {
        a.x() * b.y() - a.y() * b.x()
    }
}

#[cfg(test)]
mod tests {
    use crate::float::vector::FloatPointMath;

    #[test]
    fn test_add() {
        let a = [2.0, 5.0];
        let b = [3.0, 1.0];
        let c = FloatPointMath::add(&a, &b);

        assert_eq!(c[0], 5.0);
        assert_eq!(c[1], 6.0);
    }

    #[test]
    fn test_sub() {
        let a = [2.0, 5.0];
        let b = [3.0, 1.0];
        let c = FloatPointMath::sub(&a, &b);

        assert_eq!(c[0], -1.0);
        assert_eq!(c[1], 4.0);
    }

    #[test]
    fn test_scale() {
        let a = [2.0, 5.0];
        let c = FloatPointMath::scale(&a, 2.0);

        assert_eq!(c[0], 4.0);
        assert_eq!(c[1], 10.0);
    }

    #[test]
    fn test_normalize() {
        let a = [6.0, 8.0];
        let c = FloatPointMath::normalize(&a);
        let dx = c[0] - 3.0f32 / 5.0f32;
        let dy = c[1] - 4.0f32 / 5.0f32;
        assert!(dx < 0.0001);
        assert!(dy < 0.0001);
    }
}
