// An `async move` block captures precisely (edition 2021, as a closure does): it takes only the
// fields its body uses, and the rest of the variable stays the enclosing fn's, dropped when that
// fn's scope ends (rustc upvar.rs; the block's own captures drop when it completes). An axum-like
// extractor -- `Box::pin(async move { parse(&req.body) })` -- leaked the request's other parts
// under the JIT. Lines "| " are compared with rustc --test (each names its test).
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
fn say(test: &str, case: &str) {
    println!("| {}: {}: {}", test, case, take());
}
struct D(&'static str);
impl Drop for D {
    fn drop(&mut self) {
        log(self.0);
    }
}

type BoxFuture<T> = Pin<Box<dyn Future<Output = T>>>;

fn noop_raw() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker {
        noop_raw()
    }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    RawWaker::new(std::ptr::null(), &VT)
}
fn block_on<F: Future>(mut f: F) -> F::Output {
    let w = unsafe { Waker::from_raw(noop_raw()) };
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
    let w = unsafe { Waker::from_raw(noop_raw()) };
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

struct Parts {
    path: D,
    ok: bool,
}
struct Req {
    parts: Parts,
    body: D,
}
fn req(ok: bool) -> Req {
    Req { parts: Parts { path: D("parts"), ok }, body: D("body") }
}
fn check(b: &D, ok: bool) -> Result<u8, u8> {
    if ok && !b.0.is_empty() { Ok(1) } else { Err(4) }
}
fn eat(b: D) -> u8 {
    drop(b);
    2
}

// the field is borrowed in the block: a move block takes it by value, the rest stays here
fn borrowed_field(req: Req) -> BoxFuture<Result<u8, u8>> {
    Box::pin(async move {
        log("start");
        Yield(false).await;
        check(&req.body, req.parts.ok)
    })
}
// the block moves the field on
fn moved_field(req: Req) -> BoxFuture<u8> {
    Box::pin(async move { eat(req.body) })
}
// a statement moves the other field out
fn moved_by_let(req: Req) -> BoxFuture<u8> {
    Box::pin(async move {
        let parts = req.parts;
        Yield(false).await;
        parts.path.0.len() as u8
    })
}
// a Copy field inside a Drop value: copied, nothing moves (the request drops here, before the block runs)
fn nested_field(req: Req) -> BoxFuture<u8> {
    Box::pin(async move {
        Yield(false).await;
        req.parts.path.0.len() as u8
    })
}
// the whole variable: nothing stays
fn whole(req: Req) -> BoxFuture<u8> {
    Box::pin(async move {
        Yield(false).await;
        let r = req;
        r.parts.ok as u8
    })
}
// a field of a field, not Copy: the rest at both depths stays here
fn nested_moved(req: Req) -> BoxFuture<u8> {
    Box::pin(async move {
        Yield(false).await;
        let p = req.parts.path;
        p.0.len() as u8
    })
}
// a closure's precise captures, for comparison
fn closure_field(req: Req) -> impl FnOnce() -> u8 {
    move || eat(req.body)
}
fn closure_copy(req: Req) -> impl Fn() -> u8 {
    move || req.parts.path.0.len() as u8
}

#[test]
fn async_block_takes_only_used_fields() {
    log(&format!("={:?}", block_on(borrowed_field(req(true)))));
    say("async_block_takes_only_used_fields", "borrowed field");
    log(&format!("={:?}", block_on(borrowed_field(req(false)))));
    say("async_block_takes_only_used_fields", "borrowed field, error");
    poll_once_then_drop(borrowed_field(req(true)));
    say("async_block_takes_only_used_fields", "borrowed field, dropped at the await");
    log(&format!("={}", block_on(moved_field(req(true)))));
    say("async_block_takes_only_used_fields", "moved field");
    log(&format!("={}", block_on(moved_by_let(req(true)))));
    say("async_block_takes_only_used_fields", "moved by let");
    poll_once_then_drop(moved_by_let(req(true)));
    say("async_block_takes_only_used_fields", "moved by let, dropped at the await");
    log(&format!("={}", block_on(nested_field(req(true)))));
    say("async_block_takes_only_used_fields", "nested field");
    log(&format!("={}", block_on(nested_moved(req(true)))));
    say("async_block_takes_only_used_fields", "nested moved field");
    log(&format!("={}", block_on(whole(req(true)))));
    say("async_block_takes_only_used_fields", "whole");
    log(&format!("={}", closure_field(req(true))()));
    say("async_block_takes_only_used_fields", "closure");
    let c = closure_copy(req(true));
    log(&format!("={}", c() + c()));
    drop(c);
    say("async_block_takes_only_used_fields", "closure, Copy field");
}

// an extractor chain: destructure, rebuild, hand the request to a precise block
async fn handler(req: Req) -> u8 {
    let Req { parts, body } = req;
    Yield(false).await;
    let req = Req { parts, body };
    match borrowed_field(req).await {
        Ok(v) => v,
        Err(e) => e,
    }
}

#[test]
fn extractor_chain_drops_its_request() {
    log(&format!("={}", block_on(handler(req(true)))));
    say("extractor_chain_drops_its_request", "ok");
    log(&format!("={}", block_on(handler(req(false)))));
    say("extractor_chain_drops_its_request", "error");
}
