//~ E0433
mod family {
    pub mod facade {
        mod hidden {
            pub(in crate::family) mod target { pub use crate::emit; }
        }
        macro_rules! publish {
            () => { pub(in crate::family) use self::hidden::target as alias; };
        }
        publish!();
        pub fn anchor() {}
    }

    pub fn inside() { facade::alias::emit!(); }
}

use family::facade::*;
#[macro_export]
macro_rules! emit { () => {} }
fn main() {
    anchor();
    family::inside();
    alias::emit!();
}
