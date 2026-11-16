// A pair of async fns boxing each other, first used from the end CYCLE_ORDER names (the outer by
// default, `inner` for the other). agree.sh runs it both ways, the second time from the code the
// first run cached: code cached while the frames were found in one order runs in a process that
// finds them in the other. The inner future holds the outer one by value, the outer holds a 4 KB
// array; the allocator counts the boxes of the inner future's full size. Each `| ` line must equal
// rustc's.
use std::alloc::{GlobalAlloc, Layout, System};
use std::future::Future;
use std::mem::size_of_val;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

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
fn fill(n: u32) -> [u64; 512] {
    let mut buf = [0u64; 512];
    for (i, b) in buf.iter_mut().enumerate() { *b = (i as u64).wrapping_mul(0x9e37_79b9) ^ n as u64; }
    buf
}
fn fold(seed: u64, buf: &[u64; 512]) -> u64 { buf.iter().fold(seed, |h, &x| h.wrapping_mul(31).wrapping_add(x)) }

async fn outer(n: u32) -> u64 {
    let buf = fill(n);
    YieldOnce(false).await;
    let deeper = if n > 0 { Box::pin(inner(n - 1)).await } else { 0 };
    YieldOnce(false).await;
    fold(deeper, &buf)
}
async fn inner(n: u32) -> u64 { outer(n).await.wrapping_add(1) }

// each order in a fn of its own: code is made for the path the run takes
#[inline(never)]
fn outer_first() -> (usize, usize) {
    let o = outer(3);
    let i = inner(3);
    (size_of_val(&o), size_of_val(&i))
}
#[inline(never)]
fn inner_first() -> (usize, usize) {
    let i = inner(3);
    let o = outer(3);
    (size_of_val(&o), size_of_val(&i))
}

#[test]
fn a01_pair_both_orders() {
    let (o, i) = if std::env::var("CYCLE_ORDER").as_deref() == Ok("inner") { inner_first() } else { outer_first() };
    println!("| inner holds outer {} outer holds its array {}", i >= o, o >= 4096);
    WATCH_HITS.store(0, Relaxed);
    WATCH_SIZE.store(i, Relaxed);
    let v = block_on(outer(3));
    WATCH_SIZE.store(usize::MAX, Relaxed);
    println!("| outer {} inner {} boxes of the inner's size {}", v, block_on(inner(3)), WATCH_HITS.load(Relaxed));
}
