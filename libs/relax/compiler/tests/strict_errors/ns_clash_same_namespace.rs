//~ E0255
mod m { pub fn f() {} }
pub use m::f;
fn f() {}
fn main() {}
