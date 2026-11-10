// The run-time module fixtures (libs/relax/compiler/tests/modules): run.sh runs each fixture
// program under the runner and checks its output, its tier-up and its reload trace; every case
// runs, and the failing ones print their output. One cargo test, so the test buckets run it.
use std::process::Command;

#[test]
fn the_module_fixtures_pass_under_the_runner() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = root.join("libs/relax/compiler/tests/modules/run.sh");
    // Under `makepad test` the runner is the one running this test (its loader); under cargo
    // it is the binary cargo just built for this package. (`CARGO_BIN_EXE_makepad` is, under the
    // runner, a wrapper that would run the runner's own main as a program.)
    let runner = std::env::var("MAKEPAD_RUN_LOADER").unwrap_or_else(|_| env!("CARGO_BIN_EXE_makepad").to_string());
    let output = Command::new("bash").arg(&script).arg(&runner).current_dir(&root).output().unwrap();
    let all = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "libs/relax/compiler/tests/modules/run.sh failed:\n{all}");
}

/// What rustc refuses in the app's own crates, a run refuses too and says why, validation-only
/// checks included: E0436 used to compile and run, and the run said nothing.
#[test]
fn a_run_refuses_what_rustc_refuses_in_the_apps_crates() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runner = std::env::var("MAKEPAD_RUN_LOADER").unwrap_or_else(|_| env!("CARGO_BIN_EXE_makepad").to_string());
    let scratch = std::env::temp_dir().join(format!("run-refuses-{}", std::process::id()));
    let output = Command::new(&runner)
        .args(["run", "run-refuses"])
        .current_dir(root.join("libs/relax/compiler/tests/run_refuses"))
        .env("MAKEPAD_RUN_STD", root.join("libs/relax/compiler/std"))
        .env("MAKEPAD_RUN_CACHE_DIR", scratch.join("cache"))
        .env("MAKEPAD_RUN_TARGET_DIR", scratch.join("target"))
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
    let (stdout, stderr) = (String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.status.code(), Some(101), "{stdout}\n{stderr}");
    assert!(!stdout.contains("started"), "it ran: {stdout}\n{stderr}");
    assert!(stderr.contains("E0436: functional record update syntax requires a struct"), "{stderr}");
}
