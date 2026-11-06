// A trait gets a method before any trait object of it is made: the edit patches, and
// the new trait objects' tables hold it.
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
