# makepad_test

`makepad_test` provides Rust-native UI tests for Makepad examples and apps.

The intended workflow is the same one Rust developers already use for `#[test]` and `#[tokio::test]`:

- add the crate as a `dev-dependency`
- put UI tests in `tests/*.rs`
- annotate them with `#[makepad_test]`
- run them with `cargo test -p <package>`

The crate starts a Makepad Studio hub in-process, mounts the current package, launches the current package in headless mode, and drives it through the existing studio protocol.

## Quick Start

Add this to your example crate:

```toml
[dev-dependencies]
makepad-test = { path = "../../libs/makepad_test", version = "0.1.0" }
```

Create an integration test:

```rust,ignore
use makepad_test::{makepad_test, Selector, TestApp};

#[makepad_test]
fn return_submits(app: TestApp) {
    app.locator(Selector::id("input_singleline"))
        .wait_visible()
        .click();
    app.type_text("hello");
    app.press_return();
    app.wait_for_log_contains("Returned from singleline: \"hello\"");
}
```

Run it with:

```bash
cargo test -p makepad-example-text-input --test ui -- --nocapture --test-threads=1
```

## What You Get

- `#[makepad_test]` for package-local UI tests
- `TestApp` for app-scoped actions and assertions
- `Selector` and `Locator` for widget lookup and interaction
- failure artifacts under `target/makepad-ui-tests/<package>/<test>/`
- a lower-level `run_with_config` entry point when you need to bypass the macro

## Current Surface

- selectors: `Selector::id`, `Selector::widget_type`, `Selector::raw`
- actions: `click`, `type_text`, `press_return`
- waits: `wait_visible`, `wait_for_log_contains`
- inspection: `widget_dump`, `screenshot`
- raw escape hatch: `forward(Vec<StudioToApp>)`

Selectors are geometry-based today. They resolve through the existing widget query protocol and require exactly one visible match for interaction.

## Artifacts

When a `#[makepad_test]` test fails, the runtime captures artifacts in:

```text
target/makepad-ui-tests/<package>/<test>/
```

The runtime currently writes:

- `failure.txt`
- `logs.txt`
- `widget-tree.txt` or `widget-tree-error.txt`
- `failure-screenshot.png` or `failure-screenshot-error.txt`

## Constraints

- synchronous tests only in v1
- current-package targeting only in v1
- append-style `type_text`, not `fill`
- no JS adapter, image diffing, or trace viewer

## Guide

For the authoring model, API details, artifacts, and troubleshooting, see [GUIDE.md](./GUIDE.md).
