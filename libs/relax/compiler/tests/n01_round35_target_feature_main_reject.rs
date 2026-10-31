// T7: target_feature on an entry point; owner: entry-signature/check_attr follow-up.
#[target_feature(enable = "sse2")]
fn main() {}
#[test]
#[ignore = "T7: target_feature entry signature; check_attr"]
fn target_feature_entry() { unsafe { main(); } }
