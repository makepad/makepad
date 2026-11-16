// The try_reserve family returns Err for a size the allocator cannot give, or one past the
// collection's maximum, and the collection stays usable: String, Vec, VecDeque, BinaryHeap,
// OsString, PathBuf, HashMap and HashSet. Relax issue 231: `try_reserve_exact` of an absurd size
// aborted ("memory allocation failed") instead of returning Err, and VecDeque, BinaryHeap, OsString
// and PathBuf lacked some of these methods.
// Differential file: `| ` lines must match under rustc --test and makepad test.

use std::collections::{BinaryHeap, HashMap, HashSet, TryReserveError, VecDeque};
use std::ffi::OsString;
use std::path::PathBuf;

fn say(what: &str, r: Result<(), TryReserveError>) {
    match r {
        Ok(()) => println!("| {what}: ok"),
        Err(e) => println!("| {what}: {e}"),
    }
}

#[test]
fn a01_strings_and_vectors() {
    let mut s = String::from("kept");
    say("String exact usize::MAX", s.try_reserve_exact(usize::MAX));
    say("String exact isize::MAX", s.try_reserve_exact(isize::MAX as usize));
    say("String exact 2^60", s.try_reserve_exact(1 << 60));
    say("String 2^60", s.try_reserve(1 << 60));
    let mut v: Vec<u64> = vec![1, 2];
    say("Vec<u64> exact 2^58", v.try_reserve_exact(1 << 58));
    say("Vec<u64> usize::MAX/4", v.try_reserve(usize::MAX / 4));
    s.push_str(" and more");
    v.push(3);
    say("String 16", s.try_reserve(16));
    say("Vec 16", v.try_reserve_exact(16));
    println!("| {s} {v:?} {}", s.capacity() >= s.len() + 16);
}

#[test]
fn a02_deque_and_heap() {
    let mut d: VecDeque<u8> = (0..6).collect();
    d.rotate_left(4);
    say("VecDeque exact 2^60", d.try_reserve_exact(1 << 60));
    say("VecDeque 2^60", d.try_reserve(1 << 60));
    say("VecDeque usize::MAX", d.try_reserve(usize::MAX));
    say("VecDeque exact 64", d.try_reserve_exact(64));
    d.push_back(9);
    println!("| {d:?} {}", d.capacity() >= 64);
    let mut h: BinaryHeap<u32> = [5, 1, 9].into_iter().collect();
    say("BinaryHeap exact 2^60", h.try_reserve_exact(1 << 60));
    say("BinaryHeap 2^60", h.try_reserve(1 << 60));
    say("BinaryHeap 8", h.try_reserve(8));
    h.push(7);
    h.shrink_to(2);
    println!("| {:?} {}", h.into_sorted_vec(), true);
}

#[test]
fn a03_os_strings_paths_and_maps() {
    let mut o = OsString::from("os");
    say("OsString exact 2^60", o.try_reserve_exact(1 << 60));
    say("OsString 2^60", o.try_reserve(1 << 60));
    o.reserve_exact(10);
    o.push("-more");
    o.shrink_to_fit();
    let mut p = PathBuf::from("a/b");
    say("PathBuf exact 2^60", p.try_reserve_exact(1 << 60));
    say("PathBuf 2^60", p.try_reserve(1 << 60));
    p.reserve_exact(4);
    p.push("c");
    p.shrink_to(0);
    println!("| {o:?} {} {}", p.display(), o.capacity() == o.len());
    let mut m: HashMap<u64, u64> = HashMap::new();
    let mut hs: HashSet<u32> = HashSet::new();
    println!("| HashMap 2^58: {}", m.try_reserve(1 << 58).is_err());
    println!("| HashSet 2^60: {}", hs.try_reserve(1 << 60).is_err());
    m.insert(1, 2);
    hs.insert(3);
    println!("| {m:?} {hs:?}");
}
