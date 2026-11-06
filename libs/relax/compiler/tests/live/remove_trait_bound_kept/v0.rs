// A trait goes and an unchanged generic bound still names it: refused (E0405 at the bound).
pub trait Words {
    fn bytes(&self) -> u32;
}
pub struct A(u32);
impl Words for A {
    fn bytes(&self) -> u32 {
        self.0 * 4
    }
}

fn zero<T: Words>(_t: &T) -> u32 {
    0
}

#[test]
fn run() {
    println!("out {} {}", A(1).bytes(), zero(&A(2)));
}
