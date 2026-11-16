// diff_fast corpus program: async recursion broken by a Box, through a second (and third) async fn
// and directly. A future that holds another by value is at least that big, and a future holding a
// 4 KB array across an await is at least 4 KB, whichever end of the cycle is first used. Two pairs
// of the same shape are first used from opposite ends (their sizes must agree), three chains of
// three from each of their members. A recursion through a plain fn returning a boxed dyn future
// needs no size at all. The values go through the boxed frames; a box sized from a partial layout
// (the future's state without its saved locals) is written past its end without a visible effect,
// so the allocator also counts the boxes of each future's full size.
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
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::pin::Pin;
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
fn block_on<F: Future>(f: F) -> (F::Output, u32) {
    let mut f = Box::pin(f);
    let w = Waker::from(Arc::new(Noop));
    let mut cx = Context::from_waker(&w);
    let mut polls = 0;
    loop { polls += 1; if let Poll::Ready(v) = f.as_mut().poll(&mut cx) { return (v, polls); } }
}
struct Watch;
static WATCH_SIZE: AtomicUsize = AtomicUsize::new(usize::MAX);
static WATCH_HITS: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Watch {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if l.size() == WATCH_SIZE.load(Relaxed) { WATCH_HITS.fetch_add(1, Relaxed); }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { unsafe { System.dealloc(p, l) } }
}
#[global_allocator]
static ALLOC: Watch = Watch;
// how many allocations of `size` bytes running `f` makes
fn boxes_of(size: usize, f: impl FnOnce() -> u64) -> usize {
    WATCH_HITS.store(0, Relaxed);
    WATCH_SIZE.store(size, Relaxed);
    std::hint::black_box(f());
    WATCH_SIZE.store(usize::MAX, Relaxed);
    WATCH_HITS.load(Relaxed)
}

fn fill(n: u32) -> [u64; 512] {
    let mut buf = [0u64; 512];
    for (i, b) in buf.iter_mut().enumerate() { *b = (i as u64).wrapping_mul(0x9e37_79b9) ^ n as u64; }
    buf
}
fn fold(seed: u64, buf: &[u64; 512]) -> u64 { buf.iter().fold(seed, |h, &x| h.wrapping_mul(31).wrapping_add(x)) }

// pair A, first used from the outer end
async fn outer_a(n: u32) -> u64 {
    let buf = fill(n + 1);
    YieldOnce(false).await;
    let deeper = if n > 0 { Box::pin(inner_a(n - 1)).await } else { 0 };
    YieldOnce(false).await;
    fold(deeper, &buf)
}
async fn inner_a(n: u32) -> u64 { outer_a(n).await.wrapping_add(3) }

// pair B, the same shape, first used from the inner end
async fn outer_b(n: u32) -> u64 {
    let buf = fill(n + 1);
    YieldOnce(false).await;
    let deeper = if n > 0 { Box::pin(inner_b(n - 1)).await } else { 0 };
    YieldOnce(false).await;
    fold(deeper, &buf)
}
async fn inner_b(n: u32) -> u64 { outer_b(n).await.wrapping_add(3) }

// three deep: c1 holds c2 holds c3, and c3 boxes c1
async fn c1(n: u32) -> u64 {
    let buf = fill(n + 2);
    YieldOnce(false).await;
    let v = c2(n).await;
    fold(v, &buf)
}
async fn c2(n: u32) -> u64 { c3(n).await ^ 0x55 }
async fn c3(n: u32) -> u64 {
    YieldOnce(false).await;
    if n > 0 { Box::pin(c1(n - 1)).await } else { 7 }
}
// the same, first used from the boxing end
async fn c1x(n: u32) -> u64 {
    let buf = fill(n + 5);
    YieldOnce(false).await;
    let v = c2x(n).await;
    fold(v, &buf)
}
async fn c2x(n: u32) -> u64 { c3x(n).await ^ 0x66 }
async fn c3x(n: u32) -> u64 {
    YieldOnce(false).await;
    if n > 0 { Box::pin(c1x(n - 1)).await } else { 9 }
}
// and from the middle
async fn c1y(n: u32) -> u64 {
    let buf = fill(n + 6);
    YieldOnce(false).await;
    let v = c2y(n).await;
    fold(v, &buf)
}
async fn c2y(n: u32) -> u64 { c3y(n).await ^ 0x77 }
async fn c3y(n: u32) -> u64 {
    YieldOnce(false).await;
    if n > 0 { Box::pin(c1y(n - 1)).await } else { 11 }
}

// boxing itself
async fn selfrec(n: u32) -> u64 {
    let buf = fill(n + 4);
    YieldOnce(false).await;
    let deeper = if n > 0 { Box::pin(selfrec(n - 1)).await } else { 2 };
    fold(deeper, &buf)
}

// through a plain fn: the frame holds a Pin<Box<dyn Future>>
fn dyn_step(n: u32) -> Pin<Box<dyn Future<Output = u64>>> { Box::pin(dyn_outer(n)) }
async fn dyn_outer(n: u32) -> u64 {
    let buf = fill(n + 3);
    YieldOnce(false).await;
    let deeper = if n > 0 { dyn_step(n - 1).await } else { 1 };
    fold(deeper, &buf)
}

fn work() {
    let (o, i) = (outer_a(3), inner_a(3));
    d!("pair A: inner holds outer {} outer holds its array {}", size_of_val(&i) >= size_of_val(&o), size_of_val(&o) >= 4096);
    d!("pair A: outer {:?} inner {:?}", block_on(o), block_on(i));

    let i = inner_b(3);
    let o = outer_b(3);
    d!("pair B: inner holds outer {} outer holds its array {}", size_of_val(&i) >= size_of_val(&o), size_of_val(&o) >= 4096);
    d!("pair B: inner {:?} outer {:?}", block_on(i), block_on(o));
    d!("pair A: boxes of the inner's size {}", boxes_of(size_of_val(&inner_a(0)), || block_on(outer_a(3)).0));
    d!("pair B: boxes of the inner's size {}", boxes_of(size_of_val(&inner_b(0)), || block_on(outer_b(3)).0));
    d!("pairs: outers agree {} inners agree {}", size_of_val(&outer_a(0)) == size_of_val(&outer_b(0)), size_of_val(&inner_a(0)) == size_of_val(&inner_b(0)));

    let (a, b, c) = (c1(4), c2(4), c3(4));
    d!("chain: c1 holds c2 {} c2 holds c3 {} c1 holds its array {}", size_of_val(&a) >= size_of_val(&b), size_of_val(&b) >= size_of_val(&c), size_of_val(&a) >= 4096);
    d!("chain: c3 {:?} c2 {:?} c1 {:?}", block_on(c), block_on(b), block_on(a));

    let (c, b, a) = (c3x(4), c2x(4), c1x(4));
    d!("chain x: c1 holds c2 {} c2 holds c3 {} c1 holds its array {}", size_of_val(&a) >= size_of_val(&b), size_of_val(&b) >= size_of_val(&c), size_of_val(&a) >= 4096);
    d!("chain x: c3 {:?} c2 {:?} c1 {:?}", block_on(c), block_on(b), block_on(a));

    let (b, a, c) = (c2y(4), c1y(4), c3y(4));
    d!("chain y: c1 holds c2 {} c2 holds c3 {} c1 holds its array {}", size_of_val(&a) >= size_of_val(&b), size_of_val(&b) >= size_of_val(&c), size_of_val(&a) >= 4096);
    d!("chain y: c3 {:?} c2 {:?} c1 {:?}", block_on(c), block_on(b), block_on(a));
    d!("chains: boxes of c1's size {} {} {}", boxes_of(size_of_val(&c1(0)), || block_on(c3(4)).0), boxes_of(size_of_val(&c1x(0)), || block_on(c3x(4)).0), boxes_of(size_of_val(&c1y(0)), || block_on(c3y(4)).0));
    d!("chains agree {}", size_of_val(&c1(0)) == size_of_val(&c1x(0)) && size_of_val(&c1x(0)) == size_of_val(&c1y(0)) && size_of_val(&c2(0)) == size_of_val(&c2x(0)) && size_of_val(&c2x(0)) == size_of_val(&c2y(0)));

    let f = selfrec(3);
    d!("self: holds its array {} -> {:?}", size_of_val(&f) >= 4096, block_on(f));
    d!("self: boxes of its size {}", boxes_of(size_of_val(&selfrec(0)), || block_on(selfrec(3)).0));

    let f = dyn_outer(5);
    d!("dyn: holds its array {} -> {:?}", size_of_val(&f) >= 4096, block_on(f));
}
