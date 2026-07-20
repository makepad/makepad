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
    app.locator(Selector::id("test_image"))
        .wait_visible()
        .scroll(0.0, -1200.0);
    app.locator(Selector::all().text_exact("Loading Spinner"))
        .wait_visible();
}

#[makepad_test]
fn splash_atlas_upload_bench(app: TestApp) {
    app.locator(Selector::id("atlas_upload_toggle"))
        .wait_visible()
        .click();
    app.locator(Selector::id("atlas_upload_bench_status"))
        .wait_text("variant B");
    app.locator(Selector::id("atlas_upload_bench_start"))
        .wait_visible()
        .click();
    app.wait_for_log_contains("ATLAS_BENCH SCHEDULER_PROXY");
}
