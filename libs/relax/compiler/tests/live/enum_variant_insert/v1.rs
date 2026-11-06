// M10: variants inserted before running ones while a live value holds one. The held value keeps
// its variant (a running variant keeps its tag by name) and the new variants order and number by
// the new text.
//  v1: `X(u16)` inserted before the held `C` (it fits the running layout); describe() and the
//      derived ordering read it.
//  v2: a unit variant inserted first; `==` and `mem::discriminant` read the held value.
//  v3: a variant inserted after `mem::discriminant` ran: the numbers it read move, so the edit
//      takes the restart rung.
use std::cell::RefCell;

#[derive(Debug, Clone, PartialEq, PartialOrd)]
enum Mode {
    A,
    X(u16),
    B(u32),
    C { level: u8 },
}

thread_local! {
    static HELD: RefCell<Option<Mode>> = RefCell::new(None);
}

fn describe(m: &Mode) -> String {
    match m {
        Mode::C { level } => format!("C{}", level),
        Mode::X(k) => format!("X{}", k),
        other => format!("{:?}", other),
    }
}

#[test]
fn run() {
    HELD.with(|h| {
        if h.borrow().is_none() {
            *h.borrow_mut() = Some(Mode::C { level: 4 });
        }
        let held = h.borrow();
        let held = held.as_ref().unwrap();
        println!("out held {} fresh {} {} ord {} {} {}", describe(held), describe(&Mode::B(7)), describe(&Mode::X(3)), Mode::A < *held, Mode::X(3) < *held, Mode::B(7) > Mode::X(3));
    });
}
