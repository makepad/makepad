// A type alias `a::Id` goes while a signature names another module's `b::Id`: patched.
mod a {
}
mod b {
    pub type Id = u64;
}

fn next(x: b::Id) -> u64 {
    x + 1
}

#[test]
fn run() {
    println!("out {}", next(2));
}
