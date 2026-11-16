// A pair of async fns boxing each other, the boxed future first bound to a local that is moved
// into the box: nothing borrows it, so it is not held across the await (rustc accepts; a frame
// saving it would hold itself, E0733).
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
pub async fn outer(n: u32) -> u64 {
    if n > 0 {
        let f = inner(n - 1);
        Box::pin(f).await
    } else { 0 }
}
pub async fn inner(n: u32) -> u64 { outer(n).await + 1 }
#[test]
fn runs() { assert_eq!(block_on(outer(3)), 3); assert_eq!(block_on(inner(2)), 3); }
