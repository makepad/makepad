//~ none
pub fn f() {}
mod bridge {
    pub use crate::f as core;
    pub use core as alias;
}
fn main() {
    bridge::alias();
    let _ = bridge::alias::stringify!(x);
}
