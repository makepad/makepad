# makepad_test

`makepad_test` provides UI regression tests for Makepad apps. Splash is the suite authoring language; Rust is only the host bridge that launches and drives the suite.

## Quick Start (Splash suite)

Add this to the package under test:

```toml
[dev-dependencies]
makepad-test = { path = "../../libs/makepad_test", version = "0.1.0" }
```

Author the suite in Splash (`tests/ui.splash`) and use a small Rust host (`tests/ui.rs`). In this repo, [examples/splash](../../examples/splash) is the reference layout.

```rust,ignore
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
```

Run that package’s UI integration test:

```bash
cargo test -p makepad-example-splash --test ui -- --test-threads=1
```

Add `--show-output` to print per-case `[makepad_test] splash case …` lines (timings and summary) from `run_splash_suite`.

Splash suites now run in **per-case isolated mode by default**. Each `test.case` gets a fresh app session unless the suite opts into `test.configure({ session_mode: "shared" })`. Artifacts live under a suite root with per-case directories; see [GUIDE.md](./GUIDE.md).

Run the curated repo UI suite on macOS (splash example only):

```bash
tools/run_ui_tests.sh
```

Failure-artifact capture for `run_with_config` is covered by `cargo test -p makepad-test --test artifact_capture`.

### Visible Studio mode

Run the same test visibly inside a running Makepad Studio session:

```bash
MAKEPAD_TEST_VISIBLE=1 cargo test -p makepad-example-splash --test ui -- --test-threads=1
```

Visible mode expects Studio to already be running at `127.0.0.1:8001`. Set
`MAKEPAD_TEST_STUDIO=<ip:port>` to override the address.

To make the run easy to watch inside Studio, add pacing:

```bash
MAKEPAD_TEST_VISIBLE=1 \
MAKEPAD_TEST_STARTUP_DELAY_MS=1000 \
MAKEPAD_TEST_ACTION_DELAY_MS=750 \
MAKEPAD_TEST_KEEP_OPEN_MS=3000 \
cargo test -p makepad-example-splash --test ui -- --test-threads=1
```

## Surface Area

- `run_splash_suite(...)` for Splash-authored suites (recommended for new work)
- `run_splash_recorder(...)` for visible-mode draft Splash case generation
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

- `click`, `hover`, `type_text`, `fill`, `clear`
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

Suites write artifacts under:

```text
target/makepad_test/<package>/<suite>/
```

Per-case artifacts live under `cases/<case>/` inside that suite directory. Each suite also writes `suite-report.json` and `index.html`.

The runtime captures:

- `failure.txt`
- `logs.txt`
- `widget-snapshot.json`
- `widget-tree.txt` or `widget-tree-error.txt`
- `failure-screenshot.png` or `failure-screenshot-error.txt`
- `case-report.json`
- `suite-report.json`
- `index.html`

## Current Constraints

- synchronous API only
- default launch runs the **current Cargo package** under test; Splash suites may instead use `test.configure({ launch: "splash_run_item", ... })` to drive headless/visible Studio run items when the app is registered that way (see the splash example)
- recorder output is artifact-based draft Splash, not a Studio-integrated UI yet
- milestone-1 repo suite is validated on macOS first
- no visual diffing yet

## Guide

For Splash `test.*` API, Rust vs Splash selectors, runtime behavior, and troubleshooting, see [GUIDE.md](./GUIDE.md).
