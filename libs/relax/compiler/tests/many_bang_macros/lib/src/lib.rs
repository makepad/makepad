//! futures-util's shape: a proc macro re-exported and called through `$crate::` by a macro_rules!
#[doc(hidden)]
pub use many_bang_mac::*;

#[macro_export]
macro_rules! last_value {
    () => {{ $crate::value_63!() }};
}
