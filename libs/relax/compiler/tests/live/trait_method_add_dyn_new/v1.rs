// A trait gets a method before any trait object of it is made: the edit patches, and
// the new trait objects' tables hold it.
pub trait Words {
    fn bytes(&self) -> u32;
    fn kb(&self) -> u32 {
        self.bytes() + 100
    }
}
pub struct A(u32);
impl Words for A {
    fn bytes(&self) -> u32 {
        self.0 * 4
    }
}
pub struct B;
impl Words for B {
    fn bytes(&self) -> u32 {
        9
    }
    fn kb(&self) -> u32 {
        1000
    }
}
fn show(n: &dyn Words) -> u32 {
    n.kb() * 10 + n.bytes()
}

#[test]
fn run() {
    println!("out {} {} {}", A(1).bytes(), show(&A(2)), show(&B));
}
