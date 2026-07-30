use i_float::int::number::int::IntNumber;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LineRange<I: IntNumber> {
    pub(crate) min: I,
    pub(crate) max: I,
}
