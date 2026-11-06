// A trait gets a method while the running code made trait objects of it: their method
// tables are the old trait's, the edit asks for a restart.
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
fn show(n: &dyn Words) -> u32 {
    n.bytes() + n.kb()
}

#[test]
fn run() {
    println!("out {}", show(&A(1)));
}
