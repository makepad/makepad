// A Drop impl may restate an outlives bound its type's fields already imply: `T: 'a` through the
// `&'a Cell<T>` field, `'b: 'a` through `&'a &'b u32` (rustc's Drop check holds the type's inferred
// outlives bounds in its environment).
use std::cell::Cell;
pub struct Restore<'a, T: Copy> {
    slot: &'a Cell<T>,
    old: T,
}
impl<'a, T: Copy + 'a> Drop for Restore<'a, T> {
    fn drop(&mut self) {
        self.slot.set(self.old);
    }
}
pub struct Tally<'a, 'b> {
    amount: &'a &'b u32,
    total: &'a Cell<u32>,
}
impl<'a, 'b: 'a> Drop for Tally<'a, 'b> {
    fn drop(&mut self) {
        self.total.set(self.total.get() + **self.amount);
    }
}
#[test]
fn runs() {
    let c = Cell::new(1u8);
    {
        let _g = Restore { slot: &c, old: c.get() };
        c.set(9);
        assert_eq!(c.get(), 9);
    }
    assert_eq!(c.get(), 1);
    let v = 5u32;
    let r = &v;
    let total = Cell::new(2);
    drop(Tally { amount: &r, total: &total });
    assert_eq!(total.get(), 7);
}
