use std::future::{poll_fn, ready, Future};
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

fn noop_raw() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker { noop_raw() }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    RawWaker::new(std::ptr::null(), &VT)
}

fn block_on<F: Future>(mut future: F) -> F::Output {
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    let mut future = unsafe { Pin::new_unchecked(&mut future) };
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) { return value; }
    }
}

#[derive(Default)]
struct Rotator<const COUNT: u32> { next: u32 }

impl<const COUNT: u32> Rotator<COUNT> {
    fn num_skip(&mut self) -> u32 {
        let skip = self.next;
        self.next += 1;
        if self.next == COUNT { self.next = 0; }
        skip
    }
}

trait RotatorSelect { type Rotator<const COUNT: u32>: Default; }
struct SelectNormal;
impl RotatorSelect for SelectNormal { type Rotator<const COUNT: u32> = Rotator<COUNT>; }

// Tokio join!'s normalization and polling loop, with boxed futures and output slots.
macro_rules! join {
    (@ { rotator_select=$rotator_select:ty;
        ($($count:tt)*) ($($total:tt)*) $( ($($skip:tt)*) $e:expr, )*
    }) => {{
        let mut futures = ($( (Box::pin($e), None), )*);
        let futures = &mut futures;
        let mut rotator = <$rotator_select as RotatorSelect>::Rotator::<{$($total)*}>::default();
        poll_fn(move |cx| {
            const COUNT: u32 = $($total)*;
            let mut is_pending = false;
            let mut to_run = COUNT;
            let mut skip = rotator.num_skip();
            loop {
                $(
                    if skip == 0 {
                        if to_run == 0 { break; }
                        to_run -= 1;
                        let ($($skip,)* (future, output), ..) = &mut *futures;
                        if output.is_none() {
                            match future.as_mut().poll(cx) {
                                Poll::Ready(value) => *output = Some(value),
                                Poll::Pending => is_pending = true,
                            }
                        }
                    } else {
                        skip -= 1;
                    }
                )*
            }
            if is_pending {
                Poll::Pending
            } else {
                Poll::Ready(($({
                    let ($($skip,)* (_, output), ..) = &mut *futures;
                    output.take().unwrap()
                },)*))
            }
        }).await
    }};
    (@ { rotator_select=$rotator_select:ty; ($($s:tt)*) ($($n:tt)*) $($t:tt)* } $e:expr, $($r:tt)*) => {
        join!(@ { rotator_select=$rotator_select; ($($s)* _) ($($n)* + 1) $($t)* ($($s)*) $e, } $($r)*)
    };
    ($($e:expr),+ $(,)?) => {
        join!(@ { rotator_select=SelectNormal; () (0) } $($e,)*)
    };
}

#[test]
fn macro_closure_block_const_under_await() {
    let result = block_on(async { join!(ready(7u32), async { 11u64 }, ready(13u8)) });
    assert_eq!(result, (7, 11, 13));
}

// rustc_resolve/src/late.rs resolve_block enters the item module before any statement.
#[test]
fn closure_items_before_and_after_declaration() {
    let closure = || {
        let before: Local = Local(add(COUNT));
        const COUNT: u32 = 3;
        fn add(value: u32) -> u32 { value + COUNT }
        struct Local(u32);
        let after: Local = Local(add(COUNT));
        (before.0, after.0)
    };
    assert_eq!(closure(), (6, 6));
}

#[test]
fn awaited_closure_items_before_and_after_declaration() {
    let result = block_on(async {
        poll_fn(|_| {
            let before: Local = Local(add(COUNT));
            const COUNT: u32 = 5;
            fn add(value: u32) -> u32 { value + COUNT }
            struct Local(u32);
            let after: Local = Local(add(COUNT));
            Poll::Ready((before.0, after.0))
        }).await
    });
    assert_eq!(result, (10, 10));
}

#[test]
fn const_generic_block_items() {
    let mut rotator = Rotator::<{ const COUNT: u32 = 1 + 1 + 1; COUNT }>::default();
    assert_eq!(rotator.num_skip(), 0);
    assert_eq!(rotator.num_skip(), 1);
    assert_eq!(rotator.num_skip(), 2);
    assert_eq!(rotator.num_skip(), 0);
}
