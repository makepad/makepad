// A struct goes and an unchanged type alias still stands for a type naming it: refused (E0412 in
// the alias). It patched.
pub struct S(pub u32);
pub type Held = Option<S>;

#[test]
fn run() {
    let h: Held = Some(S(2));
    println!("out {}", h.map_or(0, |s| s.0));
}
