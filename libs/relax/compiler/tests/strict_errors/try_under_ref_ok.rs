//~ none
// Where the referent of a `?` under `&` is only a hint, the `&` coerces at the enclosing site:
// an unsized or generic referent, a later match arm's (the earlier arm's type) and the right
// operand of `&A == &B` (left open by `&A: PartialEq<&B>`).
use std::rc::Rc;
struct Code(i32);
impl PartialEq<Code> for i32 {
    fn eq(&self, other: &Code) -> bool { *self == other.0 }
}
fn shared(n: u32) -> Result<Rc<Vec<u32>>, String> { Ok(Rc::new(vec![n])) }
fn owned(n: u32) -> Result<Vec<u32>, String> { Ok(vec![n]) }
fn code(n: i32) -> Result<Code, String> { Ok(Code(n)) }
fn len(s: &[u32]) -> usize { s.len() }
fn one<T: ?Sized>(_: &T) -> usize { 1 }
fn run(n: u32) -> Result<usize, String> {
    let rc = shared(n)?;
    let a = Vec::clone(&rc);
    let b = Vec::clone(&owned(n)?);
    let c = Vec::clone(&*shared(n)?);
    let d = len(&shared(n)?) + one(&shared(n)?);
    let same = &5i32 == &code(5)?;
    let w = vec![n];
    let e = match a.len() { 0 => &w, _ => &shared(n)? };
    let f = match b.len() { 0 => &w, _ => { &shared(n)? } };
    Ok(a.len() + b.len() + c.len() + d + e.len() + f.len() + same as usize)
}
fn main() {
    let _ = run(1);
}
