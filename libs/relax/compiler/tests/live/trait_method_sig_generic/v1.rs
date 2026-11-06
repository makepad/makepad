// A trait method's signature changes and a generic caller follows (patch); then the
// generic caller's call is the old one's again: refused, E0061.
pub trait Words {
    fn bytes(&self, scale: u32) -> u32;
}
pub struct A(u32);
impl Words for A {
    fn bytes(&self, scale: u32) -> u32 {
        self.0 * scale
    }
}
fn total<T: Words>(t: &[T]) -> u32 {
    t.iter().map(|x| x.bytes(10)).sum()
}

#[test]
fn run() {
    println!("out {} {}", A(1).bytes(3), total(&[A(2), A(3)]));
}
