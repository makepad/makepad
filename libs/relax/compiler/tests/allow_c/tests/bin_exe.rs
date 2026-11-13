// relax issue 119: the binary an integration test starts through CARGO_BIN_EXE_<bin> is the one cargo
// builds: its argv[0] names it, and its stderr holds only what it prints itself (verify.py runs this
// under `cargo test` and `makepad test --allow-c`)
#[test]
fn the_packages_binary_names_itself_and_keeps_its_stderr() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_allow-c-fixture")).arg("--name").output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "allow-c-fixture\n");
}
