// Async closures (rustc 1.85): `async |args| body` is a closure returning its body as a
// coroutine that owns the arguments and uses the closure's captures; `AsyncFn*(A) -> R`
// bounds take them (and closures returning futures). Lines "| " are compared with rustc.
use std::future::Future;
use std::pin::{pin, Pin};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

fn noop_raw() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker { noop_raw() }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    RawWaker::new(std::ptr::null(), &VT)
}

struct Yield(bool);
impl Future for Yield {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if self.0 { Poll::Ready(()) } else { self.0 = true; Poll::Pending }
    }
}

fn block_on<F: Future>(f: F) -> F::Output {
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    let mut f = pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

struct D(&'static str);
impl Drop for D {
    fn drop(&mut self) { println!("| drop {}", self.0); }
}

async fn apply<F, Fut>(f: F, n: u32) -> u32 where F: Fn(u32) -> Fut, Fut: Future<Output = u32> {
    f(n).await + f(n + 1).await
}
async fn apply2<F: AsyncFn(u32) -> u32>(f: F, n: u32) -> u32 { f(n).await + f(n + 1).await }
async fn apply3<F: AsyncFnMut(u32) -> u32>(mut f: F) -> u32 { f(1).await + f(2).await }
async fn apply4<F: AsyncFnOnce(u32) -> String>(f: F) -> String { f(9).await }

#[test]
fn c01_async_closures() {
    let c = async |x: u32| { Yield(false).await; x + 1 };
    println!("| {}", block_on(c(4)));
    let s = String::from("hello");
    let c2 = async |n: usize| -> usize { Yield(false).await; s.len() + n };
    println!("| {} {}", block_on(c2(1)), block_on(c2(2)));
    println!("| {}", s);
    println!("| {}", block_on(apply(async move |x| { Yield(false).await; x * 10 }, 3)));
    let d = D("moved");
    let c3 = async move || { Yield(false).await; let _ = &d; 7 };
    println!("| {}", block_on(c3()));
    let c4 = async |v: Vec<D>| { Yield(false).await; v.len() };
    println!("| {}", block_on(c4(vec![D("a"), D("b")])));
    drop(c3);
    println!("| end");
}

#[test]
fn c02_async_fn_bounds() {
    let k = 10;
    println!("| {}", block_on(apply2(async |x| { Yield(false).await; x * k }, 3)));
    let mut total = 0;
    println!("| {}", block_on(apply3(async |x| { total += x; total })));
    println!("| {}", total);
    let s = String::from("own");
    println!("| {}", block_on(apply4(async move |x| { Yield(false).await; format!("{s}{x}") })));
}

// rustc_ty_utils/src/instance.rs:320-409: fn items and fn pointers use
// the FnPtr shim for both ordinary and async call traits.
#[test]
fn c03_function_items_and_pointers_returning_futures() {
    fn ready(x: u32) -> std::future::Ready<u32> { std::future::ready(x + 1) }
    async fn consume<F: AsyncFnOnce(u32) -> u32>(f: F) -> u32 { f(8).await }
    let pointer: fn(u32) -> std::future::Ready<u32> = ready;
    assert_eq!(block_on(apply(ready, 3)), 9);
    assert_eq!(block_on(apply(pointer, 3)), 9);
    assert_eq!(block_on(apply2(ready, 3)), 9);
    assert_eq!(block_on(apply2(pointer, 3)), 9);
    assert_eq!(block_on(apply3(ready)), 5);
    assert_eq!(block_on(apply3(pointer)), 5);
    assert_eq!(block_on(consume(ready)), 9);
    assert_eq!(block_on(consume(pointer)), 9);
}
