//~ E0599
// the bound of an `impl Trait` return type does not bring the trait's methods into scope
mod traits { pub trait Shout { fn shout(&self) -> u32 { 7 } } impl Shout for u8 {} pub fn make() -> impl Shout { 1u8 } }
fn main() { let _ = traits::make().shout(); }
