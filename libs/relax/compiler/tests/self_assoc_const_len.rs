// `Self::CONST` / `<Self as Tr>::CONST` as an array length in an impl of a different trait
// (itoa 1.0: `type Buffer = [MaybeUninit<u8>; Self::MAX_STR_LEN]` in `impl Sealed for u32`, the
// const from `impl Integer for u32`): a type-relative path on the self type resolves over every
// trait the type implements that declares the const, not only the enclosing impl's trait
// (rustc resolve_fully_qualified_call); `Self` in a qualified path is the impl's self type.
use std::mem::MaybeUninit;

pub trait Integer: Sealed { const MAX_STR_LEN: usize; }
pub trait Sealed: Copy {
    type Buffer: 'static;
    fn cap(self) -> usize;
}
impl Integer for u32 { const MAX_STR_LEN: usize = u32::MAX.ilog10() as usize + 1; }
impl Sealed for u32 {
    type Buffer = [MaybeUninit<u8>; Self::MAX_STR_LEN];
    fn cap(self) -> usize { std::mem::size_of::<Self::Buffer>() }
}
impl Integer for i64 { const MAX_STR_LEN: usize = i64::MAX.ilog10() as usize + 2; }
impl Sealed for i64 {
    type Buffer = [MaybeUninit<u8>; <Self as Integer>::MAX_STR_LEN];
    fn cap(self) -> usize { std::mem::size_of::<Self::Buffer>() }
}

#[test]
fn self_const_of_another_trait() {
    assert_eq!(<u32 as Integer>::MAX_STR_LEN, 10);
    assert_eq!(5u32.cap(), 10);
}

#[test]
fn qualified_self_const() {
    assert_eq!(5i64.cap(), 20);
}
