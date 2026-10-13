// `Pin<&mut dyn Future>`.poll(cx) is the trait object's poll (rustc's probe matches the
// `self: Pin<&mut Self>` receiver with Self = dyn Future), not Pin's own Future impl; boxed
// trait-object futures, async recursion through them, `pin!`, async fns in traits.
use std::future::Future;
use std::pin::{pin, Pin};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

fn noop_raw() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker { noop_raw() }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    RawWaker::new(std::ptr::null(), &VT)
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

struct Yield(bool);
impl Future for Yield {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if self.0 { Poll::Ready(()) } else { self.0 = true; Poll::Pending }
    }
}

fn depth(n: u32) -> Pin<Box<dyn Future<Output = u32>>> {
    Box::pin(async move {
        if n == 0 {
            return 1;
        }
        Yield(false).await;
        depth(n - 1).await + 1
    })
}

trait Store {
    async fn get(&self, k: u32) -> u32;
}
struct S(u32);
impl Store for S {
    async fn get(&self, k: u32) -> u32 {
        Yield(false).await;
        self.0 + k
    }
}
async fn use_store<T: Store>(t: &T) -> u32 {
    t.get(1).await + t.get(2).await
}

#[test]
fn p01_dyn_pin() {
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    let mut b: Pin<Box<dyn Future<Output = u32>>> = Box::pin(async { 7 });
    println!("| {:?}", b.as_mut().poll(&mut cx));
    let mut c: Pin<Box<dyn Future<Output = u32>>> = Box::pin(async { 8 });
    println!("| {:?}", Pin::new(&mut c).poll(&mut cx));
    println!("| {}", block_on(depth(5)));
    let v: Vec<Pin<Box<dyn Future<Output = u32>>>> = vec![depth(0), depth(2), Box::pin(async { 40 })];
    let mut t = 0;
    for f in v {
        t += block_on(f);
    }
    println!("| {}", t);
}

#[test]
fn p02_pin_macro_trait_async() {
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    let mut f = pin!(async { 5 });
    println!("| {:?}", f.as_mut().poll(&mut cx));
    println!("| {}", block_on(use_store(&S(10))));
}
