// relax issue 81: a future that borrows its caller's temporary or local across a suspension.
// The caller's future holds the value and the callee's future a pointer to it. When the poll
// returns Pending and the executor polls again, that pointer has to reach into the caller's
// future (its saved state), not into the stack frame of the poll that made it: the executor
// clobbers the stack between the polls, so a pointer into a dead frame reads junk.
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

#[derive(Default)]
struct Limits {
    depth: Option<usize>,
}

struct Wide {
    depth: Option<usize>,
    count: u64,
}

/// Pending on its first poll, ready on the second.
struct PendingOnce(bool);

impl Future for PendingOnce {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            return Poll::Ready(());
        }
        self.0 = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

async fn depth_later(limits: &Limits) -> Option<usize> {
    PendingOnce(false).await;
    limits.depth
}

async fn wide_later(wide: &Wide) -> (Option<usize>, u64) {
    PendingOnce(false).await;
    (wide.depth, wide.count)
}

async fn default_temporary() -> Option<usize> {
    depth_later(&Limits::default()).await
}

async fn default_local() -> Option<usize> {
    let limits = Limits::default();
    depth_later(&limits).await
}

async fn literal_temporary(n: usize) -> Option<usize> {
    depth_later(&Limits { depth: Some(n) }).await
}

async fn wide_temporary(n: usize) -> (Option<usize>, u64) {
    wide_later(&Wide { depth: Some(n), count: n as u64 * 3 }).await
}

/// Writes over the stack below the executor's frame, where the last poll's frames were.
#[inline(never)]
fn clobber_stack() -> u64 {
    let junk = [0xa5u8; 16384];
    std::hint::black_box(&junk);
    junk.iter().map(|&b| b as u64).sum()
}

fn noop_raw() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker {
        noop_raw()
    }
    fn noop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    RawWaker::new(std::ptr::null(), &VTABLE)
}

/// Polls `f` to completion, clobbering the stack between the polls: its value and how many
/// polls returned Pending.
fn block_on<F: Future>(f: F) -> (F::Output, u32) {
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    let mut f = std::pin::pin!(f);
    let mut pending = 0;
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return (v, pending);
        }
        pending += 1;
        clobber_stack();
    }
}

#[test]
fn a_borrow_held_across_an_await_survives_a_pending_poll() {
    assert_eq!(block_on(default_temporary()), (None, 1), "a Default temporary the callee's future borrows");
    assert_eq!(block_on(default_local()), (None, 1), "a named local the callee's future borrows");
    assert_eq!(block_on(literal_temporary(5)), (Some(5), 1), "a struct literal temporary");
    assert_eq!(block_on(wide_temporary(4)), ((Some(4), 12), 1), "a wider temporary");
}
