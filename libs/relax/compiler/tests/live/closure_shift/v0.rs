// G5 / 3.7: closure identity must not depend on AST expression ids. Two closures are stored on the
// first run; only the first is called until the third run (the first after the edit), so the second
// is compiled lazily after the edit. v1 inserts an unrelated statement with a closure ABOVE them
// in the same fn (shifting every expression id below it) and edits nothing else: both stored
// closures must keep their behaviour.
use std::cell::{Cell, RefCell};

thread_local! {
    static PAIR: RefCell<Option<(Box<dyn Fn(u32) -> u32>, Box<dyn Fn(u32) -> u32>)>> = RefCell::new(None);
    static RUNS: Cell<u32> = Cell::new(0);
}

fn make_pair(base: u32) -> (Box<dyn Fn(u32) -> u32>, Box<dyn Fn(u32) -> u32>) {
    let one: Box<dyn Fn(u32) -> u32> = Box::new(move |x| x + base);
    let two: Box<dyn Fn(u32) -> u32> = Box::new(move |x| x * base * 2);
    (one, two)
}

#[test]
fn run() {
    let n = RUNS.with(|r| { r.set(r.get() + 1); r.get() });
    PAIR.with(|p| {
        if p.borrow().is_none() {
            *p.borrow_mut() = Some(make_pair(10));
        }
        let b = p.borrow();
        let (one, two) = b.as_ref().unwrap();
        println!("out one {}", one(1));
        if n >= 3 {
            println!("out two {}", two(1));
        }
    });
}
