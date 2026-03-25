use makepad_test::{makepad_test, run_with_config, Selector, TestApp, TestConfig, TestError};
use std::fs;

#[makepad_test]
fn fill_clear_and_submit_singleline_input(app: TestApp) {
    app.locator(Selector::id("input_singleline"))
        .wait_visible()
        .fill("hello")
        .wait_value("hello")
        .clear()
        .wait_value("")
        .fill("hello")
        .wait_value("hello");
    app.press_return();
    app.locator(Selector::id("status_label"))
        .wait_text("Returned from singleline: \"hello\"");
    app.wait_for_log_contains("Returned from singleline: \"hello\"");
}

#[makepad_test]
fn waits_for_visible_inputs(app: TestApp) {
    app.locator(Selector::id("input_email")).wait_visible();
}

#[makepad_test]
fn missing_selector_reports_useful_error(app: TestApp) -> Result<(), TestError> {
    let err = app
        .locator(Selector::id("input_missing"))
        .try_click()
        .unwrap_err();
    assert!(err.message().contains("matched no visible widgets"));
    Ok(())
}

#[makepad_test]
fn type_selector_reports_multiple_matches(app: TestApp) -> Result<(), TestError> {
    let err = app
        .locator(Selector::widget_type("TextInput"))
        .try_click()
        .unwrap_err();
    assert!(err.message().contains("matched multiple widgets"));
    assert!(err.message().contains("input_singleline"));
    Ok(())
}

#[test]
fn captures_failure_artifacts() {
    let config = TestConfig::current_package(
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_PKG_NAME"),
        "ui::captures_failure_artifacts",
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
