// 4.3: enum variant edits while a live value holds a variant.
//  v1: a variant is inserted before the held one (discriminants move) and describe() gains its arm:
//      the held value is still C.
//  v2: the held variant C is removed and the enum has no #[default]: refused, the value's home is
//      named, the running code is unchanged.
use std::cell::RefCell;

#[derive(Debug)]
enum Mode {
    A,
    X(u64, u64),
    B(u32),
}

thread_local! {
    static HELD: RefCell<Option<Mode>> = RefCell::new(None);
}

fn describe(m: &Mode) -> String {
    match m {
        Mode::X(a, b) => format!("X{}", a + b),
        other => format!("{:?}", other),
    }
}

#[test]
fn run() {
    HELD.with(|h| {
        if h.borrow().is_none() {
            *h.borrow_mut() = Some(Mode::A);
        }
        println!("out held {} fresh {} {}", describe(h.borrow().as_ref().unwrap()), describe(&Mode::B(7)), describe(&Mode::X(1, 2)));
    });
}
