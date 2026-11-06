// A trait goes while code still calls its method: refused (E0599), as rustc refuses the program.
pub struct A(u32);
pub struct B(u32);

fn size(a: &A, b: &B) -> u32 {
    a.bytes() + b.bytes()
}

#[test]
fn run() {
    println!("out {}", size(&A(1), &B(2)));
}
