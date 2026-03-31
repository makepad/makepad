use makepad_test::run_splash_suite;

const SUITE_PATH: &str = "tests/ui.splash";

#[test]
fn splash_suite() {
    run_splash_suite(
        env!("CARGO_PKG_NAME"),
        env!("CARGO_MANIFEST_DIR"),
        module_path!(),
        SUITE_PATH,
    )
    .unwrap();
}
