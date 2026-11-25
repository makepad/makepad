// rejects: attribute cannot be used on structs
// `#[target_feature]` on a struct: rustc rejects ("cannot be used on structs").
// (a plain fn, not a #[test]: `cargo check` of the crate, without cfg(test), rejects it too)
fn target_feature_on_non_function() {
    #[target_feature(enable = "sse2")]
    struct Invalid;
    let _ = Invalid;
}
