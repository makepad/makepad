// A trait and its impls go, with the code that called them: patched live (it took the restart rung:
// "trait `Words` removed").
pub struct A(u32);
pub struct B(u32);
fn size(a: &A, b: &B) -> u32 {
    a.0 * 4 + b.0 * 8 + 1
}

#[test]
fn run() {
    println!("out {}", size(&A(1), &B(2)));
}
