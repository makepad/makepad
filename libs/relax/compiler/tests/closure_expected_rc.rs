use std::any::Any;
use std::rc::Rc;

#[test]
fn closure_output_from_expected_option() {
    let boxed: Box<dyn Any> = Box::new(42u32);
    let value: Option<Rc<dyn Any>> = Some(boxed).map(|value| Rc::from(value));
    assert_eq!(value.unwrap().downcast_ref::<u32>(), Some(&42));
}
