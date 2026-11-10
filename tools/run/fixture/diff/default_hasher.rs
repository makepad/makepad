// diff_fast corpus program: DefaultHasher::new() / default() (fixed keys) give real std's values, SipHash-1-3 over the
// byte stream real core's Hash impls write (a str's 0xff, slices' length prefixes, integers at their widths), so a
// hash a program stores or compares is a native build's. (A RandomState's hasher may differ: its keys are random.)
use std::collections::hash_map::DefaultHasher as MapDefaultHasher;
use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher, Hash, Hasher};
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

#[derive(Hash)]
enum Shape { Dot, Box { w: u32, h: u32 }, Name(String) }
#[derive(Hash)]
struct Part { id: u64, shapes: Vec<Shape>, scale: Option<i16> }

#[inline(never)]
fn of<T: Hash + ?Sized>(x: &T) -> u64 {
    let mut h = DefaultHasher::new();
    x.hash(&mut h);
    h.finish()
}
fn work() {
    d!("str abc {:016x}", of("abc"));
    d!("str empty {:016x}", of(""));
    d!("String {:016x}", of(&String::from("the quick brown fox jumps")));
    d!("u64 42 {:016x}", of(&42u64));
    d!("u8 {:016x} i32 {:016x} u128 {:016x} usize {:016x} char {:016x} bool {:016x}", of(&7u8), of(&-7i32), of(&(u128::MAX - 5)), of(&123usize), of(&'z'), of(&true));
    d!("tuple {:016x}", of(&(1u16, "x", 'c', true, -3i64)));
    d!("vec u8 {:016x} slice u32 {:016x} array {:016x}", of(&vec![1u8, 2, 3]), of(&[5u32, 6, 7][..]), of(&[9u64; 3]));
    let part = Part { id: 9, shapes: vec![Shape::Dot, Shape::Box { w: 3, h: 4 }, Shape::Name("hub".into())], scale: Some(-2) };
    d!("derived {:016x}", of(&part));
    // writes that straddle the 8-byte tail, then integers
    let mut h = DefaultHasher::default();
    h.write(b"abc");
    h.write(b"defghijklmnop");
    h.write_u32(0xdead_beef);
    h.write_i8(-1);
    h.write_usize(1 << 40);
    d!("raw writes {:016x}", h.finish());
    d!("map DefaultHasher {:016x}", { let mut m = MapDefaultHasher::new(); 42u64.hash(&mut m); m.finish() });
    d!("BuildHasherDefault {:016x}", BuildHasherDefault::<DefaultHasher>::default().hash_one("abc"));
    // a clone carries on from where it was
    let mut a = DefaultHasher::new();
    "pre".hash(&mut a);
    let mut b = a.clone();
    1u32.hash(&mut a);
    1u32.hash(&mut b);
    d!("clone {}", a.finish() == b.finish());
}
