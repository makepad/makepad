use makepad_test::{run_with_config, KeyCode, Role, Selector, TestConfig, TestError};

#[test]
fn backspace_in_filter_keeps_current_folder() -> Result<(), TestError> {
    let mut config = TestConfig::current_package(
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_PKG_NAME"),
        "ui::backspace_in_filter_keeps_current_folder",
    )?;
    // Demo preferences and bookmarks stay in memory; other state stays in the test home.
    config.app_args.push("--demo".into());
    config.env.insert(
        "MAKEPAD_HOME".into(),
        config.artifacts_dir.join("home/.makepad").display().to_string(),
    );

    run_with_config(config, |app| {
        app.locator(Selector::role(Role::Button).name("Documents"))
            .wait_visible()
            .click_at_center();
        let folder = app.locator(Selector::id("c2.crumb_title").state("visible"));
        folder
            .try_wait_text("Documents")
            .expect("the sidebar must enter the demo Documents subfolder");

        // The filter belongs to the map view's storage options.
        app.locator(Selector::role(Role::Button).name("Flat map"))
            .wait_visible()
            .click_at_center();
        app.locator(Selector::role(Role::Button).name("Storage options"))
            .wait_visible()
            .click_at_center();
        app.locator(Selector::id("ctx_label").text_exact("Show Filters"))
            .wait_visible()
            .click_at_center();
        let query = app
            .locator(Selector::id("filter_query"))
            .wait_visible()
            .click_at_center()
            .fill("notes");
        query
            .try_wait_value("notes")
            .expect("the filter query must contain the typed word before Backspace");

        app.press_key(KeyCode::Backspace);
        query
            .try_wait_value("note")
            .expect("Backspace in the filter must delete exactly the last query character");
        folder
            .try_assert_text("Documents")
            .expect("Backspace in the filter must keep the current folder at /Demo/Documents");
    })
}
