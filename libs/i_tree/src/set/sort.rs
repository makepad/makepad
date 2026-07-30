use core::cmp::Ordering;

pub trait KeyValue<K> {
    fn key(&self) -> &K;
}

pub trait SetCollection<K, V> {
    fn insert(&mut self, val: V);
    fn delete_by_index(&mut self, index: u32);
    fn index_before(&self, index: u32) -> u32;
    fn first_index_less_by<F>(&self, f: F) -> u32
    where
        F: Fn(&K) -> Ordering;

    /// # Safety
    ///
    /// `index` must reference an existing element in this collection.
    unsafe fn value_by_index(&self, index: u32) -> &V;

    /// # Safety
    ///
    /// `index` must reference an existing element in this collection.
    unsafe fn value_by_index_mut(&mut self, index: u32) -> &mut V;
}
