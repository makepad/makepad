//! Type-level unsigned numbers: a binary list of bits, its value an associated constant.
use core::marker::PhantomData;

pub trait Bit {
    const BIT: usize;
}
pub struct B0;
pub struct B1;
impl Bit for B0 {
    const BIT: usize = 0;
}
impl Bit for B1 {
    const BIT: usize = 1;
}

pub trait Unsigned {
    const USIZE: usize;
}
pub struct UTerm;
pub struct UInt<U, B>(PhantomData<(U, B)>);
impl Unsigned for UTerm {
    const USIZE: usize = 0;
}
impl<U: Unsigned, B: Bit> Unsigned for UInt<U, B> {
    const USIZE: usize = U::USIZE * 2 + B::BIT;
}

pub mod sizes {
    use super::{UInt, UTerm, B0, B1};
    pub type U1 = UInt<UTerm, B1>;
    pub type U2 = UInt<U1, B0>;
    pub type U3 = UInt<U1, B1>;
    pub type U4 = UInt<U2, B0>;
    pub type U5 = UInt<U2, B1>;
    pub type U6 = UInt<U3, B0>;
    pub type U7 = UInt<U3, B1>;
    pub type U8 = UInt<U4, B0>;
}
