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
//!
//! The last test in the file pins the *other* half of "a modal covers the
//! page" — depth, not layout. The page carries a `deep_band` at
//! `draw_depth: 12`, and an overlay only outranks it because
//! `DrawList2d::begin_overlay_inner` gives every overlay draw list a depth
//! floor. Without it the band paints over the card and over the backdrop.

use makepad_test::{makepad_test, Selector, TestApp, WidgetSnapshot};
use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
use makepad_zune_png::PngDecoder;

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

// ---------------------------------------------------------------------------
// The other half of "a modal covers the page": depth.
// ---------------------------------------------------------------------------

/// The page's `deep_band` colour, `#xcc22cc` (see `src/main.rs`).
const BAND: [u8; 3] = [0xcc, 0x22, 0xcc];

struct Grab {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    /// Device pixels per layout point, so the widget rects the harness reports
    /// can be turned into pixel coordinates on any dpi.
    scale: f64,
}

impl Grab {
    fn take(app: &TestApp) -> Grab {
        let window = drawn(app, "main_window");
        let path = app.screenshot();
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|err| panic!("cannot read grab {}: {err}", path.display()));
        let mut decoder = PngDecoder::new(ZCursor::new(&bytes));
        let pixels = decoder
            .decode_raw()
            .unwrap_or_else(|err| panic!("cannot decode grab {}: {err:?}", path.display()));
        let (width, height) = decoder.dimensions().expect("grab has no dimensions");
        let components = decoder
            .colorspace()
            .expect("grab has no colorspace")
            .num_components();
        assert!(components >= 3, "grab is not a colour image");
        let mut rgba = vec![0u8; width * height * 4];
        for i in 0..width * height {
            let src = i * components;
            rgba[i * 4] = pixels[src];
            rgba[i * 4 + 1] = pixels[src + 1];
            rgba[i * 4 + 2] = pixels[src + 2];
            rgba[i * 4 + 3] = if components == 4 { pixels[src + 3] } else { 255 };
        }
        Grab {
            width,
            height,
            rgba,
            scale: width as f64 / window.width.max(1) as f64,
        }
    }

    /// The pixel under a point given in layout points.
    fn at(&self, x: i64, y: i64) -> [u8; 3] {
        let px = ((x as f64 * self.scale) as usize).min(self.width - 1);
        let py = ((y as f64 * self.scale) as usize).min(self.height - 1);
        let p = (py * self.width + px) * 4;
        [self.rgba[p], self.rgba[p + 1], self.rgba[p + 2]]
    }
}

fn near(a: [u8; 3], b: [u8; 3], tolerance: i32) -> bool {
    (0..3).all(|c| (a[c] as i32 - b[c] as i32).abs() <= tolerance)
}

/// The three points this test reads, in layout points: the middle of the band
/// (which the dialog card covers) and both ends of it (which it cannot reach).
fn band_probes(app: &TestApp) -> (i64, i64, i64, i64) {
    let band = drawn(app, "deep_band");
    let y = band.y + band.height / 2;
    let middle = band.x + band.width / 2;
    let left = band.x + 12;
    let right = band.x + band.width - 12;
    (left, middle, right, y)
}

/// An open modal covers a page that spends `draw_depth`.
///
/// A 2D pass has ONE depth buffer, shared by every draw list in it, and a
/// vertex lands at `world.z = draw_depth + draw_call.zbias`. Drawing the modal
/// later than the page buys it a single `zbias_step` — 0.001 — of z, so the
/// `deep_band` at `draw_depth: 12` outranks it and the depth test throws the
/// dialog away: measured, before the fix, the band painted straight over the
/// card AND over the dim backdrop, exactly as an engraved score did.
///
/// `DrawList2d::begin_overlay_inner` gives every overlay draw list a depth
/// floor clear of that band. Nothing in this app asks for it.
#[makepad_test]
fn an_open_modal_covers_a_page_that_uses_draw_depth(app: TestApp) {
    app.locator(Selector::id("open_button")).wait_visible();
    let (left, middle, right, y) = band_probes(&app);

    // Closed: the band is the page, and it is the band's own colour.
    let closed = Grab::take(&app);
    for x in [left, middle, right] {
        assert!(
            near(closed.at(x, y), BAND, 8),
            "the deep band is not drawn at ({x}, {y}): {:?}",
            closed.at(x, y),
        );
    }

    app.locator(Selector::id("open_button")).click();
    app.locator(Selector::all().text_exact("DIALOG A")).wait_visible();
    let open = Grab::take(&app);

    // The card covers the middle of the band outright.
    let covered = open.at(middle, y);
    assert!(
        !near(covered, BAND, 40),
        "the page punched through the dialog: ({middle}, {y}) is still the \
         band's colour {covered:?}. The overlay draw list lost the depth test \
         against draw_depth 12 — see DrawList2d::begin_overlay_inner",
    );

    // And the dim backdrop — also overlay content, also at depth 0 — reaches
    // the parts of the band the card does not cover. Checking only the card's
    // own region is how this bug stayed hidden the first time.
    for x in [left, right] {
        let dimmed = open.at(x, y);
        assert!(
            !near(dimmed, BAND, 40),
            "the backdrop did not dim the deep band at ({x}, {y}): {dimmed:?}",
        );
        let before = closed.at(x, y);
        assert!(
            (0..3).all(|c| dimmed[c] <= before[c]),
            "the deep band at ({x}, {y}) went from {before:?} to {dimmed:?} — \
             the backdrop should only darken it",
        );
    }
}
