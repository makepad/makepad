//! `makepad-app-flow`: the Flow window, or with `--headless`/`--check` a
//! workflow package run without one.

#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    flow_app::main()
}

#[cfg(target_arch = "wasm32")]
fn main() {
    flow_app::run_window();
}
