use core::cmp::Ordering;

pub trait KeyExpCollection<K, E, V> {
    fn insert(&mut self, key: K, val: V, time: E);
    fn first_less(&mut self, time: E, default: V, key: K) -> V;
    fn first_less_or_equal_by<F>(&mut self, time: E, default: V, f: F) -> V
    where
        F: Fn(K) -> Ordering;
    fn clear(&mut self);
}
