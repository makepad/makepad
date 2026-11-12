//! A reference to a constructor whose value needs no drop is promoted to a 'static even when its
//! type has drop glue (rustc qualifies promotion by the value): `&Kind::Unit`, `&Kind::Pair(3, 4)`
//! and `&Wrapper { id: 9, tag: None }` outlive the function that returns them. A stack temporary
//! would move with its caller's frame and be overwritten by the next call.
#[derive(Debug, PartialEq)]
enum Kind {
    Unit,
    Named(String),
    Pair(u8, u8),
}

struct Wrapper {
    id: u32,
    tag: Option<String>,
}

fn unit() -> &'static Kind {
    &Kind::Unit
}

fn pair() -> &'static Kind {
    &Kind::Pair(3, 4)
}

fn wrapper() -> &'static Wrapper {
    &Wrapper { id: 9, tag: None }
}

// where `unit()` puts its value, called `depth` frames further down the stack
fn unit_at(depth: u32) -> *const Kind {
    if std::hint::black_box(depth) == 0 { unit() } else { unit_at(depth - 1) }
}

fn clobber() -> u64 {
    let a = std::hint::black_box([0xdead_beef_u64; 64]);
    a.iter().fold(0, |x, y| x ^ y)
}

#[test]
fn promoted_constructors_outlive_their_function() {
    assert_eq!(unit_at(0), unit_at(16), "one static, not a temporary in each caller's frame");
    let (u, p, w) = (unit(), pair(), wrapper());
    std::hint::black_box(clobber());
    assert_eq!((u, p), (&Kind::Unit, &Kind::Pair(3, 4)), "the enum values after the stack was reused");
    assert_eq!((w.id, w.tag.is_none()), (9, true), "the struct value after the stack was reused");
    assert_ne!(*u, Kind::Named(String::from("x")));
}
