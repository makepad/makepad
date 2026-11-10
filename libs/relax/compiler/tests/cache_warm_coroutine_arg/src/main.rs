// Instances over a coroutine's type that a warm run is the first to build. `make` holds the
// `async` block; a run after the first takes its code from the disk cache and never type-checks
// it again, so only the cached keys say that the type they name is a coroutine's. A coroutine that
// is a plain closure to the solver has no `Future` impl: an instance whose body names
// `<F as Future>::Output` stopped with "failed to normalize body for codegen: NoSolution".
//   a  the cold run: the coroutine polled through `block_on`; `deferred` built, its closure not called
//   b  `direct`: an instance over the coroutine's type itself
//   c  `boxed` (and `block_on`): over `Pin<Box<coroutine>>`
//   d  `deferred`'s closure: a closure of an instance the cold run built
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

fn raw_waker() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker {
        raw_waker()
    }
    fn nop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, nop, nop, nop);
    RawWaker::new(std::ptr::null(), &VTABLE)
}

#[inline(never)]
fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = Box::pin(f);
    let waker = unsafe { Waker::from_raw(raw_waker()) };
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

struct Yield(bool);

impl Future for Yield {
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

// The body that defines the coroutine; `held` lives across its suspension point.
#[inline(never)]
fn make(n: u64) -> impl Future<Output = u64> {
    async move {
        let held = vec![n, 1];
        Yield(false).await;
        held.iter().sum()
    }
}

#[inline(never)]
fn direct<F: Future>(f: F) -> Option<F::Output> {
    let out: F::Output = block_on(f);
    Some(out)
}

#[inline(never)]
fn boxed<F: Future + Unpin>(f: F) -> Option<F::Output> {
    let out: F::Output = block_on(f);
    Some(out)
}

#[inline(never)]
fn deferred<F: Future>(f: F, call: bool) -> Option<F::Output> {
    let run = move || -> Option<F::Output> { Some(block_on(f)) };
    if call {
        run()
    } else {
        None
    }
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let f = make(1);
    let r = match mode.as_str() {
        "b" => direct(f),
        "c" => boxed(Box::pin(f)),
        "d" => deferred(f, true),
        _ => deferred(make(1), false).or(Some(block_on(f))),
    };
    println!("cache_warm_coroutine_arg result {}", r.unwrap_or(0) * 10 + 1);
}
