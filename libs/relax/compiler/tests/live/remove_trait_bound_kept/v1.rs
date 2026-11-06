// A trait goes and an unchanged generic bound still names it: refused (E0405 at the bound).
pub struct A(u32);

fn zero<T: Words>(_t: &T) -> u32 {
    0
}

#[test]
fn run() {
    println!("out {} {}", A(1).0, 0);
}
