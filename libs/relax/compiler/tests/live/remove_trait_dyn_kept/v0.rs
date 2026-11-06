// A trait goes and a `dyn` of it stays in an unchanged signature: refused (E0405).
pub trait Words {
    fn bytes(&self) -> u32;
}
pub struct A(u32);
impl Words for A {
    fn bytes(&self) -> u32 {
        self.0 * 4
    }
}

fn zero(_w: Option<&dyn Words>) -> u32 {
    0
}

#[test]
fn run() {
    println!("out {} {}", A(1).bytes(), zero(None));
}
