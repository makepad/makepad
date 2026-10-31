// std-core repro F-STD19 (core_diff.sh): `f.as_mut().poll(cx)` on Pin<&mut T> must call T's
// `poll(self: Pin<&mut Self>)` (receiver matches by value), not `impl Future for Pin<P>` via
// autoref. Relax picks the Pin impl -> recursion on a bogus pointer -> SIGSEGV.
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
struct One;
impl Future for One {
    type Output = u32;
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<u32> {
        Poll::Ready(7)
    }
}
#[test]
fn a01() {
    let mut cx = Context::from_waker(Waker::noop());
    let mut f = One;
    let mut p = Pin::new(&mut f);
    println!("| {:?}", p.as_mut().poll(&mut cx));
}
