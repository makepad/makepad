use makepad_test::{makepad_test, Selector, TestApp};

#[makepad_test]
fn map_renders_smoke(app: TestApp) {
    app.locator(Selector::id("status_label")).wait_visible();
    // Tiles + detail merge need a moment; the screenshot is the artifact.
    std::thread::sleep(std::time::Duration::from_secs(14));
    let path = app.screenshot();
    eprintln!("MAP_SCREENSHOT: {}", path.display());
}
