#![allow(dead_code)]
#[repr(u8)] enum Eval { A = 255u8 + 1u8 }
#[repr(u8)] enum Overflow { A = 255, B }
#[repr(i8)] enum SignedOverflow { A = 127, B }
#[repr(u8)] enum Duplicate { A = 2, B = 2, C = 2 }
fn main() {}
