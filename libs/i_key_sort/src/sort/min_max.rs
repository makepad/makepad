use crate::sort::key::KeyFn;

pub(crate) trait MinMax<T> {
    fn min_max<K, F>(&self, key: F) -> (K, K)
    where
        K: Copy + Ord,
        F: KeyFn<T, K>;
}

impl<T> MinMax<T> for [T] {
    #[inline(always)]
    fn min_max<K, F: KeyFn<T, K>>(&self, key: F) -> (K, K)
    where
        K: Copy + Ord,
    {
        debug_assert!(!self.is_empty());
        let first_val = self.first().unwrap();
        let first_key = key(first_val);

        let mut min_key = first_key;
        let mut max_key = first_key;

        for val in self.iter().skip(1) {
            let k = key(val);
            min_key = min_key.min(k);
            max_key = max_key.max(k);
        }

        (min_key, max_key)
    }
}
