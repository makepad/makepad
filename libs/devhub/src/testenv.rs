//! What this crate's tests start besides themselves: role processes of the test binary, kept to
//! their test's sandbox.

use std::path::Path;
use std::process::Command;

/// A role process of this test binary (`--exact <test>`), announcing itself in `sandbox`'s own
/// registry. Under the JIT it is a JIT of this package, which announces itself, and a lease's
/// run names the node's registry (`MAKEPAD_REMOTE_REGISTRY`, ahead of `XDG_RUNTIME_DIR`): the
/// node's hub watched the roles a test killed and reported each as a crash (bursts of 5-6
/// "makepad-devhub" crashes per test run on a Linux node, 11-04).
pub fn role(test: &str, sandbox: &Path) -> Command {
    let registry = sandbox.join("registry");
    std::fs::create_dir_all(&registry).unwrap();
    let mut c = Command::new(std::env::current_exe().unwrap());
    c.args(["--exact", test]).env("MAKEPAD_REMOTE_REGISTRY", registry);
    c
}

/// What [`a_role_announces_in_its_sandbox`] starts: writes where it would announce itself.
#[test]
fn testenv_role_registry() {
    let Some(file) = std::env::var_os("DEVHUB_TEST_REGISTRY_OUT") else { return };
    std::fs::write(file, makepad_remote::discovery::registry_dir().display().to_string()).unwrap();
}

#[test]
fn a_role_announces_in_its_sandbox() {
    let base = std::env::temp_dir().join(format!("devhub-testenv-{}", std::process::id()));
    let out = base.join("registry.txt");
    let status = role("testenv::testenv_role_registry", &base).env("DEVHUB_TEST_REGISTRY_OUT", &out).stdout(std::process::Stdio::null()).status().unwrap();
    assert!(status.success());
    assert_eq!(std::fs::read_to_string(&out).unwrap(), base.join("registry").display().to_string());
    let _ = std::fs::remove_dir_all(&base);
}

/// Writes an executable script that the test runs next. A child `sh` writes it, so this process
/// never holds a write fd to it: a role or lease process another test thread forks in that window
/// inherits such an fd until it execs, and an exec of the script then fails with ETXTBSY (exit
/// 126: "makepad.jit-x --version failed" in a fast-bucket run, 11-15).
#[cfg(unix)]
pub fn exec_script(path: &Path, text: &str) {
    use std::io::Write;
    let mut sh = Command::new("/bin/sh").args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"]).arg(path).stdin(std::process::Stdio::piped()).spawn().unwrap();
    sh.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
    let status = sh.wait().unwrap();
    assert!(status.success(), "writing the script {} failed: {status}", path.display());
}
