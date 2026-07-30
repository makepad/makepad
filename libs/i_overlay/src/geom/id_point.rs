use i_float::int::number::int::IntNumber;
use i_float::int::point::IntPoint;

#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) struct IdPoint<I: IntNumber> {
    pub(crate) id: usize,
    pub(crate) point: IntPoint<I>,
}

impl<I: IntNumber> IdPoint<I> {
    pub(crate) fn new(id: usize, point: IntPoint<I>) -> Self {
        Self { id, point }
    }
}
