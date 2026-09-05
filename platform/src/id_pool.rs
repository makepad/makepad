use std::{cell::RefCell, collections::VecDeque, ops::Deref, ops::DerefMut, rc::Rc};

#[derive(Clone, Default, Debug, PartialEq)]
pub struct IdPoolFree(Rc<RefCell<IdPoolFreeState>>);

#[derive(Default, Debug, PartialEq)]
struct IdPoolFreeState {
    free: Vec<usize>,
    is_free: Vec<bool>,
    generations: Vec<u64>,
    retirement_pending: VecDeque<usize>,
    retirement_queued: Vec<bool>,
}

#[derive(Default, Debug)]
pub struct IdPool<T>
where
    T: Default,
{
    pub pool: Vec<IdPoolItem<T>>,
    pub free: IdPoolFree,
}

#[derive(Debug, PartialEq)]
pub struct IdPoolItem<T> {
    pub item: T,
    pub generation: u64,
}

impl<T> Deref for IdPoolItem<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

impl<T> DerefMut for IdPoolItem<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.item
    }
}

#[derive(Debug, PartialEq)]
pub struct PoolId {
    pub id: usize,
    pub generation: u64,
    pub free: IdPoolFree,
}

impl PoolId {
    pub fn free(&mut self) {
        let mut state = self.free.0.borrow_mut();
        // A detached borrowed handle has an empty state and intentionally
        // cannot return the Cx-owned slot. Also coalesce Script GC + Drop so a
        // slot appears only once in both queues.
        if self.id >= state.is_free.len()
            || state.generations[self.id] != self.generation
            || state.is_free[self.id]
        {
            return;
        }
        state.is_free[self.id] = true;
        state.free.push(self.id);
        if !state.retirement_queued[self.id] {
            state.retirement_queued[self.id] = true;
            state.retirement_pending.push_back(self.id);
        }
    }
}

impl Drop for PoolId {
    fn drop(&mut self) {
        self.free()
    }
}

impl<T> IdPool<T>
where
    T: Default,
{
    pub fn slot_count(&self) -> usize {
        self.pool.len()
    }

    pub fn free_count(&self) -> usize {
        self.free.0.borrow().free.len()
    }

    /// True if `id`'s slot is currently in the free list (dropped, awaiting reuse).
    pub fn is_free(&self, id: usize) -> bool {
        self.free
            .0
            .borrow()
            .is_free
            .get(id)
            .copied()
            .unwrap_or(false)
    }

    pub fn live_count(&self) -> usize {
        self.slot_count().saturating_sub(self.free_count())
    }

    /// Whether drops are waiting to be examined at the next safe point.
    pub(crate) fn has_pending_retirements(&self) -> bool {
        !self.free.0.borrow().retirement_pending.is_empty()
    }

    pub fn alloc(&mut self) -> PoolId {
        let last_from_free_pool = {
            let mut state = self.free.0.borrow_mut();
            let id = state.free.pop();
            if let Some(id) = id {
                state.is_free[id] = false;
            }
            id
        };
        if let Some(id) = last_from_free_pool {
            self.pool[id].generation += 1;
            self.free.0.borrow_mut().generations[id] = self.pool[id].generation;
            PoolId {
                id,
                generation: self.pool[id].generation,
                free: self.free.clone(),
            }
        } else {
            self.alloc_new(None)
        }
    }

    pub fn alloc_new(&mut self, item: Option<T>) -> PoolId {
        let id = self.pool.len();
        self.pool.push(IdPoolItem {
            generation: 0,
            item: item.unwrap_or_else(|| T::default()),
        });
        let mut state = self.free.0.borrow_mut();
        state.is_free.push(false);
        state.generations.push(0);
        state.retirement_queued.push(false);
        drop(state);
        PoolId {
            id,
            generation: 0,
            free: self.free.clone(),
        }
    }

    /// Allocates an item in the pool, potentially reusing an existing slot.
    ///
    /// This method attempts to find a reusable slot in the pool that satisfies the given filter.
    /// If a suitable slot is found, it's reused; otherwise, a new slot is allocated.
    ///
    /// # Arguments
    /// * `filter` - A closure that determines if an existing item can be reused.
    /// * `item` - The new item to be stored in the pool.
    ///
    /// # Returns
    /// A tuple containing:
    /// - `PoolId`: The ID of the allocated or reused slot.
    /// - `Option<T>`: The previous item if a slot was reused, or None if a new slot was allocated.
    pub fn alloc_with_reuse_filter<F>(&mut self, mut filter: F, item: T) -> (PoolId, Option<T>)
    where
        F: FnMut(&IdPoolItem<T>) -> bool,
    {
        let maybe_free_id = self.free.0.borrow().free.iter().enumerate().find_map(
            |(index, &id)| {
                if filter(&self.pool[id]) {
                    Some((index, id))
                } else {
                    None
                }
            },
        );

        if let Some((index, id)) = maybe_free_id {
            let mut state = self.free.0.borrow_mut();
            state.free.swap_remove(index);
            state.is_free[id] = false;
            drop(state);
            self.pool[id].generation += 1;
            self.free.0.borrow_mut().generations[id] = self.pool[id].generation;
            let old_item = std::mem::replace(&mut self.pool[id].item, item);

            let pool_id = PoolId {
                id,
                generation: self.pool[id].generation,
                free: self.free.clone(),
            };
            (pool_id, Some(old_item))
        } else {
            (self.alloc_new(Some(item)), None)
        }
    }

    /// Examine up to `limit` pending slots and return those which are still
    /// free at this safe point.
    /// Drops are coalesced per slot, so pending metadata is bounded by the
    /// pool's slot count and allocation never needs to search it.
    pub(crate) fn take_free_retirements(&mut self, limit: usize) -> Vec<usize> {
        let mut state = self.free.0.borrow_mut();
        let mut retired = Vec::with_capacity(limit.min(state.retirement_pending.len()));
        for _ in 0..limit {
            let Some(id) = state.retirement_pending.pop_front() else {
                break;
            };
            state.retirement_queued[id] = false;
            if state.is_free[id] {
                retired.push(id);
            }
        }
        retired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retirement_is_only_reported_while_the_slot_is_free() {
        let mut pool = IdPool::<u32>::default();
        let first = pool.alloc();
        let first_generation = first.generation;
        drop(first);

        let reused = pool.alloc();
        assert_eq!(reused.id, 0);
        assert_ne!(reused.generation, first_generation);
        assert!(pool.take_free_retirements(8).is_empty());

        drop(reused);
        assert_eq!(pool.take_free_retirements(8), vec![0]);
        assert!(pool.take_free_retirements(8).is_empty());
    }

    #[test]
    fn repeated_free_calls_are_coalesced() {
        let mut pool = IdPool::<u32>::default();
        let mut id = pool.alloc();
        id.free();
        id.free();
        drop(id);

        assert_eq!(pool.free_count(), 1);
        assert_eq!(pool.take_free_retirements(1), vec![0]);
        assert!(pool.take_free_retirements(1).is_empty());
    }

    #[test]
    fn stale_drop_cannot_free_a_reused_generation() {
        let mut pool = IdPool::<u32>::default();
        let mut old = pool.alloc();
        old.free();

        let reused = pool.alloc();
        assert_eq!(reused.id, old.id);
        assert_ne!(reused.generation, old.generation);

        drop(old);
        assert!(!pool.is_free(reused.id));
        assert_eq!(pool.free_count(), 0);
        assert!(pool.take_free_retirements(8).is_empty());

        drop(reused);
        assert_eq!(pool.take_free_retirements(8), vec![0]);
    }

    #[test]
    fn repeated_stale_free_calls_are_ignored_after_reuse() {
        let mut pool = IdPool::<u32>::default();
        let mut old = pool.alloc();
        old.free();
        old.free();

        let mut reused = pool.alloc();
        old.free();
        old.free();
        assert!(!pool.is_free(reused.id));
        assert_eq!(pool.free_count(), 0);

        reused.free();
        reused.free();
        assert_eq!(pool.free_count(), 1);
        assert_eq!(pool.take_free_retirements(8), vec![0]);
    }

    #[test]
    fn reuse_filter_publishes_the_new_generation_before_stale_drop() {
        let mut pool = IdPool::<u32>::default();
        let mut old = pool.alloc();
        old.free();

        let (reused, previous) = pool.alloc_with_reuse_filter(|_| true, 7);
        assert_eq!(previous, Some(0));
        assert_eq!(reused.id, old.id);
        assert_ne!(reused.generation, old.generation);

        drop(old);
        assert!(!pool.is_free(reused.id));
        drop(reused);
        assert_eq!(pool.take_free_retirements(8), vec![0]);
    }

    #[test]
    fn detached_borrowed_handle_cannot_free_an_owning_pool_slot() {
        let mut pool = IdPool::<u32>::default();
        let owner = pool.alloc();
        let mut borrowed = PoolId {
            id: owner.id,
            generation: owner.generation,
            free: IdPoolFree::default(),
        };

        borrowed.free();
        drop(borrowed);
        assert!(!pool.is_free(owner.id));
        assert_eq!(pool.live_count(), 1);

        drop(owner);
        assert_eq!(pool.take_free_retirements(8), vec![0]);
    }

    #[test]
    fn retirement_work_obeys_the_requested_bound() {
        let mut pool = IdPool::<u32>::default();
        let ids: Vec<_> = (0..5).map(|_| pool.alloc()).collect();
        drop(ids);

        assert_eq!(pool.take_free_retirements(2).len(), 2);
        assert_eq!(pool.take_free_retirements(2).len(), 2);
        assert_eq!(pool.take_free_retirements(2).len(), 1);
    }

    #[test]
    fn retirement_backlog_drains_to_quiescence_with_live_recycled_candidates() {
        const LIMIT: usize = 32;
        const DROP_COUNT: usize = LIMIT * 2 + 9;
        const RECYCLED_COUNT: usize = 5;

        let mut pool = IdPool::<u32>::default();
        let dropped: Vec<_> = (0..DROP_COUNT).map(|_| pool.alloc()).collect();
        drop(dropped);
        assert!(pool.has_pending_retirements());

        // Reuse a few queued slots before the first safe point. They remain
        // live and must be examined but not retired while the backlog drains.
        let recycled: Vec<_> = (0..RECYCLED_COUNT).map(|_| pool.alloc()).collect();
        let recycled_ids: Vec<_> = recycled.iter().map(|id| id.id).collect();

        let mut safe_points = 0;
        let mut retired = Vec::new();
        while pool.has_pending_retirements() {
            let batch = pool.take_free_retirements(LIMIT);
            assert!(batch.len() <= LIMIT);
            retired.extend(batch);
            safe_points += 1;
        }

        assert_eq!(safe_points, 3);
        assert_eq!(retired.len(), DROP_COUNT - RECYCLED_COUNT);
        assert!(recycled_ids.iter().all(|id| !retired.contains(id)));
        assert_eq!(pool.live_count(), RECYCLED_COUNT);
        assert!(!pool.has_pending_retirements());
        assert!(pool.take_free_retirements(LIMIT).is_empty());
    }

    #[test]
    fn retirement_bound_counts_live_candidates_examined() {
        const STALE: usize = 7;
        const LIMIT: usize = 3;

        let mut pool = IdPool::<u32>::default();
        let mut stale: Vec<_> = (0..STALE).map(|_| pool.alloc()).collect();
        for id in &mut stale {
            id.free();
        }
        let live: Vec<_> = (0..STALE).map(|_| pool.alloc()).collect();
        let tail = pool.alloc();
        let tail_id = tail.id;
        drop(tail);

        let pending_len = |pool: &IdPool<u32>| pool.free.0.borrow().retirement_pending.len();
        let before = pending_len(&pool);
        assert!(pool.take_free_retirements(LIMIT).is_empty());
        assert_eq!(before - pending_len(&pool), LIMIT);

        let before = pending_len(&pool);
        assert!(pool.take_free_retirements(LIMIT).is_empty());
        assert_eq!(before - pending_len(&pool), LIMIT);

        let before = pending_len(&pool);
        assert_eq!(pool.take_free_retirements(LIMIT), vec![tail_id]);
        assert!(before - pending_len(&pool) <= LIMIT);
        assert_eq!(pending_len(&pool), 0);

        drop(live);
        drop(stale);
    }

    #[test]
    fn repeated_slot_reuse_does_not_grow_pending_metadata() {
        let mut pool = IdPool::<u32>::default();
        for generation in 0..100 {
            let id = pool.alloc();
            assert_eq!(id.generation, generation);
            drop(id);
        }

        assert_eq!(pool.slot_count(), 1);
        assert_eq!(pool.free.0.borrow().retirement_pending.len(), 1);
        assert_eq!(pool.take_free_retirements(8), vec![0]);
    }
}
