// A const argument of an impl header may name an associated constant (platform/hal's
// FixedSizeEncoding): coherence evaluates it, so a `makepad check` must run coherence with the
// unit's const evaluator available (it used to report a false E0080).
pub trait Enc<const B: usize> { const LEN: usize = B; }
struct M;
impl M { const Z: usize = 4; }
struct L;
impl Enc<{ M::Z }> for L {}
impl Enc<{ (u16::BITS / 8) as usize }> for u16 {}
impl Enc<{ u8::MAX as usize }> for u8 {}
#[test]
fn header_consts() {
    assert_eq!(<L as Enc<4>>::LEN, 4);
    assert_eq!(<u16 as Enc<2>>::LEN, 2);
    assert_eq!(<u8 as Enc<255>>::LEN, 255);
}
