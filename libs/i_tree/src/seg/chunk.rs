use crate::seg::entity::Entity;
use crate::{Expiration, ExpiredVal};
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone)]
pub(super) struct Chunk<E, V> {
    pub(super) buffer: Vec<Entity<E, V>>,
}

impl<E: Expiration, V: ExpiredVal<E>> Chunk<E, V> {
    #[inline]
    pub(super) fn new() -> Self {
        Self { buffer: vec![] }
    }

    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    #[inline]
    pub(super) fn entity(&self, index: usize) -> &Entity<E, V> {
        unsafe { self.buffer.get_unchecked(index) }
    }

    #[inline]
    pub(super) fn insert(&mut self, entity: Entity<E, V>) {
        // self.clear_expired(time);
        // self.min_exp = self.min_exp.min(entity.val.expiration());
        self.buffer.push(entity);
    }
    //
    // #[inline]
    // pub(super) fn clear_expired(&mut self, time: E) {
    //     if self.min_exp >= time {
    //         return;
    //     }
    //     let mut new_min_exp = E::max_expiration();
    //     self.buffer.retain(|entity| {
    //         let exp = entity.val.expiration();
    //         let keep = exp > time;
    //         if keep {
    //             new_min_exp = new_min_exp.min(exp);
    //         }
    //         keep
    //     });
    //     self.min_exp = new_min_exp;
    // }

    #[inline]
    pub(super) fn clear(&mut self) {
        // self.min_exp = E::max_expiration();
        self.buffer.clear();
    }
}
