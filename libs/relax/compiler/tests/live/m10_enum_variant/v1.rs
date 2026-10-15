use std::sync::Mutex;

enum Mode {
    A,
    B(u32),
    C,
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
            Mode::C => println!("out C"),
        }
    }
    if let Mode::B(n) = &mut modes[1] {
        *n = 20;
    }
}
