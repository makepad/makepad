// front-bck audit 1.14 / 1.16: rustc_resolve check_trait_item (E0407, E0437, E0438, E0323,
// E0324, E0325, E0201) and the `default` diagnostics. Checked as a package with `makepad check`.
#![allow(dead_code)]
trait Tr { fn f(&self); const C: u8; type T; }
struct S;
impl Tr for S {
    fn f(&self) {}
    const C: u8 = 0;
    type T = ();
    fn g(&self) {}
    type U = ();
    const D: u8 = 1;
}
struct M;
impl Tr for M {
    const f: u8 = 0;
    fn C(&self) {}
    type T = ();
}
struct Dup;
impl Tr for Dup {
    fn f(&self) {}
    fn f(&self) {}
    const C: u8 = 0;
    type T = ();
}
default fn free() {}
trait WithDefault { default fn m(&self) {} }
impl S { default fn inherent(&self) {} }
