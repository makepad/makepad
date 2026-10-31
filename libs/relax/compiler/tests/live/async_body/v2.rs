// G2 / 3.8: async fn bodies. Each run completes the future the previous run left suspended at its
// await (across the edit), then runs a fresh one and leaves a new one suspended.
//  v1: body-only edit after the await (same state layout, same awaits): the suspended future
//      resumes in the NEW poll code, and fresh futures run the new body.
//  v2: a local held across the await is added (the state layout changes): a raw future the user
//      holds cannot be cancelled by the runtime, so it must finish on the OLD (v1) code; fresh
//      futures run the new body. Never the new poll on the old state.
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

fn noop_raw() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker { noop_raw() }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    RawWaker::new(std::ptr::null(), &VT)
}

fn poll_once(f: &mut Pin<Box<dyn Future<Output = u64>>>) -> Option<u64> {
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    match f.as_mut().poll(&mut cx) {
        Poll::Ready(v) => Some(v),
        Poll::Pending => None,
    }
}

struct Yield(bool);
impl Future for Yield {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if self.0 { Poll::Ready(()) } else { self.0 = true; Poll::Pending }
    }
}

async fn step(x: u64) -> u64 {
    let pad = [x; 4];
    Yield(false).await;
    pad[3] * 10 + 3
}

thread_local! {
    static PENDING: RefCell<Option<Pin<Box<dyn Future<Output = u64>>>>> = RefCell::new(None);
}

fn finish(mut f: Pin<Box<dyn Future<Output = u64>>>) -> u64 {
    loop {
        if let Some(v) = poll_once(&mut f) {
            return v;
        }
    }
}

#[test]
fn run() {
    if let Some(f) = PENDING.with(|p| p.borrow_mut().take()) {
        println!("out resumed {}", finish(f));
    }
    println!("out fresh {}", finish(Box::pin(step(4))));
    let mut f: Pin<Box<dyn Future<Output = u64>>> = Box::pin(step(7));
    assert!(poll_once(&mut f).is_none());
    PENDING.with(|p| *p.borrow_mut() = Some(f));
}
