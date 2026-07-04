// Port of box3d/src/id_pool.h + id_pool.c
// Index pool with a free list.

use crate::b3_assert;
use crate::container::array_byte_count;

#[derive(Clone, Debug, Default)]
pub struct IdPool {
    pub free_array: Vec<i32>,
    pub next_index: i32,
}

pub fn create_id_pool() -> IdPool {
    IdPool {
        free_array: Vec::with_capacity(32),
        next_index: 0,
    }
}

pub fn destroy_id_pool(pool: &mut IdPool) {
    *pool = IdPool::default();
}

pub fn alloc_id(pool: &mut IdPool) -> i32 {
    if let Some(id) = pool.free_array.pop() {
        return id;
    }

    let id = pool.next_index;
    pool.next_index += 1;
    id
}

pub fn free_id(pool: &mut IdPool, id: i32) {
    b3_assert!(pool.next_index > 0);
    b3_assert!(0 <= id && id < pool.next_index);

    // todo does not work with assertion above
    // should probably be `id == pool->nextIndex - 1`
    if id == pool.next_index {
        pool.next_index -= 1;
        return;
    }

    pool.free_array.push(id);
}

/// Validation only: asserts that `id` is on the free list.
pub fn validate_free_id(pool: &IdPool, id: i32) {
    #[cfg(debug_assertions)]
    {
        for &free in &pool.free_array {
            if free == id {
                return;
            }
        }

        b3_assert!(false, "id is not free");
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (pool, id);
    }
}

#[inline]
pub fn get_id_count(pool: &IdPool) -> i32 {
    pool.next_index - pool.free_array.len() as i32
}

#[inline]
pub fn get_id_capacity(pool: &IdPool) -> i32 {
    pool.next_index
}

#[inline]
pub fn get_id_bytes(pool: &IdPool) -> i32 {
    array_byte_count(&pool.free_array)
}
