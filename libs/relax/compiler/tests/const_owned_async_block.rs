// relax issue 44: a closure in a const's or a static's value that makes an async block (a
// coroutine whose owner item is the const), read through a generic `module::<M>()`. The
// well-formedness walk went into the coroutine's parent marker and asked the const for a
// function signature: an internal compiler error ("function signature") in each such body.
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

pub type Start = fn() -> Pin<Box<dyn Future<Output = u32>>>;

pub trait Module {
    const NAME: &'static str;
    const START: Start = || Box::pin(async { 7 });
}

pub static STATIC_START: Start = || Box::pin(async { 3 });
pub const CONST_START: Start = || Box::pin(async { 4 });

struct Image;
struct Text;

impl Module for Image {
    const NAME: &'static str = "image";
}

impl Module for Text {
    const NAME: &'static str = "text";
    const START: Start = || Box::pin(async { 11 });
}

fn ready(mut f: Pin<Box<dyn Future<Output = u32>>>) -> u32 {
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    fn noop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    match f.as_mut().poll(&mut Context::from_waker(&waker)) {
        Poll::Ready(v) => v,
        Poll::Pending => 0,
    }
}

fn module<M: Module>() -> (&'static str, u32) {
    (M::NAME, ready((M::START)()))
}

#[test]
fn a_const_owned_async_block_runs() {
    assert_eq!(module::<Image>(), ("image", 7), "the trait's default const");
    assert_eq!(module::<Text>(), ("text", 11), "the impl's const");
    assert_eq!(ready(STATIC_START()) + ready(CONST_START()), 7, "a static's and a const's");
}
