use crate::key::entity::Entity;
use crate::key::exp::KeyExpCollection;
use crate::{Expiration, ExpiredKey};
use alloc::vec::Vec;
use core::cmp::Ordering;

#[derive(Clone)]
pub struct KeyExpList<K, E, V> {
    pub(super) buffer: Vec<Entity<K, E, V>>,
    min_exp: E,
}

impl<K: ExpiredKey<E>, E: Expiration, V: Copy> KeyExpList<K, E, V> {
    #[inline(always)]
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(capacity),
            min_exp: E::max_expiration(),
        }
    }

    #[inline(always)]
    pub fn reserve_capacity(&mut self, capacity: usize) {
        let additional = capacity.saturating_sub(self.buffer.capacity());
        if additional > 0 {
            self.buffer.reserve(additional);
        }
    }
}

impl<K: ExpiredKey<E>, E: Expiration, V: Copy> KeyExpCollection<K, E, V> for KeyExpList<K, E, V> {
    #[inline]
    fn insert(&mut self, key: K, val: V, time: E) {
        self.clear_expired(time);
        self.min_exp = self.min_exp.min(key.expiration());
        let index = self
            .buffer
            .binary_search_by_key(&key, |e| e.key)
            .unwrap_or_else(|index| index);
        self.buffer.insert(index, Entity::new(key, val));
    }

    #[inline]
    fn first_less(&mut self, time: E, default: V, key: K) -> V {
        self.clear_expired(time);
        let index = self
            .buffer
            .binary_search_by(|e| e.key.cmp(&key))
            .unwrap_or_else(|index| index);

        if index > 0 {
            unsafe { self.buffer.get_unchecked(index - 1) }.val
        } else {
            default
        }
    }

    #[inline]
    fn first_less_or_equal_by<F>(&mut self, time: E, default: V, f: F) -> V
    where
        F: Fn(K) -> Ordering,
    {
        self.clear_expired(time);
        match self.buffer.binary_search_by(|e| f(e.key)) {
            Ok(index) => unsafe { self.buffer.get_unchecked(index) }.val,
            Err(index) => {
                if index > 0 {
                    unsafe { self.buffer.get_unchecked(index - 1) }.val
                } else {
                    default
                }
            }
        }
    }

    #[inline]
    fn clear(&mut self) {
        self.min_exp = E::max_expiration();
        self.buffer.clear();
    }
}

impl<K: ExpiredKey<E>, E: Expiration, V: Copy> KeyExpList<K, E, V> {
    #[inline]
    fn clear_expired(&mut self, time: E) {
        if self.min_exp > time {
            return;
        }
        let mut new_min_exp = E::max_expiration();
        self.buffer.retain(|s| {
            let exp = s.key.expiration();
            let keep = exp > time;
            if keep {
                new_min_exp = new_min_exp.min(exp);
            }
            keep
        });
        self.min_exp = new_min_exp;
    }
}
