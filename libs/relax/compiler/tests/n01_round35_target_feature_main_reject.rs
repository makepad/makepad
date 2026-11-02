// `#[target_feature]` on `main`: under the test harness `main` is no entry point, so rustc --test
// accepts it on x86_64 (a binary rejects it: check/entry.rs TargetFeatureOnMain).
#[target_feature(enable = "sse2")]
fn main() {}
#[test]
fn target_feature_entry() { unsafe { main(); } }
