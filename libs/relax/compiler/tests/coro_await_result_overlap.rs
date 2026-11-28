// An awaited future and the value its await returns may share the coroutine state's bytes only if
// the future is dropped before the value is written: here the value (an aggregate) lives on across
// a later await, and the awaited future's Drop writes its own fields. Each test runs coroutines to
// the end and drops them while suspended at every poll count, then checks that nothing leaked.
#![allow(dead_code)]

use std::future::Future;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, Ordering::Relaxed};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

struct Noop;
impl Wake for Noop {
    fn wake(self: Arc<Self>) {}
}

// Written between polls so a stack slot reused by mistake shows up deterministically.
#[inline(never)]
fn scrub_stack(seed: u64) -> u64 {
    let mut a = [0u64; 512];
    for (i, x) in a.iter_mut().enumerate() {
        *x = seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
    let a = std::hint::black_box(a);
    a.iter().fold(0, |h, x| h ^ x)
}

fn poll_n<F: Future>(n: u32, f: F) -> Option<F::Output> {
    let mut f = Box::pin(f);
    let w = Waker::from(Arc::new(Noop));
    let mut cx = Context::from_waker(&w);
    for i in 0..n {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return Some(v);
        }
        std::hint::black_box(scrub_stack(i as u64));
    }
    None
}

// Aligned like the aggregates, so a later variant's aggregate starts where the awaitee does.
#[repr(align(8))]
struct Yield(u32);
impl Future for Yield {
    type Output = u64;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u64> {
        if self.0 == 0 {
            return Poll::Ready(7);
        }
        self.0 -= 1;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

// Live-object counters: every creation and every drop is counted.
struct Stats {
    inners: AtomicI64,
    guards: AtomicI64,
    marks: AtomicI64,
}
struct Inner {
    id: u64,
    st: Arc<Stats>,
}
impl Inner {
    fn new(id: u64, st: &Arc<Stats>) -> Arc<Inner> {
        st.inners.fetch_add(1, Relaxed);
        Arc::new(Inner { id, st: st.clone() })
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.st.inners.fetch_sub(1, Relaxed);
    }
}

struct Svc {
    inner: Arc<Inner>,
    st: Arc<Stats>,
    open: bool,
}
#[derive(Debug, PartialEq)]
struct Closed;
struct Guard {
    inner: Arc<Inner>,
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.inner.st.guards.fetch_sub(1, Relaxed);
    }
}
impl Guard {
    async fn wait(&self) -> u64 {
        Yield(1).await + self.inner.id
    }
}

// An aggregate returned by value from an await: an Arc and an array, so not held in registers.
#[repr(C)]
struct Kept {
    arc: Arc<Inner>,
    buf: [u64; 2],
}
#[repr(C)]
struct Plain {
    arc: Arc<Inner>,
    a: u64,
    b: u64,
    c: u64,
}

// An awaited future whose Drop clears its first word, as an intrusive waiter unlinks itself.
#[repr(C)]
struct KeptFut {
    link: *const Inner,
    src: *const Arc<Inner>,
    polls: u32,
}
impl Future for KeptFut {
    type Output = Kept;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Kept> {
        assert!(!self.link.is_null(), "KeptFut intact when polled");
        if self.polls > 0 {
            self.polls -= 1;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        let arc = unsafe { (*self.src).clone() };
        Poll::Ready(Kept { arc, buf: [1, 2] })
    }
}
impl Drop for KeptFut {
    fn drop(&mut self) {
        self.link = std::ptr::null();
    }
}

#[repr(C)]
struct PlainFut {
    link: *const Inner,
    src: *const Arc<Inner>,
}
impl Future for PlainFut {
    type Output = Plain;
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Plain> {
        Poll::Ready(Plain { arc: unsafe { (*self.src).clone() }, a: 1, b: 2, c: 3 })
    }
}
impl Drop for PlainFut {
    fn drop(&mut self) {
        self.link = std::ptr::null();
    }
}

impl Svc {
    fn new(open: bool) -> Svc {
        let st = Arc::new(Stats { inners: AtomicI64::new(0), guards: AtomicI64::new(0), marks: AtomicI64::new(0) });
        Svc { inner: Inner::new(40, &st), st, open }
    }
    fn acquire(&self) -> Result<Guard, Closed> {
        if self.open {
            self.st.guards.fetch_add(1, Relaxed);
            Ok(Guard { inner: self.inner.clone() })
        } else {
            Err(Closed)
        }
    }
    fn kept(&self, polls: u32) -> KeptFut {
        KeptFut { link: &*self.inner, src: &self.inner, polls }
    }
    // An async fn as the first awaited future: it holds an Arc across its own await, and its
    // drop glue reads its own state. buf[0] is a pattern that, written over this future before
    // its drop, reads as a suspended state.
    async fn kept_async(&self, polls: u32) -> Kept {
        let held = self.inner.clone();
        Yield(polls).await;
        Kept { arc: held, buf: [0x0303_0303_0303_0303, 9] }
    }
}

// Nothing leaked and nothing double-dropped after a run or a suspended drop.
fn settled(s: &Svc, what: &str) {
    assert_eq!(Arc::strong_count(&s.inner), 1, "{what}: Arc strong count back to 1");
    assert_eq!(s.st.inners.load(Relaxed), 1, "{what}: every Inner but the service's own was dropped exactly once");
    assert_eq!(s.st.guards.load(Relaxed), 0, "{what}: every Guard created was dropped exactly once");
    assert_eq!(s.st.marks.load(Relaxed), 0, "{what}: every MarkFut created was dropped exactly once");
}

fn run<F: Future>(s: &Svc, what: &str, polls: u32, k: u32, f: F) -> Option<F::Output> {
    let r = poll_n(k, f);
    settled(s, &format!("{what} polls={polls} k={k}"));
    r
}

// `lead` is the number of Pending polls before `s.acquire()?.wait().await` is reached.
fn chain_expect(open: bool, lead: u32, k: u32, ok: u64) -> Option<Result<u64, Closed>> {
    if open {
        if k >= lead + 2 { Some(Ok(ok)) } else { None }
    } else if k >= lead + 1 {
        Some(Err(Closed))
    } else {
        None
    }
}

// An awaited future of 24 bytes whose Drop checks it is intact, then overwrites every word of
// itself. The Arc sits in MaybeUninit so a zero in word 0 stays a valid value after the drop.
const S32: u32 = 0xC0DE_F00D;
const S64: u64 = 0x0123_4567_89AB_CDEF;
const MARK: u64 = 0x5a5a_5a5a_5a5a_5a5a;
const OWN_ID: u64 = 40;
trait Make: Sized {
    fn make(own: &Arc<Inner>) -> Self;
}
impl Make for Kept {
    fn make(own: &Arc<Inner>) -> Kept {
        Kept { arc: own.clone(), buf: [1, 2] }
    }
}
// The returned Arc is a different allocation (id 77) from the one the future owns (id 40).
impl Make for Arc<Inner> {
    fn make(own: &Arc<Inner>) -> Arc<Inner> {
        Inner::new(77, &own.st)
    }
}
impl Make for (Arc<Inner>, u64) {
    fn make(own: &Arc<Inner>) -> (Arc<Inner>, u64) {
        (Inner::new(77, &own.st), 5)
    }
}
impl Make for (u64, u64) {
    fn make(_own: &Arc<Inner>) -> (u64, u64) {
        (11, 13)
    }
}
impl Make for () {
    fn make(_own: &Arc<Inner>) {}
}
#[repr(C)]
struct MarkFut<T: Make> {
    own: MaybeUninit<Arc<Inner>>,
    polls: u32,
    s32: u32,
    s64: u64,
    _t: PhantomData<fn() -> T>,
}
impl<T: Make> MarkFut<T> {
    fn new(s: &Svc, polls: u32) -> Self {
        s.st.marks.fetch_add(1, Relaxed);
        MarkFut { own: MaybeUninit::new(s.inner.clone()), polls, s32: S32, s64: S64, _t: PhantomData }
    }
}
impl<T: Make> Future for MarkFut<T> {
    type Output = T;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        assert_eq!(self.s32, S32, "MarkFut word 1 sentinel intact when polled");
        assert_eq!(self.s64, S64, "MarkFut word 2 sentinel intact when polled");
        if self.polls > 0 {
            self.polls -= 1;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        Poll::Ready(T::make(unsafe { self.own.assume_init_ref() }))
    }
}
impl<T: Make> Drop for MarkFut<T> {
    fn drop(&mut self) {
        assert_eq!(self.s32, S32, "MarkFut word 1 sentinel intact when Drop runs (result written over the awaitee first)");
        assert_eq!(self.s64, S64, "MarkFut word 2 sentinel intact when Drop runs (result written over the awaitee first)");
        let own = unsafe { self.own.assume_init_ref() };
        assert_eq!(own.id, OWN_ID, "MarkFut's own Arc intact when Drop runs");
        assert!(Arc::strong_count(own) >= 1, "MarkFut's own Arc live when Drop runs");
        let st = own.st.clone();
        unsafe { self.own.assume_init_drop() };
        st.marks.fetch_sub(1, Relaxed);
        let p = self as *mut Self as *mut u64;
        unsafe {
            std::ptr::write_volatile(p, 0);
            std::ptr::write_volatile(p.add(1), MARK);
            std::ptr::write_volatile(p.add(2), MARK);
        }
    }
}

// The reported chain: the Guard temporary lives across the second await. `kept` is read by
// value only: a borrow of it would keep it out of the bytes the awaitee used.
async fn chain(s: &Svc, polls: u32) -> Result<u64, Closed> {
    let kept = s.kept(polls).await;
    let v = s.acquire()?.wait().await;
    Ok(v + kept.buf[1])
}

#[test]
fn chain_guard_ok_complete_and_drop_suspended() {
    let s = Svc::new(true);
    for polls in 0..=2u32 {
        for k in 1..=6u32 {
            let r = run(&s, "chain open", polls, k, chain(&s, polls));
            assert_eq!(r, chain_expect(true, polls, k, 49), "chain open result polls={polls} k={k}");
        }
    }
}

#[test]
fn chain_guard_err_early_return_and_drop_suspended() {
    let s = Svc::new(false);
    for polls in 0..=2u32 {
        for k in 1..=4u32 {
            let r = run(&s, "chain closed", polls, k, chain(&s, polls));
            assert_eq!(r, chain_expect(false, polls, k, 0), "chain closed result polls={polls} k={k}");
        }
    }
}

async fn chain_async(s: &Svc, polls: u32) -> Result<u64, Closed> {
    let kept = s.kept_async(polls).await;
    let v = s.acquire()?.wait().await;
    let (b0, b1) = (kept.buf[0], kept.buf[1]);
    assert_eq!(b0, 0x0303_0303_0303_0303, "kept.buf[0] intact after the async fn awaitee's drop");
    Ok(v + b1)
}

#[test]
fn chain_with_async_fn_awaitee() {
    for open in [true, false] {
        let s = Svc::new(open);
        for polls in 0..=2u32 {
            for k in 1..=6u32 {
                let r = run(&s, "chain_async", polls, k, chain_async(&s, polls));
                assert_eq!(r, chain_expect(open, polls, k, 47 + 9), "chain_async result open={open} polls={polls} k={k}");
            }
        }
    }
}

async fn v2(s: &Svc, polls: u32) -> Result<u64, Closed> {
    let _kept = s.kept(polls).await;
    let v = s.acquire()?.wait().await;
    Ok(v)
}
async fn v3(s: &Svc) -> Result<u64, Closed> {
    let p = PlainFut { link: &*s.inner, src: &s.inner }.await;
    let v = s.acquire()?.wait().await;
    Ok(v + p.c)
}
async fn v4(s: &Svc, polls: u32) -> u64 {
    let kept = s.kept(polls).await;
    let y = Yield(1).await;
    y + kept.buf[0]
}

#[test]
fn result_dropped_only_plain_struct_and_no_guard() {
    let s = Svc::new(true);
    for polls in 0..=2u32 {
        for k in 1..=6u32 {
            let r = run(&s, "v2", polls, k, v2(&s, polls));
            assert_eq!(r, chain_expect(true, polls, k, 47), "v2 (result only dropped by Arc glue) polls={polls} k={k}");
            let r = run(&s, "v3", polls, k, v3(&s));
            assert_eq!(r, chain_expect(true, 0, k, 47 + 3), "v3 (4-word plain struct) polls={polls} k={k}");
            let r = run(&s, "v4", polls, k, v4(&s, polls));
            assert_eq!(r, if k >= polls + 2 { Some(8) } else { None }, "v4 (no guard, no ?) polls={polls} k={k}");
        }
    }
}

#[test]
fn awaitee_drop_verifies_sentinels_then_marker_fills_all_words() {
    assert_eq!(std::mem::size_of::<MarkFut<Kept>>(), 24, "MarkFut is the size of Kept");
    assert_eq!(std::mem::align_of::<MarkFut<Kept>>(), 8, "MarkFut aligns like Kept");
    assert_eq!(std::mem::size_of::<Kept>(), 24, "Kept is 24 bytes");
    async fn mark_chain(s: &Svc, polls: u32) -> Result<u64, Closed> {
        let kept = MarkFut::<Kept>::new(s, polls).await;
        let v = s.acquire()?.wait().await;
        let (b0, b1) = (kept.buf[0], kept.buf[1]);
        assert_eq!([b0, b1], [1, 2], "kept.buf intact after the awaitee's marker fill");
        Ok(v + b1)
    }
    for open in [true, false] {
        let s = Svc::new(open);
        for polls in 0..=2u32 {
            for k in 1..=5u32 {
                let r = run(&s, "mark_chain", polls, k, mark_chain(&s, polls));
                assert_eq!(r, chain_expect(open, polls, k, 47 + 2), "mark_chain result open={open} polls={polls} k={k}");
            }
        }
    }
}

async fn l_niche(s: &Svc, polls: u32) -> u64 {
    let arc = MarkFut::<Arc<Inner>>::new(s, polls).await;
    let y = Yield(1).await;
    arc.id + y
}
async fn l_addr_taken(s: &Svc, polls: u32) -> u64 {
    let arc = MarkFut::<Arc<Inner>>::new(s, polls).await;
    let r = &arc;
    let y = Yield(1).await;
    r.id + y
}
async fn l_pair(s: &Svc, polls: u32) -> u64 {
    let t = MarkFut::<(Arc<Inner>, u64)>::new(s, polls).await;
    let y = Yield(1).await;
    t.0.id + t.1 + y
}
async fn l_mem_poll(s: &Svc, polls: u32) -> u64 {
    let (a, b) = MarkFut::<(u64, u64)>::new(s, polls).await;
    let y = Yield(1).await;
    a + b + y
}
async fn l_zst(s: &Svc, polls: u32) -> u64 {
    MarkFut::<()>::new(s, polls).await;
    let kept = Kept { arc: s.inner.clone(), buf: [3, 4] };
    let y = Yield(1).await;
    kept.buf[1] + y
}

#[test]
fn val_l_register_and_addr_taken_results() {
    let s = Svc::new(true);
    for polls in 0..=1u32 {
        for k in 1..=4u32 {
            let done = k >= polls + 2;
            let r = run(&s, "l_niche", polls, k, l_niche(&s, polls));
            assert_eq!(r, if done { Some(77 + 7) } else { None }, "Poll<Arc> in one register polls={polls} k={k}");
            let r = run(&s, "l_addr_taken", polls, k, l_addr_taken(&s, polls));
            assert_eq!(r, if done { Some(77 + 7) } else { None }, "Poll<Arc> address-taken local polls={polls} k={k}");
            let r = run(&s, "l_pair", polls, k, l_pair(&s, polls));
            assert_eq!(r, if done { Some(77 + 5 + 7) } else { None }, "Poll<(Arc, u64)> in two registers polls={polls} k={k}");
            let r = run(&s, "l_mem_poll", polls, k, l_mem_poll(&s, polls));
            assert_eq!(r, if done { Some(11 + 13 + 7) } else { None }, "Poll<(u64, u64)> read from memory into registers polls={polls} k={k}");
            let r = run(&s, "l_zst", polls, k, l_zst(&s, polls));
            assert_eq!(r, if done { Some(4 + 7) } else { None }, "Poll<()> ZST result polls={polls} k={k}");
        }
    }
}

async fn pat_struct(s: &Svc, polls: u32) -> u64 {
    let Kept { arc, buf } = s.kept(polls).await;
    let y = Yield(1).await;
    y + buf[1] + arc.id
}
async fn pair_fut(s: &Svc, polls: u32) -> (Kept, Kept) {
    let a = s.kept(polls).await;
    let b = s.kept(0).await;
    (a, b)
}
async fn pat_partial(s: &Svc, polls: u32) -> u64 {
    let (k1, _) = pair_fut(s, polls).await;
    let y = Yield(1).await;
    y + k1.buf[0] + k1.arc.id
}
async fn opt_kept_fut(s: &Svc, polls: u32, some: bool) -> Option<Kept> {
    let k = s.kept(polls).await;
    if some { Some(k) } else { None }
}
async fn pat_let_else(s: &Svc, polls: u32, some: bool) -> Result<u64, Closed> {
    let Some(kept) = opt_kept_fut(s, polls, some).await else {
        return Err(Closed);
    };
    let v = s.acquire()?.wait().await;
    Ok(v + kept.buf[1])
}

#[test]
fn result_bound_in_patterns() {
    let s = Svc::new(true);
    for polls in 0..=2u32 {
        for k in 1..=5u32 {
            let done = k >= polls + 2;
            let r = run(&s, "pat_struct", polls, k, pat_struct(&s, polls));
            assert_eq!(r, if done { Some(7 + 2 + 40) } else { None }, "struct pattern polls={polls} k={k}");
            let r = run(&s, "pat_partial", polls, k, pat_partial(&s, polls));
            assert_eq!(r, if done { Some(7 + 1 + 40) } else { None }, "partial move pattern polls={polls} k={k}");
            let r = run(&s, "let_else some", polls, k, pat_let_else(&s, polls, true));
            assert_eq!(r, chain_expect(true, polls, k, 47 + 2), "let-else Some path polls={polls} k={k}");
            let r = run(&s, "let_else none", polls, k, pat_let_else(&s, polls, false));
            assert_eq!(r, if k >= polls + 1 { Some(Err(Closed)) } else { None }, "let-else None path polls={polls} k={k}");
        }
    }
}

async fn expr_tuple(s: &Svc) -> u64 {
    let (a, b) = (s.kept(1).await, s.kept(0).await);
    let y = Yield(1).await;
    y + a.buf[0] + b.buf[1] + a.arc.id + b.arc.id
}
async fn expr_temp(s: &Svc) -> Result<u64, Closed> {
    let n = s.kept(1).await.buf[1] + s.acquire()?.wait().await;
    Ok(n)
}
async fn consume_fut(k: Kept) -> u64 {
    let y = Yield(1).await;
    y + k.buf[0] + k.arc.id
}
async fn expr_nested(s: &Svc) -> u64 {
    consume_fut(s.kept(1).await).await
}

#[test]
fn await_inside_larger_expressions() {
    let s = Svc::new(true);
    let c = Svc::new(false);
    for k in 1..=5u32 {
        let r = run(&s, "expr_tuple", 0, k, expr_tuple(&s));
        assert_eq!(r, if k >= 3 { Some(7 + 1 + 2 + 40 + 40) } else { None }, "tuple of two awaits k={k}");
        let r = run(&s, "expr_temp open", 0, k, expr_temp(&s));
        assert_eq!(r, if k >= 3 { Some(Ok(2 + 47)) } else { None }, "await temporary spanning an await, open k={k}");
        let r = run(&c, "expr_temp closed", 0, k, expr_temp(&c));
        assert_eq!(r, if k >= 2 { Some(Err(Closed)) } else { None }, "await temporary dropped on ? exit k={k}");
        let r = run(&s, "expr_nested", 0, k, expr_nested(&s));
        assert_eq!(r, if k >= 3 { Some(7 + 1 + 40) } else { None }, "await as operand of an outer future k={k}");
    }
}

async fn looped(s: &Svc, polls: u32) -> u64 {
    let mut acc = 0;
    for _ in 0..3 {
        let kept = MarkFut::<Kept>::new(s, polls).await;
        let y = Yield(1).await;
        acc += y + kept.buf[0];
    }
    acc
}
async fn deferred(s: &Svc, cond: bool) -> u64 {
    let kept;
    if cond {
        kept = s.kept(1).await;
    } else {
        kept = s.kept(0).await;
    }
    let y = Yield(1).await;
    y + kept.buf[1]
}

#[test]
fn await_in_loop_and_deferred_init() {
    let s = Svc::new(true);
    for polls in 0..=2u32 {
        for k in 1..=12u32 {
            let r = run(&s, "looped", polls, k, looped(&s, polls));
            assert_eq!(r, if k >= 3 * (polls + 1) + 1 { Some(3 * 8) } else { None }, "loop of three awaits polls={polls} k={k}");
        }
    }
    for cond in [true, false] {
        for k in 1..=4u32 {
            let r = run(&s, "deferred", 0, k, deferred(&s, cond));
            let need = if cond { 3 } else { 2 };
            assert_eq!(r, if k >= need { Some(7 + 2) } else { None }, "deferred init cond={cond} k={k}");
        }
    }
}

async fn make_inner_fut<'a>(s: &'a Svc, polls: u32) -> impl Future<Output = Kept> + 'a {
    // The returned coroutine captures `tag`, a pattern that, written over this future before its
    // drop, reads as a suspended state.
    let tag = Yield(polls).await | 0x0303_0303_0303_0300;
    async move {
        Yield(1).await;
        Kept { arc: s.inner.clone(), buf: [tag, 9] }
    }
}
async fn nested_coro(s: &Svc) -> u64 {
    let next = make_inner_fut(s, 1).await;
    let y = Yield(1).await;
    let k = next.await;
    assert_eq!(k.buf[0], 0x0303_0303_0303_0307, "nested coroutine capture intact after the outer awaitee's drop");
    y + k.buf[1] + k.arc.id
}

#[test]
fn await_returning_nested_coroutine() {
    let s = Svc::new(true);
    for k in 1..=6u32 {
        let r = run(&s, "nested_coro", 0, k, nested_coro(&s));
        assert_eq!(r, if k >= 4 { Some(7 + 9 + 40) } else { None }, "await returning a coroutine k={k}");
    }
}
