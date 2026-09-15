use std::path::PathBuf;

/// Returns Makepad's shared per-user state directory.
///
/// `MAKEPAD_HOME` overrides the default. Otherwise the default is `.makepad`
/// below the user's home directory, with the process temporary directory used
/// only when the platform exposes no home directory. The AI hub has an older
/// copy of this rule and should call this helper when its dependency direction
/// permits it.
pub fn makepad_home() -> PathBuf {
    if let Some(home) = std::env::var_os("MAKEPAD_HOME") {
        return PathBuf::from(home);
    }
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".makepad")
}

/// Returns the root used by the native key/value storage backend.
pub fn storage_dir() -> PathBuf {
    makepad_home().join("storage")
}
