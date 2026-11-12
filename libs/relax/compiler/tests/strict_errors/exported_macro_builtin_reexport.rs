//~ E0255
//! Importing the real built-in macro still conflicts with an exported macro.
mod builtins { pub use std::column; }
pub use builtins::column;
#[macro_export]
macro_rules! column { () => { 7 }; }
fn main() {}
