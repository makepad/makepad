// const_eval_resolve executes the anonymous constant body through the JIT.
const MAGIC: &[u8; 4] = b"OPEN";
#[test]
fn method_call_in_anonymous_array_length() {
    let bytes = [0u8; MAGIC.len()];
    assert_eq!(bytes.len(),4);
}

#[test]
fn scalar_from_aggregate() {
    const FIRST: u8 = [7u8, 9u8][0];
    assert_eq!(FIRST, 7);
}

struct SelfArray { bytes: [u8; Self::COUNT] }
impl SelfArray { const COUNT: usize = 3; }
#[test]
fn self_in_field_constant() { assert_eq!(SelfArray { bytes: [1; 3] }.bytes.len(), 3); }
fn flag<const B: bool>() -> bool { B }
fn character<const C: char>() -> char { C }
#[test]
fn scalar_const_argument_widths() { assert!(flag::<true>()); assert_eq!(character::<'Q'>(), 'Q'); }
trait Encoding<const N: usize> {
    const LEN: usize = N;
    fn decode(bytes: [u8; N]) -> Self;
}
impl Encoding<{ (Self::BITS / 8) as usize }> for u64 {
    fn decode(bytes: [u8; Self::LEN]) -> Self { u64::from_le_bytes(bytes) }
}
fn receive<T: Encoding<N>, const N: usize>() -> T {
    let bytes = [0; N];
    T::decode(bytes)
}
#[test]
fn inferred_const_impl_argument_reaches_repeat() { assert_eq!(receive::<u64, _>(), 0); }

#[test]
fn local_const_size_of_in_array_length() {
    const SIZE: usize = core::mem::size_of::<u16>();
    let space = [0u8; SIZE];
    assert_eq!(u16::from_be_bytes(space), 0);
}
