// M10: two thread-local holders share one Rc<RefCell<Item>> across edits.
// v1 observes the default then writes through a; b must see the same field.
// v2 reads it again to verify persistence. Rc requires thread-local storage.

use std::cell::RefCell;
use std::rc::Rc;

struct Item {
    n: u32,
    done: bool,
}

thread_local! {
    static HOLDERS: RefCell<Option<(Rc<RefCell<Item>>, Rc<RefCell<Item>>)>> = RefCell::new(None);
}

#[test]
fn run() {
    HOLDERS.with(|state| {
        let mut state = state.borrow_mut();
        if state.is_none() {
            let item = Rc::new(RefCell::new(Item { n: 10, done: false }));
            *state = Some((item.clone(), item));
        }
        let (a, b) = state.as_ref().unwrap();
        println!("out n {} {}", a.borrow().n, b.borrow().n);
        println!("out before {} {}", a.borrow().done, b.borrow().done);
        a.borrow_mut().done = false;
        println!("out after {} {}", a.borrow().done, b.borrow().done);
        b.borrow_mut().n += 1;
    });
}
