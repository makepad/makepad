// A trait gets a required method that its impl does not define: refused, E0046.
pub trait Words {
    fn bytes(&self) -> u32;
    fn kb(&self) -> u32;
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
