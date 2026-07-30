use crate::key::node::Node;
use crate::{Expiration, ExpiredKey};
use alloc::vec::Vec;

pub(super) struct Pool<K, E, V> {
    pub(super) buffer: Vec<Node<K, E, V>>,
    pub(super) unused: Vec<u32>,
}

impl<K: ExpiredKey<E>, E: Expiration, V: Copy> Pool<K, E, V> {
    #[inline(always)]
    pub(super) fn new(capacity: usize) -> Self {
        let capacity = capacity.max(8);
        let mut store = Self {
            buffer: Vec::with_capacity(capacity),
            unused: Vec::with_capacity(capacity),
        };
        store.reserve(capacity);
        store
    }

    #[inline]
    pub(super) fn reserve(&mut self, additional: usize) {
        debug_assert!(additional > 0);
        let n = self.buffer.len() as u32;
        let l = additional as u32;
        self.buffer.reserve(additional);
        self.buffer
            .resize(self.buffer.len() + additional, Node::default());
        self.unused.reserve(additional);
        self.unused.extend((n..n + l).rev());
    }

    #[inline(always)]
    pub(super) fn get_free_index(&mut self) -> u32 {
        if self.unused.is_empty() {
            self.reserve(self.unused.capacity());
        }
        self.unused.pop().unwrap()
    }

    #[inline(always)]
    pub(super) fn put_back(&mut self, index: u32) {
        self.unused.push(index)
    }
}
