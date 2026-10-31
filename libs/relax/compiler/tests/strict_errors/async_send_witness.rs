//~ E0277
//~ help: is used across an await at
use std::rc::Rc;

fn needs_send<T: Send>(_: T) {}

async fn inner() {
    let rc = Rc::new(1u32);
    async {}.await;
    assert_eq!(*rc, 1);
}
async fn outer() { inner().await; }

fn main() {
    needs_send(async {
        let rc = Rc::new(42u32);
        async {}.await;
        assert_eq!(*rc, 42);
    });
    needs_send(outer());
}
