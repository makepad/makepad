// A closure capturing a generic, Copy-in-the-instance key and comparing it with `==`
// (`|kv| kv.0 == k`, as std's HashMap::insert does). Rust captures `k` by shared reference in
// the generic body and in every instance (only scalars compare built in; rustc op.rs
// by_ref_binop). A warm run links some of this code from the disk cache and compiles the
// rest: both halves must agree on the closure's environment.
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Uid(u64);

struct Node {
    name: String,
    children: Vec<Uid>,
}

fn find_in<K: PartialEq + Copy, V>(items: &[(K, V)], k: K) -> Option<usize> {
    let pos = |kv: &(K, V)| kv.0 == k;
    items.iter().position(pos)
}

fn insert_all<K: PartialEq + Eq + std::hash::Hash + Copy, V>(map: &mut HashMap<K, V>, items: Vec<(K, V)>) {
    for (k, v) in items {
        map.insert(k, v);
    }
}

fn main() {
    let mut graph: HashMap<Uid, Node> = HashMap::new();
    let mut total = 0usize;
    for round in 0..50u64 {
        let items: Vec<(Uid, Node)> = (0..40u64)
            .map(|i| (Uid(round * 1000 + i), Node { name: format!("n{}", i), children: vec![Uid(i), Uid(i + 1)] }))
            .collect();
        let probe = Uid(round * 1000 + 7);
        total += find_in(&items, probe).unwrap_or(0);
        insert_all(&mut graph, items);
        if let Some(n) = graph.get_mut(&probe) {
            n.children.push(Uid(round));
        }
    }
    let kids: usize = graph.values().map(|n| n.children.len()).sum();
    let names: usize = graph.values().map(|n| n.name.len()).sum();
    println!("cache_warm_capture result {} {} {} {}", graph.len(), total, kids, names);
}
