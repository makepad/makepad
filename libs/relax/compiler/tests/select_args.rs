// The call's bounds are selected before an argument coerces to a formal type with inference
// variables (rustc check_argument_types -> resolve_vars_with_obligations). `set.contains(&k)`
// with k: &K and `K: Borrow<Q>`: the one impl `Borrow<T> for T` makes Q = K, and `&&K` derefs
// to `&K`. SILENT before: Q = &K, every lookup missed. uizoo: makepad ComponentMap::retain_visible
// (`map.retain(|k, _| visible.contains(&k))`) dropped every live tab bar, so its draw list was
// freed while the parent still pointed at it, and the dock's page content never painted.
use std::collections::{HashMap, HashSet};
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Id(u64);
fn gen<K: Eq + std::hash::Hash + Copy>(s: &HashSet<K>, k: &K) -> bool { s.contains(&k) }
fn retain_visible<K: Eq + std::hash::Hash + Copy, V>(map: &mut HashMap<K, V>, visible: &HashSet<K>) {
    map.retain(|k, _| visible.contains(&k));
}
#[test] fn a_refref_concrete() { let mut s = HashSet::new(); s.insert(5u64); let k = 5u64; let r = &k; assert!(s.contains(&r)); }
#[test] fn b_refref_generic() { let mut s = HashSet::new(); s.insert(Id(3)); assert!(gen(&s, &Id(3))); assert!(!gen(&s, &Id(4))); }
#[test] fn c_retain_keeps_visible() {
    let mut m = HashMap::new(); m.insert(Id(7), 70); m.insert(Id(9), 90);
    let mut v = HashSet::new(); v.insert(Id(7));
    retain_visible(&mut m, &v);
    assert_eq!(m.len(), 1); assert_eq!(m.get(&Id(7)), Some(&70));
}
