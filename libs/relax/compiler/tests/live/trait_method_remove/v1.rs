// A trait loses a provided method nothing calls: the edit patches.
pub trait Words {
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
    println!("out {} {}", A(1).bytes(), 7);
}
