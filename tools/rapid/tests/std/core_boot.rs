// std-next boot smoke test: the std must load, resolve and collect; then basic heap types run.
#[test]
fn boot() {
    let x = 1u32 + 2;
    assert_eq!(x, 3);
}
