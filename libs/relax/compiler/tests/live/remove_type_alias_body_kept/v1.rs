// A struct goes and an unchanged type alias still stands for a type naming it: refused (E0412 in
// the alias). It patched.
pub type Held = Option<S>;

#[test]
fn run() {
    println!("out {}", 3);
}
