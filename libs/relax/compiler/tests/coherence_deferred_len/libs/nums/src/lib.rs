//! Type-level unsigned numbers; their aliases are in a module of their own.
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

pub mod sizes;
