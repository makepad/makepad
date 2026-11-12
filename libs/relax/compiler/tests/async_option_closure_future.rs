// An async block that moves in an `Option` of a future made by a function taking a closure, takes it
// out with `if let` and awaits it (relax issue 73): the function's check stopped with an internal
// error, a type variable of another inference context indexed into an empty table.
use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

fn off(f: impl FnOnce() -> u32 + 'static) -> impl Future<Output = u32> {
    async move { f() }
}

fn highlight_pass() -> impl Future<Output = u32> {
    let job = Some(off(|| 7u32));
    async move {
        if let Some(job) = job {
            return job.await + 1;
        }
        0
    }
}

fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

#[test]
fn an_awaited_future_taken_out_of_an_option_runs() {
    assert_eq!(block_on(highlight_pass()), 8);
}
