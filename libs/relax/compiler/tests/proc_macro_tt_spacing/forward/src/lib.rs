//! Macros of another crate, shaped like equator's `assert!`: `$($tokens:tt)*` passed on to a proc macro.
pub use spacing_macro::spacings;

#[macro_export]
macro_rules! forward {
    ($($tokens:tt)*) => { $crate::spacings!($($tokens)*) };
}

#[macro_export]
macro_rules! written {
    () => { $crate::spacings!(f::<u32>() !-x &&!x) };
}
