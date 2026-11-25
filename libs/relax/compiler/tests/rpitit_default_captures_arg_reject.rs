// rejects: E0716
//! A trait method's `impl Trait` output captures every lifetime in scope, its arguments' included, so
//! the future a default `step` returns borrows the temporary passed to it: keeping the future past
//! the statement is E0716, as rustc reports it. Relax ran such a program.
use std::future::Future;

trait Sink {}
struct NoSink;
impl Sink for NoSink {}

trait Machine {
    fn step(&mut self, _k: &mut impl Sink) -> impl Future<Output = ()> {
        async {}
    }
}
struct S;
impl Machine for S {}

pub fn the_temporary_is_borrowed_by_the_future() {
    let mut s = S;
    let f = s.step(&mut NoSink);
    let _x = 1;
    drop(f);
}
