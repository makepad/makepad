// A struct goes and an unchanged fn signature still names it: refused (E0412 at the name), as a
// fresh build refuses it. The signature kept the removed type, and the edit patched.
pub struct S(u32);

fn f(_s: &S) -> u32 {
    0
}

#[test]
fn run() {
    println!("out {}", f(&S(1)));
}
