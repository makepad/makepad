// rustc_middle/src/ty/normalize_erasing_regions.rs:140-149; codegen opens
// the item's region binder and preserves any nested function binders.
trait Get<'b> { type Out; }
impl<'a, 'b> Get<'b> for &'a u32 { type Out = (); }
fn projected<'a, 'b>(_x: &'a u32, _y: <&'a u32 as Get<'b>>::Out) {}
#[test]
fn codegen_normalizes_bound_signature_payload() { projected(&0, ()); }
#[test]
fn generic_pattern_return_projection() {
    let mut iter = "a=b=c".match_indices('=');
    assert_eq!(iter.next(),Some((1,"=")));
    assert_eq!(iter.next(),Some((3,"=")));
    assert_eq!(iter.next(),None);
}
