// A struct goes and a fn declared in another fn's body still names it in its signature: refused
// (E0412).
pub struct S(pub u32);

fn outer() -> u32 {
    fn inner(_s: Option<&S>) -> u32 {
        5
    }
    inner(None)
}

#[test]
fn run() {
    println!("out {} {}", outer(), S(1).0);
}
