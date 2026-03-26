# makepad_test Guide

This guide covers how to write, run, and debug UI tests with `makepad_test`.

## Authoring Model

Tests live beside the package they exercise, usually under `tests/`. Splash is the source of truth for the suite, and Rust is just the `cargo test` host stub.

```text
examples/splash/
├── Cargo.toml
├── tests/ui.splash
└── tests/ui.rs
```

The Rust stub matches the splash example:

```rust
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

The Splash file registers the suite:

```text
examples/splash/
├── Cargo.toml
├── src/main.rs
├── tests/ui.rs
└── tests/ui.splash
```

`run_splash_suite(...)` is current-package oriented by default:

- `env!("CARGO_MANIFEST_DIR")` provides the package root
- `env!("CARGO_PKG_NAME")` provides the package to run
- `tests/ui.splash` registers the cases and suite config with `mod.test`

That keeps the normal `cargo test` workflow intact while moving test authoring into Splash.

## Suite Behavior

`run_splash_suite(...)`:

1. loads `tests/ui.splash`
2. installs `mod.test`
3. collects `test.configure(...)` and `test.case(...)`
4. starts **one** app + hub session for the entire suite (all `test.case` bodies run in order against the same process)
5. executes each case in order
6. captures failure artifacts on returned errors or panics

**Shared process:** Cases are not isolated by default — they share one running app. If a case leaves modals, navigation, or global state in a bad state, later cases can fail. Prefer returning to a known screen at the start of each case, or split into separate suite files if you need a hard process boundary. A future opt-out (e.g. per-case isolation) could be added if needed.

**Artifact directory:** Failure artifacts use a suite-level test name, not per-case names: `splash_suite` when `module_path!()` is empty, or `{module_path}::splash_suite` (sanitized for paths under `target/makepad_test/.../`). Error messages still include the failing `test.case` name.

Splash suites can either:

- launch the current package, or
- launch visible/headless Splash run items via `test.configure({ launch: "splash_run_item", ... })`

## Runtime Defaults

The runtime is synchronous and serial-first:

- action timeout: `10s`
- poll interval: `50ms`
- artifacts: `target/makepad_test/<package>/<test>/`

The in-process runner also serializes app sessions, so UI suites should be invoked with `--test-threads=1`.

## Visible Studio Mode

By default, `makepad_test` launches the app headlessly through an in-process hub.
For local debugging, you can switch the same test to a visible Studio-backed run:

```bash
MAKEPAD_TEST_VISIBLE=1 cargo test -p makepad-example-splash --test ui -- --test-threads=1
```

Visible mode behavior:

- reuses the same `TestApp` and `Locator` APIs
- connects to an already running Makepad Studio instance
- clears older builds for the same package before launching a fresh run
- launches through Studio `Run`, so the app is visible in Studio's runview

Environment variables:

- `MAKEPAD_TEST_VISIBLE=1` enables visible mode
- `MAKEPAD_TEST_STUDIO=127.0.0.1:8001` overrides the Studio address
- `MAKEPAD_TEST_STARTUP_DELAY_MS=1000` waits after the app appears before the test starts
- `MAKEPAD_TEST_ACTION_DELAY_MS=750` waits after each interaction so clicks and typing are visible
- `MAKEPAD_TEST_KEEP_OPEN_MS=3000` keeps the app open briefly before shutdown

For this repo, visible mode defaults to the Studio mount `makepad`. If your
Studio session uses a different mount name, set `MAKEPAD_TEST_STUDIO_MOUNT`.

## Selectors

Selectors are snapshot-based. They match structured widget state instead of only relying on geometry query strings.

Constructors:

- `Selector::all()`
- `Selector::id("widget_id")`
- `Selector::widget_type("TextInput")`
- `Selector::raw("text:hello")`

Builder filters:

- `.text_exact("...")`
- `.text_contains("...")`
- `.nth(index)`
- `.window("panel_window")`
- `.window_index(1)`
- `.any_window()`

Selectors default to the primary window. That keeps single-window tests terse while still allowing explicit multi-window targeting.

## Splash API

Suites import `mod.test` and register cases by side effect:

```text
use mod.test

test.configure({
    launch: "splash_run_item"
    visible_run_item: "makepad-example-splash"
    headless_run_item: "makepad-example-splash-headless-test"
})

test.case("smoke", || {
    test.click({id: "submit"})
    test.wait_text({id: "status"}, "Saved")
})
```

Available host methods include:

- `test.configure`
- `test.case`
- `test.fail`
- `test.click`, `test.fill`, `test.clear`, `test.type_text`
- `test.press_return`, `test.press_key`, `test.scroll`, `test.drag`
- `test.wait_visible`, `test.wait_hidden`, `test.wait_count`
- `test.wait_text`, `test.wait_value`, `test.wait_checked`, `test.wait_enabled`
- `test.expect_text`, `test.expect_value`, `test.expect_checked`, `test.expect_enabled`
- `test.snapshot`, `test.snapshots`, `test.widget_dump`, `test.screenshot`
- `test.logs`, `test.wait_log`

## Selectors

Selectors are passed as Splash objects and matched against structured widget state.

Supported fields:

- `id`
- `widget_type`
- `raw`
- `text_exact`
- `text_contains`
- `nth`
- `window`
- `window_index`
- `any_window`

## Structured Widget State

Each snapshot record exposes:

- widget id
- widget type
- bounds
- window id and window index
- visible/enabled state
- widget-specific state when available:
  - `text`
  - `value`
  - `checked`
  - `selected`

That is enough to cover common labels, buttons, text inputs, checkboxes/toggles, dock tabs, and multi-window widgets without scraping raw dumps.

## Failure Artifacts

Failed tests write to:

```text
target/makepad_test/<package>/<test>/
```

Typical contents:

- `failure.txt`
- `logs.txt`
- `widget-snapshot.json`
- `widget-tree.txt` or `widget-tree-error.txt`
- `failure-screenshot.png` or `failure-screenshot-error.txt`

If a capture step fails, the runtime writes a `*-error.txt` file instead of silently dropping the artifact.

## Running Tests

Package-local (splash example in this repo):

```bash
cargo test -p makepad-example-splash --test ui -- --test-threads=1
```

Per-case progress: `run_splash_suite` writes lines like `[makepad_test] splash case 1/3: …` and `… ok (12.34s)` to **stderr** for each `test.case`, plus **`splash: app ready`** (time before any case — hub, Cargo build, launch), **`splash suite: N cases ran`** (Splash bodies only), and **`splash: total`** (startup + cases + teardown). The number **`cargo test` prints at the end** (`finished in XXs`) is close to **`splash: total`** and is **not** the same as summing per-case lines — startup and teardown happen outside the case loop. Cargo hides this for passing tests unless you pass **`--show-output`** (show all test output) or **`--nocapture`** (do not capture stdout/stderr):

```bash
cargo test -p makepad-example-splash --test ui -- --test-threads=1 --show-output
```

Harness-level artifact capture (runs against a small example app path inside the `makepad-test` crate):

```bash
cargo test -p makepad-test --test artifact_capture -- --test-threads=1
```

Curated repo suite on macOS:

```bash
tools/run_ui_tests.sh
```

That runner executes `makepad-example-splash` and prints the artifact directory.

## Headless Transport

The runtime reuses the Studio protocol rather than inventing a separate automation channel.

Current shape:

- the hub runs in-process
- the app runs headless
- widget snapshots, screenshots, and logs move through the Studio protocol
- direct stdio is used for headless control where supported

This keeps the test surface aligned with how Studio itself talks to Makepad apps.

## Troubleshooting

If a Splash case times out or fails to resolve a widget:

1. inspect `target/makepad_test/.../logs.txt`
2. inspect `widget-snapshot.json` for text/value/checked/selected state
3. inspect `widget-tree.txt` for the raw compact tree
4. verify the selector is scoped tightly enough

If you need hub-level transport diagnostics:

```bash
MAKEPAD_STUDIO_HUB_DEBUG=1 cargo test -p makepad-example-splash --test ui -- --test-threads=1
```

Screenshot capture is intentionally given a longer timeout than normal widget-state queries because PNG encoding and transport cost more than structured snapshot requests.

## Current Limitations

- default execution targets the **current Cargo package**; optional `splash_run_item` configuration routes through Studio run items instead
- synchronous API only
- no visual diffing or trace viewer yet
- some complex widgets still need more structured state over time

Milestone 1 is intentionally scoped around reliable Rust-local UI regression coverage first, with cross-platform expansion and richer tooling following after the harness stabilizes.
