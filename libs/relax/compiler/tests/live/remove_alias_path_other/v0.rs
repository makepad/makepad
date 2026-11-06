// A type alias `a::Id` goes while a signature names another module's `b::Id`: patched.
mod a {
    pub type Id = u32;
}
mod b {
    pub type Id = u64;
}

fn next(x: b::Id) -> u64 {
    x + 1
}

#[test]
fn run() {
    let y: a::Id = 4;
    println!("out {} {}", next(1), y);
}
