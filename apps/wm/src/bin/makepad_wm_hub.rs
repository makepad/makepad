//! `makepad-wm-hub build`: the deploy's build of this node's AI Hub, the very `cargo` command
//! the WM's start of it runs with `build` for `run` (apps/wm/src/hub_app.rs `build_node_hub`).
//! Run it in the checkout the WM runs from, in the WM session's environment (the deploy's
//! build-next.sh, the session script): the WM's `cargo run` of the hub then compiles nothing.

fn main() -> std::process::ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("build") => makepad_app_wm::build_node_hub(),
        _ => {
            eprintln!("usage: makepad-wm-hub build");
            std::process::ExitCode::from(2)
        }
    }
}
