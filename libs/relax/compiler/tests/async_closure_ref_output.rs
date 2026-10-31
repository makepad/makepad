// An `AsyncFn(&str) -> &str` bound gives the async closure's body its return type with the
// closure's own late-bound region (rustc coroutine-closure `return_ty`); the closure must be
// accepted and run.
use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

fn noop_raw() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker { noop_raw() }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    RawWaker::new(std::ptr::null(), &VT)
}

fn block_on<F: Future>(f: F) -> F::Output {
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    let mut f = pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) { return v; }
    }
}

async fn call(f: impl AsyncFn(&str) -> &str) -> usize {
    f("hello").await.len()
}

#[test]
fn ref_output() {
    assert_eq!(block_on(call(async |x| x)), 5);
    assert_eq!(block_on(call(async |x: &str| { let y = x; y })), 5);
}
