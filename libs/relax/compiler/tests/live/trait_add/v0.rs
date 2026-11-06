// A trait added with an impl, and a call of its method: the edit patches.
pub struct A(u32);

#[test]
fn run() {
    println!("out {}", A(1).0);
}
