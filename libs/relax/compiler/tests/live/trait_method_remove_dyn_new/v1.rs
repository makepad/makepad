// A trait loses a provided method and its first trait objects are made after: the edit
// patches, the tables have one method.
pub trait Words {
    fn bytes(&self) -> u32;
}
pub struct A(u32);
impl Words for A {
    fn bytes(&self) -> u32 {
        self.0 * 4
    }
}
fn show(n: &dyn Words) -> u32 {
    n.bytes() + 50
}

#[test]
fn run() {
    println!("out {} {}", A(1).bytes(), show(&A(3)));
}
