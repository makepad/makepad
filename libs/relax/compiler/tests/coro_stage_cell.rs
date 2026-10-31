// tokio's task cell shape (runtime/task/core.rs: Stage::Running(T) / Finished / Consumed in an
// UnsafeCell, polled through Pin::new_unchecked): an async fn first polled by a plain block_on,
// then stored in such a cell and polled to completion there, on this thread and on another one.
// Relax: the cell poll found the coroutine "resumed after completion" (Robrix / tokio spawn).
use std::cell::UnsafeCell;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

struct YieldNow(bool);
impl Future for YieldNow {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 { return Poll::Ready(()); }
        self.0 = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

async fn work(n: u32) -> Result<u32, String> {
    YieldNow(false).await;
    Ok(n)
}

fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) { return v; }
    }
}

enum Stage<T: Future> {
    Running(T),
    Finished(T::Output),
    Consumed,
}

struct Core<T: Future> {
    stage: UnsafeCell<Stage<T>>,
}
unsafe impl<T: Future + Send> Sync for Core<T> {}

impl<T: Future> Core<T> {
    fn poll(&self, cx: &mut Context<'_>) -> Poll<()> {
        let res = unsafe {
            match &mut *self.stage.get() {
                Stage::Running(fut) => Pin::new_unchecked(fut).poll(cx),
                _ => panic!("unexpected stage"),
            }
        };
        match res {
            Poll::Ready(out) => {
                unsafe { *self.stage.get() = Stage::Finished(out) };
                Poll::Ready(())
            }
            Poll::Pending => Poll::Pending,
        }
    }
    fn take_output(&self) -> T::Output {
        match unsafe { std::mem::replace(&mut *self.stage.get(), Stage::Consumed) } {
            Stage::Finished(v) => v,
            _ => panic!("not finished"),
        }
    }
}

fn run_core<T: Future>(c: &Core<T>) -> T::Output {
    let mut cx = Context::from_waker(Waker::noop());
    while c.poll(&mut cx).is_pending() {}
    c.take_output()
}

#[test]
fn block_on_then_cell_same_thread() {
    assert_eq!(block_on(work(3)), Ok(3));
    let c = Box::new(Core { stage: UnsafeCell::new(Stage::Running(work(6))) });
    assert_eq!(run_core(&c), Ok(6));
}

#[test]
fn block_on_then_cell_other_thread() {
    assert_eq!(block_on(work(2)), Ok(2));
    let c = std::sync::Arc::new(Core { stage: UnsafeCell::new(Stage::Running(work(7))) });
    let c2 = c.clone();
    let out = std::thread::spawn(move || run_core(&c2)).join().unwrap();
    assert_eq!(out, Ok(7));
    println!("| coro stage cell {:?}", out);
}

// tokio task/yield_now.rs: the flag is a local of the async fn that only the poll_fn closure
// touches, by reference; it lives across the await, so it is saved in the coroutine and the
// closure's reference points into the (pinned) coroutine. A stack slot would be garbage at the
// next poll (here the stack is overwritten between the polls).
async fn yield_once() {
    let mut yielded = false;
    std::future::poll_fn(|cx| {
        if yielded {
            return Poll::Ready(());
        }
        yielded = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    })
    .await
}

async fn work_flag(n: u32) -> Result<u32, String> {
    yield_once().await;
    Ok(n)
}

#[inline(never)]
fn clobber(depth: u32, v: u8) -> u64 {
    let buf = [v; 512];
    let s: u64 = buf.iter().map(|&b| b as u64).sum();
    if depth == 0 { s } else { s + clobber(depth - 1, v) }
}

#[test]
fn borrowed_local_across_await_survives_stack_reuse() {
    for v in [0u8, 1, 0xff] {
        let mut f = Box::pin(work_flag(9));
        let mut cx = Context::from_waker(Waker::noop());
        let mut polls = 0;
        let out = loop {
            polls += 1;
            assert!(polls < 10, "never finished");
            if let Poll::Ready(r) = f.as_mut().poll(&mut cx) { break r; }
            clobber(8, v);
        };
        assert_eq!(out, Ok(9));
        assert_eq!(polls, 2);
    }
}
