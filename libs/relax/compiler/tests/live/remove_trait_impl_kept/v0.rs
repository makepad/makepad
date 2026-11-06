// A trait goes and an impl of it stays: refused (E0405 at the impl's trait).
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
    println!("out {}", A(1).bytes());
}
