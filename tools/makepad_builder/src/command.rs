//! The public app command never exports the private compiler environment.
use std::{env, path::Path, process::Command};
use std::fs;

pub fn launch_scope() -> Result<(), String> {
    let root = crate::validate_install_root(&crate::default_root())?;
    let binary = root.join(if cfg!(windows) { "scope.exe" } else { "scope.bin" });
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

#[cfg(windows)]
fn command_path() -> Result<std::path::PathBuf, String> {
    Ok(std::path::PathBuf::from(env::var_os("LOCALAPPDATA").ok_or("Missing LocalAppData directory")?).join("Makepad/bin/scope.cmd"))
}

#[cfg(windows)]
fn write_command(root: &Path, id: &str) -> Result<(), String> {
    let path = command_path()?;
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let executable = root.join("makepad-builder.exe");
    let executable = executable.to_str().ok_or("Non-UTF8 installation path")?;
    // CMD does not need the Win32 extended-length prefix for ordinary paths.
    let executable = executable.strip_prefix("\\\\?\\").unwrap_or(executable);
    let executable = crate::runtime::batch(executable)?;
    fs::write(path, format!("@rem Makepad scope command {id}\r\n@echo off\r\nsetlocal DisableDelayedExpansion\r\n\"{executable}\" launch-scope %*\r\nexit /b %errorlevel%\r\n")).map_err(|e| e.to_string())
}

/// Repair only the command owned by this installation after its folder moves.
/// A different installation or a command the user removed is left alone.
pub fn repair(root: &Path) -> Result<(), String> {
    #[cfg(windows)] {
        let Ok(id) = fs::read_to_string(root.join(".scope-command-id")) else { return Ok(()) };
        let Ok(script) = fs::read_to_string(command_path()?) else { return Ok(()) };
        if !id.is_empty() && script.lines().next() == Some(&format!("@rem Makepad scope command {id}")) {
            write_command(root, &id)?;
        }
    }
    #[cfg(unix)] {
        let Ok(id) = fs::read_to_string(root.join(".scope-command-id")) else { return Ok(()) };
        let path = command_path()?;
        let owner = path.with_extension("makepad-id");
        if !id.is_empty() && fs::read_to_string(owner).is_ok_and(|s| s == id) && fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            let target = root.join("scope");
            if fs::read_link(&path).ok().as_ref() != Some(&target) {
                fs::remove_file(&path).map_err(|e| e.to_string())?;
                std::os::unix::fs::symlink(target, path).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn command_path() -> Result<std::path::PathBuf, String> {
    Ok(std::path::PathBuf::from(env::var_os("HOME").ok_or("Missing home directory")?).join(".local/bin/scope"))
}

#[cfg(unix)]
pub fn install(root: &Path) -> Result<(), String> {
    use std::io::Write;
    if !root.join("scope.bin").is_file() || !root.join("scope").is_file() { return Err("Build Scope before enabling its command".into()); }
    let path = command_path()?;
    let owner = path.with_extension("makepad-id");
    if fs::symlink_metadata(&path).is_ok() {
        if !owner.is_file() || !fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("An existing ~/.local/bin/scope was not created by Builder; it was left unchanged".into());
        }
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let id = format!("{:x}-{:x}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    fs::write(root.join(".scope-command-id"), &id).map_err(|e| e.to_string())?;
    fs::write(owner, &id).map_err(|e| e.to_string())?;
    std::os::unix::fs::symlink(root.join("scope"), &path).map_err(|e| e.to_string())?;
    if !env::var_os("PATH").is_some_and(|v| env::split_paths(&v).any(|p| Some(p.as_path()) == path.parent())) {
        let home = std::path::PathBuf::from(env::var_os("HOME").ok_or("Missing home directory")?);
        let shell = env::var("SHELL").unwrap_or_default();
        let profile = if shell.ends_with("/zsh") { ".zshrc" } else if shell.ends_with("/bash") { ".bashrc" } else { ".profile" };
        let profile = home.join(profile);
        let line = "export PATH=\"$HOME/.local/bin:$PATH\"";
        if !fs::read_to_string(&profile).unwrap_or_default().lines().any(|l| l == line) {
            let mut file = fs::OpenOptions::new().create(true).append(true).open(profile).map_err(|e| e.to_string())?;
            writeln!(file, "\n# Makepad app commands (private compiler paths are not exported)\n{line}").map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Called only after the menu's explicit confirmation of the user PATH change.
#[cfg(windows)]
pub fn install(root: &Path) -> Result<(), String> {
    if !root.join("scope.exe").is_file() { return Err("Build Scope before enabling its command".into()); }
    if let Ok(existing) = fs::read_to_string(command_path()?) {
        if !existing.starts_with("@rem Makepad scope command ") {
            return Err("An existing scope command in the Makepad command directory was not created by Builder. Move or rename it before setting up this command.".into());
        }
    }
    let id_path = root.join(".scope-command-id");
    let id = fs::read_to_string(&id_path).unwrap_or_else(|_| format!("{:x}-{:x}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()));
    fs::write(id_path, &id).map_err(|e| e.to_string())?;
    write_command(root, &id)?;
    // Use a fixed script and pass the path as data. Preserve unexpanded PATH
    // entries (such as %USERPROFILE%) and its existing registry value kind.
    let script = r#"$ErrorActionPreference='Stop'
$key=[Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
try {
 $old=$key.GetValue('Path','',[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
 $kind=if($key.GetValueNames() -contains 'Path'){$key.GetValueKind('Path')}else{[Microsoft.Win32.RegistryValueKind]::ExpandString}
 $bin=$env:MAKEPAD_COMMAND_BIN
 $present=@($old -split ';' | Where-Object {[Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') -ieq $bin.TrimEnd('\')}).Count -gt 0
 if(!$present){$next=if($old){$old.TrimEnd(';')+';'+$bin}else{$bin};$key.SetValue('Path',$next,$kind)}
} finally {$key.Dispose()}
"#;
    let mut command = Command::new("powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-Command", script]).env("MAKEPAD_COMMAND_BIN", command_path()?.parent().unwrap());
    crate::runtime::hide_console(&mut command);
    let output = command.output().map_err(|e| e.to_string())?;
    if !output.status.success() { return Err(format!("Could not set up the user command: {}", String::from_utf8_lossy(&output.stderr))); }
    // Notify Explorer so new terminals inherit the new user PATH.
    #[link(name = "user32")]
    unsafe extern "system" { fn SendMessageTimeoutW(window: usize, message: u32, wparam: usize, lparam: isize, flags: u32, timeout: u32, result: *mut usize) -> isize; }
    let environment: Vec<u16> = "Environment\0".encode_utf16().collect();
    unsafe { SendMessageTimeoutW(0xffff, 0x001a, 0, environment.as_ptr() as isize, 2, 2000, std::ptr::null_mut()); }
    Ok(())
}
