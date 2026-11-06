// A supertrait gets a method, read through a subtrait's trait object made after the
// edit: the edit patches.
pub trait Words {
    fn bytes(&self) -> u32;
    fn kb(&self) -> u32 {
        self.bytes() + 100
    }
}
pub trait More: Words {
    fn more(&self) -> u32;
}
pub struct A(u32);
impl Words for A {
    fn bytes(&self) -> u32 {
        self.0 * 4
    }
}
impl More for A {
    fn more(&self) -> u32 {
        self.bytes() + 1
    }
}
fn show(n: &dyn More) -> u32 {
    n.more() * 1000 + n.kb()
}

#[test]
fn run() {
    println!("out {} {} {}", A(1).bytes(), A(1).more(), show(&A(2)));
}
