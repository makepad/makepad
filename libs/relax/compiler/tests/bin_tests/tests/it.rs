use std::process::Command;

#[test]
fn runs_the_bin() {
    let out = Command::new(env!("CARGO_BIN_EXE_bt-app")).arg("x").arg("y z").output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hello from bin x,y z 42\n");
}

#[test]
fn target_tmpdir() {
    let d = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    assert!(d.is_dir());
}
