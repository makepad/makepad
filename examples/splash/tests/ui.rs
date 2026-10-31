use makepad_test::{makepad_test, Selector, TestApp};

#[makepad_test]
fn splash_modal_smoke(app: TestApp) {
    app.locator(Selector::role(makepad_test::Role::Tab).text_exact("Modal"))
        .wait_visible()
        .select();
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
    app.locator(Selector::role(makepad_test::Role::Tab).text_exact("Toggles"))
        .wait_visible()
        .select();
    app.locator(Selector::id("checkbox"))
        .wait_visible()
        .check(true)
        .wait_checked(true);
    app.locator(Selector::id("toggle"))
        .wait_visible()
        .check(true)
        .wait_checked(true);

    // The Quick Select dropdown is the last thing on the Toggles page, below
    // the fold of the left panel. Scroll that page from a widget that is on it
    // (the dock's centre belongs to another panel).
    app.locator(Selector::id("toggle")).wheel(0.0, 600.0);
    app.locator(Selector::id("smoke_dropdown"))
        .wait_visible()
        .wait_text("Option A");
}

#[makepad_test]
fn splash_media_scroll_smoke(app: TestApp) {
    app.locator(Selector::role(makepad_test::Role::Tab).text_exact("Media"))
        .wait_visible()
        .select();
    // The media page is taller than the dock body, so `test_image` and the
    // spinner start below the fold with no on-screen rect to aim at. Scroll
    // the page from the dock itself — a point that is always on screen — and
    // let each `wait_visible` prove the content actually arrived.
    app.locator(Selector::id("dock")).wait_visible().wheel(0.0, 800.0);
    app.locator(Selector::id("test_image")).wait_visible();
    app.locator(Selector::id("dock")).wheel(0.0, 1400.0);
    app.locator(Selector::all().text_exact("Loading Spinner"))
        .wait_visible();
}
