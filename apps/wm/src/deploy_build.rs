//! `makepad-wm-build`: the deploy's prebuild of what this WM starts by itself. Every package is
//! built with the very `cargo` command its launch runs, `build` in place of `run`: an app or the
//! JIT tool (`makepad run`) through `AppLaunch::build_command` (the launcher's environment), the
//! node's AI Hub and devhub through `hub_app::build_node_hub` / `build_node_devhub`. Which runner an app gets comes from the WM's
//! own flags (`--jit <id>`, `--module <id>`) and `~/.makepad/wm/apps.splash`, read the way the
//! WM reads them. The WM's launches then compile nothing.
//!
//! Run it in the checkout the WM runs from, in the WM session's environment:
//! `makepad-wm-build [--jit <id>]... -- hub devhub aichat terminal sandbox ...`

use crate::apps::AppRegistry;
use crate::build::WmBuild;
use makepad_app_registry::{self as registry, AppLaunch, Runner};
use std::process::ExitCode;

/// The WM flags before `--`, the packages to build after it.
pub fn build_launched(args: &[String]) -> ExitCode {
    let Some(split) = args.iter().position(|a| a == "--") else {
        eprintln!("usage: makepad-wm-build [WM flags: --jit <id> --module <id>] -- <hub | devhub | app id>...");
        return ExitCode::from(2);
    };
    let (flags, ids) = (&args[..split], &args[split + 1..]);
    let apps = AppRegistry::load(&crate::theme::makepad_home().join("wm/apps.splash"), flags, &WmBuild::default(), true);
    let Some(root) = registry::repo_root() else {
        eprintln!("makepad-wm-build: no makepad checkout here");
        return ExitCode::from(2);
    };
    let mut failed = false;
    // One cargo per package (a multi -p build unifies features and leaves each lone launch
    // dirty); the JIT tool once, however many apps run under it.
    let mut jit_built = false;
    for id in ids {
        if id == "hub" {
            failed |= crate::hub_app::build_node_hub() != ExitCode::SUCCESS;
            continue;
        }
        if id == "devhub" {
            failed |= crate::hub_app::build_node_devhub() != ExitCode::SUCCESS;
            continue;
        }
        let Some(entry) = registry::find(id) else {
            eprintln!("makepad-wm-build: unknown app {id}");
            failed = true;
            continue;
        };
        let runner = apps.runner(id);
        if runner == Runner::Jit {
            if jit_built { continue; }
            jit_built = true;
        }
        let mut cmd = match AppLaunch::build_command(entry, runner, root.clone()) {
            Ok(cmd) => cmd,
            Err(e) => {
                eprintln!("makepad-wm-build: {id}: {e}");
                failed = true;
                continue;
            }
        };
        // A log, not a terminal: plain lines.
        cmd.env("CARGO_TERM_PROGRESS_WHEN", "never");
        let what = if runner == Runner::Jit { format!("{id} (under the JIT: makepad-run)") } else { id.clone() };
        eprintln!(
            "makepad-wm-build: {what}: {} {}",
            cmd.get_program().to_string_lossy(),
            cmd.get_args().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" ")
        );
        crate::host::no_console_window(&mut cmd);
        match cmd.status() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("makepad-wm-build: {id}: build failed: {status}");
                failed = true;
            }
            Err(e) => {
                eprintln!("makepad-wm-build: {id}: cannot start cargo: {e}");
                failed = true;
            }
        }
    }
    if failed { ExitCode::from(1) } else { ExitCode::SUCCESS }
}
