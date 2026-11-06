// A trait goes and an unchanged trait still lists it as a supertrait: refused (E0405). It patched:
// the trait's supertraits kept the removed one.
pub trait Words {
    fn bytes(&self) -> u32;
}
pub trait Loud: Words {}
pub struct A(u32);
impl Words for A {
    fn bytes(&self) -> u32 {
        self.0 * 4
    }
}

#[test]
fn run() {
    println!("out {}", A(1).bytes());
}
