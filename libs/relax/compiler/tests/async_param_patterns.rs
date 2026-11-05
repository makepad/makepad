// `async fn` parameters that are patterns (`_: D`, `(a, _b): (D, D)`, `Wrap(x): Wrap`): rustc moves
// each argument whole into the future and matches the pattern in the coroutine on its first poll
// (rustc_ast_lowering: `let <pat> = __arg;` first in the body), so nothing is destructured or
// dropped when the future is made, a future dropped before its first poll drops the whole
// arguments, and the bindings drop in the body's order.
// Run with `makepad test` and rustc --test; lines "| " are compared.
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

thread_local! { static LOG: RefCell<Vec<String>> = RefCell::new(Vec::new()); }
fn log(s: &str) {
    LOG.with(|l| l.borrow_mut().push(s.to_string()));
}
fn take() -> String {
    LOG.with(|l| std::mem::take(&mut *l.borrow_mut())).join(",")
}
struct D(&'static str);
impl Drop for D {
    fn drop(&mut self) {
        log(self.0);
    }
}
struct Wrap(D, u32);

fn noop_raw() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker {
        noop_raw()
    }
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
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            Poll::Pending
        }
    }
}

async fn split(_: D, (a, _b): (D, D), Wrap(w, n): Wrap) -> u32 {
    log("body");
    Yield(false).await;
    drop(a);
    log(w.0);
    n + 1
}

#[test]
fn patterns_match_in_the_coroutine() {
    let f = split(D("c"), (D("a"), D("b")), Wrap(D("w"), 4));
    log("made");
    let n = block_on(f);
    log("done");
    println!("| {} {}", n, take());
}

#[test]
fn unpolled_future_drops_whole_arguments() {
    let f = split(D("c2"), (D("a2"), D("b2")), Wrap(D("w2"), 1));
    log("made");
    drop(f);
    log("dropped");
    println!("| {}", take());
}

async fn by_ref(&(ref x, y): &(String, u8), mut z: Vec<u8>) -> usize {
    z.push(y);
    Yield(false).await;
    x.len() + z.len()
}

#[test]
fn reference_patterns_and_mut_bindings() {
    let pair = (String::from("abc"), 9);
    println!("| {}", block_on(by_ref(&pair, vec![1, 2])));
}
