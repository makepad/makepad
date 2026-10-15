// rustc_hir_analysis/collect/resolve_bound_vars.rs @8d1a764: declaration
// capture sets, nested opaques, and default/overridden RPITIT bodies.
fn uncaptured<'a>(_: &'a u8) -> impl Copy + use<> { 7u8 }
fn nested<'a>(x: &'a u8) -> impl Iterator<Item = impl Copy + 'a> + 'a {
    std::iter::once(x)
}
trait Source<'a> {
    fn get<'b, T: 'b>(&'b self, x: &'b T) -> impl Copy + use<'a, 'b, Self, T> { x }
}
struct DefaultSource;
impl<'a> Source<'a> for DefaultSource {}
struct OverrideSource;
impl<'a> Source<'a> for OverrideSource {
    fn get<'b, T: 'b>(&'b self, x: &'b T) -> impl Copy + use<'a, 'b, T> { x }
}
fn copy_value<T: Copy>(value: T) { let _ = (value, value); }
#[test]
fn precise_capture_does_not_retain_input_borrow() {
    let result;
    { let local = 3; result = uncaptured(&local); }
    copy_value(result);
}
#[test]
fn nested_opaque_capture() {
    let value = 11;
    let mut values = nested(&value);
    copy_value(values.next().unwrap());
    assert!(values.next().is_none());
}
#[test]
fn default_and_override_rpitit() {
    let value = 5u32;
    copy_value(DefaultSource.get(&value));
    copy_value(OverrideSource.get(&value));
}
