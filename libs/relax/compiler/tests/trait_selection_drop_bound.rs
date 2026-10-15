// A proved destructor dispatch still has an impl predicate to fulfill.
use std::cell::Cell;

struct OnDrop<F: FnMut()>(F);
impl<F: FnMut()> Drop for OnDrop<F> {
    fn drop(&mut self) { (self.0)(); }
}

#[test]
fn destructor_with_callable_predicate() {
    let count = Cell::new(0);
    {
        let _guard = OnDrop(|| count.set(count.get() + 1));
    }
    assert_eq!(count.get(), 1);
}
