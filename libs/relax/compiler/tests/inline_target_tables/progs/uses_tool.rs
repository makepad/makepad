// The inline test: it is found at its own path, and the inline bin is built for it and named by
// CARGO_BIN_EXE_<name>, as cargo does for every bin of the package.
#[test]
fn the_inline_test_runs_the_inline_bin() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_inline-tool")).arg("7").output().unwrap();
    assert!(out.status.success(), "{:?}", out.status);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "inline-tool 49\n");
    assert_eq!(inline_target_tables::square(7), 49);
}
