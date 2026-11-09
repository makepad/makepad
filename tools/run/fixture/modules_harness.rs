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
