//! Scroll containment: a scroll over a nested scrollable moves only that one, even at its limits,
//! unless it has nothing to scroll, and a gesture stays with the scrollable it began on.

use makepad_test::{makepad_test, KeyCode, RemoteMouseMove, Selector, StudioToApp, TestApp};
use std::thread;
use std::time::{Duration, Instant};

/// Longer than the wheel latch timeout, so the next wheel event starts a new scroll.
const NEW_SCROLL: Duration = Duration::from_millis(700);

/// The status line once the offsets stop changing (flings and bounces settle).
fn settled(app: &TestApp) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = String::new();
    loop {
        let text = app
            .locator(Selector::id("status"))
            .snapshot()
            .text
            .unwrap_or_default();
        if text == last || Instant::now() > deadline {
            return text;
        }
        last = text;
        thread::sleep(Duration::from_millis(100));
    }
}

fn offset(status: &str, key: &str) -> f64 {
    status
        .split_whitespace()
        .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("no `{key}` in status `{status}`"))
}

/// Settled offsets for `keys`.
fn offsets<const N: usize>(app: &TestApp, keys: [&str; N]) -> [f64; N] {
    let status = settled(app);
    keys.map(|key| offset(&status, key))
}

/// Waits for `key` to reach `value`, which an animation that doesn't move any offset can take a while to do.
fn wait_for(app: &TestApp, key: &str, value: f64) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let [now] = offsets(app, [key]);
        if now == value || Instant::now() > deadline {
            assert_eq!(now, value, "`{key}`");
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// The middle of a widget, found by id or by its text.
fn center(app: &TestApp, selector: Selector) -> (f64, f64) {
    let widget = app.locator(selector).snapshot();
    (
        widget.x as f64 + widget.width as f64 / 2.0,
        widget.y as f64 + widget.height as f64 / 2.0,
    )
}

fn wheel(app: &TestApp, (x, y): (f64, f64), dx: f64, dy: f64) {
    app.scroll_at(x, y, dx, dy, None);
}

/// One trackpad gesture at a point: `deltas` while the fingers move, then `momentum`.
fn trackpad(app: &TestApp, (x, y): (f64, f64), deltas: &[f64], momentum: &[f64]) {
    app.scroll_at(x, y, 0.0, 0.0, Some("began"));
    for &dy in deltas {
        app.scroll_at(x, y, 0.0, dy, Some("changed"));
    }
    app.scroll_at(x, y, 0.0, 0.0, Some("ended"));
    for &dy in momentum {
        app.scroll_at(x, y, 0.0, dy, Some("momentum"));
    }
    app.scroll_at(x, y, 0.0, 0.0, Some("momentum_ended"));
}

#[makepad_test]
fn wheel_over_a_nested_list_never_scrolls_the_page(app: TestApp) {
    let [page, log] = offsets(&app, ["page", "log"]);
    let over_log = center(&app, Selector::id("log_list"));
    wheel(&app, over_log, 0.0, 60.0);
    assert_eq!(offsets(&app, ["page", "log"]), [page, log + 60.0]);

    // Past the end: the list stops there and the page still doesn't move.
    wheel(&app, over_log, 0.0, 5000.0);
    let [_, log_end] = offsets(&app, ["page", "log"]);
    assert_eq!(offsets(&app, ["page"]), [page]);
    thread::sleep(NEW_SCROLL);
    wheel(&app, over_log, 0.0, 200.0);
    assert_eq!(offsets(&app, ["page", "log"]), [page, log_end]);
}

#[makepad_test]
fn a_nested_list_with_nothing_to_scroll_leaves_the_wheel_to_the_page(app: TestApp) {
    let [page, short] = offsets(&app, ["page", "short"]);
    wheel(&app, center(&app, Selector::id("short_list")), 0.0, 50.0);
    assert_eq!(offsets(&app, ["page", "short"]), [page + 50.0, short]);
}

#[makepad_test]
fn a_list_with_nothing_to_scroll_still_takes_scrolls_when_nothing_around_it_can(app: TestApp) {
    // So it still counts the travel (which apps use to load more) and rubber-bands.
    let over_fit = center(&app, Selector::id("fit_list"));
    for _ in 0..3 {
        thread::sleep(NEW_SCROLL);
        wheel(&app, over_fit, 0.0, -40.0);
    }
    assert_eq!(offsets(&app, ["fit", "fit_travel"]), [0.0, 120.0]);

    let (x, y) = over_fit;
    app.scroll_at(x, y, 0.0, 0.0, Some("began"));
    app.scroll_at(x, y, 0.0, -30.0, Some("changed"));
    app.scroll_at(x, y, 0.0, -30.0, Some("changed"));
    let [stretched] = offsets(&app, ["fit"]);
    assert!(stretched < -1.0, "the list stretched past its start ({stretched})");
    app.scroll_at(x, y, 0.0, 0.0, Some("ended"));
    assert_eq!(offsets(&app, ["fit"]), [0.0]);
}

#[makepad_test]
fn wheel_over_a_nested_scroll_view_or_text_input_never_scrolls_the_page(app: TestApp) {
    let [page, inner] = offsets(&app, ["page", "inner"]);
    let over_inner = center(&app, Selector::id("inner_view"));
    wheel(&app, over_inner, 0.0, 1000.0);
    let [_, inner_end] = offsets(&app, ["page", "inner"]);
    assert!(inner_end > inner, "the nested view scrolled");
    thread::sleep(NEW_SCROLL);
    wheel(&app, over_inner, 0.0, 100.0);
    assert_eq!(offsets(&app, ["page", "inner"]), [page, inner_end]);

    let over_notes = center(&app, Selector::id("notes"));
    thread::sleep(NEW_SCROLL);
    wheel(&app, over_notes, 0.0, 2000.0);
    thread::sleep(NEW_SCROLL);
    wheel(&app, over_notes, 0.0, 100.0);
    assert_eq!(offsets(&app, ["page"]), [page]);
}

#[makepad_test]
fn a_horizontal_scroller_leaves_vertical_wheels_to_the_page(app: TestApp) {
    wheel(&app, center(&app, Selector::id("page_top")), 0.0, 300.0);
    let [page, carousel] = offsets(&app, ["page", "carousel"]);
    assert!(page > 250.0, "the page scrolled the carousel into view");

    thread::sleep(NEW_SCROLL);
    let over_carousel = center(&app, Selector::id("carousel"));
    wheel(&app, over_carousel, 0.0, 40.0);
    assert_eq!(offsets(&app, ["page", "carousel"]), [page + 40.0, carousel]);

    thread::sleep(NEW_SCROLL);
    let over_carousel = center(&app, Selector::id("carousel"));
    wheel(&app, over_carousel, 100.0, 0.0);
    assert_eq!(offsets(&app, ["page", "carousel"]), [page + 40.0, carousel + 100.0]);

    // A multiline input whose text fits has nothing to scroll either.
    thread::sleep(NEW_SCROLL);
    wheel(&app, center(&app, Selector::id("short_notes")), 0.0, 40.0);
    assert_eq!(offsets(&app, ["page"]), [page + 80.0]);
}

#[makepad_test]
fn a_wheel_scroll_stays_with_the_scrollable_it_began_on(app: TestApp) {
    let [page, log] = offsets(&app, ["page", "log"]);
    // The first notch scrolls the list up under the pointer; the rest still scroll the page,
    // also while the hand drifts.
    let (x, y) = center(&app, Selector::id("page_top"));
    for notch in 0..6 {
        wheel(&app, (x, y + 4.0 * notch as f64), 0.0, 20.0);
    }
    assert_eq!(offsets(&app, ["page", "log"]), [page + 120.0, log]);

    // After a pause, the wheel moves whatever is under the pointer now.
    thread::sleep(NEW_SCROLL);
    wheel(&app, (x, y), 0.0, 20.0);
    assert_eq!(offsets(&app, ["page", "log"]), [page + 120.0, log + 20.0]);
}

#[makepad_test]
fn moving_the_pointer_off_a_scrollable_once_the_wheel_rests_lets_the_next_notch_go_elsewhere(app: TestApp) {
    let [page, log] = offsets(&app, ["page", "log"]);
    wheel(&app, center(&app, Selector::id("log_list")), 0.0, 40.0);
    // Well within the wheel's 500 ms scroll, but after a rest.
    thread::sleep(Duration::from_millis(200));
    let (x, y) = center(&app, Selector::id("page_top"));
    app.forward(vec![StudioToApp::MouseMove(RemoteMouseMove { x, y, ..Default::default() })]);
    wheel(&app, (x, y), 0.0, 20.0);
    assert_eq!(offsets(&app, ["page", "log"]), [page + 20.0, log + 40.0]);
}

#[makepad_test]
fn a_trackpad_gesture_stays_with_the_scrollable_it_began_on(app: TestApp) {
    let [page, log] = offsets(&app, ["page", "log"]);
    let over_page = center(&app, Selector::id("page_top"));
    trackpad(&app, over_page, &[20.0; 6], &[10.0; 3]);
    assert_eq!(offsets(&app, ["page", "log"]), [page + 150.0, log]);

    // The next gesture goes to the list now under the pointer.
    trackpad(&app, over_page, &[20.0; 2], &[]);
    assert_eq!(offsets(&app, ["page", "log"]), [page + 150.0, log + 40.0]);
}

#[makepad_test]
fn a_trackpad_gesture_at_a_nested_lists_limit_never_reaches_the_page(app: TestApp) {
    let [page] = offsets(&app, ["page"]);
    let over_log = center(&app, Selector::id("log_list"));
    wheel(&app, over_log, 0.0, 5000.0);
    let [_, log_end] = offsets(&app, ["page", "log"]);
    thread::sleep(NEW_SCROLL);
    // Its rubber band stretches and springs back, and its momentum goes nowhere.
    trackpad(&app, over_log, &[40.0; 3], &[30.0; 3]);
    assert_eq!(offsets(&app, ["page", "log"]), [page, log_end]);
}

#[makepad_test]
fn a_list_nested_in_a_list_keeps_its_wheel_scrolls(app: TestApp) {
    let [outer, nested] = offsets(&app, ["outer", "nested"]);
    let over_nested = center(&app, Selector::id("nested"));
    wheel(&app, over_nested, 0.0, 3000.0);
    let [_, nested_end] = offsets(&app, ["outer", "nested"]);
    assert!(nested_end > nested, "the nested list scrolled");
    thread::sleep(NEW_SCROLL);
    wheel(&app, over_nested, 0.0, 100.0);
    assert_eq!(offsets(&app, ["outer", "nested"]), [outer, nested_end]);

    // One with nothing to scroll leaves the wheel to the outer list.
    thread::sleep(NEW_SCROLL);
    wheel(&app, center(&app, Selector::id("nested_short")), 0.0, 30.0);
    assert_eq!(offsets(&app, ["outer", "nested_short"]), [outer + 30.0, 0.0]);
}

/// Scrolls the outer list until its far strip sits near the bottom, with a code block above it,
/// and returns the strip's middle.
fn far_strip_near_bottom(app: &TestApp) -> (f64, f64) {
    let list = app.locator(Selector::id("lists")).snapshot();
    // 1000 down the list, so this puts it 600 below the list's top.
    wheel(app, center(app, Selector::all().text_exact("Outer 10")), 0.0, 400.0);
    thread::sleep(NEW_SCROLL);
    let strip = app.locator(Selector::id("far_strip")).snapshot();
    assert!(strip.y + strip.height < list.y + list.height, "the far strip is in view");
    center(app, Selector::id("far_strip"))
}

/// The gesture starts on the far strip, which takes x while the list takes y. The strip scrolls
/// out of view and the code block moves under the pointer, but the list keeps scrolling.
fn check_a_gesture_keeps_its_list_when_the_strip_scrolls_away(app: &TestApp, is_trackpad: bool) {
    let over_strip = far_strip_near_bottom(app);
    let [outer, code] = offsets(app, ["outer", "code"]);
    if is_trackpad {
        trackpad(app, over_strip, &[-30.0; 6], &[]);
    } else {
        for _ in 0..6 {
            wheel(app, over_strip, 0.0, -30.0);
        }
    }
    assert_eq!(offsets(app, ["outer", "code"]), [outer - 180.0, code]);
}

#[makepad_test]
fn a_trackpad_gesture_keeps_its_list_when_the_strip_holding_its_other_axis_scrolls_away(app: TestApp) {
    check_a_gesture_keeps_its_list_when_the_strip_scrolls_away(&app, true);
}

#[makepad_test]
fn a_wheel_scroll_keeps_its_list_when_the_strip_holding_its_other_axis_scrolls_away(app: TestApp) {
    check_a_gesture_keeps_its_list_when_the_strip_scrolls_away(&app, false);
}

#[makepad_test]
fn dragging_a_list_nested_in_a_list_never_drags_the_outer_one(app: TestApp) {
    let [outer, nested] = offsets(&app, ["outer", "nested"]);
    app.locator(Selector::id("nested")).drag_by(0.0, -80.0);
    let [outer_after, nested_after] = offsets(&app, ["outer", "nested"]);
    assert!(nested_after >= nested + 80.0, "the nested list followed the drag");
    assert_eq!(outer_after, outer);

    // One with nothing to scroll lets the outer list take the drag.
    app.locator(Selector::id("nested_short")).drag_by(0.0, -60.0);
    let [outer_after, nested_short] = offsets(&app, ["outer", "nested_short"]);
    assert!(outer_after >= outer + 60.0, "the outer list followed the drag");
    assert_eq!(nested_short, 0.0);
}

#[makepad_test]
fn dragging_a_horizontal_strip_in_a_list_scrolls_each_its_own_way(app: TestApp) {
    let [outer, strip] = offsets(&app, ["outer", "strip"]);
    app.locator(Selector::id("strip")).drag_by(-100.0, 0.0);
    let [outer_after, strip_after] = offsets(&app, ["outer", "strip"]);
    assert!(strip_after >= strip + 100.0, "the strip followed the drag");
    assert_eq!(outer_after, outer);

    // Held, so the list doesn't fling the strip's row out of view.
    app.locator(Selector::id("strip")).drag_by_and_hold(0.0, -60.0);
    let [outer_after, strip_after_vertical] = offsets(&app, ["outer", "strip"]);
    assert!(outer_after >= outer + 60.0, "the outer list followed the drag");
    assert_eq!(strip_after_vertical, strip_after);
}

#[makepad_test]
fn touch_dragging_a_nested_scrollable_never_drags_the_page(app: TestApp) {
    let [page, log, inner] = offsets(&app, ["page", "log", "inner"]);
    app.locator(Selector::id("log_list")).touch_drag_by(0.0, -80.0);
    let [page_after, log_after] = offsets(&app, ["page", "log"]);
    assert!(log_after >= log + 80.0, "the list followed the finger");
    assert_eq!(page_after, page);

    app.locator(Selector::id("inner_view")).touch_drag_by(0.0, -60.0);
    let [page_after, inner_after] = offsets(&app, ["page", "inner"]);
    assert!(inner_after >= inner + 60.0, "the nested view followed the finger");
    assert_eq!(page_after, page);
}

#[makepad_test]
fn touch_dragging_a_list_nested_in_a_list_never_drags_the_outer_one(app: TestApp) {
    let [outer, nested] = offsets(&app, ["outer", "nested"]);
    app.locator(Selector::id("nested")).touch_drag_by(0.0, -80.0);
    let [outer_after, nested_after] = offsets(&app, ["outer", "nested"]);
    assert!(nested_after >= nested + 80.0, "the nested list followed the finger");
    assert_eq!(outer_after, outer);

    // One with nothing to scroll lets the outer list take the drag.
    app.locator(Selector::id("nested_short")).touch_drag_by(0.0, -60.0);
    let [outer_after, nested_short] = offsets(&app, ["outer", "nested_short"]);
    assert!(outer_after >= outer + 60.0, "the outer list followed the finger");
    assert_eq!(nested_short, 0.0);
}

#[makepad_test]
fn touch_dragging_a_horizontal_strip_in_a_list_scrolls_each_its_own_way(app: TestApp) {
    let [outer, strip] = offsets(&app, ["outer", "strip"]);
    app.locator(Selector::id("strip")).touch_drag_by(-100.0, 0.0);
    let [outer_after, strip_after] = offsets(&app, ["outer", "strip"]);
    assert!(strip_after >= strip + 100.0, "the strip followed the finger");
    assert_eq!(outer_after, outer);

    app.locator(Selector::id("strip")).touch_drag_by_and_hold(0.0, -60.0);
    let [outer_after, strip_after_vertical] = offsets(&app, ["outer", "strip"]);
    assert!(outer_after > outer, "the outer list followed the finger");
    assert_eq!(strip_after_vertical, strip_after);
}

#[makepad_test]
fn an_open_dropdown_keeps_scrolls_from_what_is_under_it(app: TestApp) {
    let [under, page] = offsets(&app, ["under", "page"]);
    app.locator(Selector::id("dd")).click();
    thread::sleep(Duration::from_millis(300));
    // Over the open list where it covers the view after it, beside the list, and on the page.
    let under_popup = app.locator(Selector::id("under_popup")).snapshot();
    let (x, y) = center(&app, Selector::id("under_popup"));
    wheel(&app, (under_popup.x as f64 + 20.0, y), 0.0, 60.0);
    thread::sleep(NEW_SCROLL);
    wheel(&app, (x, y), 0.0, 60.0);
    thread::sleep(NEW_SCROLL);
    wheel(&app, center(&app, Selector::id("page_top")), 0.0, 60.0);
    assert_eq!(offsets(&app, ["under", "page"]), [under, page]);

    app.press_key(KeyCode::Escape);
    thread::sleep(NEW_SCROLL);
    wheel(&app, center(&app, Selector::id("under_popup")), 0.0, 60.0);
    assert_eq!(offsets(&app, ["under"]), [under + 60.0]);
}

#[makepad_test]
fn swiping_a_scroller_in_a_stack_view_scrolls_it_instead_of_going_back(app: TestApp) {
    let [start] = offsets(&app, ["swipe"]);
    wheel(&app, center(&app, Selector::id("swipe_strip")), 300.0, 0.0);
    let [swipe, depth] = offsets(&app, ["swipe", "stack_depth"]);
    assert_eq!((swipe, depth), (start + 300.0, 1.0));
    app.locator(Selector::id("swipe_strip")).touch_drag_by_and_hold(80.0, 0.0);
    let [swipe_after, depth_after] = offsets(&app, ["swipe", "stack_depth"]);
    assert!(swipe_after <= swipe - 70.0, "the strip followed the finger ({swipe_after})");
    assert_eq!(depth_after, 1.0, "the view stayed");

    // A swipe on something that doesn't scroll sideways still goes back.
    app.locator(Selector::all().text_exact("BelowSwipeStrip")).touch_drag_by(120.0, 0.0);
    wait_for(&app, "stack_depth", 0.0);
}

#[makepad_test]
fn dragging_a_list_in_a_sliding_panel_never_slides_the_panel(app: TestApp) {
    let [panel, list] = offsets(&app, ["panel", "panel_list"]);
    app.locator(Selector::id("panel_list")).touch_drag_by_and_hold(0.0, -100.0);
    let [panel_after, list_after] = offsets(&app, ["panel", "panel_list"]);
    assert!(list_after >= list + 90.0, "the list followed the finger ({list_after})");
    assert_eq!(panel_after, panel);
}

#[makepad_test]
fn a_list_partly_scrolled_out_of_a_modal_still_takes_its_scrolls(app: TestApp) {
    app.locator(Selector::id("open_modal")).click();
    thread::sleep(Duration::from_millis(300));
    // Scroll the modal until the top of the list in it, 150 down, is hidden.
    let modal_page = app.locator(Selector::id("modal_page")).snapshot();
    wheel(&app, center(&app, Selector::id("modal_top")), 0.0, 500.0);
    let [modal, list] = offsets(&app, ["modal", "modal_list"]);
    assert!(modal > 160.0, "the list's top is scrolled out of the modal ({modal})");

    // Over the visible rest of the list.
    thread::sleep(NEW_SCROLL);
    let visible_list = 350.0 - modal;
    let over_list = (modal_page.x as f64 + 40.0, modal_page.y as f64 + visible_list / 2.0);
    wheel(&app, over_list, 0.0, 30.0);
    assert_eq!(offsets(&app, ["modal", "modal_list"]), [modal, list + 30.0]);
}

/// Scrolls the page so the carousel and the view that fits below it are in view.
fn reveal_carousel(app: &TestApp) -> f64 {
    wheel(app, center(app, Selector::id("page_top")), 0.0, 300.0);
    thread::sleep(NEW_SCROLL);
    let [page] = offsets(app, ["page"]);
    assert!(page > 250.0, "the page scrolled the carousel into view");
    page
}

#[makepad_test]
fn touch_dragging_a_horizontal_strip_in_a_page_scrolls_each_its_own_way(app: TestApp) {
    let page = reveal_carousel(&app);
    let [carousel] = offsets(&app, ["carousel"]);
    app.locator(Selector::id("carousel")).touch_drag_by_and_hold(-100.0, -10.0);
    let [page_after, carousel_after] = offsets(&app, ["page", "carousel"]);
    assert!(carousel_after > carousel + 80.0, "the strip followed the finger ({carousel} -> {carousel_after})");
    assert_eq!(page_after, page);

    app.locator(Selector::id("carousel")).touch_drag_by_and_hold(10.0, -60.0);
    let [page_after, carousel_after_vertical] = offsets(&app, ["page", "carousel"]);
    assert!(page_after > page + 40.0, "the page followed the finger ({page} -> {page_after})");
    assert_eq!(carousel_after_vertical, carousel_after);

    // A drag that starts out mostly sideways moves only the strip.
    app.locator(Selector::id("carousel")).touch_drag_by_and_hold(-60.0, -20.0);
    let [page_after_diagonal, carousel_after_diagonal] = offsets(&app, ["page", "carousel"]);
    assert_eq!(page_after_diagonal, page_after);
    assert!(carousel_after_diagonal > carousel_after_vertical + 40.0);
}

#[makepad_test]
fn mouse_dragging_a_horizontal_strip_in_a_page_scrolls_each_its_own_way(app: TestApp) {
    let page = reveal_carousel(&app);
    app.locator(Selector::id("carousel")).drag_by_and_hold(0.0, -60.0);
    let [page_after] = offsets(&app, ["page"]);
    assert!(page_after > page + 40.0, "the page followed the mouse ({page} -> {page_after})");
}

#[makepad_test]
fn touch_dragging_a_nested_scroll_view_with_nothing_to_scroll_drags_the_page(app: TestApp) {
    let page = reveal_carousel(&app);
    app.locator(Selector::id("fit_view")).touch_drag_by_and_hold(0.0, -60.0);
    let [page_after] = offsets(&app, ["page"]);
    assert!(page_after > page + 40.0, "the page followed the finger ({page} -> {page_after})");
}

#[makepad_test]
fn touch_dragging_a_multiline_input_never_drags_the_page(app: TestApp) {
    let page = reveal_carousel(&app);
    app.locator(Selector::id("short_notes")).touch_drag_by_and_hold(0.0, -60.0);
    assert_eq!(offsets(&app, ["page"]), [page]);
}

fn open_code_modal(app: &TestApp) {
    app.locator(Selector::id("open_code_modal")).click();
    app.locator(Selector::id("code_view")).wait_visible();
    thread::sleep(Duration::from_millis(300));
}

#[makepad_test]
fn touch_dragging_wide_code_scrolls_it_sideways_and_its_page_down(app: TestApp) {
    open_code_modal(&app);
    let [page, x] = offsets(&app, ["code_page", "code_x"]);
    // A little off each axis, so only the axis each drag mostly went along moves.
    app.locator(Selector::id("code_view")).touch_drag_by_and_hold(-120.0, -12.0);
    let [page_after, x_after] = offsets(&app, ["code_page", "code_x"]);
    assert!(x_after >= x + 110.0, "the code followed the finger sideways ({x_after})");
    assert_eq!(page_after, page);

    app.locator(Selector::id("code_view")).touch_drag_by_and_hold(12.0, -80.0);
    let [page_after, x_after_vertical] = offsets(&app, ["code_page", "code_x"]);
    assert!(page_after >= page + 70.0, "the page followed the finger ({page_after})");
    assert_eq!(x_after_vertical, x_after);

    // A drag that starts out diagonally moves both.
    app.locator(Selector::id("code_view")).touch_drag_by_and_hold(-60.0, -40.0);
    let [page_diagonal, x_diagonal] = offsets(&app, ["code_page", "code_x"]);
    assert!(page_diagonal >= page_after + 30.0, "the page followed the finger ({page_diagonal})");
    assert!(x_diagonal >= x_after + 50.0, "the code followed the finger ({x_diagonal})");
}

#[makepad_test]
fn touch_dragging_code_never_selects_it(app: TestApp) {
    open_code_modal(&app);
    // Toward both scroll limits, so neither scroller can keep the text under the finger.
    app.locator(Selector::id("code_view")).touch_drag_by(60.0, 60.0);
    assert_eq!(offsets(&app, ["code_sel"]), [0.0]);
}

#[makepad_test]
fn mouse_dragging_code_selects_it_and_scrolls_nothing(app: TestApp) {
    open_code_modal(&app);
    let [page, x] = offsets(&app, ["code_page", "code_x"]);
    app.locator(Selector::id("code_view")).drag_by(-120.0, -40.0);
    assert_eq!(offsets(&app, ["code_sel", "code_page", "code_x"]), [1.0, page, x]);
}

#[makepad_test]
fn touch_dragging_from_a_button_above_code_never_clicks_it(app: TestApp) {
    open_code_modal(&app);
    let [page] = offsets(&app, ["code_page"]);
    app.locator(Selector::id("code_button")).touch_drag_by_and_hold(0.0, -60.0);
    assert_eq!(offsets(&app, ["code_clicks", "code_page"]), [0.0, page]);

    // A touch that stays put is still a tap.
    app.locator(Selector::id("code_button")).touch_drag_by(0.0, 0.0);
    wait_for(&app, "code_clicks", 1.0);
}

#[makepad_test]
fn a_scroll_view_never_joins_a_drag_on_a_scroller_drawn_over_it(app: TestApp) {
    open_code_modal(&app);
    let [page] = offsets(&app, ["code_page"]);
    app.locator(Selector::id("code_floating")).touch_drag_by_and_hold(0.0, -60.0);
    assert_eq!(offsets(&app, ["code_page"]), [page]);
}

#[makepad_test]
fn a_diagonal_touch_drag_on_a_strip_in_a_list_moves_only_one_of_them(app: TestApp) {
    let [outer, strip] = offsets(&app, ["outer", "strip"]);
    // Farther along x than y, so only the strip follows.
    app.locator(Selector::id("strip")).touch_drag_by_and_hold(-60.0, -20.0);
    let [outer_after, strip_after] = offsets(&app, ["outer", "strip"]);
    assert_eq!(outer_after, outer);
    assert!(strip_after >= strip + 40.0, "the strip followed the finger ({strip} -> {strip_after})");
}

#[makepad_test]
fn touch_dragging_code_that_fits_drags_its_page(app: TestApp) {
    open_code_modal(&app);
    let [page] = offsets(&app, ["code_page"]);
    app.locator(Selector::id("narrow_code_view")).touch_drag_by_and_hold(8.0, -80.0);
    let [page_after] = offsets(&app, ["code_page"]);
    assert!(page_after >= page + 70.0, "the page followed the finger ({page_after})");
}

#[makepad_test]
fn wheel_over_code_scrolls_its_page_down_and_the_code_sideways(app: TestApp) {
    open_code_modal(&app);
    let over_code = center(&app, Selector::id("code_view"));
    let [page, x] = offsets(&app, ["code_page", "code_x"]);
    wheel(&app, over_code, 0.0, 60.0);
    assert_eq!(offsets(&app, ["code_page", "code_x"]), [page + 60.0, x]);
    thread::sleep(NEW_SCROLL);
    wheel(&app, over_code, 60.0, 0.0);
    assert_eq!(offsets(&app, ["code_page", "code_x"]), [page + 60.0, x + 60.0]);
}

#[makepad_test]
fn a_touch_drag_on_code_keeps_to_the_way_it_started(app: TestApp) {
    open_code_modal(&app);
    let [page, x] = offsets(&app, ["code_page", "code_x"]);
    // Sideways first, then down without lifting the finger: only the code moves.
    app.locator(Selector::id("code_view")).touch_drag_through_and_hold(&[(-60.0, 0.0), (-60.0, -100.0)]);
    let [page_after, x_after] = offsets(&app, ["code_page", "code_x"]);
    assert_eq!(page_after, page);
    assert!(x_after >= x + 50.0, "the code followed the finger sideways ({x_after})");

    // Down first, then sideways: only the page moves.
    app.locator(Selector::id("code_view")).touch_drag_through_and_hold(&[(0.0, -60.0), (-80.0, -60.0)]);
    let [page_last, x_last] = offsets(&app, ["code_page", "code_x"]);
    assert!(page_last >= page_after + 50.0, "the page followed the finger ({page_last})");
    assert_eq!(x_last, x_after);
}

#[makepad_test]
fn a_trackpad_gesture_on_code_keeps_to_the_way_it_started(app: TestApp) {
    open_code_modal(&app);
    let (x, y) = center(&app, Selector::id("code_view"));
    let gesture = |deltas: &[(f64, f64)]| {
        app.scroll_at(x, y, deltas[0].0, deltas[0].1, Some("began"));
        for &(dx, dy) in &deltas[1..] {
            app.scroll_at(x, y, dx, dy, Some("changed"));
        }
        app.scroll_at(x, y, 0.0, 0.0, Some("ended"));
    };
    let [page, code_x] = offsets(&app, ["code_page", "code_x"]);
    // Sideways first, then down without lifting the fingers: only the code moves.
    gesture(&[(4.0, 0.0), (4.0, 0.0), (0.0, 40.0)]);
    assert_eq!(offsets(&app, ["code_page", "code_x"]), [page, code_x + 8.0]);

    // A diagonal start scrolls both ways.
    gesture(&[(4.0, 4.0), (4.0, 4.0), (0.0, 40.0)]);
    assert_eq!(offsets(&app, ["code_page", "code_x"]), [page + 48.0, code_x + 16.0]);
}
