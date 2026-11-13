// The package's binary as its test starts it (CARGO_BIN_EXE): built by the test's build, with the
// test's inputs, whatever environment the test gives it (relax issue 168).
use std::process::Command;

fn run(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn the_binary_is_the_tests_build_in_any_environment() {
    let exe = env!("CARGO_BIN_EXE_pc-app");
    let built = "home true path true 5\n";
    assert_eq!(run(&mut Command::new(exe)), built, "started with the test's environment");
    assert_eq!(run(Command::new(exe).arg("--empty").env_clear()), built, "started with a cleared environment");
    assert_eq!(run(Command::new(exe).arg("--nested").env_clear()), built, "started with a cleared environment, starting itself with one");
    assert_eq!(run(Command::new(exe).arg("--nested")), built, "started with the test's environment, starting itself with a cleared one");
    #[cfg(unix)]
    assert_eq!(run(Command::new(exe).args(["--shell-nested", "--shell-changed"]).env_clear()), built, "started with a cleared environment, exec'd again by a shell");
}
