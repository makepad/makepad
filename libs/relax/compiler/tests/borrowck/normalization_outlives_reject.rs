// Normalizing `<&'a u32 as Get<'b>>::Out` selects an impl that requires `'a: 'b`;
// borrowck's signature normalization (free_region_relations.rs, fully_perform_op)
// must hand that outlives constraint to region inference (rustc rejects).
trait Get<'b> { type Out; }
impl<'a: 'b, 'b> Get<'b> for &'a u32 { type Out = (); }
fn f<'a, 'b>(_x: &'a u32, _y: <&'a u32 as Get<'b>>::Out) {}
fn main() { f(&0, ()); }
