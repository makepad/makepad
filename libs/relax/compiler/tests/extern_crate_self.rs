// `extern crate self as name;` at the crate root names the crate itself (rustc: an item binding
// at the root plus an extern-prelude entry, so `name::..` works in every module) -- makepad-draw
// uses it so derive output written as `makepad_draw::..` compiles inside makepad-draw.
extern crate self as me;

pub mod a {
    pub fn f() -> u32 {
        7
    }
    pub struct S(pub u32);
}

mod b {
    pub fn g() -> u32 {
        me::a::f() + me::a::S(1).0
    }
}

#[test]
fn extern_crate_self() {
    assert_eq!(me::a::f(), 7);
    assert_eq!(b::g(), 8);
    let s: me::a::S = a::S(3);
    assert_eq!(s.0, 3);
    println!("extern_crate_self ok");
}
