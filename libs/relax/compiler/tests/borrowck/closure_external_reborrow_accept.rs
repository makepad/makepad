// scope.rs: preserve both the outer reference and the ADT lifetime slots.
use std::any::Any;
struct Borrowed<'a> { value: &'a mut dyn Any }
struct View<'a> { value: &'a dyn Any }
struct ViewMut<'a> { value: &'a mut dyn Any }
fn shared<'a, 'b>(v: &'b Option<Borrowed<'a>>) -> Option<View<'b>> {
    v.as_ref().map(|r| View { value: &*r.value })
}
fn mutable<'a, 'b>(v: &'b mut Option<Borrowed<'a>>) -> Option<ViewMut<'b>> {
    v.as_mut().map(|r| ViewMut { value: &mut *r.value })
}
fn downcast<'a, 'b>(v: &'b Option<Borrowed<'a>>) -> Option<&'b u32> {
    v.as_ref().and_then(|r| r.value.downcast_ref())
}
fn downcast_mut<'a, 'b>(v: &'b mut Option<Borrowed<'a>>) -> Option<&'b mut u32> {
    v.as_mut().and_then(|r| r.value.downcast_mut())
}
fn main() {}
