// R11: a panic runs the drops of every frame it unwinds through (rustc MIR cleanup blocks),
// in the order rustc runs them, before catch_unwind (or the thread's start) catches it.
// Each test prints its drop log as "| " lines; tests/std/core_diff.sh-style comparison with
// rustc: the lines must be identical.
use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

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

fn inner(n: u32) -> u32 {
    let _a = D("inner.a");
    let _b = D("inner.b");
    if n > 2 {
        panic!("boom {}", n);
    }
    n
}
fn middle(n: u32) -> u32 {
    let _m = D("middle.m");
    let v = vec![D("vec.0"), D("vec.1")];
    let r = inner(n) + v.len() as u32;
    drop(v);
    r
}

#[test]
fn a01_frames_and_locals() {
    quiet();
    let r = catch_unwind(|| {
        let _o = D("outer.o");
        middle(5)
    });
    println!("| caught {} {}", r.is_err(), take());
    let r = catch_unwind(|| middle(1));
    println!("| ok {:?} {}", r.ok(), take());
}

fn boom() -> u32 {
    panic!("tmp")
}
fn two(_a: &D, b: u32) -> u32 {
    b
}

#[test]
fn a02_statement_temporaries() {
    quiet();
    let r = catch_unwind(|| {
        let _x = D("x");
        two(&D("temp"), boom())
    });
    println!("| temps {} {}", r.is_err(), take());
}

#[test]
fn a03_moved_and_conditional() {
    quiet();
    let r = catch_unwind(|| {
        let a = D("a");
        let b = D("b");
        let c = D("c");
        drop(b);
        let keep = if take_len() > 100 { Some(a) } else { None };
        let _ = keep;
        let moved = c;
        std::mem::forget(moved);
        let _d = D("d");
        boom()
    });
    println!("| moved {} {}", r.is_err(), take());
}
fn take_len() -> usize {
    LOG.with(|l| l.borrow().len())
}

#[test]
fn a04_loop_and_match() {
    quiet();
    let r = catch_unwind(|| {
        let _outer = D("outer");
        for i in 0..3 {
            let _it = D(["it0", "it1", "it2"][i]);
            match Some(D("m")) {
                Some(_x) if i == 1 => {
                    boom();
                }
                _ => {}
            }
        }
    });
    println!("| loop {} {}", r.is_err(), take());
}

#[test]
fn a05_rc_box_dyn() {
    quiet();
    let rc = Rc::new(5);
    let r = catch_unwind(AssertUnwindSafe(|| {
        let _c1 = rc.clone();
        let _c2 = rc.clone();
        let _b: Box<dyn std::any::Any> = Box::new(D("boxed"));
        boom()
    }));
    println!("| rc {} count {} {}", r.is_err(), Rc::strong_count(&rc), take());
}

#[test]
fn a06_mutex_guard_poison() {
    quiet();
    let m = Mutex::new(1);
    let r = catch_unwind(AssertUnwindSafe(|| {
        let mut g = m.lock().unwrap();
        *g += 1;
        let _t = D("held");
        boom();
    }));
    let v = match m.lock() {
        Ok(g) => *g,
        Err(p) => *p.into_inner(),
    };
    println!("| mutex {} poisoned {} value {} {}", r.is_err(), m.is_poisoned(), v, take());
}

#[test]
fn a07_thread_join() {
    quiet();
    let shared = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    struct T(Arc<Mutex<Vec<&'static str>>>, &'static str);
    impl Drop for T {
        fn drop(&mut self) {
            self.0.lock().unwrap().push(self.1);
        }
    }
    let s2 = shared.clone();
    let h = std::thread::spawn(move || {
        let _a = T(s2.clone(), "thread.a");
        let _b = T(s2, "thread.b");
        boom()
    });
    let r = h.join();
    println!("| thread {} {:?} strong {}", r.is_err(), shared.lock().unwrap(), Arc::strong_count(&shared));
}

#[test]
fn a08_closure_state_and_nested_catch() {
    quiet();
    let r = catch_unwind(|| {
        let _a = D("a");
        let inner = catch_unwind(|| {
            let _b = D("b");
            boom()
        });
        log(if inner.is_err() { "inner caught" } else { "inner ok" });
        let f = move || {
            let _c = D("c");
            boom()
        };
        f()
    });
    println!("| nested {} {}", r.is_err(), take());
}

#[test]
fn a09_panicking_in_drop() {
    quiet();
    struct P(&'static str);
    impl Drop for P {
        fn drop(&mut self) {
            log(&format!("{} panicking={}", self.0, std::thread::panicking()));
        }
    }
    let _ = catch_unwind(|| {
        let _p = P("p");
        boom()
    });
    {
        let _q = P("q");
    }
    println!("| panicking {} now={}", take(), std::thread::panicking());
}

struct PD(&'static str);
impl Drop for PD {
    fn drop(&mut self) {
        log(self.0);
        if self.0.starts_with('p') {
            panic!("drop {}", self.0);
        }
    }
}

#[test]
fn a10_panicking_drop_at_scope_end() {
    quiet();
    let r = catch_unwind(|| {
        let _a = D("a");
        let _p = PD("p1");
        let _b = D("b");
        let _t = (D("t0"), PD("q"), D("t2"));
    });
    println!("| scope end {} {}", r.is_err(), take());
    let r = catch_unwind(|| {
        let _x = D("x");
        keep(&PD("ptemp"), 3);
        log("after");
    });
    println!("| temp end {} {}", r.is_err(), take());
}

fn take2(_a: D, _b: u32) {}
impl D {
    fn consume(self, _n: u32) {}
}

#[test]
fn a11_evaluated_operands() {
    quiet();
    let r = catch_unwind(|| take2(D("arg"), boom()));
    println!("| args {} {}", r.is_err(), take());
    let r = catch_unwind(|| D("recv").consume(boom()));
    println!("| recv {} {}", r.is_err(), take());
    let r = catch_unwind(|| {
        let v = vec![D("e0"), D("e1")];
        let t = (D("tup"), boom());
        drop(v);
        t.1
    });
    println!("| aggregate {} {}", r.is_err(), take());
}
fn keep<T>(_a: &T, b: u32) -> u32 {
    b
}

struct Q(&'static str, bool);
impl Drop for Q {
    fn drop(&mut self) {
        log(self.0);
        if self.1 {
            panic!("drop {}", self.0);
        }
    }
}
struct TwoQ(Q, Q);

#[test]
fn a12_drop_and_replace() {
    quiet();
    let r = catch_unwind(|| {
        let mut v = Q("old", true);
        v = Q("new", false);
        let _ = &v;
    });
    println!("| local {} {}", r.is_err(), take());
    let r = catch_unwind(|| {
        let mut x = (Q("a", false), Q("b", true));
        x.1 = Q("c", false);
        let _ = &x;
    });
    println!("| field {} {}", r.is_err(), take());
}

#[test]
fn a13_glue_continues() {
    quiet();
    let r = catch_unwind(|| {
        let _t = TwoQ(Q("s0", true), Q("s1", false));
    });
    println!("| struct {} {}", r.is_err(), take());
    let r = catch_unwind(|| {
        let _a = [Q("e0", false), Q("e1", true), Q("e2", false)];
    });
    println!("| array {} {}", r.is_err(), take());
    let r = catch_unwind(|| {
        let _v = vec![Q("v0", true), Q("v1", false), Q("v2", false)];
    });
    println!("| vec {} {}", r.is_err(), take());
}

#[test]
fn a14_subslice_moves() {
    {
        let ar = [D("x0"), D("x1"), D("x2")];
        let [_, _y @ ..] = ar;
        log("bound");
        drop(_y);
        log("after");
    }
    println!("| {}", take());
}
