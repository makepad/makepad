use i_float::float::compatible::FloatPointCompatible;
use i_float::float::vector::FloatPointMath;

pub(crate) struct Math<P> {
    _phantom: core::marker::PhantomData<P>,
}
impl<P: FloatPointCompatible> Math<P> {
    #[inline(always)]
    pub(crate) fn normal(a: &P, b: &P) -> P {
        let c = FloatPointMath::sub(a, b);
        FloatPointMath::normalize(&c)
    }

    #[inline(always)]
    pub(crate) fn ortho_and_scale(p: &P, s: P::Scalar) -> P {
        let t = P::from_xy(-p.y(), p.x());
        FloatPointMath::scale(&t, s)
    }
}
