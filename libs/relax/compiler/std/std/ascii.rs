//! Operations on ASCII strings and characters.

pub use core::ascii::*;
use alloc::string::String;
use alloc::vec::Vec;

// rustc library/std/src/ascii.rs: AsciiExt and delegating_ascii_methods.
#[allow(deprecated)]
#[deprecated(since = "1.26.0", note = "use inherent methods instead")]
pub trait AsciiExt {
    type Owned;
    fn is_ascii(&self) -> bool;
    fn to_ascii_uppercase(&self) -> Self::Owned;
    fn to_ascii_lowercase(&self) -> Self::Owned;
    fn eq_ignore_ascii_case(&self, other: &Self) -> bool;
    fn make_ascii_uppercase(&mut self);
    fn make_ascii_lowercase(&mut self);
}

macro_rules! delegating_ascii_methods {
    () => {
        fn is_ascii(&self) -> bool {
            self.is_ascii()
        }
        fn to_ascii_uppercase(&self) -> Self::Owned {
            self.to_ascii_uppercase()
        }
        fn to_ascii_lowercase(&self) -> Self::Owned {
            self.to_ascii_lowercase()
        }
        fn eq_ignore_ascii_case(&self, other: &Self) -> bool {
            self.eq_ignore_ascii_case(other)
        }
        fn make_ascii_uppercase(&mut self) {
            self.make_ascii_uppercase();
        }
        fn make_ascii_lowercase(&mut self) {
            self.make_ascii_lowercase();
        }
    };
}

#[allow(deprecated)]
impl AsciiExt for u8 {
    type Owned = u8;
    delegating_ascii_methods!();
}

#[allow(deprecated)]
impl AsciiExt for char {
    type Owned = char;
    delegating_ascii_methods!();
}

#[allow(deprecated)]
impl AsciiExt for [u8] {
    type Owned = Vec<u8>;
    delegating_ascii_methods!();
}

#[allow(deprecated)]
impl AsciiExt for str {
    type Owned = String;
    delegating_ascii_methods!();
}
