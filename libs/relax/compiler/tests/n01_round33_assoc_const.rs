// typeck-audit 1.9: an associated-const value path registers its predicates (`T: Tr`).
trait Tr { const C: u8; }
impl Tr for u16 { const C: u8 = 16; }
fn generic<T: Tr>() -> u8 { <T as Tr>::C + T::C }
#[test]
fn assoc_const_paths() { assert_eq!(generic::<u16>(), 32); assert_eq!(<u16 as Tr>::C, 16); }
