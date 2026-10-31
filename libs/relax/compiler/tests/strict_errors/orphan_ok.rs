//~ none
// accepted by rustc: local types (also behind &, Box), a local type as a trait argument, a const
// param before it, inherent impls of local types and local trait objects
use std::fmt;
struct L;
trait Tr {}
impl fmt::Display for L { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { write!(f, "L") } }
impl From<L> for Vec<u8> { fn from(_: L) -> Vec<u8> { Vec::new() } }
impl<'a> PartialEq<&'a L> for Box<L> { fn eq(&self, _o: &&'a L) -> bool { true } }
impl<T> From<Box<T>> for L where T: Tr { fn from(_: Box<T>) -> L { L } }
impl L { fn f(&self) {} }
impl dyn Tr { fn g(&self) {} }
fn main() { L.f(); let _ = Vec::<u8>::from(L); }
