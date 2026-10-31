//~ E0599
//~ help: the method is in trait `Shout`, which is not in scope: `use crate::traits::Shout;`
mod traits { pub trait Shout { fn shout(&self) -> String; } impl Shout for String { fn shout(&self) -> String { self.to_uppercase() } } }
fn main() { let s = String::from("a"); let _ = s.shout(); }
