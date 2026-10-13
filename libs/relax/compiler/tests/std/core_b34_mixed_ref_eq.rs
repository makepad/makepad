use std::collections::HashMap;
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct K(u64);
struct M<K, V> { vec: Vec<(K, V)>, spill: Option<Box<HashMap<K, V>>> }
impl<K: Eq + std::hash::Hash + Copy, V> M<K, V> {
    fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        if let Some(spill) = &mut self.spill {
            return spill.get_mut(key);
        }
        for (k, v) in self.vec.iter_mut() {
            if k == key {
                return Some(v);
            }
        }
        None
    }
}
fn f() -> u32 { let mut m = M { vec: vec![(K(1), 10u32), (K(2), 20)], spill: None }; *m.get_mut(&K(2)).unwrap() += 1; m.vec[1].1 }
#[test] fn t() { assert!(f() == 21); }
