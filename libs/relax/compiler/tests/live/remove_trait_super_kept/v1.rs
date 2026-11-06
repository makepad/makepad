// A trait goes and an unchanged trait still lists it as a supertrait: refused (E0405). It patched:
// the trait's supertraits kept the removed one.
pub trait Loud: Words {}
pub struct A(u32);

#[test]
fn run() {
    println!("out {}", A(1).0);
}
