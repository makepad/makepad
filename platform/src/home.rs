use std::path::PathBuf;

/// Returns Makepad's shared per-user state directory.
///
/// A non-empty `MAKEPAD_HOME` overrides the default. Otherwise the default is `.makepad`
/// below the user's home directory, with the process temporary directory used
/// only when the platform exposes no home directory. The AI hub has an older
/// copy of this rule and should call this helper when its dependency direction
/// permits it.
pub fn makepad_home() -> PathBuf {
    // An empty MAKEPAD_HOME counts as unset: PathBuf::from("") would make
    // every path below it relative to the current folder.
    if let Some(home) = std::env::var_os("MAKEPAD_HOME").filter(|home| !home.is_empty()) {
        return PathBuf::from(home);
    }
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".makepad")
}

/// Returns the root used by the native key/value storage backend: an absolute
/// `MAKEPAD_STORAGE_DIR` (a test run's own, so it never reads what an earlier run
/// stored), else `storage` in [`makepad_home`].
pub fn storage_dir() -> PathBuf {
    std::env::var_os("MAKEPAD_STORAGE_DIR").map(PathBuf::from).filter(|dir| dir.is_absolute()).unwrap_or_else(|| makepad_home().join("storage"))
}

/// Where an app keeps its own per-user files: `~/Library/Application
/// Support/Makepad/<app>` on macOS, `%APPDATA%\Makepad\<app>` on Windows,
/// `$XDG_DATA_HOME/makepad/<app>` (or `~/.local/share/makepad/<app>`)
/// elsewhere. `None` when the platform names no such base as an absolute
/// path. The directory may not exist yet.
///
/// A test instance (an absolute `MAKEPAD_TEST_HOME`, the test harness's
/// stand-in for the person's home folder) keeps the same layout below that
/// folder instead (`<test home>/.local/share/makepad/<app>`,
/// `<test home>/AppData/Roaming/Makepad/<app>`, ...): a test never reads or
/// writes the machine's own state (preferences, linked devices).
///
/// Overrides for isolated runs (`MUSIC_DATA_DIR` and the like) are the
/// caller's: an app checks its own variable and falls back to this.
pub fn app_data_dir(app: &str) -> Option<PathBuf> {
    let absolute = |var: &str| std::env::var_os(var).map(PathBuf::from).filter(|path| path.is_absolute());
    let test_home = absolute("MAKEPAD_TEST_HOME");
    #[cfg(target_os = "macos")]
    {
        Some(test_home.or_else(|| absolute("HOME"))?.join("Library").join("Application Support").join("Makepad").join(app))
    }
    #[cfg(target_os = "windows")]
    {
        Some(test_home.map(|home| home.join("AppData").join("Roaming")).or_else(|| absolute("APPDATA"))?.join("Makepad").join(app))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let base = match test_home {
            Some(home) => home.join(".local").join("share"),
            None => absolute("XDG_DATA_HOME").or_else(|| absolute("HOME").map(|home| home.join(".local").join("share")))?,
        };
        Some(base.join("makepad").join(app))
    }
}
