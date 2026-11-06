// A trait method's signature changes and a generic caller follows (patch); then the
// generic caller's call is the old one's again: refused, E0061.
pub trait Words {
    fn bytes(&self) -> u32;
}
pub struct A(u32);
impl Words for A {
    fn bytes(&self) -> u32 {
        self.0 * 4
    }
}
fn total<T: Words>(t: &[T]) -> u32 {
    t.iter().map(|x| x.bytes()).sum()
}

#[test]
fn run() {
    println!("out {} {}", A(1).bytes(), total(&[A(2), A(3)]));
}
