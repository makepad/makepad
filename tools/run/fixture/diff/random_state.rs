// std::hash::RandomState as real std makes it (library/std/src/hash/random.rs): per-thread keys
// from OS randomness, k0 + 1 per RandomState::new, so states made one after another on a thread,
// states on other threads and (random_state.sh) other processes hash differently; a clone hashes
// the same; DefaultHasher::new has fixed keys. The `K` line (not compared) is this process's
// hash of 42, which random_state.sh compares across two processes.
use std::collections::HashMap;
use std::hash::{BuildHasher, DefaultHasher, Hasher, RandomState};

fn hash42(s: &RandomState) -> u64 {
    s.hash_one(42u64)
}

#[test]
fn run() {
    let a = RandomState::new();
    let b = RandomState::new();
    let c = a.clone();
    println!("D clone hashes the same: {}", hash42(&a) == hash42(&c));
    println!("D next state on the thread differs: {}", hash42(&a) != hash42(&b));
    let t = std::thread::spawn(|| hash42(&RandomState::new())).join().unwrap();
    println!("D other thread's state differs: {}", t != hash42(&a) && t != hash42(&b));
    let mut many = Vec::new();
    for _ in 0..64 {
        many.push(hash42(&RandomState::new()));
    }
    many.sort();
    many.dedup();
    println!("D 64 states on one thread all differ: {}", many.len() == 64);
    let (mut h1, mut h2) = (DefaultHasher::new(), DefaultHasher::new());
    h1.write_u64(42);
    h2.write_u64(42);
    println!("D DefaultHasher::new is fixed: {}", h1.finish() == h2.finish());
    // two maps with the same keys: their own states, so (with 256 keys, all but surely) another order
    let m1: HashMap<u32, ()> = (0..256).map(|k| (k, ())).collect();
    let m2: HashMap<u32, ()> = (0..256).map(|k| (k, ())).collect();
    println!("D two maps iterate in different orders: {}", m1.keys().ne(m2.keys()));
    println!("D debug hides the keys: {}", format!("{:?}", a) == "RandomState { .. }");
    println!("K {:016x}", hash42(&a));
}
