// M10: two thread-local holders share one Rc<RefCell<Item>> across edits.
// RefCell<Item> embeds Item outside the app scope, so both layout edits require
// a restart. The old shared values keep running. Rc requires thread-local storage.

use std::cell::RefCell;
use std::rc::Rc;

struct Item {
    n: u32,
}

thread_local! {
    static HOLDERS: RefCell<Option<(Rc<RefCell<Item>>, Rc<RefCell<Item>>)>> = RefCell::new(None);
}

#[test]
fn run() {
    HOLDERS.with(|state| {
        let mut state = state.borrow_mut();
        if state.is_none() {
            let item = Rc::new(RefCell::new(Item { n: 10 }));
            *state = Some((item.clone(), item));
        }
        let (a, b) = state.as_ref().unwrap();
        println!("out n {} {}", a.borrow().n, b.borrow().n);
        b.borrow_mut().n += 1;
    });
}
