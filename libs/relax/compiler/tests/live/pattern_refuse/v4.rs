// The edit's pattern errors (E0004, E0005) refuse it, as a fresh build of its text stops at them;
// a method's match on `Self` sees its enum's new variants.
//  v1: an arm of an exhaustive match is removed: E0004, refused (the running code is unchanged).
//  v2: a `let` with a refutable pattern: E0005, refused.
//  v3: a unit variant appended (it fits the running layout) that `Mode::label`'s match on `Self`
//      misses: E0004, refused.
//  v4: its arm added: applied.
#[derive(Clone, Copy)]
enum Mode {
    A,
    B(u32),
    C,
    D,
}

impl Mode {
    fn label(self) -> String {
        match self {
            Self::A => "A".to_string(),
            Self::B(n) => format!("B{}", n),
            Self::C => "C".to_string(),
            Self::D => "D".to_string(),
        }
    }
}

fn code(m: Mode) -> u32 {
    match m {
        Mode::A => 1,
        Mode::B(n) => n,
        Mode::C => 3,
        Mode::D => 4,
    }
}

#[test]
fn run() {
    println!("out code {} {} {} {}", code(Mode::A), code(Mode::B(7)), code(Mode::C), code(Mode::D));
    println!("out label {} {} {} {}", Mode::A.label(), Mode::B(2).label(), Mode::C.label(), Mode::D.label());
}
