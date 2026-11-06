// A trait object made before the edit (held in a thread_local) runs an override added to its impl,
// the default when it goes, and the next one: its vtable's entry forwards.
use std::cell::RefCell;
pub trait Named {
    fn name(&self) -> u32 {
        1
    }
}
pub struct A(u32);
impl Named for A {
    fn name(&self) -> u32 {
        self.0 + 1
    }
}
thread_local! {
    static HELD: RefCell<Option<Box<dyn Named>>> = RefCell::new(None);
}
fn keep() {
    HELD.with(|h| {
        let mut h = h.borrow_mut();
        if h.is_none() {
            *h = Some(Box::new(A(7)));
        }
    });
}

#[test]
fn run() {
    keep();
    println!("out {}", HELD.with(|h| h.borrow().as_ref().map_or(0, |w| w.name())));
}
