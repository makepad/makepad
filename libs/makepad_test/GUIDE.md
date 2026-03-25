# makepad_test Guide

This guide covers how to write, run, and debug UI tests with `makepad_test`.

## Authoring Model

Tests live beside the package they exercise, usually under `tests/`.

```text
examples/text_input/
├── Cargo.toml
├── src/main.rs
└── tests/ui.rs
```

`#[makepad_test]` is current-package oriented by default:

- `env!("CARGO_MANIFEST_DIR")` provides the mount root
- `env!("CARGO_PKG_NAME")` provides the package to run

That keeps the normal Rust workflow intact: add a dev-dependency, write `tests/*.rs`, and run `cargo test -p <package>`.

## Macro Behavior

`#[makepad_test]` expands to a normal `#[test]` wrapper that:

1. starts `StudioHub::start_in_process`
2. mounts the current package directory
3. runs the current package headlessly
4. waits for `BuildStarted` and `AppStarted`
5. passes a `TestApp` into your test body
6. captures failure artifacts on returned errors or panics

Supported signatures:

```rust
#[makepad_test]
fn smoke(app: TestApp) {
    // ...
}

#[makepad_test]
fn smoke(app: TestApp) -> Result<(), TestError> {
    // ...
    Ok(())
}
```

Unsupported:

- async tests
- methods with `self`
- generic test functions
- macro arguments

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
MAKEPAD_TEST_VISIBLE=1 cargo test -p makepad-example-counter --test ui -- --test-threads=1
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

## Locators

`Locator` methods require exactly one visible match for interaction. That strictness is intentional: it keeps tests from silently clicking the wrong widget.

Common actions:

```rust
app.locator(Selector::id("panel_input"))
    .wait_visible()
    .fill("hello")
    .wait_value("hello")
    .press_key(KeyCode::Enter);
```

Available interaction helpers:

- `click`
- `type_text`
- `fill`
- `clear`
- `press_key`
- `press_key_with_modifiers`
- `scroll`
- `drag_by`

Available waits and assertions:

- `wait_visible`
- `wait_hidden`
- `wait_count`
- `wait_text` / `assert_text`
- `wait_value` / `assert_value`
- `wait_checked` / `assert_checked`
- `wait_enabled` / `assert_enabled`

Inspection helpers:

- `snapshot()`
- `count()`
- `widget_snapshot()`
- `widget_dump()`
- `screenshot()`
- `wait_for_log_contains(...)`

Lower-level escape hatch:

```rust
app.forward(vec![/* StudioToApp messages */]);
```

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

Package-local:

```bash
cargo test -p makepad-example-text-input --test ui -- --test-threads=1
```

Curated repo suite on macOS:

```bash
tools/run_ui_tests.sh
```

That runner executes:

- `makepad-example-text-input`
- `makepad-example-counter`
- `makepad-example-todo`
- `makepad-example-floating-panel`
- `makepad-example-splash`

and prints the artifact directory for each package.

## Headless Transport

The runtime reuses the Studio protocol rather than inventing a separate automation channel.

Current shape:

- the hub runs in-process
- the app runs headless
- widget snapshots, screenshots, and logs move through the Studio protocol
- direct stdio is used for headless control where supported

This keeps the test surface aligned with how Studio itself talks to Makepad apps.

## Troubleshooting

If a test times out or fails to resolve a widget:

1. inspect `target/makepad_test/.../logs.txt`
2. inspect `widget-snapshot.json` for text/value/checked/selected state
3. inspect `widget-tree.txt` for the raw compact tree
4. verify the selector is scoped tightly enough

If you need hub-level transport diagnostics:

```bash
MAKEPAD_STUDIO_HUB_DEBUG=1 cargo test -p makepad-example-text-input --test ui -- --test-threads=1
```

Screenshot capture is intentionally given a longer timeout than normal widget-state queries because PNG encoding and transport cost more than structured snapshot requests.

## Current Limitations

- current-package execution only
- synchronous API only
- no visual diffing or trace viewer yet
- some complex widgets still need more structured state over time

Milestone 1 is intentionally scoped around reliable Rust-local UI regression coverage first, with cross-platform expansion and richer tooling following after the harness stabilizes.
