//~ E0277
//~ help: cannot be sent between threads safely
fn needs_send<T: Send>(_: T) {}
fn main() {
    let rc = std::rc::Rc::new(2u32);
    needs_send(async move { drop(rc); });
}
