use std::rc::Rc;

fn needs_send<T: Send>(_: T) {}

// Compile-reject fixture once M2b supplies coroutine interiors. Ignoring a test
// does not suppress rustc type checking: rustc rejects the Send bound here.
#[test]
#[ignore = "coroutine witness: arch-scope"]
fn rc_held_across_await_is_not_send() {
    needs_send(async {
        let rc = Rc::new(42u32);
        async {}.await;
        assert_eq!(*rc, 42);
    });
}
