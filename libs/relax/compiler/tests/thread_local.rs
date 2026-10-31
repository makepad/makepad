// thread_local! (boot B25): `static`/`const { }` initialisers, attributes and visibility, several
// declarations per call, item and statement position. rustc: all pass. (RefCell writes are left out
// here: guard temporaries are coord.md B26, frontend.)
use std::cell::{Cell, RefCell};

thread_local! {
    static A: Cell<Option<u32>> = const { Cell::new(None) };
    static R: RefCell<Vec<u32>> = RefCell::new(Vec::new());
    #[allow(dead_code)]
    pub static B: Cell<u32> = Cell::new(5);
}

mod m {
    thread_local!(pub(crate) static C: std::cell::Cell<Vec<u8>> = std::cell::Cell::new(Vec::new()));
}

#[test]
fn thread_local_items() {
    A.with(|a| {
        a.set(Some(3));
    });
    assert!(R.with(|r| r.borrow().len()) == 0);
    let v = A.with(|a| a.take().unwrap());
    B.with(|b| b.set(b.get() + v));
    assert!(B.with(|b| b.get()) == 8);
    m::C.with(|c| {
        let mut t = c.take();
        t.push(1);
        c.set(t);
    });
    assert!(m::C.with(|c| {
        let t = c.take();
        t.len()
    }) == 1);
}

#[test]
fn thread_local_in_a_block() {
    thread_local! {
        static D: Cell<u64> = const { Cell::new(10) };
    }
    D.with(|d| d.set(d.get() * 2));
    assert!(D.with(|d| d.get()) == 20);
}
