// A trait method's signature changes with its impl and its caller: the edit patches.
pub trait Words {
    fn bytes(&self, scale: u32) -> u32;
}
pub struct A(u32);
impl Words for A {
    fn bytes(&self, scale: u32) -> u32 {
        self.0 * scale
    }
}

#[test]
fn run() {
    println!("out {}", A(1).bytes(8));
}
