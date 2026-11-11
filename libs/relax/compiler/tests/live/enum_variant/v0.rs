// 4.3: enum variant edits while a live value holds a variant.
//  v1: a variant inserted before the held one, whose payload grows the enum, and describe() gains
//      its arm: running values cannot hold it, a restart; the held value is still C.
//  v2: the held variant C is removed and the enum has no #[default]: refused, the value's home is
//      named, the running code is unchanged.
use std::cell::RefCell;

#[derive(Debug)]
enum Mode {
    A,
    B(u32),
    C { level: u8 },
}

thread_local! {
    static HELD: RefCell<Option<Mode>> = RefCell::new(None);
}

fn describe(m: &Mode) -> String {
    match m {
        Mode::C { level } => format!("C{}", level),
        other => format!("{:?}", other),
    }
}

#[test]
fn run() {
    HELD.with(|h| {
        if h.borrow().is_none() {
            *h.borrow_mut() = Some(Mode::C { level: 4 });
        }
        println!("out held {} fresh {}", describe(h.borrow().as_ref().unwrap()), describe(&Mode::B(7)));
    });
}
