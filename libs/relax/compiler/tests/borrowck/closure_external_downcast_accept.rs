// scope.rs: a by-value ADT input whose region reaches a method-call result.
use std::any::Any;
struct View<'a> { value: &'a dyn Any }
fn downcast(v: Option<View<'_>>) -> Option<&u32> {
    v.and_then(|r| r.value.downcast_ref())
}
fn main() { assert_eq!(downcast(Some(View { value: &42u32 })), Some(&42)); }
