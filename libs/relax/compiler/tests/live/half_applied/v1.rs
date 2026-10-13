// G4: two fns that both run are edited in one save; one edit is valid, the other a type error.
// Today the valid one is installed and the other prints a compile error (a mixed version).
// The save must be refused as a whole.
fn a(x: u64) -> u64 {
    x + 20
}

fn b(x: u64) -> u64 {
    x * "two"
}

#[test]
fn run() {
    println!("out {} {}", a(1), b(2));
}
