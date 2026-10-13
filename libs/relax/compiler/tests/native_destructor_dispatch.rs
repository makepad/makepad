// Native codegen selection must receive the destructor impl's lifetime arguments.
use std::cell::Cell;
struct Guard<'a>(&'a Cell<u32>);
impl Drop for Guard<'_> {
    fn drop(&mut self) { self.0.set(self.0.get() + 1); }
}
#[test]
fn destructor_lifetime_arguments() {
    let count = Cell::new(0);
    { let _guard = Guard(&count); }
    assert_eq!(count.get(), 1);
}
