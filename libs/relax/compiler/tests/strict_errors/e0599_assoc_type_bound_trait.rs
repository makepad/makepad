//~ E0599
// a bound on an associated type does not bring its trait's methods into scope
mod traits { pub trait Shout { fn shout(&self) -> u32 { 7 } } impl Shout for u8 {} }
trait Has { type A: traits::Shout; fn get(&self) -> Self::A; }
fn call<T: Has>(t: T) -> u32 { t.get().shout() }
struct S;
impl Has for S { type A = u8; fn get(&self) -> u8 { 1 } }
fn main() { let _ = call(S); }
