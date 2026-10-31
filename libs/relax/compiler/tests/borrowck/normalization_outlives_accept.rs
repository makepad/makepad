// Control for normalization_outlives_reject.rs: with `'a: 'b` declared, the
// constraint from the impl's where-clause holds.
trait Get<'b> { type Out; }
impl<'a: 'b, 'b> Get<'b> for &'a u32 { type Out = (); }
fn f<'a: 'b, 'b>(_x: &'a u32, _y: <&'a u32 as Get<'b>>::Out) {}
fn main() { f(&0, ()); }
