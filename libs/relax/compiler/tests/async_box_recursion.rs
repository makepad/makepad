// A recursive async fn through Box::pin: the boxed future is a pointer, so the future type does
// not contain itself (rustc accepts it; E0733 is only for recursion by value). The poll function
// builds the callee future as a temporary and moves it into the box, which needs the future's
// layout before its own saved locals are known.
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

fn noop_raw() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker { noop_raw() }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    RawWaker::new(std::ptr::null(), &VT)
}

fn block_on<F: Future>(mut f: F) -> F::Output {
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    let mut f = unsafe { Pin::new_unchecked(&mut f) };
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

struct Yield(bool);
impl Future for Yield {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if self.0 { Poll::Ready(()) } else { self.0 = true; Poll::Pending }
    }
}

async fn sum(n: u32) -> u32 {
    if n == 0 {
        0
    } else {
        Yield(false).await;
        n + Box::pin(sum(n - 1)).await
    }
}

async fn tree(depth: u32) -> u32 {
    if depth == 0 {
        return 1;
    }
    let left = Box::pin(tree(depth - 1)).await;
    Yield(false).await;
    let right = Box::pin(tree(depth - 1)).await;
    left + right
}

#[test]
fn boxed_self_recursion() {
    assert_eq!(block_on(sum(10)), 55);
    assert_eq!(block_on(tree(5)), 32);
}
