// diff_fast corpus program: a value with drop glue in an async fn is in the frame exactly while its
// drop may still run after a suspension point. Each guard's type sums its own fields into a counter
// when dropped, so a value that was not saved (its slot reused while suspended) shows in the sum: one
// dropped at its scope's end after the resume, one moved out on one branch only (the run-time drop
// flag, both ways), one dropped with its future while suspended, one assigned again before the
// await and read after it, one moved into a call before the await. rustc --test -O and
// `makepad test` (all tiers) must print identical D lines.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

static SUM: AtomicU64 = AtomicU64::new(0);
static DROPS: AtomicU64 = AtomicU64::new(0);
struct G([u64; 6]);
impl Drop for G {
    fn drop(&mut self) {
        let h = self.0.iter().fold(17u64, |h, &x| h.wrapping_mul(31).wrapping_add(x));
        SUM.fetch_add(h, Relaxed);
        DROPS.fetch_add(1, Relaxed);
    }
}
fn g(n: u64) -> G { G([n, n * 3, n ^ 0x55, n + 7, n << 4, !n]) }
// the drops since the last call: (count, sum of the hashes)
fn drops() -> (u64, u64) { (DROPS.swap(0, Relaxed), SUM.swap(0, Relaxed)) }

struct Noop;
impl Wake for Noop { fn wake(self: Arc<Self>) {} }
struct YieldOnce(bool);
impl Future for YieldOnce {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 { Poll::Ready(()) } else { self.0 = true; cx.waker().wake_by_ref(); Poll::Pending }
    }
}
// polls `f` once (a Pending) and drops it
fn poll_once_then_drop<F: Future>(f: F) {
    let mut f = Box::pin(f);
    let w = Waker::from(Arc::new(Noop));
    let mut cx = Context::from_waker(&w);
    assert!(f.as_mut().poll(&mut cx).is_pending());
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

// held across the await, never read again: only its drop at the scope's end reads it
async fn held(n: u64) -> u64 {
    let _g = g(n);
    YieldOnce(false).await;
    n + 1
}
// moved out on the even branch only: the drop at the end runs on the odd one
async fn moved_on_one_branch(n: u64) -> u64 {
    let x = g(n);
    if n % 2 == 0 {
        drop(x);
    }
    YieldOnce(false).await;
    n * 2
}
// dropped with its future while suspended (the cancel path runs the drop)
async fn dropped_suspended(n: u64) -> u64 {
    let _g = g(n);
    YieldOnce(false).await;
    YieldOnce(false).await;
    n
}
// assigned again before the await (the old value dropped then), read after it
async fn assigned_again(n: u64) -> u64 {
    let mut x = g(n);
    x = g(n + 100);
    YieldOnce(false).await;
    x.0[2] ^ x.0[5]
}
// moved into a call before the await: nothing of it is left to drop at the end
fn consume(x: G) -> u64 { x.0[1] }
async fn moved_into_call(n: u64) -> u64 {
    let x = g(n);
    let k = consume(x);
    YieldOnce(false).await;
    k
}

fn work() {
    drops();
    let v = block_on_scribbled(held(5));
    d!("held across the await: {} drops {:?}", v, drops());
    let v = block_on_scribbled(moved_on_one_branch(4));
    d!("moved on the even branch: {} drops {:?}", v, drops());
    let v = block_on_scribbled(moved_on_one_branch(7));
    d!("kept on the odd branch: {} drops {:?}", v, drops());
    poll_once_then_drop(dropped_suspended(11));
    d!("dropped while suspended: drops {:?}", drops());
    let v = block_on_scribbled(assigned_again(3));
    d!("assigned again: {} drops {:?}", v, drops());
    let v = block_on_scribbled(moved_into_call(6));
    d!("moved into a call: {} drops {:?}", v, drops());
}
