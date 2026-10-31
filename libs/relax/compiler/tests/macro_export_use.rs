// A `#[macro_export]` macro_rules! that a macro expansion defines inside a module, re-exported
// there under another name and imported again (makepad-draw's DrawShader derive output does
// this): the imports bind the macro, and calls through the paths and the aliases resolve.
#[macro_export]
macro_rules! make {
    ($name:ident, $d:tt) => {
        #[doc(hidden)]
        #[macro_export]
        macro_rules! $name {
            ($d x:expr) => { $d x + 1 };
        }
    };
}
pub mod rt {
    pub use crate::make;
}
use crate::rt as runtime_alias;
mod m {
    super::runtime_alias::make!(__gen_abc, $);
    pub use __gen_abc as ancestry;
}
use m::ancestry as base_alias;
#[test]
fn macro_export_use() {
    assert_eq!(__gen_abc!(1), 2);
    assert_eq!(base_alias!(41), 42);
    assert_eq!(m::ancestry!(41), 42);
}
