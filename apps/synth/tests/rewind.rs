//! The transport's Rewind in the running app, driven over `--remote` by
//! makepad_test: the button, found by its accessible name, and Shift+Space
//! leave a stopped score at the top. What a rewind does to the sound is
//! `a_rewind_sounds_as_the_first_start` (makepad-score-live).

use makepad_test::{run_with_config, KeyCode, KeyModifiers, Selector, TestApp, TestConfig};

#[test]
fn rewind_leaves_a_stopped_score_at_the_top() {
    let config = TestConfig::current_package(env!("CARGO_MANIFEST_DIR"), env!("CARGO_PKG_NAME"), "rewind").unwrap();
    run_with_config(config, |app: TestApp| {
        let named = |name: &str| app.locator(Selector::all().any_window().name(name)).wait_text(name);
        named("Rewind");
        named("1.1");
        app.locator(Selector::all().any_window().name("Rewind")).click();
        named("1.1");
        app.press_key_with_modifiers(KeyCode::Space, KeyModifiers { shift: true, ..Default::default() });
        named("1.1");
    })
    .unwrap();
}
