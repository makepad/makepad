// Crate-root exports are visible even before their textual definitions.
pub const FORWARD: i32 = crate::exported!();

#[cfg(feature = "tweaker")]
#[macro_export]
macro_rules! exported {
    () => { 11 };
}
#[cfg(not(feature = "tweaker"))]
#[macro_export]
macro_rules! exported {
    () => { 12 };
}

// Reverse the feature order so neither setting can pass by taking the last one.
#[cfg(not(feature = "tweaker"))]
#[macro_export]
macro_rules! reversed {
    () => { 22 };
}
#[cfg(feature = "tweaker")]
#[macro_export]
macro_rules! reversed {
    () => { 21 };
}

macro_rules! textual {
    () => { 30 };
}
pub const BEFORE: i32 = textual!();
#[cfg(feature = "tweaker")]
macro_rules! textual {
    () => { 31 };
}
#[cfg(not(feature = "tweaker"))]
macro_rules! textual {
    () => { 32 };
}
pub const AFTER: i32 = textual!();
#[cfg(any())]
macro_rules! textual {
    () => { 99 };
}
pub const AFTER_DISABLED: i32 = textual!();

#[cfg_attr(all(), cfg_attr(feature = "tweaker", macro_export))]
macro_rules! conditional_export {
    () => { 41 };
}
pub const CONDITIONAL_LOCAL: i32 = conditional_export!();
#[cfg(not(feature = "tweaker"))]
#[macro_export]
macro_rules! conditional_export {
    () => { 42 };
}
pub const CONDITIONAL_ROOT: i32 = crate::conditional_export!();

// Nested cfg_attr must evaluate the inner predicate before its cfg(false).
#[cfg_attr(all(), cfg_attr(any(), cfg(any())))]
#[macro_export]
macro_rules! nested_cfg_attr {
    () => { 50 };
}
#[cfg_attr(all(), cfg_attr(all(), cfg(any())))]
#[macro_export]
macro_rules! nested_cfg_attr {
    () => { 99 };
}

#[macro_use]
mod earlier;
pub const EARLIER: i32 = earlier::VALUE;
#[macro_use]
mod imported;
pub const IMPORTED: i32 = imported!();

mod nested;

#[cfg(any())]
mod inactive {
    #[macro_export]
    macro_rules! exported {
        () => { 98 };
    }
}
mod inactive_inner {
    #![cfg(any())]
    #[macro_export]
    macro_rules! reversed {
        () => { 97 };
    }
}

// The original makepad-draw pattern from before uidata-refl@488de99828.
#[cfg(feature = "tweaker")]
#[macro_export]
macro_rules! tweaker_only { ($($body:tt)*) => { $($body)* }; }
#[cfg(not(feature = "tweaker"))]
#[macro_export]
macro_rules! tweaker_only { ($($body:tt)*) => {}; }
#[cfg(feature = "tweaker")]
#[macro_export]
macro_rules! shader_tweaker_value { ($enabled:expr, $disabled:expr) => { $enabled }; }
#[cfg(not(feature = "tweaker"))]
#[macro_export]
macro_rules! shader_tweaker_value { ($enabled:expr, $disabled:expr) => { $disabled }; }

pub const SHADER_TWEAKER: bool = cfg!(feature = "tweaker");
pub trait Reflect {
    fn reflected(&self) -> i32;
}
pub struct Local;
tweaker_only! {
    impl Reflect for Local {
        fn reflected(&self) -> i32 { 81 }
    }
}
pub fn local_reflect() -> i32 {
    shader_tweaker_value!(Reflect::reflected(&Local), 82)
}
