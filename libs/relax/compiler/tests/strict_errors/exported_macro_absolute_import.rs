//~ E0255
//! An absolute import names the extern crate even when a local module shadows it.
mod std { pub fn column() {} }
pub use ::std::column;
#[macro_export]
macro_rules! column { () => { 7 }; }
fn main() {}
