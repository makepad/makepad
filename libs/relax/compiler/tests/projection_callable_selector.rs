// Regression for projection recursion in ChildFactorySelector::select.
trait IntoValue { fn into_value(self) -> u32; }
impl IntoValue for u32 { fn into_value(self) -> u32 { self } }
trait Selector: Copy { fn select(self, cx: &mut u32) -> Option<u32>; }
impl<F, G> Selector for F
where F: Fn(&mut u32) -> Option<G> + Copy, G: IntoValue {
    fn select(self, cx: &mut u32) -> Option<u32> {
        self(cx).map(IntoValue::into_value)
    }
}
#[test]
fn callable_output_in_generic_impl() {
    let selector = |cx: &mut u32| Some(*cx);
    assert_eq!(selector.select(&mut 42), Some(42));
}
