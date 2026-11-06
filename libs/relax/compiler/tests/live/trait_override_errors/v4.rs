// Trait impl edits a fresh build rejects are refused: an override with another signature (E0053), a
// fn the trait does not have (E0407), a required fn removed (E0046); then an override patches.
pub trait Named {
    fn name(&self) -> u32 {
        1
    }
    fn id(&self) -> u32;
}
pub struct A;
impl Named for A {
    fn name(&self) -> u32 {
        8
    }
    fn id(&self) -> u32 {
        4
    }
}

#[test]
fn run() {
    println!("out {} {}", A.name(), A.id());
}
