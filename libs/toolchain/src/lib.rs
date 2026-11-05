//! Microsoft's C++ Build Tools, the Windows SDK, the MSVC Rust and NVIDIA's CUDA toolkit, installed
//! into one folder of the application's own: the published packages are read and unpacked here (no
//! Windows installer runs, nothing outside the folder changes), side by side over one download
//! queue and one unpack pool (`jobs`, `fetch`), with one forward-only bar per component
//! (`progress`). Downloads resume from their segments and a finished one is reused after its hash.
pub mod cab;
pub mod cuda;
pub mod curl_http;
pub mod extract;
pub mod fetch;
pub mod http;
pub mod jobs;
pub mod lzx;
pub mod msi;
pub mod msvc;
pub mod progress;
pub mod rustc;
pub mod setup;
pub mod timing;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() }
}

/// A tool started from a program without a console opens no window of its own.
pub fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetConsoleCP() -> u32;
        }
        if unsafe { GetConsoleCP() } == 0 {
            command.creation_flags(0x08000000);
        }
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// `text` with every verbatim path prefix removed: Windows canonical paths carry `\\?\`, which
/// Rust and Cargo take and cl.exe does not.
pub fn plain_paths(text: &str) -> String {
    if !text.contains(r"\\?\") {
        return text.to_owned();
    }
    text.replace(r"\\?\UNC\", r"\\").replace(r"\\?\", "")
}

/// Removes what says a step finished; one that isn't there is what was asked for.
pub(crate) fn remove_marker(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(format!("remove {}: {error}", path.display())),
        _ => Ok(()),
    }
}

/// Deletes what an install left, only strictly inside `root`. The last component of `path` is not
/// followed, so a link is removed, never what it points to. A path that does not exist is gone.
pub fn remove_inside(root: &Path, path: &Path) -> Result<(), String> {
    let Ok(root) = fs::canonicalize(root) else { return Ok(()) };
    let refuse = || format!("Refusing to delete {}: it is not inside {}", path.display(), root.display());
    let name = path.file_name().ok_or_else(refuse)?;
    let Ok(parent) = fs::canonicalize(path.parent().ok_or_else(refuse)?) else { return Ok(()) };
    let target: PathBuf = parent.join(name);
    if target == root || !target.starts_with(&root) {
        return Err(refuse());
    }
    let result = match fs::symlink_metadata(&target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => Err(error),
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(&target),
        Ok(_) => fs::remove_file(&target),
    };
    result.map_err(|error| format!("{}: {error}", target.display()))
}
