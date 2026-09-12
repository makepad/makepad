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

- `env!("CARGO_MANIFEST_DIR")` provides the package directory
- `env!("CARGO_PKG_NAME")` provides the package to run

That keeps the normal Rust workflow intact: add a dev-dependency, write `tests/*.rs`, and run `cargo test --release -p <package>`.

## Macro Behavior

`#[makepad_test]` expands to a normal `#[test]` wrapper that:

1. runs `cargo build --release -p <package>` from the package directory
2. selects the standalone executable from Cargo's compiler-artifact messages
3. launches that executable with `--remote`, hidden unless visible mode is requested
4. reads the owned PID and endpoint and waits for the app's first window
5. passes a `TestApp` into your test body
6. captures failure artifacts on returned errors or panics
7. requests `/gq`, falls back to `/quit` if needed, and confirms the owned child exits

Cargo's owning-workspace target is reused, including explicit `CARGO_TARGET_DIR`
overrides. No build target is forced beneath each package. Tests keep their
small artifact directories beneath the package directory.

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
- artifacts: `<manifest_dir>/target/makepad_test/<package>/<test>/`

The runner serializes app sessions within each test executable. Use
`--test-threads=1` for predictable suite order. `MAKEPAD_TEST_PARALLEL=1` opts out
of the runner lock when a suite is designed for concurrent owned instances.

## Visible Mode and Configuration

The default is a hidden native window (`MAKEPAD_HIDE_WINDOWS=1`). To watch a
standalone test window without focusing it:

```bash
MAKEPAD_TEST_VISIBLE=1 cargo test --release -p makepad-example-text-input --test ui -- --test-threads=1
```

Visible mode uses the same owned-process transport and never connects to an
existing app or Studio session. The harness removes `MAKEPAD_FOCUS` from the
child's environment.

Pacing variables:

- `MAKEPAD_TEST_STARTUP_DELAY_MS=1000` waits after startup before the test starts
- `MAKEPAD_TEST_ACTION_DELAY_MS=750` waits after each interaction
- `MAKEPAD_TEST_KEEP_OPEN_MS=3000` pauses briefly before shutdown

For explicit configuration, construct `TestConfig::new` or
`TestConfig::current_package`, adjust it, and pass it to `run_with_config`.
`bin_name` selects a target in a package with several binaries; `app_args` adds
arguments after `--remote`; `visible` controls visibility. `env` supplies app
environment variables. Its `CARGO_TARGET_DIR`, when present, also applies to the
build. Otherwise Cargo's inherited environment and configuration apply.

The former `mount_name` and `listen_address` fields and
`MAKEPAD_TEST_STUDIO`/`MAKEPAD_TEST_STUDIO_MOUNT` settings no longer apply. There
is no hub or mount to configure.

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
    .press_key(KeyCode::ReturnKey);
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
app.forward(vec![/* pointer, scroll, key, or text StudioToApp events */]);
```

`forward` translates supported input events to HTTP input routes. Other legacy
protocol variants return an explicit error. Native timestamps are assigned at
injection; key repeat and IME metadata are not forwarded. It does not provide live reload,
window resizing, swapchains, clipboard forwarding, or hub control. Prefer the
regular `TestApp` methods for snapshots, grabs, and logs.

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

Window names, enabled flags, and selections come from the actual widget
snapshot. Missing required fields fail decoding instead of fabricating state.
Optional state is absent when the widget does not expose it; empty labels may
omit `text`, while an empty input `value` remains an empty string.

Interactions require visible geometry. State reads prefer visible matches and
can fall back to a uniquely matched clipped widget, such as a label below a
scrolling page.

## Failure Artifacts

Failed tests write to:

```text
<manifest_dir>/target/makepad_test/<package>/<test>/
```

Builds retain stderr; launched sessions also retain app stdout/stderr and a
`shutdown.txt` record with the owned PID and shutdown result. Test-body failures
additionally capture:

- `failure.txt`
- `logs.txt`
- `widget-snapshot.json`
- `widget-tree.txt` or `widget-tree-error.txt`
- `failure-screenshot.png` or `failure-screenshot-error.txt`

If a capture step fails, the runtime writes a `*-error.txt` file instead of silently dropping the artifact.

## Running Tests

```bash
cargo test --release -p makepad-test
cargo test --release -p makepad-example-text-input --test ui -- --test-threads=1
```

## Standalone Transport and Ownership

The runtime uses the app's documented [HTTP remote surface](../../docs/agents/app-remote.md):
`/s`, `/snap`, `/d`, `/g`, `/log`, and input routes. Input requests wait for a
resulting frame. Rectangles are window-local layout points; do not apply DPI
conversion to clicks.

Screenshots are captured from the app's own drawable. Setting
`TestConfig::env["MAKEPAD_GPUSIM_DPI"]` to a positive number scales screenshots
to that pixel density for existing suites; it does not select a software
renderer or change the native window's DPI.

Cleanup first uses `/gq` to save a final frame and quit. If capture is unavailable
or the app remains alive, it sends `/quit`. A dropped response is not treated as
proof that the process failed to exit. The runner waits for its owned `Child`
and kills only that child after graceful shutdown times out. This cleanup also
runs after test panics. It never stops, replaces, or drives user-owned instances.

## Troubleshooting

If a test times out or fails to resolve a widget:

1. inspect `target/makepad_test/.../logs.txt`
2. inspect `widget-snapshot.json` for text/value/checked/selected state
3. inspect `widget-tree.txt` for the raw compact tree
4. verify the selector is scoped tightly enough

For startup failures, inspect `build-stderr.txt`, `app-stdout.txt`, and
`app-stderr.txt`. `shutdown.txt` records whether the child exited normally or
required the exact-PID fallback. For live diagnostics inside a test,
`app.pid()`, `app.remote_endpoint()`, and `app.grab_dir()` identify only that
owned instance.

A `closed by user` response is preserved as an error; the runner does not
relaunch a dismissed app. Grabs have a longer request timeout because the
backend must finish readback and PNG encoding.

## Current Limitations

- the macro targets its current package; explicit configuration can select another
- synchronous API only
- no visual diffing or trace viewer yet
- some complex widgets still need more structured state over time

The native runtime and lifecycle fixtures are currently validated on macOS.
Other platform backends may differ in capture support.
