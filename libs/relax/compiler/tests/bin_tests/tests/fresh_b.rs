// Every test target is a process of its own, as under cargo (makepad test runs each out of the
// package's one test program, run.rs --subject): its statics and its environment start fresh,
// whichever target ran before it. tests/fresh_a.rs and tests/fresh_b.rs are the same test.
#[test]
fn statics_and_environment_start_fresh() {
    assert_eq!(bt_app::TOUCHED.fetch_add(1, std::sync::atomic::Ordering::SeqCst), 0);
    assert!(std::env::var_os("BT_APP_TOUCHED").is_none());
    std::env::set_var("BT_APP_TOUCHED", "1");
}
