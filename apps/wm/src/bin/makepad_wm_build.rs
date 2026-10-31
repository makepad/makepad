//! `makepad-wm-build [WM flags] -- <hub | app id>...`: the deploy's prebuild of what the WM
//! starts, each package with the very `cargo` command the WM's start of it runs, `build` for
//! `run` (apps/wm/src/deploy_build.rs). Run it in the checkout the WM runs from, in the WM
//! session's environment, with the WM's own `--jit`/`--module` flags: the WM's launches then
//! compile nothing.

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    makepad_app_wm::build_launched(&args)
}
