// `Box<dyn Any + Send + Sync>::downcast` (relax issue 109): real alloc has it beside the `dyn Any` and
// `dyn Any + Send` ones.
use std::any::Any;

#[test]
fn a_send_sync_any_box_downcasts() {
    let boxed: Box<dyn Any + Send + Sync> = Box::new(7u32);
    let boxed = boxed.downcast::<String>().unwrap_err();
    let n: Box<u32> = boxed.downcast().unwrap();
    assert_eq!(*n, 7);
}
