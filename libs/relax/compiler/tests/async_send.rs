use std::rc::Rc;

fn needs_send<T: Send>(value: T) -> T { value }
async fn work(x: u32) -> u32 { x + 1 }
async fn generic<T: Send>(value: T) -> T { value }
fn hidden() -> impl Sized { 42u32 }
fn explicit() -> impl Send { 42u32 }

#[test]
fn futures_and_opaque_values_are_send() {
    drop(needs_send(work(2)));
    drop(needs_send(async { work(1).await }));
    drop(needs_send(generic(String::from("send"))));
    drop(needs_send(hidden()));
    drop(needs_send(explicit()));
}

#[test]
fn coroutine_upvars_are_published_before_retry() {
    let text = String::from("owned");
    drop(needs_send(async move { work(text.len() as u32).await }));
    let x = 1u32;
    drop(needs_send(async { work(x).await }));
}

#[test]
fn rc_scope_ends_before_await() {
    drop(needs_send(async {
        { let rc = Rc::new(42u32); assert_eq!(*rc, 42); }
        work(1).await
    }));
}
