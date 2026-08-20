//! Regression suite for PortalList row geometry.
//!
//! `PortalList` deliberately draws recycled rows past both edges of its
//! viewport and relies on the clip rect to hide them. The widget tree must
//! describe those rows the way a viewer sees them — clipped — or every
//! consumer that turns a reported rect into a click lands somewhere else.
//!
//! Layout under test (`src/main.rs`): a 200px-tall list of 40px rows with a
//! full-width `outside_button` parked directly beneath it. Rows 0..=4 fill the
//! viewport; row 5 is drawn in the band `outside_button` occupies and clipped
//! away. Before the fix, "Play 5" was reported as a visible button at
//! [365, 269, 49, 22] — whose centre lands on `outside_button` — so clicking
//! the row the tree pointed at pressed Outside instead.

use makepad_test::{makepad_test, Selector, TestApp, WidgetSnapshot};

/// Every row button the tree reports must lie inside the list's own viewport.
/// This is the invariant the bug broke: a row drawn past the bottom edge was
/// reported at full size in the band below the list, on top of a different
/// widget that really is drawn there.
fn assert_rows_inside_list(app: &TestApp) {
    let widgets = app.widget_snapshot();
    let list = widgets
        .iter()
        .find(|w| w.id == "list" && w.width > 0 && w.height > 0)
        .cloned()
        .expect("list is drawn");
    let rows: Vec<&WidgetSnapshot> = widgets
        .iter()
        .filter(|w| w.id == "play_button" && w.visible && w.width > 0 && w.height > 0)
        .collect();
    assert!(!rows.is_empty(), "no visible rows reported");
    for row in rows {
        assert!(
            row.y >= list.y && row.y + row.height <= list.y + list.height,
            "row {:?} reported at y {}..{}, outside the list viewport y {}..{}",
            row.text,
            row.y,
            row.y + row.height,
            list.y,
            list.y + list.height,
        );
    }
}

#[makepad_test]
fn portallist_hides_rows_clipped_past_the_viewport(app: TestApp) {
    app.locator(Selector::all().text_exact("Play 0")).wait_visible();
    app.locator(Selector::all().text_exact("Play 4")).wait_visible();
    app.locator(Selector::id("outside_button")).wait_visible();

    // Row 5 is drawn under `outside_button` and clipped away: it must not be
    // reported as a visible widget sitting on top of the button.
    app.locator(Selector::all().text_exact("Play 5")).wait_count(0);
    app.locator(Selector::all().text_exact("Row 5")).wait_count(0);

    assert_rows_inside_list(&app);
}

#[makepad_test]
fn portallist_rows_click_where_the_tree_reports_them(app: TestApp) {
    app.locator(Selector::all().text_exact("Play 0"))
        .wait_visible()
        .click();
    app.locator(Selector::id("last_played")).wait_text("Row 0");

    // The last row inside the viewport: its reported rect must be the visible
    // one, not the unclipped rect whose centre sits below the list.
    app.locator(Selector::all().text_exact("Play 4"))
        .wait_visible()
        .click();
    app.locator(Selector::id("last_played")).wait_text("Row 4");

    app.locator(Selector::id("outside_button")).click();
    app.locator(Selector::id("last_played")).wait_text("outside");
}

#[makepad_test]
fn portallist_rows_stay_addressable_after_scrolling(app: TestApp) {
    app.locator(Selector::all().text_exact("Play 0")).wait_visible();
    app.locator(Selector::id("list")).scroll(0.0, 160.0);

    // After a scroll the list keeps over-drawing rows past both edges. Every
    // row the tree still reports must be the row a click there actually hits.
    app.locator(Selector::all().text_exact("Play 6"))
        .wait_visible()
        .click();
    app.locator(Selector::id("last_played")).wait_text("Row 6");

    assert_rows_inside_list(&app);

    app.locator(Selector::id("outside_button")).click();
    app.locator(Selector::id("last_played")).wait_text("outside");
}
