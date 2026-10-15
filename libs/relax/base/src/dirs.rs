//! Where makepad run keeps what it writes: like cargo's build output, everything for a workspace
//! lives in `<target dir>/makepad-run` (the "run dir"), where the target dir is what `cargo metadata`
//! reports as `target_directory` (resolve::target_dir: CARGO_TARGET_DIR, then cargo config's
//! `build.target-dir`, then `<workspace root>/target`). `cargo clean` removes it with the rest of
//! the build output, and a fresh checkout is cold. There is no per-user cache.
//!
//! Inside the run dir:
//!   cache/                  the store files (code, front images, graph certificates,
//!                           the collected program image): store.rs
//!   registry/               crates.io packages downloaded for this workspace (registry.rs)
//!   startup/<pkg>.startup   the startup manifest of `makepad run --ahead`
//!   build-scripts/<hex>/, build/<crate>-<hex>/out   build script units and OUT_DIRs (as deep
//!                           as cargo's: scripts find the workspace 5 folders above OUT_DIR)
//!   serve/                  the `makepad serve` endpoint
//!
//! A git store run (the loader's `run <git url>`) has no workspace on disk: its workspace is
//! virtual (vfs.rs, `<run home>/vfs/<label>`). Its run dir is
//! `<run home>/target/<8 hex of the virtual workspace root>`, next to the loader's git stores
//! in the same per-user folder the loader installs to.
//!
//! MAKEPAD_RUN_CACHE_DIR replaces the whole run dir: for tests and measurement only. A run
//! that starts another run for the same program (a build script) hands its run dir over
//! with `makepad run --run-dir <dir>` (set_relax_dir).
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static SET: OnceLock<PathBuf> = OnceLock::new();
static DEFAULT: OnceLock<PathBuf> = OnceLock::new();

/// The run dir handed over by a parent run (`makepad run --run-dir`). Set before anything
/// asks for relax_dir().
pub fn set_relax_dir(dir: PathBuf) {
    let _ = SET.set(crate::zone_alloc::kept_clone(&dir));
}

/// The workspace: MAKEPAD_RUN_WORKSPACE (a git run's virtual folder, vfs.rs; it does not exist on disk,
/// so it cannot be the current folder), else the current folder.
pub fn workspace_dir() -> String {
    if let Some(w) = std::env::var_os("MAKEPAD_RUN_WORKSPACE").filter(|w| !w.is_empty()) {
        return w.to_string_lossy().into_owned();
    }
    std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| ".".to_string())
}

/// This process's relax dir: the handed-over one, else MAKEPAD_RUN_CACHE_DIR, else the workspace's
/// (relax_dir_for(workspace_dir())).
pub fn relax_dir() -> PathBuf {
    if let Some(d) = SET.get() {
        return d.clone();
    }
    if let Some(d) = override_dir() {
        return d;
    }
    DEFAULT.get_or_init(|| crate::zone_alloc::kept_with(|| relax_dir_for(&workspace_dir()))).clone()
}

/// MAKEPAD_RUN_CACHE_DIR when set (tests and measurement).
pub fn override_dir() -> Option<PathBuf> {
    std::env::var_os("MAKEPAD_RUN_CACHE_DIR").filter(|d| !d.is_empty()).map(PathBuf::from)
}

/// The relax dir of the workspace holding `start` (no override): `<cargo target dir>/makepad-run`; a
/// folder with no Cargo.toml at or above it: `<start>/target/makepad-run`; a virtual (git store)
/// workspace: `<relax home>/target/<8 hex>`.
pub fn relax_dir_for(start: &str) -> PathBuf {
    if crate::vfs::is_virtual(start) {
        let root = crate::vfs::outer_root(start).unwrap_or_else(|| crate::vfs::normalize_path(start));
        // FNV-1a: short (Windows' 260-character path limit; build outputs nest below it)
        let mut h: u64 = 0xcbf29ce484222325;
        for b in root.bytes() {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
        return crate::gitstore::relax_home().join("target").join(format!("{:08x}", h as u32));
    }
    let target = crate::resolve::target_dir(start).unwrap_or_else(|_| format!("{}/target", crate::vfs::normalize_path(start)));
    Path::new(&target).join("makepad-run")
}

/// The store files' folder: `<relax dir>/cache`.
pub fn cache_dir() -> PathBuf {
    relax_dir().join("cache")
}
