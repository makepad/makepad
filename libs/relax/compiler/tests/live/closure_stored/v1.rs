// G5 / 3.7: a closure stored in data (a boxed handler in a thread-local, made on the first run).
//  v1: the closure's body is edited (same captures): the STORED box runs the new body, as a fresh one.
//  v2: a capture is added: the stored box keeps working on its old (v1) code; a fresh box runs v2.
use std::cell::RefCell;

thread_local! {
    static H: RefCell<Option<Box<dyn Fn(u32) -> u32>>> = RefCell::new(None);
}

fn make() -> Box<dyn Fn(u32) -> u32> {
    let k = 3;
    Box::new(move |x| x * k + 2)
}

#[test]
fn run() {
    H.with(|h| {
        if h.borrow().is_none() {
            *h.borrow_mut() = Some(make());
        }
        let stored = (h.borrow().as_ref().unwrap())(5);
        println!("out stored {} fresh {}", stored, make()(5));
    });
}
