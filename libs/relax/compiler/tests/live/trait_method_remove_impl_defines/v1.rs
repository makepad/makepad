// A trait loses a method its impl still defines: refused, E0407 (the impl's own text
// did not change).
pub trait Words {
    fn bytes(&self) -> u32;

}
pub struct A(u32);
impl Words for A {
    fn bytes(&self) -> u32 {
        self.0 * 4
    }
    fn kb(&self) -> u32 {
        1
    }
}

#[test]
fn run() {
    println!("out {} {}", A(1).bytes(), 2);
}
