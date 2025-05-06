use {
    crate::{Info, Measure, Metric},
    array_vec::ArrayVec,
    btree,
    std::marker::PhantomData,
};

#[cfg(not(test))]
const MAX_LEN: usize = 1024;
#[cfg(test)]
const MAX_LEN: usize = 8;

#[derive(Debug)]
pub(super) struct Leaf<T, M> {
    vec: ArrayVec<T, MAX_LEN>,
    phantom: PhantomData<M>,
}

impl<T, M> Leaf<T, M> {
    pub(super) fn as_slice(&self) -> &[T] {
        &self.vec
    }

    pub(super) fn push(&mut self, item: T) {
        self.vec.push(item);
    }
}

impl<T, M> Clone for Leaf<T, M>
where
    T: Clone,
{
    fn clone(&self) -> Self {
        Self {
            vec: self.vec.clone(),
            phantom: self.phantom.clone(),
        }
    }
}

impl<T, M> btree::Leaf for Leaf<T, M>
where
    M: Metric<T>,
{
    const MAX_LEN: usize = MAX_LEN;

    type Info = Info<M::Measure>;

    fn new() -> Self {
        Self {
            vec: ArrayVec::new(),
            phantom: PhantomData,
        }
    }

    fn is_at_least_half_full(&self) -> bool {
        self.len() >= Self::MAX_LEN / 2
    }

    fn can_split_at(&self, _index: usize) -> bool {
        true
    }

    fn len(&self) -> usize {
        self.vec.len()
    }

    fn info_to(&self, end: usize) -> Self::Info {
        Info {
            len: end,
            measure: self.vec[..end]
                .iter()
                .map(|item| M::measure(item))
                .fold(M::Measure::empty(), |acc, measure| acc.combine(measure)),
        }
    }

    fn move_left(&mut self, other: &mut Self, end: usize) {
        debug_assert!(self.len() + end <= Self::MAX_LEN);
        self.vec.extend(other.vec.drain(..end));
    }

    fn move_right(&mut self, other: &mut Self, start: usize) {
        debug_assert!(self.len() - start + other.len() <= Self::MAX_LEN);
        other.vec.splice(..0, self.vec.drain(start..));
    }

    fn remove_from(&mut self, start: usize) {
        self.vec.truncate(start);
    }

    fn remove_to(&mut self, end: usize) {
        self.vec.drain(..end);
    }

    fn split_off(&mut self, index: usize) -> Self {
        Self {
            vec: self.vec.split_off(index),
            phantom: PhantomData,
        }
    }
}
