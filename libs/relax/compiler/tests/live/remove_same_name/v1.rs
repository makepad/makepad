// A struct `S` of one module goes while another module's own `S` is in a signature: patched.
mod a {
}
mod b {
    pub struct S(pub u32);
    pub fn g(s: &S) -> u32 {
        s.0 + 1
    }
}

#[test]
fn run() {
    println!("out {}", b::g(&b::S(2)));
}
