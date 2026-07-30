use crate::{Expiration, ExpiredVal};
use core::marker::PhantomData;

#[derive(Clone, Copy)]
pub(super) struct Entity<E, V> {
    pub(super) val: V,
    pub(super) mask: u64,
    phantom_data: PhantomData<E>,
}

impl<E: Expiration, V: ExpiredVal<E>> Entity<E, V> {
    #[inline]
    pub(super) fn new(val: V, mask: u64) -> Self {
        Self {
            val,
            mask,
            phantom_data: Default::default(),
        }
    }
}
