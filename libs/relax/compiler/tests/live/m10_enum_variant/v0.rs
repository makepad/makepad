// M10: v1 appends a unit variant fitting the enum layout; live A and B persist.
// v2 removes A while the first static value still holds A and must restart.
// B stabilises after its first mutation, keeping out=prev valid on rejection.

use std::sync::Mutex;

enum Mode {
    A,
    B(u32),
}

static MODES: Mutex<Vec<Mode>> = Mutex::new(Vec::new());

#[test]
fn run() {
    let mut modes = MODES.lock().unwrap();
    if modes.is_empty() {
        modes.push(Mode::A);
        modes.push(Mode::B(10));
    }
    for mode in modes.iter() {
        match mode {
            Mode::A => println!("out A"),
            Mode::B(n) => println!("out B {}", n),
        }
    }
    if let Mode::B(n) = &mut modes[1] {
        *n = 20;
    }
}
