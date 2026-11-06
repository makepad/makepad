// A trait and its impls go, with the code that called them: patched live (it took the restart rung:
// "trait `Words` removed").
pub trait Words {
    fn bytes(&self) -> u32;
}
pub struct A(u32);
pub struct B(u32);
impl Words for A {
    fn bytes(&self) -> u32 {
        self.0 * 4
    }
}
impl Words for B {
    fn bytes(&self) -> u32 {
        self.0 * 8
    }
}

fn size(a: &A, b: &B) -> u32 {
    a.bytes() + b.bytes()
}

#[test]
fn run() {
    println!("out {}", size(&A(1), &B(2)));
}
