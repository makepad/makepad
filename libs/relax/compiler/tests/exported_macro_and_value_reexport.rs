//! A function and an exported macro share a spelling: `pub use layout::column` imports the function,
//! not the built-in `column!`, so it is no E0255 beside `#[macro_export] macro_rules! column`.
mod layout {
    pub fn column() -> u32 { 7 }
    #[macro_export]
    macro_rules! column { () => { $crate::layout::column() + 1 }; }
}
pub use layout::column;
mod prelude { pub use crate::column; }

#[test]
fn functions_and_exported_macros_have_separate_namespaces() {
    assert_eq!(column(), 7);
    assert_eq!(column!(), 8);
    assert_eq!(prelude::column(), 7);
    assert_eq!(prelude::column!(), 8);
}
