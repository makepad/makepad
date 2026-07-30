use crate::key::entity::Entity;
use crate::{Expiration, ExpiredKey};

#[derive(PartialEq, Clone, Copy)]
pub(super) enum Color {
    Red,
    Black,
}

#[derive(Clone, Copy)]
pub(super) struct Node<K, E, V> {
    pub(super) parent: u32,
    pub(super) left: u32,
    pub(super) right: u32,
    pub(super) color: Color,
    pub(super) entity: Entity<K, E, V>,
}

impl<K: ExpiredKey<E>, E: Expiration, V: Copy> Node<K, E, V> {
    #[inline(always)]
    pub(super) fn is_not_expired(&self, time: E) -> bool {
        self.entity.key.expiration() > time
    }
}

impl<K: ExpiredKey<E>, E: Expiration, V: Copy> Default for Node<K, E, V> {
    #[inline]
    fn default() -> Self {
        Self {
            parent: 0,
            left: 0,
            right: 0,
            color: Color::Red,
            entity: unsafe { core::mem::zeroed() },
        }
    }
}
