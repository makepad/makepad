// A type alias goes and an unchanged signature still names it: refused (E0412). The types the alias
// stood for keep no trace of it: the name is resolved again.
fn next(x: Id) -> u32 {
    x + 1
}

#[test]
fn run() {
    println!("out {}", next(2));
}
