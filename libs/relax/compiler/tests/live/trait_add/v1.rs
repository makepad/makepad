// A trait added with an impl, and a call of its method: the edit patches.
pub struct A(u32);
pub trait Words {
    fn bytes(&self) -> u32;
}
impl Words for A {
    fn bytes(&self) -> u32 {
        self.0 * 4
    }
}

#[test]
fn run() {
    println!("out {}", A(1).bytes());
}
