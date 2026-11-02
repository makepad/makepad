// finalize_import: a private module does not hide the macro binding of the same name.
mod zv {
    #[allow(dead_code)] mod zerovec { pub fn f() -> u8 { 1 } }
    macro_rules! zerovec_impl { ($($x:expr),*) => { vec![$($x * 2),*] } }
    pub(crate) use zerovec_impl as zerovec;
}
use zv::zerovec;
#[test] fn expands() { assert_eq!(zerovec![1, 2], [2, 4]); }
