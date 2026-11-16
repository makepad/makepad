// diff_fast corpus program: derives see only cfg-enabled generic parameters, including
// lifetimes and consts; nested default types and function bounds keep their commas/arrows.
// rustc --test -O and Relax must print identical D lines.
use std::collections::BTreeMap;
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }

#[derive(Clone, Debug, PartialEq, Default)]
struct Record<T, #[cfg(any())] S, #[cfg(all())] S = BTreeMap<T, Vec<u8>>,> where S: Clone {
    values: Vec<T>, state: S,
}
#[derive(Clone, Debug, PartialEq, Default)]
enum Choice<T, #[cfg(any())] S, #[cfg(all())] S = T> {
    Item(T, S),
    #[default]
    Empty,
}
#[derive(Clone, Debug, PartialEq, Default)]
struct Borrowed<#[cfg(any())] 'a, #[cfg(all())] 'a> { text: &'a str }
#[derive(Clone, Debug, PartialEq, Default)]
struct Counted<#[cfg(any())] const N: usize> { value: u8 }
#[derive(Clone)]
struct Callback<F: Fn(u8) -> u8, #[cfg(any())] S, #[cfg(all())] S = Vec<u8>,> { f: F, state: S }

fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

fn work() {
    let mut state = BTreeMap::new();
    state.insert(5u8, vec![8u8, 13]);
    let r: Record<u8> = Record { values: vec![2, 3], state };
    let c = r.clone();
    let empty: Record<u8> = Default::default();
    d!("record clone={:?} equal={} default={:?}", c, r == c, empty);
    let e: Choice<u8> = Choice::Item(21, 34);
    let c = e.clone();
    let empty: Choice<u8> = Default::default();
    d!("enum clone={:?} equal={} default={:?}", c, e == c, empty);
    let b = Borrowed { text: "cfg" };
    let c = b.clone();
    let empty: Borrowed<'_> = Default::default();
    d!("lifetime clone={:?} equal={} default={:?}", c, b == c, empty);
    let n = Counted { value: 55 };
    let c = n.clone();
    let empty: Counted = Default::default();
    d!("const clone={:?} equal={} default={:?}", c, n == c, empty);
    let f = Callback { f: |x: u8| x + 1, state: vec![89u8] };
    let c = f.clone();
    d!("callback value={} state={:?}", (c.f)(4), c.state);
}
