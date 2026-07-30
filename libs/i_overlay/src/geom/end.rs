use i_float::int::number::int::IntNumber;
use i_float::int::point::IntPoint;

#[derive(Clone, Copy)]
pub(crate) struct End<I: IntNumber> {
    pub(crate) index: usize,
    pub(crate) point: IntPoint<I>,
}

impl<I: IntNumber> Default for End<I> {
    #[inline(always)]
    fn default() -> Self {
        Self {
            index: 0,
            point: IntPoint::ZERO,
        }
    }
}
