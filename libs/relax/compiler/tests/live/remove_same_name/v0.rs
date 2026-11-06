// A struct `S` of one module goes while another module's own `S` is in a signature: patched.
mod a {
    pub struct S(pub u32);
}
mod b {
    pub struct S(pub u32);
    pub fn g(s: &S) -> u32 {
        s.0 + 1
    }
}

#[test]
fn run() {
    println!("out {} {}", a::S(1).0, b::g(&b::S(2)));
}
