//! A trait's default method returning `impl Trait` is checked against its opaque type, as rustc does:
//! an async block's future behind `impl Future` from a method taking `&mut impl Sink`, a future that
//! captures that argument, and an iterator over borrowed data with an impl that overrides it with
//! another iterator type. Relax's borrow check failed on such default bodies with an internal error
//! (the return place had the body's type, not the declared opaque).
use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

trait Sink {
    fn put(&mut self, v: u32);
}
struct Count(u32);
impl Sink for Count {
    fn put(&mut self, v: u32) {
        self.0 += v;
    }
}

trait Machine {
    fn step(&mut self, _s: &mut impl Sink) -> impl Future<Output = Result<u32, String>> {
        async { Ok(5) }
    }
    fn feed(&mut self, s: &mut impl Sink) -> impl Future<Output = ()> {
        async move { s.put(2) }
    }
}
struct S;
impl Machine for S {}

fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

trait Bag {
    fn items(&self) -> &[u32];
    fn evens(&self) -> impl Iterator<Item = &u32> {
        self.items().iter().filter(|x| **x % 2 == 0)
    }
}
struct Plain(Vec<u32>);
impl Bag for Plain {
    fn items(&self) -> &[u32] {
        &self.0
    }
}
struct Reversed(Vec<u32>);
impl Bag for Reversed {
    fn items(&self) -> &[u32] {
        &self.0
    }
    fn evens(&self) -> impl Iterator<Item = &u32> {
        self.0.iter().rev()
    }
}

#[test]
fn async_defaults_run() {
    let mut s = S;
    let mut c = Count(0);
    assert_eq!(block_on(s.step(&mut c)), Ok(5));
    block_on(s.feed(&mut c));
    c.put(1);
    assert_eq!(c.0, 3);
}

#[test]
fn iterator_default_and_override() {
    assert_eq!(Plain(vec![1, 2, 3, 4]).evens().copied().collect::<Vec<_>>(), [2, 4]);
    assert_eq!(Reversed(vec![1, 2]).evens().copied().collect::<Vec<_>>(), [2, 1]);
}
