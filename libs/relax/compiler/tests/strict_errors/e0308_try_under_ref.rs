//~ E0308
// The value of a `?` under `&` coerces to the referent: rustc checks a `&` operand under the
// referent's type, and the `?`'s match coerces its value to it. `&Rc<Vec<_>>` would
// deref-coerce to `&Vec<_>` at the call, but `Rc<Vec<_>>` is no `Vec<_>`.
use std::rc::Rc;
fn shared(n: u32) -> Result<Rc<Vec<u32>>, String> { Ok(Rc::new(vec![n])) }
fn owned(n: u32) -> Result<Vec<u32>, String> { Ok(Vec::clone(&shared(n)?)) }
fn main() {
    let _ = owned(1);
}
