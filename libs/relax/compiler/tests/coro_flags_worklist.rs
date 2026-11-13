// Values and borrows live across a suspension are saved in the coroutine frame, and a drop that
// is not live there runs once. CoroFlags::run and the backward "read after this point" pass
// (lower.rs coroutine_transform) reach their fixpoints on worklists (relax PR #96: the old scans
// did not finish on an async fn of ~180k blocks); this program locks their result through a loop,
// whose back edge the fixpoint has to carry: a borrowed array lane and a String survive a stack
// clobber between polls, and each guard drops once. `rustc --test` and `makepad test` pass it.
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

fn noop_raw() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker {
        noop_raw()
    }
    fn noop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    RawWaker::new(std::ptr::null(), &VTABLE)
}

#[inline(never)]
fn clobber_stack() {
    let junk = [0xa5u8; 16384];
    std::hint::black_box(&junk);
}

fn block_on<F: Future>(f: F) -> (F::Output, u32) {
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    let mut f = std::pin::pin!(f);
    let mut pending = 0u32;
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return (v, pending);
        }
        pending += 1;
        clobber_stack();
    }
}

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

thread_local! {
    static DROPS: RefCell<Vec<&'static str>> = RefCell::new(Vec::new());
}

struct Guard(&'static str);

impl Drop for Guard {
    fn drop(&mut self) {
        DROPS.with(|d| d.borrow_mut().push(self.0));
    }
}

fn take_drops() -> Vec<&'static str> {
    DROPS.with(|d| std::mem::take(&mut *d.borrow_mut()))
}

async fn loop_holds(n: u32) -> (u32, String, u32) {
    let data = [10u32, 20, 30, 40];
    let mut acc = 0u32;
    let mut text = String::from("v");
    let mut i = 0u32;
    while i < n {
        let lane = data[(i as usize) % 4];
        let borrowed = &data[(i as usize) % 4];
        PendingOnce(false).await;
        acc += *borrowed + lane;
        text.push(char::from(b'a' + (i as u8 % 26)));
        i += 1;
    }
    (acc, text, data[0])
}

async fn early_then_kept() {
    let early = Guard("early");
    drop(early);
    let kept = Guard("kept");
    PendingOnce(false).await;
    let _ = kept.0;
}

#[test]
fn live_values_survive_awaits_and_guards_drop_once() {
    let (value, pending) = block_on(loop_holds(8));
    assert_eq!(pending, 8);
    assert_eq!(value.0, 400);
    assert_eq!(value.1, "vabcdefgh");
    assert_eq!(value.2, 10);

    let _ = take_drops();
    let (_, pending) = block_on(early_then_kept());
    assert_eq!(pending, 1);
    let mut drops = take_drops();
    drops.sort();
    assert_eq!(drops, ["early", "kept"]);
}
