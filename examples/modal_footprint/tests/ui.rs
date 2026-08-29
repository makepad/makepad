//! Regression suite for the layout footprint of a `Modal`.
//!
//! A modal draws on its own overlay draw list, on a root turtle sized by the
//! pass. It is never laid out by the parent that holds it, so it must claim no
//! space in that parent — open or closed.
//!
//! Layout under test (`src/main.rs`): two identical `flow: Down` columns side
//! by side, each `header(40) / body(Fill) / footer(40)`. The right-hand column
//! additionally parks three `Modal`s between its body and its footer. The left
//! column is the control.
//!
//! Before the fix a `Modal` reported `Fill`/`Fill` upward — the size of the
//! overlay it paints inside its own pass, which is not a request a parent can
//! honour. A `Fill` child of a `flow: Down` parent is a *deferred fill*, and
//! the parent hands every deferred fill an equal share of the column's spare
//! height at resolve time whether or not the child then draws anything. Three
//! closed modals beside one real `Fill` body split that spare height four ways,
//! so `modal_body` measured a quarter of `plain_body` and the footer beneath it
//! floated in the middle of the column with a dead band underneath.
//!
//! (Measured on the VJ DJ page, which is where this was found: `page_body`
//! resolved to 214pt of an 874pt column, and its own `lists_column` — the
//! content explorer and the queue — was then laid out at -122pt and never drew.)

use makepad_test::{makepad_test, Selector, TestApp, WidgetSnapshot};

/// The one widget with this id that is actually drawn.
fn drawn(app: &TestApp, id: &str) -> WidgetSnapshot {
    app.widget_snapshot()
        .into_iter()
        .find(|w| w.id == id && w.width > 0 && w.height > 0)
        .unwrap_or_else(|| panic!("{id} is not drawn"))
}

/// The body of a column must take every point the header and footer leave, and
/// the footer must end where the column ends.
fn assert_column_is_packed(app: &TestApp, column: &str, header: &str, body: &str, footer: &str) {
    let column = drawn(app, column);
    let header = drawn(app, header);
    let body = drawn(app, body);
    let footer = drawn(app, footer);

    assert_eq!(
        body.y,
        header.y + header.height,
        "{} starts at {} but its header ends at {}",
        body.id,
        body.y,
        header.y + header.height,
    );
    assert_eq!(
        footer.y,
        body.y + body.height,
        "{} ends at {} but its footer starts at {} — the gap is height the \
         body was not given",
        body.id,
        body.y + body.height,
        footer.y,
    );
    assert_eq!(
        footer.y + footer.height,
        column.y + column.height,
        "{} ends at {} but its column ends at {}",
        footer.id,
        footer.y + footer.height,
        column.y + column.height,
    );
}

/// Closed modals parked in a column take nothing from the `Fill` beside them.
#[makepad_test]
fn closed_modals_take_no_height_from_a_fill_sibling(app: TestApp) {
    app.locator(Selector::id("open_button")).wait_visible();

    assert_column_is_packed(&app, "plain_column", "plain_header", "plain_body", "plain_footer");
    assert_column_is_packed(&app, "modal_column", "modal_header", "modal_body", "modal_footer");

    // The two columns are declared identically apart from the modals, so the
    // bodies must measure the same. With the modals counted as deferred fills
    // this was `plain / 4`.
    let plain = drawn(&app, "plain_body");
    let modal = drawn(&app, "modal_body");
    assert_eq!(
        plain.height, modal.height,
        "the column carrying three modals gave its body {}pt where the same \
         column without them gave {}pt",
        modal.height, plain.height,
    );
}

/// An open modal still takes nothing: it paints over the page rather than
/// inside the slot its parent would hand it.
#[makepad_test]
fn an_open_modal_takes_no_height_either(app: TestApp) {
    app.locator(Selector::id("open_button")).wait_visible();
    let closed = drawn(&app, "modal_body");

    app.locator(Selector::id("open_button")).click();
    app.locator(Selector::all().text_exact("DIALOG A")).wait_visible();

    assert_column_is_packed(&app, "modal_column", "modal_header", "modal_body", "modal_footer");
    let open = drawn(&app, "modal_body");
    assert_eq!(
        closed.height, open.height,
        "opening a modal moved the page under it: the body went from {}pt to {}pt",
        closed.height, open.height,
    );

    // And the page comes back unchanged when it closes.
    app.locator(Selector::id("close_a")).click();
    app.locator(Selector::all().text_exact("DIALOG A")).wait_count(0);
    let reclosed = drawn(&app, "modal_body");
    assert_eq!(closed.height, reclosed.height);
}

/// The dim backdrop covers the whole window, not the slot a parent thought it
/// was handing over. The modal is parked inside a half-width column below a
/// header, so a backdrop sized by that slot would leave most of the page lit.
#[makepad_test]
fn the_backdrop_covers_the_whole_window(app: TestApp) {
    app.locator(Selector::id("open_button")).wait_visible().click();
    app.locator(Selector::all().text_exact("DIALOG A")).wait_visible();

    let backdrop = drawn(&app, "bg_view");
    let column = drawn(&app, "modal_column");
    assert!(
        backdrop.x <= 0 && backdrop.y <= 0,
        "backdrop starts at ({}, {}) instead of the window origin",
        backdrop.x,
        backdrop.y,
    );
    assert!(
        backdrop.width > column.width && backdrop.height > column.height,
        "backdrop is {}x{}, no bigger than the {}x{} column that holds the \
         modal — it was sized by the parent's slot",
        backdrop.width,
        backdrop.height,
        column.width,
        column.height,
    );
}
