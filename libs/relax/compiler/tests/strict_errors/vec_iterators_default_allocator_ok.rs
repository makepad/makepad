//~ none
// The unstable allocator parameter of Vec's iterators (rustc 1.100.0) is only gated when a written
// type gives it an argument: the default, an inferred one, and Vec's own stable parameter compile.
use std::alloc::System;

fn first(d: std::vec::Drain<'_, u8>) -> usize {
    d.len()
}

fn main() {
    let mut v: Vec<u8, System> = Vec::new_in(System);
    v.push(1);
    let it = v.into_iter();
    let mut w = vec![1u8, 2];
    let n = first(w.drain(..));
    let k: std::vec::IntoIter<u8> = w.into_iter();
    println!("{} {} {}", it.len(), n, k.len());
}
