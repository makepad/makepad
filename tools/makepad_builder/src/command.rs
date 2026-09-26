//! The public app command never exports the private compiler environment.
use std::{env, process::Command};
#[cfg(target_os = "macos")]
use std::fs;

pub fn launch_scope() -> Result<(), String> {
    let root = crate::validate_install_root(&crate::default_root())?;
    let binary = if cfg!(windows) { crate::home_of(&root).join("scope.exe") } else { root.join("scope.bin") };
    if !binary.is_file() {
        return Err("Build Scope in Makepad Builder first".into());
    }
    let mut args: Vec<_> = env::args_os().skip(2).collect();
    if args.first().is_some_and(|arg| !arg.to_string_lossy().starts_with('-')) {
        args.insert(0, "--cwd".into());
    }
    #[cfg(target_os = "macos")]
    let binary = {
        let data = fs::read(root.join("installed/scope.json")).map_err(|e| e.to_string())?;
        let release = crate::catalog::Release::parse(&makepad_strict_json::parse(&data).map_err(str::to_owned)?)?;
        let cwd = env::current_dir().map_err(|e| e.to_string())?;
        let project = args.windows(2).find(|args| args[0] == "--cwd")
            .map(|args| cwd.join(&args[1])).unwrap_or_else(|| cwd.clone());
        let executable = crate::desktop::prepare(&root, &release, &project)?;
        if !args.iter().any(|arg| arg == "--cwd") {
            args.splice(0..0, ["--cwd".into(), cwd.into_os_string()]);
        }
        executable
    };
    let mut child = Command::new(binary);
    child.args(args).env_remove("MAKEPAD_LOADER_EMAIL").env_remove("MAKEPAD_PACKAGE_DIR");
    let cuda = root.join("toolchain/cuda/bin");
    if cuda.is_dir() {
        let mut paths = vec![cuda];
        if let Some(inherited) = env::var_os("PATH") { paths.extend(env::split_paths(&inherited)); }
        child.env("PATH", env::join_paths(paths).map_err(|e| e.to_string())?);
    }
    crate::runtime::hide_console(&mut child);
    let status = child.status().map_err(|e| format!("Could not open Scope: {e}"))?;
    if !status.success() { return Err(format!("Scope exited {status}")); }
    Ok(())
}
