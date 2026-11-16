// A pair of async fns boxing each other, the boxed future first destructured from a tuple a call
// returns: the tuple temporary and the binding are moved out before the await, so neither is held
// across it (rustc accepts; a frame saving either would hold itself, E0733).
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
struct Noop;
impl Wake for Noop { fn wake(self: Arc<Self>) {} }
fn block_on<F: Future>(f: F) -> F::Output {
    let mut f: Pin<Box<F>> = Box::pin(f);
    let w = Waker::from(Arc::new(Noop));
    let mut cx = Context::from_waker(&w);
    loop { if let Poll::Ready(v) = f.as_mut().poll(&mut cx) { return v; } }
}
fn pass<F: Future>(f: F) -> (u32, F) { (7, f) }
pub async fn outer(n: u32) -> u64 {
    if n > 0 {
        let (k, f) = pass(inner(n - 1));
        Box::pin(f).await + k as u64
    } else { 0 }
}
pub async fn inner(n: u32) -> u64 { outer(n).await + 1 }
#[test]
fn runs() { assert_eq!(block_on(outer(3)), 24); assert_eq!(block_on(inner(2)), 17); }
