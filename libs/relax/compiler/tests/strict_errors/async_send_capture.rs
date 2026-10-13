//~ msg: unsatisfied obligation core::marker::Send<
fn needs_send<T: Send>(_: T) {}
async fn captured(value: std::rc::Rc<u32>) { let _ = *value; }
fn main() {
    needs_send(captured(std::rc::Rc::new(1u32)));
}
