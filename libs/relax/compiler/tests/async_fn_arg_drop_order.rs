// When an `async fn`'s arguments drop: rustc moves each one into the coroutine in declaration
// order (rustc_ast_lowering lower_coroutine_body_with_moved_arguments: `let x = __argN;` for a plain
// binding, `let mut __argN = __argN; let <pat> = __argN;` for any other pattern), so they drop in
// reverse declaration order, and what a pattern leaves unbound (`_: D`, the `_` of `(x, _)`) drops
// in its parameter's place -- as a plain fn's parameters do (the Reference's "patterns in
// parameters" order). Run to completion, and dropped at a suspension point.
// Run with `makepad test` and rustc --test; lines "| " are compared (each names its test).
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
/// One case's drops, named by its test (the runners order tests differently)
fn say(test: &str, case: &str) {
    println!("| {}: {}: {}", test, case, take());
}
struct D(&'static str);
impl Drop for D {
    fn drop(&mut self) {
        log(self.0);
    }
}

fn noop_raw() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker {
        noop_raw()
    }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    RawWaker::new(std::ptr::null(), &VT)
}

fn waker() -> Waker {
    unsafe { Waker::from_raw(noop_raw()) }
}

fn block_on<F: Future>(mut f: F) -> F::Output {
    let w = waker();
    let mut cx = Context::from_waker(&w);
    let mut f = unsafe { Pin::new_unchecked(&mut f) };
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

/// Polls once (to the first suspension), then drops the future.
fn poll_once_then_drop<F: Future>(mut f: F) {
    let w = waker();
    let mut cx = Context::from_waker(&w);
    let pending = unsafe { Pin::new_unchecked(&mut f) }.poll(&mut cx).is_pending();
    log(if pending { "pending" } else { "ready" });
    drop(f);
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

async fn underscore(a: D, _: D, c: D) {
    log("body");
    Yield(false).await;
    let _ = (&a, &c);
}
fn underscore_sync(a: D, _: D, c: D) {
    log("body");
    let _ = (&a, &c);
}

async fn underscore_first(_: D, b: D) {
    log("body");
    Yield(false).await;
    let _ = &b;
}
fn underscore_first_sync(_: D, b: D) {
    log("body");
    let _ = &b;
}

async fn rest_second(a: D, (x, _): (D, D)) {
    log("body");
    Yield(false).await;
    let _ = (&a, &x);
}
fn rest_second_sync(a: D, (x, _): (D, D)) {
    log("body");
    let _ = (&a, &x);
}

// a pattern's bindings are made in its parameter's place: before a later plain parameter's
async fn pattern_first((x, _): (D, D), b: D) {
    log("body");
    Yield(false).await;
    let _ = (&x, &b);
}
fn pattern_first_sync((x, _): (D, D), b: D) {
    log("body");
    let _ = (&x, &b);
}

// the Reference's example: 3 2 0 1
async fn two_tuples((x, _): (D, D), (_, y): (D, D)) {
    log("body");
    Yield(false).await;
    let _ = (&x, &y);
}
fn two_tuples_sync((x, _): (D, D), (_, y): (D, D)) {
    log("body");
    let _ = (&x, &y);
}

// a body local drops before every argument
async fn with_local(a: D, _: D) {
    let _l = D("local");
    log("body");
    Yield(false).await;
    let _ = &a;
}

#[test]
fn unbound_argument_drops_in_its_place() {
    block_on(underscore(D("a"), D("_"), D("c")));
    say("unbound_argument_drops_in_its_place", "async run");
    poll_once_then_drop(underscore(D("a"), D("_"), D("c")));
    say("unbound_argument_drops_in_its_place", "async dropped at the await");
    underscore_sync(D("a"), D("_"), D("c"));
    say("unbound_argument_drops_in_its_place", "sync");
}

#[test]
fn unbound_first_argument() {
    block_on(underscore_first(D("_"), D("b")));
    say("unbound_first_argument", "async run");
    poll_once_then_drop(underscore_first(D("_"), D("b")));
    say("unbound_first_argument", "async dropped at the await");
    underscore_first_sync(D("_"), D("b"));
    say("unbound_first_argument", "sync");
}

#[test]
fn pattern_rest_drops_after_its_bindings() {
    block_on(rest_second(D("a"), (D("x"), D("x_rest"))));
    say("pattern_rest_drops_after_its_bindings", "async run");
    poll_once_then_drop(rest_second(D("a"), (D("x"), D("x_rest"))));
    say("pattern_rest_drops_after_its_bindings", "async dropped at the await");
    rest_second_sync(D("a"), (D("x"), D("x_rest")));
    say("pattern_rest_drops_after_its_bindings", "sync");
}

#[test]
fn pattern_before_plain_argument() {
    block_on(pattern_first((D("x"), D("x_rest")), D("b")));
    say("pattern_before_plain_argument", "async run");
    poll_once_then_drop(pattern_first((D("x"), D("x_rest")), D("b")));
    say("pattern_before_plain_argument", "async dropped at the await");
    pattern_first_sync((D("x"), D("x_rest")), D("b"));
    say("pattern_before_plain_argument", "sync");
}

#[test]
fn reference_example_order() {
    block_on(two_tuples((D("0"), D("1")), (D("2"), D("3"))));
    say("reference_example_order", "async run");
    poll_once_then_drop(two_tuples((D("0"), D("1")), (D("2"), D("3"))));
    say("reference_example_order", "async dropped at the await");
    two_tuples_sync((D("0"), D("1")), (D("2"), D("3")));
    say("reference_example_order", "sync");
}

#[test]
fn body_local_drops_first() {
    block_on(with_local(D("a"), D("_")));
    say("body_local_drops_first", "async run");
    poll_once_then_drop(with_local(D("a"), D("_")));
    say("body_local_drops_first", "async dropped at the await");
}
