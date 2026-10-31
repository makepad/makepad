// tokio's CachedParkThread::block_on shape (runtime/park.rs): a Waker from a `Result<Waker, _>?`,
// a Context borrowing it, a pinned `&mut F`, an inline(always) budget closure with a reset guard;
// the Waker's drop at the return must call the waker vtable's drop with its data (Relax called
// a stack address: the Waker's slot was overwritten while it was live).
use std::cell::Cell;
use std::future::Future;
use std::pin::{pin, Pin};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

thread_local! {
    static BUDGET: Cell<Option<u8>> = const { Cell::new(None) };
    static DROPS: Cell<u32> = const { Cell::new(0) };
}

struct Reset(Option<u8>);
impl Drop for Reset {
    fn drop(&mut self) { BUDGET.with(|b| b.set(self.0)); }
}

#[inline(always)]
fn with_budget<R>(budget: Option<u8>, f: impl FnOnce() -> R) -> R {
    let prev = BUDGET.with(|b| b.replace(budget));
    let _g = Reset(prev);
    f()
}

#[inline(always)]
fn budget<R>(f: impl FnOnce() -> R) -> R { with_budget(Some(128), f) }

unsafe fn w_clone(p: *const ()) -> RawWaker {
    RawWaker::new(Box::into_raw(Box::new(unsafe { *(p as *const u64) })) as *const (), &VT)
}
unsafe fn w_wake(_: *const ()) {}
unsafe fn w_drop(p: *const ()) {
    DROPS.with(|d| d.set(d.get() + 1));
    drop(unsafe { Box::from_raw(p as *mut u64) });
}
static VT: RawWakerVTable = RawWakerVTable::new(w_clone, w_wake, w_wake, w_drop);

#[derive(Debug)]
struct AccessError { _p: () }

struct Park { polls: u32 }
impl Park {
    fn waker(&self) -> Result<Waker, AccessError> {
        Ok(unsafe { Waker::from_raw(RawWaker::new(Box::into_raw(Box::new(7u64)) as *const (), &VT)) })
    }
    fn park(&mut self) { self.polls += 1; }
    fn block_on<F: Future>(&mut self, f: F) -> Result<F::Output, AccessError> {
        let waker = self.waker()?;
        let mut cx = Context::from_waker(&waker);
        let mut f = pin!(f);
        loop {
            if let Poll::Ready(v) = budget(|| f.as_mut().poll(&mut cx)) {
                return Ok(v);
            }
            self.park();
        }
    }
}

struct Rx { left: u32 }
impl Future for Rx {
    type Output = Result<(), ()>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.left == 0 { return Poll::Ready(Ok(())); }
        self.left -= 1;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

#[test]
fn block_on_mut_ref_future() {
    let mut rx = Rx { left: 3 };
    let mut p = Park { polls: 0 };
    let before = DROPS.with(|d| d.get());
    let r = p.block_on(&mut rx);
    assert!(matches!(r, Ok(Ok(()))));
    assert_eq!(p.polls, 3);
    assert_eq!(DROPS.with(|d| d.get()) - before, 1);
}

#[test]
fn block_on_ready_future() {
    let mut p = Park { polls: 0 };
    let before = DROPS.with(|d| d.get());
    assert_eq!(p.block_on(async { 42 }).unwrap(), 42);
    assert_eq!(DROPS.with(|d| d.get()) - before, 1);
}

// tokio runtime/park.rs unparker_to_raw_waker: `&RawWakerVTable::new(..)` of fn items is promoted
// to a 'static constant (rustc_mir_transform/src/promote_consts.rs: Validator::validate_call).
// A stack temporary instead leaves the Waker with a dangling vtable.
#[inline(never)]
unsafe fn raw_inline(p: *const ()) -> RawWaker {
    RawWaker::new(p, &RawWakerVTable::new(w_clone, w_wake, w_wake, w_drop))
}

#[inline(never)]
fn vtable_inline() -> &'static RawWakerVTable {
    &RawWakerVTable::new(w_clone, w_wake, w_wake, w_drop)
}

#[inline(never)]
fn clobber(n: u64) -> u64 {
    let a = [n; 64];
    std::hint::black_box(&a);
    if n == 0 { a.iter().sum() } else { clobber(n - 1) + a[(n % 64) as usize] }
}

#[test]
fn promoted_vtable_outlives_the_call() {
    let vt = vtable_inline();
    assert!(clobber(20) > 0);
    let before = DROPS.with(|d| d.get());
    let w = unsafe { Waker::from_raw(raw_inline(Box::into_raw(Box::new(9u64)) as *const ())) };
    clobber(30);
    let w2 = w.clone();
    drop(w);
    clobber(30);
    drop(w2);
    assert_eq!(DROPS.with(|d| d.get()) - before, 2);
    // the promoted table is the same 'static data after the stack below it was reused
    let w3 = unsafe { Waker::from_raw(RawWaker::new(Box::into_raw(Box::new(3u64)) as *const (), vt)) };
    clobber(30);
    drop(w3);
    assert_eq!(DROPS.with(|d| d.get()) - before, 3);
}

fn pointer_f() { DROPS.with(|d| d.set(d.get() + 1)); }
fn pointer_g() { DROPS.with(|d| d.set(d.get() + 2)); }

#[inline(never)]
fn pointer_array() -> &'static [fn(); 2] { &[pointer_f as fn(), pointer_g as fn()] }

#[inline(never)]
fn pointer_option() -> &'static Option<fn()> { &Some(pointer_f as fn()) }

#[test]
fn promoted_fn_pointer_array_outlives_the_call() {
    // rustc_mir_transform/src/promote_consts.rs: Validator::validate_rvalue accepts reification.
    let fs = pointer_array();
    clobber(30);
    let before = DROPS.with(|d| d.get());
    fs[0]();
    fs[1]();
    assert_eq!(DROPS.with(|d| d.get()) - before, 3);
}

#[test]
fn promoted_fn_pointer_option_outlives_the_call() {
    let f = pointer_option();
    clobber(30);
    let before = DROPS.with(|d| d.get());
    f.unwrap()();
    assert_eq!(DROPS.with(|d| d.get()) - before, 1);
}

#[inline(never)]
fn dynamic_vtable(clone: unsafe fn(*const ()) -> RawWaker, outer: Option<&RawWakerVTable>) {
    // A parameter operand is not promotable, even for a #[rustc_promotable] callee.
    let vt = &RawWakerVTable::new(clone, w_wake, w_wake, w_drop);
    if let Some(outer) = outer {
        assert!(!std::ptr::eq(outer, vt));
    } else {
        dynamic_vtable(clone, Some(vt));
    }
}

const fn ordinary_const_fn() -> RawWakerVTable {
    RawWakerVTable::new(w_clone, w_wake, w_wake, w_drop)
}

#[inline(never)]
fn ordinary_temporary(outer: Option<&RawWakerVTable>) {
    // Validator::validate_call does not promote an unmarked const fn in a runtime body.
    let vt = &ordinary_const_fn();
    if let Some(outer) = outer {
        assert!(!std::ptr::eq(outer, vt));
    } else {
        ordinary_temporary(Some(vt));
    }
}

#[test]
fn non_promotable_calls_keep_distinct_temporaries() {
    dynamic_vtable(w_clone, None);
    ordinary_temporary(None);
}

// tokio task/coop/mod.rs with_budget exactly: the guard comes back from a thread-local
// try_with as Result<ResetGuard, AccessError>, and the closure's value is a Poll<()> (one byte,
// no payload). Relax lost the Ready of `rt.block_on(tokio::task::yield_now())`: block_on parked
// and polled the finished yield_now again ("`async fn` resumed after completion").
#[derive(Clone, Copy)]
struct Budget2(Option<u8>);
thread_local! {
    static BUDGET2: Cell<Budget2> = const { Cell::new(Budget2(None)) };
}
fn budget_cell<R>(f: impl FnOnce(&Cell<Budget2>) -> R) -> Result<R, std::thread::AccessError> {
    BUDGET2.try_with(f)
}
#[inline(always)]
fn with_budget2<R>(budget: Budget2, f: impl FnOnce() -> R) -> R {
    struct ResetGuard { prev: Budget2 }
    impl Drop for ResetGuard {
        fn drop(&mut self) { let _ = budget_cell(|cell| cell.set(self.prev)); }
    }
    #[allow(unused_variables)]
    let maybe_guard = budget_cell(|cell| {
        let prev = cell.get();
        cell.set(budget);
        ResetGuard { prev }
    });
    f()
}

async fn yield_flag() {
    let mut yielded = false;
    std::future::poll_fn(|cx| {
        if yielded { return Poll::Ready(()); }
        yielded = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    })
    .await
}

fn block_on_budget<F: Future>(f: F) -> (F::Output, u32) {
    let waker = unsafe { Waker::from_raw(RawWaker::new(Box::into_raw(Box::new(1u64)) as *const (), &VT)) };
    let mut cx = Context::from_waker(&waker);
    let mut f = pin!(f);
    let mut polls = 0;
    loop {
        polls += 1;
        assert!(polls < 5, "polled after completion");
        if let Poll::Ready(v) = with_budget2(Budget2(Some(128)), || f.as_mut().poll(&mut cx)) {
            return (v, polls);
        }
    }
}

#[test]
fn unit_poll_through_budget_guard() {
    let ((), polls) = block_on_budget(yield_flag());
    assert_eq!(polls, 2);
    let (v, polls) = block_on_budget(async { yield_flag().await; 5u8 });
    assert_eq!((v, polls), (5, 2));
}
