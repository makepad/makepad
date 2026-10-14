// Explicit x86-64 conventions survive definitions, pointer casts, calls and unwinding.
// Run with `makepad test libs/relax/compiler/tests/x86_64_abi.rs` and rustc --test.
#![cfg(target_arch = "x86_64")]

#[repr(C)]
#[derive(Clone, Copy)]
struct Mixed { integer: i64, float: f64 }

fn mix(a: i64, b: f64, p: Mixed, c: i64, d: i64, e: f64, f: i64, g: i64, h: i64) -> Mixed {
    Mixed { integer: a + 3 * p.integer + 5 * c + 7 * d + 11 * f + 13 * g + 17 * h, float: b + 2.0 * p.float + 4.0 * e }
}

#[inline(never)]
extern "sysv64" fn sysv(a: i64, b: f64, p: Mixed, c: i64, d: i64, e: f64, f: i64, g: i64, h: i64) -> Mixed {
    mix(a, b, p, c, d, e, f, g, h)
}

#[inline(never)]
extern "win64" fn win(a: i64, b: f64, p: Mixed, c: i64, d: i64, e: f64, f: i64, g: i64, h: i64) -> Mixed {
    // Calling the opposite convention also exercises Win64's callee-saved registers.
    sysv(a, b, p, c, d, e, f, g, h)
}

fn check(p: Mixed) {
    assert_eq!(p.integer, 664);
    assert_eq!(p.float, 18.5);
}

#[cfg(not(windows))]
extern "sysv64" { fn abs(x: i32) -> i32; }
#[cfg(windows)]
extern "win64" { fn abs(x: i32) -> i32; }

#[test]
fn explicit_foreign_abi() {
    assert_eq!(unsafe { abs(-37) }, 37);
}

#[test]
fn explicit_abis_direct_and_pointer_roundtrip() {
    let p = Mixed { integer: 3, float: 4.0 };
    check(sysv(2, 1.5, p, 5, 7, 2.25, 11, 13, 17));
    check(win(2, 1.5, p, 5, 7, 2.25, 11, 13, 17));
    let s: extern "sysv64" fn(i64, f64, Mixed, i64, i64, f64, i64, i64, i64) -> Mixed = sysv;
    let w: extern "win64" fn(i64, f64, Mixed, i64, i64, f64, i64, i64, i64) -> Mixed = win;
    let s: extern "sysv64" fn(i64, f64, Mixed, i64, i64, f64, i64, i64, i64) -> Mixed = unsafe { std::mem::transmute(s as usize) };
    let w: extern "win64" fn(i64, f64, Mixed, i64, i64, f64, i64, i64, i64) -> Mixed = unsafe { std::mem::transmute(w as usize) };
    for _ in 0..3 {
        check(s(2, 1.5, p, 5, 7, 2.25, 11, 13, 17));
        check(w(2, 1.5, p, 5, 7, 2.25, 11, 13, 17));
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Big { a: i64, b: f64, c: i64 }

#[inline(never)]
extern "sysv64-unwind" fn sysv_unwind(p: Big, n: i64) -> Big {
    if n < 0 { panic!("sysv64 panic"); }
    Big { a: p.a + n, b: p.b * 2.0, c: p.c - n }
}

#[inline(never)]
extern "win64-unwind" fn win_unwind(p: Big, n: i64) -> Big {
    sysv_unwind(p, n)
}

struct Guard<'a>(&'a std::cell::Cell<u32>);
impl Drop for Guard<'_> {
    fn drop(&mut self) { self.0.set(self.0.get() + 1); }
}

#[test]
fn unwind_abis_with_memory_aggregates() {
    let p = Big { a: 3, b: 1.5, c: 10 };
    let s: extern "sysv64-unwind" fn(Big, i64) -> Big = sysv_unwind;
    let w: extern "win64-unwind" fn(Big, i64) -> Big = win_unwind;
    let a = s(p, 4);
    let b = w(p, 4);
    assert!(a.a == 7 && a.b == 3.0 && a.c == 6);
    assert!(b.a == 7 && b.b == 3.0 && b.c == 6);
    let drops = std::cell::Cell::new(0);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = Guard(&drops);
        s(p, -1)
    })).is_err());
    assert_eq!(drops.get(), 1);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = Guard(&drops);
        w(p, -1)
    })).is_err());
    assert_eq!(drops.get(), 2);
}
