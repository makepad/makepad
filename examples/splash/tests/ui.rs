use makepad_test::{makepad_test, Selector, TestApp};

#[makepad_test]
fn splash_modal_smoke(app: TestApp) {
    app.locator(Selector::widget_type("DockTab").text_exact("Modal"))
        .wait_visible()
        .click();
    app.locator(Selector::id("open_modal_btn"))
        .wait_visible()
        .click();
    app.locator(Selector::id("modal_status"))
        .wait_text("Modal status: Basic Modal Open");
    app.locator(Selector::id("close_modal_btn"))
        .wait_visible()
        .click();
    app.locator(Selector::id("modal_status"))
        .wait_text("Modal status: Closed via button");
}

#[makepad_test]
fn splash_toggle_and_dropdown_smoke(app: TestApp) {
    app.locator(Selector::widget_type("DockTab").text_exact("Toggles"))
        .wait_visible()
        .click();
    app.locator(Selector::id("checkbox"))
        .wait_visible()
        .click()
        .wait_checked(true);
    app.locator(Selector::id("toggle"))
        .wait_visible()
        .click()
        .wait_checked(true);

    app.locator(Selector::id("smoke_dropdown"))
        .wait_visible()
        .wait_text("Option A");
}

#[makepad_test]
fn splash_media_scroll_smoke(app: TestApp) {
    app.locator(Selector::widget_type("DockTab").text_exact("Media"))
        .wait_visible()
        .click();
    // The media page is taller than the dock body, so `test_image` and the
    // spinner start below the fold with no on-screen rect to aim at. Scroll
    // the page from the dock itself — a point that is always on screen — and
    // let each `wait_visible` prove the content actually arrived.
    app.locator(Selector::id("dock")).wait_visible().scroll(0.0, 800.0);
    app.locator(Selector::id("test_image")).wait_visible();
    app.locator(Selector::id("dock")).scroll(0.0, 1400.0);
    app.locator(Selector::all().text_exact("Loading Spinner"))
        .wait_visible();
}
