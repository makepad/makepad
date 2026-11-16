// diff_fast corpus program: what an async cycle builds while its frames are being found keeps no
// partial layout (the future's state without its saved locals). A vtable made there for a boxed dyn
// future of the cycle has the future's full size in its size word; a constant `size_of` of one is
// its full size; boxed frames dropped before they complete free what they allocated; an async fn
// awaited inside the cycle keeps its own frame; a future of the cycle bound to a local, or
// destructured from a tuple, and moved into its box is not held across the await, nor one borrowed
// in a block that ends before the await (also in a loop's body, and left early through a labelled
// block's break and a `?`) or borrowed as a statement's temporary. Each cycle holds a 4 KB array
// across an await.
// rustc --test -O and `makepad test` (all tiers) must print identical D lines.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

use std::alloc::{GlobalAlloc, Layout, System};
use std::future::Future;
use std::mem::size_of_val;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

// allocations and deallocations of WATCH_SIZE bytes
struct Watch;
static WATCH_SIZE: AtomicUsize = AtomicUsize::new(usize::MAX);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static FREES: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Watch {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if l.size() == WATCH_SIZE.load(Relaxed) { ALLOCS.fetch_add(1, Relaxed); }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if l.size() == WATCH_SIZE.load(Relaxed) { FREES.fetch_add(1, Relaxed); }
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static ALLOC: Watch = Watch;
fn watching(size: usize, f: impl FnOnce()) -> (usize, usize) {
    ALLOCS.store(0, Relaxed);
    FREES.store(0, Relaxed);
    WATCH_SIZE.store(size, Relaxed);
    f();
    WATCH_SIZE.store(usize::MAX, Relaxed);
    (ALLOCS.load(Relaxed), FREES.load(Relaxed))
}

struct Noop;
impl Wake for Noop { fn wake(self: Arc<Self>) {} }
struct YieldOnce(bool);
impl Future for YieldOnce {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 { Poll::Ready(()) } else { self.0 = true; cx.waker().wake_by_ref(); Poll::Pending }
    }
}
fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = Box::pin(f);
    let w = Waker::from(Arc::new(Noop));
    let mut cx = Context::from_waker(&w);
    loop { if let Poll::Ready(v) = f.as_mut().poll(&mut cx) { return v; } }
}
// polls `f` `n` times (each a Pending: the frames below it are boxed by then) and drops it
fn poll_then_drop<F: Future>(f: F, n: u32) {
    let mut f = Box::pin(f);
    let w = Waker::from(Arc::new(Noop));
    let mut cx = Context::from_waker(&w);
    for _ in 0..n { assert!(f.as_mut().poll(&mut cx).is_pending()); }
}
fn fill(n: u32) -> [u64; 512] {
    let mut buf = [0u64; 512];
    for (i, b) in buf.iter_mut().enumerate() { *b = (i as u64).wrapping_mul(0x9e37_79b9) ^ n as u64; }
    buf
}
fn fold(seed: u64, buf: &[u64; 512]) -> u64 { buf.iter().fold(seed, |h, &x| h.wrapping_mul(31).wrapping_add(x)) }

// the size word of the vtables made inside the cycles
static VT_SELF: AtomicUsize = AtomicUsize::new(0);
static VT_INNER: AtomicUsize = AtomicUsize::new(0);
static CONST_INNER: AtomicUsize = AtomicUsize::new(0);

// boxes itself as a dyn future
async fn vt_self(n: u32) -> u64 {
    let buf = fill(n);
    YieldOnce(false).await;
    let deeper = if n > 0 {
        let f: Pin<Box<dyn Future<Output = u64>>> = Box::pin(vt_self(n - 1));
        VT_SELF.store(size_of_val(&*f), Relaxed);
        f.await
    } else { 1 };
    fold(deeper, &buf)
}

// a pair first used from the outer end: the outer boxes the inner as a dyn future, the inner holds
// the outer by value
async fn vt_outer(n: u32) -> u64 {
    let buf = fill(n + 1);
    YieldOnce(false).await;
    let deeper = if n > 0 {
        let f = Box::pin(vt_inner(n - 1)) as Pin<Box<dyn Future<Output = u64>>>;
        VT_INNER.store(size_of_val(&*f), Relaxed);
        f.await
    } else { 2 };
    fold(deeper, &buf)
}
async fn vt_inner(n: u32) -> u64 { vt_outer(n).await ^ 0x11 }

// a constant of the inner future's size, read inside the outer one (from the fn's type: no future
// is made for it); and an async fn of its own awaited before the box
fn const_size<A, F: Future>(_: impl Fn(A) -> F) -> usize { const { std::mem::size_of::<F>() } }
async fn helper(n: u32) -> u64 {
    let b = [n as u64 + 1; 8];
    YieldOnce(false).await;
    b.iter().sum()
}
async fn k_outer(n: u32) -> u64 {
    let buf = fill(n + 2);
    let h = helper(n).await;
    YieldOnce(false).await;
    let deeper = if n > 0 {
        CONST_INNER.store(const_size(k_inner), Relaxed);
        Box::pin(k_inner(n - 1)).await
    } else { 3 };
    fold(deeper ^ h, &buf)
}
async fn k_inner(n: u32) -> u64 { k_outer(n).await.wrapping_add(5) }

// the boxed future first bound to a local moved into the box, and first destructured from a tuple
// a call returns: neither is held across the await, so the outer future is the smaller
async fn b_outer(n: u32) -> u64 {
    let buf = fill(n + 3);
    YieldOnce(false).await;
    let deeper = if n > 0 {
        let f = b_inner(n - 1);
        Box::pin(f).await
    } else { 4 };
    fold(deeper, &buf)
}
async fn b_inner(n: u32) -> u64 { b_outer(n).await ^ 0x22 }
static PASSED: AtomicUsize = AtomicUsize::new(0);
fn pass<F: Future>(f: F) -> (usize, F) { (std::mem::size_of::<F>(), f) }
async fn c_outer(n: u32) -> u64 {
    let buf = fill(n + 4);
    YieldOnce(false).await;
    let deeper = if n > 0 {
        let (size, f) = pass(c_inner(n - 1));
        PASSED.store(size, Relaxed);
        Box::pin(f).await
    } else { 5 };
    fold(deeper, &buf)
}
async fn c_inner(n: u32) -> u64 { c_outer(n).await.wrapping_mul(3) }

// borrowed in a block that ends before the await: its storage ends with the block
fn by_ref<T>(t: &T) { std::hint::black_box(t); }
async fn d_outer(n: u32) -> u64 {
    let buf = fill(n + 5);
    YieldOnce(false).await;
    let deeper = if n > 0 {
        let b = { let f = d_inner(n - 1); by_ref(&f); Box::pin(f) };
        b.await
    } else { 6 };
    fold(deeper, &buf)
}
async fn d_inner(n: u32) -> u64 { d_outer(n).await ^ 0x33 }
// a statement's borrowed temporary: its storage ends with the statement
async fn e_outer(n: u32) -> u64 {
    let buf = fill(n + 6);
    YieldOnce(false).await;
    let deeper = if n > 0 {
        by_ref(&e_inner(n - 1));
        Box::pin(e_inner(n - 1)).await
    } else { 7 };
    fold(deeper, &buf)
}
async fn e_inner(n: u32) -> u64 { e_outer(n).await.wrapping_add(9) }
// the block in a loop's body, run twice
async fn l_outer(n: u32) -> u64 {
    let buf = fill(n + 7);
    YieldOnce(false).await;
    let mut deeper = 8;
    if n > 0 {
        for r in 0..2u64 {
            let b = { let f = l_inner(n - 1); by_ref(&f); Box::pin(f) };
            deeper ^= b.await.rotate_left(r as u32 + 1);
        }
    }
    fold(deeper, &buf)
}
async fn l_inner(n: u32) -> u64 { l_outer(n).await.rotate_left(3) }
// left through a labelled block's break on odd levels
async fn x_outer(n: u32) -> u64 {
    let buf = fill(n + 8);
    YieldOnce(false).await;
    let deeper = if n > 0 {
        let b = 'a: {
            let f = x_inner(n - 1);
            by_ref(&f);
            if n % 2 == 1 { break 'a Box::pin(f); }
            Box::pin(f)
        };
        b.await
    } else { 9 };
    fold(deeper, &buf)
}
async fn x_inner(n: u32) -> u64 { x_outer(n).await ^ 0x44 }
// left through a `?` at level 2 (the future made in the block is dropped unpolled)
fn limit(n: u32) -> Result<(), u32> { if n == 2 { Err(n) } else { Ok(()) } }
async fn q_outer(n: u32) -> Result<u64, u32> {
    let buf = fill(n + 9);
    YieldOnce(false).await;
    let deeper = if n > 0 {
        let b = { let f = q_inner(n - 1); by_ref(&f); limit(n)?; Box::pin(f) };
        b.await?
    } else { 10 };
    Ok(fold(deeper, &buf))
}
async fn q_inner(n: u32) -> Result<u64, u32> { Ok(q_outer(n).await? ^ 0x55) }

fn work() {
    let v = block_on(vt_self(3));
    d!("vtable, itself: size word is the future's {} -> {}", VT_SELF.load(Relaxed) == size_of_val(&vt_self(0)), v);
    let v = block_on(vt_outer(3));
    d!("vtable, pair: size word is the inner future's {} -> {}", VT_INNER.load(Relaxed) == size_of_val(&vt_inner(0)), v);

    let v = block_on(k_outer(3));
    d!("constant: the inner future's size {} -> {}", CONST_INNER.load(Relaxed) == size_of_val(&k_inner(0)), v);
    d!("helper: holds its array {} -> {}", size_of_val(&helper(0)) >= 64, block_on(helper(4)));
    let v = block_on(b_outer(3));
    d!("bound, then boxed: the outer future is the smaller {} -> {}", size_of_val(&b_outer(0)) < size_of_val(&b_inner(0)), v);
    let v = block_on(c_outer(3));
    d!("destructured, then boxed: the outer future is the smaller {}, the size passed is the inner future's {} -> {}",
        size_of_val(&c_outer(0)) < size_of_val(&c_inner(0)), PASSED.load(Relaxed) == size_of_val(&c_inner(0)), v);

    let v = block_on(d_outer(3));
    d!("borrowed in a block: the outer future is the smaller {} -> {}", size_of_val(&d_outer(0)) < size_of_val(&d_inner(0)), v);
    let v = block_on(e_outer(3));
    d!("a borrowed temporary: the outer future is the smaller {} -> {}", size_of_val(&e_outer(0)) < size_of_val(&e_inner(0)), v);
    let v = block_on(l_outer(3));
    d!("borrowed in a loop's block: the outer future is the smaller {} -> {}", size_of_val(&l_outer(0)) < size_of_val(&l_inner(0)), v);
    let v = block_on(x_outer(3));
    d!("left through a break: the outer future is the smaller {} -> {}", size_of_val(&x_outer(0)) < size_of_val(&x_inner(0)), v);
    let (v, w) = (block_on(q_outer(1)), block_on(q_outer(3)));
    d!("left through a `?`: the outer future is the smaller {} -> {:?} {:?}", size_of_val(&q_outer(0)) < size_of_val(&q_inner(0)), v, w);

    // polled three times (each Pending: inner boxes exist by then), then dropped
    let (a, f) = watching(size_of_val(&k_inner(0)), || poll_then_drop(k_outer(3), 3));
    d!("dropped early: inner boxes allocated {} freed {}", a, f);
    let (a, f) = watching(size_of_val(&vt_inner(0)), || poll_then_drop(vt_outer(3), 3));
    d!("dropped early, dyn: inner boxes allocated {} freed {}", a, f);
}
