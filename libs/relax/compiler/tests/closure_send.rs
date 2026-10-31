use std::rc::Rc;

fn needs_send<T: Send>(_: T) {}

#[test]
fn owned_and_borrowed_send_captures() {
    let text = String::from("owned");
    needs_send(move || text.len());
    let value = 42u32;
    needs_send(|| value);
    let mut value = 1u32;
    needs_send(|| value += 1);
    needs_send(|| 42u32);
}

trait Host: Sync { fn value(&self) -> u32; }
impl Host for u32 { fn value(&self) -> u32 { *self } }

#[test]
fn shared_capture_requires_sync() {
    let host: &dyn Host = &42u32;
    needs_send(move || host.value());
    needs_send(|| host.value());
}

#[test]
fn only_the_captured_field_needs_send() {
    let pair = (String::from("send"), Rc::new(1u32));
    needs_send(move || pair.0.len());
    let pair = (42u32, Rc::new(1u32));
    needs_send(|| pair.0);
    assert_eq!(*pair.1, 1);
}

#[test]
fn nested_closure_captures_are_resolved() {
    let text = String::from("nested");
    let inner = move || text.len();
    needs_send(move || inner());
}

#[test]
fn dyn_coercion_retries_the_closure_auto_trait_goal() {
    let text = String::from("boxed");
    let callback: Box<dyn FnOnce() -> usize + Send> = Box::new(move || text.len());
    assert_eq!(callback(), 5);
    let host: &dyn Host = &42u32;
    let callback: Box<dyn FnOnce() -> u32 + Send + '_> = Box::new(move || host.value());
    assert_eq!(callback(), 42);
}
