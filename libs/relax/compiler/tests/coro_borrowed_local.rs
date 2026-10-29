use std::cell::Cell;
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

#[inline(never)]
fn poll_same_frame<F: Future>(f: Pin<&mut F>) -> Poll<F::Output> {
    f.poll(&mut Context::from_waker(Waker::noop()))
}

async fn copied_field_borrow(seed: u64, address: &Cell<usize>) -> u64 {
    let mut local = (seed, [None; 2]);
    local.1 = [Some(&local.0); 2];
    // Copy the field holding references, leaving the borrowed sibling in place.
    // rustc_mir_dataflow/src/impls/borrowed_locals.rs::MaybeBorrowedLocals::gen_statement
    // retains the borrow of `local` until StorageDead.
    let refs = local.1;
    let mut yielded = false;
    poll_fn(move |cx| {
        address.set(refs[0].unwrap() as *const u64 as usize);
        if yielded { return Poll::Ready(*refs[1].unwrap()); }
        yielded = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    }).await
}

#[test]
fn copied_field_keeps_its_borrowed_sibling_across_await() {
    let a = Cell::new(0);
    let b = Cell::new(0);
    let mut first = Box::pin(copied_field_borrow(17, &a));
    let mut second = Box::pin(copied_field_borrow(29, &b));
    assert!(poll_same_frame(first.as_mut()).is_pending());
    assert!(poll_same_frame(second.as_mut()).is_pending());
    // Both referents are still live. Reusing the same poll stack slot for them is
    // observable before dereferencing a stale pointer or relying on stack garbage.
    assert_ne!(a.get(), b.get(), "borrowed locals shared the poll stack slot");
    assert_eq!(poll_same_frame(first.as_mut()), Poll::Ready(17));
    assert_eq!(poll_same_frame(second.as_mut()), Poll::Ready(29));
}

#[inline(always)]
fn trace_leaf() -> Poll<()> { Poll::Ready(()) }

async fn yield_borrowed_flag() {
    let mut yielded = false;
    poll_fn(|cx| {
        std::task::ready!(trace_leaf());
        if yielded { return Poll::Ready(()); }
        yielded = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    }).await
}

async fn work(n: u32) -> Result<u32, String> {
    yield_borrowed_flag().await;
    Ok(n)
}

#[test]
fn nested_borrowed_flags_have_independent_storage() {
    let mut first = Box::pin(work(3));
    let mut second = Box::pin(work(6));
    assert!(poll_same_frame(first.as_mut()).is_pending());
    assert!(poll_same_frame(second.as_mut()).is_pending());
    assert_eq!(poll_same_frame(first.as_mut()), Poll::Ready(Ok(3)));
    assert_eq!(poll_same_frame(second.as_mut()), Poll::Ready(Ok(6)));
}
