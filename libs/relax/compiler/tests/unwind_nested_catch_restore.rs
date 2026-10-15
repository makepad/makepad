// A panic caught, state put back and resumed (`catch_unwind` + `resume_unwind`, as makepad's
// `run_parked` / `with_vm_parked` do around a script VM) must reach every enclosing catch with
// each frame's drops run and each restore executed, through `extern "C-unwind"` closure
// trampolines and `&mut dyn FnMut` calls, for an index-out-of-bounds panic and an explicit one.
use std::cell::RefCell;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

thread_local! {
    static LOG: RefCell<Vec<&'static str>> = RefCell::new(Vec::new());
    static SLOT: RefCell<Option<u32>> = RefCell::new(Some(7));
    static HELD: RefCell<Option<u32>> = RefCell::new(None);
}
fn log(s: &'static str) { LOG.with(|l| l.borrow_mut().push(s)); }
fn take() -> Vec<&'static str> { LOG.with(|l| std::mem::take(&mut *l.borrow_mut())) }
struct G(&'static str);
impl Drop for G { fn drop(&mut self) { log(self.0); } }

struct Call<'a> { ctx: *mut u8, call: unsafe extern "C-unwind" fn(*mut u8), _b: std::marker::PhantomData<&'a mut u8> }
impl<'a> Call<'a> {
    fn new<F: FnMut()>(f: &'a mut F) -> Self {
        unsafe extern "C-unwind" fn tramp<F: FnMut()>(p: *mut u8) { (&mut *p.cast::<F>())() }
        Call { ctx: f as *mut F as *mut u8, call: tramp::<F>, _b: std::marker::PhantomData }
    }
    fn invoke(&mut self) { unsafe { (self.call)(self.ctx) } }
}

// run_parked: take the state, run, restore on both paths, resume the panic
fn parked(f: &mut Call) {
    let v = SLOT.with(|s| s.borrow_mut().take()).expect("taken");
    HELD.with(|h| *h.borrow_mut() = Some(v));
    let out = catch_unwind(AssertUnwindSafe(|| { let _g = G("parked body"); f.invoke(); }));
    // the holder gave it up (or lost it on the way: then the slot is already filled)
    let held = HELD.with(|h| h.borrow_mut().take());
    match out {
        Ok(()) => { SLOT.with(|s| *s.borrow_mut() = held); }
        Err(p) => { log("restore"); if let Some(v) = held { SLOT.with(|s| *s.borrow_mut() = Some(v)); } resume_unwind(p) }
    }
}
fn with_state<R>(f: impl FnOnce() -> R) -> R {
    let mut f = Some(f);
    let mut out = None;
    parked(&mut Call::new(&mut || out = Some((f.take().unwrap())())));
    out.unwrap()
}
// with_vm_parked: park inside, take back after, also across a panic
fn inner_parked(f: &mut dyn FnMut()) {
    let v = HELD.with(|h| h.borrow_mut().take()).unwrap();
    SLOT.with(|s| *s.borrow_mut() = Some(v));
    let out = catch_unwind(AssertUnwindSafe(|| f()));
    let back = SLOT.with(|s| s.borrow_mut().take());
    match (back, out) {
        (Some(v), Ok(())) => { HELD.with(|h| *h.borrow_mut() = Some(v)); }
        (None, Ok(())) => panic!("lost"),
        (back, Err(p)) => { log("inner restore"); if let Some(v) = back { HELD.with(|h| *h.borrow_mut() = Some(v)); } resume_unwind(p) }
    }
}
fn deep(n: u32, v: &[u32], i: usize) -> u32 {
    let _g = G("deep frame");
    if n == 0 { v[i] } else { deep(n - 1, v, i) + 1 }
}
fn slot_present() -> bool { SLOT.with(|s| s.borrow().is_some()) }

fn scenario(oob: bool) -> Vec<&'static str> {
    let v = vec![1, 2, 3];
    let r = catch_unwind(AssertUnwindSafe(|| {
        let _outer = G("outer");
        with_state(|| {
            let _a = G("a");
            let mut body = || {
                let _b = G("b");
                if oob { deep(2, &v, 9); } else { deep(2, &v, 0); panic!("explicit"); }
            };
            inner_parked(&mut body);
        });
    }));
    assert!(r.is_err());
    assert!(slot_present(), "the state came back after the unwind");
    take()
}

#[test]
fn explicit_and_oob_restore_every_level() {
    for oob in [true, false] {
        let l = scenario(oob);
        let mut want = vec!["deep frame", "deep frame", "deep frame", "b"];
        want.extend(["inner restore", "a", "parked body", "restore", "outer"]);
        assert_eq!(l, want, "oob={oob}");
        // and again: nothing left held
        assert_eq!(scenario(oob), want, "second run oob={oob}");
    }
}
