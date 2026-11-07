// The edit's errors refuse it even where it also needs a restart (the restart would stop at them).
//  v1: a variant whose payload grows the held enum (a restart), which `Mode::label`'s match on
//      `Self` misses: E0004, refused.
//  v2: its arm added: the layout change alone, a restart.
//  v3: a field added to the struct a thread-local holds (a restart) and a type error in a body:
//      refused.
//  v4: the type error fixed: the field alone, a restart.
use std::cell::RefCell;

enum Mode {
    A,
    B(u32),
    C { level: u8 },
}

impl Mode {
    fn label(&self) -> String {
        match self {
            Self::A => "A".to_string(),
            Self::B(n) => format!("B{}", n),
            Self::C { level } => format!("C{}", level),
        }
    }
}

// (Copy: it takes its new layout, which a value a thread-local holds cannot)
#[derive(Clone, Copy)]
struct State {
    runs: u32,
}

thread_local! {
    static HELD: RefCell<Option<Mode>> = RefCell::new(None);
    static STATE: RefCell<Option<State>> = RefCell::new(None);
}

fn runs() -> u32 {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.is_none() {
            *s = Some(State { runs: 0 });
        }
        let st = s.as_mut().unwrap();
        st.runs += 1;
        st.runs
    })
}

#[test]
fn run() {
    HELD.with(|h| {
        if h.borrow().is_none() {
            *h.borrow_mut() = Some(Mode::C { level: 4 });
        }
        println!("out held {} fresh {}", h.borrow().as_ref().unwrap().label(), Mode::B(7).label());
    });
    let _ = runs();
}
