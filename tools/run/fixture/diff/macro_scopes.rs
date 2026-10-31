// diff_fast corpus program: textual macro_rules! scope ends with its block; an inner definition shadows the outer
// one only inside the block (rustc_resolve legacy scope). Every result line is `D ...`.
#![allow(unused_macros)]
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

macro_rules! v { () => { 1 } }

mod shadow {
    macro_rules! v { () => { 100 } }
    pub fn inside() -> i32 { v!() }
}
mod plain { pub fn after() -> i32 { v!() } }

fn work() {
    macro_rules! loc { () => { 2 } }
    let before = loc!();
    let c = || loc!();
    let inner = {
        macro_rules! loc { () => { 3 } }
        let a = loc!();
        {
            macro_rules! loc { () => { 4 } }
            a + loc!()
        }
    };
    let after = loc!();
    let top = { macro_rules! v { () => { 10 } } v!() };
    d!("{} {} {} {} {}", before, c(), inner, after, top);
    d!("{} {} {}", v!(), shadow::inside(), plain::after());
    let f = || { macro_rules! loc { () => { 5 } } loc!() };
    d!("{} {}", f(), loc!());
}
