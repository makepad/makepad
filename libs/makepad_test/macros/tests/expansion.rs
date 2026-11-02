//! Compile the expansion cases as ordinary integration tests so Cargo builds
//! and loads the proc macro for the correct host and profile.
#![deny(unused_attributes)]

extern crate self as makepad_test;
use makepad_test_macros::makepad_test;

struct TestApp;

mod __private {
    pub fn run_current_package_test<R>(
        package: &str,
        manifest: &str,
        module: &str,
        name: &str,
        body: fn(super::TestApp) -> R,
    ) {
        assert_eq!(package, env!("CARGO_PKG_NAME"));
        assert_eq!(manifest, env!("CARGO_MANIFEST_DIR"));
        assert_eq!(module, "expansion");
        assert!(!name.is_empty());
        body(super::TestApp);
    }
}

thread_local! {
    static CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[makepad_test]
pub(crate) fn return_submits(mut app: TestApp) -> Result<(), ()> {
    let _ = &mut app;
    CALLS.with(|calls| calls.set(calls.get() + 1));
    Ok(())
}

#[test]
fn expansion_wraps_test_body() {
    CALLS.with(|calls| calls.set(0));
    return_submits();
    CALLS.with(|calls| assert_eq!(calls.get(), 1));
    assert!(__makepad_test_inner_return_submits(TestApp).is_ok());
    CALLS.with(|calls| assert_eq!(calls.get(), 2));
}

#[makepad_test]
#[cfg(all())]
#[ignore]
#[should_panic(expected = "boom")]
fn smoke(_app: TestApp) {
    panic!("boom");
}

#[makepad_test]
#[cfg(any())]
fn disabled(_app: MissingType) {}

#[test]
fn expansion_preserves_test_attrs_on_wrapper() {
    let _inner: fn(TestApp) = __makepad_test_inner_smoke;
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "smoke", "--ignored"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("1 passed"), "{stdout}");
}
