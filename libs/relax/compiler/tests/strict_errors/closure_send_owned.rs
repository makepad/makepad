//~ E0277
//~ help: cannot be sent between threads safely
fn needs_send<T: Send>(_: T) {}
fn main() {
    let value = std::rc::Rc::new(42u32);
    needs_send(move || *value);
}
