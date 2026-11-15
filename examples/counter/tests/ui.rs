use makepad_test::{makepad_test, run_with_config, Selector, TestApp, TestConfig};
use std::time::Duration;
use makepad_zune_png::{makepad_zune_core::bytestream::ZCursor, PngDecoder};

/// A grab's pixels, RGB, row by row.
fn pixels(path: &std::path::Path) -> Vec<[u8; 3]> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("cannot read the grab {}: {e}", path.display()));
    let mut decoder = PngDecoder::new(ZCursor::new(&bytes));
    let raw = decoder.decode_raw().unwrap_or_else(|e| panic!("cannot decode the grab {}: {e:?}", path.display()));
    let components = decoder.colorspace().expect("the grab has no colorspace").num_components();
    raw.chunks_exact(components).map(|p| [p[0], p[1], p[2]]).collect()
}

/// The count follows the clicks, also across a GPU device loss (a driver reset, a GPU hang
/// recovery), which is survived as natively: the HAL makes a new device, and the app runs on,
/// takes input and draws on it. Under the JIT the run's own window took the loss for its end,
/// and the app quit with it.
#[makepad_test]
fn counter_tracks_clicks_across_a_gpu_device_loss(app: TestApp) {
    for _ in 0..3 {
        app.locator(Selector::id("increment_button")).click();
    }
    app.locator(Selector::id("counter_label")).wait_text("Count: 3");
    app.lose_gpu_device();
    if let Err(err) = app.try_wait_for_log_contains("is up; uploading and painting again") {
        panic!("the app did not run on to its new GPU device: {}", err.message());
    }
    app.locator(Selector::id("increment_button")).click();
    app.locator(Selector::id("counter_label")).wait_text("Count: 4");
    let frame = pixels(&app.screenshot());
    assert!(frame.iter().any(|p| *p != frame[0]), "the frame after the reset is drawn, not one flat colour");
}

/// How long the waits below let the app sit idle before they fail. What they guard is what counts
/// as the app's own work, not how long the rule waits: a short limit ends each in seconds (the
/// default, 10 s, made these two most of the counter's test time).
const IDLE_LIMIT: Duration = Duration::from_secs(2);
/// The main thread's CPU the waits below may spend: the default, so the idle rule, not the CPU
/// budget, is what ends them.
const CPU_BUDGET: Duration = Duration::from_secs(10);

/// `test` on its own app, whose waits fail once it sat idle for [`IDLE_LIMIT`].
fn with_idle_limit(name: &str, test: impl FnOnce(TestApp)) {
    let mut config = TestConfig::current_package(env!("CARGO_MANIFEST_DIR"), env!("CARGO_PKG_NAME"), format!("ui::{name}")).unwrap();
    config.action_timeout = IDLE_LIMIT;
    if let Err(err) = run_with_config(config, test) {
        panic!("{}", err.message());
    }
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
#[test]
fn a_wait_that_asks_all_the_while_fails_once_the_app_sits_idle() {
    with_idle_limit("a_wait_that_asks_all_the_while_fails_once_the_app_sits_idle", |app| {
        let mut asks = 0;
        let err = app
            .try_wait_until_within(CPU_BUDGET, || {
                asks += 1;
                assert!(asks < MOST_ASKS, "the wait never saw the app idle: {asks} asks");
                let seen: usize = (0..SNAPSHOTS_PER_ASK).map(|_| app.widget_snapshot().nodes().len()).sum();
                Some(format!("a label that never shows ({seen} nodes seen)"))
            })
            .unwrap_err();
        assert!(err.message().contains(": it sat idle for "), "fails on the idle rule: {}", err.message());
        println!("{} after {asks} asks", err.message());
    });
}

/// Grabs a wait takes at each of its asks: the app's UI thread draws and reads back its window
/// for each, work no serving thread counts, enough that answering would alone keep the app above
/// the idle share.
const GRABS_PER_ASK: usize = 4;

/// A wait for what never comes fails once the app sits idle, though its UI thread is busy
/// answering the wait's asks all the while: what the app runs while it is asked is not its own.
#[test]
fn a_wait_whose_asks_keep_the_ui_thread_busy_fails_once_the_app_sits_idle() {
    with_idle_limit("a_wait_whose_asks_keep_the_ui_thread_busy_fails_once_the_app_sits_idle", |app| {
        let mut asks = 0;
        let err = app
            .try_wait_until_within(CPU_BUDGET, || {
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
    });
}
