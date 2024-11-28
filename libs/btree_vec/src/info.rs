use crate::Measure;

#[derive(Clone, Copy, Debug)]
pub(super) struct Info<M> {
    pub(super) len: usize,
    pub(super) measure: M,
}

impl<M> btree::Info for Info<M>
where
    M: Measure,
{
    fn empty() -> Self {
        Self {
            len: 0,
            measure: Measure::empty(),
        }
    }

    fn combine(self, other: Self) -> Self {
        Self {
            len: self.len + other.len,
            measure: self.measure.combine(other.measure),
        }
    }

    fn len(&self) -> usize {
        self.len
    }
}
