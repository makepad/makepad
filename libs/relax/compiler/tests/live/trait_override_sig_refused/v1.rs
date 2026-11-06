// A trait impl's fn replaced by one of another signature is refused (E0053), and the next edit
// finds the running fn linked to the trait's item again.
pub trait Named {
    fn name(&self, k: u32) -> u32;
}
pub struct A;
impl Named for A {
    fn name(&self, k: u64) -> u32 {
        k as u32 + 1
    }
}
fn show(n: &dyn Named) -> u32 {
    n.name(3)
}

#[test]
fn run() {
    println!("out {} {}", A.name(2), show(&A));
}
