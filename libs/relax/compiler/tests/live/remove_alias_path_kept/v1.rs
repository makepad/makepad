// A type alias of a module goes and an unchanged signature names it by its path (`a::Id`): refused
// (E0412). It patched.
mod a {
}

fn next(x: a::Id) -> u32 {
    x + 1
}

#[test]
fn run() {
    println!("out {}", next(2));
}
