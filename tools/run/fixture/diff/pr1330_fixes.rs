// diff_fast corpus program: what PR #1330 adds that changes what a program computes or whether it
// compiles. `D <case> <value>` per case, equal to rustc's output.
// - std: result::Iter / IterMut and binary_heap::Iter as their own types (Debug output, size_hint,
//   clone, rev, last, Default, `&Result` / `&mut Result` / `&BinaryHeap` into_iter);
// - const generic parameters read by format strings (`{N}`) and as values, narrow and 128-bit;
// - `$crate` inside a macro_rules! defined by a macro of this crate; derives named by path;
//   `use m::name;` where `m` holds a private module and a public macro of that name;
// - overloaded indexing of unsized places (`Box<[T]>`, `Box<str>`, `str` through a range).
#![allow(unused_unsafe, dead_code, unused_mut, unused_variables, unreachable_code, unused_macros)]
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

use std::collections::{binary_heap, BinaryHeap};
use std::hint::black_box;

fn result_iters() {
    let r: Result<i32, &str> = black_box(Ok(5));
    let e: Result<i32, &str> = black_box(Err("no"));
    let mut it = r.iter();
    d!("res.iter {:?} {:?} {}", it, it.size_hint(), it.len());
    let c = it.clone();
    d!("res.next {:?} {:?} {:?}", it.next(), it.next(), it.size_hint());
    d!("res.clone {:?} {:?}", c.clone().rev().collect::<Vec<_>>(), c.count());
    d!("res.err {:?} {:?} {}", e.iter(), e.iter().size_hint(), e.iter().len());
    let mut it = r.iter();
    d!("res.back {:?} {:?}", it.next_back(), it.next());
    let mut m: Result<Vec<u8>, String> = black_box(Ok(vec![1, 2]));
    {
        let mut im = m.iter_mut();
        d!("res.iter_mut {:?} {:?}", im, im.size_hint());
        if let Some(v) = im.next() { v.push(3); }
        let rest = im.next();
        d!("res.iter_mut.after {:?}", rest);
    }
    for v in &mut m { v.push(4); }
    let mut s = 0u32;
    for v in &m { s += v.iter().map(|&x| x as u32).sum::<u32>(); }
    d!("res.into_iter {:?} {}", m, s);
    let mut me: Result<u8, u8> = black_box(Err(1));
    d!("res.iter_mut.err {:?}", me.iter_mut());
    d!("res.iter_mut.err.back {:?}", me.iter_mut().next_back());
    let total: i32 = [Ok::<i32, ()>(1), Err(()), Ok(3)].iter().flat_map(|r| r.iter()).sum();
    d!("res.flat_map {}", total);
}

fn heap_iters() {
    let h: BinaryHeap<i32> = black_box(vec![3, 1, 4, 1, 5, 9, 2, 6]).into_iter().collect();
    let it = h.iter();
    d!("heap.iter {:?} {:?} {}", it, it.size_hint(), it.len());
    d!("heap.rev {:?}", h.iter().rev().collect::<Vec<_>>());
    d!("heap.last {:?} {:?}", h.iter().last(), h.iter().nth(2));
    let mut it = h.iter();
    it.next();
    let c = it.clone();
    d!("heap.clone {:?} {:?} {:?}", it.next_back(), c, c.clone().count());
    let empty: binary_heap::Iter<'_, u8> = Default::default();
    d!("heap.default {:?} {} {:?}", empty, empty.len(), empty.clone().next());
    let mut sum = 0;
    for x in &h { sum += *x; }
    d!("heap.into_iter {} {}", sum, (&h).into_iter().filter(|&&x| x > 2).count());
    let e: BinaryHeap<String> = BinaryHeap::new();
    d!("heap.empty {:?}", e.iter());
}

fn fmt_n<const N: usize>() -> String { format!("{N}|{N:>4}|{N:x}") }
fn fmt_i8<const N: i8>() -> String { format!("{N} {N:?}") }
fn fmt_i128<const N: i128>() -> String { format!("{N}") }
fn fmt_u128<const N: u128>() -> String { format!("{N:#x}") }
fn fmt_bool<const B: bool>() -> String { format!("{B}") }
fn fmt_char<const C: char>() -> String { format!("{C}{C:?}") }
fn val_i128<const N: i128>() -> i128 { black_box(N) / 3 }
fn val_u128<const N: u128>() -> u128 { N.wrapping_add(black_box(1)) }
fn val_i8<const N: i8>() -> i64 { N as i64 * black_box(2) }
fn val_i16<const N: i16>() -> i32 { N as i32 }

fn const_params() {
    d!("const.usize {}", fmt_n::<42>());
    d!("const.i8 {} {}", fmt_i8::<-3>(), fmt_i8::<{ i8::MIN }>());
    d!("const.i128 {} {}", fmt_i128::<{ -170141183460469231731687303715884105728 }>(), fmt_i128::<{ 1 << 100 }>());
    d!("const.u128 {}", fmt_u128::<{ u128::MAX - 5 }>());
    d!("const.bool {} {}", fmt_bool::<true>(), fmt_bool::<false>());
    d!("const.char {}", fmt_char::<'é'>());
    d!("const.val {} {} {} {}", val_i128::<{ -(1 << 90) }>(), val_u128::<{ u128::MAX }>(), val_i8::<-7>(), val_i16::<-300>());
}

macro_rules! define_getter {
    ($name:ident) => {
        macro_rules! $name { () => { $crate::helper_value() } }
    };
}
fn helper_value() -> u32 { black_box(7) }
define_getter!(getter);

#[derive(std::fmt::Debug, core::clone::Clone, ::core::cmp::PartialEq, Default)]
struct Pathed(u8, &'static str);

mod zv {
    mod zerovec { pub fn f() -> u8 { 1 } }
    macro_rules! zerovec_impl { ($($x:expr),*) => { vec![$($x * 2),*] } }
    pub(crate) use zerovec_impl as zerovec;
}
use zv::zerovec;

fn macros_and_paths() {
    d!("crate.nested {}", getter!());
    let p = Pathed(3, "x");
    d!("derive.path {:?} {} {:?}", p.clone(), p == Pathed(3, "x"), Pathed::default());
    let v: Vec<i32> = zerovec![1, 2, 3];
    d!("use.macro {:?}", v);
}

fn unsized_index() {
    let b: Box<[u16]> = black_box(vec![10, 20, 30, 40]).into_boxed_slice();
    d!("index.box_slice {:?} {:?} {}", &b[1..3], &b[2..], b[black_box(3)]);
    let s: Box<str> = black_box(String::from("héllo")).into_boxed_str();
    d!("index.box_str {} {}", &s[3..], &s[..1]);
    let t: &str = black_box("abcdef");
    d!("index.str {} {} {}", &t[1..=2], &(*t)[4..], &t[..]);
    let mut v: Vec<Box<[u8]>> = vec![vec![1u8, 2, 3].into_boxed_slice()];
    v[0][1] = black_box(9);
    d!("index.nested {:?}", v);
}

fn work() {
    result_iters();
    heap_iters();
    const_params();
    macros_and_paths();
    unsized_index();
}
