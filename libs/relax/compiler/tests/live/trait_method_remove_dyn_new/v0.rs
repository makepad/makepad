// A trait loses a provided method and its first trait objects are made after: the edit
// patches, the tables have one method.
pub trait Words {
    fn first(&self) -> u32 {
        1
    }
    fn bytes(&self) -> u32;
}
pub struct A(u32);
impl Words for A {
    fn bytes(&self) -> u32 {
        self.0 * 4
    }
}

#[test]
fn run() {
    println!("out {}", A(1).bytes() + A(1).first());
}
