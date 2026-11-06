// A trait goes and a `dyn` of it stays in an unchanged signature: refused (E0405).
pub struct A(u32);

fn zero(_w: Option<&dyn Words>) -> u32 {
    0
}

#[test]
fn run() {
    println!("out {} {}", A(1).0, 0);
}
