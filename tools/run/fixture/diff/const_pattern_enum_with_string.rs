// A constant pattern of an enum whose other variant holds a String. rustc builds the pattern from the
// constant's value, which holds only `Number`; the String's pointer in `Name` never enters it. Relax
// refused the type for that pointer, and the borrow check stopped on the pattern it could not build.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

#[derive(PartialEq, Eq, Debug)]
enum Key {
    Number(u64),
    Name(String),
}

mod field {
    use super::Key;
    pub const DELETED: Key = Key::Number(1);
    pub const ARCHIVED: Key = Key::Number(2);
}

// through a reference, as `match &key { &DELETED => .. }`
fn permits(key: &Key) -> &'static str {
    match key {
        &field::DELETED => "deleted",
        &field::ARCHIVED => "archived",
        Key::Number(_) => "number",
        Key::Name(_) => "name",
    }
}

// by value, with the constant's path pattern
fn kind(key: Key) -> String {
    match key {
        field::DELETED => "deleted".to_string(),
        Key::Name(n) => format!("name {n}"),
        other => format!("{other:?}"),
    }
}

#[derive(PartialEq, Eq)]
enum State {
    Idle,
    Named(String),
}

const IDLE: State = State::Idle;

fn state_name(s: &State) -> &str {
    match s {
        &IDLE => "idle",
        State::Named(n) => n.as_str(),
    }
}

const NONE_BOX: Option<Box<u64>> = None;

fn box_value(opt: Option<Box<u64>>) -> u64 {
    match opt {
        NONE_BOX => 0,
        Some(b) => *b,
    }
}

fn work() {
    let keys = [Key::Number(1), Key::Number(2), Key::Number(3), Key::Name("1".to_string())];
    for k in &keys {
        d!("{:?} {}", k, permits(std::hint::black_box(k)));
    }
    for k in keys {
        d!("{}", kind(std::hint::black_box(k)));
    }
    for s in [State::Idle, State::Named("ready".to_string())] {
        d!("{}", state_name(std::hint::black_box(&s)));
    }
    for opt in [None, Some(Box::new(42))] {
        d!("{}", box_value(std::hint::black_box(opt)));
    }
}
