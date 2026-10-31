use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

fn noop_raw() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker { noop_raw() }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    RawWaker::new(std::ptr::null(), &VT)
}

fn block_on<F: Future>(mut f: F) -> F::Output {
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    let mut f = unsafe { Pin::new_unchecked(&mut f) };
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


// async/await: async blocks and fns are coroutines polled to completion; awaits suspend
// (rustc's coroutine transform); dropping a suspended future drops what is live at its
// suspension point; tests print "| " lines compared with rustc.
use std::cell::RefCell;
thread_local! { static LOG: RefCell<Vec<String>> = RefCell::new(Vec::new()); }
fn log(s: &str) { LOG.with(|l| l.borrow_mut().push(s.to_string())); }
fn take() -> String { LOG.with(|l| std::mem::take(&mut *l.borrow_mut())).join(",") }
struct D(&'static str);
impl Drop for D { fn drop(&mut self) { log(self.0); } }

async fn add(a: u32, b: u32) -> u32 {
    Yield(false).await;
    a + b
}

async fn chain(n: u32) -> String {
    let mut s = String::from("x");
    for i in 0..n {
        let v = add(i, 1).await;
        s.push_str(&v.to_string());
    }
    s
}

async fn steps(n: u32) -> u32 {
    let _a = D("a");
    let mut t = 0;
    for i in 0..n {
        let _it = D("it");
        Yield(false).await;
        t += i;
    }
    let _b = D("b");
    Yield(false).await;
    t
}

trait Shape { fn area(&self) -> u32; }
struct Sq(u32);
impl Shape for Sq { fn area(&self) -> u32 { self.0 * self.0 } }
async fn dyn_area(s: Box<dyn Shape>) -> u32 { Yield(false).await; s.area() }
async fn nested() -> impl std::fmt::Debug { (add(1, 2).await, chain(2).await) }

#[test]
fn a01_values() {
    println!("| {}", block_on(async { 1 + 2 }));
    println!("| {}", block_on(add(2, 3)));
    let x = 10;
    println!("| {}", block_on(async move { add(x, 5).await * 2 }));
    println!("| {}", block_on(chain(4)));
    println!("| {}", block_on(dyn_area(Box::new(Sq(7)))));
    println!("| {:?}", block_on(nested()));
    let v = vec![1u32, 2, 3];
    let r = &v;
    println!("| {}", block_on(async { let mut s = 0; for x in r { s += add(*x, 0).await; } s }));
}

#[test]
fn a02_drops() {
    println!("| {} {}", block_on(steps(3)), take());
    {
        let waker = unsafe { Waker::from_raw(noop_raw()) };
        let mut cx = Context::from_waker(&waker);
        let mut f = Box::pin(steps(3));
        let _ = f.as_mut().poll(&mut cx);
        let _ = f.as_mut().poll(&mut cx);
        log("drop");
        drop(f);
    }
    println!("| cancel {}", take());
    let g = async move { let _x = D("x"); 1 };
    drop(g);
    let d = D("captured");
    let h = async move { let _ = &d; 2 };
    drop(h);
    println!("| unpolled {}", take());
    let d2 = D("cap2");
    println!("| {} {}", block_on(async move { let _ = &d2; log("body"); 7 }), take());
}

#[test]
fn a03_panic() {
    std::panic::set_hook(Box::new(|_| {}));
    let r = std::panic::catch_unwind(|| block_on(async { let _p = D("p"); Yield(false).await; if true { panic!("boom") } 1 }));
    println!("| panic {} {}", r.is_err(), take());
}

// the coroutine's saved locals live inline in its state (rustc's coroutine layout): references
// into the state stay valid across suspension points, a future moved before its first poll
// keeps working, futures holding borrows of the caller's or their own locals
async fn self_borrow(n: u32) -> u32 {
    let mut arr = [1u32, 2, 3, 4];
    let r = &mut arr;
    let mut s = String::from("s");
    let rs = &mut s;
    for i in 0..n {
        Yield(false).await;
        r[i as usize % 4] += i;
        rs.push('x');
    }
    Yield(false).await;
    r.iter().sum::<u32>() + rs.len() as u32
}

async fn sum_ref(v: &[u32]) -> u32 {
    let mut t = 0;
    for x in v {
        Yield(false).await;
        t += *x;
    }
    t
}

async fn borrows_local() -> u32 {
    let v = vec![5u32, 6, 7];
    let a = sum_ref(&v).await;
    let w = [1u32, 1];
    let b = sum_ref(&w[..]).await + sum_ref(&vec![10, 20]).await;
    a + b + v.len() as u32
}

fn boxed_depth(n: u32) -> Pin<Box<dyn Future<Output = u32>>> {
    Box::pin(async move {
        if n == 0 {
            return 1;
        }
        let d = D("lvl");
        Yield(false).await;
        let r = boxed_depth(n - 1).await + 1;
        drop(d);
        r
    })
}

#[test]
fn a04_inline_state() {
    println!("| {}", block_on(self_borrow(6)));
    // moved before the first poll (it holds no addresses yet)
    let f = self_borrow(3);
    let g = (f, 1u8);
    let h = Box::new(g);
    println!("| moved {}", block_on(h.0));
    println!("| {}", block_on(borrows_local()));
    println!("| {} {}", block_on(boxed_depth(4)), take());
    // dropped mid-way, at different suspension points
    for stop in 0..4 {
        let waker = unsafe { Waker::from_raw(noop_raw()) };
        let mut cx = Context::from_waker(&waker);
        let mut f = boxed_depth(3);
        for _ in 0..stop {
            let _ = f.as_mut().poll(&mut cx);
        }
        drop(f);
        println!("| stop {} {}", stop, take());
    }
    // no saved locals: the state word alone; awaited futures overlap (the variant of each point)
    println!("| sizes {} {}", std::mem::size_of_val(&async { 1 }), std::mem::size_of_val(&async { Yield(false).await; Yield(false).await }));
}

async fn many_points(n: u32) -> u64 {
    let mut acc = 0u64;
    let big = [7u64; 64];
    let mut i = 0;
    while i < n {
        let a = add(i, 1).await as u64;
        let b = { let t = big[i as usize % 64]; Yield(false).await; t };
        let c = match i % 3 { 0 => add(a as u32, 2).await as u64, 1 => { Yield(false).await; a * 2 } _ => b };
        acc += a + b + c;
        i += 1;
    }
    acc + big.iter().sum::<u64>()
}

#[test]
fn a05_many_points() {
    println!("| {}", block_on(many_points(20)));
    let f1 = many_points(5);
    let f2 = many_points(7);
    println!("| {} {}", block_on(f2), block_on(f1));
}

async fn generic_await<F: Future>(future: F) -> F::Output { future.await }
#[test]
fn rigid_future_output_in_generic_body() {
    assert_eq!(block_on(generic_await(std::future::ready(23u32))), 23);
}
