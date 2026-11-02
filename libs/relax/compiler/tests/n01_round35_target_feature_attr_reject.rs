// `#[target_feature]` on a struct: rustc rejects ("cannot be used on structs").
#[test]
fn target_feature_on_non_function() {
    #[target_feature(enable = "sse2")]
    struct Invalid;
    let _ = Invalid;
}
