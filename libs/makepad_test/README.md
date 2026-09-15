# makepad_test

`makepad_test` provides Rust-native UI regression tests for Makepad apps. Tests live next to the package they exercise, run through normal `cargo test`, and drive an owned standalone release app through its `--remote` HTTP surface. Test windows are hidden by default; no Studio instance or hub is required.

## Quick Start

Add this to the package under test:

```toml
[dev-dependencies]
makepad-test = { path = "../../libs/makepad_test", version = "0.1.0" }
```

Create an integration test:

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
cargo test --release -p makepad-example-text-input --test ui -- --test-threads=1
```

To watch a standalone test window, opt into visible mode. It remains unfocused:

```bash
MAKEPAD_TEST_VISIBLE=1 \
MAKEPAD_TEST_STARTUP_DELAY_MS=1000 \
MAKEPAD_TEST_ACTION_DELAY_MS=750 \
MAKEPAD_TEST_KEEP_OPEN_MS=3000 \
cargo test --release -p makepad-example-text-input --test ui -- --test-threads=1
```

Each test builds from its package directory, reusing Cargo's owning-workspace
release target. An inherited `CARGO_TARGET_DIR` or explicit
`TestConfig::env["CARGO_TARGET_DIR"]` override is respected; the harness does not
create a separate build target per example.

The runner starts only its own executable, reads its PID and ephemeral port from
the startup line, and closes it with `/gq` (grab and quit), falling back to `/quit`
when needed. It waits for the owned process to exit and kills that exact child
only if graceful shutdown fails. Existing user instances are never reused.

## Surface Area

- `#[makepad_test]` for current-package UI tests
- `TestApp` for app-scoped input, waits, logs, screenshots, and low-level input forwarding
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

Artifacts are package-local (separate from the shared Cargo build target):

```text
<manifest_dir>/target/makepad_test/<package>/<test>/
```

Builds write `build-stderr.txt`. Launched sessions also write `app-stdout.txt`,
`app-stderr.txt`, and `shutdown.txt`. Test-body failures additionally capture:

- `failure.txt`
- `logs.txt`
- `widget-snapshot.json`
- `widget-tree.txt` or `widget-tree-error.txt`
- `failure-screenshot.png` or `failure-screenshot-error.txt`

## Current Constraints

- synchronous API only
- the macro targets its current package; `run_with_config` can target another manifest/package
- `forward` supports pointer, scroll, key, and text input; other legacy protocol messages return an error
- widget snapshots require the current remote fields `window_id` and boolean `enabled`; optional `selected` is preserved
- milestone-1 repo suite is validated on macOS first
- no visual diffing or trace viewer yet

## Guide

For the full authoring model, runtime behavior, and troubleshooting notes, see [GUIDE.md](./GUIDE.md).
