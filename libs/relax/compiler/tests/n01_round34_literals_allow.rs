#![allow(overflowing_literals)]
pub const A: u8 = 256;
pub const B: i8 = -129;
pub const C: i8 = 0xff;
pub const D: f32 = 1e100;
#[deny(overflowing_literals)]
pub fn allowed_statement() { #[allow(overflowing_literals)] 256u8; }
#[test] fn permitted_truncation() { assert_eq!(A, 0); assert_eq!(B, 127); assert_eq!(C, -1); }
