// Regression: a `for` loop of `t += mid(i).await` inside an async fn (nested coroutine polls inlined into one body).
// At O3 the CSE of a join below a loop kept `Iconst 0` available through a multi-def vreg that the loop redefined.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
struct Noop; impl Wake for Noop { fn wake(self: Arc<Self>) {} }
struct YieldOnce(bool);
impl Future for YieldOnce { type Output = (); fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> { if self.0 { Poll::Ready(()) } else { self.0 = true; cx.waker().wake_by_ref(); Poll::Pending } } }
fn block_on<F: Future>(f: F) -> (F::Output, u32) {
    let mut f = Box::pin(f); let w = Waker::from(Arc::new(Noop)); let mut cx = Context::from_waker(&w); let mut polls = 0;
    loop { polls += 1; if let Poll::Ready(v) = f.as_mut().poll(&mut cx) { return (v, polls); } }
}
async fn leaf(x: u64) -> u64 { YieldOnce(false).await; x * 3 + 1 }
async fn mid(x: u64) -> u64 { let a = leaf(x).await; let b = leaf(a).await; a + b }
async fn top(n: u64) -> u64 { let mut t = 0; for i in 0..n { t += mid(i).await; } t }

fn work() { d!("{:?}", block_on(top(25))); }
