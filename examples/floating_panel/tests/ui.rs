use makepad_test::{makepad_test, Selector, TestApp};

#[makepad_test]
fn panel_input_updates_main_window_status(app: TestApp) {
    app.locator(Selector::id("panel_input").window("panel_window"))
        .wait_visible()
        .fill("hello")
        .wait_value("hello");
    app.locator(Selector::id("status_label"))
        .wait_text("Panel typing: \"hello\"");
    app.press_return();
    app.locator(Selector::id("status_label"))
        .wait_text("Panel submitted: \"hello\"");
    app.locator(Selector::id("ping_button").window("panel_window"))
        .click();
    app.locator(Selector::id("status_label"))
        .wait_text("Panel button pressed 1 time.");
}

#[makepad_test]
fn floating_panel_can_be_dragged(app: TestApp) {
    app.locator(Selector::id("drag_target").window("panel_window"))
        .wait_visible()
        .drag_by(140.0, 40.0);
    app.locator(Selector::id("drag_value").window("panel_window"))
        .wait_text("Drag delta: 140, 40");
}
