// The level of a lint on an expression comes from the attributes around it.
#[deny(overflowing_literals)]
pub fn statement() { #[allow(overflowing_literals)] 256u8; }
#[deny(overflowing_literals)]
pub fn in_block() -> u8 { #[allow(overflowing_literals)] { 300u8 } }
#[test] fn runs() { statement(); assert_eq!(in_block(), 44); }
