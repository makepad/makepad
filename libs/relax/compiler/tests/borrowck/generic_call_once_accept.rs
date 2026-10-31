// The call trait belongs to the generic callee type, including pattern locals,
// fields and autoderef targets, even when F is instantiated with FnMut or Fn.
fn once<F: FnOnce()>(f: F) {
    let mut o = Some(f);
    (|| if let Some(f) = o.take() { f() })();
}
struct Slot<F> { f: F }
fn field<F: FnOnce()>(s: Slot<F>) { (|| (s.f)())(); }
fn deref<F: FnMut()>(mut f: Box<F>) { f(); }
fn shared<F: Fn() + FnMut() + FnOnce()>(f: F) {
    let o = Some(f);
    if let Some(f) = o { f(); f(); }
}
fn main() {
    let mut n = 0;
    once(|| n += 1);
    once(|| assert_eq!(n, 1));
    field(Slot { f: || n += 1 });
    field(Slot { f: || assert_eq!(n, 2) });
    deref(Box::new(|| n += 1));
    shared(|| assert_eq!(n, 3));
    assert_eq!(n, 3);
    let lock = std::sync::OnceLock::new();
    assert_eq!(*lock.get_or_init(|| { n += 1; n }), 4);
    let shared_lock = std::sync::OnceLock::new();
    assert_eq!(*shared_lock.get_or_init(|| n), 4);
}
