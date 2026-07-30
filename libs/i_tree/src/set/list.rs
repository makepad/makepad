use crate::EMPTY_REF;
use crate::set::sort::{KeyValue, SetCollection};
use alloc::vec::Vec;
use core::cmp::Ordering;

pub struct SetList<V> {
    pub(super) buffer: Vec<V>,
}

impl<V> SetList<V> {
    #[inline(always)]
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(capacity),
        }
    }
}

impl<K: Ord + Copy, V: KeyValue<K>> SetCollection<K, V> for SetList<V> {
    #[inline]
    fn insert(&mut self, val: V) {
        let index = self
            .buffer
            .binary_search_by_key(&val.key(), |v| v.key())
            .unwrap_or_else(|index| index);
        self.buffer.insert(index, val);
    }

    #[inline]
    fn delete_by_index(&mut self, index: u32) {
        self.buffer.remove(index as usize);
    }

    fn index_before(&self, index: u32) -> u32 {
        index - 1
    }

    #[inline]
    fn first_index_less_by<F>(&self, f: F) -> u32
    where
        F: Fn(&K) -> Ordering,
    {
        match self.buffer.binary_search_by(|v| f(v.key())) {
            Ok(index) => index as u32,
            Err(index) => {
                if index > 0 {
                    (index - 1) as u32
                } else {
                    EMPTY_REF
                }
            }
        }
    }

    #[inline]
    unsafe fn value_by_index(&self, index: u32) -> &V {
        unsafe { self.buffer.get_unchecked(index as usize) }
    }

    #[inline]
    unsafe fn value_by_index_mut(&mut self, index: u32) -> &mut V {
        unsafe { self.buffer.get_unchecked_mut(index as usize) }
    }
}
