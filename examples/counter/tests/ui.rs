use makepad_test::{makepad_test, Selector, TestApp};

#[makepad_test]
fn counter_smoke(app: TestApp) {
    app.locator(Selector::id("increment_button"))
        .wait_visible()
        .click();
    app.locator(Selector::id("counter_label"))
        .wait_text("Count: 1");
}

#[makepad_test]
fn counter_tracks_multiple_clicks(app: TestApp) {
    for _ in 0..3 {
        app.locator(Selector::id("increment_button")).click();
    }
    app.locator(Selector::id("counter_label"))
        .wait_text("Count: 3");
}
