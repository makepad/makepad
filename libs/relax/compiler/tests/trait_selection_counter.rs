#[test]
fn tuple_clone_clones_owned_fields() {
    let original = (vec![String::from("before")], (String::from("nested"), ()));
    let mut cloned = original.clone();
    cloned.0[0].push_str(" after");
    assert_eq!(original.0[0], "before");
    assert_eq!(cloned.0[0], "before after");
    assert_eq!(cloned.1.0, "nested");
    assert_eq!(().clone(), ());
}

#[test]
fn removing_auto_traits_keeps_object_dispatch() {
    let source: Box<dyn std::any::Any + Send + Sync> = Box::new(42u32);
    let shared: &dyn std::any::Any = &*source;
    assert_eq!(shared.downcast_ref::<u32>(), Some(&42));
    let owned: Box<dyn std::any::Any> = source;
    assert_eq!(*owned.downcast::<u32>().unwrap(), 42);
}

#[test]
fn assert_unwind_safe_wraps_non_unwind_safe_captures() {
    let value = std::cell::RefCell::new(0);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        *value.borrow_mut() += 1;
    }));
    assert!(result.is_ok());
    assert_eq!(*value.borrow(), 1);
}

#[test]
fn nested_iterator_projection_and_pending_closures() {
    let values = vec![(String::from("a"), 3u64), (String::from("b"), 8u64)];
    let maximum = values.iter().map(|entry| entry.1).chain(std::iter::once(5u64)).max();
    let strings: Vec<_> = values.iter().map(|entry| entry.0.clone()).collect();
    let lengths: Vec<_> = strings.iter().map(|text| text.len()).collect();
    assert_eq!(maximum, Some(8));
    assert_eq!(lengths, vec![1, 1]);
}
