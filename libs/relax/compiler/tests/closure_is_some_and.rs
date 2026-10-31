#[test]
fn successive_closure_method_calls() {
    assert!("entry::one".strip_prefix("entry").is_some_and(|rest| rest.starts_with("::")));
    assert!("entry::two".strip_prefix("entry").is_some_and(|rest| rest.starts_with("::")));
}
