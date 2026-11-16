// diff_fast corpus program: a value holding a future in an async fn stays in the frame while a
// borrow of it may still be used after a suspension point, though lowering marks where its storage
// ends. Each case holds a pair (a polled leaf future, an array) across an await, with the stack
// scribbled while suspended, and reads it after the await through a borrow that is not the slot's
// own address: a local borrowed into a struct; a temporary a `let` extends; `pin!`; a block's value
// passed to a call whose next argument awaits; a block's local moved out to a `let` and read through
// a borrow of a borrow; the futures of a `for`; a `match` scrutinee, by value and through `&mut`;
// `let..else` and `if let` scrutinees; `poll_fn` polling a local through `Pin::new_unchecked`; a
// block tail's temporary (edition 2021: it lives to the end of the enclosing statement).
// rustc --test -O and `makepad test` (all tiers) must print identical D lines.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

use std::future::Future;
use std::pin::{pin, Pin};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

struct Noop;
impl Wake for Noop { fn wake(self: Arc<Self>) {} }
struct YieldOnce(bool);
impl Future for YieldOnce {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 { Poll::Ready(()) } else { self.0 = true; cx.waker().wake_by_ref(); Poll::Pending }
    }
}
// a stack scribbler between polls: a frame that was not saved would be overwritten
#[inline(never)]
fn scribble(n: u64) -> u64 {
    let mut a = [n; 64];
    for i in 0..64 { a[i] = a[i].wrapping_mul(i as u64 + 3) ^ 0xdead_beef; }
    std::hint::black_box(&mut a);
    a.iter().sum()
}
fn block_on_scribbled<F: Future>(f: F) -> F::Output {
    let mut f = Box::pin(f);
    let w = Waker::from(Arc::new(Noop));
    let mut cx = Context::from_waker(&w);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) { return v; }
        std::hint::black_box(scribble(9));
    }
}

fn arr(n: u64) -> [u64; 8] { [n, n * 3, n ^ 0x55, n + 7, n << 4, !n, n * n, n + 1] }
fn sum(a: &[u64; 8]) -> u64 { a.iter().fold(17u64, |h, &x| h.wrapping_mul(31).wrapping_add(x)) }
// its array lives in its own frame across its await
async fn leaf(n: u64) -> u64 {
    let b = arr(n + 11);
    YieldOnce(false).await;
    sum(&b)
}
fn by_ref<T>(t: &T) { std::hint::black_box(t); }
// polls a pinned future once (it suspends on its own await)
async fn poll_once<F: Future>(f: Pin<&mut F>) {
    let mut f = Some(f);
    std::future::poll_fn(|cx| { let _ = f.take().map(|f| f.poll(cx)); Poll::Ready(()) }).await
}

struct H<'a, T> { r: &'a T }
async fn local_in_struct(n: u64) -> u64 {
    let t = (leaf(n), arr(n));
    let h = H { r: &t };
    YieldOnce(false).await;
    sum(&h.r.1)
}
async fn extended_temporary(n: u64) -> u64 {
    let h = H { r: &(leaf(n), arr(n)) };
    YieldOnce(false).await;
    sum(&h.r.1)
}
async fn pinned(n: u64) -> u64 {
    let mut p = pin!(leaf(n));
    poll_once(p.as_mut()).await;
    YieldOnce(false).await;
    p.as_mut().await
}
fn consume<F>(t: (F, [u64; 8]), _: ()) -> u64 { sum(&t.1) }
async fn block_value_then_await(n: u64) -> u64 {
    consume({ let t = (leaf(n), arr(n)); t }, YieldOnce(false).await)
}
async fn moved_out_of_block(n: u64) -> u64 {
    let x = { let f = (leaf(n), arr(n)); by_ref(&f); f };
    let r = &x;
    let rr = &r;
    YieldOnce(false).await;
    sum(&rr.1)
}
async fn for_over(n: u64) -> u64 {
    let mut acc = 0;
    for f in [leaf(n), leaf(n + 1), leaf(n + 2)] {
        by_ref(&f);
        YieldOnce(false).await;
        acc ^= f.await;
        if acc % 4 == 3 { break; }
    }
    acc
}
async fn match_by_value(n: u64) -> u64 {
    match (leaf(n), arr(n)) {
        (f, a) => { YieldOnce(false).await; f.await ^ sum(&a) }
    }
}
async fn match_mut_ref(n: u64) -> u64 {
    match &mut (leaf(n), arr(n)) {
        t => {
            poll_once(unsafe { Pin::new_unchecked(&mut t.0) }).await;
            YieldOnce(false).await;
            let v = unsafe { Pin::new_unchecked(&mut t.0) }.await;
            v ^ sum(&t.1)
        }
    }
}
async fn let_else(n: u64) -> u64 {
    let Some((f, a)) = Some((leaf(n), arr(n))) else { return 0 };
    YieldOnce(false).await;
    f.await ^ sum(&a)
}
async fn if_let_mut_ref(n: u64) -> u64 {
    if let Some(t) = &mut Some((leaf(n), arr(n))) {
        poll_once(unsafe { Pin::new_unchecked(&mut t.0) }).await;
        YieldOnce(false).await;
        let v = unsafe { Pin::new_unchecked(&mut t.0) }.await;
        v ^ sum(&t.1)
    } else { 0 }
}
async fn poll_fn_local(n: u64) -> u64 {
    let mut f = leaf(n);
    let a = arr(n);
    let v = std::future::poll_fn(|cx| unsafe { Pin::new_unchecked(&mut f) }.poll(cx)).await;
    v ^ sum(&a)
}
async fn read_after<F>(t: &(F, [u64; 8]), y: YieldOnce) -> u64 { y.await; sum(&t.1) }
async fn tail_temporary(n: u64) -> u64 {
    let v = read_after({ &(leaf(n), arr(n)) }, YieldOnce(false)).await;
    v
}

fn work() {
    d!("a local borrowed into a struct: {}", block_on_scribbled(local_in_struct(3)));
    d!("a temporary a let extends: {}", block_on_scribbled(extended_temporary(4)));
    d!("pin!: {}", block_on_scribbled(pinned(5)));
    d!("a block's value, then an await: {}", block_on_scribbled(block_value_then_await(6)));
    d!("a block's local moved out: {}", block_on_scribbled(moved_out_of_block(7)));
    d!("for over futures: {}", block_on_scribbled(for_over(8)));
    d!("match by value: {}", block_on_scribbled(match_by_value(9)));
    d!("match through &mut: {}", block_on_scribbled(match_mut_ref(10)));
    d!("let..else: {}", block_on_scribbled(let_else(11)));
    d!("if let through &mut: {}", block_on_scribbled(if_let_mut_ref(12)));
    d!("poll_fn on a local: {}", block_on_scribbled(poll_fn_local(13)));
    d!("a block tail's temporary: {}", block_on_scribbled(tail_temporary(14)));
}
