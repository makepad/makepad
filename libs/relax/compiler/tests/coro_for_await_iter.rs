// `for h in handles { t += h.await }` in an async block: the loop's iterator is the IntoIter
// call's result slot and lives across the await (rustc: locals_live_across_suspend_points). It
// must be saved in the coroutine, not left in the poll fn's stack frame: the executor runs other
// code between polls (tokio: other tasks), which overwrites that stack. Robrix/tokio: awaiting
// JoinHandles in a loop polled a null task pointer.
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

struct Handle { value: Box<u32>, polled: bool }
impl Future for Handle {
    type Output = Result<u32, ()>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<u32, ()>> {
        if self.polled { Poll::Ready(Ok(*self.value)) } else { self.polled = true; cx.waker().wake_by_ref(); Poll::Pending }
    }
}

#[inline(never)]
fn clobber_stack(seed: u64) -> u64 {
    let mut a = [0u64; 512];
    for (i, x) in a.iter_mut().enumerate() { *x = seed.wrapping_mul(i as u64 + 0x9e37_79b9); }
    std::hint::black_box(&mut a);
    a.iter().fold(0, |s, x| s ^ x)
}

fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    let mut n = 0u64;
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) { return v; }
        n = n.wrapping_add(clobber_stack(n + 1));
        std::hint::black_box(n);
    }
}

fn handles(n: u32) -> Vec<Handle> {
    (1..=n).map(|i| Handle { value: Box::new(i), polled: false }).collect()
}

#[test]
fn vec_iterator_lives_across_await() {
    let t = block_on(async {
        let mut t = 0;
        for h in handles(8) { t += h.await.unwrap(); }
        t
    });
    assert_eq!(t, 36);
}

#[test]
fn array_iterator_lives_across_await() {
    let t = block_on(async {
        let mut t = 0;
        let [a, b, c]: [Handle; 3] = handles(3).try_into().ok().unwrap();
        for h in [a, b, c] { t += h.await.unwrap(); }
        t
    });
    assert_eq!(t, 6);
}

#[test]
fn result_ignored_in_loop() {
    let n = block_on(async {
        let mut n = 0;
        for h in handles(4) { let _ = h.await; n += 1; }
        n
    });
    assert_eq!(n, 4);
}
