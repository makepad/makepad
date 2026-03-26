# makepad_test

`makepad_test` provides UI regression tests for Makepad apps. Rust stays as the host bridge for `cargo test` and the Studio protocol; Splash is the test language for suite authoring.

## Quick Start

Add this to the package under test:

```toml
[dev-dependencies]
makepad-test = { path = "../../libs/makepad_test", version = "0.1.0" }
```

Create a Rust-hosted test:

```rust,ignore
use makepad_test::{makepad_test, Selector, TestApp};

#[makepad_test]
fn fill_and_submit(app: TestApp) {
    app.locator(Selector::id("input_singleline"))
        .wait_visible()
        .fill("hello")
        .wait_value("hello");
    app.press_return();
    app.locator(Selector::id("status_label"))
        .wait_text("Returned from singleline: \"hello\"");
}
```

Run a package-local suite with:

```bash
cargo test -p makepad-example-text-input --test ui -- --test-threads=1
```

Run the same test visibly inside a running Makepad Studio session with:

```bash
MAKEPAD_TEST_VISIBLE=1 cargo test -p makepad-example-counter --test ui -- --test-threads=1
```

Visible mode expects Studio to already be running at `127.0.0.1:8001`. Set
`MAKEPAD_TEST_STUDIO=<ip:port>` to override the address.

To make the run easy to watch inside Studio, add pacing:

```bash
MAKEPAD_TEST_VISIBLE=1 \
MAKEPAD_TEST_STARTUP_DELAY_MS=1000 \
MAKEPAD_TEST_ACTION_DELAY_MS=750 \
MAKEPAD_TEST_KEEP_OPEN_MS=3000 \
cargo test -p makepad-example-counter --test ui -- --test-threads=1
```

Run the curated repo UI suites serially on macOS with:

```bash
tools/run_ui_tests.sh
```

Create a Splash suite and run it from one Rust stub:

```rust,ignore
use makepad_test::run_splash_suite;

#[test]
fn splash_suite() {
    run_splash_suite(
        env!("CARGO_PKG_NAME"),
        env!("CARGO_MANIFEST_DIR"),
        module_path!(),
        "tests/ui.splash",
    ).unwrap();
}
```

## Surface Area

- `#[makepad_test]` for current-package UI tests
- `run_splash_suite(...)` for Splash-authored suites
- `TestApp` for app-scoped input, waits, logs, screenshots, and raw protocol forwarding
- `Selector` for structured snapshot matching
- `Locator` for strict single-widget interaction and assertions

Structured selectors support:

- `Selector::all()`
- `Selector::id("...")`
- `Selector::widget_type("...")`
- `Selector::raw("...")`
- builder filters: `.text_exact(...)`, `.text_contains(...)`, `.nth(...)`, `.window(...)`, `.window_index(...)`, `.any_window()`

Common locator actions:

- `click`, `type_text`, `fill`, `clear`
- `press_key`, `press_key_with_modifiers`
- `scroll`, `drag_by`

Common waits and assertions:

- `wait_visible`, `wait_hidden`, `wait_count`
- `wait_text`, `wait_value`, `wait_checked`, `wait_enabled`
- `assert_text`, `assert_value`, `assert_checked`, `assert_enabled`

Inspection helpers:

- `widget_snapshot()`
- `widget_dump()`
- `screenshot()`
- `wait_for_log_contains(...)`

## Failure Artifacts

Failed tests write artifacts under:

```text
target/makepad_test/<package>/<test>/
```

The runtime captures:

- `failure.txt`
- `logs.txt`
- `widget-snapshot.json`
- `widget-tree.txt` or `widget-tree-error.txt`
- `failure-screenshot.png` or `failure-screenshot-error.txt`

## Current Constraints

- synchronous API only
- current-package targeting only
- milestone-1 repo suite is validated on macOS first
- no visual diffing or trace viewer yet

## Guide

For the full authoring model, runtime behavior, and troubleshooting notes, see [GUIDE.md](./GUIDE.md).
