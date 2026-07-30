use crate::{Expiration, ExpiredKey};
use core::marker::PhantomData;

#[derive(Clone, Copy)]
pub(super) struct Entity<K, E, V> {
    pub(super) key: K,
    pub(super) val: V,
    phantom_data: PhantomData<E>,
}

impl<K: ExpiredKey<E>, E: Expiration, V: Copy> Entity<K, E, V> {
    #[inline]
    pub(super) fn new(key: K, val: V) -> Self {
        Self {
            key,
            val,
            phantom_data: Default::default(),
        }
    }
}
