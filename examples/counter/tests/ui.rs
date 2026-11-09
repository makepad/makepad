use makepad_test::{makepad_test, Selector, TestApp};

#[makepad_test]
fn counter_tracks_multiple_clicks(app: TestApp) {
    for _ in 0..3 {
        app.locator(Selector::id("increment_button")).click();
    }
    app.locator(Selector::id("counter_label"))
        .wait_text("Count: 3");
}

/// Snapshots a wait takes at each of its asks: an observer busy enough that what answering it
/// costs the app would alone keep the app above the idle share (10 a time do not: the counter's
/// tree is small).
const SNAPSHOTS_PER_ASK: usize = 100;
/// The asks the wait below may take: the app quiets once it has started, so its idle budget ends
/// the wait long before this many.
const MOST_ASKS: u32 = 1_500;

/// A wait for what never comes fails once the app sits idle, though it asks the app all the
/// while: what the app spends answering its observers is not its own work.
#[makepad_test]
fn a_wait_that_asks_all_the_while_fails_once_the_app_sits_idle(app: TestApp) {
    let mut asks = 0;
    let err = app
        .try_wait_until(|| {
            asks += 1;
            assert!(asks < MOST_ASKS, "the wait never saw the app idle: {asks} asks");
            let seen: usize = (0..SNAPSHOTS_PER_ASK).map(|_| app.widget_snapshot().nodes().len()).sum();
            Some(format!("a label that never shows ({seen} nodes seen)"))
        })
        .unwrap_err();
    assert!(err.message().contains(": it sat idle for "), "fails on the idle rule: {}", err.message());
    println!("{} after {asks} asks", err.message());
}

/// Grabs a wait takes at each of its asks: the app's UI thread draws and reads back its window
/// for each, work no serving thread counts, enough that answering would alone keep the app above
/// the idle share.
const GRABS_PER_ASK: usize = 4;

/// A wait for what never comes fails once the app sits idle, though its UI thread is busy
/// answering the wait's asks all the while: what the app runs while it is asked is not its own.
#[makepad_test]
fn a_wait_whose_asks_keep_the_ui_thread_busy_fails_once_the_app_sits_idle(app: TestApp) {
    let mut asks = 0;
    let err = app
        .try_wait_until(|| {
            asks += 1;
            assert!(asks < MOST_ASKS, "the wait never saw the app idle: {asks} asks");
            for _ in 0..GRABS_PER_ASK {
                app.screenshot();
            }
            Some("a label that never shows".to_string())
        })
        .unwrap_err();
    assert!(err.message().contains(": it sat idle for "), "fails on the idle rule: {}", err.message());
    println!("{} after {asks} asks", err.message());
}
