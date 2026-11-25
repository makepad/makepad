// diff_fast corpus program: the pointer parts of by-value pair and closure arguments take rustc's
// `noalias` (rustc_ty_utils abi.rs arg_attrs_for_rust_scalar on each Scalar/ScalarPair scalar):
// loops keep a `&mut` capture's target in a register while they write other memory (a fold's
// accumulator through a by-value closure, two captures, two `&mut` parameters carried in one loop
// with a call and an early exit in it, a `&mut [T]`'s data, an `Option<&mut T>`, a panic out of
// the loop), next to the parts that must stay aliased: a pair of `&Cell` (not frozen), a `&mut` to
// a !Unpin value written through a pointer into itself, raw pointers.
// rustc --test -O and `makepad test` (all tiers) must print identical D lines.
use std::cell::Cell;
use std::marker::PhantomPinned;
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

// the closure (one `&mut` capture: a Scalar) is called in a loop that also writes `out`
#[inline(never)]
fn fill_fold<F: FnMut(u64)>(out: &mut [u64], mut f: F) {
    for i in 0..out.len() {
        out[i] = (i as u64).wrapping_mul(0x9e37_79b9) >> 7;
        f(out[i]);
    }
}

// two captures (a ScalarPair), the loop writing through one and reading the other's target
#[inline(never)]
fn pair_fold<F: FnMut(u64) -> u64>(n: u64, mut f: F) -> u64 {
    let mut last = 0;
    for i in 0..n {
        last = f(i ^ last);
    }
    last
}

// two `&mut` parameters, both carried, a call in the loop (it may panic) and an early exit
struct Acc {
    n: u64,
    k: u32,
}

#[inline(never)]
fn check_step(x: u64) {
    if x == 0xdead {
        panic!("step");
    }
}

#[inline(never)]
fn two_carried(a: &mut Acc, b: &mut Acc, xs: &[u64]) -> usize {
    for (i, &x) in xs.iter().enumerate() {
        a.n = a.n.wrapping_mul(31).wrapping_add(x);
        b.n ^= a.n >> 5;
        b.k += 1;
        check_step(x);
        if b.n & 0x3f == 0x15 {
            return i;
        }
    }
    usize::MAX
}

// a `&mut [u64]` (a pair: data and length) next to a `&mut u64`
#[inline(never)]
fn scan(acc: &mut u64, xs: &mut [u64]) {
    for x in xs.iter_mut() {
        *acc = acc.wrapping_add(*x);
        *x = *acc;
    }
}

// a niche-filled `Option<&mut u64>` (a Scalar that may be null)
#[inline(never)]
fn count_into(o: Option<&mut u64>, xs: &[u64]) -> u64 {
    match o {
        Some(c) => {
            for x in xs {
                if x % 3 == 0 {
                    *c += 1;
                }
            }
            *c
        }
        None => u64::MAX,
    }
}

// a `&mut dyn` (data and vtable) called in a loop, and a `Box<dyn Fn>` (not Unpin: no noalias)
#[inline(never)]
fn dyn_loop(n: u64, f: &mut dyn FnMut(u64) -> u64) -> u64 {
    let mut s = 0;
    for i in 0..n {
        s ^= f(i);
    }
    s
}

#[inline(never)]
fn boxed_loop(n: u64, f: Box<dyn Fn(u64) -> u64>) -> u64 {
    (0..n).map(|i| f(i)).fold(0, |a, x| a.wrapping_mul(3).wrapping_add(x))
}

// `&Cell` parts are not frozen: both may name one cell
#[inline(never)]
fn cells(p: (&Cell<u32>, &Cell<u32>)) -> u32 {
    p.0.set(1);
    p.1.set(2);
    p.0.get()
}

// a closure over two `&Cell` in a loop
#[inline(never)]
fn cell_loop<F: FnMut(u32) -> u32>(n: u32, mut f: F) -> u32 {
    let mut s = 0u32;
    for i in 0..n {
        s = s.wrapping_mul(31).wrapping_add(f(i));
    }
    s
}

// `&mut` to a !Unpin value: the value may be written through a pointer into itself
struct SelfRef {
    x: u32,
    p: *mut u32,
    _pin: PhantomPinned,
}

#[inline(never)]
fn pinned(p: (&mut SelfRef, u32)) -> u32 {
    p.0.x = p.1;
    unsafe { *p.0.p = p.1 + 1 };
    p.0.x
}

// raw pointer parts may alias
#[inline(never)]
fn raws(p: (*mut u32, *mut u32)) -> u32 {
    unsafe {
        *p.0 = 1;
        *p.1 = 2;
        *p.0
    }
}

fn work() {
    let mut acc = 0u64;
    let mut out = vec![0u64; 300];
    fill_fold(&mut out, |x| acc = acc.wrapping_mul(7).wrapping_add(x));
    d!("fill {} {} {}", acc, out[17], out[299]);

    let (mut a, mut b) = (1u64, 0u64);
    let last = pair_fold(500, |x| {
        a = a.wrapping_mul(x | 1);
        b ^= a >> 3;
        b
    });
    d!("pair {} {} {}", a, b, last);

    let (mut x, mut y) = (Acc { n: 1, k: 0 }, Acc { n: 2, k: 0 });
    let ys: Vec<u64> = (0..300u64).map(|i| i * 7 + 3).collect();
    let at = two_carried(&mut x, &mut y, &ys);
    d!("two {} {} {} {}", at, x.n, y.n, y.k);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let zs: Vec<u64> = (0..50u64).map(|i| if i == 20 { 0xdead } else { i }).collect();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| two_carried(&mut x, &mut y, &zs)));
    std::panic::set_hook(hook);
    d!("two unwind {} {} {} {}", r.is_err(), x.n, y.n, y.k);

    let mut total = 5u64;
    let mut xs: Vec<u64> = (0..200).map(|i| i * i).collect();
    scan(&mut total, &mut xs);
    d!("scan {} {} {}", total, xs[100], xs[199]);

    let mut c = 2u64;
    d!("count {} {} {}", count_into(Some(&mut c), &xs), c, count_into(None, &xs));

    let mut hits = 0u64;
    let s = dyn_loop(100, &mut |i| {
        hits += i & 1;
        i * hits
    });
    let k = std::hint::black_box(9u64);
    d!("dyn {} {} {}", s, hits, boxed_loop(50, Box::new(move |i| i * k + 1)));

    // a panic from the closure leaves the loop with the accumulator's latest value stored
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let (mut seen, mut calls) = (0u64, 0u32);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fill_fold(&mut out, |x| {
            seen = seen.wrapping_add(x);
            calls += 1;
            if calls == 40 {
                panic!("stop");
            }
        })
    }));
    std::panic::set_hook(hook);
    d!("unwind {} {} {} {}", r.is_err(), seen, calls, out[39]);

    let one = Cell::new(0u32);
    let f: fn((&Cell<u32>, &Cell<u32>)) -> u32 = std::hint::black_box(cells);
    let other = Cell::new(0u32);
    d!("cells {} {} {}", f((&one, &one)), f((&one, &other)), one.get() + other.get());

    let (p, q) = (Cell::new(1u32), Cell::new(2u32));
    let s = cell_loop(64, |i| {
        p.set(p.get().wrapping_add(i));
        q.set(q.get() ^ p.get());
        p.get().wrapping_add(q.get())
    });
    let r = &p;
    let t = cell_loop(64, |i| {
        r.set(i);
        p.set(p.get() * 3);
        r.get()
    });
    d!("cell loops {} {} {} {}", s, t, p.get(), q.get());

    let mut s = Box::pin(SelfRef { x: 0, p: std::ptr::null_mut(), _pin: PhantomPinned });
    let r = unsafe { s.as_mut().get_unchecked_mut() };
    r.p = &mut r.x;
    let g: fn((&mut SelfRef, u32)) -> u32 = std::hint::black_box(pinned);
    d!("pinned {}", g((r, 40)));

    let mut v = 0u32;
    let pv = &mut v as *mut u32;
    let h: fn((*mut u32, *mut u32)) -> u32 = std::hint::black_box(raws);
    d!("raws {} {}", h((pv, pv)), v);
}
