use makepad_test::{makepad_test, Selector, TestApp};

// A dev machine running this standalone (not under the Linux direct
// backend, and not hosted by the WM) reports no screens at all --
// `wm_api::screens()` is empty there -- so the status line's steady state
// is the "no screens reported" message.
#[makepad_test]
fn screen_span_reports_no_screens_standalone(app: TestApp) {
    app.locator(Selector::id("status_label"))
        .wait_text("no screens reported");
}
