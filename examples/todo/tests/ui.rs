use makepad_test::{makepad_test, Selector, TestApp};

#[makepad_test]
fn todo_smoke(app: TestApp) {
    app.locator(Selector::id("todo_input")).wait_visible();
    app.locator(Selector::all().text_exact("Get AI to control UI"))
        .wait_visible();
}

#[makepad_test]
fn todo_add_toggle_delete_and_disambiguate_duplicates(app: TestApp) {
    app.locator(Selector::widget_type("CheckBox").nth(0))
        .click()
        .wait_checked(false);

    for _ in 0..2 {
        app.locator(Selector::id("todo_input"))
            .fill("Write tests")
            .wait_value("Write tests");
        app.locator(Selector::id("add_button")).click();
    }

    app.locator(Selector::all().text_exact("Write tests"))
        .wait_count(2);
    app.locator(Selector::widget_type("CheckBox")).wait_count(3);
    app.locator(Selector::widget_type("CheckBox").nth(1))
        .click()
        .wait_checked(true);
    app.locator(Selector::all().text_exact("x").nth(2)).click();
    app.locator(Selector::all().text_exact("Write tests"))
        .wait_count(1);
    app.locator(Selector::widget_type("CheckBox")).wait_count(2);
}

#[makepad_test]
fn todo_clear_completed_removes_only_checked_items(app: TestApp) {
    for item in ["Pay invoice", "Email Ada", "Ship demo"] {
        app.locator(Selector::id("todo_input"))
            .fill(item)
            .wait_value(item);
        app.locator(Selector::id("add_button")).click();
    }

    app.locator(Selector::all().text_exact("Pay invoice"))
        .wait_visible();
    app.locator(Selector::all().text_exact("Email Ada"))
        .wait_visible();
    app.locator(Selector::all().text_exact("Ship demo"))
        .wait_visible();

    app.locator(Selector::widget_type("CheckBox").nth(1))
        .click()
        .wait_checked(true);
    app.locator(Selector::widget_type("CheckBox").nth(3))
        .click()
        .wait_checked(true);

    app.locator(Selector::id("clear_done")).click();

    app.locator(Selector::all().text_exact("Get AI to control UI"))
        .wait_count(0);
    app.locator(Selector::all().text_exact("Pay invoice"))
        .wait_count(0);
    app.locator(Selector::all().text_exact("Email Ada"))
        .wait_visible();
    app.locator(Selector::all().text_exact("Ship demo"))
        .wait_count(0);
    app.locator(Selector::widget_type("CheckBox")).wait_count(1);
}
