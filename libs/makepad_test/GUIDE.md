# makepad_test Guide

This guide covers how to write, run, and debug UI tests with `makepad_test`.

## Authoring Model

Tests live beside the package they exercise, usually under `tests/`.

Example:

```text
examples/text_input/
├── Cargo.toml
├── src/main.rs
└── tests/ui.rs
```

The macro is package-local by default:

- `env!("CARGO_MANIFEST_DIR")` is used as the mount root
- `env!("CARGO_PKG_NAME")` is used as the package to run

That means `#[makepad_test]` is optimized for the common case of "test the current example crate."

## Macro Behavior

`#[makepad_test]` expands to a normal `#[test]` wrapper.

The wrapper:

1. starts `StudioHub::start_in_process`
2. mounts the current package directory
3. runs the current package through `ClientToHub::Run`
4. waits for `BuildStarted` and `AppStarted`
5. passes a `TestApp` into your test body
6. captures failure artifacts if the test returns an error or panics

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

Unsupported in v1:

- async tests
- methods with `self`
- generic test functions
- macro arguments

## Runtime API

`TestApp` owns the running package session.

Core helpers:

```rust
app.locator(Selector::id("input_email")).wait_visible().click();
app.type_text("hello");
app.press_return();
app.wait_for_log_contains("Returned from singleline");
let dump = app.widget_dump();
let screenshot = app.screenshot();
```

Lower-level escape hatch:

```rust
app.forward(vec![/* StudioToApp messages */]);
```

## Selectors And Locators

Available selector constructors:

- `Selector::id("widget_id")`
- `Selector::widget_type("TextInput")`
- `Selector::raw("id:input_email")`

`Locator` interaction methods require exactly one visible match.

Failure modes are explicit:

- zero matches: `selector '...' matched no visible widgets`
- multiple matches: `selector '...' matched multiple widgets: ...`

This is deliberate. It keeps tests strict and reduces accidental clicks against the wrong widget.

## Failure Artifacts

Failed tests write to:

```text
target/makepad-ui-tests/<package>/<test>/
```

Typical contents:

- `failure.txt`: the test error or panic message
- `logs.txt`: build and child-app logs
- `widget-tree.txt`: compact widget dump
- `failure-screenshot.png`: captured UI state

If a capture step fails, the runtime writes an `*-error.txt` file instead of silently dropping the artifact.

## Running Tests

Recommended command shape:

```bash
cargo test -p makepad-example-text-input --test ui -- --nocapture --test-threads=1
```

`--test-threads=1` is recommended for UI suites because the runtime serializes app sessions and artifacts are easier to read when one UI test runs at a time.

## Headless Transport

The runtime uses the existing Makepad studio protocol.

Current transport shape:

- the hub runs in-process
- the app runs headless
- widget queries, input, and screenshots are driven through the studio protocol
- stdio is used for direct headless control when the app is operating in stdout-mode studio transport

This keeps the authoring model Rust-native while reusing the same protocol surface Studio already speaks.

## Troubleshooting

If a test times out:

1. inspect `target/makepad-ui-tests/.../logs.txt`
2. inspect `widget-tree.txt` to confirm the widget exists and is visible
3. check whether the selector is too broad or too early

If you need bridge-level logging from the hub:

```bash
MAKEPAD_STUDIO_HUB_DEBUG=1 cargo test -p makepad-example-text-input --test ui -- --nocapture --test-threads=1
```

That enables verbose transport diagnostics for the in-process hub.

If screenshot capture is slower than expected, keep in mind that screenshots move large PNG payloads through the studio protocol. The runtime already uses a longer timeout for screenshot capture than it does for normal widget queries.

## Current Limitations

- current-package execution only
- synchronous tests only
- no `fill()` helper yet
- selectors only expose geometry-backed matching
- no DOM-style text/value/state assertions yet
- no trace viewer or visual diffing

These are deliberate v1 constraints. The goal is to keep the first version simple, Rust-local, and aligned with the existing Studio protocol rather than inventing a separate runner model.
