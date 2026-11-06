// A trait impl's fn is replaced (its parameter pattern changes the signature's text): the old one
// leaves before the new one is linked to the trait's item, and callers through `dyn` run the new
// one (E0201, E0046).
pub trait Named {
    fn name(&self, k: u32) -> u32;
}
pub struct A;
impl Named for A {
    fn name(&self, _k: u32) -> u32 {
        50
    }
}
fn show(n: &dyn Named) -> u32 {
    n.name(3)
}

#[test]
fn run() {
    println!("out {} {}", A.name(2), show(&A));
}
