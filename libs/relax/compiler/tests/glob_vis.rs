// F33: a glob re-export carries only the bindings visible to the importer (rustc_resolve
// imports.rs: glob binding vis = min(glob vis, binding vis)); a module's private `use` of a
// trait must not put that trait in scope for `use lib::*` users.
use std::cell::RefCell;
mod lib {
    pub mod string {
        #[allow(unused_imports)]
        use std::borrow::Borrow;
        pub fn n() -> u32 { 1 }
        pub(crate) fn inner() -> u32 { 2 }
        pub mod child {
            use super::*;
            // a child sees its parent's private imports through `use super::*`
            pub fn b(s: &String) -> usize { let x: &str = s.borrow(); x.len() }
        }
    }
    pub use string::*;
}
mod user {
    use super::lib::*;
    use std::cell::RefCell;
    pub fn f(c: &RefCell<Vec<u32>>) -> usize {
        // RefCell::borrow, not Borrow::borrow (Borrow is not in scope here)
        let v = c.borrow();
        v.len() + n() as usize + inner() as usize
    }
}
mod shadow {
    mod a { pub struct X(pub u32); }
    mod b {
        #[allow(dead_code)]
        struct X(pub u64);
        pub fn g() -> u32 { 7 }
    }
    pub use a::*;
    pub use b::*;
    pub fn h() -> u32 { X(5).0 + g() }
}
#[test]
fn glob_private_use_does_not_leak() {
    let c = RefCell::new(vec![1u32, 2, 3]);
    assert_eq!(user::f(&c), 6);
    assert_eq!(lib::string::child::b(&String::from("abcd")), 4);
    assert_eq!(shadow::h(), 12);
}
