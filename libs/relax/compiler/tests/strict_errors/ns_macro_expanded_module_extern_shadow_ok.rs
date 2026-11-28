//~ none
mod family {
    pub mod facade {
        pub(in crate::family) mod target { pub use crate::number as stringify; }
        macro_rules! publish {
            () => {
                pub(in crate::family) use self::target as core;
                pub(in crate::family) use core as alias;
            };
        }
        publish!();
    }
    pub fn inside() -> u8 { facade::alias::stringify!(x) }
}

#[macro_export]
macro_rules! number { ($x:ident) => { 7u8 } }
fn main() {
    let _: u8 = family::inside();
}
