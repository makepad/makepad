//~ none
// accepted by rustc: deferred init (also through a tuple pattern), mut bindings, &mut params,
// ref mut bindings, Cell through &, a moved-in closure that does not mutate
use std::cell::Cell;
struct S { n: u32, c: Cell<u32> }
fn g(s: &mut S, r: &S) { s.n += 1; r.c.set(2); }
fn main() {
    let (a, b); a = 1; b = 2;
    let mut s = S { n: 0, c: Cell::new(0) };
    let t = S { n: 1, c: Cell::new(1) };
    g(&mut s, &t);
    let mut v = vec![1u8]; { let r = &mut v; r.push(2); }
    let w = &mut v; w.push(3);
    if let Some(ref mut x) = Some(5u8) { *x += 1; }
    let k = 7; let f = move || k + 1;
    println!("{} {} {} {} {}", a, b, s.n, v.len(), f());
}
