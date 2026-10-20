// A recursive boxed async fn: the future built for `Box::pin(boxed(n - 1))` is of the type being
// lowered. Its slot must have the final size (24 bytes: state, flags and saved locals), not the
// layout so far: the callee wrote 24 bytes into an 8-byte slot (coro_frame lowers the body again;
// the first lowering is not what the build takes). Found at the baseline tier with the quick
// register allocator (MAKEPAD_RUN_O1=always), where the frame order exposed the overwrite.
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
struct Noop; impl Wake for Noop { fn wake(self: Arc<Self>) {} }
fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = Box::pin(f); let w = Waker::from(Arc::new(Noop)); let mut cx = Context::from_waker(&w);
    loop { if let Poll::Ready(v) = f.as_mut().poll(&mut cx) { return v; } }
}
async fn boxed(n: u32) -> u32 { if n == 0 { 0 } else { n + Box::pin(boxed(n - 1)).await } }
#[test]
fn run() { assert_eq!(std::mem::size_of_val(&boxed(3)), 24);
    assert_eq!(block_on(boxed(2)), 3);
    assert_eq!(block_on(boxed(12)), 78); }
