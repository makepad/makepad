// Panics unwinding through a mix of interpreted, JIT-compiled and native frames run every
// frame's drops in rustc's order. Functions named `xi_*` are the interpreted ones when the run sets
// MAKEPAD_RUN_INTERP=::xi_ (the threaded interpreter, codegen/src/interp.rs); `xj_*` stay JIT
// code; libc's qsort is a native frame between them. Each test prints its drop log as "| " lines,
// which must be identical under rustc (`rustc --test`), the JIT and every interpreter mix:
//   MAKEPAD_RUN_INTERP=all, =::xi_, =::xj_ (the opposite split), unset.
// interp/unwind_cmp.py runs the comparison.
use std::cell::RefCell;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

thread_local! { static LOG: RefCell<Vec<String>> = RefCell::new(Vec::new()); }
fn log(s: &str) {
    LOG.with(|l| l.borrow_mut().push(s.to_string()));
}
fn take() -> String {
    LOG.with(|l| std::mem::take(&mut *l.borrow_mut())).join(",")
}
struct D(&'static str);
impl Drop for D {
    fn drop(&mut self) {
        log(self.0);
    }
}
fn quiet() {
    std::panic::set_hook(Box::new(|_| {}));
}
fn payload(e: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = e.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = e.downcast_ref::<String>() {
        s.clone()
    } else if let Some(n) = e.downcast_ref::<u32>() {
        format!("u32 {}", n)
    } else {
        "?".to_string()
    }
}

// ---- chains alternating interpreted and JIT frames, panic at the bottom

#[inline(never)]
fn xi_a(n: u32) -> u32 {
    let _g = D("xi_a.g");
    let v = vec![D("xi_a.v0"), D("xi_a.v1")];
    let r = xj_b(n) + v.len() as u32;
    log("xi_a.after");
    r
}
#[inline(never)]
fn xj_b(n: u32) -> u32 {
    let _g = D("xj_b.g");
    let s = (D("xj_b.t0"), n);
    xi_c(s.1) + 1
}
#[inline(never)]
fn xi_c(n: u32) -> u32 {
    let _g = D("xi_c.g");
    let b = Box::new(D("xi_c.box"));
    let r = xj_d(n);
    drop(b);
    r
}
#[inline(never)]
fn xj_d(n: u32) -> u32 {
    let _g = D("xj_d.g");
    if n > 3 {
        panic!("deep {}", n);
    }
    n
}

#[test]
fn u01_alternating_chain() {
    quiet();
    let r = catch_unwind(|| {
        let _o = D("outer");
        xi_a(7)
    });
    println!("| chain {} {} {}", r.is_err(), r.as_ref().err().map(payload).unwrap_or_default(), take());
    let r = catch_unwind(|| xi_a(2));
    println!("| chain-ok {:?} {}", r.ok(), take());
}

// ---- the panic raised in interpreted code (bounds check, unwrap, explicit), JIT caller

#[inline(never)]
fn xi_index(v: &[u32], k: usize) -> u32 {
    let _g = D("xi_index.g");
    v[k]
}
#[inline(never)]
fn xi_unwrap(o: Option<u32>) -> u32 {
    let _g = D("xi_unwrap.g");
    o.unwrap() + 1
}
#[inline(never)]
fn xj_call_both(k: usize) -> u32 {
    let _g = D("xj_call_both.g");
    let a = xi_index(&[1, 2, 3], k);
    let b = xi_unwrap(if k == 1 { None } else { Some(5) });
    a + b
}

#[test]
fn u02_raised_in_interpreted_code() {
    quiet();
    for k in 0..3 {
        let r = catch_unwind(|| xj_call_both(if k == 2 { 9 } else { k }));
        println!("| raised {} {} {}", k, r.is_err(), take());
    }
}

// ---- nested catch_unwind inside interpreted and JIT frames, resume_unwind, panic_any

#[inline(never)]
fn xi_catcher(n: u32) -> u32 {
    let _g = D("xi_catcher.g");
    let r = catch_unwind(AssertUnwindSafe(|| {
        let _h = D("xi_catcher.inner");
        xj_d(n)
    }));
    match r {
        Ok(v) => v,
        Err(e) => {
            log(&format!("xi_catcher.caught[{}]", payload(&e)));
            if n > 10 {
                resume_unwind(e);
            }
            100
        }
    }
}
#[inline(never)]
fn xj_any(n: u32) -> u32 {
    let _g = D("xj_any.g");
    if n == 1 {
        std::panic::panic_any(42u32);
    }
    xi_catcher(n)
}

#[test]
fn u03_nested_catch_and_resume() {
    quiet();
    for n in [2u32, 5, 12, 1] {
        let r = catch_unwind(|| {
            let _o = D("outer");
            xj_any(n)
        });
        println!("| nested {} {:?} {} {}", n, r.as_ref().ok(), r.as_ref().err().map(payload).unwrap_or_default(), take());
    }
}

// ---- closures, trait objects and generic code on the unwind path

trait Step {
    fn step(&self, n: u32) -> u32;
}
struct Panicky(&'static str);
impl Step for Panicky {
    fn step(&self, n: u32) -> u32 {
        let _g = D(self.0);
        if n == 3 {
            panic!("dyn step");
        }
        n + 1
    }
}
#[inline(never)]
fn xi_run_steps(steps: &[Box<dyn Step>], f: &dyn Fn(u32) -> u32) -> u32 {
    let _g = D("xi_run_steps.g");
    let mut n = 0;
    for s in steps {
        let _k = D("xi_run_steps.iter");
        n = f(s.step(n));
    }
    n
}
#[inline(never)]
fn xj_generic<T: Clone, F: FnOnce(T) -> u32>(t: T, f: F) -> u32 {
    let _g = D("xj_generic.g");
    let c = t.clone();
    drop(t);
    f(c)
}

#[test]
fn u04_dyn_closures_generics() {
    quiet();
    let r = catch_unwind(|| {
        let steps: Vec<Box<dyn Step>> = vec![Box::new(Panicky("s0")), Box::new(Panicky("s1")), Box::new(Panicky("s2")), Box::new(Panicky("s3"))];
        let cap = D("closure.cap");
        xi_run_steps(&steps, &|n| {
            let _x = &cap;
            n + 1
        })
    });
    println!("| dyn {} {}", r.is_err(), take());
    let r = catch_unwind(|| {
        xj_generic(String::from("abc"), |s| {
            let _l = D("generic.closure");
            if s.len() == 3 {
                panic!("generic");
            }
            0
        })
    });
    println!("| generic {} {}", r.is_err(), take());
}

// ---- a native frame in the middle: qsort calling an interpreted extern "C" comparator

extern "C" {
    fn qsort(base: *mut u8, n: usize, size: usize, cmp: extern "C" fn(*const u8, *const u8) -> i32);
}
extern "C" fn xi_cmp(a: *const u8, b: *const u8) -> i32 {
    let (x, y) = unsafe { (*(a as *const u32), *(b as *const u32)) };
    log_count();
    (x > y) as i32 - (x < y) as i32
}
thread_local! { static CMPS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }
fn log_count() {
    CMPS.with(|c| c.set(c.get() + 1));
}
#[inline(never)]
fn xi_sort_then_panic(v: &mut [u32]) -> u32 {
    let _g = D("xi_sort.g");
    unsafe { qsort(v.as_mut_ptr() as *mut u8, v.len(), 4, xi_cmp) };
    let _h = D("xi_sort.h");
    if v[0] == 1 {
        panic!("after sort");
    }
    v[0]
}

#[test]
fn u05_native_frame_between() {
    quiet();
    let r = catch_unwind(|| {
        let mut v = vec![5u32, 3, 9, 1, 7];
        let out = xi_sort_then_panic(&mut v);
        (out, v)
    });
    println!("| native {} cmps>0 {} {}", r.is_err(), CMPS.with(|c| c.get()) > 0, take());
}

// ---- drops of a panicking Drop during normal scope exit, and a thread

struct Bomb(&'static str);
impl Drop for Bomb {
    fn drop(&mut self) {
        log(self.0);
        panic!("bomb");
    }
}
#[inline(never)]
fn xi_scope_with_bomb() {
    let _a = D("xi_bomb.a");
    let _b = Bomb("xi_bomb.b");
    let _c = D("xi_bomb.c");
}

#[test]
fn u06_panicking_drop_and_thread() {
    quiet();
    let r = catch_unwind(|| xi_scope_with_bomb());
    println!("| bomb {} {}", r.is_err(), take());
    let h = std::thread::spawn(|| {
        let _t = D("thread.t");
        xi_a(9)
    });
    let r = h.join();
    println!("| thread {} {}", r.is_err(), take());
}
