//! Ensures `run_with_config` writes the expected failure artifacts for a failing test body.
//! Uses a small example app as the current package (this crate is a library only).

use makepad_test::{run_with_config, TestConfig, TestError};
use std::fs;
use std::path::PathBuf;

fn example_counter_manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/counter")
        .canonicalize()
        .expect("examples/counter relative to makepad_test crate")
}

#[test]
fn captures_failure_artifacts() {
    let manifest_dir = example_counter_manifest_dir();
    let config = TestConfig::current_package(
        &manifest_dir,
        "makepad-example-counter",
        "artifact_capture::captures_failure_artifacts",
    )
    .unwrap();
    let artifact_dir = config.artifacts_dir.clone();
    let _ = fs::remove_dir_all(&artifact_dir);

    let err = run_with_config(config, |_app| -> Result<(), TestError> {
        Err(TestError::new("intentional failure for artifact capture"))
    })
    .unwrap_err();

    assert!(
        err.message().contains("intentional failure"),
        "{}",
        err.message()
    );
    assert!(artifact_dir.join("failure.txt").exists());
    assert!(artifact_dir.join("logs.txt").exists());
    assert!(artifact_dir.join("widget-tree.txt").exists());
    assert!(artifact_dir.join("widget-snapshot.json").exists());
    assert!(artifact_dir.join("failure-screenshot.png").exists());
}
