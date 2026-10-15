// G5 / 3.7: as closure_shift, but the edit is in ANOTHER fn above make_pair (make_pair is not rebuilt):
// its uncompiled closure `two` (made before the edit) must still be lowered from the version that
// made it when it is first called after the edit (expression ids below the edit shift).


use std::cell::{Cell, RefCell};

thread_local! {
    static PAIR: RefCell<Option<(Box<dyn Fn(u32) -> u32>, Box<dyn Fn(u32) -> u32>)>> = RefCell::new(None);
    static RUNS: Cell<u32> = Cell::new(0);
}

fn helper(x: u32) -> u32 {
    x + 1
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
