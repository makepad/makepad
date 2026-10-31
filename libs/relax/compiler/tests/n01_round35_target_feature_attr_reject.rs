// T7: target_feature check_attr; owner: entry-signature/check_attr follow-up.
#[test]
#[ignore = "T7: target_feature attribute targets; check_attr"]
fn target_feature_on_non_function() {
    #[target_feature(enable = "sse2")]
    struct Invalid;
    let _ = Invalid;
}
