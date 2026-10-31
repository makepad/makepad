#[path = "../examples/gen_unroll.rs"]
#[allow(dead_code)]
mod gen_unroll;

#[test]
fn checked_in_unroll_matches_generator() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("unroll.rs");
    let checked_in = std::fs::read_to_string(&path).unwrap();
    assert!(
        checked_in == gen_unroll::generate(),
        "src/unroll.rs is stale: run `cargo run --release -p crunchy --example gen_unroll`"
    );
}
